# 模式体系收敛设计（Input Layer）

> 状态：**设计稿**，待拍板
> 出发点：用户反馈"感觉是一盆散沙，优化到模式之间有关联"

---

## 1. 诊断：为什么它是散的

现在有 **4 个互不相识的状态机**，各自有定义、路由、提示、显示：

| # | 状态机 | 定义处 | 谁能改它 | 按键在哪路由 | 提示从哪来 | 状态栏怎么显示 |
|---|---|---|---|---|---|---|
| 1 | 编辑模式 `Mode`（Normal/Insert/Select） | `helix-view/src/document.rs:66` | 命令（`enter_insert_mode` 等） | `EditorView::handle_keymap_event` → `keymaps.get(mode, key)` | keymap 自身 | 左侧图标 |
| 2 | 平级模式 `PaneMode`（Locked/Pane/Resize/Move/Scroll） | `compositor.rs:24` | `set_pane_mode` | `Compositor::handle_event`（**在编辑器之前**，`compositor.rs:497`） | `pane_mode_entries()`（Rust 表） | which-key / Info |
| 3 | 终端输入模式 `TermInputMode`（Insert/Normal） | `plugin_terminal.rs:584` | 终端组件自己（`Esc`/`i`） | 终端叶子的 `handle_event` | 组件内 | 不可见 |
| 4 | 调试"暂停接管"（不是模式） | `editor.is_debug_paused()` | DAP 事件 | `EditorView::handle_keymap_event` 前置（`editor.rs:1028`） | `dap_keymap()` | 插件段 `⏸` |

四个具体病症：

1. **路由顺序散在三处**（compositor → 叶子 → 组件自身），没有任何一处能回答"这组键现在归谁"。
2. **退出语义三套**：平级模式"同一个前缀键退出 + `Esc`"、编辑/终端模式"`Esc` 退"，调试靠状态自动退。
3. **提示来源三套**（Rust `pane_mode_entries` / keymap sticky 菜单 / JS which-key 回调），优先级不成文。
4. **新增能力没有落脚点**：上一轮我们之所以把 `Debug` 平级模式**删掉**，正是因为它跟"编辑模式 + 提示表 + 只读"三者都打架——这就是"散"的代价实证。

---

## 2. 目标形态：**有序的输入层栈（Input Layer Stack）**

把"模式"统一抽象成 **InputLayer**，一个**有序栈**，**一个**路由点，**一套**提示与显示。

```rust
/// 一层 = "在某条件下，这批键归我；我可能让下层继续拿"
pub struct InputLayer {
    pub kind: LayerKind,             // 见下表
    pub keymap: KeyTrie,             // 这层的键位表（唯一来源）
    pub swallow: Swallow,            // 未绑定的键：吞掉 / 让下层继续
    pub toggle_key: Option<KeyEvent>,// "同一个键进出"（zellij 语义）的统一表达
    pub badge: &'static str,         // 状态栏徽标（"PANE" / "DEBUG"）
}
```

**层清单（现有 11 个状态如何落到层上）**

| 顺序（上→下） | 层 | 何时在栈上 | 未绑定的键 | 提示/徽标 |
|---|---|---|---|---|
| 1 | `Transient(Ask/<prompt>)` | 命令行/搜索/picker 打开时 | 吞 | 自带 |
| 2 | `Transient(DebugPaused)` | 会话暂停（`is_paused()`） | **让下层**（现在就是这样） | `DEBUG` |
| 3 | `PaneAction(PaneMode)` | `C-p/n/h/y/g` 进入 | 吞（Locked 除外：全让） | `PANE/RESIZE/MOVE/SCROLL/LOCKED` |
| 4 | `TerminalInput(TermInputMode)` | 焦点在终端叶子 | Insert 时全让给 PTY；Normal 让下层 | `TERM` |
| 5 | `Editing(Mode)` | 永远在栈底 | 按 keymap | `NORMAL/INSERT/SELECT` |

**统一的三条规则**（"有关联"就体现在这里）：

