# Helix × AI:现状盘点与缺口清单

> **日期**:2026-10-10 · **性质**:评估草稿(**未定方向**,不是设计规格,不改变任何行为)
> **依据**:对当前工作区做**只读**走查;每条结论附 `文件:行` 证据。
> **验证等级**:✅ = 读过**实现**确认 · 📄 = 仅文档/类型声明 · ❓ = 未验证
> **验证方法**(2026-10-10):先读 `helix-js/src/lib.rs` 里 127 个 `NativeFunction` 注册名当**权威 API 面**(不拿 `helix.d.ts` 当定论)→ 再按符号名读实现 → 每个否定结论用**多模式交叉 grep** 复核(grep 空结果不等于不存在,本文已两次踩到)。
> ⚠️ 核查中发现 **2 处文档与实现不符**(见 §6)——纯文档陈述不可当依据;**这 2 处已于 2026-10-10 修正**。

---

## 0. 三句话结论

1. 这个 fork(boa JS 插件系统 `helix-js`)已经把 AI 集成的**外围**做完了:流式进程、流式重绘、富 UI 容器、上下文读取、编辑落回、配置、插件分发。
2. 真正的缺口只有三个:**(a) 结构化"变更"的表示与预览/原子落回 (b) 原生出网通道与流式/取消语义 (c) 密钥与会话持久化**。
3. 上游 helix 与本仓库**目前都没有 AI 代码**(✅ 全仓 grep `openai|anthropic|llm|chat_completion` 仅命中 `runtime/grammars/` 下第三方文档)。所以这是**从零集成**,不是改造。

---

## 1. 已具备、可直接复用的能力(别再造)

| # | 能力 | 现成 API | 证据 | 备注 |
|---|---|---|---|---|
| 1 | 流式进程输出 | `helix.spawn({cmd,onChunk,onExit,pty?})` | ✅ `helix-js/src/shell.rs:330-392`;stdout/stderr 各一读线程逐块发 `Chunk`(`shell.rs:74-166`) | 非 pty 也流式,无聚合 |
| 2 | 杀进程 | `helix.term_kill(id)` | ✅ `shell.rs:441-450`:spawn 的 id 与终端共用 `TERM_WORKERS`,`term_kill` 移除并投 `TermCtrl::Kill` | **"spawn 杀不掉"是错的**;见 §2 P0-3 |
| 3 | 流式上屏 | worker 事件 → `set_term_wake(request_redraw)` | ✅ `helix-term/src/application.rs:180-182`;同模式先例 `helix-js/src/server_tasks.rs:8` | 打字机效果**不需要**新机制 |
| 4 | 可交互终端视图 | `open_terminal` / `term_write` / `term_kill` / `term_resize` | ✅ `plugins/helix.d.ts:333-343` | 可直接托一个 CLI 会话 |
| 5 | 浮窗/面板/布局树 | `open_popup` / `open_panel` / `split` / `pane.float` | ✅ `helix.d.ts:313-317`, `helix.d.ts:389`(`pane.float`) | 聊天面板可做**真布局叶子** |
| 6 | 富文本行渲染 | `StyledText[]`(一行多 span) | ✅ `helix.d.ts:73-74` | 代码块着色/角色配色可行 |
| 7 | 上下文读取 | `doc.text`/`selection`/`cursor` · `diagnostics()` · `lsp.hover\|document_symbols\|code_actions` · `glob_async`/`read_file_async`/`read_tree` | ✅ `helix.d.ts:40-54,297-310,473+` | 够喂一份"当前上下文" |
| 8 | 编辑落回 | `ctx.doc.insert\|replace\|delete` · `begin_edit/end_edit` · `by_path` · `set_selection` | ✅ `helix.d.ts:44-46,286-296`(声明)+ ✅ 实现:`helix-term/src/commands/typed.rs:4658-4659`「按目标 buffer 分组,每组一个 Transaction」⇒ 一次撤销 | 是 AI 写回的**唯一**通道 |
| 9 | 装饰 | `set_virtual_text` / `set_highlight` | ✅ `helix.d.ts:297-299` | 可在真 buffer 上画 +/- 标记 |
| 10 | 配置 | `define_config(name, schema)` | ✅ `helix-js/src/config.rs:3,12+`;从用户 `config.toml` 合并校验 | 见 §2 P1-1 |
| 11 | 分发 | 插件目录约定 + 两层覆盖 + `contrib/install-plugins.sh` + arsenal 市场 | 📄 `docs/plugin-layout.md`, `docs/arsenal.md` | 装 AI 插件成本低 |
| 12 | diff 算法 | histogram diff + git `DiffHandle` | ✅ `helix-core/src/diff.rs:154`(`Algorithm::Histogram`);`helix-vcs/src/lib.rs:26-98` | ⚠️ 内置 diff 是**对 git HEAD** 的,不是"两份任意文本" |

