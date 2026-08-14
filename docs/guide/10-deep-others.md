# 10 · 其余 crate 速览

> 除 core/view/term/lsp 外，workspace 还有 8 个小 crate。每个 crate 按
> 「职责 → 关键类型 → 使用方式」介绍，篇幅从短到长。

## 1. helix-tui：终端渲染原语

> **职责**：把"要画什么"翻译成"终端能懂的东西"。从 tui-rs fork，受 Cursive 启发。
> 约 16 个文件、6 千行。模块见 `helix-tui/src/lib.rs`。

```
helix-tui
 ├─ buffer.rs    Buffer / Cell：内存屏幕（渲染核心，见 05 教程第 4 节）
 ├─ terminal.rs  Terminal：双缓冲、diff、draw、光标管理（见 05 教程第 5 节）
 ├─ backend/     Backend trait：TerminaBackend / CrosstermBackend / TestBackend
 ├─ widgets/     Widget trait 与预置组件（Paragraph、List、Table 等）
 ├─ layout.rs    布局计算（Rect、Layout 约束系统）
 ├─ text.rs      Span / Spans / Line 文本模型
 └─ symbols.rs   制表符/图形符号（`symbols::line::VERTICAL` 等）
```

**关键概念**：组件（helix-term 的 `Component`）不直接碰终端，只往 `Buffer` 的
`Vec<Cell>` 里写字；`Terminal::draw` 负责 diff 和输出。这是"渲染与后端解耦"的根基。

> 使用方：`helix-term/src/application.rs`（`Terminal`、`TerminalBackend`）、
> `helix-term/src/ui/*`（widgets）。

## 2. helix-event：事件系统与消息队列

> **职责**：让编辑器各组件在不强耦合的前提下通信——同步钩子、异步钩子、重绘请求、
> 状态消息。约 9 个文件、1.1 千行。自述见 `helix-event/src/lib.rs` 顶部注释。

```
helix-event
 ├─ registry.rs  Event trait + 事件注册/分发中心
 ├─ hook.rs      同步钩子：Fn(&mut impl Event) -> Result<()>，dispatch 时立即执行
 ├─ debounce.rs  AsyncHook：带防抖的异步钩子（channel 接收事件）
 ├─ cancel.rs    TaskController / cancelable_future：取消进行中的异步任务
 ├─ redraw.rs    request_redraw / lock_frame / start_frame：帧与重绘控制
 ├─ runtime.rs   跨线程运行时辅助（runtime_local 宏）
 └─ status.rs    StatusMessage：状态消息通道
```

**关键用法**：

```rust
helix_event::dispatch(DocumentDidChange { ... }); // 广播事件（同步钩子立即执行）
helix_event::request_redraw();                    // 请求主循环重绘
```

**两个阵营**：
- *同步钩子*：能立即改编辑器状态（如关补全弹窗），但无自有状态；
- *异步钩子（AsyncHook）*：基于 channel，可持有状态、做后台计算/防抖；
  数据通过 `send_blocking` 送回。

> 使用方：几乎全部——`helix-view` 的 `DocumentDidChange`/`SelectionDidChange`、
> `helix-term` 的 `PostCommand`/`OnModeSwitch`（`helix-term/src/events.rs`）。

## 3. helix-vcs：版本控制集成

> **职责**：为编辑器提供版本控制信息（git 等）——diff 高亮、文件变更状态。
> 约 8 个文件、1.4 千行。

```
helix-vcs
 ├─ diff.rs       DiffHandle / Hunk：后台计算"文档 vs 磁盘"的 diff（线程池）
 ├─ git/         git 后端（feature = "git"）：git diff 输出解析
 ├─ status.rs    FileChange：文件状态（新增/修改/删除）
 └─ lib.rs       导出：DiffHandle、Hunk、FileChange
```

**关键类型**：

```rust
pub struct DiffHandle { /* 文档快照 + 后台 diff 任务 */ }
pub struct Hunk { pub start: usize, pub end: usize, pub kind: DiffKind }
pub enum FileChange { Added, Modified, Removed }
```

- `DiffHandle::update_document(text, force)`：文档变更后异步重算 diff；
- 渲染时（`ui/editor.rs`）用 `Hunk` 在行号栏画 `+`/`-`/`~` 标记。

> 使用方：`helix-view/src/document.rs`（`diff_handle` 字段）、`helix-term/src/ui/editor.rs`。

## 4. helix-dap + helix-dap-types：调试器客户端

> **职责**：实现 Debug Adapter Protocol（DAP）客户端，支持断点、单步、变量查看。
> `helix-dap` 4 个文件 1.2 千行；`helix-dap-types` 1 个文件 1.1 千行（协议类型）。

```
helix-dap
 ├─ client.rs      Client：调试器进程管理、请求/响应/事件分发
 ├─ registry.rs    Registry：调试器注册表
 ├─ transport.rs   Transport：stdin/stdout 通信（与 LSP 同构）
 └─ lib.rs         导出 Client / Payload / Response / Transport
```