1. **路由唯一**：`Compositor::handle_event` 调 `Router::route(event)`，从栈顶往下问，第一个 `Consumed` 生效；任一层的 `swallow=Pass` 表示继续往下问。现有三处路由代码全部收敛到这里。
2. **退出唯一**：`Esc` = **弹出栈顶可弹出层**；栈里只剩 `Editing` 时（Normal/Insert/Select）才做原来的编辑语义。`toggle_key` 用于"同一个键进出"（`C-p` 再按一次退）。
3. **显示唯一**：状态栏左侧 `NORMAL│DEBUG│PANE` 串联全部活动层徽标；提示由 `Router::hints()` 生成（一层一行，which-key/内置 Info 共用），不再各自为政。

---

## 3. 三个方案（按侵入度）

### 方案 A：完整 Input Layer 栈（推荐）

- 新增 `helix-term/src/input_router.rs`：`Router { layers: Vec<InputLayer>, route(), hints() }`。
- `PaneMode` / `TermInputMode` / `DebugPaused` / `Editing(Mode)` 全部实现成层；`Mode` 作为**永驻栈底**。
- `Compositor::handle_event` → `router.route()`；`EditorView::handle_keymap_event` 里的调试前置、`compositor.rs:497` 的平级模式分支、终端的 `TermInputMode` 分支全部删除（变成层）。
- 状态栏/Info 从 `router.hints()` 取；`helix.pane_mode.*` 与 `pane-mode-change` 变成 `helix.input.layers()`（保留旧名兼容一段时间）。

| 维度 | 评价 |
|---|---|
| 收益 | 真正"一套系统"：新增能力 = 加一层（表+提示+进出命令），不再改三处；调试/终端/平级互相可组合（谁在上面谁优先，规则写在 `Router` 一处） |
| 代价 | **3–4 天**；动 `compositor.rs`/`ui/editor.rs`/`ui/plugin_terminal.rs`/`statusline.rs`，以及 `helix.pane_mode.*` 的兼容层 |
| 风险 | 中。最大风险是"编辑模式不是 compositor 的东西"（它住在 `helix-view`），要把路由改成"层协议由 helix-term 定义、`Mode` 作为只读输入" —— 用 `ModeLayer` 薄包装即可 |

### 方案 B：只抽公共骨架，不合并路由（折中）

- 抽 trait `ModeLike { name(); entries(); key(&key)->Outcome; toggle_key(); }`，让 `PaneMode`、`DebugPaused` 先实现；`Compositor` 持 `Vec<Box<dyn ModeLike>>`。
- 路由仍是"compositor 先问自己的层，再交给叶子"（顺序仍在两处，但至少**规则写成文档 + 单测**）。
- 终端与编辑模式暂不动。

| 维度 | 评价 |
|---|---|
| 收益 | **1.5–2 天**；立刻解决"提示表三套""退出语义三套"；为 A 铺路 |
| 代价/风险 | 低；但"路由唯一"没达成，长期仍要还债 |

### 方案 C：彻底 keymap 化（层 = keymap 层）

- 让 `Keymaps` 本身支持多层（`Keymaps::get(mode, event)` → 查 `active_layers` 依次匹配），`PaneMode`/`TermInputMode`/`DebugPaused` 都只是"层名"。
- 用户可用 `[keys.pane]` / `[keys.debug]` 覆盖任意层（配置即文档）。

| 维度 | 评价 |
|---|---|
| 收益 | 最"有关联"：一套解析器、一套提示、一套配置；用户可自定义层键位 |
| 代价 | **4–6 天**，改 `keymap.rs` 解析器 + `space` 前缀与层的交互定义；上游 diff 最大 |
| 风险 | 高（keymap 是热路径 + 有大量测试） |

---

## 4. 推荐：目标 A、路径 A←B

先按 B 的粒度把"层"抽出来（1.5 天，风险低、立刻见效），再一次性把路由收敛到 `Router`（A 的后半，2 天）。

**迁移步骤（每步可独立验证）**

