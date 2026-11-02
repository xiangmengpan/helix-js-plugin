# filetree 搜索能力设计（面板内过滤 / 原生 picker / 内容命中树）

> **状态：方案，未实现**（2026-11-02）——本文是设计记录，落地时请在这里逐条更新 P0–P3 的状态。
>
> 起因：`filetree` 现在只有**导航**（展开/打开/改名/删除），没有任何"找"的手段。中等规模的仓库里
> 唯一的办法是手动 `l` 进去一层层翻，或者退回核心的 `:open <目录>`。用户诉求：**这个插件没有搜索功能**。
>
> 相关代码：`plugins/filetree/plugin.js`（686 行）· `plugins/filetree/tests/filetree.test.js`（74 行）
> 相关 API：[`../../api/fuzzy.md`](../../api/fuzzy.md) ·
> [`../../api/picker-theme.md`](../../api/picker-theme.md) ·
> [`../../api/ui.md`](../../api/ui.md) · [`../../api/process-fs.md`](../../api/process-fs.md)
>
> 行号口径：本文件里的 `plugin.js:N` 指 2026-11-02 的源文件行号，改动后可能漂移。

---

## 0. 一句话方向

> 搜索不是**一个**功能，是三种需求的分离：
> **① 在树里找名字**（面板内原地过滤，保留层级）· **② 全库跳文件**（交给核心 picker，别重写）
> · **③ 找文件内容**（`rg` 结果投影回树）。
> 三者共用**一套行视图 + 一套键位**，不各写一遍。

---

## 1. 现状：四个硬约束（设计空间由它们决定，不是偏好）

| 事实 | 出处 | 对搜索的影响 |
|---|---|---|
| 数据流是 `tree → visible_rows() → S.cursor → node_at(idx)`；打开 / 展开 / 改名 / 删除 / `context_dir()` **全部**经 `node_at` | `plugin.js:39`（`visible_rows`）· `:176`（`node_at`）· `:265` · `:584` | **只要把过滤层插在 `visible_rows` 与 cursor 之间，搜索结果天然支持全部既有操作** —— 这是改动量最小的撬点 |
| 渲染是 `HEADER_H = 2`（头 + 分隔线）+ `push_scroll` + `window_rows` / `scroll_top`，纯 `helix.draw`，无 `el` 节点 | `plugin.js:354`–`:400` | 查询行、滚动、高亮都要自己画；`window_rows` / `scroll_top` 的**签名不要动**（node 测试钉死了它们） |
| 面板是非模态的：`C-*` / `M-*` 一律 `return "ignore"` 穿透给编辑器 | `plugin.js:594` | 搜索状态下**不能用 `C-n` / `C-p` / `C-f`**，只能用字符 + `Tab` / `Enter` / `Esc` |
| 面板无节点点击命中、无鼠标、`focusable` 未开（`render` 收 `focus = null`） | `plugin.js:415`–`:429` · `docs/api/ui.md:181` | 结果导航纯键盘；不能用 `el("input")` 走"Tab 聚焦"那套 |

另外两件与搜索直接相关的事实：

- **手写输入的天花板**：现有 `prompt()`（`plugin.js:448`）自己拼字符、不支持粘贴与 IME，注释里还写着
  "`input` 组件聚焦时 `Enter` 会被吞"（`:447`）。搜索框要打的字比文件名长得多，这个天花板会被放大。
- **头部只有两行预算**：`HEADER_H = 2` 是常量，加一行查询行要让它随模式变。

---

## 2. 定位：不要重复核心已经有的能力

| 已有能力 | 出处 | 结论 |
|---|---|---|
| `:open <目录>` 打开核心 `ui::file_picker`（`ignore::WalkBuilder` **尊重 .gitignore** + nucleo + 预览 + 默认动作） | `helix-term/src/commands/typed.rs:308`、`:339` | 全库"找文件并打开"**已经解决**，filetree 不该再造一个 |
| `helix.picker.define/run` 可注册任意源（`files`/`grep`/`buffers`/`symbols` 本身就是 JS 写的） | `plugins/examples/picker.js` · `docs/api/picker-theme.md` | 想要"原生 picker 的搜索手感"，成本只有十几行（见 P1） |

