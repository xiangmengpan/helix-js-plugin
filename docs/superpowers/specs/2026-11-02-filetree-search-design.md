# filetree 搜索能力设计（面板内过滤 / 原生 picker / 内容命中树）

> **状态：P0–P3 已全部实现（2026-11-02）；§8 顺带项 ③ 多打开标记（§13）、② 诊断标记（§14）已补** ——本文其余部分是方案与依据，改动落地后请逐条更新状态。
>
> 落地记录见 §9（P0）、§10（P1）、§11（P2）、§12（P3，含两个偏离：持久化**默认关**、
> 状态写在 `<用户插件根>/filetree-state.json` 而不是 `[plugins.filetree]`；以及一个真 bug：
> `reveal`/`:cd` 换根后 git 状态列不再刷新）。
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
| **P0** ✅ | 面板内过滤树：`build_rows()`（实际叫 `filter_rows`/`rows_for_view`）+ query 状态机 + 头部查询行 + 可见窗口高亮 + `/` `Esc` `Backspace` 模式化 + `pre_query` 恢复 | 只动 `plugins/filetree/plugin.js` + `tests/filetree.test.js`（+ 一条集成用例） | ≈180 行 | ① 过滤后 `a`/`m`/`d`/`Enter`/`P` 全部照常；② `window_rows` / `scroll_top` 既有测试不改仍绿；③ 新增纯函数测试：`filter_rows`（祖先保留 / depth 正确 / 隐藏文件联动）、`match_name`、`highlight_segments`；④ 集成用例 `filetree_search_filter_open_and_escape` |
| **P1** ✅ | `:filetree-find`（原生 picker：`rg --files` + preview + action 里 `reveal` 同步面板）；find 模式 `Tab` 切「全库」flat 命中（`rg --files` + `helix.fuzzy` 会话，按 query 缓存） | 同文件追加 | ≈150 行 | ✅ 全库找文件能跳且面板同步；`set_root`/`R` 后列表跟着变；`drop_flat()` 在 `close_panel`/`set_root`/`onUnload` 都调了 `helix.fuzzy.drop` |
| **P2** ✅ | grep 内容树：命中文件为父节点、命中行为子节点、`Enter` 跳行、`←/→` 折叠文件组；`:filetree-search <q>` 命令化 | 同文件追加 | ≈190 行 | ✅ `rg --json` 解析（行号 0 起始换算）+ 上限封顶；`rg` 缺失/参数错在查询行显示失败原因；搜索历史（`Up` 回溯）**未做**——回车即入输入，`Up` 已归导航，留 P3 另找键 |
| **P3** ✅ | 过滤 DSL（`>` 输入：glob / `#M`·`#A`·`#D`·`#?` / 子串 / `!` 排除，Enter 应用）+ git 状态列 + 会话持久化（可关） | 配置 schema 4 键 | ≈260 行 | ✅ 配置项都有 `doc`；`:config-reload` 生效（`CFG` 每次加载重读）；诊断过滤**未做**（见 §12.5） |

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

- **git 状态列**：✅ 已做（P3，见 §12；实际命令带 `-C <root>` 与 `-z`）
- **诊断点标记**：✅ 已做（§14：诊断列 `E/W/i/h` + `#E`/`#W`/`#I`/`#H` 谓词；**但数据面有核心缺口**）
- **多打开标记**：✅ 已做（§13：`●` 当前 / `·` 已打开非当前）
- **折叠状态持久化**：⚠️ 已做但换了载体（`<用户插件根>/filetree-state.json`，默认关；见 §12.2）
- **双实例 / 双 root**：❌ 未做（仍是全局单例，见 §7.2）

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

---

## 9. 落地记录（P0，2026-11-02）

改动文件：

| 文件 | 改动 |
|---|---|
| `plugins/filetree/plugin.js` | +纯逻辑（`match_name` / `node_match` / `rel_display` / `filter_rows` / `highlight_segments`）· `rows_for_view()` 成为唯一取行入口 · `S.mode/query/pre_query/eager_done` · `enter_find` / `exit_find` / `refresh_find` / `find_key` · 渲染加查询行与命中高亮 · `close_panel`/`set_root`/`reload_tree` 清理搜索态 · `read_dir_sync(dir, quiet)` |
| `plugins/filetree/tests/filetree.test.js` | 修桩（补 `path_join`/`path_name`/`path_parent`）+ 7 个新用例 |
| `helix-term/tests/test/filetree.rs` | 新集成用例 `filetree_search_filter_open_and_escape` + `panel_body()` 辅助 |