| 步 | 内容 | 验证 |
|---|---|---|
| 1 | 定义 `InputLayer`/`Swallow`/`LayerKind` 与 `Router`（空实现，只做"层栈 + 徽标 + hints"） | 单测：栈序、徽标串联、hints 拼接 |
| 2 | `PaneMode` 变成层（表 + hints 从 `pane_mode_entries` 搬进层） | 现有 5 个平级模式的 pty 回归 + which-key 提示不变 |
| 3 | `DebugPaused` 变成层（替换 `editor.rs:1028` 的前置判断） | 暂停时 `n/s/o/c` 生效、`i` 照常插入、面板聚焦仍同义 |
| 4 | `Esc`/`toggle_key` 统一到 `Router` | 平级模式 `C-p`→`Esc`/`C-p` 都能退；调试层 `Esc` = 让出接管（不是"退出会话"） |
| 5 | `TerminalInput` 变成层（终端组件只保留 PTY 转发） | 终端里 `i`/`Esc` 行为不变 |
| 6 | 状态栏/Info 统一取 `Router::hints()`；`helix.pane_mode.*` 保留为兼容视图 | `:js-permission` 与 dapui/which-key 无回归 |
| 7 | 删掉三处旧路由（compositor 前置、editor 前置、终端分支） | 全量测试 + 真机 |

**明确不做**

- 不改 `Mode` 的定义（Normal/Insert/Select 仍是 `helix-view` 的，`ModeLayer` 只做只读包装），否则 `helix-view` 会依赖 `helix-term`。
- 不把"只读"（`readonly_forced`）做成层：它是**文档属性**，任何层下都应该生效。
- 不引入"嵌套子层"（如 `PANE→RESIZE` 再嵌套）：平级模式之间仍是**替换**关系（`C-n` 从 Pane 直接切 Resize），保持 zellij 语义。

---

## 5. 验收清单

- [ ] 全仓只有**一处**按键路由入口（`Router::route`），三个旧分支删除
- [ ] 新增一个模式只需：加一个 `InputLayer` + 一行徽标 + 一条测试（不碰 `compositor.rs`/`editor.rs`）
- [ ] 状态栏能同时显示多层：`NORMAL│DEBUG`、`INSERT│TERM`、`NORMAL│PANE`
- [ ] `Esc` 在全仓语义一致：弹出栈顶；栈底才是编辑语义
- [ ] `C-p` 在“暂停中”和“普通”下都能正常进出 Pane 层（层可组合）
- [ ] which-key 与内置 Info 的提示都来自 `Router::hints()`，无第二份表
- [ ] 现有 pty 回归脚本全绿（平级模式 5 个、调试键、终端输入、dapui 面板键冒泡）


---

## 6. 落地记录（步 1–3 完成）

| 步 | 状态 | 落点 |
|---|---|---|
| 1 | ✅ | `helix-term/src/input_router.rs`：`Layer`/`LayerKind`/`Swallow`/`layers()`/`badges()` + 4 条单测（栈序、未命中策略、每个平级模式都有提示表、调试键含 SHIFT 兜底） |
| 2 | ✅ | 平级模式提示表**移进** `input_router::pane_hints()`；`compositor::pane_mode_entries` 变薄包装（单一来源） |
| 3 | ✅ | 调试键查表从 `ui/editor.rs` 的本地副本搬进 `input_router::debug_key()`；徽标经 `helix.input_badges()` 暴露给插件（compositor 每帧同步，值不变不写） |
| 4 | ✅ | `input_router::plan(pane, debug, key, allow_prefix) -> Plan`：前缀键切换、`Esc` 弹层（`Plan::Pop`）、Locked 放行、调试层命令、`Pass`，**纯函数**（7 条单测）。`compositor::handle_event` 只剩 `match plan {}` 执行 |
| 5 | ⚠️ 部分 | 终端层进了栈（`LayerKind::TerminalInput` + `TERM` 徽标，`badges_with_terminal`），但**键仍由终端组件转给 PTY**：把 PTY 键转换搬到 Router 要动 `plugin_terminal` 的热路径，收益不抵风险（见下"偏差"） |
| 6 | ✅ | 提示唯一来源 `input_router::hints(pane, debug)`：调试层优先（标题 `Debug`，条目来自 `dap_keymap()`），否则平级模式；`compositor::pane_mode_hint` 只剩渲染 |
| 7 | ✅ | 删掉三处旧路由：`handle_event` 的 Ctrl 前缀块、`pane_mode_key` 里的前缀/`Esc`/Locked 分支、`ui/editor.rs` 里的本地调试查表 |

