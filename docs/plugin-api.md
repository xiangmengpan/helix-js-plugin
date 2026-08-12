# Helix JS 插件 API 参考

> 本文档描述 helix 魔改版的 JavaScript 插件系统完整 API。插件为 `.js` 文件，放在 `~/.config/helix/plugins/`（启动时自动加载），或通过 `:plugin-load <path>` 手动加载。

## 目录

- [1. 加载与生命周期](#1-加载与生命周期)
- [2. 命令注册与消息](#2-命令注册与消息)
- [3. 文档读写](#3-文档读写)
- [4. 选区与光标](#4-选区与光标)
- [5. Shell 执行](#5-shell-执行)
- [6. 事件钩子](#6-事件钩子)
- [7. 键位绑定](#7-键位绑定)
- [8. 弹窗](#8-弹窗)
- [9. 侧边面板](#9-侧边面板)
- [10. 界面定制](#10-界面定制)
- [11. 主题](#11-主题)
- [12. 插件管理](#12-插件管理)
- [13. 已知限制](#13-已知限制)

---

## 1. 加载与生命周期

- **唯一入口**：启动只自动加载 `~/.config/helix/init.js`（配置根；兼容旧位置 `plugins/init.js`）；其他插件脚本必须经 `helix.load` 导入（不再全量扫描 `*.js`）
- `:plugin-load <path>` 可手动加载任意文件
- 每个插件在独立 IIFE 作用域求值（顶层 `let`/`const` 不跨插件共享；`helix` 全局对象除外）
- `:plugin-reload` 清空全部插件状态（命令/处理器/钩子/主题覆盖）并按加载顺序重跑；模块文件**重读磁盘**（init.js 本体重跑记录文本）
- `helix` 全局对象是唯一的宿主 API 入口

```js
// ~/.config/helix/init.js —— 唯一入口示例
helix.load("icons.js");                       // 立即导入
helix.lazy("terminal.js", "term");            // 懒加载：首次 :term 才加载
```

## 2. 命令注册与消息

### `helix.register_command(name, fn, doc?)`

注册一个 `:name` 命令。`doc`（可选字符串）会在命令行输入 `:name` 时显示在提示区。

```js
helix.register_command("hello", (ctx) => {
  helix.echo("world");
}, "打印 world");
```

- `fn(ctx)` 的 `ctx` 见 [§3 文档对象](#3-文档读写) / [§4 选区光标](#4-选区与光标)
- 命令内可编辑文档、移动光标、开弹窗/面板；全部效果在命令返回后原子应用（一个命令 = 一次撤销）

### `helix.echo(text)`

在状态栏显示消息。

```js
helix.echo("hello");
```

### `helix.map(mode, key, command)`

绑定键位。见 [§7](#7-键位绑定)。

## 3. 文档读写

命令的 `ctx.doc` 是**只读快照 + 编辑队列**：

```js
ctx.doc.path      // string | null，文件路径
ctx.doc.text      // string，全文
ctx.doc.cursor    // { row, col }，主光标位置（0-based）
ctx.doc.insert(row, col, text)     // 插入
ctx.doc.replace(sr, sc, er, ec, text)  // 替换区间
ctx.doc.delete(sr, sc, er, ec)     // 删除区间
```

- 坐标为 0-based 行列，基于命令开始时的**原始快照**（多次编辑互不偏移）
- 越界自动 clamp；一次命令的所有编辑 = 一个事务 = 一次撤销
- 编辑后游标被自动重映射（在插入处插入后，游标落在插入文本之后）

```js
helix.register_command("stamp", (ctx) => {
  ctx.doc.insert(ctx.cursor.row, ctx.cursor.col, new Date().toISOString());
});
```

## 4. 选区与光标

```js
ctx.selection.anchor      // { row, col } 选区锚点
ctx.selection.head        // { row, col } 选区头（点光标时与 anchor 相等）

helix.set_cursor(row, col);                              // 移动主光标
helix.set_selection(anchorRow, anchorCol, headRow, headCol);  // 设置选区
```

- 写操作队列化，命令返回后应用（快照坐标；与编辑同语义）
- 光标请求在编辑事务之前应用——编辑的重映射会正确推进光标

```js
helix.register_command("upper", (ctx) => {
  const a = ctx.selection.anchor, h = ctx.selection.head;
  if (a.row !== h.row) { helix.echo("single-line only"); return; }
  const line = ctx.doc.text.split("\n")[a.row];
  const lo = Math.min(a.col, h.col), hi = Math.max(a.col, h.col);
  ctx.doc.replace(a.row, lo, a.row, hi, line.slice(lo, hi).toUpperCase());
});
```

## 5. Shell 执行

### `helix.run(cmd)` — 同步

执行 `sh -c cmd`，返回 stdout 字符串。失败（非零退出码/启动失败）**抛错**，`message` 含 stderr。

```js
try {
  const branch = helix.run("git branch --show-current");
  helix.echo("branch: " + branch.trim());
} catch (e) { helix.echo("failed: " + e.message); }
```

> ⚠️ 同步阻塞编辑器主线程——只用于短命令。输出截断到 64KB（超出附 `(truncated)` 标记）。

### `helix.run_async(cmd, cb)` — 异步（完成回调）

```js
helix.run_async("git status", (err, out) => {
  if (err) helix.echo("failed: " + err);
  else helix.echo(out.trim());
});
```

不阻塞编辑器；回调在事件循环中执行。

### `helix.spawn({ cmd, pty?, onChunk, onExit })` — 流式进程

返回句柄 id。实时接收输出块、可写 stdin、可杀。

```js
const id = helix.spawn({
  cmd: "bash --norc --noprofile",
  pty: true,                       // 可选：true = 跑在真实 PTY 下（交互式/curses 程序可用）
  onChunk: (chunk) => { /* 输出块（stdout+stderr 合并，UTF-8） */ },
  onExit: (code) => { /* 退出码 */ },
});
helix.term_write(id, "ls\n");      // 写 stdin（或 PTY master）
helix.term_kill(id);               // 杀进程
helix.term_resize(id, rows, cols); // 仅 PTY：设置窗口尺寸（默认 24×80）
```

### 回调内可以做什么

所有异步回调（run_async cb / onChunk / onExit）在主线程执行，回调内可以：
- `helix.echo`、编辑文档（`ctx.doc` 不可用——回调无 ctx；但可调用命令、开弹窗/面板、`helix.run_async` 链式）
- 修改模块级状态（供弹窗/面板 render 读取）

## 6. 事件钩子

### `helix.on(event, fn)`

| 事件 | 触发时机 | 回调签名 |
|------|---------|---------|
| `save` | 保存**前**（编辑先应用再保存 → format-on-save） | `(doc)` |
| `mode-change` | 模式切换后 | `(mode, doc)`，mode: `"normal"\|"insert"\|"select"` |
| `buffer-open` | `:open` 打开文件后 | `(doc)` |
| `buffer-close` | 关闭（quit/force_quit 路径）前 | `(doc)` |
| `doc-change` | 文档文本变化，编辑停顿 250ms 后（idle 防抖） | `(doc)` |

- `doc` 与命令 `ctx.doc` 同构（含 cursor 与编辑方法）
- 同事件可注册多个处理器，按注册顺序调用；处理器抛错 → 状态栏报错，不阻断主流程
- `save` 处理器编辑 = 保存前变更（一次撤销）

```js
// format-on-save：保存时清理行尾空白
helix.on("save", (doc) => {
  const lines = doc.text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const t = lines[i].replace(/[ \t]+$/, "");
    if (t !== lines[i]) doc.replace(i, t.length, i, lines[i].length, "");
  }
});
```

## 7. 键位绑定

### `helix.map(mode, key, command)`

| 参数 | 说明 |
|------|------|
| `mode` | `"normal" \| "insert" \| "select"` |
| `key` | 键序列：单键 `"K"`、修饰键 `"C-n"`、空格分隔多键 `"C-n gd"`、逐字符多键 `"gd"` |
| `command` | 命令名字符串（内置或已注册插件命令）**或** JS 回调（自动注册为隐藏命令 `__mapped_N`） |

```js
helix.map("normal", "W", "show-where");          // 绑定插件命令
helix.map("insert", "C-n", () => helix.echo("hi"));  // 绑定回调
helix.map("normal", ";", "fileicon");            // 覆盖内置键位
```

- 重复绑定 = 覆盖；绑定立即生效；重启失效（插件启动时重新注册）
- 回调收到 `(ctx)`（同命令 ctx）

## 8. 弹窗

### `helix.open_popup({ render, onKey?, onClose?, width?, height?, position? })`

JS 渲染的覆盖层弹窗。

```js
helix.open_popup({
  width: 40, height: 10,                     // 可选：尺寸上限（clamp）
  position: { row: 5, col: 10 },             // 可选：屏幕锚点
  render: () => [
    { text: "Error: ", style: "error" },     // 样式行：style = 主题 scope 名
    "普通行",                                // 或纯字符串
  ],
  onKey: (key, doc) => {                     // key 见下方；doc 可编辑（选中即插入）
    if (key.name === "Enter") { doc.insert(doc.cursor.row, doc.cursor.col, "x"); return "close"; }
    if (key.name === "Down") return "handled";
    return "ignore";                         // 事件穿透
  },
  onClose: () => { /* 弹窗关闭时 */ },
});
```

- `render` 返回字符串数组或 `{ text, style }` 对象数组（style = 任意主题 scope 名，如 `"error"`/`"warning"`/`"ui.popup.info"`；未知名 → 默认样式）
- `onKey` 返回值：`"close"`（关闭弹窗并触发 onClose）\| `"handled"`（消费按键）\| `"ignore"`（穿透）
- 未提供 `onKey` 时：Esc 关闭，其余穿透
- `key` 对象：`{ name: string, shift: bool, ctrl: bool, alt: bool }`；name 取值 `"a"`..`"9"`、`"Enter"`、`"Esc"`、`"Up"`/`"Down"`/`"Left"`/`"Right"`、`"Tab"`、`"Backspace"`、`"PageUp"`/`"PageDown"`、`"Home"`/`"End"`、`"Delete"`、`"Insert"`、`"F1"`..`"F12"`
- `onKey` 的 `doc` 参数：同命令 ctx.doc（编辑 = 一次按键一个事务）——弹窗菜单"选中即插入"
- 多次 `open_popup` 替换前一个弹窗

## 9. 侧边面板

### `helix.open_panel({ side, size, render, onKey?, onClose? })`

返回面板 id。常驻停靠条带，**真实收缩编辑器布局**（推挤而非覆盖）。

```js
const id = helix.open_panel({
  side: "right",        // "right" | "left" | "bottom"
  size: 30,             // 条带宽度（字符列）或高度（bottom）
  render: () => [...],  // 同弹窗 render（支持样式行）
  onKey: (key) => "handled" | "ignore" | "close",  // 可选：消费/穿透/关闭
  onClose: () => {},
});
helix.close_panel(id);  // 按 id 精确关闭
```

- **多面板并存**：同侧从边缘向内叠加（先开靠边）；互不替换
- 无 `onKey` 时面板完全事件穿透（编辑器照常编辑）；有 `onKey` 时 `"handled"` 消费、`"ignore"` 穿透
- 面板内容可被异步回调更新（`onChunk` 里改模块状态 → 面板重绘）
- 面板是独立层，不随 Esc 关闭

## 10. 界面定制

### `helix.set_buffer_icon(fn)`

bufferline 标签栏图标钩子，每个缓冲区渲染时调用一次。

```js
helix.set_buffer_icon((path) => path?.endsWith(".rs") ? "🦀" : null);
```

- 返回图标字符串或 null（null = 默认行为）；单例，重复调用覆盖

### `helix.set_statusline(fn)`

状态栏右侧自定义文本，每帧渲染时调用。

```js
helix.set_statusline((ctx) => `${ctx.cursor.row}:${ctx.cursor.col}`);
```

- `ctx = { path, mode, cursor: { row, col } }`（轻量，不含全文）
- 返回字符串或 null（null = 不显示）；`set_statusline(null)` 清除
- ⚠️ 每帧调用——不要在钩子里跑 `helix.run`（同步阻塞）或重逻辑

## 11. 主题

### `helix.set_theme({ scope: color })`

实时覆盖任意主题 scope 的颜色。

```js
helix.set_theme({ "ui.popup": "#ff79c6", "error": "red", "warning": "#f1fa8c" });
```

- color 支持主题 palette 的颜色名或 `#rrggbb`；非法颜色条目忽略
- 覆盖即时生效（重建主题并替换）；支持继承主题（inherits）正确解析

### `helix.reset_theme()
helix.load(name) -> exports
helix.export(obj)
helix.lazy(name, ...commands)
helix.run_command(name, ctx?)
helix.open_file(path)
helix.move_panel(id, side)
helix.read_dir(path)
// 方案二：
helix.read_file_async / write_file_async / stat_async / glob_async
helix.el("text"|"row"|"col"|"scroll", ...)
helix.open_terminal({ cmd, side, size, onExit? })`

清空全部覆盖，恢复启动时的基础主题。

## 12. 插件管理

内置命令（编辑器内）：

```
:plugin list                  # 列出已加载插件
:plugin install <path>        # 复制插件文件到 plugins 目录并加载
:plugin remove <name>         # 从 plugins 目录删除 {name}.js
:plugin reload                # 热重载全部（同 :plugin-reload）
:plugin status                # 已加载数量 + 目录路径
```

- `install`/`remove` 只操作 `~/.config/helix/plugins/` 目录（路径校验防逃逸）
- `:plugin-reload` 是 `:plugin reload` 的别名（历史命令）

## 12.5 模块导入与懒加载

### `helix.load(name) -> exports`

从插件目录加载脚本并返回其导出值。相对名（`"icons.js"`）解析到插件目录；绝对路径可用；`.js` 后缀强制。重复加载返回缓存；文件不存在/语法错误抛错。

```js
// mod.js 内：
helix.export({ helper: () => 42 });
// init.js 内：
const mod = helix.load("mod.js");
mod.helper();   // 42
```

### `helix.export(obj)`

在当前脚本中声明导出（被 `helix.load` 的返回值拿到）。

### `helix.lazy(name, ...commands)`

注册命令桩：首次调用任一命令时才加载插件，然后转执行真实命令。

```js
helix.lazy("git.js", "gitbranch", "gitstatus");  // 首次 :gitbranch 或 :gitstatus 时加载 git.js
```

### `helix.run_command(name, ctx?)
helix.open_file(path)
helix.move_panel(id, side)
helix.read_dir(path)
// 方案二：
helix.read_file_async / write_file_async / stat_async / glob_async
helix.el("text"|"row"|"col"|"scroll", ...)
helix.open_terminal({ cmd, side, size, onExit? })`

程序化调用插件命令（`ctx` 可选，缺省空快照；支持命令 ctx 形状）。lazy 桩的内部底座，也可直接脚本化。

### `helix.open_file(path)`

在编辑器中打开文件（当前视图替换打开；目录/不存在抛错）。文件树面板的"打开"动作。

### `helix.move_panel(id, side)`

把面板移动到另一侧（`"right"|"left"|"bottom"`）。下一帧自动重排。

### `helix.read_dir(path)
// 方案二：
helix.read_file_async / write_file_async / stat_async / glob_async
helix.el("text"|"row"|"col"|"scroll", ...)
helix.open_terminal({ cmd, side, size, onExit? }) -> [{ name, is_dir, path }]`

结构化列出目录条目（同步，不递归，按名字排序）。文件树的构建原语。

```js
helix.read_dir("/tmp").forEach(e => {
  helix.echo((e.is_dir ? "[d] " : "    ") + e.name);
});
```

### 异步 fs（方案二）

```js
helix.read_file_async(path, (err, content) => {...})   // 读文件（UTF-8 lossy）
helix.write_file_async(path, content, (err) => {...})   // 写文件
helix.stat_async(path, (err, { is_dir, size, mtime }) => {...})
helix.glob_async(pattern, (err, paths) => {...})        // * / ** / ?（相对 CWD）
```

worker 线程执行，回调在主线程事件循环，不阻塞编辑器。

### 组件模型（方案二）

`render` 可返回嵌套组件树（旧行数组仍兼容）：

```js
helix.el("text", "hello", { style: "error", width: 20 })  // 文本（style/截断）
helix.el("row", [children], { gap: 1 })                   // 水平并排
helix.el("col", [children], { gap: 0 })                   // 垂直堆叠
helix.el("scroll", [children], { height: 10 })            // 高度裁剪（保留末 height 行）
```

组件可任意嵌套；`row` 各段样式不一致时整行样式坍缩为无。

### 原生终端视图（方案二）

```js
helix.open_terminal({ cmd: "bash", side: "right", size: 44, onExit?: (code) => {} })

helix.set_terminal_mode(id, "dock" | "fullscreen" | "floating" | "minimized")  // 切换模式
helix.term_clear(id)          // 清空终端网格
helix.resize_term(id, size)   // 调整面板尺寸（列宽/行高）
```

- **模式**：dock（停靠推挤）/ fullscreen（占满）/ floating（居中悬浮，16 行）/ minimized（底部 1 行细条，按键穿透编辑器、进程保活）
- **输出即时性**：worker 发事件即唤醒事件循环（~33ms 上屏），不再等 250ms idle
- **pty raw mode**：输入即达 bash（readline 处理回显/Ctrl-C），无 canonical 缓冲；kill 杀整个进程组（`sh` 未 exec 的孙进程也杀）

PTY 输出经 vte 解析成网格渲染（光标/颜色/清屏/滚回/alt screen 支持，vim/htop 可显示）；面板尺寸变化实时 TIOCSWINSZ；按键直通；Esc 关闭。返回面板 id（可 `move_panel`）。

### 富文本行与多 span（方案乙）

`helix.el("text", [...])` 支持富文本段数组——一行多色：

```js
helix.el("text", [
  { text: "标题 ", style: "error" },
  { text: "尾注", style: "comment" },
])
```

### 节点事件与焦点（方案乙）

`button` / `input` 控件节点带 `id` + 处理器；Tab/Shift-Tab 在可聚焦节点间移动焦点，Enter 触发聚焦按钮的 `onPress`，输入框按键路由到 `onKey`：

```js
helix.el("button", "OK", { id: "btn1", onPress: () => {...} })
helix.el("input", { id: "in1", value: "...", onKey: (k) => {...} })
```

`render(focus)` 回调收到当前焦点节点 id——JS 据此渲染焦点样式（如高亮）。

### 脏格增量渲染（方案乙）

弹窗/面板渲染改为逐格 diff——只把变化的格写入屏幕（终端输出流、高频更新场景显著减少重绘开销）。对插件透明。

## 13. 已知限制

| 限制 | 说明 |
|------|------|
| **无异步语言特性** | boa 引擎无 `setTimeout`/网络；异步只来自 `run_async`/`spawn`/`*_async` 回调 |
| **终端视图非完整仿真器** | vte 网格支持光标/颜色/清屏/滚回/alt screen；宽字符、鼠标、选择复制不支持 |
| **组件行样式坍缩** | row 混合样式整行坍缩为无样式（多 span 行模型未做） |
| **弹窗 render 无 doc** | 只有 `onKey` 有 doc 参数；render 回调不能编辑 |
| **事件 ctx 范围** | `save`/`mode-change` 等事件只带当前文档（其他分屏文档不触发） |
| **无多光标** | 选区/光标 API 只操作主光标 |
| **Unix-only** | `helix.run`/`spawn`/PTY 依赖 `sh` 与 libc openpty（Windows 不可用） |
| **同步阻塞** | `helix.run` 阻塞主线程；`spawn` 无超时（挂死命令会一直占着 worker） |
| **键位不持久** | `helix.map` 绑定重启失效（插件启动时重新注册） |
| **config-reload 键位不热更新** | 键位槽是启动快照（需重启应用新 config 键位） |
| **插件间无共享状态** | 每个插件独立作用域（IIFE 隔离）；共享需通过 `helix.*` API 或命令 |

---

## 附录：完整 API 索引

```
helix.register_command(name, fn, doc?)
helix.echo(text)
helix.run(cmd)
helix.run_async(cmd, cb)
helix.spawn({ cmd, pty?, onChunk, onExit }) -> id
helix.term_write(id, text)
helix.term_kill(id)
helix.term_resize(id, rows, cols)
helix.on(event, fn)
helix.map(mode, key, command)
helix.open_popup({ render, onKey?, onClose?, width?, height?, position? })
helix.open_panel({ side, size, render, onKey?, onClose? }) -> id
helix.close_panel(id)
helix.set_buffer_icon(fn)
helix.set_statusline(fn)
helix.set_theme({ scope: color })
helix.reset_theme()
helix.load(name) -> exports
helix.export(obj)
helix.lazy(name, ...commands)
helix.run_command(name, ctx?)
helix.open_file(path)
helix.move_panel(id, side)
helix.read_dir(path)
// 方案二：
helix.read_file_async / write_file_async / stat_async / glob_async
helix.el("text"|"row"|"col"|"scroll", ...)
helix.open_terminal({ cmd, side, size, onExit? })

命令 ctx：{ doc: { path, text, cursor, insert(), replace(), delete() },
            cursor: { row, col }, selection: { anchor, head } }
按键对象：{ name, shift, ctrl, alt }
事件：save | mode-change | buffer-open | buffer-close | doc-change
键位模式：normal | insert | select
面板方向：right | left | bottom
```