### 与本文的三处差异（都是落地时发现本文不够）

1. **多了「有预算的补加载」**（`eager_load_once`，`EAGER_DIRS = 400` / `EAGER_ENTRIES = 20000`）。
   本文原方案是"过滤只看已加载节点"，实测体验是"按 `/` 搜 `handler` 什么都没搜到"——
   **"搜不到"比"慢几十毫秒"更糟**。所以进 find 时一次性按预算递归 `ensure_children`（`seen` 防符号链接成环、
   预算防超大树、失败静默不刷屏）。代价：首次 `/` 有一次有界 IO（本机 ~400 目录量级，个位数毫秒）；
   预算外的深层目录仍然搜不到 —— 那正是 P1 的 `rg --files` 要解决的。
2. **`refresh_find()` 的落点规则**：query 变化后光标落在**首个真命中**（`r.hit` 为真），
   而不是"被保留的祖先行"。否则输入 `main` + `Enter` 会变成"钻进 `src/`"，用户还得再按一次。
   副作用：逐字输入时被保留祖先行上的光标会被抢走，这符合"输完直接 Enter"的预期。
3. **`panel_body()` 辅助**：集成测试里 `panel_text()` 会把底部状态栏拼进去，而状态栏含当前文件路径 ⇒
   "断言某文件被滤掉"会误伤（第一版就是这么红的）。负向断言一律用只取带边框行的 `panel_body()`。

### 顺带修掉的一个既有红灯

P0 开工时 `node --test plugins/filetree/tests/filetree.test.js` **整份是红的**：
`IS_WINDOWS = helix.path_join("a", "b")`（`plugin.js:118` 一带）是**加载期**依赖，
而测试桩没实现 `path_join` ⇒ `require` 直接抛 `TypeError`，后续用例一个都跑不到。
已补桩。教训与第 5 节测试口径一致：**这类纯逻辑测试没人复跑就会悄悄烂掉**。

### 验证（本次实际执行）

| 命令 | 结果 |
|---|---|
| `node --test plugins/filetree/tests/filetree.test.js` | 10 passed |
| `cargo test -p helix-term --features integration --test integration filetree` | 12 passed（含新用例） |
| `tsc --noEmit -p plugins/jsconfig.json` | 干净 |
| `node plugins/tools/check_api_surface.mjs` | 全绿（含 `runtime/plugins` 镜像一致） |
| `node plugins/tools/check_capability_surface.mjs` · `check_doc_surface.mjs` | 全绿 |
| `node plugins/tools/check_signatures.mjs` | 全绿 |
| `cargo fmt -p helix-term -- --check` | 干净 |

### 已知天花板（P1/P2 待补）

- 只搜**文件/目录名**（+ 含 `/` 时的相对路径），没有内容搜索。
- 手写输入不支持**粘贴 / 中文 IME**（与 `prompt()` 同一套按键拼接）。
- `S` 仍是全局单例（§7.2），两个面板实例会互相干扰。
- 过滤视图下 `l`/`h`/`o` 不进 find 分支（`find_key` 只给了 `Enter` 打开语义）；要加"过滤视图里也能展开"再议。

---

## 10. 落地记录（P1，2026-11-02）

### 10.1 两条入口

| 入口 | 行为 |
|---|---|
| find 模式按 `Tab` | 范围在 **当前树 / 全库** 之间切。全库 = `rg --files` 列表 + `helix.fuzzy`（nucleo 打分）；行是**扁平相对路径**（`src/main.js`），不是树 —— 同名文件靠路径区分 |
| `:filetree-find` | 原生 picker（核心 UI：nucleo + 预览 + 核心键位）。选中 → `pick_open()`：打开文件；**面板开着时**顺手 `reveal()` 同步定位 |

