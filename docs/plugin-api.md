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

### 与 `config.toml` 的分工(先看这条)

同一件事可能两处都能配(键位、主题),规则是**后发生者生效**:JS 运行期 > TOML 启动期;
但 `editor.*`(无 JS 写入口)、`[plugins.<name>]`(唯一数据通道)、`[theme]` 初值(首帧要用)
是"只有一条路"。表格、`:config-reload` 与 `:plugin-reload` 各自管什么、以及"改键位要重启"
这类坑,见 [`config-precedence.md`](config-precedence.md)。

### 进程内模糊匹配

`helix.fuzzy.session(items)` / `run(id, query, opts?)` / `drop(id)` —— nucleo，与核心 picker 同一套。
**会话式**：条目只搬一次。5000 行从 7.1ms/按键降到 2.0ms（2026-10-21 横评）；
一次性 `filter(query, items)` 那种形状反而更慢（11.0ms，搬运成本 > 打分）。

→ 详细见 [`api/fuzzy.md`](api/fuzzy.md)

### 加载 / 命令 / 消息 / 键位

`helix.load(name)`(缓存加载)、`helix.export(obj)`(导出)、`helix.lazy(name, ...commands)`(懒加载桩)、`helix.run_command(name, ctx?)`、`helix.register_command(name, fn, doc?)`(一个命令 = 一次撤销)、`helix.echo(text)`、`helix.map(mode, key, command|fn)`。

- 优点:init.js 单一入口;懒加载减启动开销;命令原子应用;键位支持多键序列/回调。
- 局限:跨插件状态只能走 `helix.*` API;boa 嵌套 eval 污染(依赖必须用 `helix.plugin` deps);键位重启失效。

→ 详细见 [`api/loading-commands.md`](api/loading-commands.md)

### 插件配置

`helix.define_config(name, {key: {type, default, doc}})`(脚本加载时声明)、`helix.get_config(name)`(默认值 + 用户 `[plugins.<name>]` 覆盖,只读)、`helix.get_config_docs(name)`(键/类型/默认值/说明,自建设置面板用)。类型只有 `boolean`/`number`/`string`/`enum`;写错回退默认值并报错,**拼错的键也报错**(不静默)。

- 优点:声明式 + 类型校验;插件 load / reload / 懒加载后自动重建(装完插件不会读到 null);用户覆盖只走 `config.toml`,不用改插件。
- 局限:只支持标量(无嵌套对象/数组);只读、无写入 API(改了走 `:config-reload`);`[plugins]` 顶层键暂无读取入口。

→ 详细见 [`api/config.md`](api/config.md)

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

`helix.on(event, fn)`:文档系(`save` 前/mode-change/buffer-open/close/doc-change 带合并范围/theme-change)、**工作目录(`cwd-change`)**、诊断/光标(`lsp-diagnostics`/`cursor-move`/`selection-change`)、终端系(`term-open/mode-change/exit/close/resize/title/key`)、`component-event`。

- 布局系:`layout-change` —— 回调形状**恒定** `(kind, doc)`,`kind` 缺省时为 `null`;详见 [`api/events.md`](api/events.md)。

- 优点:事件丰富;多处理器按序;抛错不阻断;doc-change 带合并范围。
- 局限:只带当前文档;doc 只读快照;doc-change 250ms 防抖。

→ 详细见 [`api/events.md`](api/events.md)

### 弹窗 / 组件树 / 面板 / 界面定制

`helix.open_popup`(模态层,onKey 三态)、`helix.el` 组件树(text/row/col/scroll/button/input,焦点系统 + 引擎权威 input)、`helix.open_panel`(布局叶子)/`close_panel`/`move_panel`;`helix.set_statusline`、`set_buffer_icon`、`set_completion_icon`(kind 列图标)、`set_completion_render`(候选行 JS 渲染,回退原生两列)、`set_diagnostic_icons`、`set_component_render`(组件外观)、`get_component_state`、`set_keymap_hint`(which-key)。

- 优点:声明式树 + 焦点;input 引擎权威;渲染脏格 diff;补全/组件外观全可定制且回退安全。
- 局限:scroll O(内容);无树内点击命中;面板无节点焦点;completion 行钩子每帧每行(勿跑重逻辑)。

