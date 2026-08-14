# 05 · 渲染流程：View → Surface → 终端绘制

> 本文跟随一次重绘的完整路径：从 `Application::render` 到终端屏幕上出现文字。
> 主线：`application.rs` → `compositor.rs` → `ui/editor.rs` → `ui/document.rs` → `helix-tui`。

## 1. 渲染入口：Application::render

```rust
// helix-term/src/application.rs
async fn render(&mut self) {
    // 需要全量重绘时先清屏
    if self.compositor.full_redraw {
        self.terminal.clear().expect("Cannot clear the terminal");
        self.compositor.full_redraw = false;
    }

    let mut cx = crate::compositor::Context {
        editor: &mut self.editor,
        jobs: &mut self.jobs,
        scroll: None,
    };

    helix_event::start_frame();      // 帧开始标记（事件系统）
    cx.editor.needs_redraw = false;

    let area = self.terminal.autoresize().expect("...");  // 终端尺寸可能已变化

    let surface = self.terminal.current_buffer_mut();     // ← 当前帧的 Buffer

    self.compositor.render(area, surface, &mut cx);       // ← 组件树绘制

    let (pos, kind) = self.compositor.cursor(area, &self.editor); // 光标位置与形状
    self.editor.cursor_cache.reset();                     // 清光标缓存

    let pos = pos.map(|pos| (pos.col as u16, pos.row as u16));
    self.terminal.draw(pos, kind).unwrap();               // ← 把 Buffer 画到终端
}
```

渲染不是"每次全量重画"，而是：**组件把状态画进内存里的 Surface（Buffer），
`terminal.draw` 把 Surface 与上一帧做 diff，只输出变化的部分**（增量刷新）。

## 2. Compositor::render：层叠绘制

```rust
// helix-term/src/compositor.rs
pub fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
    for layer in &mut self.layers {
        layer.render(area, surface, cx);
    }
}
```

按**栈底到栈顶**顺序渲染每个组件层：先画编辑器主体，再画弹出层（补全菜单、picker 等），
后画的盖住先画的。光标位置则反向查（`cursor()` 从栈顶往下找第一个有光标的组件）。

## 3. 主编辑器组件的 render（ui/editor.rs）

`ui::EditorView::render`（`helix-term/src/ui/editor.rs:1625`）：

```
清背景（theme 的 ui.background）
计算编辑器区域：
  ├─ 底部留 1 行给命令行/状态消息
  ├─ 若启用 bufferline（多文档时）顶部再留 1 行
  └─ cx.editor.resize(editor_area)   // 分屏树按新尺寸重排
渲染 bufferline（可选）
for (view, is_focused) in editor.tree.views() {
    render_view(editor, doc, view, area, surface, is_focused)   // ← 每个分屏
}
渲染 autoinfo（按键候选提示框）
渲染状态消息（error/status）
渲染 count / pending keys / 宏录制指示（底部）
```

### render_view：一个分屏怎么画（ui/editor.rs:77）

每个分屏（`View`）独立绘制：

```
view.inner_area(doc)          // 分屏内减去行号栏等后的文本区域
获取 view_offset（滚动偏移 anchor + 视觉偏移）
收集 text_annotations（虚拟文本、inlay hints 等行内标注）
准备 decorations（DecorationManager）：
  ├─ cursorline / cursorcolumn（配置开启时）
  ├─ DAP 当前栈帧行高亮
  └─ 光标块（Cursordecoration，含光标形状）
创建语法高亮器 doc_syntax_highlighter
收集 overlays（叠层高亮）：
  ├─ 语法高亮 overlay
  ├─ 彩虹括号
  ├─ document links
  ├─ 诊断（diagnostics）高亮
  ├─ 选区高亮（doc_selection_highlights）
  └─ tabstop 高亮
渲染行号栏 render_gutter
渲染标尺 render_rulers
render_document(surface, inner, doc, view_offset, ...)   // ← 核心：逐行渲染文本
绘制分屏右边框（非最右分屏）
渲染状态栏 statusline::render
```

### render_document：逐行渲染（helix-term/src/ui/document.rs）

```rust
pub fn render_document(surface, viewport, doc, offset, doc_annotations,
                       syntax_highlighter, overlay_highlights, theme, decorations) {
    let mut renderer = TextRenderer::new(surface, doc, theme, Position::new(...), viewport);
    render_text(&mut renderer, doc.text().slice(..), offset.anchor,
                &doc.text_format(...), doc_annotations, syntax_highlighter,
                overlay_highlights, theme, decorations)
}
```

`render_text` 内部的流程：

1. **软换行**：`DocumentFormatter`（`helix-core/src/doc_formatter.rs`）按终端宽度把
   Rope 文本切成视觉行（visual line），处理折行、制表符宽度；
2. **语法高亮**：`SyntaxHighlighter` 迭代 tree-sitter 语法树节点，把每个 token 的
   capture 映射为主题样式（`helix-core/src/syntax.rs` 的 `Highlighter`）；
3. **行内标注**：`text_annotations`（inlay hints、虚拟文本、诊断内联显示）插入到对应位置；
4. **decorations**：光标、光标行、诊断波浪线等在文本绘制后叠加；
5. 每个字符调用 `renderer.set_char(...)` 写入 Surface 的对应 `Cell`。

`TextRenderer`（`ui/document.rs:176`）维护当前位置，处理 UTF-8 宽度（全角字符占 2 列）与
宽字符换行边界。

