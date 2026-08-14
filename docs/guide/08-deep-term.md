# 08 · 深入 helix-term：终端 UI 层

> `helix-term` 是 workspace 中最大的 crate（58 文件、3.2 万行），把 core（编辑模型）、
> view（状态）、lsp/dap（外部服务）、tui（绘制）粘合成完整的编辑器。
> 入口：`main.rs`；核心：`application.rs`（事件循环）、`compositor.rs`（组件栈）、
> `keymap.rs`、`commands.rs`、`ui/`（界面组件）、`handlers/`（异步响应）、`job.rs`（任务调度）。

## 核心概念

- **一切围绕事件循环**：`Application::event_loop_until_idle` 是唯一的主循环（详见
  [02-tutorial-startup.md](./02-tutorial-startup.md) 第 6 节），五路事件源
  （信号/终端事件/job 回调/状态消息/编辑器事件）汇聚于此；
- **组件栈**：UI 由 `Component` 层栈构成，弹出层就是 push 新层；
- **命令体系**：按键绑定到 `MappableCommand`，命令拿到 `&mut Editor` 直接改状态。

## 1. Application（application.rs）

见 [02-tutorial-startup.md](./02-tutorial-startup.md)。这里补充 `handle_language_server_message`
的职责（`application.rs:799`）：把 LSP 服务器的请求/通知分发到对应 handler：

```
LSP 消息（Call）
 ├─ 请求（Request）：如 workspace/applyEdit、window/showMessage → 注册回调处理
 └─ 通知（Notification）：诊断/进度/完成项 → editor.handlers.* 更新
```

## 2. Compositor：组件栈（compositor.rs）

### Component trait

```rust
pub trait Component: Any + AnyComponent {
    fn handle_event(&mut self, event: &Event, ctx: &mut Context) -> EventResult;
    fn render(&mut self, area: Rect, frame: &mut Surface, ctx: &mut Context);
    fn cursor(&self, area: Rect, ctx: &Editor) -> (Option<Position>, CursorKind);
    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)>;
    fn id(&self) -> Option<&'static str>;  // 用于 replace_or_push / remove 定位
}
```

- `EventResult::Consumed` / `Ignored`，均可携带 `Callback`（延迟闭包）；
- `AnyComponent` 提供 `downcast`：Compositor 上可以按类型/ID 找到具体组件
  （`compositor.find::<T>()` / `find_id`）。

### Compositor

```rust
pub struct Compositor {
    layers: Vec<Box<dyn Component>>,  // 栈底 → 栈顶
    area: Rect,
    pub(crate) full_redraw: bool,
}
```

- `push` / `pop` / `remove` / `replace_or_push`：层管理；
- `handle_event`：从**栈顶**向下冒泡，第一个 `Consumed` 停止（详见
  [03-tutorial-keypress.md](./03-tutorial-keypress.md) 第 2 节）；
- `render`：从**栈底**向上绘制，后画的盖先画的；
- `cursor`：从栈顶向下找第一个有光标的组件；
- `Context`（`compositor.rs`）：渲染/事件上下文，含 `editor`、`jobs`、`scroll`；
  提供 `block_try_flush_writes`（阻塞等待写盘完成，`:write!` 等场景用）。

## 3. Keymap：按键映射（keymap.rs）

```rust
pub enum KeyTrie {
    Node(KeyTrieNode),                  // 前缀节点（如 g 键进入子表）
    MappableCommand(MappableCommand),   // 叶子：绑定命令
    Sequence(Vec<MappableCommand>),     // 叶子：一串命令（宏）
}

pub enum KeymapResult {
    Pending(KeyTrieNode),               // 前缀匹配，等待更多键
    Matched(MappableCommand),
    MatchedSequence(Vec<MappableCommand>),
    NotFound,                            // 完全未命中
    Cancelled(Vec<KeyEvent>),            // 前缀作废，返回已按的键
}

pub struct Keymaps {
    pub map: Box<dyn DynAccess<HashMap<Mode, KeyTrie>>>,  // 每模式一棵 trie
    state: Vec<KeyEvent>,                // 挂起的按键前缀
    pub sticky: Option<KeyTrieNode>,     // 粘滞节点（如 Space 模式）
}
```

