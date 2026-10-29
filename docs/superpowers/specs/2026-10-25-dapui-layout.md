# `dapui` 插件面板布局（细化）

> 依赖 [`2026-10-25-dap-api-p1.md`](2026-10-25-dap-api-p1.md) 的 API 面；主设计见
> [`2026-10-25-dap-redesign-design.md`](2026-10-25-dap-redesign-design.md)。
> 本文只回答"画什么、放哪、按什么键、什么状态显示什么"。

## 0. 面板能力的边界（先认账，再设计）

| 能力 | 现状 | 对设计的影响 |
|---|---|---|
| `side` | `right \| left \| bottom`（**无 top**） | 变量树放右、REPL/Output 放底 |
| tab 原语 | **没有** | 底 panel 的 `REPL / Output` 手绘一行 tab |
| 同侧多面板 | 语义未验证（`open_panel` 只是往布局树插叶子） | **v1 不依赖**：右 + 底各一个，不同侧 |
| 面板边框 | 由插件自己画（`el("box",{border,title})` 或 `draw.rect`） | 右面板用 `box(round, title:"DAP")` |
| 点击 | `draw.*` 画的格子**不参与交互**；只有 `el("button")` 能点 | v1 变量树**不支持点击**，纯键盘（与 filetree 一致） |
| 面板内输入 | `el("input")` 需要焦点路由（`focusable:true`） | REPL 依赖它；**若实测不生效**，退化为"`onKey` 自己攒字符 + 自绘输入行"（写在风险里，别静默坏掉） |

与既有占用者的关系：`tabbar` 用顶部槽位、`statusline` 用最底 1 行、`filetree` 用左侧、
`terminal` 的 `:vterm/:hterm` 用右/底。⇒ dapui 右面板与 `:vterm` 会争同侧空间，
打开顺序即嵌套顺序；默认 `panel_size = 34`，比 filetree(32) 略宽以容纳 `name = value` 长行。

## 1. 屏幕布局

```
┌──────────────────────── tabbar（TABBAR_ID）─────────────────────────┐
│ 编辑区                                │ ╭ DAP ────────────────────╮ │
│                                       │ │ Local            ▾ (3)  │ │
│                                       │ │   x = 42                │ │
│                                       │ │   vec = Vec(4)     ▸ (4)│ │
│                                       │ │ Registers         ▸     │ │
│                                       │ ╰─────────────────────────╯ │
├───────────────────────────────────────┴─────────────────────────────┤
│ ▸REPL  Output                          #1 stopped · breakpoint      │
│ (f) x + 1                                                          │
│ x + 1 => 43 (i32)                                                  │
│ ╭──────────────────────────────────────────────────────────────────╮│
│ │ > _                                                             ││
│ ╰──────────────────────────────────────────────────────────────────╯│
├──────────────────────────────────────────────────────────────────────┤
│ statusline（最底 1 行）· dap 段：`#1 ⏸ breakpoint`                   │
└──────────────────────────────────────────────────────────────────────┘
```

- **A 右 panel**：变量树。`side:"right", size:34, focusable:true, onKey, onClose`
- **B 底 panel**：REPL / Output（手绘 tab）。`side:"bottom", size:自适应, focusable:true, onKey`
  —— 实现见 v1.1：`bottom_rows()` 取 `min(bottom_size, 视口行数 × 40%)`，
  因为核心把 `size` 当"占多少行 / 当前视口行数"，小终端上写死 12 行会吃掉半个屏。
- **C 断点**：**不占面板**，用 Picker（`:dapui-breakpoints`）
- **D 编辑器内**：当前执行行高亮 + 行尾内联值（`set_highlight` / `set_virtual_text`）

## 2. 状态机：每个状态各面显示什么（禁止空白）

| 会话状态 | A 变量树 | B REPL/Output | D 编辑器内 | statusline `dap` 段 |
|---|---|---|---|---|
| `no-session` | `no session`<br>`:debug-launch 启动`（灰） | REPL 可输入但会 reject（红字回显） | 无 | **不显示**（不占宽） |
| `starting` | `Starting lldb-dap …`<br>进度条（`dap-progress`） | Output tab 自动切前台 | 无 | `#1 ⏳ starting` |
| `running` | 上次栈帧灰显 + `Running…` | REPL 可用 | 清高亮、清内联值 | `#1 ▶ running` |
| `stopped` | 重拉栈帧/作用域，**默认展开根作用域一级** | 切回 REPL tab | 写高亮 + 内联值（若开启） | `#1 ⏸ <reason>`（breakpoint/exception/step） |
| `terminated` | 保留最后内容，标题加 `[terminated]` | 保留 Output | 清高亮/内联值 | `#1 ■ exited(<code>)`，3 秒后消失 |

