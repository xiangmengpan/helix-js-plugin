# helix × zellij:把 zellij 的终端仿真 / 模式系统 / pane 模型搬进 helix 设计规格

日期:2026-09-11
状态:待用户审查
上游:本规格之前的三轮讨论(路线选择 → 探路 spike → 四节设计确认);实现计划另行产出。

> 一句话:**不引入 zellij 运行时**,把 zellij 的三样东西按算法搬进 helix——终端仿真内核(alacritty fork 的上游 `alacritty_terminal`)、模式系统(locked/pane/resize/move/scroll)、pane 模型(tiled + floating + stacked)。helix 继续自己独占终端,保持单二进制。

---

## 0. 决策记录(用户确认)

| # | 决策 | 出处 |
|---|---|---|
| D1 | **路线 2:搬进 helix**。zellij 当宿主(路线 1)已否决——那会让现有 window mode/compositor 作废,并出现两套窗口系统 | 用户选择 |
| D2 | **模式集只搬有语义的五个**:locked / pane / resize / move / scroll。**丢掉 session 与 tmux**(helix 无对应,顺带消解 `C-o`/`C-b` 冲突);**tab 留空**(`C-t` 预留) | 用户选择 |
| D3 | **删掉 `C-w`**。全局拦截与 `default.rs` 里的 `"C-w" => { "Window" }` 子图一并删除,不保留别名 | 用户点名 |
| D4 | **插件 API 可不兼容**。以更干净的命名为准,插件后续重写 | 用户授权 |
| D5 | 其余细节由实现者定(本文档给出结论,标注为"实现者定") | 用户授权 |

### 实现者定的细节(可改)

- **D5-a scroll 前缀 = `C-y`**:zellij 原为 `C-s`,但 `C-s` 在 helix normal(`save_selection`)与 insert(`commit_undo_checkpoint`)两侧都被占。`C-y` 两侧皆空 → **零牺牲**。
- **D5-b Locked 全局有效**:语义 = "除 `C-g` 外,helix 不再解释任何多路复用器前缀键,全部原样交给当前叶子"。终端叶子 → 强制 pty 直通(退出时恢复原输入模式);编辑器叶子 → 编辑器照常工作,仅前缀键失效。理由:这才是 zellij 语义的忠实移植("关掉多路复用器的耳朵"),且 `C-g` 在任何焦点下都有确定行为。
- **D5-c `z`(zellij 的 TogglePaneFrames)阶段①不做**:helix 没有"每个 pane 一个可切换边框"的概念,不发明该功能;阶段② 统一 border 时再定。
- **D5-d 内部类型名不机械重命名**:对外(JS / 文档 / 状态栏)一律称 **pane**,内部保留 `LayoutTree` / `leaf` 命名。纯改名对行为零收益、对 diff 有大成本。

---

## 1. 目标与非目标

### 1.1 目标

1. 终端面板达到"能正常跑 TUI 程序"的水准(鼠标上报、bracketed paste、滚动区、reflow、CJK 列宽、完整 SGR)。
2. 一套 zellij 手感的**模式系统**:`C-g` locked / `C-p` pane / `C-n` resize / `C-h` move / `C-y` scroll;模式内键位照搬 zellij 语义。
3. 布局模型补上 zellij 的两样东西:**floating 层**与**stack**,并顺手把现在**两套并行层级**收敛成一套。
4. 插件 API 重做为统一 pane 模型(pane = 内容 × 摆放),并让插件**能读到布局与模式数据**。

### 1.2 非目标(明确不做)

- **不引入 zellij 运行时**(不依赖 `zellij-server`,不做 IPC/CLI 遥控,不做 WASM 插件)。
- **不获得 zellij 的服务端特性**:会话 detach/attach、多客户端、session resurrection——那些来自它的 client/server 架构,搬模型拿不到。
- **不换几何算法**:保留二叉切分树,不移植 zellij 的"洞填充"几何(理由见 §5.6)。
- **不做 sixel/图像协议**:`alacritty_terminal` 0.26.0 不含(已核实)。
- **不搬 tab 模式**:helix 没有标签页概念。
- 不改 helix 的编辑键位(除删 `C-w` 外)。

---

## 2. 现状与证据

### 2.1 终端是画布级 PoC

`helix-term/src/ui/plugin_terminal.rs`(1534 行)手写 `TerminalGrid` + 精简 `vte` 分发器。实测(`grep -c` 于该文件):

| 能力 | 现状 |
|---|---|
| DEC 私有模式 | **只认 `?1049`**(alternate screen);鼠标上报 `?1000/1002/1006`、bracketed paste `?2004`、focus `?1004` **全部落进 `_ => {}`** |
| CSI 覆盖 | 光标移动 / 擦除 / SGR / ICH·IL·DL·SU·SD;**缺 `CSI r`(滚动区)**、DCH、ECH、REP、DA/DSR、光标样式、字符集、`?6` 原始模式、`?7` 自动换行、`?1007` 备用滚屏 |
| 宽字符 / 组合字符 | 29 处手算 `width`;无 zero-width 概念 |
| `resize()` | `resize_cells` 只做截断 / 补齐,**无 reflow** |
| 超链接 | 无 OSC 8 |

**后果**:vim/tmux/htop 里鼠标废、粘贴变逐字符、拉窗口内容错位、vim 分屏花屏。

