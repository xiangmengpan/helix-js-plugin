# Helix JS 插件系统：参考与教程

> 本文档描述 helix 魔改版的 JavaScript 插件系统（`helix-js` crate，boa 引擎）。插件为 `.js` 文件。
> 所有 API 通过全局 `helix` 对象暴露。本文基于当前实现（`helix-js/src/lib.rs` + `helix-term/src/ui/plugin_*.rs`）。

## 目录

- [1. 架构与执行模型](#1-架构与执行模型)
- [2. 加载与生命周期](#2-加载与生命周期)
- [3. 命令与消息](#3-命令与消息)
- [4. 文档编辑与选区](#4-文档编辑与选区)
- [5. 进程执行](#5-进程执行)
- [6. 异步文件系统](#6-异步文件系统)
- [7. 事件钩子](#7-事件钩子)
- [8. 键位绑定](#8-键位绑定)
- [9. 弹窗](#9-弹窗)
- [10. 组件树（el）](#10-组件树el)
- [11. 侧边面板](#11-侧边面板)
- [12. 终端](#12-终端)
- [13. 布局树](#13-布局树)
- [14. 界面定制](#14-界面定制)
- [15. 主题](#15-主题)
- [16. 插件管理](#16-插件管理)
- [17. 综合示例：文件树面板](#17-综合示例文件树面板)
- [18. 已知限制](#18-已知限制)
- [19. API 速查索引](#19-api-速查索引)
- [20. LSP 请求](#20-lsp-请求)

---

## 1. 架构与执行模型

理解执行模型是正确使用插件的前提：

```
┌─ 主线程（唯一 JS 线程）───────────────────────────────┐
│  boa Context（!Send，全部 JS 状态/回调都在这）         │
│  · 命令、事件、render、onKey 都在主线程同步执行         │
│  · 队列：UI_REQUESTS（渲染/布局请求，命令边界 drain）   │
│  · 队列：MESSAGES（echo 消息，状态栏刷新）              │
├─ worker 线程（std::thread，无 JS）─────────────────────┤
│  · 进程执行（run_async / spawn / pty）→ TermEvent      │
│  · 异步 fs（read_file_async 等）→ AsyncEvent           │
│  · 结果经 mpsc 通道送回主线程，drain 后 resolve 到回调  │
└────────────────────────────────────────────────────────┘
```

- **JS 永远只在主线程跑**（boa 的 `Context` 是 `!Send`），所以 JS 侧无并发问题。
- **耗时操作必须用异步 API**（`run_async` / `spawn` / `*_async`），它们在 worker 线程执行，promise 恢复（`.then`/`.catch`）回到主线程事件循环。
- **`helix.run`（同步）会阻塞编辑器主线程**——只用于短命令。
- 弹窗/面板/终端现在是**布局树叶子**（见 §11/§12/§13）：面板和终端会真实收缩编辑器布局，不再只是覆盖层。
- 渲染模型：JS `render` 回调每次重绘全量返回内容 → 布局引擎拍平成 `StyledLine`（多 span）→ 脏格 diff 只写变化的格。

---

## 2. 加载与生命周期

- **唯一入口**：启动只自动加载 `~/.config/helix/init.js`（兼容旧位置 `plugins/init.js`）；其他插件脚本必须经 `helix.load` 导入。
- `:plugin-load <path>` 可手动加载任意文件；`:plugin-reload`（或 `:plugin reload`）清空全部插件状态并按加载顺序重跑（模块文件重读磁盘）。
- 每个插件在**独立 IIFE 作用域**求值：顶层 `let`/`const` 不跨插件共享；`helix` 全局对象除外。
- 插件间共享状态只能通过 `helix.*` API（命令、事件、导出）或你自己的外部文件。

```js
// ~/.config/helix/init.js —— 唯一入口示例
helix.load("icons.js");                       // 立即导入
helix.lazy("terminal.js", "term");            // 懒加载：首次 :term 才加载
```

### 模块导入：`helix.load(name)` / `helix.export(obj)` / `helix.lazy(name, ...commands)`

```js
// mod.js 内：
helix.export({ helper: () => 42 });

// init.js 内：
const mod = helix.load("mod.js");   // 相对名解析到插件目录；重复加载返回缓存
mod.helper();                       // 42
```

- `helix.lazy("git.js", "gitbranch", "gitstatus")`：注册命令桩，首次调用任一命令时才加载插件再转执行真实命令。

### `helix.run_command(name, ctx?)`

程序化调用插件命令（`ctx` 可选，缺省空快照）。lazy 桩的内部底座，也可直接脚本化。

---

## 3. 命令与消息

### `helix.register_command(name, fn, doc?)`

注册 `:name` 命令。`doc`（可选字符串）在命令行输入 `:name` 时显示在提示区。

```js
helix.register_command("hello", (ctx) => {
  helix.echo("world");
}, "打印 world");
```

- `fn(ctx)` 的 `ctx` 见 §4。
- 命令内可编辑文档、移动光标、开弹窗/面板/终端；全部效果在命令返回后原子应用（**一个命令 = 一次撤销**）。
- 命令内发起的 UI 请求（`open_file`/`move_panel`/`set_terminal_mode`/`split` 等）在命令边界 drain 并即时应用。

### `helix.echo(text)`

在状态栏显示消息。

---

## 4. 文档编辑与选区

命令的 `ctx` 是**只读快照 + 编辑队列**：

```js
ctx.path                  // string | null，文件路径
ctx.text                  // string，全文
ctx.cursor                // { row, col }，主光标位置（0-based）
ctx.selection             // { anchor: {row,col}, head: {row,col} }
ctx.doc                   // 文档对象（含编辑方法，见下）
ctx.doc.insert(row, col, text)              // 插入
ctx.doc.replace(sr, sc, er, ec, text)       // 替换区间
ctx.doc.delete(sr, sc, er, ec)              // 删除区间
```

- 坐标为 0-based 行列，基于命令开始时的**原始快照**（多次编辑互不偏移）。
- 越界自动 clamp；一次命令的所有编辑 = 一个事务 = 一次撤销。
- 编辑后游标被自动重映射（在插入处插入后，游标落在插入文本之后）。

### `helix.begin_edit()` / `helix.end_edit()`(批量编辑事务)

```js
helix.begin_edit();
await something();          // async 命令跨 await:泵循环每帧取编辑会被拆事务
ctx.doc.insert(0, 0, "a");
helix.end_edit();           // 期间所有编辑合并为一个事务 = 一次撤销
```

- 可嵌套(深度计数)；`end_edit` 深度归零时才放行编辑应用。
- **未配对 begin**(begin 后无 end):下个命令/事件入口复位深度并丢弃积压编辑(不崩)。
- 同步命令本身已是一个事务,本 API 主要用于 async 跨 await 场景。

### `helix.by_path(path)`(跨 buffer 访问)

按路径查**已打开**的 buffer,返回与 `ctx.doc` 同构的只读快照对象(路径已规范化,相对路径基于启动时 cwd):

```js
const other = helix.by_path("src/main.rs");
// → { path: "/abs/src/main.rs", text: "...", cursor: { row: 0, col: 0 } }
//   + insert(row, col, str) / replace(sr, sc, er, ec, str) / delete(sr, sc, er, ec)
// 未打开 → null(不会自动打开文件)
```

- 编辑方法与 `ctx.doc` 一致,但作用于目标 buffer;一个命令改多个 buffer 时每个 buffer 一次撤销
- `cursor` 恒为 {row: 0, col: 0}(后台 buffer 无视图光标)
- 快照是命令开始时的文本;坐标基于该快照,跨 await 的过期坐标风险与 `ctx.doc` 相同
- scratch(无路径)buffer 不可查
- 目标 buffer 在命令期间被关闭 → 编辑应用时报错入状态栏(不崩)
- 同一命令内同时用 `ctx.doc` 与 `by_path(当前文件路径)` 改同一 buffer → 视为两个目标,撤销两次

### `helix.set_virtual_text(path, row, col, text, style)` / `helix.set_highlight(path, sr, sc, er, ec, style)`(装饰/标记)

给已打开 buffer 加装饰(不修改文本;命令/事件期间累积,应用时**整体替换**该 buffer 的插件装饰):

```js
helix.set_virtual_text("src/main.rs", 1, 4, " // TODO", "ui.help");
helix.set_highlight("src/main.rs", 0, 0, 5, 0, "ui.selection");   // [start, end) 半开区间
helix.set_virtual_text("src/main.rs");                            // 清除该 buffer 全部装饰
```

- `style`: 主题 scope 字符串或 null(解析失败/省略 = 不渲染)
- 路径规则与 `by_path` 一致(规范化匹配,未打开静默忽略);坐标在命令结束时按当时文本换算,不随文档变更重映射——监听 `doc-change` 重推
- 装饰按 buffer 存储,所有窗口共享;buffer 关闭或脚本重载时清空

### `helix.set_cursor(row, col)` / `helix.set_selection(ar, ac, hr, hc)` / `helix.set_selection([...])`

```js
helix.set_cursor(3, 10);                          // 移动主光标
helix.set_selection(1, 0, 3, 5);                  // 设置单选区
helix.set_selection([                             // 多选区(数组形态)
  { anchor: { row: 0, col: 0 }, head: { row: 0, col: 2 } },
  { anchor: { row: 2, col: 1 }, head: { row: 2, col: 4 } },
]);
```

- 多选区按 range 起点排序,主光标 = 排序后末位(与 helix 原生多光标一致)；空数组/元素缺字段报错。

- 写操作队列化，命令返回后应用；光标请求在编辑事务之前应用。

```js
helix.register_command("upper", (ctx) => {
  const a = ctx.selection.anchor, h = ctx.selection.head;
  if (a.row !== h.row) { helix.echo("single-line only"); return; }
  const line = ctx.text.split("\n")[a.row];
  const lo = Math.min(a.col, h.col), hi = Math.max(a.col, h.col);
  ctx.doc.replace(a.row, lo, a.row, hi, line.slice(lo, hi).toUpperCase());
});
```

---

## 5. 进程执行

### `helix.run(cmd)` — 同步（慎用）

执行 `sh -c cmd`，返回 stdout 字符串。失败（非零退出码/启动失败）**抛错**，`message` 含 stderr。

```js
try {
  const branch = helix.run("git branch --show-current");
  helix.echo("branch: " + branch.trim());
} catch (e) { helix.echo("failed: " + e.message); }
```

> ⚠️ 同步阻塞编辑器主线程——只用于短命令。输出截断到 64KB（超出附 `(truncated)` 标记）。

### `helix.run_async(cmd)` — 异步（返回 Promise）

```js
const out = await helix.run_async("git status");
helix.echo(out.trim());
```

不阻塞编辑器；成功 resolve stdout（退出码 0），失败 reject `Error`（`e.message` 可取错误）。`.then`/`.catch` 在主线程事件循环执行。

### `helix.spawn({ cmd, pty?, onChunk, onExit })` — 流式进程

返回进程 id。实时接收输出块、可写 stdin、可杀。

```js
const id = helix.spawn({
  cmd: "bash --norc --noprofile",
  pty: true,                       // 可选：true = 跑在真实 PTY 下（交互式/curses 程序可用）
  onChunk: (chunk) => { /* 输出块（stdout+stderr 合并，UTF-8） */ },
  onExit: (code) => { /* 退出码 */ },
});
helix.term_write(id, "ls\n");      // 写 stdin（或 PTY master）
helix.term_kill(id)
helix.term_list()                        // [{view_id, cmd}] 当前终端
helix.term_close(view_id)                // 按 id 关闭终端;               // 杀进程（pty 模式杀整个进程组）
helix.term_resize(id, rows, cols); // 仅 PTY：设置窗口尺寸（TIOCSWINSZ，默认 24×80；Unix-only）
```

### 异步恢复点内可以做什么

所有 promise 的 `.then`/`.catch`（`run_async` / `*_async`）在主线程执行，可以：
- `helix.echo`、开弹窗/面板/终端、调用命令、链式 `helix.run_async`；
- 修改模块级状态（供弹窗/面板 `render` 读取）；
- **没有 `ctx`**（回调无文档快照），编辑文档要走命令或 `doc` 参数路径。

---

## 6. 异步文件系统

worker 线程执行，不阻塞编辑器。均返回 Promise：成功 resolve 值，失败 reject `Error`（`e.message` 可取消息）。

```js
helix.read_dir(path)                              // 同步！返回 [{name, is_dir, path}]（不递归，按名排序）
helix.read_file_async(path)                       // Promise → 文件内容（UTF-8 lossy）
helix.write_file_async(path, content)             // Promise → undefined
helix.stat_async(path)                            // Promise → {is_dir, size, mtime}（mtime 为 Unix 秒）
helix.glob_async(pattern)                         // Promise → paths[]（* / ** / ?，相对 CWD；** 跨目录）
```

```js
helix.read_dir("/tmp").forEach(e => {
  helix.echo((e.is_dir ? "[d] " : "    ") + e.name);
});
```

---

## 7. 事件钩子

### `helix.on(event, fn)`

| 事件 | 触发时机 | 回调签名 |
|------|---------|---------|
| `save` | 保存**前**（编辑先应用再保存 → format-on-save） | `(doc)` |
| `mode-change` | 模式切换后 | `(mode, doc)`，mode: `"normal"\|"insert"\|"select"` |
| `buffer-open` | `:open` 打开文件后 | `(doc)` |
| `buffer-close` | 关闭（quit/force_quit 路径）前 | `(doc)` |
| `doc-change` | 文档文本变化，编辑停顿 250ms 后（idle 防抖） | `(doc)` |

`doc-change` 的 `doc` 额外带 `changes`(防抖窗口内变更的**合并范围**,无变更时 `[]`)：

```js
helix.on("doc-change", (doc) => {
  doc.changes  // [{ oldRange: {start:{row,col}, end:{row,col}},
                //    newRange: {start:{row,col}, end:{row,col}} }]
});
```

- 合并语义:窗口内多次变更合并为包围范围(old = 各 old 起点最小..终点最大,new 同理)；单次变更即其本身。
- 坐标是各变更发生时刻的坐标(非当前文本),窗口语义由插件自行处理。

- `doc` 与命令的 `ctx.doc` 同构（含 cursor 与编辑方法）。
- 同事件可注册多个处理器，按注册顺序调用；处理器抛错 → 状态栏报错，不阻断主流程。
- `save` 处理器编辑 = 保存前变更（一次撤销）。

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

---

## 8. 键位绑定

### `helix.map(mode, key, command)`

| 参数 | 说明 |
|------|------|
| `mode` | `"normal" \| "insert" \| "select"` |
| `key` | 键序列：单键 `"K"`、修饰键 `"C-n"`、空格分隔多键 `"C-n gd"`、逐字符多键 `"gd"` |
| `command` | 命令名字符串（内置或已注册插件命令）**或** JS 回调（自动注册为隐藏命令 `__mapped_N`） |

```js
helix.map("normal", "W", "show-where");              // 绑定插件命令
helix.map("insert", "C-n", () => helix.echo("hi"));  // 绑定回调
helix.map("normal", ";", "fileicon");                // 覆盖内置键位
```

- 重复绑定 = 覆盖；绑定立即生效；重启失效（插件启动时重新注册）。
- 回调收到 `(ctx)`（同命令 ctx）。

---

## 9. 弹窗

### `helix.open_popup({ render, onKey?, onClose?, width?, height?, position? })`

JS 渲染的覆盖层弹窗（模态层，重复 `open_popup` 替换前一个）。返回弹窗 id。

```js
helix.open_popup({
  width: 40, height: 10,                     // 可选：尺寸上限（clamp）
  position: { row: 5, col: 10 },             // 可选：屏幕锚点
  render: (focus, ctx) => [                  // focus = 焦点节点 id 或 null（见 §10）
    { text: "Error: ", style: "error" },     // 样式行：style = 主题 scope 名
    "普通行",                                // 或纯字符串
  ],
  onKey: (key, doc) => {                     // key 见下方；doc 可编辑
    if (key.name === "Enter") { doc.insert(doc.cursor.row, doc.cursor.col, "x"); return "close"; }
    if (key.name === "Down") return "handled";
    return "ignore";                         // 事件穿透
  },
  onClose: () => { /* 弹窗关闭时 */ },
});
```

- **render 返回两种形式**：字符串/`{text, style}` 数组（行 API），或 `helix.el` 组件树（§10）。
- **onKey 返回值**：`"close"`（关闭弹窗并触发 onClose）\| `"handled"`（消费按键）\| `"ignore"`（穿透）。未识别返回值 → `"handled"`（安全默认）。
- 未提供 `onKey` 时：Esc 关闭，其余穿透。
- `key` 对象：`{ name, shift, ctrl, alt }`；name 取值：字符键、`"Enter"`、`"Esc"`、`"Tab"`、`"Backspace"`、`"Delete"`、`"Insert"`、`"Up"`/`"Down"`/`"Left"`/`"Right"`、`"Home"`/`"End"`、`"PageUp"`/`"PageDown"`、`"F1"`..`"F12"`（Shift-Tab 不可表示，见 §18）。
- onKey 的 `doc` 参数：同命令 ctx.doc（编辑 = 一次按键一个事务）。
- onKey 返回后：编辑/光标请求/消息/UI 请求都会在本次按键内同步应用。

---

## 10. 组件树（el）

`render` 可返回嵌套组件树（旧行数组仍兼容）。组件树节点由 `helix.el(type, arg, opts)` 构造：

### 节点类型

```js
// text：单行文本。arg 为字符串或富文本段数组（一行多色）
helix.el("text", "hello", { style: "error", width: 20 })
helix.el("text", [ { text: "标题 ", style: "error" }, { text: "尾注", style: "comment" } ])

// row / col：水平并排 / 垂直堆叠（gap = 列间距 / 行间距）
helix.el("row", [child, child], { gap: 1 })
helix.el("col", [child, child], { gap: 0 })

// scroll：高度裁剪容器（保留最后 height 行；内容按无限高布局再裁头）
helix.el("scroll", [child, ...], { height: 10 })

// button：可聚焦按钮，渲染为 [ label ]，Enter/Space 触发 onPress
helix.el("button", "run", { id: "btn1", onPress: () => helix.echo("pressed"), style: "error" })

// input：可聚焦输入框。value 由**引擎权威**维护（JS 传值仅初始化），渲染带光标 "|"；
// 编辑键（字符/Backspace/Delete）自动改值并触发 onChange(新值)；导航键（↑↓/Enter）走 onKey。
// 注意：onChange 需用原始对象形式（helix.el 暂不透传 onChange）
{ type: "input", id: "q", value: "abc", width: 20,
  onChange: (v) => { /* v = 编辑后的新值（光标移动不触发） */ },
  onKey: (k) => { /* k = "Up" | "Down" | "Enter" | ...（见下） */ } }
```

节点可任意嵌套。

### 焦点系统

- 组件树中的 `button` / `input`（有 `id`）自动成为**可聚焦节点**；`render` 回调签名变为 `render(focus, ctx)`，`focus` = 当前焦点节点 id（无焦点时为 `null`）——JS 据此渲染焦点样式（如高亮）。
- **Tab** 在可聚焦节点间循环移动焦点（按树序）。无 Shift-Tab。
- 焦点在 `button` 上：**Enter/Space** → `onPress`。
- 焦点在 `input` 上，按键语义（引擎维护 value + 光标，渲染为 `value` 中插 `|`）：

  | 按键 | 行为 |
  |------|------|
  | 单字符 / Backspace / Delete | 编辑（引擎改值 + 光标）→ 触发 `onChange(新值)` |
  | Left / Right / Home / End | 仅移动光标，**不**触发 onChange |
  | Up / Down | → 节点 `onKey("Up"/"Down")`（候选导航） |
  | Enter | → 节点 `onKey("Enter")`（空格键是单字符编辑键，命中首行插入输入框） |
  | Tab | 焦点移到下一个可聚焦节点 |
  | Esc | 关闭弹窗（弹窗级缺省；焦点在 input 上同样生效） |

- 强制改值（候选回填/清空）：`helix.set_input_value(popup_id, node_id, value)`——写入引擎状态，光标置末尾，下次渲染生效；弹窗关闭时引擎自动清理该弹窗全部 input 状态。
- 焦点节点按键不经过弹窗级 `onKey`；弹窗级 `onKey` 只在焦点系统未消费时收到按键。
- 面板的 render 目前固定收到 `focus = null`（面板无节点焦点，见 §18）。

### 布局语义

- `text` 的 `width` 是**截断上限**（不是占位宽度）；viewport 宽度优先。
- `row`：第 i 行 = 各子节点第 i 行拼接（行数不足补空行）；gap = 空格串；总宽超 viewport 截断。
- `col`：自上而下堆叠，gap = 空行；viewport 高度限制。
- `scroll`：按 col（gap=0）**完整布局**再保留最后 height 行——O(内容) 而非 O(视口)。
- **多 span 行**：`StyledLine` 内部是 `Vec<TextSpan>`，row 混合样式**不再坍缩**——每段独立保留样式（一行多色已支持）。

### 渲染性能

弹窗/面板渲染走**脏格 diff**（`DiffRenderer`）：与上次输出逐格对比，只把变化的格写入屏幕（终端输出流、高频更新场景显著减少重绘）。

---

## 11. 侧边面板

### `helix.open_panel({ side, size, render, onKey?, onClose?, focusable? })`

返回面板 id。面板是**布局树叶子**：真实收缩编辑器布局（切分活动叶子，推挤而非覆盖）。

```js
const id = helix.open_panel({
  side: "right",        // "right" | "left" | "bottom"
  size: 30,             // 条带宽度（字符列）或高度（bottom）
  render: () => [...],  // 同弹窗 render（行 API 或 el 组件树）
  onKey: (key) => "handled" | "ignore" | "close",
  onClose: () => {},
  focusable: false,     // 可选，默认 false；true 时启用节点焦点路由（见下）
});
helix.close_panel(id);            // 按 id 精确关闭
helix.move_panel(id, "left");     // 移动面板到另一侧
```

- **多面板并存**：每个面板都是独立叶子，可同侧叠加。
- 无 `onKey` 时面板完全事件穿透（编辑器照常编辑）；有 `onKey` 时 `"handled"` 消费、`"ignore"` 穿透。
- **`focusable: true` 启用节点焦点路由**（与弹窗同机制，仅影响启用面板）：`Tab` 在可聚焦节点（`button`/`input` 的 `id`）间循环移动焦点；焦点在节点时按键直达节点（`Enter` → `button` 的 `onPress` / `input` 的 `onKey("Enter")`，字符 → `input` 插入触发 `onChange`，方向键 → `input` 光标移动/候选导航）；`Esc` 取消焦点回 `onKey`（面板非模态，不关闭）；焦点节点从树中消失（render 后）自动重置焦点。未启用（默认）时按键一律走 `onKey`，完全保持现状。
- 面板内容可被异步回调更新（`onChunk` 里改模块状态 → 面板重绘）。
- `:panel-close` 关闭最近打开的面板。

---

## 12. 终端

### `helix.open_terminal({ cmd, side, size, onExit? })`

打开原生终端面板（真实 PTY + vte 终端模拟器）。返回 `view_id`（= 面板 id）。

```js
const view = helix.open_terminal({
  cmd: "bash",
  side: "right",        // "right" | "left" | "bottom"
  size: 44,             // 列宽 / 行高
  onExit: (code) => {}, // 进程退出回调
});

helix.set_terminal_mode(view, "dock" | "fullscreen" | "floating" | "minimized");
helix.term_clear(view);          // 清空终端网格
helix.resize_term(view, size);   // 运行中调整面板尺寸
helix.term_feed(view, chunk);    // 内部桥接（open_terminal 内部使用，一般不用直接调）
```

### 四种显示模式

| 模式 | 行为 |
|------|------|
| `dock`（默认） | 停靠推挤（布局树叶子） |
| `fullscreen` | 占满整个编辑区 |
| `floating` | **居中悬浮窗（带边框，60%×70% 视口），不占 split 布局，渲染在最上层** |
| `minimized` | 底部 1 行细条；按键穿透编辑器、进程保活 |

### 终端能力

- vte 解析：光标 / SGR 颜色 / 粗体 / 清屏 / 滚回 / alt screen（vim、htop 可显示）。
- **C-\ 模式切换**（lazyvim 语义）：insert → 终端 normal（滚动缓冲 j/k/gg/G/PageUp/PageDown 查看）；
  normal 再 C-\ → 回 helix（收起浮窗 + 聚焦编辑器）；normal 内 i/a/Esc 回 insert、q 关闭。
- 滚动缓冲**可查看**（normal 模式滚动，不干扰 pty 活动输出）。
- **宽字符（CJK）支持**（主格+占位格，背景连续）。
- 按键直通 pty（raw mode，含方向键/Ctrl 组合 xterm 应序）；面板尺寸变化实时 TIOCSWINSZ；
  Esc 关闭（Drop 时杀 pty，杀整个进程组）。
- 组合字符 / 鼠标 / 选择复制**暂不支持**（后续）。
- PTY 仅 Unix。

### 终端管理

- `helix.term_list() -> [{ view_id, cmd }]`：当前打开的终端（注册表在 Rust 侧，reload 后仍准确）。
- `helix.term_close(view_id)`：按 id 关闭指定终端（替代 remove_type 关全部）。
- 配套插件 `plugins/terminal.js`：`:term` 浮动 toggle、`:vterm`/`:hterm` 分屏、`:term-list`、`:term-close`。

---

## 13. 布局树

弹窗之外，面板/终端/编辑器都挂在**布局树**上：编辑器是 id=0 的叶子，面板和终端通过切分活动叶子挂载。JS 可以直接操作布局树：

```js
// 切分活动叶子。dir: "right"|"left"|"top"|"bottom"
// 终端分支：{ terminal: { cmd, size? } }
// 面板分支：{ panel: { render, onKey?, size? } }
const leafId = helix.split("right", { terminal: { cmd: "htop", size: 40 } });
const leafId2 = helix.split("bottom", {
  panel: { render: () => helix.el("col", [helix.el("text", "hi")]), onKey: (k) => "handled" },
});

helix.close_leaf(id);              // 移除叶子（终端进程被杀、面板回调清掉）
helix.zoom(id);                    // 缩放指定叶子到全屏
helix.unzoom();                    // 恢复
helix.resize_leaf(id, 0.3);        // 调整包含该叶子的分界比例（0~1）
helix.focus(id);                   // 聚焦叶子
```

### 布局序列化（半成品）

```js
const layout = helix.get_layout();     // { tree: {...}, active: id, zoomed: id|null } | null
helix.restore_layout(layout);          // 目前只写入缓存（树重建未接线，见 §18）
```

`tree` 为嵌套 JSON：`{ "type": "leaf", "id": N }` 或 `{ "type": "split", "dir": "h"|"v", "ratio": 0.5, "first": ..., "second": ... }`。

---

## 14. 界面定制

### `helix.set_statusline(fn)`

状态栏右侧自定义分段文本，每帧渲染时调用。

```js
helix.set_statusline((ctx) => `${ctx.cursor.row}:${ctx.cursor.col}`);            // 字符串 = 单段
helix.set_statusline((ctx) => [
  { text: " N ", style: "ui.statusline.normal" },   // mode 色块（分段着色）
  ctx.path ? ctx.path.split("/").pop() : "",
  { text: String(ctx.diagnostics_error), style: "error", zone: "right" }, // 右区
]);
```

- `ctx = { path, mode, cursor: { row, col }, total_lines, diagnostics_error, diagnostics_warning }`（轻量快照，不含全文）。
- 返回 `null`（不显示）/ `string`（单段）/ 数组 `[ "str" | { text, style?, right? } ]`（多段，每段可带 theme scope 着色；`right: true` 的段右对齐）。
- **replace 模式**：`helix.set_statusline(fn, { replace: true, zones: [左, 中, 右] })`——整个状态栏由 JS 控制（默认组件不渲染），按 `zones` 比例（默认 `[1,1,1]`）分左/中/右三区：左区靠左、中区居中、右区右对齐；段间自动 1 空格 gap。分段用 `zone: "left" | "center" | "right"`（默认 left）。默认（无第二参）仍是追加到右侧。
- `set_statusline(null)` 清除（同时退出 replace 模式）；非法参数报错。
- ⚠️ 每帧调用——不要在钩子里跑 `helix.run`（同步阻塞）或重逻辑；建议配合 `plugins/icons.js` 取图标。

### `helix.set_buffer_icon(fn)`

bufferline 标签栏图标钩子，每个缓冲区渲染时调用一次。

```js
helix.set_buffer_icon((path) => path?.endsWith(".rs") ? "🦀" : null);
```

- 返回图标字符串或 null（null = 默认行为）；单例，重复调用覆盖。

### `helix.set_completion_render(fn)`

补全菜单行渲染钩子：注册后每个候选行由 JS 生成，未注册/抛错/返回不可用内容时回退原生两列（label + kind）。

```js
helix.set_completion_render((ctx) => [
  { type: "text", text: `${ctx.label}  ${ctx.kind}`, style: "ui.completion" },
]);
```

- 回调 `fn(ctx) -> 数组 | 组件树节点`，每帧对每个可见候选行调用一次。返回语义与 `open_popup` 的 render 一致：数组（字符串/`{text, style?}` 对象）→ 行；含 `type` 的节点对象 → 组件树。
- `ctx = { label, kind, kindNum, provider, detail, deprecated, matchIndices }`：
  - `label`：候选标签；`kind`：kind 文本（如 `"method"`）；`kindNum`：数字 kind（1-25，非 LSP/未知为 0）；
  - `provider`：`"lsp"` | `"word"` | `"path"` | `"snippet"`；`detail`：LSP 候选的详情（字符串或 null）；
  - `deprecated`：候选是否标记弃用；`matchIndices`：当前输入匹配位置数组（grapheme 索引，高亮用）。
- 当前实现把返回内容**全部拼进第一列**（单行单列拼接，多 Cell/多行会破坏 Table 对齐，本批次不支持）；`style` 字符串经当前主题查表（未知 scope → 默认样式）。
- ⚠️ 每行每帧调用——不要在钩子里跑 `helix.run`（同步阻塞）或重逻辑。

### `helix.set_diagnostic_icons({ error?, warning?, info?, hint? })`

诊断标记列图标（gutter 诊断符号；未设置时用默认 ●）。

```js
helix.set_diagnostic_icons({ error: "✗", warning: "!", info: "ℹ", hint: "?" });
```

- 值可为任意字符串（nerd font 字形 / unicode / 字符）；非字符串或空串忽略；整体替换。
- 建议从统一映射表 `plugins/icons.js`（`ICONS.diagnostic`）取图标。

---

## 15. 主题

### `helix.set_theme({ scope: color | { fg?, bg?, modifiers? } })`

实时覆盖任意主题 scope 的样式（整体替换旧的覆盖集）。scope 可用 `ui.*` 或任意语法高亮 scope（如 `keyword`、`string`）。

```js
helix.set_theme({ "ui.popup": "#ff79c6", "error": "red" });            // 字符串 = fg
helix.set_theme({
  "ui.selection": { fg: "#111111", bg: "#ff0000", modifiers: ["italic"] },
  "warning": { fg: "#f1fa8c", modifiers: ["bold", "underline"] },
});
```

- 颜色支持主题 palette 的颜色名或 `#rrggbb`（不支持引用另一 scope 作颜色）；非法条目忽略（不整体报错）。
- modifiers 可选：`bold` `dim` `italic` `underline` `strikethrough` `reversed`。
- 覆盖即时生效（重建主题并替换）；支持继承主题（inherits）正确解析。
- 语法高亮 scope 的覆盖同样生效（重建时更新 syn_loader scope 集）。

### `helix.reset_theme()`

清空全部覆盖，恢复当前基础主题。

### `helix.get_style(scope) -> { fg, bg, modifiers } | null`

读取当前合并后样式（hex 字符串；未定义项为 null，未知 scope 返回 null）。叠加覆盖、状态栏取色、自定义组件配色用。

```js
const s = helix.get_style("ui.selection");   // { fg: "#000000", bg: "#00ff00", modifiers: [...] }
```

### `helix.theme_info() -> { name, themes }`

当前主题名 + 可用主题名列表（主题切换器用）。

```js
const { name, themes } = helix.theme_info();
```

### `helix.set_theme_name(name)`

切换基础主题（异步应用：加载 + 应用 + 清空当前覆盖；失败经状态栏报错）。

```js
helix.set_theme_name("base16_default");
```

### `helix.on("theme-change", fn)`

主题变化事件（set_theme / reset_theme / set_theme_name / `:theme` 后触发）；处理器签名与 buffer-open 一致（收到文档 ctx）。

---

## 16. 插件管理

内置命令（编辑器内）：

```
:plugin list                  # 列出已加载插件
:plugin install <path>        # 复制插件文件到 plugins 目录并加载
:plugin remove <name>         # 从 plugins 目录删除 {name}.js
:plugin reload                # 热重载全部（同 :plugin-reload）
:plugin status                # 已加载数量 + 目录路径
:plugin-load <path>           # 手动加载任意文件
:plugin-reload                # :plugin reload 的别名（历史命令）
:panel-close                  # 关闭最近打开的面板
```

- `install`/`remove` 只操作 `~/.config/helix/plugins/` 目录（路径校验防逃逸）。

---

## 17. 综合示例：文件树面板

组合 §6 fs + §10 组件树 + §11 面板 + §4 文档打开，写一个最小文件树：

```js
// filetree.js
let cwd = helix.run("pwd").trim();      // 起始目录（同步 run 仅此一次，可接受）
let entries = [];
let expanded = {};                      // 目录展开状态（JS 侧自己维护）

function loadDir(dir) {
  expanded[dir] = true;
  entries = helix.read_dir(dir);
}

helix.register_command("filetree", () => {
  loadDir(cwd);
  const id = helix.open_panel({
    side: "left",
    size: 32,
    render: (focus) => {
      // 每行一个 text：目录可再展开，文件可打开
      const rows = entries.map((e) =>
        helix.el("text",
          (e.is_dir ? "[d] " : "    ") + e.name,
          { style: focus === "tree-" + e.path ? "ui.selection" : (e.is_dir ? "ui.virtual" : null) })
      );
      return helix.el("col", rows, { gap: 0 });
    },
    onKey: (key, doc) => {
      if (key.name === "Enter") {              // 打开/进入
        const hit = entries[key._row ?? 0];    // 简化：JS 需自行维护行命中（见 §18）
        if (hit.is_dir) loadDir(hit.path);
        else helix.open_file(hit.path);
        return "handled";
      }
      return "ignore";
    },
  });
});
```

> 注：组件树目前**没有节点级点击命中**——树内导航（上下移动选中行）需要 JS 自己维护当前行号，并在 `render` 里按行号高亮（上例做了简化）。

---

## 18. 已知限制

| 限制 | 说明 |
|------|------|
| **无异步语言特性** | boa 无 `setTimeout`/网络/事件循环；异步只来自 `run_async`/`spawn`/`*_async` 的 Promise（await/.then） |
| **同步阻塞** | `helix.run` 阻塞主线程；`spawn` 无超时（挂死命令一直占着 worker） |
| **终端视图非完整仿真器** | 宽字符/组合字符、鼠标、选择复制不支持；滚回只存不显示（上限 1000 行） |
| **Shift-Tab 不可用** | `KeyCode` 无 BackTab，焦点只能 Tab 正向循环 |
| **面板无节点焦点** | 面板 render 固定收到 `focus = null`（节点焦点只在弹窗生效） |
| **无树内命中** | 组件树节点不响应点击/悬停；命中检测（哪一行被选中）由 JS 自己维护 |
| **restore_layout 未接线** | `get_layout`/`restore_layout` 目前只读写缓存，布局树重建未实现 |
| **Scroll 全量布局** | scroll 是 O(内容) 而非 O(视口)，几千行内容每帧全量布局 |
| **事件 ctx 范围** | 事件只带当前文档（其他分屏文档不触发） |
| **无多光标** | 选区/光标 API 只操作主光标 |
| **Unix-only** | `helix.run`/`spawn`/PTY 依赖 `sh` 与 libc openpty（Windows 不可用） |
| **键位不持久** | `helix.map` 绑定重启失效（插件启动时重新注册） |
| **插件间无共享状态** | 每个插件独立 IIFE 作用域；共享需通过 `helix.*` API 或外部文件 |
| **嵌套 load 污染风险** | boa 0.21 嵌套 eval 会破坏外层闭包的函数/变量绑定（`typeof` 变 object、函数不可调用）。**依赖必须用 `helix.plugin` 的 `deps` 声明**（保证先加载并缓存），不要在命令/事件/渲染回调里 `helix.load` 未缓存脚本。**同一脚本内 `helix.load` 之后再注册的闭包（箭头函数/回调）也会被污染**（实测 DefInitVar 越界 panic，2026-08-26）；回调/钩子注册应放在无嵌套 load 的脚本里（如 icons.js 自注册）。单独 `:plugin-load` 一个依赖插件的插件前，先加载其依赖。升级 boa 后重新评估（上游 bug，0.21.1 为最新版） |

---

## 19. API 速查索引

### 全局对象

```
helix.echo(text)
helix.register_command(name, fn, doc?)
helix.run_command(name, ctx?)
helix.run(cmd)                                  // 同步，抛错，64KB 截断
helix.run_async(cmd)                          // Promise：await → stdout；失败 reject Error
helix.spawn({ cmd, pty?, onChunk, onExit }) -> id
helix.term_write(id, text)
helix.term_kill(id)
helix.term_list()                        // [{view_id, cmd}] 当前终端
helix.term_close(view_id)                // 按 id 关闭终端
helix.term_resize(id, rows, cols)               // pty TIOCSWINSZ，Unix
helix.on(event, fn)                             // save | mode-change | buffer-open | buffer-close | doc-change
helix.map(mode, key, commandOrFn)
helix.set_cursor(row, col)
helix.set_selection(ar, ac, hr, hc)
helix.set_input_value(popup_id, node_id, value)   // 强制改 input 值（光标置末尾）
helix.open_popup({ render, onKey?, onClose?, width?, height?, position? }) -> id
helix.open_panel({ side, size, render, onKey?, onClose?, focusable? }) -> id
helix.close_panel(id)
helix.move_panel(id, side)
helix.open_file(path, { row?, col? })
helix.lsp.hover([{ row, col }]) -> Promise       // → Hover | null
helix.lsp.completion([{ row, col }]) -> Promise  // → CompletionItem[] | {isIncomplete, items} | null
helix.lsp.goto_definition([{ row, col }]) -> Promise // → Location | Location[] | LocationLink[] | null（含 path）
helix.lsp.document_symbols() -> Promise          // → DocumentSymbol[] | null
helix.lsp.format() -> Promise                    // → {applied:true} | null（自动应用）
helix.lsp.rename(newName) -> Promise             // → {applied:true,files:n} | null（自动应用）
helix.lsp.code_actions([{ row, col }]) -> Promise // → CodeAction[] | null
helix.lsp.execute_code_action(action) -> Promise // → {applied:true} | null（自动应用）
helix.el(type, arg, opts)                       // text|row|col|scroll|button|input
helix.open_terminal({ cmd, side, size, onExit? }) -> view_id
helix.set_terminal_mode(view_id, "dock"|"fullscreen"|"floating"|"minimized")
helix.term_clear(view_id)
helix.resize_term(view_id, size)
helix.split(dir, { terminal:{cmd,size?} } | { panel:{render,onKey?,size?} }) -> leaf_id
helix.close_leaf(id)
helix.zoom(id) / helix.unzoom()
helix.resize_leaf(id, ratio)
helix.focus(id)
helix.get_layout()                              // {tree, active, zoomed} | null
helix.restore_layout(layoutObj)                 // 仅写缓存
helix.read_dir(path)                            // 同步 -> [{name, is_dir, path}]
helix.read_file_async(path)                     // Promise → 内容（UTF-8 lossy）
helix.write_file_async(path, content)           // Promise
helix.stat_async(path)                          // Promise → {is_dir, size, mtime}
helix.glob_async(pattern)                       // Promise → paths[]
helix.set_statusline(fn)                        // fn({path, mode, cursor}) -> string|null
helix.set_buffer_icon(fn)                       // fn(path) -> string|null
helix.set_completion_render(fn)                 // fn(ctx) -> 行内容数组|组件树（补全菜单行钩子）
helix.set_diagnostic_icons({...})               // 诊断标记列图标（gutter）
helix.set_theme({ scope: color | {fg,bg,modifiers} })
helix.reset_theme()
helix.get_style(scope)
helix.theme_info()
helix.set_theme_name(name)
helix.load(name) -> exports
helix.export(obj)
helix.lazy(name, ...commands)
```

### 数据形状

```
命令 ctx：{ path, text, cursor: {row,col}, selection: {anchor, head}, doc }
doc：{ path, text, cursor, insert(sr,sc,text), replace(sr,sc,er,ec,text), delete(sr,sc,er,ec) }
按键对象：{ name, shift, ctrl, alt }     // name: 字符 | Enter|Esc|Tab|Backspace|Delete|Insert|方向键|Home|End|PageUp|PageDown|F1..F12
onKey 返回值："close" | "handled" | "ignore"
render 返回值：数组（string|{text,style}）或 el 组件树；签名 render(focus, {width, height})
组件树节点：text|row|col|scroll|button|input（见 §10）
终端模式：dock | fullscreen | floating | minimized
面板方向：right | left | bottom
split 方向：right | left | top | bottom
事件：save | mode-change | buffer-open | buffer-close | doc-change | theme-change
键位模式：normal | insert | select
```

---

## 20. LSP 请求

插件可主动向当前 buffer 的语言服务器发请求。八个方法都挂在 `helix.lsp` 命名空间下，全部返回 Promise；响应为 LSP 协议原始 JSON（`serde_json` 序列化后 `JSON.parse` 透传），字段名与 LSP 协议一致。请求在**调用时**快照当前光标位置，之后移动光标不影响已发出的请求。

```js
const hover = await helix.lsp.hover();            // → Hover | null
const items = await helix.lsp.completion();       // → CompletionItem[] | {isIncomplete, items} | null
const locs  = await helix.lsp.goto_definition();  // → Location | Location[] | LocationLink[] | null
const syms  = await helix.lsp.document_symbols(); // → DocumentSymbol[] | null

// 位置覆盖（可选）：字符坐标 (row, col)，与 set_cursor 一致；缺省 = 当前光标快照
// 注意：row/col 须同传，只传其一（如 { row: 5 }）按缺省（当前光标）静默处理
await helix.lsp.hover({ row: 5, col: 3 });
```

| 方法 | LSP 请求 | 返回 |
|------|----------|------|
| `helix.lsp.hover()` | `textDocument/hover` | `Hover \| null` |
| `helix.lsp.completion()` | `textDocument/completion` | `CompletionItem[] \| {isIncomplete, items} \| null` |
| `helix.lsp.goto_definition()` | `textDocument/definition` | `Location \| Location[] \| LocationLink[] \| null` |
| `helix.lsp.document_symbols()` | `textDocument/documentSymbol` | `DocumentSymbol[] \| null` |

**唯一便利字段**：`goto_definition` 返回的每项（`Location` 或 `LocationLink`）附 `path`（由 `uri`/`targetUri` 解析，剥除 `file://` 前缀并做百分号解码）。跳转/显示直接用 `path`，无需自己解析 URI。

**标量形态**：LSP 协议允许 server 返回**单条** `Location`（非数组），此时 JS 收到裸对象——遍历前先 `Array.isArray` 归一（demo 见下）。

### 返回字段（LSP 协议原始字段）

| 方法 | 主要字段 |
|------|----------|
| `hover` | `contents`（MarkedString \| MarkupContent \| 二者数组，字符串/`{value}`/`{kind, value}` 三种形态）、`range?` |
| `completion` | `items[]`（`label`/`detail`/`documentation`/`insertText`/`data` 等，零丢失）；List 变体（rust-analyzer 等常见）为 `{isIncomplete, items}` 对象 |
| `goto_definition` | 标量 `Location`（非数组）或 `Location[]`（`uri`+`range`）或 `LocationLink[]`（`targetUri`+`targetRange`），每项附 `path` |
| `document_symbols` | `DocumentSymbol[]`（`name`/`kind`/`range`/`selectionRange`/`children`）或 `SymbolInformation[]` |

### `helix.lsp.format()` / `helix.lsp.rename(newName)` / `helix.lsp.code_actions(pos?)` / `helix.lsp.execute_code_action(action)`（LSP 编辑操作）

```js
await helix.lsp.format();                        // → { applied: true } 或 null
await helix.lsp.rename("newName");               // → { applied: true, files: n } 或 null
const actions = await helix.lsp.code_actions();  // → [{ title, kind, ...完整 LSP action }] 或 null
await helix.lsp.execute_code_action(actions[0]); // → { applied: true } 或 null
```

- 自动应用编辑（一次撤销/文件）；rename 可跨 buffer（自动打开未打开文件）；code_actions 两阶段无状态（execute 原样传回列表项）
- 无 server / 能力不支持 → resolve null（与查询类 4 方法一致）；请求失败/协议错误同样 resolve null 而非 reject（与查询类不同）；超时 = 该语言服务器的 `timeout` 配置（默认 20s），到时 reject `Error`
- 响应到达即应用；format 不校验文档版本（插件用 await 时序自行控制），rename 经 apply_workspace_edit 校验版本（过期 → null，不落地陈旧编辑）；format 仅全文档

### 空 / 错语义

| 情形 | 行为 |
|------|------|
| 无 LSP server（文件类型无配置 / server 未启动） | `resolve(null)` |
| server 不支持该功能 | `resolve(null)` |
| server 返回 LSP 协议 error | `reject(Error(message))` |
| 连接断开等异常 | `reject(Error(message))` |
| 请求挂起不返回 | 超时 = 语言服务器 `timeout` 配置（默认 20s，per-server 可配），到时 `reject(Error("Timeout..."))`——promise 不悬挂 |

“没结果”是插件常态（文件类型不匹配、功能未启用），只 resolve `null` 不抛错；只有真正出错的调用才 reject，插件只需为“发请求”兜底。

### 示例：hover 弹窗 + 跳转定义

完整 demo 见 `plugins/features/lsp-hover/index.js`（`:plugin-load plugins/features/lsp-hover/index.js`）：

```js
// :lsp-hover 光标处 hover 弹窗
helix.register_command("lsp-hover", async () => {
  const hover = await helix.lsp.hover();
  if (!hover) return helix.echo("no hover");
  const text = Array.isArray(hover.contents)
    ? hover.contents.map((c) => c.value ?? c).join("\n")
    : hover.contents.value ?? hover.contents;
  helix.open_popup({
    render: () => helix.el("col", String(text).split("\n").map((l) => helix.el("text", l))),
  });
});

// :lsp-goto 跳转到定义（首个结果；path 由 uri/targetUri 注入）
helix.register_command("lsp-goto", async () => {
  const locs = await helix.lsp.goto_definition().catch((e) => {
    helix.echo("lsp-goto error: " + e.message);
    return null;
  });
  if (!locs) return helix.echo("no definition");
  // Scalar(Location) | Location[] | LocationLink[]，统一成数组
  const arr = Array.isArray(locs) ? locs : [locs];
  // 无 path 的项跳过；Location 用 range，LocationLink 用 targetRange
  const l = arr.find((x) => x.path && (x.range ?? x.targetRange));
  if (!l) return helix.echo("no definition");
  const r = l.range ?? l.targetRange;
  helix.open_file(l.path, { row: r.start.line, col: r.start.character });
});
```

> **注意**：`open_file(path, { row, col })` 的定位要求 **row 和 col 都传**。只传其一（如 `{ row: 9 }` 或 `{ col: 3 }`）会静默不定位——文件照常打开，但光标不移动、无报错。

### 手动验证

需要带 LSP 的环境（如 Rust/TS 项目，`languages.toml` 已配置 server）：

```bash
cargo build
# 打开一个带 LSP 的项目文件后：
:plugin-load plugins/features/lsp-hover/index.js
# :lsp-hover 应在光标处弹窗显示 hover；:lsp-goto 应跳到定义位置
```