→ 详细见 [`api/ui.md`](api/ui.md)

### Picker 与主题

**图标(`helix.icons.*`)**:`file(path)` / `dir(expanded)` / `mode(mode)` / `diagnostic(sev)` /
`git(status)` / `completion(kind)` / `enabled(on?)`。图标表在**核心**(`helix-js/src/icons.rs`,7 组 145 条),
所以**所有插件零依赖可用** —— 不需要再声明 `deps: ["lib/icons.js"]`(那 4 处已删)。
`enabled(false)` 一处降级:文件类图标变空、目录回退 `▸/▾`,或在 `config.toml` 里写
**`[plugins.icons] nerd_font = false`**(方案 C 的段名;**`[icons]` 是旧写法,写了会让
config 加载报 `unknown field`**)。**已知时序**:该配置在 `plugins/icons/plugin.js` 加载时
应用一次,`:config-reload` 不重推(再 `:plugin-reload` 即可);终态是把这段收进 `helix-view`
的 Config、启动时由 Rust 直接设置,届时那个薄壳与 init 里的显式加载都可删。见 [`api/config.md`](api/config.md)。

`helix.picker.define/run`(定义数据源,调起**原生 Picker**:nucleo 模糊匹配/滚动/预览/核心键位(插件可用源的 onKey 认领**核心不认识的**键);行格式数组或 `{cells, payload}` 分离;四个常用源 files/grep/buffers/symbols **本身就是 JS**(`plugins/examples/picker.js` 用 `helix.picker.define` 定义);核心只提供**注册表 + 分发**与**原生 UI**(nucleo 模糊匹配/滚动/预览/键位));`helix.set_theme`(scope 级实时覆盖)/`reset_theme`/`get_style`/`theme_info`/`set_theme_name`/`on("theme-change")`。

- 优点:picker 性能原生 + 插件可定义任意源;主题覆盖即时生效。
- 局限:picker 候选一次性返回;无旋钮透传;预览仅文件;主题颜色不支持引用 scope;`onKey` 只能**加**键,不能**改**核心键。

→ 详细见 [`api/picker-theme.md`](api/picker-theme.md)

### 终端与布局树

`helix.open_terminal`(原生 pty 面板)/`term_write/feed/kill/list/close/resize/clear/save`/`set_terminal_mode`(dock/fullscreen/floating/minimized)/`term_state`(跨会话持久化);布局:`buffer_open`(开新叶)/`pane.close`/`pane.zoom`/`pane.unzoom`/`pane.resize`/`layout.*`/`pane.focus`/`layout.get`/`layout.restore`;窗口管理走 compositor 层的**zellij 式平级模式**(`C-g` Locked / `C-p` Pane / `C-n` Resize / `C-h` Move / `C-y` Scroll;旧 `C-w` 已删),模式与键位表对插件可见:`helix.pane_mode.current()/keymap()` + `pane-mode-change` 事件;`helix.pane.list()` 给出统一 pane 清单(含浮窗)。`open_panel({side:"left"/"right", rail:true})` 注册**侧栏 rail**(贴边全高、不被窗口分割;filetree 即此模式;`layout.get` 叶子带 `rail` 标记)。

- 优点:原生 pty(引擎为 `alacritty_terminal`:真实 reflow、鼠标上报、括号粘贴、CJK/组合字符);四种显示模式;滚动缓冲;zellij 式平级模式 + 多浮窗;layout_fix;布局可还原。
- 局限:OSC 4/10/11 查颜色与 OSC52 **读**剪贴板未接;PTY 仅 Unix。堆叠组**已**进 `layout.get()`(重启可还原)。

→ 详细见 [`api/terminal-layout.md`](api/terminal-layout.md)

### 插件管理

`:plugin install/update/pin/unpin/remove/list/status/reload`(manifest 来源追踪,本地/git 源;**实现全在 JS 侧** `plugins/plugin/plugin.js`);JS API `helix.plugin(name, {deps})`(加载期依赖,核心)+ `helix.plugin.install/update/remove`(**该插件挂上的**兼容层)。

