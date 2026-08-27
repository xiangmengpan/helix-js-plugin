# doc-change 事件粒度(new/old_range 合并范围)实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** `doc-change` 事件参数带 `changes`(防抖窗口内变更的合并 old/new range),供自动格式化/补全联动/脏区标记使用。

**架构：** `Document::apply_inner`(统一入口)从 transaction changeset 记录变更 range 到 `pending_doc_changes` 字段;事件触发(现状 idle 防抖 + revision 前进)时取走并合并为包围范围,序列化进事件参数(挂在 doc 对象上)。old_range = 变更时旧文本坐标,new_range = changeset 映射后的新坐标。

**技术栈：** helix-core transaction(ChangeSet::changes_iter / map_pos)、helix-view Document、boa 序列化。

**规格：** `docs/superpowers/specs/2026-08-26-js-docchange-range-design.md`(已批准)

---

### 任务 1：helix-view — apply_inner 记录变更 range

**文件：**
- 修改：`helix-view/src/document.rs`(Document 加 `pending_doc_changes` 字段 + apply_inner 记录)

- [ ] **步骤 1：编写失败的测试**(`helix-view/src/document.rs` 测试模块或现有测试,断言 apply 后 pending 记录)

```rust
#[test]
fn pending_changes_recorded_on_apply() {
    let mut doc = Document::default(); // 或现有测试构造方式
    let text = doc.text();
    let txn = Transaction::change(
        text,
        vec![Change::from((5usize, 5usize, "abc".into()))],
    );
    doc.apply(&txn, ViewId(0));
    let changes = doc.pending_doc_changes(); // 新增访问器(测试用)
    assert_eq!(changes.len(), 1);
    // old = (5, 5)(插入点),new = (5, 8)(插入后)
    assert_eq!(changes[0].0, (5, 5));
    assert_eq!(changes[0].1, (5, 8));
}
```

- [ ] **步骤 2：运行确认失败**

运行：`cargo test -p helix-view pending_changes_recorded_on_apply`
预期：FAIL(字段/访问器不存在)。

- [ ] **步骤 3：实现**

`document.rs` Document 结构(144 行 selections 附近)加字段:
```rust
/// 自上次 doc-change 事件以来 apply 的变更(旧坐标, 新坐标);事件触发时取走
pub(crate) pending_doc_changes: Vec<((usize, usize), (usize, usize))>,
```
构造(742 行附近)初始化空。

`apply_inner` 里(找到 `self.apply_impl` 或文本更新处,apply 成功分支)记录:
```rust
let changeset = transaction.changes();
for (start, end, _text) in changeset.changes_iter() {
    use helix_core::Assoc;
    let new_start = changeset.map_pos(start, Assoc::Before);
    let new_end = changeset.map_pos(end, Assoc::After);
    self.pending_doc_changes.push(((start, end), (new_start, new_end)));
}
```
> 注:apply_inner 的实际结构与记录插入点以实现者看到的代码为准(apply_temporary 不记录——预览事务不算变更;若 apply_inner 是共用实现,加 bool 参数区分)。

访问器(测试/helix-term 用):
```rust
/// 取走并清空 pending 变更(事件触发时调用)
pub(crate) fn take_pending_changes(&mut self) -> Vec<((usize, usize), (usize, usize))> {
    std::mem::take(&mut self.pending_doc_changes)
}
```

- [ ] **步骤 4：运行确认通过**

运行：`cargo test -p helix-view`
预期：pending 测试 + 既有测试全 PASS。

- [ ] **步骤 5：Commit**

```bash
git add helix-view/src/document.rs
git commit -m "feat(view): Document 记录 pending 变更 range(apply_inner, old/new 坐标)"
```

### 任务 2：helix-term — 事件触发时合并序列化

**文件：**
- 修改：`helix-term/src/application.rs`(handle_idle_timeout 783 行,doc-change 触发分支)
- 修改：`helix-term/src/commands/typed.rs`(`emit_plugin_event` 或 doc_to_js 注入 changes)

- [ ] **步骤 1：编写失败的测试**(integration,`helix-term/tests/test/` 新建 `plugin_docchange.rs` 或并入现有)

模拟:打字(或插件 doc.insert)→ 触发 doc-change → 断言事件参数含 changes 且 old/new range 正确。参照现有 `plugin_input_edit_and_nav` 的 test_key_sequences 模式。

- [ ] **步骤 2：运行确认失败**

运行：`cargo test -p helix-term --features integration --test integration plugin_docchange`
预期：FAIL(事件参数无 changes 字段)。

- [ ] **步骤 3：实现**

`application.rs` 783 行分支:触发前取 pending:
```rust
if changed {
    let (_, doc) = helix_view::current_ref!(self.editor);
    let changes = doc.take_pending_changes(); // 需 pub(crate) 可见
    crate::commands::typed::emit_plugin_event(&mut self.editor, "doc-change", None);
    // 序列化 changes 到事件参数(见 typed.rs)
}
```
> 注:借用顺序——current_ref 借 self.editor,与后续 emit_plugin_event(&mut self.editor) 冲突。先取 changes 再释放借用(drop),再 emit。实现时处理借用。

`typed.rs` `emit_plugin_event`:doc_to_js 构造 doc 对象后,若 name == "doc-change",注入 `changes` 属性(合并后的包围范围数组):
```rust
// changes 由调用方传入(extra 扩展或新参数)
// 合并:old = min(old_start)..max(old_end),new = min(new_start)..max(new_end)
```
合并逻辑放 helix-js 序列化侧或 term 侧(实现者选择,规格 3.3 语义为准):
- 单条:直接该条
- 多条:old = (min old_start, max old_end),new = (min new_start, max new_end)

- [ ] **步骤 4：运行确认通过**

运行：`cargo test -p helix-term --features integration --test integration plugin_docchange` + `cargo clippy -p helix-term -p helix-view --all-targets` 零警告
预期：PASS。

- [ ] **步骤 5：Commit**

```bash
git add helix-term/src/application.rs helix-term/src/commands/typed.rs helix-term/tests/test/plugin_docchange.rs helix-term/tests/integration.rs
git commit -m "feat(term): doc-change 事件带合并 old/new range"
```