### 2.2 窗口层是两套并行层级

| 组件 | 行数 | 职责 |
|---|---|---|
| `helix-term/src/compositor.rs` | 1221 | `layers: Vec<Box<dyn Component>>`(注释原文:"瞬态覆盖层(弹窗/菜单/提示)——不参与布局,渲染在主区域之上")+ `main_tree: LayoutTree` |
| `helix-term/src/ui/layout.rs` | 1695 | `LayoutTree`:二叉切分树,叶子 = 编辑器 / 终端 / 面板;含 `fixed: HashSet<u64>` |
| `helix-term/src/ui/plugin_panel.rs` + compositor 的 `register_panel`/`remove_panel`/`set_panel_side` | — | **布局树之外自己写的一套停靠 + 尺寸逻辑**(`PanelSide`、`set_side`) |

渲染顺序:`main_tree` → `layers` 逐个 → 状态栏。

> **更正**:`helix-term/src/ui/comp_layout.rs`(1244 行)**不是窗口层级**——它是插件 UI 内容内部的排版引擎(把 JS 的 `CompNode` 树 Row/Col/Text/Scroll 排成 `StyledLine`,相当于 flexbox),与窗口布局正交。早先讨论中"三层职责交叠"的说法不准确。

### 2.3 zellij 侧实证

- 本机装有 **zellij 0.44.1**;`zellij setup --dump-config` 给出模式与键位的权威定义。
- **模式前缀 = 该模式的退出键**(zellij 惯例):`locked`←`C-g`、`pane`←`C-p`、`resize`←`C-n`、`move`←`C-h`、`tab`←`C-t`、`scroll`←`C-s`、`session`←`C-o`、`tmux`←`C-b`。
- **屏幕仿真 fork 自 alacritty**:源码路径 `zellij-server/src/panes/alacritty_functions.rs`、`grid.rs`、`terminal_character.rs`、`hyperlink_tracker.rs`、`search.rs`,解析器 `vte`。它 vendored `termwiz` **只用于输入解析**(`zellij-utils/src/vendored/termwiz/input.rs` 等)。
- **几何代码剥不出来**:`zellij-server-0.45.1`(MIT)的 `tiled_panes/mod.rs` 依赖块包含 `os_input_output::ServerOsApi`、`output::Output`、`plugins::PluginInstruction`、`thread_bus::ThreadSenders`、`ui::boundaries::Boundaries`、`ui::pane_contents_and_ui::PaneContentsAndUi`、`ClientId` —— 与 zellij 运行时深度耦合。几何本体约 10,139 行(`tiled_panes` 3186+2363+341、`stacked_panes` 1233、`floating_panes` 1780+945、`swap_layouts` 291)。
- 结论:**只能按算法重实现,不能搬文件**。

### 2.4 键位占用实测(helix normal 顶层)

按模式块严格解析 `helix-term/src/keymap/default.rs`(缩进 8 空格 = 顶层;注意 `C-w` 子图与 insert 块会污染粗粒度 grep):

- normal 顶层已占的 `C-*`:`C-a C-b C-c C-d C-f C-i C-o C-s C-u C-w C-x C-z`
- insert 顶层已占的 `C-*`:`C-s C-x C-r C-w C-u C-k C-h C-d C-j`

| zellij 前缀 | normal | insert | 处置 |
|---|---|---|---|
| `C-g` locked | 空 | 空 | 直接用 |
| `C-p` pane | 空 | 空 | 直接用 |
| `C-t` tab | 空 | 空 | 预留 |
| `C-n` resize | 空 | 空 | 直接用 |
| `C-h` move | 空 | **占**`delete_char_backward` | 靠 §4.2 豁免规则解决 |
| `C-s` scroll | **占**`save_selection` | **占**`commit_undo_checkpoint` | 换 `C-y`(D5-a) |
| `C-o` session | **占**`jump_backward` | 空 | **丢模式**,冲突自消 |
| `C-b` tmux | **占**`page_up` | 空 | **丢模式**,冲突自消 |

---

## 3. 阶段⓪:终端引擎换成 `alacritty_terminal` 0.26.0(探路结论)

独立、有界、可单独先做与单独回退。**与引擎无关的"写"侧工作量见 §3.3,不要漏算。**

### 3.1 实测对比(真值夹具,非文档推断)

夹具:专用 tmux socket 跑真实 TUI,`pipe-pane` 抓原始 PTY 字节流,`capture-pane -p` 抓同一时刻屏幕真值。

| 观测项 | `vt100` 0.16.2 | **`alacritty_terminal` 0.26.0** |
|---|---|---|
| vim / htop / less 文本 vs tmux 真值 | 0/24 行差异 | **0/24 行差异** |
| alt screen / bracketed paste / 鼠标模式 / 编码 | 正确 | 正确 |
| **80→40 列 resize 字符存活** | **68/108(前 40 字符被销毁)** | **108/108(reflow;首行入滚回历史)** |
| OSC 8 超链接 | 无(仅 `unhandled_osc` 回调) | 有(`Cell::hyperlink()`) |
| 接线代码量 | 19 行 | 52 行(含 12 行 `Dimensions` 实现) |
| **能否不 fork 直接嵌** | ✅ | ✅(`VoidListener` 已有默认实现;`Dimensions` 只要 3 个方法) |
| 名字级真正新增 crate | 0(itoa/unicode-width/vte 已在树中) | **4**(polling / errno / rustix-openpty / cursor-icon) |

