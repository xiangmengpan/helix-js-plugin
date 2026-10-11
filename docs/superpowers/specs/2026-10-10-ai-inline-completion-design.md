# AI 内联补全(Copilot 式)设计 —— 插件优先

> **状态**:设计草稿,**待评审**;形态选择仍开放(见 §0.2),本文给推荐路线与理由。
> **日期**:2026-10-10 · **不改动任何代码**
> **前置**:[`2026-10-10-helix-ai-integration-gap-inventory.md`](2026-10-10-helix-ai-integration-gap-inventory.md)(全局缺口清单)
> **验证等级**:✅ = 读过实现(附 `文件:行`)· ❓ = **未验证**(附验证方法)

---

## 0. 结论

### 0.1 一句话选型

**做成插件,走一条独立通道(内联装饰 + 常驻子进程),不要扩展现有 LSP 补全菜单。**

原生补全菜单里**没有**外部补全源的位置(`CompletionProvider` 是封闭枚举),而且那条管线是 tokio 竞速;
而"在光标处显示灰色建议"所需的**显示层已经存在**,且与 LSP inlay hints 共用同一条渲染路径 ✅ ——
所以这条路的**核心改动量是 0**。

### 0.2 本文未替你决定的一件事

形态只推荐不拍板:**内联 ghost text(Copilot 式)** vs 手动触发的候选列表。两者共用 §2.3 的传输层与 §2.4 的显示层,
差别只在 §2.2 / §2.5(触发与接受)。本文按**内联**写,手动触发是其子集(去掉 §2.2 的自动触发即可)。

---

## 1. 现状:三条既有通道

### 1.1 原生补全管线 —— **不要往里塞**(三条硬约束)

| # | 约束 | 证据 |
|---|---|---|
| 1 | `CompletionProvider` 是**封闭枚举** `Lsp(LanguageServerId) \| Path \| Word \| Snippet`,没有"外部源"的位置 ⇒ JS 没有注入候选的入口 | ✅ `helix-core/src/completion.rs:18-24` |
| 2 | 管线是 **tokio 竞速**:`JoinSet` 并发多源 + `is_incomplete` 反复重问 + `cancelable_future` 取消 | ✅ `helix-term/src/handlers/completion/request.rs:200-295` |
| 3 | JS 是 boa **单线程**,异步只靠事件泵 resolve promise ⇒ **进不了**上面那套竞速/取消语义 | ✅ `docs/plugin-api.md` §1 架构图 |
| 4 | JS 对原生菜单只有**显示钩子**:`set_completion_icon` / `set_completion_render` 只能画行,不能增删项、不能改插入文本 | ✅ `helix-js/src/popup.rs:882, 2074` |

另外交互性质不同:原生菜单是同步列表 + nucleo 过滤 + resolve(敲就弹);AI 是 200ms~2s 且流式。
混在一起,要么菜单卡住,要么补全的取消/重问语义失效。

> 附带事实:`helix-lsp-types` 里有 LSP 规范的 `InlineCompletion*` 类型,但**客户端零引用**(未注册 capability)
> ⇒ "走 LSP 内联补全"等于从零写一个客户端,蹭不到任何东西。✅ 全仓 grep 无命中。

### 1.2 内联装饰 —— **可以直接蹭**(关键发现)

- `PluginInlineAnnotation { char_idx, text, style }` ✅ `helix-view/src/document.rs:142-153`
- 渲染端把它按 style 分组注入 **`TextAnnotations::add_inline_annotations`** —— **与 LSP inlay hints 同一条路径** ✅ `helix-view/src/view.rs:507-540`
- **生效时机正确**:事件处理完 → `apply_plugin_decorations` → `request_redraw()` ✅ `helix-term/src/application.rs:470-495`
  ⇒ handler 里设的文本 **~33ms 内上屏**,不需要新机制
- **每帧整体替换**:一个批次里 `Clear` 后追加,最后 `doc.plugin_decorations = {virtual_text, highlights}` 整体赋值
  ✅ `helix-term/src/commands/typed.rs:4735-4819` ⇒ **每次回调重设一次即可,不必显式清理**
- ⚠️ 装饰坐标**不随文本重映射**(已知限制)⇒ 必须**每次变更都重推**,否则贴纸漂位

### 1.3 JS 桥 —— 触发 / 键位 / 进程都现成