实现位置：状态判断只读 `helix.dap.sessions()`（快照，零成本），**不在插件里自己维护状态机**——
事件只用来触发"重拉"，真相永远在核心（避免两处状态漂移）。

## 3. A 面：变量树

**渲染方式**：`draw.*` 立即模式（同 filetree），`return helix.draw.done()`。理由：树是"每帧全量重画"，
`draw` 比 `el` 树少一层解析；代价是不支持点击（v1 明确不做）。

**节点模型（插件内部）**

```js
Node = { label, value, ref,          // ref>0 才可展开（variablesReference）
         named, indexed,             // 计数徽标 `(3)` / `[4]`
         children: [] | null,        // null = 未加载
         expanded: bool, loading: bool, depth }
```

**行格式**（列宽固定，便于扫读）

```
<缩进 2×depth><▸|▾|空格> <label> = <value><徽标>
  缩进   折叠符(叶子用空格)  名字        值      ` (3)` / ` [4]`
```

- 超长截断：value 超过 `剩余宽度` 时右侧 `…`（保留名字完整，值更重要是"能看到类型"⇒ 用 `helix.text.truncate`）
- 样式：容器名 `ui.text`、叶子值 `ui.virtual`、`<optimized out>` / 加载中 `…` 用 `ui.virtual` + 斜体修饰
- 选中行：整行 `ui.selection`（`draw.fill` 该行再 `draw.text`）

**懒加载规则**

| 动作 | 行为 |
|---|---|
| 展开（`l`/`→`/`Enter`） | `ref>0 && !children` ⇒ 置 `loading`，`helix.dap.variables(ref)` → 填 `children` |
| 折叠（`h`/`←`） | 只改 `expanded`，**不丢** children（下次展开零延迟） |
| 自动展开深度 | `variables_max_depth`（默认 2）：`stopped` 时按层级展开前 2 层 |
| 节点上限 | `max_nodes`（默认 2000）：超出在末尾画 `… (truncated)`，并**不再发请求** |
| 刷新（`R` 或 `dap-stopped`） | 重拉 `stack_frames` + 根 `scopes`；按 `name` 路径恢复展开状态（路径不存在的节点自然丢） |

**线程/栈帧切换**：树顶部固定两行虚拟节点：
`Thread 1 ▸` / `Frame #2 main.rs:42`，用 `t` / `f` 打开 Picker（`threads()` / `stack_frames()`），
选中后重拉作用域。

## 4. B 面：REPL / Output（手绘 tab）

**第 1 行 = tab 条 + 会话徽标**

```
▸REPL  Output                        #1 stopped · breakpoint
^^^^active（▸ 前缀 + ui.selection）
```
- `Tab` / `S-Tab` 切换；`1` / `2` 直达
- 右侧徽标 = active 会话摘要；多会话时显示 `#1/2`，`c` 切换（Picker）

**REPL tab**
- 历史区（`el("scroll", {height: 剩余行})`）：每条一行
  - 输入回显：`(f) <expr>`（`ui.virtual`）
  - 成功：`<expr> => <result> (<type>)`
  - 失败：`<expr> !! <message>`（`error` scope）— **不 `echo`**，避免状态栏刷屏
- 输入行：`el("input", {id:"dapui-repl", placeholder:"> "})`
- `Enter` ⇒ `helix.dap.eval(expr, 当前 frame)`；空串忽略
- `↑` / `↓` 翻历史（最多 200 条，去重相邻重复）
- 求值上下文：有 stopped frame 用 frame，否则用 active 会话的最近 frame；都没有 ⇒ 红字 `no frame`

