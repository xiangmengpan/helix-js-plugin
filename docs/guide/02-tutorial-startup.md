# 02 · 启动流程：从 `main()` 到编辑器就绪

> 本文跟随 `hx`（Helix 二进制）从进程启动到进入事件循环的完整路径。
> 主线代码：`helix-term/src/main.rs` → `helix-term/src/application.rs`。

## 1. 入口：`main()`

```rust
// helix-term/src/main.rs
fn main() -> Result<()> {
    let exit_code = main_impl()?;
    std::process::exit(exit_code);
}

#[tokio::main]
async fn main_impl() -> Result<i32> { ... }
```

注意两点：

- `#[tokio::main]`：整个编辑器跑在 tokio 运行时上（异步事件循环的根基）；
- 退出码通过 `std::process::exit` 显式返回，`Editor.exit_code` 是最终值来源（见
  `application.rs::Application::run` 末尾 `Ok(self.editor.exit_code)`）。

## 2. 参数解析与早期分支

`main_impl` 的前半段（`main.rs`）：

1. `Args::parse_args()` — 命令行参数解析（`helix-term/src/args.rs`），包括 `-c` 配置文件、
   `-v` 日志级别、`-w` 工作目录、`--vsplit/--hsplit`、`+N` 光标定位等；
2. `helix_loader::initialize_config_file` / `initialize_log_file` — 初始化配置文件与日志文件路径
   （`helix-loader` crate）；
3. 一系列**提前退出分支**（不进入编辑器 UI）：
   - `--help` / `-V`：打印帮助/版本后 `exit(0)`；
   - `--health`：运行 `helix_term::health::print_health` 健康检查后退出；
   - `-g fetch/build`：`helix_loader::grammar::fetch_grammars` / `build_grammars` 管理
     tree-sitter grammar 后退出。

## 3. 工作目录与配置加载

```rust
// 设置工作目录（影响配置加载路径）
helix_stdx::env::set_current_working_dir(path)?;
// 加载用户配置，失败时回退默认配置
let config = Config::load_default() ...;
```

配置加载在 `helix-term/src/config.rs`：`Config` 包含 `editor`、`keys`、`theme` 等部分，
**加载失败不回退到崩溃**，而是提示按回车用默认配置继续（`main.rs` 中 `BadConfig` 分支）。

随后：

```rust
let workspace_trust = WorkspaceTrust::new(...);
let lang_loader = helix_core::config::user_lang_loader(&workspace_trust) ...;
```

- `WorkspaceTrust`（`helix-loader/src/workspace_trust.rs`）：工作区信任检查（打开陌生项目时的
  权限提示逻辑）；
- `lang_loader`：加载 `languages.toml` 语言配置（每个语言的语法、注释符号、LSP 命令等）。

## 4. `Application::new`：组装所有部件

`helix-term/src/application.rs::Application::new` 按顺序创建：

```
创建主题加载器 theme::Loader（从配置目录 + runtime 目录）
创建终端后端 TerminalBackend（TerminaBackend/CrosstermBackend/TestBackend）
创建 tui::Terminal（管理屏幕缓冲区）
创建 Compositor（UI 组件栈）
创建 handlers（LSP/DAP 等异步响应的处理器注册表，handlers::setup）
创建 Editor（全局编辑器状态，Editor::new）
加载主题（load_configured_theme）
加载 config 目录下的 JS 插件（helix_js::load_script，实验特性）
创建 Keymaps（ArcSwap 包装的按键映射，可从配置热更新）
创建 ui::EditorView（主编辑器组件）并 push 进 Compositor
创建 Jobs（异步任务队列）
打开启动文件（tutor / 命令行指定的文件 / 目录 picker）
```

### 关键结构：`Application`

```rust
// helix-term/src/application.rs
pub struct Application {
    pub compositor: Compositor,   // UI 组件栈
    terminal: Terminal,           // 终端屏幕
    pub editor: Editor,           // 全局编辑器状态
    config: Arc<ArcSwap<Config>>, // 运行时配置（可热重载）
    signals: Signals,             // 操作系统信号（SIGINT/SIGTERM/SIGWINCH）
    jobs: Jobs,                   // 异步任务队列
    lsp_progress: LspProgressMap, // LSP 进度上报跟踪
    ...
}
```

注意 `config` 用 `ArcSwap` 包装：配置在运行时可替换（`:config-refresh` 后整体换新），
而 `Editor` 内的 `config()` 返回的是 `DynGuard` 动态读取（`helix-view/src/editor.rs`）。

## 5. 打开文件

`Application::new` 末尾按优先级打开文件：

1. `--tutor`：打开内置教程文件，并 `set_path(None)` 防止误存到原文件；
2. 命令行文件列表：第一个若是目录则弹出文件 picker（`ui::file_picker` + `overlaid` 包装成
   弹出层），其余文件按 `--vsplit`/`--hsplit` 决定分屏方式打开；