**可见成果（release 实测）**

```
普通                    状态栏           （无徽标）
命中暂停                [DEBUG]          ← 调试层
暂停中 C-p              [DEBUG│PANE]     ← 两层并存（层可组合）
Esc（弹掉栈顶）          [DEBUG]          ← 只退出 Pane 层，调试层还在
继续                    （无徽标）
运行中 C-p              [PANE]
```

`Esc` 的行为**已经是对的**（弹掉栈顶层），只是路由还留在 `compositor::pane_mode_key` 里 —— 步 4 才把它搬进 `Router`。

**过程中发现的两个坑（已记）**

1. 状态栏被 `plugins/statusline` 用 `set_statusline(render, {replace:true})` **整体接管** ⇒ Rust 侧 `parts.left` 的徽标**永远不渲染**。所以徽标必须走插件通道：新增 `helix.input_badges()`，并在 statusline 插件里加内置段 `layers`（默认左区 `["window","layers","mode",…]`）。
2. 用户的 `[plugins.statusline] left` 是**显式配置**，会覆盖默认列表 ⇒ 加新段时要同步改用户配置（本次已改 `~/.config/helix/config.toml`）。


### 6.1 与设计稿的两处偏差（有意）

1. **终端层的键仍留在 `PluginTerminal`**：`TermInputMode::Insert` 时按键要转成 PTY 字节（`key_to_term_bytes`，还依赖 `grid.engine_mode()`），
   这是终端的**领域知识**。若搬进 Router，Router 就得认识终端 grid —— 反而制造新的耦合。
   所以终端层只贡献 `TERM` 徽标与栈位（`swallow=Pass`），键路由保持组件内。
2. **调试层命令的实际执行点仍在 `EditorView`**：Router 只做"这键归调试层"的**决策**（`Plan::Debug`），
   执行需要 `commands::Context`（count / register / callback / jobs），那是编辑器叶子的东西。
   `compositor` 收到 `Plan::Debug` 时什么都不做 —— 让事件继续流向叶子，由叶子用同一个
   `input_router::debug_key()` 解析执行（**同一张表**，不重复实现）。

### 6.2 完整回归（release 实测）

```
普通 → C-p PANE → C-n RESIZE(替换不叠加) → C-h MOVE → C-y SCROLL → C-g LOCKED
Locked 里按 n（放行，仍 LOCKED） → C-g 退出（无徽标）          ✓ 平级模式全绿
调试：命中 [DEBUG] → C-p [DEBUG│PANE] → Esc [DEBUG] → c 清空    ✓ 层可组合 + Esc 弹层
```


### 6.3 层与提示打通（同轮收尾）

| 项 | 之前 | 现在 |
|---|---|---|
| 暂停时有没有提示 | 只有 `?` 手动查 | **进入暂停的边沿自动弹一次** `Debug ▸ r 重启 · T 结束会话 · c 继续 · p 暂停 · n 单步跳过 …`（`input_router::hints()` → which-key 的 line 模式） |
| 继续之后 | 无 | **自动收起**（边沿检测，不再挂着） |
| 中文描述 | which-key 不认识 `Debug` 标题 | `PREFIX_KEY["Debug"]="dap"` + 20 条 `dap …` 中文，标题取自引擎（不硬编码第二份键位） |

实现要点：`Compositor.last_debug_paused` 做边沿检测（每帧一次，仅翻转时动作），
`pane_mode_hint()` 用同一套渲染路径（`keymap_hint` → JS 接管 → 内置兜底）。


---

## 7. 附带：构建输出流式化（同批）

