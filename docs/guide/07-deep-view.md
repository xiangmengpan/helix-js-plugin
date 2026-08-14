# 07 · 深入 helix-view：编辑器状态层

> `helix-view` 提供"打开的文件"这一层抽象：`Editor`（全局状态）、`Document`（一个文件）、
> `View`（一个分屏）以及配套的输入、寄存器、handlers。约 25 个文件、1.5 万行。
> 模块清单见 `helix-view/src/lib.rs`。

## 核心概念

- **三个层次**：`Editor` 拥有所有 `Document` 和分屏树 `Tree`；`Tree` 里的每个叶子是
  `View`；每个 `View` 引用一个 `DocumentId`，多个 View 可以看同一个 Document。
- **Document 是"文件 + 派生状态"的聚合**：文本（Rope）、每视图光标、语法树、undo 历史、
  LSP 客户端、诊断、diff，全部挂在 Document 上。
- **命令通过 `&mut Editor` 操作一切**：命令函数（helix-term 层）拿到 `&mut Editor`
  后，用 `current!` / `doc!` / `view!` 宏取出当前文档和视图。

## 1. Editor：全局状态（editor.rs）

```rust
pub struct Editor {
    pub mode: Mode,                            // Normal / Insert / Select
    pub tree: Tree,                            // 分屏树
    pub documents: BTreeMap<DocumentId, Document>,
    pub registers: Registers,                  // 寄存器（"a 等）
    pub count: Option<NonZeroUsize>,           // 数字前缀
    pub selected_register: Option<char>,       // " 键选中的寄存器
    pub macro_recording: Option<(char, Vec<KeyEvent>)>, // 宏录制状态
    pub language_servers: helix_lsp::Registry, // 所有 LSP 客户端
    pub diagnostics: Diagnostics,              // 跨文档诊断集合
    pub diff_providers: DiffProviderRegistry,  // 版本控制 diff
    pub debug_adapters: dap::registry::Registry, // DAP 调试器
    pub theme: Theme,                          // 当前主题
    pub status_msg: Option<(Cow<str>, Severity)>, // 状态栏消息
    pub config: Arc<dyn DynAccess<Config>>,    // 运行时配置（ArcSwap 动态读取）
    pub idle_timer: Pin<Box<Sleep>>,           // 空闲定时器（LSP 延迟请求用）
    pub last_motion: Option<Motion>,           // 上次移动（; 重复）
    pub exit_code: i32,                        // 退出码
    pub handlers: Handlers,                    // LSP/DAP 响应处理器
    pub workspace_trust: WorkspaceTrust,
    // ...
}
```

要点：

- `config` 是 `Arc<dyn DynAccess<Config>>`：配置热重载时（`:config-refresh`）整个 ArcSwap
  换新，所有读取点拿到新值；
- `idle_timer`：每次按键 `reset_idle_timer()`，超时触发 `EditorEvent::IdleTimer`，
  用于"停手后才请求"的延迟操作（如文档诊断刷新）；
- `Motion = Box<dyn Fn(&mut Editor)>`：`apply_motion` / `repeat_last_motion` 实现
  `;`（重复上次移动）；
- 常用便捷方法：`mode()`、`config()`、`set_status`/`set_error`、`refresh_config`、
  `should_close()`。

### EditorEvent（editor.rs:1351）

```rust
pub enum EditorEvent {
    DocumentSaved(DocumentSavedEventResult),
    ConfigEvent(ConfigEvent),                 // Refresh / Update / ThemeChanged
    LanguageServerMessage((LanguageServerId, Call)),
    DebuggerEvent((DebugAdapterId, dap::Payload)),
    IdleTimer,
    Redraw,
}
```

Editor 内部产生的事件通过 `editor.wait_event()`（application 事件循环的一个分支）送达
`Application::handle_editor_event`。

## 2. Document：一个打开的文件（document.rs）

