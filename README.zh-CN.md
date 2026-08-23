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

### 完整 API 列表

`echo` `register_command` `run_command` `on` `map` `el` `export` `plugin` `load` `lazy`
`open_popup` `open_panel` `close_panel` `move_panel` `read_dir` `open_file` `set_buffer_icon` `set_statusline`
`open_terminal` `term_write` `term_feed` `term_kill` `term_list` `term_close` `term_resize` `term_clear` `term_save` `set_terminal_mode`
`split` `buffer_open` `close_leaf` `zoom` `unzoom` `resize_leaf` `layout_resize` `layout_swap` `layout_minimize` `layout_focus` `layout_swap_dir` `layout_equalize` `layout_fix` `focus` `get_layout` `restore_layout`
`set_cursor` `set_selection` `get_str`
`set_theme` `reset_theme` `get_style` `theme_info` `set_theme_name` `set_diagnostic_icons`
`run` `run_async` `spawn` `read_file_async` `write_file_async` `stat_async` `glob_async`
`set_component_render` `get_component_state` `set_keymap_hint` `term_state`

**事件白名单**:`save` `mode-change` `buffer-open` `buffer-close` `doc-change` `theme-change`
`term-open` `term-mode-change` `term-exit` `term-close` `term-resize` `term-title` `term-key` `component-event`

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

## 📄 文档

- 本 fork 设计文档:`docs/superpowers/specs/2026-08-15-js-ui-rendering-design.md`(JS 视图层分层)
- 交接记录:`docs/handoff-2026-08-14.md`(窗口模式/终端/插件演进)
- 原版 Helix 文档:[官网](https://helix-editor.com) · [文档](https://docs.helix-editor.com/) · [键位表](https://docs.helix-editor.com/keymap.html)

## 🙏 致谢

上游 [Helix 编辑器](https://github.com/helix-editor/helix)(Kakoune/Neovim 灵感,Rust 编写)——本项目所有基础能力来自它,本分支仅在其上增加 JS 插件系统与窗口模式等特性。