**Output tab**
- `helix.on("dap-output")` 攒环形缓冲（500 行，超出丢最旧）
- 按 `category` 上色：`stdout` 正常 / `stderr` `error` / `console` `ui.virtual` / `telemetry` 灰
- 自动跟随尾部；用户 `↑`/`PageUp` 翻看后**停止跟随**，按 `G` 回尾部

## 5. C 面：断点（Picker，不占面板）

`:dapui-breakpoints` → `Picker` 列 `path:line  [cond]  verified?`：

- `Enter` 跳转（`helix.open_file(path, {row})`）
- `d` 删除、`c` 改条件、`l` 改日志消息（走 Prompt，`set_breakpoint` 重设）
- 开关一条日志断点时先查 capability（核心已在 `breakpoints_changed` 里检查，这里只负责把错误显示出来）

## 6. D 面：编辑器内

| 时机 | 动作 |
|---|---|
| `dap-stopped` | **一次写齐**（Rust 侧按文档整组替换，分两次写会互相清）：<br>① `set_virtual_text(path)` = Clear（VT + highlight 一起清）<br>② `set_highlight(path, row, 0, row+1, 0, scope)` 当前执行行（`highlight_current_line`，默认**开**）<br>③ 若 `inline_values`：对已加载作用域的前 8 个变量写行尾内联值（默认关） |
| `dap-continued` / `dap-terminated` | `set_virtual_text(path)` 清 + 清高亮（`set_highlight` 传空区间或 `style:null`） |
| 切会话（多会话） | 先清旧会话的装饰，再按新 active 写 |

约束：
- `inline_values` 默认 **false**（每帧虚拟文本有渲染成本，且对噪声敏感）
- `highlight_current_line` 默认 **true**；scope 默认 `dap.current-line`，主题没定义时按
  `ui.cursorline.primary` / `ui.selection` 的底色**派生注册**（`set_theme` 合并语义，不抹别人）
- 装饰的时机：`T.ctx.source/line` 来自 **异步**的 `stack_frames` ⇒ 装饰在栈帧回来之后重写
  （在事件 handler 里同步调会拿到 `undefined`，什么都不画 —— 这是实现时踩过的坑）
- 内联值只对**当前文件**的变量写（跨文件变量不猜位置）；`available` 的 scope 里只取 `Local`
- `path` 必须是绝对路径（`dap-stopped` 时从 frame 的 `source.path` 取）

## 7. 键位总表

**全局（编辑器，插件注册命令 + `init.js` 里绑）**

| 键 | 命令 | 说明 |
|---|---|---|
| `<space>g d` | `:dapui-toggle` | 开/关 dapui（右 + 底一起） |
| `<space>g B` | `:dapui-breakpoints` | 断点 Picker |
| `<space>g R` | `:dapui-repl` | 聚焦 REPL 输入 |

> 现有 DAP 组是 `space-g`（`l/r/b/c/h/i/o/n/v/t/…`，`default.rs:216-236`），保持不动。

**A 面（变量树，`onKey`）**：`j/k` `↑/↓` 移动 · `l` `→` `Enter` 展开/内联求值 · `h` `←` 折叠 ·
`t` 切线程 · `f` 切栈帧 · `R` 重拉 · `g/G` 首/尾 · `Esc`/`q` 关面板 · `?` 帮助

**B 面（REPL/Output）**：`Enter` 执行 · `↑/↓` 历史/滚动 · `Tab` `S-Tab` 切 tab · `1`/`2` 直达 ·
`G` 回尾部 · `Esc` 焦点回编辑器（`helix.pane.focus(0)`）