- 优点:来源追踪(manifest:source/kind/installed_at/pinned/commit/files);卸载/更新闭环;写/删走核心的**限域**文件 API(相对路径 + 拒 `..` + 必须在插件根内)。
- 局限:JS API fire-and-forget(结果看状态栏);git 操作同步阻塞;装完不自动加载(需 `:plugin-reload`);**安装期依赖解析(git 仓库 `plugin.json` 递归)已删除**,只剩加载期 `deps`;`:plugin reload` **已实装**(2026-10-19):走 `helix.reload_plugins()` 入队、命令边界执行,与 `:plugin-reload` 同一实现。

→ 详细见 [`api/plugin.md`](api/plugin.md)

### server 管理(非插件特性;JS 可镜像调用)

`:server list/search/panel/install/update/remove/status`(mason 式 LSP/DAP/linter/formatter 管理器):统一注册表 + mirror/sha256 下载安装,自动写 `languages.toml` 标记段(`# >>> helix-managed`)。JS API `helix.server.list/search/install/update/remove/status`(经 `UiRequest::ServerOp`,单向,结果看状态栏)+ `helix.server.rows(cb)`(行回传)/`helix.server.task(items,cb)`(入队后台任务,phase/progress/done 事件泵回)——arsenal 浮层市场窗(见下)即基于 rows/task 的典型用法。

- 优点:注册表/受管目录单一;浮层市场窗(arsenal)提供即搜/分类/标记批量/版本输入/进度;命令行族可脚本化。
- 局限:JS 单向调用结果看状态栏;版本需显式输入(下载源含 `{version}` 且未固定 version 时);运行中语言配置需重启加载。

→ 详细见 [`server-manager.md`](server-manager.md) 与 [`arsenal.md`](arsenal.md)

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
let cwd = helix.cwd();                  // 起始目录(每次现取;跟随变更监听 cwd-change)
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
| **终端视图非完整仿真器** | 引擎已给 reflow/鼠标上报/括号粘贴/CJK/组合字符;**仍缺**:OSC 4/10/11 查颜色、OSC52 读剪贴板、DECKPAM 应用小键盘、Kitty 键盘协议 |
| **Shift-Tab 的表示** | `Shift-Tab` 在 JS 侧**不是**独立键名:键码层无 `BackTab`(`input.rs` 把 termina 的它归一化成 `Tab+SHIFT`)⇒ 插件看到的是 `{name:"Tab", shift:true}`。焦点环已支持反向(`S-tab`),但**判断方向必须看 `shift`**,别找 `"BackTab"` |
| **面板节点焦点需显式开启** | 缺省 `focusable: false` ⇒ 面板 render 收到 `focus = null`;`open_panel({focusable: true})` 后启用节点焦点路由:Tab 移动焦点、Esc 取消、焦点在节点时按键直达(实测 2026-10-10) |
| **`move_panel` 只支持 rail** | 面板住在布局树里(`open_panel` → rail 叶子或普通叶子);`move_panel` 走树后已可用,但**只有 rail 面板**能换边(普通叶分裂换边会牵动布局树结构),非 rail 会明确拒绝 |
| **无树内命中** | 组件树节点不响应点击/悬停;命中检测(哪一行被选中)由 JS 自己维护 |
| ~~堆叠未持久化~~ **（已支持）** | `layout.get()` 已含 `stacks`（`LayoutDump.stacks`），`restore` 按锚重建；堆叠组也有 header 行（`▶类型名` 标记当前显示的那个）。回归：`restore_roundtrip_keeps_stacks`（`helix-term/src/ui/layout.rs`） |
| **picker 键位只可「加」不可「改」** | 源可选的 `onKey` 只能认领**核心不认识的键**(`Esc`/`Enter`/`Tab`/翻页等核心先处理,插件拿不到)=> 无法重绑核心键;纯字符输入也永不交给插件(防吞打字)。详见 [`api/picker-theme.md`](api/picker-theme.md) |
| **Scroll 全量布局** | scroll 是 O(内容) 而非 O(视口),几千行内容每帧全量布局 |
| **事件 ctx 范围** | 事件只带当前文档(其他分屏文档不触发) |
| **`layout-change` 的 `kind` 常为 `null`** | 形状**恒定** `(kind, doc)`(2026-10-13 统一;此前缺种类时只传 `(doc)` ⇒ `(kind) => …` 静默拿到事件对象)。`sync_layout_cache()` 37 处调用点仅 10 处设种类（实测 `grep`） ⇒ 判 `kind === null` 即可 |
| **无多光标编辑** | `helix.set_selection` **支持多选区数组形态**(`[{anchor, head}, …]`;按 range 起点排序,主光标 = 排序后末位,与原生多光标一致)。缺的是**逐光标各自的编辑** —— 编辑队列只有一份快照坐标(实测 2026-10-10) |
| **Unix-only** | `helix.run`/`spawn`/PTY 依赖 `sh` 与 libc openpty(Windows 不可用) |
| **键位不持久** | `helix.map` 绑定重启失效(插件启动时重新注册) |
| **插件间无共享状态** | 每个插件独立 IIFE 作用域;共享需通过 `helix.*` API 或外部文件 |
| **嵌套 load 污染风险(已修复)** | boa 0.21 的嵌套 eval 破坏外层闭包绑定(`typeof` 变 object、函数不可调用,实测 2026-08-26;DefInitVar 越界 panic)。**boa 0.22 升级后验证修复**(2026-08-31 复现测试通过);防御(icons.js 工具函数 `|| {}` 兜底)保留作双保险。旧建议(依赖用 `helix.plugin` deps 声明、回调不放嵌套 load 后)仍是最佳实践 |

