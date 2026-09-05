# Helix JS 插件系统:总览与索引

> 本文档描述 helix 魔改版的 JavaScript 插件系统(`helix-js` crate,boa 引擎)。插件为 `.js` 文件。
> 所有 API 通过全局 `helix` 对象暴露。**本文是总览与索引;每个域的详细说明(签名/示例/优缺点)在 [`docs/api/`](api/) 分文件。**

## 目录

- [1. 架构与执行模型](#1-架构与执行模型)
- [2. API 索引(按域)](#2-api-索引按域)
- [3. 综合示例:文件树面板](#3-综合示例文件树面板)
- [4. 已知限制](#4-已知限制)
- [5. API 速查索引](#5-api-速查索引)

---

## 1. 架构与执行模型

理解执行模型是正确使用插件的前提:

```
┌─ 主线程(唯一 JS 线程)───────────────────────────────┐
│  boa Context(!Send,全部 JS 状态/回调都在这)          │
│  · 命令、事件、render、onKey 都在主线程同步执行        │
│  · 队列:UI_REQUESTS(渲染/布局请求,命令边界 drain)    │
│  · 队列:MESSAGES(echo 消息,状态栏刷新)               │
├─ worker 线程(std::thread,无 JS)─────────────────────┤
│  · 进程执行(run_async / spawn / pty)→ TermEvent      │
│  · 异步 fs(read_file_async 等)→ AsyncEvent           │
│  · 结果经 mpsc 通道送回主线程,drain 后 resolve 到回调  │
└──────────────────────────────────────────────────────┘
```

- **JS 永远只在主线程跑**(boa 的 `Context` 是 `!Send`),所以 JS 侧无并发问题。
- **耗时操作必须用异步 API**(`run_async` / `spawn` / `*_async`),它们在 worker 线程执行,promise 恢复(`.then`/`.catch`)回到主线程事件循环。
- **`helix.run`(同步)会阻塞编辑器主线程**——只用于短命令。
- 弹窗/面板/终端现在是**布局树叶子**(面板和终端会真实收缩编辑器布局,不再只是覆盖层)。
- 渲染模型:JS `render` 回调每次重绘全量返回内容 → 布局引擎拍平成 `StyledLine`(多 span)→ 脏格 diff 只写变化的格。

---

## 2. API 索引(按域)

### 加载 / 命令 / 消息 / 键位

`helix.load(name)`(缓存加载)、`helix.export(obj)`(导出)、`helix.lazy(name, ...commands)`(懒加载桩)、`helix.run_command(name, ctx?)`、`helix.register_command(name, fn, doc?)`(一个命令 = 一次撤销)、`helix.echo(text)`、`helix.map(mode, key, command|fn)`。

- 优点:init.js 单一入口;懒加载减启动开销;命令原子应用;键位支持多键序列/回调。
- 局限:跨插件状态只能走 `helix.*` API;boa 嵌套 eval 污染(依赖必须用 `helix.plugin` deps);键位重启失效。

→ 详细见 [`api/loading-commands.md`](api/loading-commands.md)

### 文档编辑与选区

命令 `ctx` = 只读快照 + 编辑队列(`ctx.doc.insert/replace/delete`,0-based,越界 clamp,一次命令一次撤销);`helix.begin_edit()/end_edit()`(批量事务,async 跨 await 用);`helix.by_path(path)`(跨 buffer 编辑);`helix.set_virtual_text/set_highlight`(装饰/标记);`helix.set_cursor/set_selection`(光标/选区,多选区数组)。

- 优点:快照模型无偏移;批量事务;跨 buffer;装饰不污染文本。
- 局限:快照坐标跨 await 会过期;by_path 只查已打开 buffer;装饰坐标不随文本重映射;无多光标编辑。

→ 详细见 [`api/editing.md`](api/editing.md)

### 进程执行与异步文件系统

`helix.run(cmd)`(同步,**阻塞**)、`helix.run_async(cmd)`(Promise)、`helix.spawn({cmd, pty?, onChunk, onExit})`(流式);`helix.read_dir`(同步)、`read_file_async/write_file_async/stat_async/glob_async/read_tree`(异步 Promise)。

- 优点:异步不阻塞;read_tree 递归+排序+depth;glob `**` 跨目录。
- 局限:run 阻塞;spawn 无超时;read_dir 同步不递归;read_tree 一次性返回。

→ 详细见 [`api/process-fs.md`](api/process-fs.md)

### 事件钩子

`helix.on(event, fn)`:文档系(`save` 前/mode-change/buffer-open/close/doc-change 带合并范围/theme-change)、诊断/光标(`lsp-diagnostics`/`cursor-move`/`selection-change`)、终端系(`term-open/mode-change/exit/close/resize/title/key`)、`component-event`。

- 优点:事件丰富;多处理器按序;抛错不阻断;doc-change 带合并范围。
- 局限:只带当前文档;doc 只读快照;doc-change 250ms 防抖。

→ 详细见 [`api/events.md`](api/events.md)

### 弹窗 / 组件树 / 面板 / 界面定制

`helix.open_popup`(模态层,onKey 三态)、`helix.el` 组件树(text/row/col/scroll/button/input,焦点系统 + 引擎权威 input)、`helix.open_panel`(布局叶子)/`close_panel`/`move_panel`;`helix.set_statusline`、`set_buffer_icon`、`set_completion_icon`(kind 列图标)、`set_completion_render`(候选行 JS 渲染,回退原生两列)、`set_diagnostic_icons`、`set_component_render`(组件外观)、`get_component_state`、`set_keymap_hint`(which-key)。

- 优点:声明式树 + 焦点;input 引擎权威;渲染脏格 diff;补全/组件外观全可定制且回退安全。
- 局限:scroll O(内容);无树内点击命中;面板无节点焦点;completion 行钩子每帧每行(勿跑重逻辑);无 Shift-Tab。

→ 详细见 [`api/ui.md`](api/ui.md)

### Picker 与主题

`helix.picker.define/run`(定义数据源,调起**原生 Picker**:nucleo 模糊匹配/滚动/预览/键位全核心;行格式数组或 `{cells, payload}` 分离;内置插件 `plugins/features/picker.js` 提供 files/grep/buffers/symbols);`helix.set_theme`(scope 级实时覆盖)/`reset_theme`/`get_style`/`theme_info`/`set_theme_name`/`on("theme-change")`。

- 优点:picker 性能原生 + 插件可定义任意源;主题覆盖即时生效。
- 局限:picker 候选一次性返回;无旋钮透传;预览仅文件;主题颜色不支持引用 scope。

→ 详细见 [`api/picker-theme.md`](api/picker-theme.md)

### 终端与布局树

`helix.open_terminal`(原生 pty 面板)/`term_write/feed/kill/list/close/resize/clear/save`/`set_terminal_mode`(dock/fullscreen/floating/minimized)/`term_state`(跨会话持久化);布局:`split`/`buffer_open`/`close_leaf`/`zoom`/`unzoom`/`resize_leaf`/`layout_*`/`focus`/`get_layout`/`restore_layout`(半成品)+ `C-w` 窗口模式。

- 优点:原生 pty;四种显示模式;滚动缓冲;zellij 式窗口管理;layout_fix。
- 局限:组合字符/鼠标/选择复制不支持;restore_layout 未接线;PTY 仅 Unix。

→ 详细见 [`api/terminal-layout.md`](api/terminal-layout.md)

### 插件管理

`:plugin install/update/pin/unpin/remove/list/status/reload`(manifest 来源追踪,本地/git 源,依赖解析 plugin.json 递归 + 循环检测);JS API `helix.plugin(name, {deps})` + `helix.plugin.install/update/remove`。

- 优点:来源追踪;卸载/更新闭环;依赖自动装;装后即用。
- 局限:JS API fire-and-forget(结果看状态栏);git 操作同步阻塞;依赖不自动加载。

→ 详细见 [`api/plugin.md`](api/plugin.md)

### LSP

`helix.lsp.hover/completion/goto_definition/document_symbols`(查询)+ `format/rename/code_actions/execute_code_action`(编辑,自动应用)。返回 LSP 协议原始 JSON;失败 resolve null 不悬挂;超时(默认 20s)reject。

- 优点:查询/编辑全链路;`path` 便利字段;版本校验(rename)。
- 局限:execute 冻结主线程;format 不校验版本。

→ 详细见 [`api/lsp.md`](api/lsp.md)

---

## 3. 综合示例:文件树面板

组合 fs + 组件树 + 面板 + 文档打开,写一个最小文件树:

```js
// filetree.js
let cwd = helix.run("pwd").trim();      // 起始目录(同步 run 仅此一次,可接受)
let entries = [];
let expanded = {};                      // 目录展开状态(JS 侧自己维护)

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
      // 每行一个 text:目录可再展开,文件可打开
      const rows = entries.map((e) =>
        helix.el("text",
          (e.is_dir ? "[d] " : "    ") + e.name,
          { style: focus === "tree-" + e.path ? "ui.selection" : (e.is_dir ? "ui.virtual" : null) })
      );
      return helix.el("col", rows, { gap: 0 });
    },
    onKey: (key, doc) => {
      if (key.name === "Enter") {              // 打开/进入
        const hit = entries[key._row ?? 0];    // 简化:JS 需自行维护行命中(见 §4 已知限制)
        if (hit.is_dir) loadDir(hit.path);
        else helix.open_file(hit.path);
        return "handled";
      }
      return "ignore";
    },
  });
});
```

> 注:组件树目前**没有节点级点击命中**——树内导航(上下移动选中行)需要 JS 自己维护当前行号,并在 `render` 里按行号高亮(上例做了简化)。

---

## 4. 已知限制

| 限制 | 说明 |
|------|------|
| **无异步语言特性** | boa 无 `setTimeout`/网络/事件循环;异步只来自 `run_async`/`spawn`/`*_async` 的 Promise(await/.then) |
| **同步阻塞** | `helix.run` 阻塞主线程;`spawn` 无超时(挂死命令一直占着 worker) |
| **终端视图非完整仿真器** | 宽字符/组合字符、鼠标、选择复制不支持;滚回只存不显示(上限 1000 行) |
| **Shift-Tab 不可用** | `KeyCode` 无 BackTab,焦点只能 Tab 正向循环 |
| **面板无节点焦点** | 面板 render 固定收到 `focus = null`(节点焦点只在弹窗生效) |
| **无树内命中** | 组件树节点不响应点击/悬停;命中检测(哪一行被选中)由 JS 自己维护 |
| **restore_layout 未接线** | `get_layout`/`restore_layout` 目前只读写缓存,布局树重建未实现 |
| **Scroll 全量布局** | scroll 是 O(内容) 而非 O(视口),几千行内容每帧全量布局 |
| **事件 ctx 范围** | 事件只带当前文档(其他分屏文档不触发) |
| **无多光标** | 选区/光标 API 只操作主光标 |
| **Unix-only** | `helix.run`/`spawn`/PTY 依赖 `sh` 与 libc openpty(Windows 不可用) |
| **键位不持久** | `helix.map` 绑定重启失效(插件启动时重新注册) |
| **插件间无共享状态** | 每个插件独立 IIFE 作用域;共享需通过 `helix.*` API 或外部文件 |
| **嵌套 load 污染风险(已修复)** | boa 0.21 的嵌套 eval 破坏外层闭包绑定(`typeof` 变 object、函数不可调用,实测 2026-08-26;DefInitVar 越界 panic)。**boa 0.22 升级后验证修复**(2026-08-31 复现测试通过);防御(icons.js 工具函数 `|| {}` 兜底)保留作双保险。旧建议(依赖用 `helix.plugin` deps 声明、回调不放嵌套 load 后)仍是最佳实践 |

---

## 5. API 速查索引

### 全局对象

```
helix.echo(text)
helix.register_command(name, fn, doc?)
helix.run_command(name, ctx?)
helix.run(cmd)                                  // 同步,抛错,64KB 截断
helix.run_async(cmd)                          // Promise:await → stdout;失败 reject Error
helix.spawn({ cmd, pty?, onChunk, onExit }) -> id
helix.term_write(id, text)
helix.term_kill(id)
helix.term_list()                        // [{view_id, cmd}] 当前终端
helix.term_close(view_id)                // 按 id 关闭终端
helix.term_resize(id, rows, cols)               // pty TIOCSWINSZ,Unix
helix.on(event, fn)                             // save | mode-change | buffer-open | buffer-close | doc-change | ...
helix.map(mode, key, commandOrFn)
helix.set_cursor(row, col)
helix.set_selection(ar, ac, hr, hc)
helix.set_input_value(popup_id, node_id, value)   // 强制改 input 值(光标置末尾)
helix.open_popup({ render, onKey?, onClose?, width?, height?, position? }) -> id
helix.open_panel({ side, size, render, onKey?, onClose?, focusable? }) -> id
helix.close_panel(id)
helix.move_panel(id, side)
helix.open_file(path, { row?, col? })
helix.picker.define(name, { columns, items, preview?, action? })
helix.picker.run(name)
helix.lsp.hover([{ row, col }]) -> Promise       // → Hover | null
helix.lsp.completion([{ row, col }]) -> Promise  // → CompletionItem[] | {isIncomplete, items} | null
helix.lsp.goto_definition([{ row, col }]) -> Promise // → Location | Location[] | LocationLink[] | null(含 path)
helix.lsp.document_symbols() -> Promise          // → DocumentSymbol[] | null
helix.lsp.format() -> Promise                    // → {applied:true} | null(自动应用)
helix.lsp.rename(newName) -> Promise             // → {applied:true,files:n} | null(自动应用)
helix.lsp.code_actions([{ row, col }]) -> Promise // → CodeAction[] | null
helix.lsp.execute_code_action(action) -> Promise // → {applied:true} | null(自动应用)
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
helix.read_file_async(path)                     // Promise → 内容(UTF-8 lossy)
helix.write_file_async(path, content)           // Promise
helix.stat_async(path)                          // Promise → {is_dir, size, mtime}
helix.glob_async(pattern)                       // Promise → paths[]
helix.read_tree(path, { depth? })               // Promise → [{name, is_dir, path}](递归)
helix.set_statusline(fn)                        // fn({path, mode, cursor}) -> string|null
helix.set_buffer_icon(fn)                       // fn(path) -> string|null
helix.set_completion_icon(fn)                   // fn(kindNum) -> 图标字符(kind 列)
helix.set_completion_render(fn)                 // fn(ctx) -> 行内容数组|组件树(补全菜单行钩子)
helix.set_diagnostic_icons({...})               // 诊断标记列图标(gutter)
helix.set_component_render(id, fn)              // 组件外观 JS 绘制
helix.get_component_state(id)                   // 组件状态读取
helix.set_keymap_hint(fn)                       // which-key 提示
helix.set_theme({ scope: color | {fg,bg,modifiers} })
helix.reset_theme()
helix.get_style(scope)
helix.theme_info()
helix.set_theme_name(name)
helix.load(name) -> exports
helix.export(obj)
helix.lazy(name, ...commands)
helix.plugin(name, { deps, version? })          // 声明加载依赖
helix.plugin.install(arg) / update(name?) / remove(name)   // 管理(结果走状态栏)
```

### 数据形状

```
命令 ctx:{ path, text, cursor: {row,col}, selection: {anchor, head}, doc }
doc:{ path, text, cursor, insert(sr,sc,text), replace(sr,sc,er,ec,text), delete(sr,sc,er,ec) }
按键对象:{ name, shift, ctrl, alt }     // name: 字符 | Enter|Esc|Tab|Backspace|Delete|Insert|方向键|Home|End|PageUp|PageDown|F1..F12
onKey 返回值:"close" | "handled" | "ignore"
render 返回值:数组(string|{text,style})或 el 组件树;签名 render(focus, {width, height})
组件树节点:text|row|col|scroll|button|input(见 api/ui.md)
picker 行:数组 [c1, c2, ...] 或 { cells: [...], payload: [...] };preview/action 收到 payload 数组(见 api/picker-theme.md)
终端模式:dock | fullscreen | floating | minimized
面板方向:right | left | bottom
split 方向:right | left | top | bottom
事件:save | mode-change | buffer-open | buffer-close | doc-change | theme-change | lsp-diagnostics | cursor-move | selection-change | term-* | component-event
键位模式:normal | insert | select
```
