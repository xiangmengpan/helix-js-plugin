# 设计：原生终端视图（方案二-C）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

PTY 输出经 vte 解析成网格，原生终端视图组件渲染（光标/颜色/清屏/滚回），面板尺寸变化实时 TIOCSWINSZ。

## API

```js
helix.open_terminal({ cmd: "bash", side: "right", size: 44, onExit?: (code) => {...} })
```

- 打开一个**原生终端面板**（非 JS 文本渲染）：pty spawn + vte 网格 + 按键直通 + 尺寸联动
- 返回面板 id（可 move_panel 移动）；Esc/`:panel-close` 关闭（杀进程）
- 交互：面板是顶层时按键直通 pty（原生组件处理，无 JS onKey）

## 实现

- **依赖**：workspace 加 `vte = "0.15"`（轻量 VT 解析器，已核实可用）
- **helix-js**：
  - `js_open_terminal`（校验 cmd/side/size）→ 内部：spawn pty（复用 spawn 机制，onChunk = 桥接闭包调 `helix.term_feed(viewId, chunk)`，onExit = 用户回调）→ `UiRequest::OpenTerminal { view_id, pty_id, cmd, side, size }` → 返回面板 id
  - `helix.term_feed(view_id, chunk)` 原生函数（校验 → `UiRequest::TermFeed { view_id, chunk }`）
- **helix-term**：
  - 新组件 `PluginTerminal`（ui/plugin_terminal.rs）：
    - 持有 view_id、pty_id、`TerminalGrid`（cols×rows Cell{ch,fg,bg,bold} + cursor + 滚回行 + alt screen）
    - `feed(chunk)`：vte::Parser + Perform 实现（print/execute/csi_dispatch/esc_dispatch：光标移动、擦除、SGR 颜色、滚动、alt screen）
    - `render`：网格画到 surface（解析出的颜色 → tui Color）；尺寸变化 → `term_resize(pty_id, rows, cols)` + grid.resize（截断/填充）
    - `handle_event`：按键 → `term_write(pty_id, key)`（Esc 关闭面板——与面板一致）
  - drain：`OpenTerminal` → push PluginTerminal 层；`TermFeed` → 找 view → feed
  - 关闭：面板关 → kill pty
- **网格**：Cell { ch, fg: Option<tui Color>, bg, bold }；滚回保留最近 N（如 1000）行；宽字符暂不处理（PoC）

## 测试

- 单测（helix-term 网格）：feed "hello" → 网格前 5 格 "hello"；光标移动 CSI；SGR 颜色；清屏；alt screen 切换；resize 截断/填充
- 集成：`:term-native` 开原生终端面板 → 层存在；渲染进 Buffer 断言输出文本可见（feed 后渲染）
- helix-js 单测：open_terminal 校验与入队；term_feed 校验

## 非目标

- 滚回滚动交互（先存后显示最后 N 行）、选择/复制、宽字符/组合字符、256 色以外的真彩色细节（基础 ANSI 16 + 256 色索引）
- 终端内交互程序的光标闪烁/鼠标

## 涉及文件

- `Cargo.toml`（workspace vte）
- `helix-js/src/lib.rs`（js_open_terminal/js_term_feed/OpenTerminal/TermFeed/单测）
- `helix-term/src/ui/plugin_terminal.rs`（新，网格 + vte 解析 + 渲染 + 单测）
- `helix-term/src/commands/typed.rs`（drain）