```rust
pub struct Document {
    id: DocumentId,
    text: Rope,                                     // 文本本体
    selections: HashMap<ViewId, Selection>,         // 每个视图各有一份光标
    view_data: HashMap<ViewId, ViewData>,           // 每个视图的滚动位置等
    active_snippet: Option<ActiveSnippet>,          // 进行中的 snippet 展开
    path: Option<PathBuf>,
    encoding: &'static Encoding,                    // 文件编码
    line_ending: LineEnding,
    syntax: Option<Syntax>,                         // tree-sitter 语法
    language: Option<Arc<LanguageConfiguration>>,
    changes: ChangeSet,                             // 未提交的累积变更
    old_state: Option<State>,                       // 变更前的快照（undo 起点）
    history: Cell<History>,                         // undo 历史
    savepoints: Vec<Weak<SavePoint>>,               // 补全等功能的恢复点
    last_saved_revision: usize,                     // 是否 modified 的依据
    version: i32,                                   // 变更版本号
    diagnostics: Vec<Diagnostic>,
    language_servers: HashMap<LanguageServerName, Arc<Client>>, // 该文档的 LSP 客户端
    diff_handle: Option<DiffHandle>,                // 后台 diff 计算
    // ... LSP 相关缓存（inlay hints、document links、highlights 等）
}
```

### 关键方法

| 方法 | 作用 |
|------|------|
| `Document::open(path, encoding)` | 从磁盘加载（`document.rs:794`） |
| `save()` / `save_impl` | 异步写盘（原子写：临时文件 + rename） |
| `apply(&Transaction, view_id)` | 应用事务（完整流程见 04 教程） |
| `apply_temporary` | 应用但不通知 LSP（补全预览用） |
| `undo(view)` / `redo(view)` | 撤销/重做 |
| `append_changes_to_history(view)` | 把累积变更提交进 undo 历史 |
| `savepoint(view)` | 创建可恢复快照（补全选择后恢复用） |
| `selection(view_id)` / `set_selection` | 读写某视图的光标 |
| `is_modified()` | 是否有未保存修改（比较当前修订与 last_saved_revision） |
| `text()` / `doc_mut!` | 访问文本 |

### SavePoint

```rust
pub fn savepoint(&mut self, view: &View) -> Arc<SavePoint>
```

返回引用计数的快照：补全/预览功能在修改文档前记 savepoint，取消时
`restore(view, savepoint)` 恢复（`SavePoint.revert` 累积了自创建以来的反向事务，
见 `apply_impl` 中 `if !self.savepoints.is_empty() { ... }` 分支）。

## 3. View：一个分屏（view.rs）

```rust
pub struct View {
    pub id: ViewId,
    pub area: Rect,               // 屏幕上的区域（由 Tree 计算）
    pub doc: DocumentId,          // 当前显示的文档
    pub jumps: JumpList,          // 跳转历史（Ctrl+o / Ctrl+i）
    pub docs_access_history: Vec<DocumentId>, // 最近访问的文档（buffer picker 用）
    pub gutters: GutterConfig,    // 行号栏配置
    doc_revisions: HashMap<DocumentId, usize>, // 各文档上次同步的修订号（懒同步）
    pub diagnostics_handler: DiagnosticsHandler,
}
```

### 关键方法

| 方法 | 作用 |
|------|------|
| `inner_area(doc)` / `inner_width(doc)` | 文本区域（减去 gutter） |
| `ensure_cursor_in_view(doc, scrolloff)` | 光标滚出视野时滚动 |
| `text_annotations(doc, theme)` | 收集该视图的虚拟文本/inlay hints |
| `pos_at_screen_coords(row, col)` | 屏幕坐标 → 文本 char 索引（鼠标点击定位） |
| `text_pos_at_visual_coords` | 视觉坐标 → char（考虑软换行） |
| `screen_coords_at_pos` | 反向：char → 屏幕坐标（光标渲染） |
| `add_to_history(doc_id)` / `remove` | 跳转历史维护 |

**懒同步机制**：`doc_revisions` 记录每个文档上次同步的修订号。切换分屏时
（`Editor::focus` 相关逻辑）只应用"新修订"，避免每帧同步所有视图。

## 4. Tree：分屏树（tree.rs）