---

## 2. 缺口清单

分级:**P0** = 不解决做不了 · **P1** = 决定体验成败 · **P2** = 打磨。

### P0-1 没有"预览 → 接受/拒绝 → 原子落回"(但**机制**已有先例)

- **现象**:JS 只有即时写操作,没有 pending changeset 对象,也没有 diff 预览 UI。
- **证据**:✅ `helix-js/src/lib.rs` 注册面里 `insert/replace/delete` 只存在于 `ctx.doc`(即时写);✅ `helix-term/src/ui/` 下无 `diff.rs`;✅ 全仓无 `:diff` 类命令(仅主题色 `commands.rs:3522-3526` 与 `:diffget`)。
- **✅ 重要修正(原稿低估了现有能力)**:内置已有 **hunk 级"回退到 git 基线"原语** —— `:reset-diff-change`(别名 `diffget`),用**一个** `Transaction::change(hunks)` 落盘、一次撤销(`helix-term/src/commands/typed.rs:2884-2911`;命令注册 `typed.rs:4173-4183`)。
  ⇒ **缺的不是"hunk 应用机制",而是 ① diff 基源只能是 git HEAD(`doc.diff_handle()` 来自 `helix-vcs`,`helix-view/src/document.rs:2035`)② 没有"接受/拒绝"界面**。
- **影响**:AI 回大 patch 时只能"整段写进去再撤销",或自造预览。流式场景下坐标还会过期(`docs/api/editing.md`:快照坐标跨 await 会过期)。
- **绕过**:用 `helix-core` 的 diff(histogram,`helix-core/src/diff.rs:154`)自己算 hunk → `set_virtual_text`/`set_highlight` 画 → 确认后逐段 `replace`。
- **若下沉核心**:把 `reset_diff_change` 的"hunk → Transaction::change"模式**参数化 diff 基源**(AI 结果 vs 当前 buffer)+ 接受/拒绝命令。这比新造概念省。

### P0-2 出网:JS 侧没有网络

- **事实**:✅ 实现确认:JS 注册面 127 个函数里**无** `fetch`/http/socket(`helix-js/src/lib.rs` 全文注册表);✅ `helix-js/src` 全目录无 HTTP 客户端(多重模式交叉 grep: `fetch|XMLHttp|reqwest|ureq|TcpStream|std::net|socket` 零命中)。文档同结论(`docs/plugin-api.md` 已知限制表)。
- **但不是死路**:✅ `spawn("curl -N …")` 能拿流式 chunk(`shell.rs:74-166`);或 `open_terminal` 直接跑 `claude -p` / `codex exec` / `ollama run`。
- **缺的本体**:原生通道 + SSE 解析 + 取消。Rust 侧已有 `ureq`(✅ `helix-term/Cargo.toml:99`),但当前只用于 server-manager 的**同步下载**(✅ `helix-term/src/commands/server_manager.rs:138-148`)——阻塞式,不适合几十秒的流式回答。
- **若下沉核心**:a) 只给 JS 一个 `helix.http.stream()`(最小面);或 b) 直接做 `helix.ai.*`(provider/模型/流式/取消)。

### P0-3 取消与超时

- ✅ `term_kill(id)` 可杀 spawn 的进程,但 **Promise 侧没有取消**:没有 AbortSignal 概念,`run_async` 只能等 `Exit`;`spawn` 无超时(`plugin-api.md` 已知限制自陈)。
- **影响**:网络卡住/模型跑飞只能"杀进程",UI 状态要插件自己收敛。

### P1-1 密钥没有落点

- ✅ 配置来自用户 `config.toml`(`helix-js/src/config.rs:3`),仓库无 keyring/secret 通道。
- **结果**:API key 只能明文配置或环境变量。
- **影响**:决定"能不能给非技术用户用"。

### P1-2 会话持久化 / 插件间无共享状态

- 📄 已知限制:"每个插件独立 IIFE 作用域;共享需通过 `helix.*` API 或外部文件"。
- **影响**:多轮历史、prompt 模板、编辑记录都得自己落盘(`write_file_async` 可用)。

### P1-3 无 `setTimeout`(节流/防抖/退避)

- ✅ 实测:注册面无 timer(`helix-js/src/lib.rs`);`setTimeout|setInterval|Timer` 在 `helix-js/src` 零命中。唯一异步源是 Promise。文档同结论。
- **影响**:chunk 合并刷新、重试退避要手写(可借 `doc-change` 的 250ms 防抖先例)。

