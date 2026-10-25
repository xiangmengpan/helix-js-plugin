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