### 10.2 几个实现决定（都有依据，不是口味）

1. **`rg --files` 而不是 `read_tree`/`glob_async`**：后者是裸 `fs::read_dir` 递归（`shell.rs:754`/`:779`），
   **不读 .gitignore** —— 在本仓库会把 `target/` 几十万个文件全列出来。`rg` 才是"尊重 ignore 的列表"。
   用 **argv 形式** `helix.run_async(["rg", "--files", "--", root])`：不过 shell ⇒ 空格/引号/中文/Windows 盘符都安全。
   `rg` 不存在或参数错时**降级 `read_tree(depth: 12)`** 并在状态栏说明"不忽略 .gitignore"。
2. **`rg` 无匹配 = exit 1 是常规退出**：映射成空列表，不能当错误（`examples/picker.js` 已踩过）。
3. **nucleo 会话 + 按 query 缓存**：`rows_for_view()` 一次按键会被调好几次，而 `fuzzy.run` 是跨界调用
   （~2ms/5k 条）⇒ `S.flat.cache_key/cache_rows` 缓存"这个 query 的结果"；会话只在条目集变化时重建。
   释放点：`close_panel` / `set_root` / `reload_tree` / `plugin.onUnload` 全部走 `drop_flat()`。
4. **扁平行的 `node.name` 放相对路径**：全库视图若只显示 basename，`index.js` 会有一屏同名。
   合成节点 `{name: rel, path: abs, is_dir: false}` ⇒ `node_at`/打开/删除照旧按 `path` 走，
   只有 `context_dir()`（新建文件的落点）会退化到 `S.root` —— 已知且可接受。
5. **高亮仍自己算**：nucleo 不给匹配位置（`docs/api/fuzzy.md:47`），只对 `<=FLAT_LIMIT(200)` 行算子序列。

### 10.3 两个测试坑（都是这次实测出来的）

- **`pump(&mut app, "")` 会卡约 10 分钟**：空按键列表下 `event_loop_until_idle` 收不到空闲信号。
  本次实测 580s（第一次跑到 876s）。picker 的行数据是**跟 `OpenPicker` 请求一起来的**，
  层出现时候选就绪 ⇒ 根本不需要再泵一轮，直接用。
- **断言里比 CJK 要先去空白**：渲染行的宽字符占两列，`全库` 在 `render_rows` 里是 `全 库` ⇒
  `text.contains("全库")` 必红。压掉空白再比（`chars().filter(|c| !c.is_whitespace())`）。

### 10.4 验证（本次实际执行）

| 命令 | 结果 |
|---|---|
| `node --test plugins/filetree/tests/filetree.test.js` | 13 passed（+`flat_items`、全库行序/缓存/`drop`） |
| `cargo test -p helix-term --features integration --test integration filetree` | 14 passed（+`filetree_search_all_scope_uses_rg_files`、`filetree_find_command_opens_from_picker`），35.8s |
| `tsc --noEmit -p plugins/jsconfig.json` | 干净 |
| `check_api_surface` · `check_capability_surface` · `check_doc_surface` · `check_signatures` | 全绿 |
| `cargo fmt -p helix-term -- --check` · `cargo xtask sync-runtime` | 干净 / 已同步 |

### 10.5 剩下的天花板（P2/P3）

- 没有**内容搜索**（grep）：P2。
- 手写输入仍不支持粘贴 / 中文 IME。
- 全库列表是**一次性快照**：外部改动要关面板重开或 `R` 才反映（不做 `watch`，见 §4.4）。
- `S` 仍是全局单例（§7.2）。
- `find_key` 里 `Tab` 已被"切全库"占用；将来若要"在过滤视图里也能展开/钻取"，得另找键。

---

## 11. 落地记录（P2，2026-11-02）

### 11.1 入口与键位

| 入口 | 行为 |
|---|---|
| `G`（树模式） | 进内容搜索（空 query），逐字增量触发 `rg` |
| `:filetree-search <词>` | 同上但带初始词（`helix.command_args().join(" ")`）；面板没开时先开（要显示结果树） |
| 结果树里的 `Enter` | 打开文件并跳到**命中行**（`open_file({row})`，0 起始） |
| 结果树里的 `←` / `→` | 折叠 / 展开当前**文件组** |
| `Esc` | 退出搜索、回到进搜索前的节点（与 find 模式同一套 `exit_search()`） |