**通用约定**（照 filetree，`filetree/plugin.js:546-552`）：
- `key.ctrl || key.alt` ⇒ **`return "ignore"` 穿透**（面板非模态，`: 命令` / 自定义 `C-*` 必须照常生效），
  唯一例外是 `C-\`：焦点回编辑器（保留面板）
- 未映射键 ⇒ `"ignore"`（不是 `"handled"`）

## 8. 配置（`[plugins.dapui]`）

```toml
[plugins.dapui]
auto_open       = true    # 首次 dap-stopped 时自动开右面板（底面板独立开关）
auto_open_repl  = false
panel_size      = 34      # 右面板列数
bottom_size     = 12      # 底面板行数
inline_values   = false   # 行尾内联值（有渲染成本）
variables_max_depth = 2   # stopped 时自动展开层数
max_nodes       = 2000    # 变量树节点上限
```

全部**每帧读**（`current_cfg()`，`:config-reload` 即时生效）——与 `statusline` 插件同一写法。

`auto_open` 的例外：会话在**没有变量可看**的状态（`starting`）不自动开，避免闪一下。

## 9. 空态 / 错误态（不许静默空白）

| 场景 | 显示 |
|---|---|
| 面板开着但没有会话 | A：`no session` + `:debug-launch 启动`（灰）；B 的 REPL 仍可输入 |
| adapter 起不来 | A 顶部红字一行（`error` scope）：`adapter 启动失败: <msg>`，保留到下次 stopped |
| `eval` reject | REPL 历史里红字一行（不 `echo`） |
| `variables` reject | 该节点下画 `!! <message>` 一行（可 `R` 重试），**不吞掉** |
| 超过 `max_nodes` | 末行 `… (truncated, max_nodes=2000)` |

## 10. 可测性（`node --test`，桩注入 `global.helix`）

照 `plugins/statusline/tests/` 的写法（`require` 前注入 `global.helix`，捕获 `open_panel`/`set_highlight`/
`set_virtual_text`/`on` 的回调）。断言：

1. `dap-stopped` ⇒ A 面渲染出现 `Local` 根节点、D 面写了高亮；`dap-continued` ⇒ D 面清了内联值
2. 展开节点 ⇒ 只发一次 `variables(ref)`（再展开用缓存）
3. `eval` reject ⇒ REPL 历史多一行红色错误（且没调 `echo`）
4. `output` 500 行后环形丢最旧（长度恒 ≤ 500）
5. `max_nodes` 截断行出现且不再发请求
6. `Tab` 切换 tab 只改本地状态、不重新拉数据

## 11. P1 不做（避免范围膨胀）

- 点击交互（`draw` 不支持，要做就换 `el("button")` 并重新评估性能）
- 独立 `Watches` tab / 变量值编辑（`setVariable`）/ 内存与反汇编
- 多会话**并排**树（只显示 active，多会话靠 `#id` + `c` 切换）
- 折叠状态持久化到磁盘（跨会话恢复）

## 12. 一个前置依赖：statusline 的 `dap` 段需要新通道

**事实**：`statusline` 插件的段是**闭集**（`plugins/statusline/plugin.js` 里的 `const SEGMENTS = {...}`），
其他插件无法往里加段；`helix.set_statusline` 的**追加模式**也被 `replace: true` 短路
（`helix-term/src/ui/statusline.rs:207` 的全替换分支直接 `return`，后面的 append 分支永不执行）。

⇒ 三条路，必须选一条：

| 方案 | 代价 | 评价 |
|---|---|---|
| **① 不碰状态栏** | 零 | 调试状态只能靠面板 tab 条与 `:debug-*` 的 `set_status`；状态栏永远看不到 |
| **② 核心开一条段注册通道** `helix.register_statusline_segment(name, fn)` | ~30 行 Rust（照 `set_statusline` 的 per-host 存储）+ statusline 里 `SEGMENTS[name] || EXTERNAL[name]` + d.ts + 文档 + `api-check` | 与本仓“核心留原语”一致；顺带给以后的插件（server 状态、AI 进度）留出口 |
| **③ dapui 自己 `set_statusline` 追加** | 0 行 | **不可行**：与 statusline 插件的 `replace:true` 互斥（见上） |

**建议 ②**，且把它放进 **P1 的前置任务**（否则 §2 表里的 `dap` 段是画不出来的）。
若不想在 P1 动核心，则本文件 §2 的 `dap` 段一列整列删掉（其余设计不受影响），状态栏集成推到 P2。

> **已定（2026-10-25）：选 ②。** 施工细节见
> [`../plans/2026-10-25-dap-redesign.md`](../plans/2026-10-25-dap-redesign.md) 的 **P1-T0**：
> 新增 `helix.register_statusline_segment(name, fn|null)` + `helix.statusline_segment(name, ctx)`，
> 内置段名优先、按属主回收、**段级容错**（单段抛错不影响整行）。