### 判定为「不做」（不是待办）

这两条曾经以"缺 API"的形式挂在限制表里，读者无法区分"没人做"与"决定不做"。判定本身就是交付物。

| 项 | 判定 | 理由 |
|---|---|---|
| **安装期依赖**（git 仓库根 `plugin.json` 的 `deps`） | ❌ 不做 | 加载期 `deps` 已覆盖真实需求（拓扑加载 + 按 key 缓存 + 循环依赖报错）。安装期求解需要版本约束求解器，而 git 源实际只有一个可装版本（HEAD）⇒ 代价远大于收益。第三方插件的依赖请在它的 README 里让用户手动装。 |
| **插件间共享状态**（新增一个全局 `shared` 命名空间之类） | ❌ 不做 | 跨插件共享会让"谁能改谁的状态"变成隐式耦合，且无法在插件重载后定义清理时机。现有两条出路——`helix.*` API（核心承担生命周期）与插件根下的文件（用户可见）——都显式、可测。真需要共享就建一个被 `deps` 依赖的中间插件。 |


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
helix.on(event, fn)                             // 21 个事件见下方"命名空间与事件索引"
helix.map(mode, key, commandOrFn)
helix.set_cursor(row, col)
helix.set_selection(ar, ac, hr, hc)
helix.set_input_value(popup_id, node_id, value)   // 强制改 input 值(光标置末尾)
helix.open_popup({ render, onKey?, onClose?, width?, height?, position? }) -> id
helix.popup_exists(id) -> boolean / helix.close_popup(id)   // 自查 + 主动关闭(等价于 onKey 返回 "close")
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
helix.pane.close(id)
helix.pane.zoom(id) / helix.pane.unzoom()
helix.pane.resize(id, ratio)
helix.pane.focus(id)
helix.layout.get()                              // {tree, active, zoomed} | null
helix.layout.restore(layoutObj)                 // 仅写缓存
// ── 文档/构建期常见但此前漏记的(本次对账补上)──
helix.viewport()                                // [宽, 高] 列数/行数(居中/铺满用)
helix.picker_state()                            // picker 状态摘要(query/命中/总数)
helix.command_args()                            // 当前键入命令的参数数组(`:layout save dev` → ["save","dev"])
helix.loaded_plugins()                          // **已加载**插件名数组(与 `:plugin list` 同源)
helix.plugins_dir()                             // 插件目录绝对路径(未推入 → null)
helix.cwd()                                     // **当前**工作目录绝对路径(每次现取;跟随:`helix.on("cwd-change", …)`)
helix.draw.rect(x,y,w,h,{border,title,fill,style}) // 立即模式绘制:边框盒子/填充(最后必须 `return helix.draw.done()`)
helix.draw.fill(x,y,w,h,{style,ch})             // 无边框填充块(缺省铺带样式的空格)
helix.draw.hline(x,y,len,{ch,style})            // 水平线(缺省 ─)
helix.draw.vline(x,y,len,{ch,style})            // 垂直线(缺省 │)
helix.draw.text(x,y,"字"|[{text,style}],{style,maxWidth}) // 按坐标写文字(宽字符占 2 列)
helix.draw.braille([[dx,dy],…],{style})         // **半像素**点阵(1 列 = 2 点、1 行 = 4 点)
helix.draw.push_clip(x,y,w,h) / pop()           // 显式裁剪区(坐标仍绝对;done 兜底闭合)
helix.draw.push_scroll(x,y,w,h,{dx,dy}) / pop() // 滚动容器(内容坐标 - 偏移;矩形即裁剪区)
helix.text.width(s) / truncate(s,w) / pad(s,w,{align}) // 显示宽度口径(与渲染器同表;替代插件里的 dw()/cut())
helix.runtime_path(name)                        // 自带 runtime 内的绝对路径(找不到 → null;内置内容型插件用)
helix.remove_plugin_file(rel)                   // 删**插件根内**的文件/目录(限域;true=删了,false=本来没有)
helix.define_config(name, schema)               // 声明配置 schema({key: {type, default, doc}})
helix.get_config(name)                          // 合并后的配置值(含默认值;未声明 → null)
helix.get_config_docs(name)                     // [{key, type, default, doc}]
helix.watch(path, cb) -> id                     // 文件监听(回调收 [{kind, path}])
helix.unwatch(id)                               // 停止监听
helix.diagnostics()                             // [{line, message, severity, code, source}] | null(只读快照)
helix.quit()                                    // 关当前视图(最后一个则退出);`run_command("quit")` 走不通,用这个
helix.reload_plugins()                          // 热重载全部插件(入队,命令边界执行;同 `:plugin-reload`)
helix.term_feed(view_id, chunk)                 // 把 PTY 输出喂进该终端视图的网格
helix.term_save(view_id, path?)                 // 导出 scrollback+屏幕(省略 path → ~/.cache/helix/term-<id>.log)
helix.term_state(ptyId, key[, value])           // 终端任意状态持久化(读/写,跨会话)
helix.buffer_open(path)                         // 在新的布局叶里打开 buffer(与 open_file 的叶级版本)
helix.layout_resize(id, ratio)                   // [`pane.resize` 的扁平别名,历史兼容]
helix.layout_swap(id1, id2)                      // [`pane.swap`… 的扁平别名;新代码用命名空间形式]
helix.read_dir(path)                            // 同步 -> [{name, is_dir, path}]
helix.read_file_async(path)                     // Promise → 内容(UTF-8 lossy)
helix.write_file_async(path, content)           // Promise
helix.stat_async(path)                          // Promise → {is_dir, size, mtime}
helix.glob_async(pattern)                       // Promise → paths[]
helix.read_tree(path, { depth? })               // Promise → [{name, is_dir, path}](递归)
helix.set_statusline(fn, opts?)                 // fn(ctx) -> string | StatuslinePart[] | null
                                               // opts = { replace?: true, zones?: [左,中,右] }
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
helix.plugin(name, { deps, version?, schema_version?, min_editor?, permissions?, contributes? })
                                                // 声明加载依赖 + **能力清单**(见下)
