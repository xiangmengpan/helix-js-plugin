# 04 · 一次编辑的生命周期：命令 → Transaction → Rope → undo

> 本文以插入字符 `i` 后输入 `x` 为例，跟踪一次文本修改从命令函数到 Rope 变更、
> 再到可撤销历史记录的完整路径。
> 主线：`commands.rs::insert::insert_char` → `Document::apply` → `ChangeSet::apply` → `History`。

## 1. 命令函数：构造 Transaction

`insert_char`（`helix-term/src/commands.rs` 的 `pub mod insert`）是插入模式下的核心命令：

```rust
pub fn insert_char(cx: &mut Context, c: char) {
    let (view, doc) = current_ref!(cx.editor);
    let text = doc.text();
    let selection = doc.selection(view.id);

    // 每个光标处构造一个"插入点"：Change = (from, to, Some(new_text))
    let insert_char = |range: Range, ch: char| {
        let cursor = range.cursor(text.slice(..));
        let t = Tendril::from_iter([ch]);
        ((cursor, cursor, Some(t)), None)
    };

    let transaction = Transaction::change_by_and_with_selection(text, selection, |range| {
        // 自动配对（auto-pairs）、钩子等……最终产出 Change
        ...
    });
    doc.apply(&transaction, view.id);
}
```

### 核心 API：`Transaction::change_by_selection`

`helix-core/src/transaction.rs`：

```rust
pub fn change_by_selection<F>(doc: &Rope, selection: &Selection, f: F) -> Self
where
    F: FnMut(&Range) -> Change,
{
    Self::change(doc, selection.iter().map(f))
}
```

对**每个光标 Range** 调用闭包，得到一个 `Change`，全部收集起来构造一个 `Transaction`。
这就是多光标编辑的实现基础——一次事务同时修改所有光标位置的文本。

`Change` 是一个元组 `(usize, usize, Option<Tendril>)`：`(起始, 结束, 新文本)`，
`None` 表示纯删除。

## 2. Transaction 内部：ChangeSet / Operation

```rust
// helix-core/src/transaction.rs
pub enum Operation {
    Retain(usize),  // 跳过 n 个字符（不动）
    Delete(usize),  // 删除 n 个字符
    Insert(Tendril),// 插入文本
}

pub struct ChangeSet {
    changes: Vec<Operation>,  // 有序操作序列
    len: usize,               // 应用前的文档长度（校验用）
    len_after: usize,         // 应用后的文档长度
}
```

这是一种 **OT 风格的变更表示**（与 CodeMirror 6 的 changes 一致）：`Retain`/`Delete`/`Insert`
按顺序描述对文档的完整改写。例如在位置 5 插入 "x"：`Retain(5) + Insert("x") + Retain(剩余)`。

`Transaction` 在 ChangeSet 之上附加一个可选的新 `Selection`：

```rust
pub struct Transaction {
    changes: ChangeSet,
    selection: Option<Selection>,  // 有则应用后显式替换光标位置
}
```

## 3. 应用事务：Document::apply

`Document::apply(&transaction, view_id)`（`helix-view/src/document.rs`）→ `apply_inner` → `apply_impl`。

### apply_inner：记录 old_state 并累积变更

```rust
fn apply_inner(&mut self, transaction, view_id, emit_lsp_notification: bool) -> bool {
    // 若这是新一轮编辑的开始（changes 为空），记下变更前的状态——undo 的起点
    if self.changes.is_empty() && !transaction.changes().is_empty() {
        self.old_state = Some(State {
            doc: self.text.clone(),
            selection: self.selection(view_id).clone(),
        });
    }
    let success = self.apply_impl(transaction, view_id, emit_lsp_notification);
    if !transaction.changes().is_empty() {
        // 把本次事务合并进待提交的 changes（insert 模式的连续输入会合并成一次 undo）
        take_with(&mut self.changes, |changes| changes.compose(transaction.changes().clone()));
    }
    success
}
```

### apply_impl：真正的"改文档"

```rust
fn apply_impl(&mut self, transaction, view_id, emit_lsp_notification: bool) -> bool {
    let old_doc = self.text().clone();
    let changes = transaction.changes();
    if !changes.apply(&mut self.text) { return false; }   // ← ① 修改 Rope 本体

    self.modified_since_accessed = true;
    self.version += 1;

    // ② 把每个视图的 Selection 映射到新文本坐标（编辑后光标不跑偏的关键）
    for selection in self.selections.values_mut() {
        *selection = selection.clone()
            .map(transaction.changes())
            .ensure_invariants(self.text.slice(..));
    }

    // ③ 视图滚动锚点同样映射
    for view_data in self.view_data.values_mut() {
        view_data.view_position.anchor = transaction.changes()
            .map_pos(view_data.view_position.anchor, Assoc::Before);
    }

    // ④ 更新 savepoint 的 revert 事务（保存点始终可恢复）
    if !self.savepoints.is_empty() {
        let revert = transaction.invert(&old_doc);
        ...compose 进每个 savepoint...
    }

    // ⑤ 增量更新 tree-sitter 语法树（只重解析受影响区域）
    if let Some(syntax) = &mut self.syntax {
        syntax.update(old_doc.slice(..), self.text.slice(..), transaction.changes(), &loader);
    }

    // ⑥ 启动 diff 计算（版本控制集成，后台线程）
    if let Some(diff_handle) = &self.diff_handle {
        diff_handle.update_document(self.text.clone(), false);
    }

    // ⑦ 把 diagnostics / inlay hints / document highlights 的位置映射到新文本
    changes.update_positions(...);

    // ⑧ 广播 DocumentDidChange 事件（LSP 客户端订阅它，向服务器发 didChange）
    helix_event::dispatch(DocumentDidChange { doc: self, view: view_id, old_text: &old_doc, changes, ... });

    // ⑨ 若事务带显式新 Selection，则替换之
    if let Some(selection) = transaction.selection() { ... }

    true
}
```