## 13. v1.1 落地记录（2026-10-25）

规格里"REPL 在底部第二面板 + 手绘 tab"已实现，v1 的过渡方案（REPL 挤在右面板 + 模态弹窗）**已删除**，
现在只有**一份** REPL 实现：

| 面 | 位置 | 键位 |
|---|---|---|
| 变量树 | 右 panel（`size = panel_size`） | `j/k/↑↓` 移动 · `l/Enter` 展开 · `h` 折叠 · `R` 重取 · `q/Esc` 关 |
| REPL | 底 panel 的 `REPL` tab | 可打印字符进输入 · `Enter` 求值 · `↑↓` 历史 · `Tab` 切 tab · `Esc` 关 |
| Output | 底 panel 的 `Output` tab | `j/k` 滚动（停跟随） · `g/G` 首/尾（`G` 恢复跟随） · `Tab` 切回 |

- 命令：`:dapui`（右面板开关）· `:dapui-repl`（开底面板→REPL）· `:dapui-output`（开底面板→Output）
  · `:dapui-breakpoints`（断点 Picker）
- 配置新增 `bottom_size`（期望高度，最终会被 40% 视口上限收住）与 `auto_open_repl`（默认 false）。
- 验证：node 14 条（tab 切换 / REPL 求值与失败 / 历史 / output 环形 500 / stderr 样式 / 状态栏段 /
  键位穿透 / 命令齐备）+ Rust 集成 2 条（真引擎里开面板 + tab 条渲染）。

## 14. v1.1 补：当前执行行高亮（2026-10-25）

规格 §2 的"D 面"此前只做了内联值（默认关），缺了当前行高亮。现已补齐：

- `[plugins.dapui] highlight_current_line = true`（默认开）+ `current_line_scope = "dap.current-line"`。
- scope 解析顺序：主题定义了 `dap.current-line` → 直接用；否则取 `ui.cursorline.primary`（再退
  `ui.selection`）的 **bg** 经 `set_theme` 派生注册；都没有 → 借用 `ui.cursorline.primary` 名字（不注册）。
  换主题时清缓存重算（依赖 `theme-change` 事件）。
- 清装饰只发一条 `set_virtual_text(path)`（= Clear，VT 与 highlight 一起清）：
  `dap-continued` / `dap-terminated` / `dap-exited` 都走它。
- 验证：node 5 条（默认开 / 关掉不写 / 派生 scope / 换主题重算 / 继续执行清装饰）。

## 15. v1.2：断点进底部面板第三 tab + 变量树 `t`/`f`（2026-10-25）

### 15.1 为什么把断点从 Picker 挪到底部面板

§5 的 C 面原先写的是"Picker + `d`/`c`/`l`"。**这条键位在平台上不可实现**：

- 核心合同（`helix-term/src/ui/picker.rs:1304-1306`）：核心不认识的键才先问插件的
  `PickerOptions.onKey`，而**纯字符键（无 Ctrl/Alt 的 Char）永不交给插件**——拾字安全阀。
  `d`/`c`/`l` 都是纯字符 ⇒ 插件根本收不到（旧实现里那个 `onKey` 判断 `e.key === "d"` 是死代码）。
- 面板/弹层的 `onKey` 收全部键（REPL 就是靠这个攒输入），所以断点列表放**底部面板的第三个 tab**
  （`REPL | Output | Breakpoints`）：既不新占屏幕，又拿得到完整键位。

`:dapui-breakpoints` 仍是那个命令名，行为改成"开底面板 → 切到断点 tab"。

### 15.2 键位

| 面 | 键位 |
|---|---|
| 断点 tab | `j/k/↑↓` 移动 · `g/G` 首/尾 · `Enter` 跳到断点行 · `d` 删 · `c` 条件 · `l` 日志消息 · `e` 异常断点 · `R` 报数量 · `Esc` 关面板 |
| tab 条 | `Tab` 循环三页 · `1/2/3` 直达（REPL 输入非空时 `1/2` 仍让给输入） |
| 变量树（新增） | `t` 切线程 · `f` 切栈帧 · `s` 切会话 |