| 项 | 之前 | 现在 |
|---|---|---|
| 构建期间 Output | `Command::output()` 一次性收集 ⇒ **一片空白**（20 秒像卡死） | `stdout`/`stderr` 各起 reader 任务逐行 `dap_output::push()` |
| 传递机制 | 全部塞进完成时的 `Callback` | 进程级队列（4096 上限，满了丢最旧）+ `helix_event::request_redraw()` 唤醒；主线程 `pump_term_events` 每帧 drain 成 `dap-output` 事件 |
| 面板可见性 | 要手动 `:dapui-output` | 第一行到达**自动开底部并切到 Output**（不抢焦点） |
| 失败诊断 | 结束后一次性打印 | 边跑边显示；结束时照旧报 `构建失败（exit N）` |

**验证（release，用 `sleep 1` 的 Makefile 做慢构建）**

```
+1.6s  panel: [build] step 1: preparing / step 2: compiling
+3.0s  panel: [build] warning: demo warning / step 3: linking / cc …
+4.5s  panel: [build] build done
```

**踩到的坑**：两个单元测试共享进程级队列 ⇒ 并行跑互相污染（`cargo test` 默认多线程），
加了测试内串行锁。另：`make` 看到产物已存在会立刻“up to date”退出 —— 测流式前先删产物。


## 8. 可见性/手感补丁（同批）

| 项 | 之前 | 现在 |
|---|---|---|
| 断点在编辑区 | 只能去 Breakpoints 页看 | 断点行首画 `●`/`◯`（`breakpoint_marks`，默认开）|
| REPL 的 `Esc` | 直接回编辑器（丢掉已输入内容） | **有输入先清输入**，清空后再按才回编辑器 |
| 树里 `y` 在帧节点上 | 无动作 | 复制 `file:line` |

**坑（值得记）**：`set_virtual_text(path)` = **Clear 整个文件**的装饰。所以"断点标记"必须画在
`refresh_decorations()` 的**最后**（它开头会 Clear 一次，把高亮/内联/标记一起重画），
否则标记会被自己的清屏抹掉（实测第一次就是标记刚画上就消失）。


## 9. REPL 补全（同批）

- `Tab` / `Shift-Tab`：发 DAP `completions`（`text` + 1-based `column` + 当前 `frameId`），
  候选条画在提示行上一行 `[2/7] size_t  static  …`，再次 `Tab` 循环、打字/退格清掉。
- **两个 lldb 坑**：
  1. 候选里有一个 `{label:"", text:""}` 的空项 ⇒ 不过滤的话第一次 `Tab` 等于没反应（过滤掉）；
  2. `label` 常是 `"s -- int &"` 这种"名字 + 文档"，而 `text` 有时是**原文**（不推进）⇒
     插入规则改成"取 label 第一个词；只有它以当前输入为前缀时才用它，否则退回 `text`"。
- 顺手：`Tab` 在 REPL **有输入时**不再切页（否则补全键被 tab 切换吃掉）。


## 10. 多会话并排（同批）

| 项 | 之前 | 现在 |
|---|---|---|
| 树 | 只画 active 会话 | 每个会话一个节点：`▾ #2 ⏸ t1 (当前)`（展开）/ `▸ #1 ⏸ t5`（折叠） |
| 切会话 | `S` 弹层（记不住） | 选中别会话按 **`Enter`**；展开它还能看它的栈帧（不带 scopes） |
| 单会话 | 帧直接铺在顶层 | **不变**（`show_all_sessions` 关或只有 1 个会话时零回归，有单测钉住） |

实现：`load_roots` 在多会话时把"当前会话的帧"包进会话节点，其余会话给一个占位子节点；
`expand_node` 新增 `node.session ≠ T.ctx.id` 分支（拉 `stack_frames(thread, session)` 只列帧）；
`key_tree` 的 `Enter` 在会话节点上是"切换"而不是"展开"。


## 11. 重启带重建（同批）

| 项 | 之前 | 现在 |
|---|---|---|
| `r`（重启） | 直接重放 `launch` 参数 ⇒ **跑的还是旧二进制**（改了代码按 `r` 看不到新行为） | 会话**记住起它时的构建命令**（`Client::starting_build`），`r` 先 `dap_build_run` 再启动：`重启：先构建（make）…` → `构建完成，正在启动…` |

