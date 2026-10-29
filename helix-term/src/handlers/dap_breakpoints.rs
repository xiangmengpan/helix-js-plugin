//! 编辑 → 断点行号重映射的**入队**钩子。
//!
//! 真正挪行号的地方在 `helix_view::breakpoint_remap::apply_pending`（事件循环里排空队列，
//! 那里才有 `&mut Editor`）。这里只负责把 `DocumentDidChange` 的 `(old_text, changes)` 记下来。
//!
//! 为什么要这个钩子：DAP 的断点按**行号**存在适配器与本地表里，文本一改行号就漂 ——
//! 上游 helix 的 `dap.rs` 里那条 `TODO: need to map breakpoints over edits` 就是它。

use helix_event::register_hook;
use helix_view::events::DocumentDidChange;

pub(super) fn register_hooks(_handlers: &super::Handlers) {
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        // ghost transaction（例如只改选择的空事务）没有文本变化，跳过
        if event.ghost_transaction || event.changes.is_empty() {
            return Ok(());
        }
        let Some(path) = event.doc.path() else {
            return Ok(());
        };
        helix_view::breakpoint_remap::queue_edit(
            path,
            event.old_text,
            event.doc.text(),
            event.changes,
        );
        Ok(())
    });
}