所以 filetree 的**增量价值**只有两条：

1. **在树的上下文里搜**：保留层级、命中祖先路径可见、搜完直接继续 `a` / `m` / `d` / `P`（picker 做不到）；
2. **把内容命中投影回树**：目录 = 命中文件，子节点 = 命中行，`Enter` 跳行（核心没有形态等价的东西）。

---

## 3. 三个候选方案

| | **A. 面板内过滤树** | **B. 原生 picker 投影** | **C. 内容命中树（grep）** |
|---|---|---|---|
| 交互 | `/` 进过滤，逐字增量 | `:filetree-find`（或 `/` + `Tab`） | `/` 再按一次切 `grep` 模式 |
| 匹配 | 自选：路径包含 / 子序列（可见窗口内自算高亮） | 核心 nucleo（免费、带高亮、带预览） | 内层过滤同 A |
| 数据来源 | 已加载节点（懒加载树，零 IO）；可选全量 `rg --files` | `rg --files` 或 `helix.read_tree` | `run_async(["rg","-n","--json", …])` |
| 代码量 | **中（≈110 行）** | **极低（≈15 行）** | 中高（≈150 行：异步 + 解析 + 缓存） |
| 能力 | 快、原地、能继续文件操作；结构清晰 | 全库最快、有预览、零渲染代码 | 唯一能做"找内容" |
| 风险 | 懒加载 ⇒ 未展开目录里的文件搜不到（见 §4.4） | 与 `:open .` 功能重叠 | `rg` 输出解析、大仓库首屏延迟、结果失效 |
| 结论 | **主力** | **薄壳补齐全库跳转** | **第二阶段** |

> A + B 是互补而非竞争：A 解决"我知道它在附近"，B 解决"我只记得名字"。

---

## 4. 推荐形态的详细设计

### 4.1 视图层：一个函数顶住全部，且不改既有签名

```js
// 现状
const rows = visible_rows(S.tree, S.show_hidden, 0, []);

// 目标
const rows = S.mode === "tree"
  ? visible_rows(S.tree, S.show_hidden, 0, [])
  : build_rows();   // 过滤模式：返回 [{ node, depth, label?, hit? }]
```

`build_rows()` 的三种来源（已加载树过滤 / 全量 flat 命中 / grep 命中树）**统一返回同形状**，因此
`node_at`、`context_dir`、`open_selected`、`enter_dir`、`leave_dir`、`reveal` **一行都不用改**。

过滤语义（三条，都要在测试里钉住）：

1. **保留祖先**：节点命中 → 其全部祖先一起保留，`depth` 保持真实层级（缩进不塌，用户知道自己在哪）；
2. **命中目录自动展开**（只在过滤视图内生效，退出后恢复原折叠状态）；
3. **query 含 `/` 时按相对路径匹配**（`src/api`、`.rs` 这类带分隔符/后缀的输入才有意义）。

**排序：保持树序，不按 score 排。** 理由：score 排序会让每次按键后行序重排，`j`/`k` 的行进方向随机化；
而 `reload_tree`（`plugin.js:274`）与 `reveal`（`:296`）都假设"行序 = 树序"。

**不改 `visible_rows` 的签名/行为**：它是 `reveal`、`reload_tree`、`count_visible`、`open_selected` 与
node 测试的唯一公共面。过滤要么在其上加可选谓词参数，要么另起 `build_rows`（推荐后者，零回归风险）。

### 4.2 状态机与键位

```js
S.mode       = "tree" | "find" | "grep";
S.query      = "";
S.hits       = null;   // grep 模式：null = 加载中
S.pre_query  = null;   // 进模式前的光标节点路径，Esc 退出时恢复（很关键：别让用户丢了位置）
```