> 依赖增量的口径:`helix-term` 现有 368 个 crate,按"名字不在树中"计。传递依赖数(16 vs 34)会误导——`libc`/`rustix`/`signal-hook`/`regex-automata`/`base64`/`home`/`parking_lot`/`vte`/`unicode-width` 你本来就拉。

**结论:选 `alacritty_terminal`**。唯一硬差异是 reflow,而它正是用户会立刻察觉的问题;代价是 ~33 行接线与 4 个 crate。

### 3.2 接线坑(实测踩到,非推测)

1. **`Dimensions::total_lines()` 若等于 `screen_lines()`**(不给 scrollback),reflow 溢出内容会被丢弃。必须留历史。
2. **reflow 后内容会进历史行**(负 `Line`),可见行索引随 `display_offset` 移动——取屏不能只扫 `Line(0..rows)`。
3. `Processor` 需要显式类型参数(`Processor::<StdSyncHandler>::new()`),否则 E0283。
4. `advance()` 收 `&[u8]` 而不是单字节,整段喂更合适。
5. 真实 crate 依赖走的是 `rsproxy.cn` 镜像(本机 cargo 配置),构建可用。

### 3.3 与引擎无关、躲不掉的工作量

引擎只解决**读**(应用输出),不解决**写**(发给 pty):

- **鼠标事件编码**:引擎只给模式标志(`MOUSE_REPORT_CLICK`/`MOUSE_MOTION`/`MOUSE_DRAG`/`SGR_MOUSE`/`UTF8_MOUSE`),**不生成** CSI 字节。坐标换算、命中哪个 pane、按模式(1000/1002/1003 + SGR/UTF8)编码全要新写。现状:`MouseReport` 在该文件中出现 **0 次**。
- **bracketed paste 包装**:引擎给 `BRACKETED_PASTE` 标志,`\e[200~ … \e[201~` 要自己包。
- **OSC 52 剪贴板**:引擎可解出(`Config.osc52`),接系统剪贴板要新写。

### 3.4 可用到的现有能力(换引擎顺带获得)

`TermMode` 位标志含:`SHOW_CURSOR APP_CURSOR APP_KEYPAD MOUSE_REPORT_CLICK BRACKETED_PASTE SGR_MOUSE MOUSE_MOTION LINE_WRAP LINE_FEED_NEW_LINE ORIGIN INSERT FOCUS_IN_OUT ALT_SCREEN MOUSE_DRAG UTF8_MOUSE ALTERNATE_SCROLL VI URGENCY_HINTS DISAMBIGUATE_ESC_CODES REPORT_EVENT_TYPES REPORT_ALTERNATE_KEYS REPORT_ALL_KEYS_AS_ESC REPORT_ASSOCIATED_TEXT KITTY_KEYBOARD_PROTOCOL`。
另有 `Cell::zerowidth()`(组合字符)、`Cell::underline_color()`、`Term::selection_to_string()`、`Term::cursor_style()`、`Term::damage()`、`Grid::display_offset()`、`selection` / `term::search` 模块。

### 3.5 阶段⓪ 范围与边界

- **只换 `PluginTerminal` 内部**:`TerminalGrid` → `alacritty_terminal::Term`。外壳保留:29 个 pub 方法、`TermMode`(Esc↔insert)、`TermInputMode`、`term_state` 持久化、钩子。
- **不碰插件 API**;对外调用点仅 `typed.rs`(创建 + `full_text()`)与 `compositor.rs`(光标 / 类型判定)/ `layout.rs`(叶子)。
- **升级触发条件**:若接线中发现"必须 fork 引擎才能嵌进来",则本阶段**停止并升级为架构级重议**,不闷头 fork。

---

## 4. 阶段①:模式栈与键位

### 4.1 状态机

`compositor.rs` 现有两态 `WindowMode { Inactive, Active }`,替换为**平级单模式**(照 zellij,不是嵌套栈):

```rust
enum PaneMode { Normal, Locked, Pane, Resize, Move, Scroll }
```

规则:

- 除 `Locked` 外,任一模式下按任一前缀键 → **直接切到该模式**(zellij 行为,不是压栈)。`Locked` 内不响应前缀键切换,必须先 `C-g` 出来。
- 按**自己那个前缀键** → 回 `Normal`;`Esc` 也回 `Normal`。
- 模式内未绑定的键 → **吞掉,不穿透**。
- 打开弹窗 / 菜单 → 自动回 `Normal`(沿用现有 `C-w` 逻辑)。
- 鼠标点击换焦点 → **保持当前模式**。

### 4.2 拦截与豁免(最关键的一条)

前缀键在 keymap 之前全局拦截,**会盖住 insert 模式的键**。现有 `C-w` 已有先例(终端 insert 直通放行)。统一成:

> **叶子处于 insert(编辑器 insert / 终端直通)时,前缀键一律放行给叶子;只在 Normal 类状态拦截。**
> **唯一例外:`C-g` 在所有状态下都拦截**——否则从"终端 insert 直通"里根本无法进入 `Locked`(那时它会被写进 pty)。
> `Locked` 模式下只拦 `C-g`,其余全部放行(这是 Locked 的定义本身)。