helix.api_version                               // 边界协议版本(= 事件 payload 里的 api_version)
helix.permissions()                             // 本插件**现在**能做什么(能力清单查询面)
helix.plugin.install(arg) / update(name?) / remove(name)   // 管理(结果走状态栏)
// 实现位置:`plugins/plugin/plugin.js`(管理器本身是**内置插件**)。
// 命令 `:plugin …` 与上面三个 API 共用同一份实现;该 API 的**错误语义**与旧核心一致
// (例如"未安装"时 remove 抛 `failed to remove '<name>': not installed`)。
```

### 命名空间与事件索引(机械对账,勿手改)

> 下面这块与 `helix.d.ts` / Rust 注册表**逐名一致**(生成方式见本节末尾)。

```
helix.pane.*   close, embed, equalize, fix, float, focus, focus_dir, info, list, minimize, move, pin, raise, resize, unzoom, zoom
helix.layout.*   delete, get, list, load, restore, save
helix.stack.*   activate, create, list, remove
helix.buffer.*   current, focus, list
helix.pane_mode.*   current, keymap
helix.icons.*   completion, diagnostic, dir, enabled, file, git, mode
helix.fuzzy.*   drop, run, session
helix.picker.*   define, run
helix.lsp.*   code_actions, completion, document_symbols, execute_code_action, format, goto_definition, hover, rename
helix.server.*   install, list, remove, rows, search, status, task, update
helix.draw.*   braille, done, fill, hline, pop, push_clip, push_scroll, rect, text, vline
helix.text.*   pad, truncate, width