**关键概念**：与 LSP 客户端几乎同构——spawn 子进程、JSON-RPC 式请求/事件、
`DebuggerEvent((DebugAdapterId, Payload))` 进 `EditorEvent` 由主循环处理。
调试 UI（断点行、当前栈帧高亮）见 `helix-term/src/commands/dap.rs` 与
`helix-view/src/handlers/dap.rs`。

## 5. helix-loader：资源加载

> **职责**：加载编辑器运行时需要的所有外部资源。约 5 个文件、2 千行。

```
helix-loader
 ├─ lib.rs           目录解析：config 目录、runtime 目录、日志文件路径
 │                    （VERSION_AND_GIT_HASH、default_log_file、config_dir、runtime_dirs）
 ├─ config.rs        配置文件初始化（initialize_config_file）
 ├─ grammar.rs       tree-sitter grammar 的 fetch/build（-g fetch|build）
 └─ workspace_trust.rs 工作区信任（WorkspaceTrust）
```

**关键 API**：

```rust
helix_loader::config_dir()          // 用户配置目录（~/.config/helix）
helix_loader::runtime_dirs()        // runtime 目录（内置 + 用户覆盖）
helix_loader::runtime_file(path)    // 取 runtime 下的文件（tutor 等）
helix_loader::grammar::fetch_grammars / build_grammars
helix_loader::workspace_trust::WorkspaceTrust::new(...)
```

## 6. helix-parsec：解析器组合子

> **职责**：一个极简解析器组合子库（574 行单文件），用于把文本解析成结构化数据。
> 灵感来自 [parsec](https://hackage.haskell.org/package/parsec)。

```rust
type ParseResult<'a, Output> = Result<(&'a str, Output), &'a str>;
```

组合子风格：`char`、`string`、`many`、`choice`、`between` 等，用闭包链组合成解析器。
使用方很少（keymap 相关解析），是纯函数工具库。

## 7. helix-stdx：标准库扩展

> **职责**：各 crate 共用的实用工具（类似 rust-analyzer 的 stdx）。
> 约 7 个文件、2.5 千行。

```
helix-stdx
 ├─ env.rs      环境：set_current_working_dir、env var 读取等
 ├─ path.rs     路径工具：normalize、get_relative_path、expand_tilde 等
 ├─ uri.rs      Url 类型（helix_stdx::Url，贯穿 LSP/文档路径）
 ├─ rope.rs     Rope 便捷扩展（RopeSliceExt）
 ├─ range.rs    通用 Range 类型
 └─ faccess.rs 文件访问检查（可读/可写）
```

**关键点**：`helix_stdx::Url` 是 LSP 与文档路径之间的统一 URL 类型
（`helix-lsp-types` 也 re-export 它）；`path::normalize` 保证跨平台路径一致性。

## 8. helix-js：JS 插件运行时

> **职责**：较新特性——用 Boa JS 引擎运行用户插件脚本（config 目录 `plugins/*.js`）。
> 单文件 475 行。

```
helix-js/src/lib.rs
 ├─ load_script(src)           加载并执行插件脚本
 ├─ run_command(name, ctx)     执行插件注册的命令
 ├─ command_names()            列出插件命令（用于补全）
 ├─ take_messages()            取插件产生的状态消息
 ├─ take_ui_requests()         取插件请求的 UI 操作（弹出层等）
 ├─ render_popup / popup_key / close_popup  插件弹出层渲染与按键分发
 ├─ bufferline_icon(path)      插件提供的 bufferline 图标
 └─ init()                     初始化 JS 运行时（注册原生函数）
```

**架构**：插件不能直接碰编辑器——通过**消息队列**（`take_messages` /
`take_ui_requests`）把意图送回主循环。加载时机：
- `Application::new` 时加载 config 目录下所有 `*.js`（`helix-term/src/application.rs`）；
- 运行时 `:plugin-load <path>`（`commands/typed.rs` 的 `plugin_load`）。

`helix-term/src/ui/plugin_popup.rs` 是插件弹出层的 UI 组件，
`helix-term/src/events.rs` 定义插件可订阅的事件（`PostCommand`、`OnModeSwitch` 等）。

## 9. 一页速查

| crate | 一句话 | 关键文件 |
|-------|--------|---------|
| helix-tui | 终端渲染原语 | `buffer.rs`、`terminal.rs` |
| helix-event | 事件总线 + 重绘/状态通道 | `lib.rs`、`redraw.rs` |
| helix-vcs | git diff/hunk | `diff.rs` |
| helix-dap | 调试器客户端 | `client.rs`、`registry.rs` |
| helix-loader | 资源/配置/grammar 加载 | `lib.rs`、`grammar.rs` |
| helix-parsec | 解析器组合子 | `lib.rs`（单文件） |
| helix-stdx | 通用工具（Url、路径） | `uri.rs`、`path.rs` |
| helix-js | Boa JS 插件运行时 | `lib.rs`（单文件） |