- `Keymaps::get(mode, key)`：核心查找（trie 搜索，多键前缀支持）；
- `default.rs`：内置默认 keymap，`keymap!` 宏声明（如 `"g" => goto_mode`）；
- `merge_keys`：把用户配置的按键合并进默认表（递归合并节点）；
- `KeyTrieNode::merge` / `infobox`：合并逻辑与按键提示框内容生成。

## 4. 命令体系（commands.rs + commands/typed.rs）

### MappableCommand 与注册

```rust
pub enum MappableCommand {
    Typable { name, args, doc },   // :命令
    Static { name: &'static str, fun: fn(&mut Context), doc },  // 按键命令
    Macro { name, keys },          // 宏
}
```

`static_commands!` 宏（`commands.rs`）声明全部静态命令：

```rust
static_commands! {
    move_char_left, "Move left",
    move_char_right, "Move right",
    insert_mode, "Insert before selection",
    ...
}
```

每个名字同时生成一个常量（`MappableCommand::move_char_left`）与
`STATIC_COMMAND_LIST` 数组（keymap 默认表引用它们）。`MappableCommand::execute`
分派：Static 直接调函数；Typable 查 `TYPABLE_COMMAND_MAP`。

### TypableCommand（commands/typed.rs）

```rust
pub struct TypableCommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub doc: &'static str,
    pub fun: fn(&mut compositor::Context, Args, PromptEvent) -> anyhow::Result<()>,
    pub completer: CommandCompleter,  // 参数补全
    pub signature: Signature,         // 参数签名（校验）
}
pub static TYPABLE_COMMAND_MAP: Lazy<HashMap<&'static str, &'static TypableCommand>>;
```

- `:命令` 由 `command_mode`（`typed.rs`）拉起一个 `Prompt` 输入框；
- 每次按键触发 `PromptEvent`（`Update`/`Validate`/`Abort`），命令函数按事件类型
  决定行为——`Validate` 时真正执行，`Update` 时只做补全/预览；
- `execute_command`：解析 `Args`（支持 `%`/`$` 展开，`expansion::expand`），调用 `fun`。

### 命令的 Context

```rust
pub struct Context<'a> {
    pub register: Option<char>,
    pub count: Option<NonZeroUsize>,
    pub editor: &'a mut Editor,
    pub callback: Vec<Callback>,           // 延迟到事件结束执行（如 push 弹出层）
    pub on_next_key_callback: Option<...>, // 等待下一个按键（如 g + g）
    pub jobs: &'a mut Jobs,
}
```

### 常用辅助宏（commands.rs 顶部）

- `current!` / `current_ref!`：取出当前聚焦视图的 `(&mut View, &mut Document)`；
- `doc!` / `doc_mut!`：按 View 取文档；
- `typed!`：构造 Typable 命令调用。

## 5. Job：异步任务调度（job.rs）

后台任务（LSP 请求、文件 IO）不能阻塞主循环，也不能直接改 Editor：

```rust
pub enum Callback {
    EditorCompositor(Box<dyn FnOnce(&mut Editor, &mut Compositor) + Send>),
    Editor(Box<dyn FnOnce(&mut Editor) + Send>),
    Followup(Box<dyn FnOnce(&mut Editor) -> Option<Job> + Send>),  // 可链式追加任务
}

pub struct Job {
    pub future: BoxFuture<'static, anyhow::Result<Option<Callback>>>,
    pub wait: bool,   // 退出前是否必须完成
}
```

- 任务完成 → future 返回 `Callback`（闭包）→ 通过 `dispatch_callback` 经
  channel 送回主循环 → `event_loop_until_idle` 的 `jobs.callbacks.recv()` 分支执行；
- `dispatch(job)` / `dispatch_blocking(job)`：把立即闭包送回主循环；
- `Jobs::handle_callback`：执行回调，若返回 `Followup` 结果则把新 Job 重新入队；
- 编辑器代码里发起异步请求的惯用法（`commands.rs::Context::callback`）：

```rust
cx.callback(
    lsp_request_future,          // 异步任务（future）
    move |editor, compositor, result| { /* 主循环里处理结果 */ },
);
```

## 6. UI 组件（ui/）