`c`/`l` 的输入用**自己画的弹层**（`helix.open_popup` + `el` 树，与 filetree 的 `prompt` 同款）：
核心没有 JS 的 `prompt` API。空串提交 = 清除该项；改一项时另几项**原样带上**——核心
`set_breakpoint` 对 `opts` 是整组覆盖，少传就是清空。

### 15.3 `t` / `f` 的语义

只影响**本面板**的查看与求值（`T.ctx.thread/frame` → 后续 `stack_frames`/`scopes`/`variables`/
REPL 的 `eval(expr, frame)` 都用它），不动引擎的 stepping 线程/活动帧。
`dap-stopped` 到达时**不带 keep**，回到引擎给的线程/帧（手动选择不会被黏住）。

### 15.4 顺带修的核心缺口

`helix.dap.set_breakpoint` / `remove_breakpoint` / `clear_breakpoints` 之前也要求"有活动会话"，
而 `<space>G b` 不需要 —— 于是**先打点后启动**这个最常见流程在 API 侧是坏的（`dapui` 断点页改条件
会报"没有活动调试会话"）。现在这三条在无会话时**只改本地断点表**并成功兑现，
有会话才顺带推 adapter；真正启动后由 `initialized` 事件全量重放。

### 15.5 验证

- node 26 条（新增：三 tab 循环、断点页渲染/删除/跳转/改条件带 log+hit、改日志清空、
  `t` 多线程弹层与单线程不弹、`f` 用所选帧拉 scopes、`dap-stopped` 回归引擎线程）。
- 真二进制（`~/.local/bin/hx`，HEAD）pty 实测：离线打点 → `:dapui-breakpoints` 看到
  `/tmp/wkhx/sample.rs:1 ◯` → `c` 弹"条件 sample.rs:1" → 输入 `x>1` ⇒ 行变 `◯ · cond: x>1` →
  `d` ⇒ `(没有断点…)` 且 gutter 标记同步消失。

## 16. v1.3：多会话切换 / 异常断点 / 断点随编辑移动（2026-10-25）

这一轮把 §11「P1 不做」之外的三个真缺口补上了，并修了两个**原来的错误行为**。

### 16.1 多会话切换 UI（`s`）

- 树里 `s` → 列出 `sessions()`（`#id ⏸/▶/■ 适配器名`），选中即 `helix.dap.switch_session(id)`。
- 核心新增 `helix.dap.switch_session(id)`：**纯本地状态**（`debug_adapters.set_active_client`），
  同步改完 → 刷快照 → 兑现 Promise（所以 `.then` 里读 `sessions()` 已经是新 active）。
- 只有 1 个会话时不弹层，只提示（与 `t`/`f` 同一策略）。

### 16.2 异常断点（`e`）

- 断点页 `e` → 多选弹层（`Space` 勾选 / `Esc` 保存关闭）→ `helix.dap.set_exception_breakpoints(picked)`。
- 过滤器来自 adapter 的 capabilities：会话快照里新增 `filters: [{filter,label,default}]`
  （`dap_sessions_json` 直接读 `client.caps.exception_breakpoint_filters`），
  默认勾选按 capabilities 的 `default`。
- adapter 一个过滤器都没有时只提示，不弹空层。

### 16.3 断点随编辑移动（上游那条 `TODO: need to map breakpoints over edits`）

新增 `helix-view/src/breakpoint_remap.rs`：

1. 钩子 `helix-term/src/handlers/dap_breakpoints.rs` 在 `DocumentDidChange`（带
   `old_text` + `changes`）时排队 —— 那个事件**没有 `&mut Editor`**，加字段会波及所有钩子；
2. 事件循环里 `dispatch_dap_intents` 排空队列，用 `ChangeSet::map_pos(行首, Assoc::After)`
   算新行号，搬动断点（条件/日志保留，`verified/id/message` 清空等重推）；
3. 有移动的文件自动入队 `DapIntent::SyncBreakpoints`。

**顺带修的调度 bug**：`dispatch_dap_intents` 原先在 `pump_term_events` 的**早退判断之后**，
而普通编辑不产生 term/LSP 事件 ⇒ 队列永远排不空（现象：插一行后断点不跟走）。
现在移到早退之前，同时让命令里入队的 `continue/next` 不再依赖"恰好有别的事件"。

