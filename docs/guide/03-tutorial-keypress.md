# 03 · 一个按键的旅程：终端事件 → keymap → 命令

> 本文跟随用户按下一个键（比如 `x` 删除字符）后，事件如何穿过各层变成实际动作。
> 主线：`application.rs` → `compositor.rs` → `ui/editor.rs` → `keymap.rs` → `commands.rs`。

## 1. 终端事件到达

用户在终端按 `x`，终端把它编码为转义序列（原始模式）送达程序，`input_stream` 产出
`TerminalEvent::Key(...)`，被 `Application::handle_terminal_events` 接收：

```rust
// helix-term/src/application.rs
pub async fn handle_terminal_events(&mut self, event: std::io::Result<TerminalEvent>) {
    let should_redraw = match event.unwrap() {
        termina::Event::WindowResized(...) => { /* 调整大小 */ }
        termina::Event::Key(KeyEvent { kind: KeyEventKind::Release, .. }) => false, // 忽略释放
        termina::Event::Csi(...) => { /* 主题模式上报等 */ }
        event if event.is_escape() => false,
        event => self.compositor.handle_event(&event.into(), &mut cx),
    };
    if should_redraw && !self.editor.should_close() {
        self.render().await;
    }
}
```

关键点：

- **按键释放事件被忽略**（`KeyEventKind::Release`）；
- 事件被 `into()` 转换为 `compositor::Event`（`Event::Key(KeyEvent)`）；
- 处理完若需要重绘则渲染。

## 2. Compositor 事件冒泡

```rust
// helix-term/src/compositor.rs
pub fn handle_event(&mut self, event: &Event, cx: &mut Context) -> bool {
    // 若正在录制宏，先把按键记入 macro_recording
    if let (Event::Key(key), Some((_, keys))) = (event, &mut cx.editor.macro_recording) { ... }

    // 从最顶层 layer 往下冒泡
    for layer in self.layers.iter_mut().rev() {
        match layer.handle_event(event, cx) {
            EventResult::Consumed(callback) => { consumed = true; break; }
            EventResult::Ignored(callback) => { callbacks.push(callback); }
            ...
        }
    }
    for callback in callbacks { callback(self, cx) }
    consumed
}
```

`Compositor.layers` 是 `Vec<Box<dyn Component>>`，**栈顶是最新 push 的层**（补全菜单、picker、
命令面板等弹出层）。事件从栈顶向下传播，第一个返回 `Consumed` 的组件吃掉事件。
弹出层打开时（例如补全菜单），按键不会落到编辑器主体。

## 3. 主编辑器组件处理按键

正常情况下栈里只有 `ui::EditorView`，它的 `handle_event`（`helix-term/src/ui/editor.rs`）处理
`Event::Key`：

```
Event::Key(mut key)
 ├─ cx.editor.reset_idle_timer()          // 任何按键重置空闲定时器
 ├─ canonicalize_key(&mut key)            // 归一化（如 Shift+Tab → BackTab）
 ├─ cx.editor.status_msg = None           // 清除状态消息
 ├─ mode = cx.editor.mode()               // Normal / Insert / Select
 ├─ on_next_key 回调检查（注册了"下一个按键"回调？）
 ├─ Mode::Insert → insert_mode(cx, key)   // 插入模式：先喂 keymap，NotFound 则直接插字符
 └─ 其他模式   → command_mode(mode, cx, key)
```

### `canonicalize_key`

`canonicalize_key`（同文件 1760 行）做按键归一化：例如把 `Ctrl+Shift+Tab` 规范为 `BackTab`，
保证不同终端上报的等价按键在 keymap 中命中同一条目。

## 4. `command_mode`：count / 寄存器 / keymap 查找

```rust
fn command_mode(&mut self, mode: Mode, cxt: &mut commands::Context, event: KeyEvent) {
    match (event, cxt.editor.count) {
        // 已有 count 时输入数字 → 继续累加 count
        (key!(i @ '0'..='9'), Some(count)) => { cxt.editor.count = ... }
        // 数字键且不是 keymap 前缀 → 开始 count
        (key!(i @ '1'..='9'), None) if !self.keymaps.contains_key(mode, event) => { ... }
        // '.' 重复上次插入操作
        (key!('.'), _) if self.keymaps.pending().is_empty() => { /* 重放 last_insert */ }
        _ => {
            cxt.count = cxt.editor.count;         // 把 count 带入命令上下文
            cxt.register = cxt.editor.selected_register.take();
            let res = self.handle_keymap_event(mode, cxt, event);
            if matches!(&res, Some(KeymapResult::NotFound)) {
                self.on_next_key(OnKeyCallbackKind::Fallback, cxt, event); // 兜底回调
            }
            ...
        }
    }
}
```

要点：**数字前缀（count）和寄存器选择（`"` 键）在进入 keymap 之前被截获**，存入
`Context`，命令函数通过 `cx.count()` / `cx.register` 读取。

## 5. `handle_keymap_event`：查找并执行

```rust
fn handle_keymap_event(&mut self, mode: Mode, cxt: &mut commands::Context, event: KeyEvent)
    -> Option<KeymapResult>
{
    let key_result = self.keymaps.get(mode, event);   // ← 核心查找

    let mut execute_command = |command: &MappableCommand| {
        command.execute(cxt);                          // ← 执行命令
        helix_event::dispatch(PostCommand { ... });    // 广播"命令已执行"事件（插件用）
        if 模式切换了 { helix_event::dispatch(OnModeSwitch { ... }); }
        if 进入 Insert 模式 { self.last_insert.0 = command.clone(); ... } // 记录供 '.' 重放
    };

    match &key_result {
        KeymapResult::Matched(command)        => execute_command(command),
        KeymapResult::Pending(node)           => cxt.editor.autoinfo = Some(node.infobox()),
        KeymapResult::MatchedSequence(cmds)   => for c in cmds { execute_command(c) },
        KeymapResult::NotFound | Cancelled(_) => return Some(key_result),
    }
    None
}
```