实现要点：
- `start_session_with(..., build)` 把构建命令写进 `client.remember_build(...)`（三条入口都传：语言模板/launch 配置/`start_or_build`）；
- `dap_restart` 里新增分支 ①'：有构建命令 ⇒ 先 disconnect 旧会话，再 `dap_build_run(..., Some(PendingLaunch), Some(build))`；
- 实测：`r` 后日志出现 `构建中: make` → `构建完成，正在启动…` → 新会话停在断点。


## 12. 收尾三项（同批）

| 项 | 之前 | 现在 |
|---|---|---|
| **终端层进 Router** | `terminal_passthrough()` 只在 `handle_event` 里"让前缀路" | `input_router::plan_layered(..., terminal_insert, ...)` 返回 `Plan::Terminal`；直通时键归 PTY，`C-g` 是唯一例外。`handle_event` 不再判断终端 |
| **变量分页** | `supportsVariablePaging=false`，大数组一次全拉（靠 `max_nodes` 截断） | 声明支持 paging；变量一次取 200，满了挂 `… 更多（start=N，按 l 加载）`，展开拉下一页并追加。单测覆盖"请求带 start/count + 追加下一页" |
| **REPL 多行渲染** | 多行草稿压成一行（`⏎`） | 按真实换行画（最多 4 行/半屏），转录区自动收缩，候选条紧贴草稿上方 |

**踩到的两个坑**
1. `variables` 的 JS 桩只透传了第一个参数 ⇒ 分页测试"每次 start 都是 0"，误判成 bug。桩要透传 `(ref, id, start, count)`。
2. 自动展开（`max_depth`）把「… 更多」也点了 ⇒ 一路拉满。递归里要排除 `node.more`。


---

## 13. 自检记录（2026-10-27，本轮）

发现并修掉的遗漏：

| # | 问题 | 处置 |
|---|---|---|
| 1 | `hints()` 顺序写反了：暂停中进 Pane 模式会显示 **Debug** 表，而键其实归 Pane（与 `plan()` 相反） | 改成与 `plan()` 同序（平级优先），测试改名 `hints_follow_routing_order` |
| 2 | `docs/.../2026-10-26-dap-debug-mode-design.md` 的 §1–§8.1 描述的是旧模型，从顶部读会被误导 | 顶部加**状态横幅**，指向 §9 与 `docs/api/dap.md` |
| 3 | `[plugins.dapui]` 有 15 个配置项，文档零散 | `docs/api/dap.md` 加一张**完整配置表** |
| 4 | `helix.d.ts` 的 `variables()` 没写 `start`/`count` | 补签名与说明 |
| 5 | **6 个死函数**（`session_action` / `toggle_breakpoint_at_cursor` / `pick_session` / `pick_thread` / `pick_frame` / `show_help` / `caps_summary`）—— 面板改为冒泡后没人调用，只剩测试在调 | 删除函数 + 8 条对应测试（行为由"面板不接管会话键"那条 + 核心 picker 覆盖） |
| 6 | `clippy` 两个 warning：`dap_build.rs` 的 identical-blocks、`dap.rs` 的 `needless_option_as_deref` | 都清掉（helix-term 现在 clippy 0 warning） |
| 7 | 「构建完成但产物不存在: （空名字）」—— 没检测到 program 时误导 | 改成"构建完成，但没检测到要启动的产物；用 `:debug-start <模板> <可执行文件>` 指定…" |
| 8 | `keymap/default.rs` 里 `dap_keymap` 上方还留着"`PaneMode::Debug`"的旧注释 | 删掉 |

**验证（全量）**

```
cargo  helix-term 206 · helix-js 247 · helix-view 79 · helix-dap 16  全绿
clippy  helix-term 0 warning（helix-view 的 7 条在 helix-vcs，属既有、非本轮）
node   dapui 56（删了 8 条死测试）+ which-key 20 = 76 passed
api-check ✓ · rustfmt ✓ · release 已装
真机   C：命中 4 → n 5 → c 结束；python：命中 add:2；层徽标 [DEBUG] / [DEBUG│PANE] / [PANE]
```

**留着的已知项（不改）**：`helix-vcs` 的 7 条 clippy warning（不属本轮范围）；
`inline_values` 仍是实验项（`set_virtual_text` 只能插列 0）。