行的形状与树/过滤视图完全一致（文件行 `depth 0`、命中行 `depth 1` 且 `node.line` 有值），
所以 `node_at` / 渲染 / 打开**一行没改**就成立了 —— 这是 §4.1"一个函数顶住全部"的第二次回报。

### 11.2 实现决定

1. **`rg --json` 而不是 `-n --no-heading`**：后者的 `path:line:text` 在 Windows 上会被
   `split(":")` 切错（`C:\repo\a.rs:12:x`），`plugins/examples/picker.js:40` 就是这个写法。
   `--json` 每行一个事件，只认 `type: "match"`，`line_number` 是 1 起始 ⇒ 减 1 对齐 `open_file({row})`。
   非 UTF-8 路径只有 `bytes` 形式 ⇒ **跳过**（宁缺不编）。
2. **`-F` 字面量**：内容搜索不做正则（写 `(` 中途不会跳成"正则无效"）。要做正则/DSL 留给 P3 的 `>` 前缀。
3. **序号门代替防抖**：插件侧没有定时器（`helix.set_timeout` 不存在），无法 debounce ⇒
   每次按键都发一次 `rg`，用 `g.seq++` 只接受**最新**那次的结果（旧结果到达即丢，不会回跳）。
   代价是连敲时多起几个进程（`rg` 在自己的 worker 线程里，主线程不等）。
4. **三道上限**：`rg -m 20`（单文件）× 500 文件 × 2000 处，避免"一个 a 命中整个仓库"把 JS 侧撑爆。
5. **折叠状态挂在 group 上**（`grp.expanded`），行是每帧现算的 ⇒ 折叠不用重建树、不动 `S`。

### 11.3 顺手修掉一个 P0 遗留 bug

P0 的 `find_key` 把 `j`/`k`/`?` 当导航键先行 `match` 掉了 ⇒ **搜不到名字里带 j/k 的文件**
（输入 `j` 变成"下移一行"，过滤不生效）。P2 把"字符键全归输入"定为铁律：
搜索态只保留方向键/`Home`/`End`/`PageUp`/`PageDown` 导航，其余单字符一律进 query。
回归用例：`filetree_find_takes_letter_keys_as_input`（输入 `j` 必须真的过滤）。

### 11.4 验证（本次实际执行）

| 命令 | 结果 |
|---|---|
| `node --test plugins/filetree/tests/filetree.test.js` | 17 passed（+`literal_hits`、`grep_groups`×2、`grep_rows`） |
| `cargo test -p helix-term --features integration --test integration filetree` | 16 passed（+grep 跳行、字符键回归），43.6s |
| `tsc --noEmit -p plugins/jsconfig.json` · `cargo fmt --check` | 干净 |
| `check_api_surface` · `check_capability_surface` · `check_doc_surface` · `check_signatures` | 全绿 |
| `cargo xtask sync-runtime` | 已同步 |

### 11.5 剩下的（P3）

- 正则 / 过滤 DSL（`>` 前缀）；**搜索历史（`Up` 回溯）**（`Up` 已是导航，得另找键或做成"空 query 时 `Up` 回溯"）。
- 按 git 状态 / 诊断过滤；折叠状态与过滤表达式持久化到 `[plugins.filetree]`。
- `S` 仍是全局单例（§7.2）。
- 内容搜索不做多行/上下文（`-A/-B`），也不显示列号。

---

## 12. 落地记录（P3，2026-11-02）

### 12.1 三块东西

| 块 | 入口 | 行为 |
|---|---|---|
| **过滤 DSL** | `>` 键（树模式） | 进输入条，**实时预览**；`Enter` = 应用（留在树模式，头部显示 `[表达式]`）；`Esc` = 放弃草稿；删空 + `Enter` = 清除 |
| **git 状态列** | `[plugins.filetree] git_status = true` | 行尾一格 `M/A/D/R/?`；并启用 `#` 谓词。读一次 `git -C <root> status --porcelain -z --untracked-files=all` |
| **会话持久化** | `[plugins.filetree] persist = true` | 记住"上次的过滤表达式 + 展开的目录"（按根归属），下次打开同一根时恢复 |