| 能力 | API | 证据 |
|---|---|---|
| 停手触发(250ms idle 防抖) | `helix.on("doc-change", fn)` | ✅ 发射点 `helix-term/src/commands/typed.rs:4570` |
| 当前 doc 的文本/光标 | `doc-change` 的第一个参数 `doc`:`doc.path` / `doc.text` / `doc.cursor` | ✅ `helix-js/src/commands.rs:867-871`(`doc_to_js` → `build_doc_object(path,text,cursor,…)`)+ `:1142-1145` |
| insert 模式可绑键 | `helix.map("insert", key, fn)`(白名单含 insert) | ✅ `helix-js/src/commands.rs:741` |
| 常驻子进程 + 流式 | `spawn({cmd,onChunk,onExit})` / `term_write(id, s)` / `term_kill(id)` | ✅ `helix-js/src/shell.rs:330-392, 441-450` |
| 一次撤销的批量编辑 | `begin_edit` / `end_edit` + `ctx.doc.insert\|replace` | ✅ `helix-js/src/commands.rs:1919-1930` |

> ⚠️ **两个容易写错的取数点(已核实实现,别信类型声明)**:
> - `helix.buffer.current()` 返回的是**文档 id(数字)**,不是对象 ✅ `helix-js/src/layout.rs:786-802`;
>   `helix.d.ts` 里的 `BufferInfo{id,name,path}` 属于 `helix.buffer.list()`(`plugins/helix.d.ts:162-166`),不含文本/光标。
> - `helix.by_path(p)` 返回的 doc 里 **`cursor` 恒为 `(0,0)` 占位**(其它 buffer 的快照没有光标)✅ `helix-js/src/commands.rs:897`。

---

## 2. 设计

### 2.1 数据流

```
按键/停手 250ms
   │
   ├─ helix.on("doc-change")  ← 核心已做 idle 防抖
   │     └─ 取上下文(光标前 N 行 + 可选邻文件/诊断) → seq++
   │           └─ term_write(proc_id, JSON一行 + "\n")   ← 常驻进程 stdin
   │                 └─ onChunk(原始块!) → 行缓冲 → 解析 → 若 seq 不是最新则丢弃
   │                       └─ set_virtual_text(path,row,col,灰字,"comment")
   │                             └─ (核心 drain → ~33ms 上屏)
   └─ map("insert","Tab") → 接受:begin_edit → ctx.doc.insert(光标处,建议文本) → 清装饰 → end_edit
```

**必须点名的实现陷阱**:`onChunk` 给的是**任意边界的 UTF-8 文本块**(按 `TERM_CHUNK_SIZE` 读、只保证 UTF-8 完整)
✅ `helix-js/src/shell.rs:18-70` ⇒ **协议必须是"一行一条 JSON"并自己做行缓冲**,不能假设"一个 chunk = 一条消息"。

### 2.2 触发与防抖

- **主触发**:`doc-change`(核心 250ms idle 防抖,天然贴合 Copilot 手感)✅
- **不追加自己的 setTimeout**:boa 没有定时器 ✅ `docs/plugin-api.md` §4;要额外节流就自己记时间戳
- **清空时机**:mode 切走、光标离开锚点、命令开始(`PostCommand` 无此事件 ⇒ 用 `mode-change` + 下一次 `doc-change` 覆盖)
- **不触发**:insert 之外、脏行未结束(可选:仅在前缀匹配 `\w+\.` 等场景触发以减少请求)

### 2.3 传输层选型

| 方案 | 每请求开销 | 流式 | 复杂度 | 评价 |
|---|---|---|---|---|
| **常驻子进程 + JSON-lines(stdin/stdout)** | 极低(只写一行) | ✅ `onChunk` | 低(全走现成 API) | **推荐**:`spawn` 一次,`term_write` 喂,`term_kill` 收尾 |
| 每请求 `spawn("curl -N …")` | 高(fork + TLS 握手) | ✅ | 低 | 仅适合手动触发的低频场景 |
| 核心内 reqwest/SSE | — | ✅ | 高(改核心 + 密钥) | 见 §6,现在不值得 |
| `open_terminal` 起 CLI | — | 视觉污染 | 低 | 适合"手动问一次",不适合每键 |