### P1-4 权限边界没有原语

- ✅ 现状:JS 可静默 `write_file_async` / `spawn` 任意命令(`helix.d.ts:302-310`)。
- **影响**:一旦有工具调用,需要"这次写盘/跑命令要不要问用户"的机制 —— 目前只能插件自己弹窗。

### P2 其他相关已知限制

| 限制 | 对 AI 集成的含义 | 证据 |
|---|---|---|
| Scroll 容器 O(内容) 全量布局 | 长对话几千行会卡 | ✅ `helix-term/src/ui/comp_layout.rs:347-368`:`offset: None` 时按 `viewport.1 = u16::MAX` 全量布局再取末 h 行(`offset` 形态存在,`helix-js/src/lib.rs:4060`) |
| ~~面板无节点焦点~~ → **面板可开节点焦点** | 输入框放面板里聚焦**可以**,但要显式 `focusable: true` | ✅ `helix-js/src/popup.rs:329-369`;路由 `helix-term/src/ui/plugin_panel.rs:24-29`;测试 `helix-js/src/lib.rs:3556-3562`。⚠️ 文档那条已陈旧,见 §6 |
| ~~无多光标~~ → **多选区可设,多光标"编辑"未做** | 多选区改写可行;逐光标各自的编辑语义仍缺 | ✅ `helix-js/src/commands.rs:1533-1558`(数组形态)→ `CursorRequest::SetSelections` → `helix-term/src/commands/typed.rs:4638-4652`(排序 + 末位 primary);`docs/api/editing.md:78-84`。见 §6 |
| **Unix-only**(`run`/`spawn`/PTY 依赖 `sh` + libc) | Windows 不可用 | ✅ `shell.rs:372-390`(`cfg(not(unix))` 直接报错)、`shell.rs:831+`;`Command::new("sh")`(`shell.rs:86`) |
| 事件只带当前文档(其它分屏不触发) | 事件里要读别的 buffer 得走 `by_path` | ✅ 事件入口会写其它 buffer 的 `DOC_SNAPSHOTS`(`helix-js/src/state.rs:92-93`)⇒ **取得到**,只是不主动推 |

> 附:spawn **确实无超时**(✅ `helix-js/src/shell.rs` 全文无 `timeout`);但**能被 `term_kill` 杀**(见 P0-3)。

---

## 3. 三个候选路线(只列代价,不选)

| 路线 | 需新增的核心改动 | 能覆盖的形态 | 主要风险 |
|---|---|---|---|
| **A. 纯插件**(spawn CLI / curl) | **零** | 问答、选区改写、patch 预览 | 依赖外部二进制;无超时/取消语义;Windows 无 |
| **B. 核心内置 `helix.ai.*`** | 流式 HTTP + cancel + provider 配置 + 密钥 | 全部形态,体验最好 | 改动最大;必须先定 §4 六个契约;fork 维护成本 |
| **C. AI 驱动 helix**(外部 agent 操作编辑器) | 无头/常驻 + IPC | agent 多文件自主改 | ✅ 现在**完全没有**:LSP 是 stdio 客户端、DAP 是连出去的 socket(`helix-term/src/commands/dap.rs:119`)、CLI 只有少量参数(`helix-term/src/args.rs`) |

---

## 4. 无论哪条路线都必须先定的 6 个契约

1. **上下文挑选规则**(选区 / 文件 / 诊断 / git diff / 邻文件)—— 不定就会"塞太多"或"漏关键"
2. **变更表示 + 事务边界**(unified diff / 全文 / 结构化 edits)—— 不定就没有可撤销的 AI 编辑
3. **流式状态机** `idle → streaming → tool-running → awaiting-approval → cancelled/done` —— 不定就会出现"转圈但没输出"
4. **密钥与配置落点**
5. **权限粒度**(自动 / 每次问 / 白名单)
6. **失败可见性**:网络/CLI/解析失败**必须**在 UI 上说出来(与本仓 `change-discipline` 的"不给假信息"一致)

---

## 5. 待拍板(给人,不给 AI)

- 第一个形态是什么?(问答 / 选区改写 / 自主 agent)
- 走 A 还是 B —— 是否接受动核心?
- provider 选谁,是否要本地模型?
- 是否允许 AI 自动写盘 / 跑命令?
- Windows 要不要支持?(路线 A 与 C 都受此约束)
- 与另一路「Helix fork 完成度评估」的结论如何合并?

---