必须如此:`C-h` 在 insert 里是 `delete_char_backward`,不豁免就毁掉退格。
代价:`C-g` 不再能送给终端程序。可接受——`Locked` 的逃出键必须是"总能按到"的那一个。
附带收益:`Locked` 让终端的逃出键从 `Esc`/`i` 变成 `C-g`,于是 **`Esc` 可以还给终端程序**(大量 TUI 用 Esc)。

### 4.3 键位表

| 模式 | 前缀 | 模式内键 |
|---|---|---|
| **Locked** | `C-g` | 除 `C-g` 外全部原样交给当前叶子;终端叶子强制 pty 直通(退出恢复) |
| **Pane** | `C-p` | `hjkl`/箭头 聚焦 · `p` 切换焦点 · `n` 新分屏 · `d` 下分 · `r` 右分 · `x` 关闭 · `f` 全屏 · `w` 浮动`*` · `s` 堆叠`*` · `e` 嵌入`*` · `i` pin`*` |
| **Resize** | `C-n` | `hjkl` 增 · `HJKL` 减 · `=`/`+` 增 · `-` 减 |
| **Move** | `C-h` | `hjkl` 移动 / 交换 · `n`/`Tab` 下一个 · `p` 上一个 |
| **Scroll** | `C-y` | `jk`/`↓↑` 行 · `C-f`/`C-b`/`PgDn`/`PgUp`/`h`/`l` 整页 · `d`/`u` 半页 · `s` 进搜索 · `Esc`/`C-y` 退出 |

`*` = **阶段① 未实现**:按键给出"未实现"提示,并在 `pane_mode_keymap()` 里返回 `enabled:false, reason:"阶段②"`。
tab 模式(`C-t`)**不实现**,键位预留。`z`(zellij 的 `TogglePaneFrames`)不进本表,理由见 D5-c。

**Scroll 模式作用于当前叶子的视口**:终端叶子 → 滚动回看缓冲(现有 Esc 滚动模式提升为全局模式);编辑器叶子 → 只滚动视图、不动光标。`s`(进搜索)在终端叶子上搜滚回缓冲;编辑器叶子无该语义,该键提示"未实现"。

### 4.4 删除 `C-w`

- 删全局拦截中的 `C-w` 进入逻辑。
- 删 `helix-term/src/keymap/default.rs` 里的 `"C-w" => { "Window" … }` 子图(其能力被 Pane 模式完整覆盖)。
- **保留** insert 模式的 `C-w`(`delete_word_backward`)——那是编辑器功能,不是窗口模式。
- `plugins/features/which-key.js` 的 `C-w` 段下线,改由 `pane_mode_keymap()` 数据驱动。

---

## 5. 阶段②:布局模型

### 5.1 新增 floating 层

```rust
struct Compositor {
    layers:   Vec<Box<dyn Component>>,  // 保留:输入态弹窗
    main_tree: LayoutTree,              // 保留:几何算法不换
    floating: Vec<FloatingPane>,        // 新增
}
```

`FloatingPane { id, rect, z, pinned, component }`。

- 渲染顺序:`main_tree` → `floating`(按 z)→ `layers` → 状态栏。
- 焦点可在 floating 与 tiled 之间移动;关闭 floating 后焦点回 tiled 活动叶子。
- `pinned` 的浮窗不被点击穿透(zellij 语义)。

### 5.2 收敛面板停靠逻辑(两套层级的第一处实体)

`compositor.rs` 的 `register_panel` / `remove_panel` / `set_panel_side` + `plugin_panel.rs` 的 `PanelSide` / `set_side` 是**布局树之外的第二套停靠与尺寸逻辑**。

**改为:`main_tree` 的普通叶子**(停靠 = 一个方向 split),那套逻辑删除,直接复用布局树的 split / resize / swap。收益:面板能参与布局、能 resize、能 swap。

**JS 签名不变**:`open_panel({side, rail})` 照旧。
**rail**(贴边全高、不被分割)**不需要新机制**——布局树已有 `fixed`(免疫 swap/resize/close/minimize/equalize)。**rail = 一个 fixed 叶子**。

### 5.3 插件浮窗提成 floating pane

`helix.open_popup` 现在走 layer;提成 floating pane 后天然获得可移动 / 可缩放 / 多开 / z 序 / pin。**签名与行为不变**。

§5.2 + §5.3 合起来才是"合成一套":停靠面板归 tiled,插件浮窗归 floating,`layers` 只剩输入态(picker / prompt / menu / completion / select / hover)。

> 这不是妥协:`zellij` 自己也这么分——**插件 UI 是 pane,文本输入态是 mode**。

### 5.4 stack:新增叶子变体

```rust
enum Leaf { Pane(PaneId), Stack { members: Vec<PaneId>, active: usize } }
```

能建 / 能切 / 能拆。阶段① 里灰显的 `s`(堆叠)/ `w`(浮动)/ `e`(嵌入)**此时接通**。

### 5.5 布局序列化