| 键 | `tree` | `find` | `grep` |
|---|---|---|---|
| `/` | 进 find | — | — |
| `g` | — | 切 grep | — |
| 字符 | 原键位（`a` `m` `d` `P` `H` `R` `F` `?`） | 追加 query | 追加 query |
| `Backspace` | 上级目录（现状 `:612`） | **删字符**（模式内重绑，不是全局改） | 删字符 |
| `Enter` | 打开 / 展开（现状） | 打开命中项 → 回 tree | 跳到命中行 → 回 tree |
| `Tab` / `n` | — | 下一个命中 | 下一个命中行 |
| `Esc` | 关面板（现状） | 清 query → 再 `Esc` 回 tree | 同左 |
| `?` | 帮助 | 帮助文案加搜索段（`show_help`，`:632`） | 同左 |

不变项：`key.ctrl \|\| key.alt` 一律 `return "ignore"`（非模态是硬约束），`C-\` 例外保留。

### 4.3 渲染与高亮

- `HEADER_H` 从常量改为 `2 + (S.mode !== "tree" ? 1 : 0)`；`list_top` 与 `scroll_top(cursor, total, list_h)`
  继续按参数走 —— `window_rows` / `scroll_top` 的既有 node 测试原样通过。
- 查询行：`⌕ query_` + 右端计数 `12/240`；grep 模式前缀 `rg:`；加载中显示 `…`。
- **高亮需要自己算位置**：`helix.fuzzy.run` 只回 `{index, score}`，**不返回匹配位置**
  （`docs/api/fuzzy.md:47`）。做法：只对**可见窗口**（≈20 行）在 JS 里算子序列下标，命中字符用
  `ui.text.focus` 富文本段；成本可忽略（不要对全量结果算）。
- 空结果 / 错误：`无匹配（范围：src/）`；`fs_read` 失败要显示原因（现状 `read_dir_sync` 只 echo，
  面板上是空树，用户读不出是"空目录"还是"没权限"）。
- 超大结果沿用 `:396` 的 `... N 行未显示` 提示位。

### 4.4 全量数据与性能（只在"必须跨懒加载目录"时才启用）

- **不要用 `helix.read_tree` / `helix.glob_async` 建全库索引**：`shell.rs:754`（`glob_matches`）与
  `shell.rs:779`（`tree_matches`）是裸 `fs::read_dir` 递归，**不读 .gitignore** —— 在 Rust 仓库里
  会把 `target/` 的几十万个文件全列出来。用 `helix.run_async(["rg", "--files", "-0"])`
  （argv 形式不过 shell，跨平台；尊重 .gitignore）。
- **fuzzy 会话**：`S.idx = { root, items, sid }`；query 变化只调 `helix.fuzzy.run(sid, q, { limit: 200 })`，
  **条目集变化才重建 session**；`set_root` / `reload_tree` / 关面板 / `onUnload` 时 `helix.fuzzy.drop(sid)`。
- **失效策略**：不做 `watch` 驱动的索引（`helix.watch` 文档薄、permission 归 `fs_read`，收益小于复杂度）；
  改为"面板打开时 revalidate 一次 + `R` 手动刷新"。
- **去抖**：query 长度 ≥ 2 且距上次按键 ≥ 50ms 才起异步请求，避免逐字 rg。

### 4.5 异步在面板里的正确姿势

promise resolve 会**请求重绘**，所以标准形态是：按键 → 起异步 → 回调写 `S.hits` → **下一帧** `render`
读 `S.hits`。❌ 不要在 `render` 里 `await` 或发起请求（每帧路径）；❌ 不要用 `S.hits` 之外的临时变量
跨帧保存"正在加载"状态。

### 4.6 输入框：两条路，先走便宜那条

| | 手写内联输入（**建议 P0 采用**） | `open_popup` + `el("input")` |
|---|---|---|
| 代码 | 复用 `prompt()` 的按键分支（≈20 行） | 引擎维护 value，`onChange` 直接过滤 |
| 粘贴 / 中文 IME | ❌ 不支持 | ✅ 支持 |
| `Enter` | 面板 `onKey` 直接拿到 | **待实测**：源文件注释说"被吞"（`plugin.js:447`），`docs/api/ui.md` 说单行 `Enter` 走 `input.onKey` —— 两处口径矛盾，落地前先写 5 行 demo 验一下 |
| 视觉 | 查询条就在面板头部，紧贴结果 | 弹层居中，与 rail 分离 |

建议：P0 用手写内联（与现有 `prompt` 风格一致、无未知行为），把"不支持粘贴/IME"作为已知局限写进
文件头注释；只有真的需要中文路径或粘贴时，再切 popup 方案（届时 `Enter` 归属先实测）。

### 4.7 grep 的三个坑（都已有真实出处）

1. **别用 `line.split(":")`** 解析 `rg -n --no-heading`：Windows 上 `C:\repo\a.rs:12:x` 会被切错 ——
   `plugins/examples/picker.js:40` 就是这个写法（Unix 上正确，Windows 上是 bug）。
   改用 `rg --json` + `JSON.parse`（boa 里 `JSON` 可用，见 `plugins/plugin/plugin.js:70`）。
2. **`rg` 无匹配 = exit 1**，这是常规退出不是错误，要映射成空列表（`examples/picker.js:41` 已踩过）。
3. **`rg` 未安装**：argv 形式失败会 reject，捕获后回退 `glob_async` 并**限量 5000 条**，
   同时 echo 一条"未找到 rg，已降级（不忽略 .gitignore）"。

---

## 5. 分期路线图

| 阶段 | 内容 | 改动点 | 代码量 | 验收 |
|---|---|---|---|---|
| **P0** | 面板内过滤树：`build_rows()` + query 状态机 + 头部查询行 + 可见窗口高亮 + `/` `Esc` `Backspace` 模式化 + `pre_query` 恢复 | 只动 `plugins/filetree/plugin.js` + `tests/filetree.test.js` | ≈110 行 | ① 过滤后 `a`/`m`/`d`/`Enter`/`P` 全部照常；② `window_rows` / `scroll_top` 既有测试不改仍绿；③ 新增纯函数测试：`filter_rows`（祖先保留 / depth 正确 / 隐藏文件联动）、`highlight`（子序列下标） |
| **P1** | `:filetree-find`：`helix.picker.define("filetree-find")` + preview + action 里 `reveal` 同步面板；filter 模式可选全量 flat 命中（`rg --files` + fuzzy 会话） | 同文件追加 | ≈60 行 | 全库找文件能跳且面板同步；`set_root` 后列表跟着变；`S.idx.sid` 有 `drop`（`:plugin-unload` 后无残留） |
| **P2** | grep 内容树：命中文件为父节点、命中行为子节点、`Enter` 跳行；搜索历史（`Up` 回溯）；`:filetree-search <q>` 命令化 | 同文件追加 | ≈150 行 | `rg` 缺失时降级可见；首屏 < 200ms（10k 文件）；`JSON` 解析在 Windows 路径上正确 |
| **P3（可选）** | 按 git 状态 / 诊断过滤；`>` 前缀命令行过滤（filter DSL）；过滤表达式与折叠状态存进 `[plugins.filetree]` | 配置 schema 也要动 | — | 配置项有 `doc` 文案，`:config-reload` 即时生效 |

测试口径（沿用仓库约定）：

- 纯逻辑：`node --test plugins/filetree/tests/filetree.test.js`（node ≥ 24 不接受目录参数）
- 端到端：`cargo test -p helix-term --features integration --test integration plugin`（**必须带 `--features integration`**，否则假绿）
- 类型：`tsc --noEmit -p plugins/jsconfig.json`；改完跑 `cargo xtask api-check`
- 改完插件：`:plugin-reload`（真实编辑器里验；`runtime/plugins` 的副本要 `cargo xtask sync-runtime` 才更新）

---

## 6. 不做清单（以及理由）

| 不做 | 理由 |
|---|---|
| 改 `visible_rows` / `node_at` / `context_dir` 的签名与行为 | 它们是所有文件操作与 `reveal` 的公共路径，也是 node 测试的唯一依托 |
| 自己写模糊打分 | `helix.fuzzy`（nucleo）已在进程内，横评里比手写 JS 快 3.5 倍（`docs/api/fuzzy.md` 的表） |
| score 排序 / 多选 / 鼠标 | 面板无节点命中、无鼠标；score 排序会打乱树序假设 |
| 再造一个 file finder | `:open <目录>` 已是 ignore + nucleo + 预览的完整实现 |
| `watch` 驱动的实时索引 | 文档薄，收益 < 复杂度 |
| 在 `render` 里做 IO 或 `await` | 每帧路径（`docs/api/ui.md:242` 的教训同款） |

---

## 7. 待确认（落地前必须先解决）

1. **rail 口径不一致**：`docs/plugin-api.md:155` 写"filetree 即 rail 模式"，但 `plugin.js:424` 的
   `helix.open_panel({ side: "left", size: 32, … })` **没有传 `rail: true`**。这直接影响搜索键位：
   rail 上 `Esc` / 裸 `l` 是"回主区"（`plugins/helix.d.ts` 的 `PanelOptions.rail` 注释），
   而本方案要用 `Esc` 退出搜索态、`l` 是"进入目录"。**先确认哪个是当前真相**，再决定 `Esc` 的分层。
2. **`S` 是全局单例**：`open_panel()` 开第二个面板会覆盖 `S.panel_id`，两个面板共用同一棵树/光标
   （现有 bug，搜索的分层状态会让它更明显）。是"禁掉第二个实例"还是"把状态挪进实例"，需要一并定。
3. **`Enter` 归属**（若走 popup + `el("input")` 方案）：源注释与 API 文档矛盾，需实测。

---

## 8. 顺带可加的能力（不属于搜索，但同一批改动更顺）

- **git 状态列**：`run_async(["git", "status", "--porcelain"])` 一次 + 结果挂到节点 → 行尾 `M`/`A`/`?`
- **诊断点标记**：`helix.on("lsp-diagnostics")` 事件已存在
- **多打开标记**：现在只有"当前文件"有右缘 `●`，可以再区分"已打开但非当前"
- **折叠状态持久化**：写进 `[plugins.filetree]`（P3 一起做）
- **双实例 / 双 root**：见 §7.2

---

## 附录 A：证据索引

| 断言 | 出处 |
|---|---|
| 行视图与 `node_at` 是唯一公共面 | `plugins/filetree/plugin.js:39`、`:176`、`:265`、`:584` |
| `HEADER_H = 2`、`push_scroll` + `window_rows`/`scroll_top` | `plugins/filetree/plugin.js:354`–`:400` |
| 修饰键穿透（非模态） | `plugins/filetree/plugin.js:594` |
| `Backspace` 现状 = 上级目录 | `plugins/filetree/plugin.js:612`–`:613` |
| `prompt()` 手写输入 + "`Enter` 被吞"注释 | `plugins/filetree/plugin.js:447`–`:448` |
| `focusable` 未开 ⇒ `render` 收 `focus = null` | `docs/api/ui.md:181`、`plugins/filetree/plugin.js:424` |
| rail 语义（`Esc` / 裸 `l` 回主区） | `plugins/helix.d.ts` 的 `PanelOptions.rail` |
| 核心 `file_picker`（ignore walker）由 `:open <目录>` 触发 | `helix-term/src/commands/typed.rs:308`、`:339` |
| `glob_async` / `read_tree` 不读 .gitignore | `helix-js/src/shell.rs:754`、`:779` |
| nucleo 不返回匹配位置 | `docs/api/fuzzy.md:47` |
| `split(":")` 解析 rg 的 Windows 坑 | `plugins/examples/picker.js:40` |
| `JSON.parse` 在插件里可用 | `plugins/plugin/plugin.js:70` |