常驻进程建议形态:`node scripts/ai-bridge.js`(或 `python3 …`)读 stdin 的 JSON 行 → 调 provider → 逐行回吐
`{"seq":N,"delta":"…"}`;复用一条连接,告别每次 fork。

### 2.4 显示:坐标语义(**有坑**)

- 签名:`helix.set_virtual_text(path, row, col, text, style)`;只传 `path` = 清除该 doc 全部插件装饰 ✅ `helix-js/src/commands.rs:1782-1830`
- **`col` 是 char 偏移,不是 grapheme、也不是显示列** ✅ `helix-term/src/commands/typed.rs:4611-4615`(`line_start + col`)
- 🆕 **因此不能用 JS 的 `String.length` 换算坐标** —— 那是 UTF-16 code unit:emoji 算 2,而 Rust char 算 1 ⇒ 会漂位。
  必须按**码点**数:`Array.from(line).length` 或 `[...line].length`。中文( BMP)恰好一致,容易蒙对而留下隐患 ❓(需实测补一个 emoji 用例)
- `row` 超出范围会被 clamp;`text` 建议**单行**——多行要**按行拆成多条**annotation(渲染端是行内文本)❓ 观感未验证

### 2.5 接受 / 取消

- **接受**:`map("insert","Tab", fn)` → `begin_edit()` → `ctx.doc.insert(row, col, ghost)` → 清装饰 → `end_edit()`
  ⇒ 目标:**一次撤销**(`begin_edit/end_edit` 的用途)✅,且**不离开 insert 模式**❓(需实测)
- **取消**:`map("insert", "Esc", fn)` ❓ **未验证**:Esc 在 insert 模式本身是"退出插入",
  插件 map 能否先行截住、以及截住后是否还需要手动处理模式 —— **必须实测**,这是本设计最大的交互不确定性
- **拒绝但保持插入**:建议 `Ctrl-E`(不占用 Esc),避开上述冲突

### 2.6 过期结果丢弃

JS 单线程、promise 都在主线程 resolve ⇒ **无数据竞态**,只需一个自增 `seq`:
回调里 `if (msg.seq !== latestSeq) return;`。不需要 AbortSignal(boa 也没有)。
另外给常驻进程发一条 `{"cancel":seq}` 是最优解,但**不做也不影响正确性**(只是浪费算力)。

### 2.7 上下文与预算

首版:**光标前 N 行(默认 40)+ 当前文件路径 + 语言**。可选增强:`helix.diagnostics()` 的当前行诊断(能显著提升"修 bug 式补全"命中率)。
邻文件(`by_path`/`read_file_async`)留到 M3 —— 先验证延迟,再验证质量。
截断策略写在插件侧(按**行数**截,不做 token 估算:估算必然不准,不如给一个保守行数)。

### 2.8 失败必须可见

按本仓既定纪律(不给假信息):传输失败/JSON 解析失败/provider 报错**必须**在 UI 上说出来 ——
`helix.echo()` 或状态栏,而不是静默留白。已知风险:常驻进程**无超时** ✅(`plugin-api.md` §4)
⇒ 插件侧自己做"看门狗"(记录 seq 的发送时间,下一次 doc-change 时若上一条超 N 秒未回,`term_kill` 重启)。

---

## 3. 风险与未验证项

| # | 项 | 等级 | 验证方法 |
|---|---|---|---|
| R1 | `Esc` 在 insert 模式的优先级(退插入 vs 取消建议) | ❓ 高 | 真机按 Esc,看是否退出 insert + 装饰是否清除 |
| R2 | 接受后是否仍在 insert、undo 是否恰好一次 | ❓ 高 | 接受 → `u` 一次应回到接受前 |
| R3 | emoji/非 BMP 字符的坐标漂移 | ❓ 中 | 在含 emoji 的行尾看灰字是否贴错位置 |
| R4 | 多行 ghost text 观感 | ❓ 中 | 拆多条 annotation 后目视 |
| R5 | 常驻进程挂死 | ✅ 已确认无超时 | 看门狗(§2.8) |
| R6 | 每键都请求导致算力/费用 | ❓ 低 | 触发条件收紧(§2.2)+ 可选取消 |
| R7 | 装饰与 LSP inlay hints 抢位 | ❓ 低 | 开着 inlay hint 时看重叠表现 |

---

## 4. 里程碑与验收判据