- `restore_layout` **接线**(现状:只写缓存、不重建树,文档已标"半成品")。
- `get_layout()` schema → `{ version: 1, tree, active, zoomed, floating: [...], stacks: [...] }`。
- 新增 `:layout save/load/list/delete <name>`,格式 **TOML**(helix 已依赖 `toml`;KDL 要新增依赖;且与 `config.toml` 一致)。
- **必须带 `version: 1`** —— 这是要落盘持久化的数据,以后改 schema 需有迁移入口。
- zellij 的 swap-layout 循环(`:layout cycle`)**不在本次范围**(见 R8)。

### 5.6 不换几何算法(明确决策)

**不把二叉切分树换成 zellij 的"洞填充"几何。** 理由:切分树已能表达 zellij 的 split / resize / move(现已有);洞填充主要解决"pane 多且要跨树调整边界",对"编辑器 + 几个终端"的规模是过度设计。**优先级给 floating 与 stack,不给几何重写。**

---

## 6. 阶段③:插件 API v2(不兼容,用户授权)

### 6.1 为什么推翻

旧 API 有**四个入口**做同一件事("把东西放上屏幕"),各带一套位置词汇:

| 入口 | 位置词汇 |
|---|---|
| `helix.split(dir, {terminal\|panel})` | `dir` |
| `helix.open_terminal({cmd, side, size})` + `set_terminal_mode(dock/fullscreen/floating/minimized)` | `side` + 模式枚举 |
| `helix.open_panel({side, rail})` | `side` + `rail` |
| `helix.open_popup({render, width, height, position, layer})` | `position` + px/% + `layer` |

**内容类型与摆放位置焊死**,且 `layer`(输入态独占)与 `floating`(可移动窗)语义重叠却不一致。

### 6.2 新 API

```js
// 唯一入口:内容 × 摆放
const id = helix.pane.open({
  terminal: { cmd, onExit },              // 内容三选一
  doc:      { path },
  view:     { render, onKey, onClose },

  split:  "right" | "bottom",             // 摆放三选一;省略 = 切分当前焦点
  stack:  stackId,
  float:  { x, y, w, h, center, pinned },

  ratio, size, fixed, z, id,
}) -> paneId

helix.pane.close(id) · focus(id) · focus_dir(dir) · list() · state(id)
helix.pane.move(id, dir) · resize(id, {ratio}|dir)
helix.pane.set_place(id, place)           // tiled ↔ floating ↔ stack
helix.pane.zoom(id) · unzoom() · minimize(id, bool) · equalize() · fix(id, bool)
helix.pane.raise(id) · pin(id, bool)

helix.stack.create({split,ratio}) -> sid
helix.stack.add(sid, paneId) · activate(sid, idx) · remove(sid, idx) · list(sid)

helix.pane_mode.current()                 // "Normal"|"Locked"|"Pane"|"Resize"|"Move"|"Scroll"
helix.pane_mode.keymap()                  // { mode, keys:[{key,desc,enabled,reason?}] }

helix.layout.get() -> { version:1, tree, active, zoomed, floating:[…], stacks:[…] }
helix.layout.restore(l) · save(name) · load(name) · list() · delete(name)

helix.buffer.list() · current() · focus(id)   // 从 helix.buffers/focus_buffer 归拢
```

**`set_place` 是新模型里最有价值的 API** —— 它就是 zellij 的 embed/eject(浮窗↔平铺↔堆叠互转)。旧 API **根本表达不了**,因为位置焊在创建入口里。

### 6.3 旧 → 新映射

| 旧 | 新 |
|---|---|
| `split(dir, {terminal})` | `pane.open({ terminal, split: dir })` |
| `open_terminal({cmd, side, size})` | `pane.open({ terminal:{cmd}, split: side, size })` |
| `open_panel({side, rail})` | `pane.open({ view:{…}, split: side, fixed: rail })` |
| `open_popup({render, position, layer})` | `pane.open({ view:{…}, float:{center:true, …} })` |
| `set_terminal_mode(id,"floating"/"dock"/"fullscreen"/"minimized")` | `pane.set_place(id,{float})` / `{split}` / `pane.zoom(id)` / `pane.minimize(id,true)` |
| `close_leaf` `focus` `zoom` `unzoom` `resize_leaf` `resize_leaf_dir` `swap_leaves` `swap_leaf_dir` `equalize_leaf` `minimize_leaf` `layout_fix` `focus_leaf_dir` | `pane.close` `.focus` `.zoom` `.unzoom` `.resize` `.move` `.equalize` `.minimize` `.fix` `.focus_dir` |
| `get_layout` / `restore_layout` | `layout.get` / `layout.restore` |
| `buffers` / `current_buffer` / `focus_buffer` | `buffer.list` / `.current` / `.focus` |

### 6.4 迁移成本(实测)

| 插件 | 调用点 |
|---|---|
| `plugins/features/terminal.js` | 13 |
| `plugins/features/tabbar.js` | 5 |
| `plugins/features/filetree/index.js` | 5 |
| `plugins/features/arsenal/index.js` | 4 |
| `plugins/features/server-manager/index.js` | 2 |
| `plugins/features/picker.js` | 2 |
| `plugins/features/lsp-hover/index.js` | 1 |
| **合计** | **~24 处 / 7 文件** |

按 API:`open_popup` 7 · `focus` 4 · `open_terminal` 3 · `get_component_state` 3 · `open_panel` 2 · `get_layout` 2 · 其余各 1。**约一半是机械替换。**