| 组件 | 文件 | 用途 |
|------|------|------|
| `EditorView` | `ui/editor.rs` | 主编辑器组件：按键处理、渲染分屏 |
| `Completion` | `ui/completion.rs` | 补全弹出层 |
| `Prompt` | `ui/prompt.rs` | 命令行输入框（`:`、`/`、`Space+f` 等） |
| `Picker` | `ui/picker.rs` | 文件/缓冲区选择器（模糊匹配列表） |
| `Popup` | `ui/popup.rs` | 通用弹出层（信息框等） |
| `PluginPopup` | `ui/plugin_popup.rs` | JS 插件弹出层 |
| `StatusLine` | `ui/statusline.rs` | 状态栏渲染 |
| `Menu` | `ui/menu.rs` | 菜单 |
| `Select` | `ui/select.rs` | 下拉选择 |
| `Text` | `ui/text.rs` | 文本显示（帮助页等） |
| `Info` | `ui/info.rs` | 按键提示框（autoinfo） |
| `Spinner` | `ui/spinner.rs` | 进度 spinner |
| `Markdown` | `ui/markdown.rs` | Markdown 渲染（帮助页） |
| `LSP 相关` | `ui/lsp/*` | LSP 弹窗（代码操作、符号选择） |

`ui/mod.rs` 顶部提供 `prompt(...)` / `popup(...)` 等便捷工厂函数。

## 7. handlers：LSP 响应的状态管理（handlers/）

与 view 层的 `Handlers` 对应，term 层 handlers 面向具体的 UI 交互：

| 文件 | 用途 |
|------|------|
| `handlers/completion.rs` | 补全请求调度（completion/request.rs、item.rs、resolve.rs、word.rs、path.rs） |
| `handlers/diagnostics.rs` | 诊断渲染（波浪线、弹窗） |
| `handlers/signature_help.rs` | 函数签名提示 |
| `handlers/document_highlight.rs` | 光标下符号高亮 |
| `handlers/code_action_hint.rs` | 代码操作提示 |
| `handlers/document_links.rs` | 文档链接跳转 |
| `handlers/document_colors.rs` | 颜色 swatch |
| `handlers/snippet.rs` | snippet 展开 UI |
| `handlers/prompt.rs` | prompt 与命令历史 |
| `handlers/auto_save.rs` | 自动保存定时 |
| `handlers/workspace_trust.rs` | 工作区信任提示 |
| `handlers.rs` | `setup(config)` 组装全部 handler |

## 8. 其他模块

| 模块 | 用途 |
|------|------|
| `args.rs` | 命令行参数（`Args`） |
| `config.rs` | 编辑器配置：`Config` / `ConfigRaw`，`load_default`（默认 + 用户文件合并） |
| `logging.rs` | 日志初始化（`init_file`，按 `-v` 级别） |
| `health.rs` | `--health` 检查 |
| `events.rs` | term 层事件定义（`PostCommand`、`OnModeSwitch` 等，供 JS 插件订阅） |

## 9. 数据流总览

```
终端按键 → application::handle_terminal_events
         → compositor::handle_event（栈顶冒泡）
         → EditorView::handle_event
             → keymaps.get(mode, key) → KeymapResult
             → MappableCommand::execute(cx)
                 → 命令函数改 Editor/Document（Transaction）
                 → cx.callback(...) 发起异步任务（LSP 等）
             → append_changes_to_history / ensure_cursor_in_view
         → render() → compositor.render → EditorView::render → render_view
             → 各 UI 组件绘制到 Surface
         → terminal.draw()（diff + 输出）

异步完成 → jobs.callbacks channel → event_loop_until_idle
         → handlers.* 更新状态 → render()
```

## 10. 代码位置指引

| 主题 | 位置 |
|------|------|
| 入口/启动 | `helix-term/src/main.rs` |
| 事件循环 | `helix-term/src/application.rs` |
| 组件栈 | `helix-term/src/compositor.rs` |
| 按键映射 | `helix-term/src/keymap.rs`（默认表 `keymap/default.rs`） |
| 命令 | `helix-term/src/commands.rs`（`:命令` 在 `commands/typed.rs`） |
| 任务调度 | `helix-term/src/job.rs` |
| 主编辑器组件 | `helix-term/src/ui/editor.rs` |
| 文档渲染 | `helix-term/src/ui/document.rs` |
| 配置 | `helix-term/src/config.rs` |