### 16.4 两个错误行为的修正

| 现象 | 根因 | 修法 |
|---|---|---|
| 按 `t` 时面板被"面板 1 状态丢失，已自动关闭"关掉 | `PluginPanel::handle_event` 把 **onKey 抛错**和**注册表丢失**都当僵尸自愈；真错误是 `helix.dap.threads(1)` 抛 `argument must be an object` | ① `Err` 分支先查 `popup_exists_in(host, id)`：还活着 ⇒ 把**真实 JS 报错**显示到状态栏、面板留着；② `threads(id?)` 按 `helix.d.ts` 的契约接受**数字**（也保留对象写法）|
| 活性检查查错表 | `panel_has_onkey`/`popup_exists` 走"隐式活性宿主"解析，没有 active host 时会**新建空宿主** | 有 `cx` 的调用点一律走新加的 `panel_has_onkey_in(host.state(), id)` / `popup_exists_in(...)`（`cx.with_js`） |

### 16.5 验证

- Rust：`helix-view` 79（含 remap 7 条）· `helix-js` 247（含 `threads()` 数字契约 1 条）· `cargo xtask api-check` 全绿。
- node：`dapui` 30（新增 `s`/`e` 4 条）· `which-key` 12 → 共 42。
- 真机（`~/.local/bin/hx`，HEAD，`-gdwarf-4` 编译的 C 程序 + 真 lldb-dap）：
  断点命中 `#1 ⏸` → `f` 列出 `#524288 bar` / `#524289 main` / `#524290 __libc_start_call_main` →
  `t` 列出双线程、选中后 `dapui: 看线程 #45990` → `s` 切会话 `dapui: 切到会话 #2` →
  断点页 `e` 列出 `C++ Catch/Throw`、勾选后 `dapui: 异常断点: cpp_catch` →
  在断点上方插一行后 gutter 与断点页都变成 `remap.rs:2`。

### 16.6 收尾：`commands/dap.rs` 里三条老 TODO 的处理

| TODO | 处理 |
|---|---|
| `edit condition` / `edit log` 的**偏移稳定性**（"adapter 回填会改行号"） | ✅ 真正修了：新增 `edit_breakpoint_field()`，**优先按 adapter 分配的 `id` 定位**（`BreakpointStore::update_by_id`），只有还没起会话（没有 id）才回退到 `(path, line)`；断点已被 adapter 移除时报「断点已不存在」，不再静默改错行 |
| 起会话后「等 `initialized` 再放行命令 / 缓冲命令」 | ❌ 明确**不缓冲**：未就绪时每条命令都有明确报错（`调试会话还没就绪（adapter 启动中）`），缓冲会让"几秒前的 next 突然执行"，更难理解；真正的初始化动作已在 `Event::Initialized` 里做完（断点重放 + `configurationDone`） |
| 「避免重复拉配置」 | ✅ 已由 8208d0225 的**配置快照**解决（不再每帧读盘），剩下的只是 O(模板数) 的名字查找；注释已更新，不再是 TODO |

### 16.7 顺带发现并修掉：展开**非当前帧**节点会抛错

新的"onKey 报错要如实显示"（§16.4）把这条藏了很久的问题照了出来：

```
面板 1 onKey 出错: helix.dap.variables: 'reference' is required at expand_node
```

根因：帧节点只有 `frame` 没有 `variablesReference`，而 `expand_node` 一律调 `variables(node.ref)`。
现在按节点类型分派：有 `ref` → `variables`；只有 `frame` → `scopes(frame)`；两者都没有 → 摘掉
`expandable` 静默忽略（不再抛）。顺带把 scopes/variables → 树节点的映射抽成
`map_scopes()` / `map_variables()` 两处共用。

真机验证：停在 `bar` 后按 `G` 跳到最后一个帧节点 `_start`，Enter 展开出
`Locals / Globals / Registers`（此前是报错）。

## 17. 与 which-key / 布局的相处（2026-10-25）

### 17.1 提示框不许盖住布局叶子（which-key 侧改）