同时要改:`docs/api/terminal-layout.md`、`docs/plugin-api.md` 的"终端与布局树"段(re-write)、`plugins/helix.d.ts`(383 行,重生成)、用户镜像 `~/.config/helix/plugins/` 同步。

### 6.5 新事件

现有事件白名单无窗口层级事件(只有 `mode-change` 编辑器 / `term-mode-change` 终端)。新增:

| 事件 | 载荷 | 用途 |
|---|---|---|
| `pane-mode-change` | `{mode}` | which-key 显示 `[PANE]` / `[LOCKED]` 提示 |
| `layout-change` | `{kind:"split"\|"close"\|"float"\|"stack"\|"focus", id}` | tabbar / filetree 同步 |

`pane_mode_keymap()` 让 which-key 摆脱硬编码——阶段①——未实现的 `s`/`w`/`e` 返回 `enabled:false, reason:"阶段②"`,可灰显。**"未实现"从代码注释变成可展示的数据。**

---

## 7. 测试与回滚

### 7.1 测试分层(沿用现有基建)

| 层 | 覆盖 | 现有规模 |
|---|---|---|
| `helix-term --lib` | 模式状态机转换、拦截规则(insert 豁免 / Locked 例外)、floating z 序与 rect、stack 激活拆合、面板叶子 split/resize | 148 |
| `helix-js --lib` | 新 API 参数校验与形状(`pane.open` 三选一互斥、`place` 解析、`layout` schema 版本) | ~111 |
| 集成 `helix-term/tests/test/*.rs` | 每模式键位冒烟、浮窗移动/缩放/多开、stack 建切拆、7 插件迁移后行为 | 44 文件 / 281 |
| node `node:test` | API v2 替换后逐插件冒烟(arsenal 已有 28 条现成模式) | 28 |

**新增属性测试**:`get_layout → restore_layout → get_layout` **逐字段相等**。用现有 `quickcheck` 依赖对树 / 浮窗 / stack 随机构造。序列化是本次唯一落盘持久化的东西,round-trip 是最便宜的保险。

**每阶段门槛**:上述全绿 + `clippy --all-targets` 零警告 + `fmt`。

### 7.2 "不兼容"≠"停世界"

最终形态确实**不留旧 API**;但迁移过程中旧 API **短暂共存**(内部双实现指向同一套),插件逐个迁,全部迁完再删旧 API。理由:任何一步都可保持绿灯、可 bisect、可中途停。这是工程做法,不是语义妥协。

### 7.3 阶段与子步(每步都要绿)

| 阶段 | 子步 | 该步之后绿的东西 |
|---|---|---|
| ⓪ 终端引擎 | 换 `TerminalGrid` → `alacritty_terminal::Term` | 现有终端相关集成 |
| ① 模式栈 | 状态机替换 + 删 `C-w`(拦截 + keymap 子图)+ 5 模式键位 | `window_mode.rs`/`window_rail.rs`/`window_split.rs` 改写后绿;which-key 同步 |
| ②-1 | **纯新增** floating 层 | 全绿(尚无使用者) |
| ②-2 | `open_popup` 内部换 floating(签名不变) | 现有集成 |
| ②-3 | 面板 layer → fixed 叶子,删 `PanelSide`/`set_panel_side` | 面板集成改写后绿 |
| ②-4 | `Leaf::Stack` 变体 | 全绿 |
| ②-5 | 序列化接线 + TOML save/load | round-trip 属性测试 |
| ③ | 新 API 全量加入 → 7 插件逐个迁移 → 删旧 API | 每迁一个插件跑一次 node + 集成 |

### 7.4 回滚

- **commit 粒度**:每子步一个中文 commit(沿用既有约定),阶段内可 `revert`。
- **阶段间独立**:⓪(终端引擎)与 ①(模式栈)互不依赖;②-1 独立;②-2/②-3 各自独立。任何一步失败只回退那一步。
- **唯一高风险点**:②-3(面板 layer → 叶子)。替换式改动,中间态无法保持绿。缓解:**先补面板行为的集成测试**(把"停靠/尺寸/关闭"的当前行为钉住),再动实现——测试是回滚的判据。
- ③ 最后一步"删旧 API"是纯删除,单独 commit,风险最低。

### 7.5 人工冒烟清单

改 Rust 后**必须 `cargo build --release`** 才在真实终端生效(既有教训)。tmux 内逐条:

1. 五个模式各进一次、各退一次,状态栏指示正确
2. Pane 模式:聚焦 / 新建 / 关闭 / 全屏;`s`/`w`/`e` 阶段①灰显 → 阶段②接通
3. 终端里 Locked:所有键(含 `C-p`/`C-h`)进 pty;`C-g` 逃出
4. 浮窗:拖 / 缩放 / 多开 / 置顶 / 关闭,焦点回落正确
5. stack:建 / 切 / 拆
6. `:layout save` → 重启 → `:layout load` 还原
7. 7 个插件逐个可用
8. 终端:鼠标点选、多行粘贴、无花屏、拉窗口内容不丢、CJK 列宽正确

> tmux `send-keys` 的 Enter/Esc 有时序坑,`C-[` 当 Esc 用(既有经验)。

---

## 8. 预计触碰的文件