### `Keymaps::get`：trie 查找（helix-term/src/keymap.rs）

按键映射是**前缀树（trie）**，支持多键序列（如 `g` + `g` 回到文件头）：

```rust
pub fn get(&mut self, mode: Mode, key: KeyEvent) -> KeymapResult {
    // Esc 取消挂起状态
    // 从 state（已按下的前缀键）继续查找
    match trie.search(...) {
        Some(KeyTrie::MappableCommand(cmd)) => Matched(cmd.clone()),       // 命中命令
        Some(KeyTrie::Sequence(cmds))       => MatchedSequence(cmds),      // 命中序列
        Some(KeyTrie::Node(map))            => { state.push(key); Pending } // 还有更多键
        None                                => Cancelled(state)            // 前缀失效
    }
}
```

四种结果的命运：

| `KeymapResult` | 含义 | 处理 |
|----------------|------|------|
| `Matched(cmd)` | 唯一命中命令 | 立即执行 |
| `MatchedSequence(cmds)` | 命中宏序列 | 依次执行全部命令 |
| `Pending(node)` | 前缀匹配，等待更多按键 | 在状态栏显示候选键（autoinfo） |
| `NotFound` | 完全没匹配 | insert 模式直接插字符；normal 模式走 `on_next_key` 兜底 |
| `Cancelled(pending)` | 前缀作废 | 回放已按的键（insert 模式把它们当字符插入） |

### 模式（Mode）

模式只有三个（`helix-view/src/document.rs`）：

```rust
pub enum Mode { Normal = 0, Select = 1, Insert = 2 }
```

keymap 是 `HashMap<Mode, KeyTrie>`（`Keymaps.map`），所以同样按键在不同模式命中不同命令。
`g`、`Space` 等"子模式"不是独立 `Mode`，而是 trie 的中间节点——这是与 vim 的关键区别之一
（vim 的 operator-pending 模式在 Helix 里由命令自身处理）。

## 6. `MappableCommand::execute`：三种命令

```rust
// helix-term/src/commands.rs
pub enum MappableCommand {
    Typable { name, args, doc },   // :命令，如 :write
    Static { name, fun, doc },     // 普通按键命令，如 move_char_left
    Macro { name, keys },          // 键序列宏
}
```

- `Static`：直接调用函数指针 `fun(cx)`；
- `Typable`：查 `TYPABLE_COMMAND_MAP`（`commands/typed.rs`）找到 `TypableCommand` 执行；
- `Macro`：把 `keys` 逐个喂回 keymap（用于 `@` 宏录制回放）。

所有命令共享同一个 `Context`：

```rust
pub struct Context<'a> {
    pub register: Option<char>,          // 选中的寄存器（" 键）
    pub count: Option<NonZeroUsize>,     // 数字前缀
    pub editor: &'a mut Editor,          // 全局状态
    pub callback: Vec<Callback>,         // 延迟到事件循环末尾执行的闭包
    pub on_next_key_callback: Option<...>, // 等待下一个按键的回调
    pub jobs: &'a mut Jobs,              // 异步任务队列
}
```

**命令可以"预约"回调**：`cx.callback` 中的闭包在按键事件处理完后统一执行
（`handle_event` 末尾 `for callback in callbacks { callback(self, cx) }`），用于命令内
想 push 弹出层等需要 `Compositor` 的场景。

## 7. 命令执行后的收尾

`handle_event` 尾部（`ui/editor.rs`）：

```rust
// 光标滚出视野则滚动
view.ensure_cursor_in_view(doc, config.scrolloff);
// 非 insert 模式提交历史状态（insert 模式的多次修改合并为一次 undo）
if mode != Mode::Insert { doc.append_changes_to_history(view); }
// 返回 Consumed(callback) → compositor 执行回调 → application 重绘
```

回到 `handle_terminal_events`：`should_redraw == true` → `render()`。

## 8. 全景图

```
按 x
 └─ termina/crossterm 解码转义序列 → TerminalEvent::Key
    └─ application.rs::handle_terminal_events
       └─ compositor::handle_event（栈顶冒泡）
          └─ ui/editor.rs::EditorView::handle_event
             ├─ reset_idle_timer / canonicalize_key / 清状态消息
             ├─ Mode::Insert → insert_mode   │  Mode::Normal → command_mode
             │    └─ handle_keymap_event     │     ├─ count/寄存器截获
             │        └─ keymaps.get()       │     └─ handle_keymap_event
             │            └─ KeymapResult    │         └─ keymaps.get()
             │               ├─ Matched ─── MappableCommand::execute(cx)
             │               ├─ Pending → 状态栏显示候选键
             │               └─ NotFound → insert 模式插字符 / Fallback 回调
             ├─ ensure_cursor_in_view
             └─ append_changes_to_history（非 insert 模式）
          └─ EventResult::Consumed(callback)
       └─ 执行 callbacks
    └─ render()（见 05-tutorial-render.md）
```

下一步：[04-tutorial-edit.md](./04-tutorial-edit.md) 看命令内部如何修改文本。