## 14. `[keys.debug]` 与容错解析（同批）

**结论：原来没有，现已支持。** `[keys.*]` 只有 `normal`/`insert`/`select`（`Mode` 枚举）；
调试键表在 Rust 的 `dap_keymap()`，由 `input_router` 在 Normal 模式**先于** keymap 接管。

**顺带修掉的一个粗暴失败**：`keys` 原本是 `HashMap<Mode, KeyTrie>` ⇒ 写个
`[keys.debug]` 会让 **整份 config** 解析失败（`Invalid mode 'debug'`，主题/插件配置全废）。
现在改成 `KeysConfig` 自定义反序列化：未知段名**警告 + 跳过**，其余段落正常生效；
警告进 `helix.log`，`Config.warnings` 也带上（启动/`:config-reload` 时会 `set_warning`，
但会被 startup 事件里的消息覆盖，最终可靠位置是日志）。


### 14.1 落地：`[keys.debug]`

| 环节 | 实现 |
|---|---|
| 解析 | `KeysConfig` 的自定义 `Deserialize` 多收一个 `debug` → `Option<KeyTrie>`（未知段名仍只警告） |
| 合并 | `Config.keys_debug = dap_keymap() + merge_nodes(用户表)`（global 先、local 后） |
| 运行时 | `keymap/default.rs` 里 `OnceLock<RwLock<Arc<KeyTrie>>>` 缓存合并结果；`Config::load` 末尾 `set_debug_keymap(...)`，`input_router::debug_key()`/`hints()` 读它 |
| 提示 | `hints()` 用同一张合并表 ⇒ which-key 的 `Debug ▸ …` 自动显示自定义键 |
| 测试 | `keys_debug_merges_onto_dap_keymap`（x→dap_next 且内置 n 仍在）、`unknown_keys_section_only_warns` |

**实测**（`[keys.debug]` 里写 `x = "dap_next"` + `A-1/A-2`）：暂停时按 `x` 从 `add:4` 走到 `:5`；
`A-1` 进树、`q` 关掉树；`A-2` 进底部。`:config-reload` 后立即生效。


## 15. 收口四项（同批）

| # | 项 | 做法 | 测试 |
|---|---|---|---|
| 1 | 变量分页去重 + 页上限 | 按 `name` 去重；一页全重复 ⇒ 显示「adapter 不支持分页」并停止；`VAR_MAX_PAGES=20` | `变量分页：adapter 忽略 start ⇒ 不追加重复项…` |
| 5 | 配置警告可见 | `startup` 事件**之后**再 `set_warning`（dashboard 会写同一行，早设会被盖掉） | 真机：`[keys.nromal]` 一启动就提示 |
| 6 | 监视持久化 | `~/.cache/helix/dapui-watches`，`load_roots` 懒加载、增删落盘 | `监视持久化：加/删落盘，下次启动读回` |
| 4 | node 零配置 | `find_js_debug_server_in(home)` 探 `~/.vscode{,-server,-insiders}/extensions/ms-vscode.js-debug-*/src/dapDebugServer.js` 与 `~/vscode-js-debug/…`（多版本取最近修改）；`debugger_for` 里命令为空时补 `node <server>` | `finds_dap_debug_server_in_vscode_extensions`（临时目录，不改进程 HOME） |


## 16. 断点键顺手化 + 行首标记的"无声失效"（2026-10-28）

用户反馈：「`space G` 打断点卡手；按了 `space G` 之后不能上下移动」。