```rust
pub struct Tree {
    root: ViewId,
    pub focus: ViewId,            // 当前聚焦的分屏
    area: Rect,
    nodes: SlotMap<ViewId, Node>, // ViewId 是 slotmap 键
}
pub enum Content {
    View(Box<View>),
    Container(Box<Container>),    // 内部分割容器
}
pub enum Layout { Horizontal, Vertical }
```

- `Tree::split(view, layout)`：在 view 旁边开新分屏；
- `Tree::remove(view_id)`：关闭分屏（若该 View 是容器则合并相邻容器）；
- `Tree::views()` / `views_mut()`：按遍历顺序产出 `(View, is_focused)`，渲染和
  `current!` 宏都依赖它；
- `focus` / `prev` / `next`：焦点移动（`Ctrl+w+w` 等）；
- `find_split_in_direction`：方向导航（`Ctrl+w+h/j/k/l`）。

## 5. 输入原语（input.rs / keyboard）

```rust
pub struct KeyEvent {
    pub code: KeyCode,            // Char / Function / Esc / Backspace ...
    pub modifiers: KeyModifiers,  // SHIFT / CTRL / ALT / SUPER ...
}
```

- `KeyEvent` 是 keymap 查找的键值（`helix-term/src/keymap.rs` 的
  `Keymaps::get(mode, KeyEvent)`）；
- `key_sequence_format()`：把按键转成显示文本（状态栏"按 g 后"的提示）。

## 6. 寄存器：Registers（register.rs）

```rust
pub struct Registers { /* HashMap<char, String> 的封装 */ }
```

- `push(name, value)`：写入（复制/删除/宏录制都往寄存器塞文本）；
- 内置特殊寄存器：`/`（搜索历史）、`%`（文件名）、`#`（剪贴板，由
  `clipboard.rs` 的剪贴板 provider 支持）。

## 7. Handlers：异步响应的着陆点（handlers.rs）

LSP/DAP 的异步响应不能在后台线程改 Editor，必须回到主循环。`Handlers` 是这些
响应的**状态容器**，由主循环的 job 回调驱动：

```rust
pub struct Handlers {
    pub completions: completion::CompletionHandler,   // 补全项缓存与请求控制
    pub diagnostics: diagnostics::DiagnosticsHandler, // 诊断同步
    pub dap: dap::DapHandler,                          // 调试器消息
    pub word_index: word_index::WordIndexHandler,      // 词索引（下划线高亮等）
    pub lsp: lsp::LspHandler,                          // 通用 LSP 通知处理
}
```

典型链路：LSP 服务器发诊断通知 → `helix-lsp` 解析为 `Call` →
`EditorEvent::LanguageServerMessage` → `application.rs::handle_language_server_message` →
`editor.handlers.diagnostics` 更新 → 重绘。事件总线（`helix_event::dispatch`）在其中
扮演"从后台线程安全地把闭包送回主循环"的角色。

## 8. 数据流：命令如何触达 Document

```
命令函数（helix-term）
 └─ current_ref!(editor) → (&mut View, &mut Document)   // 宏：聚焦分屏的视图+文档
     └─ doc.apply(&transaction, view.id)
         └─ apply_impl（见 04 教程）
             └─ helix_event::dispatch(DocumentDidChange) → LSP didChange
```

## 9. 代码位置指引

| 主题 | 位置 |
|------|------|
| 全局状态 | `helix-view/src/editor.rs` |
| 打开的文件 | `helix-view/src/document.rs` |
| 分屏 | `helix-view/src/view.rs` |
| 分屏树 | `helix-view/src/tree.rs` |
| 寄存器 | `helix-view/src/register.rs` |
| 按键事件 | `helix-view/src/input.rs` |
| 异步处理器 | `helix-view/src/handlers/*.rs` |
| 行内标注 | `helix-view/src/annotations.rs` |
| 剪贴板 | `helix-view/src/clipboard.rs` |
| 状态栏信息 | `helix-view/src/info.rs` |
| 模块总览 | `helix-view/src/lib.rs` |
