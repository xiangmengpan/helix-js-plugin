[English](README.md) · [中文](README.zh-CN.md)

<div align="center">

<h1>
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="logo_dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="logo_light.svg">
  <img alt="Helix" height="128" src="logo_light.svg">
</picture>
</h1>

**Helix + JS 插件系统 + 窗口模式(zellij 化)**

</div>

## 📌 这是什么

基于 [Helix 编辑器](https://github.com/helix-editor/helix)(Rust 编写的 Kakoune/Neovim 风格编辑器)的深度魔改分支,核心变化:

1. **JS 插件系统**:内置 boa 嵌入式 JS 引擎,插件可以注册命令、渲染 UI 组件、监听事件、接管快捷键提示
2. **zellij 式窗口模式**:全局 C-w 窗口管理(聚焦/交换/缩放/最小化/关闭),任何叶子焦点下可用
3. **终端增强**:原生终端面板(Esc 切滚动模式、滚动缓冲、复制、钩子 API、状态持久化)
4. **快捷键提示插件化**:which-key 中文提示,位置可配置,替代内置 Info

原版 Helix 文档:[官网](https://helix-editor.com) · [文档](https://docs.helix-editor.com/) · [键位表](https://docs.helix-editor.com/keymap.html)

## ✨ 新特性

### 全局窗口模式(compositor 层,Rust)

任何焦点(编辑器/终端/面板)normal 模式按 `C-w` 进入;模式内:

| 键 | 动作 |
|---|---|
| `h` `j` `k` `l` | 方向聚焦(左/下/上/右) |
| `H` `J` `K` `L` | 方向交换 |
| `C-h` `C-j` `C-k` `C-l` | 尺寸 ∓5% |
| `x` | 关闭窗口 |
| `z` | 最小化/还原 |
| `f` | 最大化/还原 |
| `Enter` | 确认当前窗口并退出 |
| `Esc` / `C-w` | 退出 |

- insert 模式 `C-w` 保留删词原义;终端 Insert 直通模式 `C-w` 放行给 pty(vim/emacs 内 C-w)
- 终端关闭统一走窗口模式 `x`(Esc 切 normal 滚动模式,i 回 insert)

### JS 插件系统

Rust 提供底层服务(布局树、pty、网格、绘制原语、状态机),JS 负责视图与行为:

- **组件状态只读**:`helix.get_component_state(id)` → 终端 mode/title/minimized 等
- **组件视图**:`helix.set_component_render(id, fn)` → 任意组件外观由 JS 绘制(标题条/标签条)
- **快捷键提示**:`helix.set_keymap_hint(fn)` → which-key 中文提示,返回 `{text, position}` 指定位置
- **鼠标交互**:`helix.on("component-event", ...)` → 点击组件交给 JS 决策
- **终端钩子**:term-key/close/open/exit/title/resize/mode-change + `term_state` 持久化

### 终端增强

- Esc → normal 滚动模式(滚动缓冲、`y` 复制、`g`/`G` 跳转),i → insert 直通
- 终端标题条/最小化条可经 JS 视图绘制(网格渲染留 Rust,性能路径不动)
- OSC 标题、滚动偏移、模式经 `get_component_state` 暴露

## 🔌 JS API 使用

### 命令与事件

```js
// 注册命令(:my-cmd 执行;第三参为命令说明)
helix.register_command("my-cmd", (ctx) => { ... }, "我的命令");

// 监听事件(save / buffer-open / mode-change / doc-change / theme-change ...)
helix.on("save", (doc) => { ... });

// 加载/导出模块
helix.load("lib/icons.js");                  // 相对插件目录
helix.plugin("my-plugin", { deps: ["lib/icons.js"] });
helix.export({ run: myFn });                 // 供其他插件 load 后调用
```

### 布局与窗口

```js
const id = helix.split("right", { terminal: { cmd: "bash", size: 40 } }); // 分屏
const id = helix.buffer_open("/path/file.js", { split: "right" });        // buffer 叶子
helix.focus(id);          // 聚焦叶子
helix.layout_fix(id, true);  // 固定叶子(免疫交换/关闭/最小化)
const layout = helix.get_layout(); // { tree, active, leafs:[{id, fixed}], zoomed, minimized }
helix.zoom(id); helix.unzoom();
helix.layout_minimize(id, true);   // 最小化
helix.layout_resize(id, "h", 0.05); // resize
```

### UI 渲染

```js
// 状态栏
helix.set_statusline((ctx) => [ { text: " N ", style: "ui.statusline.normal" }, ctx.path, ... ]);

// 弹窗/面板(组件树:type text/row/col/scroll/button/input)
const popupId = helix.open_popup({
  width: 40, height: 10,
  render: () => helix.el("col", [
    helix.el("text", "标题", { style: "ui.popup" }),
    helix.el("text", "内容"),
  ]),
  onKey: (key) => { ... },
  onClose: () => { ... },
});
```

### 组件视图层(JS 渲染组件外观)

```js
// 终端标题条:JS 画顶部一行,网格留 Rust
helix.set_component_render(tid, (ctx) => [
  { type: "text", text: " " + (helix.get_component_state(tid).title || "terminal") },
]);

// 布局标签条(顶部槽位,helix.TABBAR_ID 常量)
helix.set_component_render(TABBAR_ID, () => {
  const layout = helix.get_layout();
  return layout.leafs.map((leaf) => ({
    type: "text", text: " " + leaf.id + " ", style: leaf.id === layout.active ? "ui.selection" : "ui.statusline.inactive",
  }));
});

// 鼠标点击组件
helix.on("component-event", (id, { kind, x, y }) => {
  if (kind === "click") { ...; return true; } // 返回 true 消费事件
});
```

### 快捷键提示(which-key)

```js
helix.set_keymap_hint((ctx) => ({
  text: ctx.entries.map((e) => e.keys + "  " + e.doc).join("\n"),
  position: "bottom-right", // bottom-right|bottom-left|top-right|top-left|center
}));
// 返回 null 则不显示;未注册回调 → 内置 Info 兜底
```

### 终端钩子

```js
helix.on("term-key", (ptyId, { code, shift, ctrl, alt }) => {
  if (code === "esc") return "normal";  // 或 pass/consume/minimize/close
});
helix.on("term-close", (ptyId, reason) => false);   // 返回 false 阻止关闭
helix.on("term-open", (ptyId, cmd) => { ... });
helix.on("term-exit", (ptyId, code) => { ... });    // 进程退出
helix.on("term-title", (ptyId, title) => { ... });
helix.term_state(ptyId, "cwd", "/path");            // 状态持久化(跨会话)
helix.term_state(ptyId, "cwd");                     // 读取
```

### Buffer 遍历、诊断与文件监听

```js
// 打开文档快照(只读;id 会话内有效)
helix.buffers();            // → [{id, path, name, dirty, language}]
helix.current_buffer();     // → id
helix.focus_buffer(id);     // 当前 view 切换到该文档

// 当前文档的 LSP 诊断(数据来自 Document::diagnostics)
helix.diagnostics();        // → [{line, message, severity, code, source}]
helix.on("lsp-diagnostics", (docId, diags) => { ... });

// 文件系统监听(notify 支撑,500ms 防抖)
const wid = helix.watch("/path/to/dir", (events) => {
  // events: [{kind: "create"|"modify"|"delete"|"rename", path}]
  filetree.refresh();
});
helix.unwatch(wid);

// 光标/选区事件(帧级节流)
helix.on("cursor-move", (docId, { row, col, mode }) => { ... });
helix.on("selection-change", (docId, { count, primary }) => { ... });
```

### API 总览(按域分组,附优缺点)

**命令与消息**

| API | 说明 | 优点 | 局限 |
|---|---|---|---|
| `register_command(name, fn, doc?)` | 注册 `:name` 命令 | 一个命令 = 一次撤销;UI 请求命令边界自动应用 | 主线程同步执行,重活请用异步 API |
| `run_command(name, ctx?)` | 程序化调插件命令 | 脚本化/懒加载桩底座 | 只能调已注册的插件命令 |
| `echo(text)` | 状态栏消息 | 简单直观 | 无历史(覆盖式) |

**文档编辑与选区**

| API | 说明 | 优点 | 局限 |
|---|---|---|---|
| `begin_edit()/end_edit()` | 批量编辑事务 | 重构类插件一次撤销 | 需成对调用 |
| `by_path(path, fn)` | 跨 buffer 访问 | 按路径编辑任意文档 | 只读快照 + 队列式编辑 |
| `set_virtual_text/set_highlight` | 装饰/标记 | 按 doc 整体替换,热重载清理 | 渲染层,不参与文本操作 |
| `set_cursor/set_selection` | 光标/选区 | 多选区数组形态 | — |

**进程执行**

| API | 说明 | 优点 | 局限 |
|---|---|---|---|
| `run(cmd)` | 同步执行 | 简单,结果即用 | **阻塞主线程**,只用于短命令 |
| `run_async(cmd)` | 异步执行 | 不阻塞;Promise 恢复回主线程 | 无流式输出 |
| `spawn({cmd, onChunk, onExit})` | 流式进程 | 逐块输出,pty 支持 | 需自己处理块拼接 |

**异步文件系统**(均返回 Promise,worker 线程执行)

| API | 说明 | 优点 | 局限 |
|---|---|---|---|
| `read_dir(path)` | 列目录(同步) | 简单 | 同步阻塞;不递归 |
| `read_file_async/write_file_async` | 读写文件 | 异步不阻塞 | UTF-8 lossy |
| `stat_async(path)` | 文件元数据 | `{is_dir, size, mtime}` | — |
| `glob_async(pattern)` | 通配匹配 | `*`/`**`/`?` | 相对 CWD |
| `read_tree(path, {depth})` | 递归目录树 | 目录先行按名排序;depth 限深 | 一次性返回,大目录有延迟 |

**事件钩子** `on(event, fn)`:`save` `mode-change` `buffer-open` `buffer-close` `doc-change`(带合并范围)`theme-change` `lsp-diagnostics` `cursor-move` `selection-change` + 终端系(`term-open/mode-change/exit/close/resize/title/key`)`component-event`。优点:同事件多处理器按序;处理器抛错不阻断主流程。局限:doc 是只读快照 + 编辑队列。

**键位绑定** `map(mode, key, command|fn)`:重复绑定即覆盖;支持多键序列与修饰键;回调自动注册隐藏命令。局限:重启失效(插件启动时重新注册)。

**弹窗/面板/组件树**:`open_popup`(覆盖层弹窗,onKey 可 return close/handled/ignore)`open_panel`(侧边面板,布局树叶子)`close_panel` `move_panel` `el`(组件树:row/col/scroll/button/input)。优点:渲染与布局引擎分离,脏格 diff;input 组件引擎权威(onChange 自动回调)。局限:popup 重复打开替换前一个。

**Picker 选择器** `helix.picker.define/run`:定义数据源,调起**原生 Picker**(nucleo 模糊匹配/滚动/预览/键位全核心)。优点:性能原生;插件可定义任意源(files/grep/buffers/symbols);行格式数组或 `{cells, payload}` 分离。局限:候选一次性返回(非流式);行渲染不支持每行组件。

**终端**:`open_terminal` `term_write/feed/kill/list/close/resize/clear/save` `set_terminal_mode` `term_state`(跨会话持久化)+ 终端钩子(`term-key` 返回 normal/pass/consume/minimize/close)。优点:原生 pty 面板,四种显示模式,滚动缓冲。局限:键位直通需手动管理模式。

**布局树**:`split` `buffer_open` `close_leaf` `zoom/unzoom` `resize_leaf` `layout_resize/swap/minimize/focus/swap_dir/equalize/fix` `focus` `get_layout` `restore_layout`。优点:zellij 式窗口管理,任意叶子焦点;layout_fix 免疫交换/关闭。局限:restore_layout 序列化半成品(见 §14)。

**Buffer/诊断/监听**:`buffers` `current_buffer` `focus_buffer` `diagnostics` `watch/unwatch`(notify 支撑 500ms 防抖)。优点:文档快照只读安全。局限:id 会话内有效。

**界面定制**:`set_statusline` `set_buffer_icon` `set_completion_render`(候选行 JS 渲染,回退原生两列)`set_completion_icon`(kind → 图标字符)`set_diagnostic_icons` `set_component_render`(组件外观 JS 绘制)`get_component_state` `set_keymap_hint`(which-key)。优点:渲染层全可定制;未注册/抛错回退默认。局限:completion 行钩子每帧每行调用——不要跑重逻辑。

**主题**:`set_theme`(实时覆盖,scope 级)`reset_theme` `get_style` `theme_info` `set_theme_name`(异步切换)。优点:即时生效,继承主题正确;语法 scope 也覆盖。局限:颜色不支持引用另一 scope。

**插件管理**:`helix.plugin(name, {deps})` 声明加载依赖;`helix.plugin.install(path|git-url)`(manifest 追踪 + 依赖解析 + 装后加载)`update`(git pull,失败保留旧版)`remove`。优点:来源追踪、卸载、更新、依赖递归、循环检测;命令面 `:plugin install/update/pin/unpin/remove/status`。局限:JS API 结果走状态栏(fire-and-forget);pin 仅 git 源。

**LSP**:`helix.lsp.hover/completion/document_symbols/workspace_symbols/format/rename/code_actions/execute_code_action`(见 [api/lsp.md](docs/api/lsp.md))。优点:查询/编辑全链路,失败 resolve null 不悬挂。局限:block_on 冻结主线程(code_action 执行)。

## 📦 现有插件

位于 `~/.config/helix/plugins/`(`init.js` 自动加载;仓库镜像在 `plugins/`):

| 插件 | 功能 |
|---|---|
| `features/filetree/` | 侧边文件树面板(图标、展开/折叠、Enter 打开并聚焦) |
| `features/terminal.js` | 终端命令(:term/:vterm/:hterm)、面板管理 |
| `features/statusline.js` | 状态栏美化(模式图标、文件类型图标、git 分支、诊断) |
| `features/which-key.js` | 快捷键中文提示(全键位映射,位置可配置) |
| `features/tabbar.js` | 布局标签条示范(顶部槽位,点击聚焦) |
| `lib/icons.js` | 统一图标映射表(文件类型/目录/模式/诊断/git) |
| `lib/layout.js` | 布局命令封装(:layout-focus/swap/resize/minimize 等) |

安装:复制到 `~/.config/helix/plugins/`,在 `~/.config/helix/init.js` 中 `helix.load("features/xxx.js")`;修改后 `:plugin-reload` 生效。

## 🛠 构建

```bash
cargo build --release
# 需要 nerd font 终端字体以显示图标
```

## 🧩 核心依赖

- [boa](https://github.com/boa-dev/boa) — 嵌入式 JavaScript 引擎(插件系统)
- [vte](https://github.com/alacritty/vte) — 终端模拟器转义序列解析(原生终端面板)
- [notify](https://github.com/notify-rs/notify) — 文件系统事件监听(helix.watch)

## 📄 文档

- **插件 API**:[`docs/plugin-api.md`](docs/plugin-api.md)(总览与索引)· [`docs/api/`](docs/api/)(分域详细:说明/示例/优缺点)
- 本 fork 设计文档:`docs/superpowers/specs/2026-08-15-js-ui-rendering-design.md`(JS 视图层分层)
- 交接记录:`docs/handoff-2026-08-14.md`(窗口模式/终端/插件演进)
- 原版 Helix 文档:[官网](https://helix-editor.com) · [文档](https://docs.helix-editor.com/) · [键位表](https://docs.helix-editor.com/keymap.html)

## 🙏 致谢

上游 [Helix 编辑器](https://github.com/helix-editor/helix)(Kakoune/Neovim 灵感,Rust 编写)——本项目所有基础能力来自它,本分支仅在其上增加 JS 插件系统与窗口模式等特性。