原来 which-key 固定 `bottom-right` ⇒ dapui 的右面板 + 底面板正好在那一角，三层糊在一起。
现在（`[plugins.which-key] avoid_panels = true`，默认开）：

1. 读 `helix.pane.list()`，取所有 **PluginPanel** 的 `area`（绝对单元格几何）；
2. 估算提示框尺寸（行数 × 最长行 + 2，与核心 Info 同口径）与视口；
3. 首选 `position`，与面板重叠就按 `bottom-right → bottom-left → top-right → top-left` 换角，
   全重叠取重叠面积最小；**没有面板时行为与以前完全一致**（可配 `avoid_panels = false` 回到老行为）。

为什么不把两者"合并"成一个东西：它们不是一类层 —— which-key 是**提示**（Info 浮层、随时出现/消失），
dapui 是**布局叶子**（占位、有焦点、有几何）。正确关系是"提示让位给叶子"，而不是塞进同一个框。

### 17.2 焦点必须能一键回来（dapui 侧改）

用户反馈：调试时面板吃掉焦点，没法回编辑器看代码。根因有两个：

| 根因 | 说明 | 修法 |
|---|---|---|
| `C-\` 在传统终端编码里 = `0x1C` = **`C-4` 同一码** | 本机 termina 解析成 `name:"4", ctrl:true`，所以 `key.name === "\\"` 永远不成立 | 两个名字都认（`is_focus_editor_key`） |
| 硬编码 `pane.focus(0)` | 编辑器 pane 的 id 是**布局给的**，不是契约 | 从 `pane.list()` 里按 `kind === "EditorView"` 找 id，找不到才退回 0 |

并且把"离开"变成**每位用户都能撞到**的键：

- `Esc`（变量树 / 底部三页）= **焦点回编辑器，面板保留** —— 调试时"看一眼代码再回来"的主路径；
- `q`（非 REPL 页）= 关面板；REPL 里 `q` 是输入字符，关面板用新命令 **`:dapui-close`**（两个面板一起关）；
- `C-\` / `C-4` 仍然可用；底部断点页页脚也写了 `Esc 回编辑器`。

`C-p`（窗口模式）→ `h/j/k/l` 从面板移动焦点也照常可用（面板对未映射的修饰键返回 `ignore` 穿透）。

**反向：怎么进面板？** auto_open 现在**不抢焦点**（`focus_on_auto_open = false` 默认）——
命中那一刻你要的是继续看代码/按 step，而不是先按 Esc 逃出来。进面板有两条路：

1. `:dapui-focus` → `pane.focus_dir(编辑器id, "right")`（面板是布局叶子，用既有的方向聚焦即可，
   不必给核心加"按面板 id 聚焦"的新 API）；
2. `C-p` + 方向键。

关面板：`q`（非 REPL 页）或 `:dapui-close`。

### 17.3 提示的两种形态：浮层（可避让）与一行摘要

§17.1 解决了"浮层压面板"，但浮层本身在调试布局里还是碍眼（变量树 + 底面板 + 提示三层）。
所以 which-key 增加了两个纯展示层的开关，核心只加了一个原语：

| 配置 | 行为 | 核心依赖 |
|---|---|---|
| `mode = "box"`（默认） | Info 浮层；`avoid_panels` 按 pane 几何换角 | 无（原能力） |
| `max_lines = 18` | 浮层最多 N 行，超出显示「… 还有 N 个键」 | 无 |
| `mode = "line"` | **不出浮层**，状态栏 `echo` 一行摘要（`Space ▸ f 文件选择器 · … +21`） | 新增 `set_keymap_hint → { hide: true }`（`KeymapHint::Hide` → `hint_info()` 返回 `None` → 不挂 `autoinfo`）|

设计取舍：**提示不是内容**，所以它既不该压布局叶子（避让），也可以完全不占画面（line 模式）。
摘要只列前 6 项 + 余数，避免把状态栏挤爆；同一行内容去重（sticky 子菜单每次按键都会重算）。

实测（44×170 pty，`--config` 切模式）：

```
mode=line    按 space → 浮层行号集合为空；状态栏: 「Space ▸ f Open file picker · F … · …」
mode=box     按 space → 8 行: f/F/e/./b/j/s + 「… 还有 21 个键」,位置右下(无面板时)
```