**M1 —— 无模型也能验证的骨架**(判据全部可执行,可写成集成测试)

- 用**假 provider**(固定返回 `"hello"`)跑通:停手 → 灰字出现 → Tab 接受 → `u` 一次撤销干净。
- 落点:`helix-term/tests/test/plugin_*.rs` 的 `AppBuilder + pump` 夹具(`PLUGIN_TEST_LOCK` 串行)✅ 该夹具先例见 `tests/test/plugin_terminal_hooks.rs`
- 判据:① 装饰存在 ② Tab 后 doc 文本 == 期望 ③ 一次 undo 回到原状 ④ Esc 后装饰消失(得 R1 结论)

**M2 —— 接真 provider**:常驻子进程 + JSON-lines + 行缓冲;流式增量更新灰字;
判据:① 首字延迟可观测 ② 中途改键不会覆盖新结果(seq)③ 进程崩溃时不静默(§2.8)

**M3 —— 上下文增强与打磨**:诊断入 prompt、看门狗、多行、触发条件收紧、`:ai-completion` 开关与配置(`define_config`)

---

## 5. 非目标(首版明确不做)

- ❌ 把 AI 候选塞进原生补全菜单(理由见 §1.1)
- ❌ 自动应用编辑(只提建议;**不授予自动写盘权**)—— 权限边界见缺口清单 §4
- ❌ 云端密钥管理(首版走本地进程/环境变量)
- ❌ Windows 支持(整条通道依赖 `spawn`,Unix-only ✅ `helix-js/src/shell.rs:372-390`)

---

## 6. 何时才值得动核心

| 触发条件 | 核心最小改动 | 规模预估 |
|---|---|---|
| 想让 AI 候选**进原生菜单**一起排序/过滤 | `CompletionProvider` 加 `External` 变体 + JS provider 通道 + 定义它如何参与 `JoinSet`/取消 | 最贵的一处,且要动并发语义 |
| 想让建议有**文本模型语义**(按词接受、多行、与移动/undo 正确协作) | 现在 `PluginInlineAnnotation` 是"贴纸",不参与文本模型 | 中高 |
| 想省掉子进程 | 核心内 reqwest/SSE(复用已有 `ureq` 依赖) | 中 |

**判据**:只有当 M2 跑完、确认"延迟与质量都够好,唯一瓶颈是菜单合体/文本语义"时才动核心。
在此之前动核心 = 用最贵的方式解决一个还没被证实存在的问题。

---

## 7. 证据索引

```
helix-core/src/completion.rs:18-24            CompletionProvider 封闭枚举
helix-term/src/handlers/completion/request.rs:200-295   JoinSet 竞速 + is_incomplete + cancelable_future
helix-js/src/popup.rs:882, 2074               set_completion_icon / set_completion_render(只画)
helix-view/src/document.rs:149-153             PluginInlineAnnotation{char_idx,text,style}
helix-view/src/view.rs:507-540                注入 add_inline_annotations(与 inlay hints 同路)
helix-term/src/application.rs:470-495         事件 drain:apply_plugin_decorations + request_redraw
helix-term/src/commands/typed.rs:4735-4819    apply_plugin_decorations:整批整体替换
helix-term/src/commands/typed.rs:4611-4615    pos_to_char:col = char 偏移(≠ JS UTF-16 索引)
helix-term/src/commands/typed.rs:4570         doc-change 发射点(250ms 防抖)
helix-js/src/commands.rs:867-871, 1142-1145  doc_to_js → 事件首个参数就是 doc(含 path/text/cursor)
helix-js/src/layout.rs:785-802               buffer.current() 只返回文档 id(不是对象)
helix-js/src/commands.rs:897                 by_path 的 cursor 恒为 (0,0) 占位
helix-js/src/commands.rs:741                  KEYMAP_MODES = normal/insert/select
helix-js/src/commands.rs:1919-1930            begin_edit / end_edit(批量事务)
helix-js/src/shell.rs:18-70                     onChunk 是任意边界的原始 UTF-8 块
helix-js/src/shell.rs:330-392, 441-450       spawn / term_write / term_kill
helix-js/src/shell.rs:372-390                 Unix-only
docs/plugin-api.md §4                         已知限制(无定时器/无网络/spawn 无超时/装饰不重映射)
```