| 阶段 | 文件 |
|---|---|
| ⓪ | `helix-term/src/ui/plugin_terminal.rs`、`helix-term/Cargo.toml`、`Cargo.lock` |
| ① | `helix-term/src/compositor.rs`、`helix-term/src/keymap/default.rs`、状态栏指示、`plugins/features/which-key.js`、`helix-term/tests/test/window_*.rs` 改写、新增 `pane_modes.rs` |
| ② | `helix-term/src/compositor.rs`、`helix-term/src/ui/layout.rs`、`helix-term/src/ui/plugin_panel.rs`、`helix-term/src/ui/plugin_popup.rs`、新增 `helix-term/src/ui/floating.rs`、终端模式文件 |
| ③ | `helix-js/src/layout.rs`(拆分)、`helix-js/src/popup.rs`、`helix-js/src/commands.rs`(事件)、`plugins/**`(7 文件)、`plugins/helix.d.ts`、`docs/api/terminal-layout.md`、`docs/plugin-api.md` |

---

## 9. 风险与未决

| # | 项 | 处置 |
|---|---|---|
| R1 | `alacritty_terminal` 版本随 alacritty 发布演进,升级时内部 API 会动 | §3.5 升级触发条件;若需 fork 则停下来重议 |
| R2 | ②-3 面板收敛是替换式改动,中间态不绿 | §7.4 先补行为测试再动实现 |
| R3 | `C-h` 作为 move 前缀依赖 insert 豁免规则,规则写错会毁掉退格 | §4.2 单测专测;冒烟清单第 3 条 |
| R4 | 用户镜像 `~/.config/helix/plugins/` 与仓库不同步 | 冒烟前同步 |
| R5 | 集成测试并行偶发 flake(reload/terminal 时序) | 按 filter 单跑,不全量并行 |
| R6 | tab 模式(`C-t`)留空,用户若后续要需定义 helix 的"标签页"语义 | 未决,阶段③ 后再议 |
| R7 | `z`(TogglePaneFrames)阶段①不做 | 未决,阶段② 统一 border 时再定 |
| R8 | zellij 的 swap-layout 循环(`:layout cycle`)不在本次范围 | 未决,阶段③ 后再议 |

---

## 附录 A:②-4 `Stack` 的实现方案(2026-09-11 追加,含低成本表示法)

§5.4 写的是 `enum Leaf { Pane(PaneId), Stack{members, active} }`。落地时发现这个写法**代价被严重低估**:
`LayoutNode` 加变体要让 **~67 处匹配点**全部跟进(`replace_leaf` / `split_side` / 邻居计算 /
rail 逻辑 / `dump` / `restore` / 渲染 / 焦点 / `leaf_ids_of` …),而且 **dump schema 也要改**
(直接撞上 §5.5 的 `version:1` 迁移问题)。

### A.1 推荐表示法:**旁表**,不动 `LayoutNode`

```rust
/// 堆叠组:一个"承载叶子"在树里占位,组内只显示 shown 那个。
/// key = 承载叶子 id(它独占树中位置,布局/交换/分裂语义全不变)
stacks: HashMap<u64, StackGroup>,
struct StackGroup { members: Vec<u64>, shown: usize }  // members[0] == 承载者
```

- 承载叶子的 rect 由 `layout_node` 照常分配(零改动)
- 渲染:承载叶子位置上改画 `members[shown]` 的组件(组件都在 `components` 里,按 id 取)
- 事件路由:把 `active`(承载者)映射成 `members[shown]`
- `C-p s`:把活动叶与**树序的下一个叶子**合并成一组(承载者 = 活动叶)
- `C-p p/Tab`:在 `members` 内循环 `shown`(与"切到下一个窗口"同键,语义按是否堆叠分派)
- `C-p x` / `remove(id)`:从组里摘掉;剩 1 个则解散组(与浮窗槽位的清理同理:
  **残留映射会让路由指向已删叶子 → 全部 Ignored → 程序僵死**)
- dump:加 `stacks` 字段(与 `floats` 同批),`restore` 只保留仍存活的 id

### A.2 表示法 B 的改动点(约 10 处,vs 方案 A 的 ~67 处)

| 位置 | 改动 |
|---|---|
| `LayoutTree` 字段 + `Default` | `stacks` 加入 |
| `handle_event` | 路由目标由 `active` 改为 `shown_member_of(active)` |
| `render` | 承载叶子位置上渲染 `shown` 成员；另画一行 header(成员列表 + 当前标记) |
| `Component::cursor` / `active_component` / `active_type_name` | 同上做一层映射 |
| `focus(id)` | id 是组内成员时：先把 `shown` 指向它，再聚焦承载者 |
| `remove(id)` | 从组里摘掉；剩 1 个则解散组(防僵死) |
| `dump` / `restore` | 增 `stacks` 字段；`restore` 只保留存活 id |
| `PaneMode::Pane` 的 `s` | 新建堆叠(替换掉现在的“阶段② 余下”桩) |
| `PaneMode::Pane` 的 `p`/`P`/`Tab` | 活动叶在堆叠组内时先循环 `shown`，否则维持“切下一个窗口” |
| `pane_mode_entries` | `s` 的 `enabled` 改 true；which-key 镜像同步 |

### A.3 要定下来的语义(实现前先写测试)