表达式语法（`compile_filter`）：

| 词 | 含义 |
|---|---|
| `*.rs` `test_?.js` | glob，**不含 `/` ⇒ 比 basename**（`src/a.rs` 也命中 `*.rs`） |
| `src/**` `src/**/a.rs` | glob，**含 `/` ⇒ 比相对路径**；`**/` 含零层（`src/a.rs` 命中 `src/**/a.rs`） |
| `#M` `#A` `#D` `#R` `#?` | git 状态谓词（`#?` = 未跟踪）。**没有 git 表时一律不命中**（不静默放行） |
| `src/` | 无通配符 ⇒ 相对路径上的子串（大小写不敏感） |
| `!xxx` | 排除（可加在任一形态前）；空格分隔的多个词是 **AND** |

### 12.2 与本文的两处偏离（都是刻意的）

1. **持久化默认 `false`**（本文写的是"过滤表达式与折叠状态存进 `[plugins.filetree]`"）：
   - 插件**没有写 `config.toml` 的 API**（`define_config`/`get_config` 只读），所以状态落在
     `<用户插件根>/filetree-state.json`（`plugins_dir()` + `write_plugin_file`）。
   - 写盘是副作用 ⇒ 默认关，用户显式开 `persist = true` 才写。测试也因此不会污染开发机的配置目录。
   - 位置刻意**不是** `<用户插件根>/filetree/`：那会造出一个用户层插件目录，让"目录级覆盖"看起来像
     已经覆盖了内置 filetree（虽然 `resolve_in` 是按 `filetree/plugin.js` 是否存在判定的，不该赌）。
2. **诊断过滤没做**：`helix.diagnostics()` 只给文件级聚合计数，拿不到"哪个文件有 error"的逐文件映射，
   做不出可靠的谓词。等 API 有了逐文件诊断再做（写进 §12.5）。

### 12.3 顺手修掉一个真 bug（集成测试逼出来的）

`git_status` 原来只在 `open_panel` 里读一次。而 `:filetree-reveal` / `:cd` 都会走 `set_root()` **换根**，
于是"面板先按 cwd 打开、再 reveal 到别的目录"这一条最常用的路径上，git 状态列**永远是空的**。
现在 `set_root()` 里清表并重读（`if (CFG.git_status) refresh_git()`），`open_panel` 只在没有表时补读；
`refresh_git` 的异步结果还会比对 `root === base_dir()`，换过根就丢弃（否则旧根的状态会标到新树上）。

### 12.4 验证（本次实际执行）

| 命令 | 结果 |
|---|---|
| `node --test plugins/filetree/tests/filetree.test.js` | 22 passed（+`glob_to_regex`、`compile_filter`、`dsl_hit`、`match_tree`、`parse_git_status`） |
| `cargo test -p helix-term --features integration --test integration filetree` | 18 passed（+DSL 应用/清除、git 列与 `#` 谓词，真 `git init` 仓库；没装 git 则跳过），50.5s |
| `tsc --noEmit -p plugins/jsconfig.json` · `cargo fmt --check` | 干净 |
| `check_api_surface` · `check_capability_surface` · `check_doc_surface` · `check_signatures` | 全绿 |
| `cargo xtask sync-runtime` | 已同步 |

重构提示：`filter_rows`（名字搜索）现在是 `match_tree` 的特例 —— 两者共用同一条"祖先保留"遍历，
既有 P0 用例覆盖它，重构后仍全绿。

### 12.5 明确没做的（以及为什么）