## 6. 核查中发现:文档已陈旧的地方

这一节独立于 AI 集成,但值得单独修 —— **旧限制表会让人以为做不了**。

| 文档处 | 文档说 | 实现是 | 证据 |
|---|---|---|---|
| `docs/plugin-api.md:203` | "**无多光标** | 选区/光标 API 只操作主光标" | **多选区可设**(数组形态,排序后末位为 primary) | ✅ `helix-js/src/commands.rs:1533-1558` + `helix-term/src/commands/typed.rs:4638-4652` |
| 同上(表内自相矛盾) | 同表说无多光标 | 但同仓 `docs/api/editing.md:84,97` 只说"无多光标**编辑**" | 两者不一致;`plugin-api.md` 那句过宽 |
| `docs/plugin-api.md` 已知限制表 | "**面板无节点焦点** | 面板 render 固定收到 `focus = null`" | `open_panel({focusable: true})` 后**有**节点焦点路由与 focusables 收集 | ✅ `helix-js/src/popup.rs:329-369` · `helix-term/src/ui/plugin_panel.rs:24-29` · 测试 `helix-js/src/lib.rs:3556-3562`(相关规格 `docs/superpowers/specs/2026-08-30-js-panel-focus-design.md`) |
| `docs/plugin-api.md` 已知限制表 | "spawn 无超时" | **成立** ✓(全文无 timeout) | ✅ `helix-js/src/shell.rs` 无 `timeout` |
| `docs/plugin-api.md` 已知限制表 | "Scroll 全量布局 O(内容)" | **成立** ✓ | ✅ `helix-term/src/ui/comp_layout.rs:347-368` |

> 教训(已计入本次方法):**类型声明/d.ts 与文档都不算依据** —— 上表前两行我只读文档就会写错。
>
> **✅ 已修(2026-10-10)**:上一节两条已改到 `docs/plugin-api.md` §4(2 行 diff);改前先跑绿基线(`cargo test -p helix-js -- --test-threads=1` ⇒ **135 passed / 0 failed**)。
> **同类的死节号引用共 4 处,也已一并修**:`editing.md:97` / `loading-commands.md:33` / `ui.md:70` 的「plugin-api §19」→「§4 已知限制」,`ui.md:116` 的「§15」→「§2 弹窗 / 组件树 / 面板 / 界面定制」(plugin-api 只有 §1–§5)。
> 其中 `docs/api/ui.md:70` **把那条陈旧的面板焦点限制又写了一遍**,已改为与 `plugin-api.md` §4 一致。
> 另: `helix-js` 测试需串行(默认并发会假红 15 个),已记入 `docs/CONTRIBUTING.md` 的 Testing 节。

---

## 7. 证据索引

```
helix-js/src/lib.rs(全文注册表 127 处)     权威 API 面:**无 fetch/http/timer**
helix-term/src/commands/typed.rs:2884-2911  reset_diff_change:hunk → 一个 Transaction(已有原子落回先例)
helix-js/src/commands.rs:1533-1558  set_selection 数组形态 → SetSelections(多选区)
helix-term/src/commands/typed.rs:4638-4652  SetSelections 消费端(排序 + 末位 primary)
helix-term/src/ui/comp_layout.rs:347-368  Scroll 全量布局(O(内容))
helix-js/src/popup.rs:329-369        open_panel focusable
helix-js/src/state.rs:92-93          DOC_SNAPSHOTS(事件里可 by_path 读其它 buffer)
helix-js/src/shell.rs:74-166        流式 spawn_worker(双读线程 → Chunk)
helix-js/src/shell.rs:330-392       js_spawn(注册 TERM_WORKERS)
helix-js/src/shell.rs:441-450       term_kill ⇒ spawn 的 id 也能被杀
helix-js/src/config.rs:3,12+        define_config / 用户 config.toml
helix-js/src/server_tasks.rs:8      worker → request_redraw 唤醒先例
helix-term/src/application.rs:180-182  set_term_wake(request_redraw),33ms
helix-term/src/commands/server_manager.rs:138-148  ureq 同步下载(唯一 HTTP)
helix-term/Cargo.toml:99            ureq = "2"
helix-core/src/diff.rs              histogram diff
helix-vcs/src/lib.rs:26-98          DiffHandle(git 侧)
helix-term/src/ui/(无 diff.rs)       结论:无任意文本 diff 预览 UI
plugins/helix.d.ts:44-46,286-343,389,473  编辑/进程/UI/LSP API 面
docs/plugin-api.md 已知限制表     权威声明 —— 但 §6 已证其中 2 条陈旧(多光标/面板焦点)
```