事件(21 个):
  buffer-close, buffer-open, component-event, cursor-move, cwd-change
  doc-change, layout-change, lsp-diagnostics, mode-change
  pane-mode-change, save, selection-change, startup
  term-close, term-exit, term-key, term-mode-change
  term-open, term-resize, term-title, theme-change
```

### 加一个新的 `helix.*` API（给改本仓的人）

唯一声明处是 **`helix-js/src/api.rs` 的 `API` 表**（一行 = 命名空间、名字、arity、实现函数、
能力、实现函数标识符）。注册、`describe()`、能力门控都从这张表来。

1. 表里加一行（照抄邻行的形状）；碰 fs/进程的必须写 `caps`（如
   `&[crate::permissions::CAP_FS_READ]`）——**不写就是没有能力要求**，而派发点按它检查；
2. `plugins/helix.d.ts` 补类型（手写：TS 类型是有价值的手工资产）；
3. `docs/api/xxx.md` 补条目、`docs/plugin-api.md` 的索引块补名字；
4. `cargo xtask api-check`（它会先跑 `api-describe` 把表导成 JSON，再对账 d.ts / 文档 / 能力）。

> 历史：这三份对账脚本曾经**各自正则解析 Rust 源码**（`register/*.rs` 的 `.function(...)` 样板）。
> 那张表存在之后，解析源码就是第二份真相 —— 现在它们只读 `target/api-surface.json`。
> 能力检查也从"每个实现里手写 `check_js(...)`"改成**派发点统一执行**：忘记加检查这个失败模式
> 已经不存在了（忘的是在表里写 `caps`，而那由哨兵段兜）。

**这份文档有门禁**:`cargo xtask api-check`(CI 的 "Check plugin API surface" 步骤)现在
跑**三半** —— `plugins/helix.d.ts ↔ 注册表`、**本文件 ↔ 注册表**
(`plugins/tools/check_doc_surface.mjs`)、以及**能力检查点 ↔ 注册的 JS API**
(`plugins/tools/check_capability_surface.mjs`:上面那张"能力 ↔ API"表的**机械版本**,
漏检查/换错能力都会红)。本文件这一半的三个判据:
① 本文件里出现的 `helix.X` / `helix.ns.X` 都必须在注册表(或**插件补的名字**白名单)里;
② 注册表 154 个成员每个都要有条目(全名或出现在上面的命名空间索引块里);
③ 21 个事件都要出现。漂移 ⇒ 退出码 1 ⇒ CI 红。

所以**改 API 时请顺手跑一次 `cargo xtask api-check`**;新增"插件补的名字"时,把它加进
`check_doc_surface.mjs` 的 `PLUGIN_PROVIDED` 白名单(那是文档有权提到、注册表里没有的
唯一来源)。

### manifest 与能力清单(permissions)—— 插件作者手册

> 一句话:**把你的插件要做的事写进 manifest**,用户就能看见、能拒绝;不写也能跑(存量兼容),
> 但你放弃了"被审计"这个能力。

#### 1. 我该写什么(照抄改这一块)

```js
helix.plugin("my-plugin", {
  deps: [],                            // 加载期依赖(拓扑加载;见 plugin-layout.md §3.4)
  version: "1.0",                      // 元数据
  schema_version: 1,                   // 声明面版本;高于编辑器支持的版本 ⇒ **加载即失败**
  min_editor: "25.7",                  // 最低编辑器版本;高于当前版本 ⇒ **状态栏警告并继续加载**
  permissions: { process: true, fs_read: true, fs_write: false },
  contributes: ["cmd:my-cmd"],         // 贡献点(记录用,`:js-permission list` 可见)
});
```

只写**你真正用到的**能力。三项全 `false` 也合法 —— 含义是"我什么都不要"(于是任何能力
第一次使用时都会被询问)。

#### 2. 能力 ↔ API 对照表(同一能力覆盖同族 API)

| 能力 | 覆盖的 API | 典型用途 |
|---|---|---|
| `process` | `helix.run` / `helix.run_async` / `helix.spawn` / **`helix.open_terminal`** / **`helix.server.*`**(list/search/install/update/remove/status/task/rows) | 调 git / rg / 起 shell 终端;**安装/升级/卸载工具**(下载 + 写盘 + 改写 `languages.toml`,rows 还会探测版本起子进程) |
| `fs_read` | `helix.read_file` / `helix.read_file_async` / `helix.read_dir` / **`helix.stat_async` / `helix.glob_async` / `helix.read_tree` / `helix.watch`** | 读配置、列目录、读项目文件;监听文件变化 |
| `fs_write` | `helix.write_file` / `helix.write_file_async` / `helix.write_plugin_file` / **`helix.remove_plugin_file`** | 写缓存/状态文件、插件自更新 |

`helix.write_plugin_file` 是**限域**写作(只能写自己插件目录下的相对路径),但仍属 `fs_write`。
打开 buffer、读 `doc.text`、`helix.by_path` 读**已打开**的文档都**不需要**声明(不是文件系统访问)。

> **`helix.server.*` 为什么也归 `process`**(2026-10 补):这些 API 会**下载二进制并安装到受管目录**、
> 改写 `languages.toml`;`rows` 会为探测版本起子进程(`<bin> --version`)。之前它们**完全在能力模型之外**
> —— 任意插件零声明就能让 helix 装东西。现在与 `helix.run` 同一能力。

#### 3. 三种情形,行为不同

| 情形 | 第一次用未声明的能力时 | 说明 |
|---|---|---|
| **没写 `permissions`**(= 空能力集) | 该次调用**报错** + 记入待授权 | **fail-closed**(2026-10-21):不写 manifest 不再等于放行 |
| **写了 `permissions`** | 同上 | 用户答一次即记住(见 §4) |
| **内置插件**(随软件分发的 runtime) | 永不询问 | 用户目录里同名覆盖的那份按第三方对待;`:plugin-load /abs/x.js` 同样按第三方(没有"提供根") |
| **没有属主的调用** | 直接拒绝 | 编辑器内部/不属于任何脚本的代码;受保护 API 只在插件回调、命令、加载期脚本里可用 |

#### 4. 被询问之后,用户怎么处理

```
:js-permission list                     // 声明 / 待授权 / 已记住的决定(已允许·已拒绝)
:js-permission allow <plugin> <cap>     // 记住"允许"(cap ∈ process / fs_read / fs_write)
:js-permission deny  <plugin> <cap>     // 记住"拒绝"
```

- 回答一次即记住(等价于 zellij 的 `request_permission`);**重新回答会覆盖**旧决定,
  且会把该条从"待授权"里移出。
- `deny` 之后**不会**再问 —— 调用一直失败,错误消息写明"已被拒绝"与改法。
- 决定是按 **(插件名, 能力)** 记的,"哪个插件在调"由**注册期绑定**决定:
  `helix.register_command` / `helix.on` / 弹窗与面板的 render·onPress / picker 的
  items·preview·action / Promise 续体 / `helix.watch` …… 都在**注册那一刻**记下属主。
  加载期顶层代码归属**脚本名**(`<name>/plugin.js` ⇒ `name`,绝对路径 ⇒ 文件名),
  脚本里 `helix.plugin("x")` 一旦执行就升级为 `x`。

#### 5. 写"能降级"的插件(推荐姿势)

不要假设能力一定可用 —— 先查再决定,失败会**只发生一次**但你至少能给用户一个体面的回退:

```js
// 老编辑器没有这个 API ⇒ 按"可用"处理(try/catch 是兼容层,不是兜底逻辑)
function hasProcess() {
  try { return !!(helix.permissions && helix.permissions().process); } catch (e) { return true; }
}

async function refreshGit(path) {
  if (!hasProcess()) { gitBranch = ""; return; }        // 降级:不显示分支,而不是弹一次失败
  gitBranch = (await helix.run_async("git -C " + shq(dirname(path)) + " branch --show-current")).trim();
}
```

`helix.permissions()` 返回 `{ process, fs_read, fs_write }`,语义 = "**不问就放行吗**":
内置插件三项都是 `true`;其余看 `permissions` 声明或**已记住的决定**
(未声明 / 未决定 ⇒ `false`,因为那次调用会被问一次;无属主 ⇒ 全 `false`)。

内置插件里已有两处按这个形状写:`statusline`(git 分支前查 `process`)、`plugin-manager`
(`install`/`update` 前查 `process`,没有就明确报错并跳过)。

#### 6. 两个版本字段:别混用

| 字段 | 不满足时的行为 | 什么时候用 |
|---|---|---|
| `schema_version` | **拒载**(`helix.plugin` 抛错) | 声明面/字段语义变了,旧编辑器**可能误读你的 manifest** |
| `min_editor` | **警告并继续加载**(状态栏一条提示) | 你只是用到某些新 API,代码本身仍可能正常工作 |

`min_editor` 与真实版本的比较是**宽松**的:`"25.7" == "25.7.0"`,`"25.07.1 (c4831272)"` 取
`[25,7,1]`;写成非版本号(`"latest"`)会被当成错误配置**警告**出来。
编辑器版本未知(库/单测场景)时**跳过**检查。

#### 7. 排错清单

| 现象 | 看什么 |
|---|---|
| 某次调用突然报"未声明能力 'x'" | `:js-permission list`:你的 manifest 声明了什么、`x` 是否在待授权里 |
| 答过 allow 却还报错 | 是不是**另一个**插件名在调(属主按注册期绑定);`list` 里核对"已允许"的插件名 |
| 内置插件不起作用了 | 你是不是把它复制到 `~/.config/helix/plugins/` 后**没带** `permissions` 声明 —— 用户目录那份按第三方对待 |
| 想确认权限模型没说谎 | `helix.permissions()`:JS 侧看到的"能做什么"与 `list` 的"已允许/已拒绝"应对得上 |

检查点实现见 `helix-js/src/permissions.rs` 的模块文档(能力判定表、问一次的逻辑、归属规则)。

### 数据形状

```
命令 ctx:{ path, text, cursor: {row,col}, selection: {anchor, head}, doc }
doc:{ path, text, cursor, insert(sr,sc,text), replace(sr,sc,er,ec,text), delete(sr,sc,er,ec) }
按键对象:{ name, shift, ctrl, alt }     // name: 字符 | Enter|Esc|Tab|Backspace|Delete|Insert|方向键|Home|End|PageUp|PageDown|F1..F12
onKey 返回值:"close" | "handled" | "ignore"
render 返回值:数组(string|{text,style})或 el 组件树;签名 render(focus, {width, height})
组件树节点:text|row|col|box|scroll|button|input(见 api/ui.md)
picker 行:数组 [c1, c2, ...] 或 { cells: [...], payload: [...] };preview/action 收到 payload 数组(见 api/picker-theme.md)
终端模式:dock | fullscreen | floating | minimized
面板方向:right | left | bottom
split 方向:right | left | top | bottom
事件:save | mode-change | cwd-change | buffer-open | buffer-close | doc-change | theme-change | lsp-diagnostics | cursor-move | selection-change | term-* | component-event
事件 payload(每个事件**位置参数之后**追加一个具名对象):
  { api_version, type, ...具名字段 }   // 形状见 helix.d.ts 的 EventPayload;doc 对象上也带 api_version/type
layout-change 的投递时机有两种模式(`[editor] js-layout-change-batch`,默认 `false`):
  · **逐点**(默认):每个变更点当场发一条,处理器看到的是**该变更点当时**的布局
    (快照同步更新,`layout.get()`/`pane.list()` 读到的就是那一刻);
  · **批量**:同一投递窗口(一次回调/一帧)内的多次变更**合并成一条**,`kind` 取窗口内
    最后一次标注,处理器看到的是**窗口结束时**的布局 —— 少一半 JS 往返(实测一次 split
    逐点 4 条 → 批量 2 条),代价是拿不到中间态。
键位模式:normal | insert | select
```