## 4. Surface：内存里的"屏幕"

`helix-tui/src/buffer.rs`：

```rust
pub struct Cell {
    pub symbol: String,   // 单元格显示的字符（可能是宽字符的一部分）
    pub style: Style,     // 前景色 / 背景色 / 修饰（粗体、斜体、下划线）
    // ...
}

pub struct Buffer {
    pub area: Rect,
    pub content: Vec<Cell>,  // 长度恒等于 area.width * area.height
}
```

组件只操作 `Buffer`（`set_string` / `set_style` 等），**完全不知道终端的存在**——
这是"渲染与终端解耦"的关键。`Surface` 就是 `Buffer` 的别名（`compositor.rs` 中
`use tui::buffer::Buffer as Surface;`）。

## 5. Terminal::draw：diff + 输出

`helix-tui/src/terminal.rs`：

```rust
pub fn draw(&mut self, cursor_position, cursor_kind) -> io::Result<()> {
    self.backend.start_sync()?;          // 开始同步帧（终端转义包裹，防闪烁）

    self.flush()?;                       // ← 核心：diff 并输出

    if let Some((x, y)) = cursor_position { self.set_cursor(x, y)?; }
    match cursor_kind { CursorKind::Hidden => self.hide_cursor()?, kind => self.show_cursor(kind)? }

    self.backend.end_sync()?;            // 结束同步帧

    self.buffers[1 - self.current].reset();   // 复用旧的"上一帧"缓冲
    self.current = 1 - self.current;          // 交换双缓冲
    self.backend.flush()?;                    // 真正刷到终端
    Ok(())
}

pub fn flush(&mut self) -> io::Result<()> {
    let previous_buffer = &self.buffers[1 - self.current];   // 上一帧
    let current_buffer  = &self.buffers[self.current];       // 这一帧
    let updates = previous_buffer.diff(current_buffer);      // ← 逐 Cell 比较
    self.backend.draw(updates.into_iter())                   // 只输出变化
}
```

关键机制：

- **双缓冲**：`buffers[0]` / `buffers[1]` 轮流当"当前帧"与"上一帧"；
- **增量 diff**：`Buffer::diff` 逐单元格比较新旧两帧，只把变化的单元格转成
  `(x, y, Cell)` 更新发给后端（`backend.draw`）——这就是为什么大幅滚动时终端输出量小；
- **同步帧**：`start_sync`/`end_sync` 用终端转义序列（如 tmux/screen 的同步帧协议）把
  整帧更新包裹起来，避免用户在重绘中途看到残影；
- **光标分离**：光标位置与形状独立于文本 diff 设置（`set_cursor`/`show_cursor`）。

## 6. 后端（Backend）

`TerminalBackend` trait（`helix-tui/src/backend/mod.rs`）是 tui 层与具体终端库之间的接口：

| 后端 | 平台 | 说明 |
|------|------|------|
| `TerminaBackend` | Unix（默认） | 基于 `termina` crate（`application.rs` 中 cfg 选择） |
| `CrosstermBackend` | Windows | 基于 `crossterm` |
| `TestBackend` | 集成测试（`feature = "integration"`） | 内存模拟终端，供测试断言屏幕内容 |

后端的职责：`draw(updates)` 把单元格更新写成终端转义序列、管理光标、清屏、查询尺寸。

## 7. 重绘触发时机

什么情况下会调用 `render()`？主要路径：

| 触发 | 位置 |
|------|------|
| 按键处理后 | `handle_terminal_events` 末尾（`should_redraw` 为 true 时） |
| 异步 job 回调后 | `event_loop_until_idle` 的 `jobs.callbacks` / `wait_futures` 分支 |
| 编辑器事件后 | `handle_editor_event`（文档保存、LSP 消息、Redraw 事件等） |
| `request_redraw()` | `helix-event` 的全局重绘请求（LSP 消息洪泛时合并成一次渲染） |

注意 `helix_event::request_redraw()` 与 `EventEditor::Redraw` 的配合：
高频事件（如 LSP 诊断流）只请求重绘，由事件循环在合适时机统一渲染，避免每帧都画。

## 8. 小结

```
需要重绘（按键 / job 回调 / 编辑器事件）
 └─ Application::render
     ├─ (full_redraw?) terminal.clear()
     ├─ surface = terminal.current_buffer_mut()
     ├─ Compositor::render(area, surface, cx)     // 栈底→栈顶逐层绘制
     │   └─ EditorView::render
     │       ├─ 清背景 / bufferline / 分屏区域计算
     │       └─ 每个 View → render_view
     │           ├─ overlays（语法/选区/诊断/彩虹括号）
     │           ├─ render_document → render_text
     │           │   ├─ DocumentFormatter 软换行
     │           │   ├─ SyntaxHighlighter 语法高亮
     │           │   ├─ text_annotations 行内标注
     │           │   └─ 写入 Buffer 的 Cell
     │           └─ gutter / rulers / statusline
     └─ Terminal::draw(pos, kind)
         ├─ start_sync（防闪烁同步帧）
         ├─ flush → Buffer::diff(上一帧, 当前帧) → backend.draw(变化单元格)
         ├─ set_cursor / 光标形状
         ├─ end_sync / 交换双缓冲 / backend.flush()
```

至此三条主线（启动 → 按键 → 编辑 → 渲染）已走完。接下来按需深入各 crate：
[06-deep-core.md](./06-deep-core.md) 起是逐 crate 详解。
