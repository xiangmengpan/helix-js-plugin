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
2. **zellij 式平级模式**:五个平级模式(`C-g`/`C-p`/`C-n`/`C-h`/`C-y`),任意叶子焦点下可用;已取代旧的全局 `C-w` 单模式
3. **终端增强**:原生终端面板(引擎换成 `alacritty_terminal`:真实 reflow、鼠标上报、括号粘贴、CJK/组合字符;Esc 切滚动模式、滚动缓冲、复制、钩子 API、状态持久化)
4. **快捷键提示插件化**:which-key 中文提示,位置可配置,替代内置 Info
5. **插件键位作用域**:面板/弹窗的键位是**声明式数据** —— 用户可在 `[plugins.<插件名>.keys]` 改键,
   `?` / `:keys` / `:which-key` 读同一份表,并接进输入层(全局前缀可穿透、Esc 回焦点、状态栏 `layers` 段显示层名)

原版 Helix 文档:[官网](https://helix-editor.com) · [文档](https://docs.helix-editor.com/) · [键位表](https://docs.helix-editor.com/keymap.html)

## ✨ 新特性

### zellij 式平级模式(compositor 层,Rust)

任何焦点(编辑器/终端/面板)normal 模式按各模式自己的前缀键进入;
**任一模式内按任一前缀键可直接切到那个模式,按同一个键回 Normal**。

| 模式 | 前缀 | 模式内键 |
|---|---|---|
| **Pane** | `C-p` | `h j k l` 聚焦 · `H J K L` 交换 · `p`/`P`/`Tab` 切下一个/上一个窗 · `n`新分屏 `d`下分 `r`右分 · `x` 关闭 · `f` 全屏 · `z` 最小化 · `w`/`e` 浮动/收回 · `i` pin · `s` 与兄弟窗堆叠 |
| **Resize** | `C-n` | `h j k l` 向该方向增大 · `H J K L` 减小 · `=`/`-` 宽度 ∓5%(活动窗是浮窗时则改浮窗尺寸) |
| **Move** | `C-h` | `h j k l` 与方向邻居交换(浮窗则搬位置) |
| **Scroll** | `C-y` | `j`/`k` 行 · `d`/`u` 半页 · `C-f`/`C-b`、`h`/`l` 整页 |
| **Locked** | `C-g` | 除 `C-g` 外全部按键原样交给当前窗口 |

- `Enter` 或 `Esc` 退模式：`Enter` = **进入当前窗格**（焦点留在 `h j k l` 刚选中的窗口，直接继续编辑）；
  状态栏显示 `PANE`/`RESIZE`/`MOVE`/`SCROLL`/`LOCKED`，并有中文键位提示
- 拦截规则:前缀键在 insert / 终端直通时**放行给叶子**(`C-h` 在 insert 里是删词);
  唯一例外是 `C-g` —— 它在所有状态下都拦,否则从终端直通里进不了 Locked
- 旧的全局 `C-w` **已删除**(insert 模式的 `C-w` 仍是编辑器删词命令);
  上游的 `Space w` 窗口子树保留
- 终端关闭统一走 Pane 模式 `x`(Esc 切 normal 滚动模式,i 回 insert)

### JS 插件系统

Rust 提供底层服务(布局树、pty、网格、绘制原语、状态机),JS 负责视图与行为:

- **组件状态只读**:`helix.get_component_state(id)` → 终端 mode/title/minimized 等
- **组件视图**:`helix.set_component_render(id, fn)` → 任意组件外观由 JS 绘制(标题条/标签条)
- **快捷键提示**:`helix.set_keymap_hint(fn)` → which-key 中文提示,返回 `{text, position}` 指定位置
- **鼠标交互**:`helix.on("component-event", ...)` → 点击组件交给 JS 决策
- **终端钩子**:term-key/close/open/exit/title/resize/mode-change + `term_state` 持久化

### 插件键位作用域

面板/弹窗把键位**声明成数据**,不再在 `onKey` 里按键名分支:

```js
helix.keymap.scope("filetree", {
  keys: {
    q:      { action: "close", desc: "关闭面板" },
    "C-\\":  "leave",                              // 简写:值就是动作名
    j:      { action: "down",  desc: "下移" },
  },
  extends:     "panel",                 // 可选预设:q / Esc / ? / / / j k / g
  passthrough: [":", "C-w", "C-p"],     // 这些全局前缀永远交回编辑器
  esc:         "blur",                  // 插件没消费 Esc ⇒ 回焦点、面板保留
});

// onKey 里只按**动作名**分派
const hit = helix.keymap.resolve("filetree", key);   // onKey 的对象或 "C-\\" 字符串都收
if (!hit) return "ignore";                            // 未绑定 ⇒ 穿透(面板非模态)
if (hit.command) { helix.run_command(hit.command); return "handled"; }
switch (hit.action) { case "close": /* … */ }
```

用户在**插件自己的表**里改键 —— 不改插件源码、不重启:

```toml
[plugins.filetree]
show_hidden = false            # 普通插件配置也在这张表里

[plugins.filetree.keys]        # 主作用域:scope 名 = 插件名
"q"   = ":filetree"
"C-s" = ":filetree-find"       # 命令在插件加载后解析 ⇒ 插件自己的命令能直接写

[plugins.filetree.keys.help]   # 辅助作用域:`filetree.help`(`?` 帮助弹层)
"esc" = ":filetree"
```

- 单一来源:`?` 帮助、`:keys` 的「插件作用域」段、`:which-key`、配置表读的是同一份声明。
- 给 `open_panel` / `open_popup` 传 `scope`,该层占着键盘时状态栏 `layers` 段显示 `FILETREE`
  (`helix.keymap.active_scope()` 返回名字)。
- `passthrough` 在调 `onKey` **之前**放行 ⇒ 模态弹窗里 `:` / `C-w` 照常可用；
  但**别**给文本输入弹层加 `:` —— 那里 `:` 是要输入的字符。
- `helix.map` 覆盖内置键会提示一次(状态栏 + 插件日志),`:keys` 里标 `←插件 <名字>`。
- 限制:只支持单键(无 `"gg"` 序列);键名用核心写法(`esc`,不写 `<esc>`)。

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
helix.load("filetree");                      // 按**插件名**加载另一个插件
helix.plugin("my-plugin", { deps: ["filetree"] }); // deps 也是插件名
helix.export({ run: myFn });                 // 供其他插件 load 后调用
```

### 布局与窗口

```js
// 创建:`split` 仍用旧名(它还没有新等价物 —— 统一入口 pane.open({content, place}) 尚未设计)
const id = helix.split("right", { terminal: { cmd: "bash", size: 40 } });

// ── pane.*(16 个;词汇对齐 zellij:float / embed)──
helix.pane.list();                    // → [{id, kind, place, focused, fixed, pinned, z?, rect?}]
helix.pane.info(id);                  // 单个 pane(无 → null);与 list() 同一份快照
helix.pane.focus(id); helix.pane.focus_dir(id, "left"); helix.pane.move(id, "down");
helix.pane.resize(id, 0.3); helix.pane.close(id);
helix.pane.zoom(id); helix.pane.unzoom();
helix.pane.minimize(id, true); helix.pane.equalize(id); helix.pane.fix(id, true);
helix.pane.float(id); helix.pane.embed(id); helix.pane.raise(id); helix.pane.pin(id, true);

// ── stack.*(`C-p s` 的编程接口;**组恒为 2 个 pane**)──
helix.stack.list();                   // → [{anchor, members}]
helix.stack.create(id);               // 与**兄弟窗**建组(非兄弟 = 无效操作)
helix.stack.activate(id, member);     // 让 member 成为当前显示的那个
helix.stack.remove(id);

// ── layout.*(序列化 + 文件持久化)──
helix.layout.get(); helix.layout.restore(dump);
helix.layout.save("dev"); helix.layout.load("dev"); helix.layout.list(); helix.layout.delete("dev");
// 命令行等价::layout save|load|list|delete <name>

// ── buffer.* ──
helix.buffer.list(); helix.buffer.current(); helix.buffer.focus(id);

// ── icons.*(核心单一来源;零依赖,不必声明 deps)──
helix.icons.file("src/main.rs"); helix.icons.dir(true); helix.icons.mode("normal");
helix.icons.diagnostic("error"); helix.icons.git("M"); helix.icons.completion(3);
helix.icons.enabled(false);  // 一处关掉全部 nerd font 图标
                             //(或 config.toml 里 [plugins.icons] nerd_font = false)
helix.pane_mode.current(); helix.pane_mode.keymap();  // 当前模式 + 其键位表(唯一来源)

// ── keymap.*(面板/弹窗的键位作用域 —— 见"插件键位作用域")──
helix.keymap.scope("filetree", { keys: { q: "close" }, passthrough: [":"] });
helix.keymap.resolve("filetree", key);   // → {action} | {command} | null
helix.keymap.hints("filetree");          // 与 `?`、`:keys` 同一份文本
helix.keymap.active_scope();             // 当前占着键盘的作用域名(或 null)

// ── 进程自省 ──
helix.proc_stats();   // → {cpu_ms, rss_kb},本进程(跨平台)
```

> **旧的扁平名已删除**(`focus` / `zoom` / `get_layout` / `buffers` / `layout_fix` /
> `layout_minimize` …),改用上面的命名空间。
> 只有 `split`、`layout_resize`、`layout_swap` 还是扁平名(暂无命名空间化替代)。

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
  const layout = helix.layout.get();
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

### 插件配置

每个插件声明自己的配置项,用户在 `config.toml` 的 `[plugins.<name>]` 段覆盖;
默认值合并、类型校验、重建都由核心负责 —— 插件不自己解析配置文件。

```js
// 插件侧:加载时声明一次
helix.define_config("filetree", {
  show_hidden: { type: "boolean", default: false, doc: "默认显示隐藏文件" },
  position:    { type: "enum",    default: "left", options: ["left", "right"], doc: "面板贴哪边" },
});
const cfg = helix.get_config("filetree") || {};   // 默认值 + 用户覆盖(只读)
helix.get_config_docs("filetree");                // [{key, type, default, doc}] 可自建设置面板
```

```toml
# 用户侧
[plugins.filetree]
show_hidden = true

[plugins.which-key]
position = "center"
```

- 类型只有 `boolean` / `number` / `string` / `enum`(`options`);不支持嵌套对象/数组。
- 类型写错:该键报错并回退默认值,其余键照常生效。
  **拼错的键也会报**(`未在 define_config 中声明`),不会静默忽略。
- 合并结果在启动、`:config-reload`、以及任何插件加载/重载后(`helix.load` / `:plugin-load` /
  `:plugin-reload`)都会重建 —— 已加载插件的 `get_config` 不会是 `null`。
- **现取**(写在 render / 命令回调里)⇒ `:config-reload` 立即生效;模块顶层缓存一份 ⇒
  改完要 `:plugin-reload` 才看到新值。
- 细节:[`docs/api/config.md`](docs/api/config.md);与 JS 键位的优先级:
  [`docs/config-precedence.md`](docs/config-precedence.md)。

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
helix.buffer.list();        // → [{id, path, name, dirty, language}]
helix.buffer.current();     // → id
helix.buffer.focus(id);     // 当前 view 切换到该文档

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

**文件系统**(同步对在主线程;`*_async` 走 worker)

| API | 说明 | 优点 | 局限 |
|---|---|---|---|
| `read_file(path)` / `write_file(path, text)` | 同步读写文件 | 不用接 Promise | 阻塞主线程;UTF-8 lossy |
| `read_dir(path)` | 列目录(同步) | 简单 | 同步阻塞;不递归 |
| `read_file_async/write_file_async` | 异步读写文件 | 不阻塞 | UTF-8 lossy |
| `stat_async(path)` | 文件元数据 | `{is_dir, size, mtime}` | — |
| `glob_async(pattern)` | 通配匹配 | `*`/`**`/`?` | 相对 CWD |
| `read_tree(path, {depth})` | 递归目录树 | 目录先行按名排序;depth 限深 | 一次性返回,大目录有延迟 |

**事件钩子** `on(event, fn)`:`save` `mode-change` `buffer-open` `buffer-close` `doc-change`(带合并范围)`theme-change` `lsp-diagnostics` `cursor-move` `selection-change` + 终端系(`term-open/mode-change/exit/close/resize/title/key`)`component-event`。优点:同事件多处理器按序;处理器抛错不阻断主流程。局限:doc 是只读快照 + 编辑队列。

**键位绑定(全局)** `map(mode, key, command|fn)`:重复绑定即覆盖,且**覆盖会提示一次**(状态栏 + 插件日志),`:keys` 标 `←插件 <名字>`;支持多键序列与修饰键;回调自动注册隐藏命令。局限:JS 绑定不落盘(插件启动时重新注册);`:config-reload` 会重建键表并**重放**这些绑定。

**键位作用域(面板/弹窗)** `keymap.scope/resolve/hints/unregister/active_scope` —— 见[插件键位作用域](#插件键位作用域)。局限:只支持单键。

**弹窗/面板/组件树**:`open_popup`(覆盖层弹窗,onKey 可 return close/handled/ignore;`layer` 同层替换/异层并存,`position:"center"`+`width/height:"NN%"` 居中浮层)`open_panel`(侧边面板,布局树叶子)`close_panel` `move_panel` `el`(组件树:row/col/scroll/button/input)。优点:渲染与布局引擎分离,脏格 diff;input 组件引擎权威(onChange 自动回调)。局限:无 layer 时 popup 重复打开替换前一个;浮层百分比仅 center 模式(须 width/height 成对)。

**Picker 选择器** `helix.picker.define/run`:定义数据源,调起**原生 Picker**(nucleo 模糊匹配/滚动/预览/键位全核心)。优点:性能原生;插件可定义任意源(files/grep/buffers/symbols);行格式数组或 `{cells, payload}` 分离。局限:候选一次性返回(非流式);行渲染不支持每行组件。

**终端**:`open_terminal` `term_write/feed/kill/list/close/resize/clear/save` `set_terminal_mode` `term_state`(跨会话持久化)+ 终端钩子(`term-key` 返回 normal/pass/consume/minimize/close)。优点:原生 pty 面板,四种显示模式,滚动缓冲。局限:键位直通需手动管理模式。

**窗格与布局**:`pane.list/info/focus/focus_dir/move/resize/close/zoom/unzoom/minimize/equalize/fix/float/embed/raise/pin` · `pane.open`(由 `pane-open` 插件提供)· `stack.create/activate/remove/list` · `layout.get/restore/save/load/list/delete` · `split` `buffer_open`(仅存的两个扁平名)。优点:zellij 式窗口管理,任意叶子焦点,`fix` 免疫交换/关闭,命名布局会话。局限:堆叠恒为一对;`split` 尚无命名空间化的替代。

**Buffer/诊断/监听**:`buffer.list/current/focus` · `diagnostics` · `watch/unwatch`(notify 支撑,500ms 防抖)。优点:文档快照只读安全。局限:id 仅会话内有效。

**界面定制**:`set_statusline` `set_buffer_icon` `set_completion_render`(候选行 JS 渲染,回退原生两列)`set_completion_icon`(kind → 图标字符)`set_diagnostic_icons` `set_component_render`(组件外观 JS 绘制)`get_component_state` `set_keymap_hint`(which-key)。优点:渲染层全可定制;未注册/抛错回退默认。局限:completion 行钩子每帧每行调用——不要跑重逻辑。

**进程自省与日志**:`proc_stats()` → **本进程**的 `{cpu_ms, rss_kb}`(Linux `/proc`、macOS `getrusage`+`task_info`、Windows `GetProcessTimes`+`GetProcessMemoryInfo`)· `log(level, msg)` → 插件日志(环形缓冲 + 应用日志,`:js-log` 查看)· `command_args()` → `:cmd a b` 的参数 · `viewport()`。优点:零能力的自我观测,便宜。局限:`proc_stats` 不自带节流,由调用方决定频率。

**服务器管理(LSP/DAP 工具链)**:`server.list/search/install/update/remove/status/task/rows`。局限:单向请求 —— 结果走状态栏,不回传返回值。

**绘制原语**:`draw.rect/text/hline/vline/fill/braille/push_clip/push_scroll/pop/done` + `text.width/pad/truncate`。优点:弹层可以整帧自绘,不必搭 `el` 树;宽度助手保证 CJK 对齐。局限:布局数学自己算(无自动换行)。

**图标**:`icons.file/dir/mode/diagnostic/git/completion/sys/enabled` —— 表在核心,插件**零依赖**;`enabled(false)`(或 `[plugins.icons] nerd_font = false`)一处关掉 nerd 字形,调用方自动退回文字。状态栏的 `cpu`/`mem` 用的就是 `icons.sys`。局限:字形取决于字体,`sys` 因此只取 Font Awesome 段。

**模糊匹配**:`fuzzy.session/run/drop` —— 进程内 nucleo;候选集稳定、只有查询在变时保留会话(比每次重过滤快约 25 倍)。局限:生命周期自己管。

**主题**:`set_theme`(实时覆盖,scope 级)`reset_theme` `get_style` `theme_info` `set_theme_name`(异步切换)。优点:即时生效,继承主题正确;语法 scope 也覆盖。局限:颜色不支持引用另一 scope。

**插件管理**:`helix.plugin(name, {deps})` 声明加载依赖;`helix.plugin.install(path|git-url)`(manifest 追踪 + 依赖解析 + 装后加载)`update`(git pull,失败保留旧版)`remove`。优点:来源追踪、卸载、更新、依赖递归、循环检测;命令面 `:plugin install/update/pin/unpin/remove/status`。局限:JS API 结果走状态栏(fire-and-forget);pin 仅 git 源。

**LSP**:`helix.lsp.hover/completion/document_symbols/workspace_symbols/format/rename/code_actions/execute_code_action`(见 [api/lsp.md](docs/api/lsp.md))。优点:查询/编辑全链路,失败 resolve null 不悬挂。局限:block_on 冻结主线程(code_action 执行)。

## 📦 内置插件与两层模型

**布局约定**(完整规则见 [`docs/plugin-layout.md`](docs/plugin-layout.md))——一个插件 = **一个目录** + 固定入口名:

```
<插件根>/
├── init.js          # 可选:该根入口(用户写的覆盖内置)
├── <name>/plugin.js # ← 一个插件。目录名 = 插件名 = helix.plugin("<name>")
├── <name>/tests/    # 该插件的 node 测试(不参与加载)
└── examples/        # 示例/模板 —— 不参与自动加载
```

`init.js` 按**名字**点名(不写文件路径):`helix.load("filetree")` → `<name>/plugin.js`;
`deps` 也用插件名:`helix.plugin("filetree", { deps: ["icons"] })`。

**两层、同一套规则**(Neovim `runtimepath` 语义:自带在前、用户殿后 ⇒ **用户覆盖内置**):

| 层 | 位置 | 作用 |
|---|---|---|
| **内置层** | `<runtime>/plugins/`(如 `~/.config/helix/runtime/plugins`) | 随软件分发 |
| **用户层** | `~/.config/helix/plugins/` | 你的;**同名目录整体覆盖**内置那份 |

```bash
sh contrib/install-plugins.sh          # 把仓库 plugins/ 装进内置层
DRY_RUN=1 sh contrib/install-plugins.sh
```

| 插件 | 功能 |
|---|---|
| `filetree/` | 侧边文件树:导航 + **`/` 按名搜索**(`Tab` 切全库)· **`G` 内容搜索** · `>` 过滤表达式(glob/git/诊断)· `?` 键位 · 最多两个面板 |
| `dapui/` | DAP 前端:变量/监视树(右)+ REPL/Output/断点三页(底);键位以作用域声明 |
| `picker-ui/` | JS 自绘 picker(`:pick*`):files/grep/buffers/symbols 任意源 |
| `dashboard/` | LazyVim 风格启动屏(`:dashboard`) |
| `terminal/` | 终端命令(:term/:vterm/:hterm)、面板管理 |
| `statusline/` | 状态栏三段分栏:`cpu`/`mem`(本进程,跨平台)、诊断、git、LSP 进度;分区段名可在 `[plugins.statusline]` 配 |
| `which-key/` | 键位提示(中文/英文):平级模式表 + **当前插件作用域**(`:which-key`,或直接追加到任何前缀键提示) |
| `tabbar/` | 布局标签条示范(顶部槽位,点击聚焦) |
| `arsenal/` | 浮层市场窗(`:arsenal`)—— 取代已废弃的 server-manager |
| `icons/` | 只有 `[plugins.icons]` 配置;图标表本身在核心(`helix.icons.*`) |
| `tutor/` | `:tutor` 教程(**第一个从核心搬进插件的功能**;内容在 `runtime/tutor`,插件打开它且**不绑定路径**) |
| `layout/` | `:layout save\|load\|list\|delete <name>` 命名布局的会话存取(**已从核心搬进插件**;数据 API `helix.layout.*` 在 `helix-js`) |
| `plugin/` | `:plugin list\|install\|remove\|update\|pin\|unpin\|reload\|status` 插件管理器(**已从核心搬出**);并对外提供 `helix.plugin.*` API |
| `pane-open/` | `helix.pane.open({place, content})` 统一 pane 创建入口(对既有 `helix.split` 的形状收敛) |
| `examples/` | `picker.js`(定义 files/grep/buffers/symbols 四个 picker 源)、`lsp-hover.js` |

说明:某个插件加载失败**不会再拖垮整个入口** —— `init.js` 用包装逐条加载,一个坏了报错后其余照常。

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

- **全部文档**:[`docs/README.md`](docs/README.md)(索引:使用手册 / 历史归档)
- **插件布局与命名**:[`docs/plugin-layout.md`](docs/plugin-layout.md)(目录 · 入口 · 依赖 · 两层覆盖 · 分发)
- **插件 API**:[`docs/plugin-api.md`](docs/plugin-api.md)(总览与索引)· [`docs/api/`](docs/api/)(分域详细:说明/示例/优缺点)
- **类型定义**:[`plugins/helix.d.ts`](plugins/helix.d.ts)(编辑器补全用)
- 本 fork 设计文档:`docs/superpowers/specs/2026-08-15-js-ui-rendering-design.md`(JS 视图层分层)
- 交接记录:[`docs/superpowers/handoff/2026-08-14.md`](docs/superpowers/handoff/2026-08-14.md)(窗口模式/终端/插件演进)
- 原版 Helix 文档:[官网](https://helix-editor.com) · [文档](https://docs.helix-editor.com/) · [键位表](https://docs.helix-editor.com/keymap.html)

## 🙏 致谢

上游 [Helix 编辑器](https://github.com/helix-editor/helix)(Kakoune/Neovim 灵感,Rust 编写)——本项目所有基础能力来自它,本分支仅在其上增加 JS 插件系统与窗口模式等特性。