3. 无文件：进入空编辑器（后续通过 `:open` 或 `Space+f` 打开）。

打开动作的核心是 `Editor::open(&path, action)`（`helix-view/src/editor.rs`），
`Action` 枚举（`Load`/`Replace`/`HorizontalSplit`/`VerticalSplit`）决定新文档的展示方式。

## 6. 进入事件循环：`run()`

```rust
// helix-term/src/application.rs
pub async fn run<S>(&mut self, input_stream: &mut S) -> Result<i32, Error> {
    self.terminal.claim()?;              // 进入原始模式（raw mode），接管终端
    self.event_loop(input_stream).await; // 事件循环（直到关闭）
    let close_errs = self.close().await; // 清理：等待 job 完成、刷新写盘、关闭 LSP
    self.restore_term()?;                // 恢复终端
    Ok(self.editor.exit_code)
}
```

`terminal.claim()`（`helix-tui/src/terminal.rs`）把终端切换到**原始模式**：关闭回显、
关闭行缓冲、启用鼠标/键盘转义序列——之后每个按键都会以转义序列形式送达。

`event_loop` 先渲染一屏，然后循环调用 `event_loop_until_idle`：

```rust
pub async fn event_loop_until_idle<S>(&mut self, input_stream: &mut S) -> bool {
    loop {
        if self.editor.should_close() { return false; }
        tokio::select! { biased;
            Some(signal) = self.signals.next() => { ... }          // OS 信号
            Some(event) = input_stream.next() => {                 // 终端事件
                self.handle_terminal_events(event).await;
            }
            Some(callback) = self.jobs.callbacks.recv() => { ... } // 异步 job 回调
            Some(msg) = self.jobs.status_messages.recv() => { ... }// 状态消息
            Some(callback) = self.jobs.wait_futures.next() => { ... }
            event = self.editor.wait_event() => {                  // 编辑器事件
                let _ = self.handle_editor_event(event).await;
            }
        }
    }
}
```

这是整个编辑器的**心脏**：一个 `tokio::select!` 同时监听五类事件源，任何一个到来就处理、
必要时重绘，然后继续等待。`biased` 保证信号优先处理。

### 五类事件源

| 事件源 | 内容 | 处理函数 |
|--------|------|---------|
| `signals` | SIGINT/SIGTERM/SIGWINCH 等 OS 信号 | `handle_signals` |
| `input_stream` | 终端按键/鼠标/粘贴事件 | `handle_terminal_events` |
| `jobs.callbacks` | 后台任务完成后的回调闭包 | `jobs.handle_callback` |
| `jobs.status_messages` | 后台任务的状态消息（如 LSP 进度） | 直接写入 `editor.status_msg` |
| `editor.wait_event()` | 编辑器内部事件（文档保存完成、LSP 消息、空闲定时器等） | `handle_editor_event` |

### `EditorEvent` 枚举（helix-view/src/editor.rs）

```rust
pub enum EditorEvent {
    DocumentSaved(DocumentSavedEventResult),      // 异步保存完成
    ConfigEvent(ConfigEvent),                      // 配置刷新/更新/主题变更
    LanguageServerMessage((LanguageServerId, Call)), // LSP 服务器发来的请求/通知
    DebuggerEvent((DebugAdapterId, dap::Payload)), // DAP 调试器消息
    IdleTimer,                                     // 空闲定时器（用于延迟执行如 lint）
    Redraw,                                        // 请求重绘
}
```

## 7. 关闭流程

`Application::close()` 尽量做完全部清理（出错也不提前返回）：

1. `jobs.finish(...)` — 等待所有需等待的后台任务；
2. `editor.flush_writes()` — 刷新所有待写盘的文档；
3. `editor.close_language_servers(None)` — 给所有 LSP 发 `shutdown`/`exit`。

然后 `restore_term()` 恢复终端（退出原始模式），返回 `editor.exit_code`
（保存失败、LSP 错误等场景会把它置为 1）。

## 8. 小结：启动时序

```
main()
 ├─ 解析参数 / 提前退出分支（help/health/grammar）
 ├─ 设置工作目录 → 加载 Config（失败回退默认）
 ├─ 加载语言配置 languages.toml
 ├─ Application::new
 │   ├─ 终端后端 + Terminal + Compositor
 │   ├─ Editor（含 Tree 分屏树、语言服务器注册表）
 │   ├─ Keymaps（默认 + 用户覆盖合并）
 │   ├─ EditorView 组件入栈
 │   └─ 打开 tutor / 文件 / 目录 picker
 ├─ run()
 │   ├─ terminal.claim()（原始模式）
 │   ├─ event_loop（渲染 + tokio::select! 五路事件循环）
 │   ├─ close()（job/写盘/LSP 清理）
 │   └─ restore_term()
 └─ exit(editor.exit_code)
```

下一步：跟随 [03-tutorial-keypress.md](./03-tutorial-keypress.md) 看一个按键如何变成命令。