| 问题 | 根因 | 修法 |
|---|---|---|
| `space G` 之后按 `j/k` 没反应 | 该菜单 `sticky=true` ⇒ 一直挂着，未绑定的键被**吞掉** | 改成 `sticky=false`：按完一个动作立刻回普通模式 |
| 打断点要三个键 | 没有直接入口 | 内置 **`F9` = `dap_toggle_breakpoint`**（VS Code 习惯）；用户配置再给 `A-b` |
| **`F9`/`space G b` 打了断点但行首没有 `●/◯`** | `helix.dap.list_breakpoints()` 读的是**缓存快照**，平时只在"有 DAP 请求完成"时刷新 ⇒ **没会话时没人刷**，插件永远看到空表 | 新增事件 `dap-breakpoints`（本地断点表变化就发，含 `F9`/JS API/条件/日志/编辑重映射），发之前先 `refresh_dap_snapshots`；dapui 订阅它重画标记 |
| 顺带发现 | `":cmd<ret>"` 这种写法不是合法 keymap 值（`<ret>` 只属于 `@...` 宏），会让**整份 config** 报 `Bad config` ⇒ 插件全不加载（现象：`:dapui` 忽然"不存在"） | 报错信息直接提示正确写法（"核心命令写法是 ":cmd"（不带 `<ret>`）"） |

**杂项**：`helix.d.ts` / `docs/plugin-api.md` 补 `dap-breakpoints`（api-check 会卡这个）。

**验证**：`space G b` → `◯` 出现在第 3 行、紧接着按 `j` 光标移到 4:1（不再卡住）；`F9` 开/关各一次；`A-b` 同理。


## 17. 收尾：`T` 的会话清理 + buffer 堆积（2026-10-28）

用户反馈：「debug 结束后打开很多 buffer 没关，底部/右侧也没关」。

| 问题 | 根因 | 修法 |
|---|---|---|
| 按 `T` 后面板不收、状态栏还挂着 `#1 ⏸ t…` | `dap_terminate` 只 `unset_active_client()`：**不发 `dap-terminated`**、也**不从 registry 摘会话** ⇒ 插件以为会话还在 | 发 `dap-terminated`（带 public id）+ `teardown_session()`（`remove_client` + `unset_active` + 断点 `invalidate_all`），与 Terminated 路径一致 |
| 单步进库文件后 buffer 堆积 | 停在哪就 `open(path, Replace)`，会话结束没人关 | `Editor` 记 `debug_docs_before`（会话真正 start 时的文档集）与 `debug_opened_docs`（跳进去的新文档）；结束时 `close_debug_opened_docs()` 只关"新开 + 未修改"的，至少留一个 |
| `:debug-log` 每敲一次多一个空 buffer | 每次 `new_file` | 记 `debug_log_doc`，复用；不存在了才新建 |
| 事件发出时插件拿到的是旧快照 | 快照只在"有 DAP 请求完成"时刷新 | `pump_term_events` 在 drain `pending_dap_events` 前先 `refresh_dap_snapshots` |

**验证**：main.c 打断点 → 命中 → `s` 进 `lib.c` → 按 `T`：底部/右侧收起、状态栏不再显示会话、`lib.c` 自动关掉回到 `main.c`；`:debug-log` 连敲三次 `documents.len()` 恒为 2。


## 18. 这一轮的四件小事（2026-10-28）

| 反馈 | 根因 | 修法 |
|---|---|---|
| `?` 只写一行状态栏，看不全 | `debug_help` 就是一句 `set_status` | 改成**开窗口**：内容从运行时键位表生成（含 `[keys.debug]`），带中文说明；复用同一个临时 buffer；`mark_saved_now()` 让 `:bc` 能直接关（否则挂 `[+]`） |
| `<space>G l` 的列表没对齐 | 手写 `format!("{:<12} {:<7} {:<9} {}")` 拼成一行 | 改用 `PickerColumn` **真列**（配置/请求/来源/语言），宽度由 picker 算 |
| 打断点出现**两个**标记 | 核心 gutter 已画 `●/◯`，dapui 又插了一段虚拟文本 | `breakpoint_marks` 默认 **false**（想恢复行内标记再开） |
| 每次打开 Output 都是上次的内容 | 会话之间没清缓冲 | `dap-session` 且 `kind == "created"` ⇒ 清 `B.output` + 半行缓存（用 `created` 而不是 `running`，后者 continue 后也会发） |

**踩坑记录**：`:q` 关不掉这个临时 buffer —— 它是主叶子唯一的视图，`quit` 关的是"视图"会被拒；
正确姿势是 `:buffer-close`（`:bc`）。帮助页里已经写了。