### `ChangeSet::apply`（helix-core/src/transaction.rs）

把操作序列应用到 Rope：从头遍历，`Retain(n)` 跳过、`Delete(n)` 移除、`Insert(t)` 插入，
最后校验文档长度与 `len_after` 一致，不一致说明事务与文档不匹配，返回 `false`（拒绝应用）。

## 4. 坐标映射：编辑后光标为什么不跑偏

`Selection::map(&ChangeSet)` 对每个 Range 调用 `ChangeSet::map_pos`：

```rust
// helix-core/src/selection.rs
impl Range {
    pub fn map(mut self, changes: &ChangeSet) -> Self {
        self.anchor = changes.map_pos(self.anchor, Assoc::After);
        self.head = changes.map_pos(self.head, Assoc::After);
        self
    }
}
```

`map_pos(pos, assoc)`（`transaction.rs`）把旧文档坐标翻译成新文档坐标。`Assoc` 枚举决定
插入点附近的坐标如何"粘附"：

| `Assoc` | 行为 |
|---------|------|
| `Before` | 位置**之前**插入文本时，坐标不移动（光标在插入点左边） |
| `After` | 位置**之后**插入文本时，坐标随插入移动（光标在插入点右边） |
| `AfterWord` / `BeforeWord` | 根据插入的是否为词字符决定（用于诊断位置） |
| `BeforeSticky` / `AfterSticky` | 精确替换区间内保持到替换起始/结束的偏移 |

这就是"在光标前插入字符时光标保持不动、在光标位置插入时光标后移"的实现。

## 5. 提交历史：append_changes_to_history

按键处理尾部（`ui/editor.rs::handle_event`，非 insert 模式）调用：

```rust
// helix-view/src/document.rs
pub fn append_changes_to_history(&mut self, view: &mut View) {
    if self.changes.is_empty() { return; }
    // 取出累积的变更，重置为空的 ChangeSet
    let changes = std::mem::replace(&mut self.changes, ChangeSet::new(...));
    let transaction = Transaction::from(changes).with_selection(self.selection(view.id).clone());

    // 用 old_state（变更前的快照）作为撤销的"恢复点"
    let old_state = self.old_state.take().expect("no old_state available");

    let mut history = self.history.take();
    history.commit_revision(&transaction, &old_state);
    self.history.set(history);

    view.apply(&transaction, self); // 同步跳转列表
}
```

### History 的结构（helix-core/src/history.rs）

```rust
pub struct State {
    doc: Rope,                 // 文本快照（Rope 克隆是 O(1)）
    selection: Selection,      // 当时的光标
}
pub struct History {
    revisions: Vec<Rev>,       // 撤销栈
    ...
    // Rev = { state: State（变更前）, transaction: Transaction（正向变更） }
}
```

- `commit_revision(&transaction, &original)` 把 `(变更前状态, 正向事务)` 压入撤销栈；
- `undo()` 弹出最近的 Rev，返回其 `transaction.invert(original)` —— 反向事务；
- `redo()` 从重做栈弹回正向事务。

**Insert 模式与 normal 模式的差异**：insert 模式下多次按键产生的多个 Transaction 通过
`compose` 累积在 `self.changes` 里，直到退出 insert（或非 insert 按键处理）才一次性提交
历史——所以"进入 insert 输入 10 个字符，退回到 normal"只产生**一条** undo 记录。

## 6. undo / redo 的完整流程

`undo`（`helix-view/src/document.rs::undo`）：

```
Document::undo(view)
 ├─ append_changes_to_history(view)      // 先把未提交的编辑提交（否则丢变更）
 ├─ history.undo() → Option<&Transaction> // 弹出最近修订，构造反向事务
 ├─ self.apply_impl(txn, view.id, true)  // 应用反向事务（走第 3 节的完整 apply_impl）
 └─ view.sync_changes(self)              // 同步跳转列表选择
```

## 7. 与 LSP 的联动

`apply_impl` 中的 `helix_event::dispatch(DocumentDidChange { ... })` 是事件总线广播
（`helix-event` crate）。`helix-view/src/handlers/lsp.rs` 订阅该事件，把变更转换为
`textDocument/didChange` 通知发给语言服务器。注意 `apply_temporary`（`emit_lsp_notification = false`）
用于**不通知服务器**的临时事务（如补全预览）。

## 8. 小结

```
命令函数（insert_char）
 └─ Transaction::change_by_selection(doc, selection, f)   // 每光标一个 Change
     └─ Document::apply → apply_inner → apply_impl
         ├─ ChangeSet::apply(&mut text)        // 改 Rope
         ├─ Selection::map / map_pos           // 光标坐标迁移
         ├─ syntax.update                      // 增量语法解析
         ├─ diagnostics 位置迁移
         ├─ dispatch(DocumentDidChange)        // → LSP didChange
         └─ compose 进 self.changes            // 等待提交
             └─ append_changes_to_history      // 非 insert 模式，按键后
                 └─ History::commit_revision(transaction, old_state)
                     └─ undo: transaction.invert(original) → apply
```

下一步：[05-tutorial-render.md](./05-tutorial-render.md) 看文本如何画到终端上。