| 没做 | 原因 |
|---|---|
| 逐文件诊断谓词（`#E`/`#W`） | 本阶段没做。**修正 §12.5 的原判断**：`helix.diagnostics()` 确实只给聚合计数，但 `helix.on("lsp-diagnostics", (docId, diags, payload))` 能拿到逐文档诊断 ⇒ 维护 `docId → path`（对 `helix.buffer.list()`）就能做标记；局限是**只有已打开的文档**有诊断，workspace 级错误不在此列 |
| 搜索历史（`Up` 回溯） | 搜索态里 `Up` 已是导航；再做要引入"空 query 时 `Up` 回溯"这种隐藏语义，收益不抵复杂度 |
| 正则 DSL（`/re/`） | `rg -F` 是刻意的默认；正则要另设转义/错误展示，留待有需求 |
| 多根同时持久化 | 只存"最后一个根"的状态（`state.root` 不匹配就忽略）；多根要设计"哪些根值得记"的策略 |
| `S` 全局单例 | 老问题（§7.2），与本阶段无关，但每加一种模式都更容易踩到 |

---

## 13. 补做：多打开标记（§8 顺带项 ③，2026-11-02）

行尾右缘两档标记（与 git 状态列 `w-4`、圆点 `w-2` 分列，不互相抢格）：

| 标记 | 含义 | 样式 |
|---|---|---|
| `●` | **当前**文件 | `ui.text.focus` |
| `·` | 已打开但非当前 | `ui.virtual`（选中行上用 focus 保证可见） |
| 无 | 未打开 | — |

内容命中行（grep 的 `depth 1`）不画标记：它们与文件行共用一个 `path`，否则每个命中行都会冒一个点。

### 13.1 维护"已打开集合"时踩到的两个核心缺口

1. **`:buffer-close` 不发任何插件事件**（`helix-term/src/commands/typed.rs` 的
   `buffer_close_by_ids_impl` 里既没有 `emit_plugin_event` 也没有 `sync_layout_cache`；
   `buffer-close` 事件只在 `:quit`/`:force-quit` 发）。所以"只靠事件维护集合"会留下僵尸 `·`（实测）。
   现在权威来源是 `helix.buffer.list()` **重新播种**，触发点：面板打开 / **面板上任何一次按键** /
   `R`。代价是每次按键一次 `buffer.list()`（文档数级别，微秒量级）。
2. **`S.current` 只在 `buffer-open` 更新**，而 `buffer-open` 也只有 `:open`/`:o` 路径会发
   （file picker 打开文件走 `ui/mod.rs` 的 `editor.open`，不经此处）。于是"关掉当前 buffer 后换到
   另一个文档"时 `S.current` 是陈旧的 —— 以前会给已关闭的文件继续画 `●`。
   现在 `opened_marker()` 用 `opened` 兜一层：**不在 opened 集合里就一个标记都不画**。

> 这两条都是"核心事件面不完整"导致的；将来核心若给 `:buffer-close` 补事件、或提供
> "当前文档路径"的读取 API，这一节可以退回成纯事件驱动。

### 13.2 验证

| 命令 | 结果 |
|---|---|
| `node --test plugins/filetree/tests/filetree.test.js` | 23 passed（+`opened_marker` 五种输入） |
| `cargo test -p helix-term --features integration --test integration filetree` | 19 passed（+`filetree_marks_open_buffers`：开两个文件 → `●`/`·` → 关当前 buffer → 标记消失），57.9s |
| `tsc` · `cargo fmt --check` · 4 个 check 脚本 · `cargo xtask sync-runtime` | 全绿 / 已同步 |

---

## 14. 补做：诊断标记 + `#E`/`#W` 谓词（§8 顺带项 ②，2026-11-02）

### 14.1 数据面：核心缺口已补（2026-11-03）

> **补之前的事实**：`helix-js` 的 `emit_lsp_diagnostics()` 全仓**没有任何调用点** ⇒
> `helix.on("lsp-diagnostics", …)` 这个事件永远不会触发（API 面有，发送方没有）。

**现在已接上**（三处小改 + 一个 mock 场景）：