1. **`s` 建堆叠**:活动叶 + 树序下一个叶子合并(承载者 = 活动叶)。
   若下一个叶子已是别组的承载者/成员，则先把它从原组摘出(或拒绝并提示)。
2. **`shown` 循环**:组内成员顺序 = 合并时决定；`p`/`Tab` 前进、`P` 后退。
3. **header 行**:占用承载矩形顶部 1 行，显示成员数+当前序号；仅当组内成员 ≥2 时画。
4. **解散**:成员降到 1 个时，组自动消失(树不动，只是旁表项被删)。
5. **与浮窗/缩放/最小化交互**:承载者浮动时整组跟着浮动(sh)own 不变)。

### A.4 测试清单(先写，后实现)

- 建堆叠：`C-p s` 后 `pane.list()` 里两个 id `place` 相同(或加 `stack` 标记)，且只有 `shown` 那个拿 rect
- `p`/`P` 循环 `shown`，光标/事件路由跟着走
- `C-p x` 关掉 `shown` 后自动显示组内下一个；关到剩 1 个时组解散
- `dump → restore` 往返保留 `stacks`；成员已不存在时整组丢弃
- 回归：**没建立堆叠时，`p`/`Tab` 行为与现在完全一致**(不能把“切下一个窗口”弄坏)

### A.5 风险与回退

- 最大的风险不是实现量，而是**路由映射漏掉某一处**(`handle_event`/`cursor`/`active_component`
  各自独立取 `active`)。漏一处的症状是“键盘忽然不响应”——与 §5.1 浮窗那次同类。
  缓解：先写一条**不建堆叠也要绿的回归**，再逐处加映射，每加一处跑一次。
- 回退：全部落在 `LayoutTree` + compositor 的 `pane_mode_key`，可 **单个 commit revert**。
- 若旁表方案后续证明不够(例如需要把堆叠当作可交换/可分裂的一级对象)，再升级到
  `LayoutNode::Stack` 变体；那时 §5.4 的原文才需要生效。**先旁表，后变体**。

---

### A.6 进一步收敛:把“路由映射”整个消掉(2026-09-11 追加)

A.2 里要改 `handle_event`/`cursor`/`active_component` 三处映射 —— 那是**症状**,
不是必要的痏。再推一轮后发现有一个不变量能让它们全部消失。

#### 三个候选模型与各自的失败模式

| 模型 | 失败点 |
|---|---|
| 隐藏成员**没有**树位置(纯旁表) | 隐藏成员的组件成了“孤儿”:承载者被关时它们无处可去(UI 上消失但仍在 `components`) |
| 隐藏成员**有**树位置且被排除出布局 | `active` 若是非锚成员,结构操作(`swap`/`split`/`focus_dir`)会作用到一个**没有 rect 的槽位** → 新 pane 出现在不存在的位置 |
| 隐藏成员有位置,且**shown 恒等于锚** | ✅ 零映射 |

#### 推荐不变量(方案③)

1. **`members[0]`(锚)持有槽位,且它就是 shown**
2. `layout_node` 把 `members[1..]` 加入已有的 skip 集(我在 ②-1 已把它从
   `Option<u64>` 泛化成 `&[u64]`,所以这里零新增机制)
3. **循环 = 轮转 `members` 数组**,让新的 shown 成为新的 `members[0]`
   —— 这正是 zellij 堆叠头的语义:槽位不动,里面换内容
4. **锚被关闭 → 解散组**:其余成员各自恢复为普通 pane(它们本来就还带着自己的树位置)

#### 为什么这样就零映射

- 渲染:只有锚有 rect,锚渲染自己 —— **不用改**
- 事件路由:`active` 恒等于锚、也恒等于 shown —— **不用改**
- 光标:`active` 的 rect 就是对的那个 —— **不用改**
- 结构操作:作用于锚,也就是用户看到的那一个 —— **语义正确**
- 关闭:普通叶子删除 + 组内摘除 —— 无孤儿、无重定键

#### 因此真实的改动点(比 A.2 更少)

| 位置 | 改动 | 只有 |
|---|---|---|
| `LayoutTree` 字段/Default | `stacks: HashMap<u64, StackGroup>` | |
| `layout_node` 跳过集 | 合并 `members[1..]` | |
| 组操作 | `stack_merge` / `stack_cycle`(轮转)/ `stack_remove_member` | |
| `remove(id)` | 锚被删则解散组(其余成员自动恢复) | |
| `dump`/`restore` | 增 `stacks` 字段(与 `floats` 同批) | |
| `PaneMode::Pane` 的 `s` | 活动叶 + 树序下一个叶合并;已在组里则不重复入组 | |
| `PaneMode::Pane` 的 `p`/`P`/`Tab` | 活动叶在组内 → 轮转;否则保持“切下一个窗口” | |
| header 行 | 占锚的矩形顶 1 行,显示 `序号/总数` + 成员类型名 | |

**代价:槽位固定。** 堆叠永远显示在**锚最初的那个位置**,循环时内容就地切换。
这与 zellij 的堆叠头一致(堆叠占据一块区域,里面切 pane),所以不是妥协。
若以后要“堆叠跟着 focused 成员跑”,再引入 slot 与 shown 分离 + 三处映射 —— 到时参考 A.2。