| 位置 | 改动 |
|---|---|
| `helix-view/src/handlers/lsp.rs` | 不动 —— `DiagnosticsDidChange` 本来就在 `Editor::handle_lsp_diagnostics` 里派发，是 **push / pull 两条 LSP 路径的唯一汇聚点** |
| `helix-term/src/handlers/diagnostics.rs` | 新增 hook：`DiagnosticsDidChange → helix_js::diagnostics::queue_diagnostics(doc)`；新增 `doc_diagnostics_json(editor, doc_id)`（逐文档序列化，形状与原来一致）与 `doc_id_by_u64` |
| `helix-js/src/diagnostics.rs` | 新增 `queue_diagnostics / take_pending_diagnostics / has_diagnostics_listener`（thread_local 去重队列） |
| `helix-term/src/application.rs` | `render` 里排空队列 → `emit_lsp_diagnostics(doc_id, json)`；没人监听就不序列化 |
| `helix-term/src/commands/typed.rs` | `serialize_diagnostics` 改为转发到 `doc_diagnostics_json`（单一来源） |
| `helix-term/src/bin/mock_lsp.rs` | 新场景 `diagnostics_push`：`didOpen` 后主动推一条 `publishDiagnostics`（severity 1 = error） |

**为什么排队而不是当场发**：`helix-js` 的 `with_engine` 在"已在 JS 调用树内"时会 **panic**；
而诊断变更点在 `helix-view`（那里也拿不到 JS 宿主）。排到 `render` 里发，与 `cursor-move` 事件同一类边界。

两条数据通道现在是这样：

| 通道 | 覆盖范围 | 触发点 |
|---|---|---|
| **A. 轮询 `helix.diagnostics()`**（当下唯一可用的） | **只有当前文档**（该 API 的口径就是"当前文档"；配 `helix.buffer.current()` + `buffer.list()` 才知道属于哪个文件） | 面板每次按键 / `buffer-open` / 打开面板 / `R`（都是人-rate，不是每帧） |
| **B. `lsp-diagnostics` 事件**（✅ 已接上，§14.1） | **逐文档**（含后台文件，push 与 pull 两条路径都覆盖） | 诊断变更 → 队列 → `render` 排空 |

数据面落地后，插件侧刚才的两条限制**只剩下**"插件在标记上是人-rate 刷新"这一条：
`poll_diagnostics()` 现在退化成兜底（事件没来/没监听时仍能标当前文档），逐文档标记由事件驱动。

### 14.2 插件侧

- 状态：`S.diag = Map<绝对路径, {severity, count}>`（只存**最严重**那一级）。
- 渲染：右缘 `w-6` 一格，`E`/`W`/`i`/`h`，scope 依次 `error`/`warning`/`ui.virtual`；
  与 `w-4` 的 git 列、`w-2` 的打开标记分列，互不抢格（32 列面板下三列都在右缘）。
- 谓词：`#E` 只认 error；`#W` = warning **或更严重**；`#I`；`#H` = 任意级别；
  `!#E` 取反；**没有诊断信息的节点一律不命中**（不静默放行，与 `#M` 对 git 表缺失的处理一致）。
- `compile_filter` 现在按字母分派：`E/W/I/H` → 诊断谓词，`M/A/D/R/?` → git 谓词。

### 14.3 验证

| 命令 | 结果 |
|---|---|
| `node --test plugins/filetree/tests/filetree.test.js` | 27 passed（+`worst_severity`、`diag_hit`、`diag_mark`、`#E/#W` 编译与过滤） |
| `cargo test -p helix-term --features integration --test integration filetree` | 19 passed（补核心接线后复跑），60.8s |
| `cargo test -p helix-term --features integration --test integration plugin_lsp` | 17 passed（含**新增** `plugin_lsp_diagnostics_event_fires`：mock 推一条 error → 插件收到 `diag:1:error`） |
| `cargo test -p helix-js diagnostics` | 3 passed |
| `tsc` · 4 个 check 脚本 · `cargo xtask sync-runtime` | 全绿 / 已同步 |

### 14.4 已知限制（都写进了插件文件头）

- 诊断数据的**新鲜度**：事件只在诊断真变化时推；打开面板时不会为所有文件"补一遍"
  （LSP 本来也只为已打开/推送过的文档报告诊断）。所以后台文件的标记可能在本次会话里是"上次推送时"的状态。
- 每帧排空队列只在**有人监听**时才序列化（`has_diagnostics_listener`），没插件监听时零成本。
- `severity` 必须按**字符串**处理（数字会被忽略）—— 单测专门钉了这条（核心序列化给的是字符串）。
