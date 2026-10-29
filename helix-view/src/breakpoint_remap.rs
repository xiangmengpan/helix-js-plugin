//! 编辑后把断点的**行号跟着挪**（`commands/dap.rs` 那条 `TODO: need to map breakpoints over edits`）。
//!
//! 为什么不能直接在 `DocumentDidChange` 的钩子里做：
//! 那个事件只带 `(doc, old_text, changes, ghost_transaction)`，**没有 `&mut Editor`**
//! （`helix-view/src/events.rs`），而断点表在 `Editor.breakpoints` 上。给事件加 `editor`
//! 字段会波及所有已注册的钩子；所以这里改成**排队**：
//!
//! 1. 钩子（`helix-term/src/handlers/dap_breakpoints.rs`）只管把
//!    `(path, old_text, new_text, changes)` 塞进线程本地的队列（Rope 是持久化树，clone 很便宜）；
//! 2. 事件循环里本来就每拍调一次的 `commands::dap::dispatch_dap_intents` 把它排空，
//!    拿着 `&mut editor.breakpoints` 做精确重映射。
//!
//! 行号重映射用 `ChangeSet::map_pos`：断点行的行首偏移按 `Assoc::After` 映射到新文本，
//! 再换回行号。被整行删掉的断点会被"顺移"到相邻行（而不是凭空消失）—— 不会因为一次误删
//! 就丢掉用户配的条件/日志。

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use helix_core::{Assoc, ChangeSet, Rope};
use helix_dap::breakpoints::BreakpointStore;

/// 一批待回放的编辑。
struct PendingEdit {
    path: PathBuf,
    old_text: Rope,
    new_text: Rope,
    changes: ChangeSet,
}

thread_local! {
    static PENDING: RefCell<Vec<PendingEdit>> = const { RefCell::new(Vec::new()) };
}

/// 单次最多排队多少批（防止连续编辑时队列无限长；丢最旧的那批，代价只是少挪一次断点）。
const MAX_PENDING: usize = 256;

/// 钩子入口：记一批编辑（**不碰 Editor**）。
pub fn queue_edit(path: &Path, old_text: &Rope, new_text: &Rope, changes: &ChangeSet) {
    if changes.is_empty() {
        return;
    }
    PENDING.with(|q| {
        let mut q = q.borrow_mut();
        if q.len() >= MAX_PENDING {
            q.remove(0);
        }
        q.push(PendingEdit {
            path: path.to_path_buf(),
            old_text: old_text.clone(),
            new_text: new_text.clone(),
            changes: changes.clone(),
        });
    });
}

/// 事件循环里排空队列：回放所有待处理编辑，返回**需要重新推给 adapter** 的文件。
///
/// 无会话时也照常回放（本地断点表是唯一真相；要不要推由调用方决定）。
pub fn apply_pending(store: &mut BreakpointStore) -> Vec<PathBuf> {
    let items = PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()));
    let mut touched: Vec<PathBuf> = Vec::new();
    for edit in items {
        let breakpoints = store.for_path(&edit.path);
        if breakpoints.is_empty() {
            continue; // 这个文件没有断点：最常见的路径，直接跳过
        }
        let mut moved = false;
        for bp in breakpoints {
            let new_line = remap_line(&edit.old_text, &edit.new_text, &edit.changes, bp.line);
            if new_line == bp.line {
                continue;
            }
            store.remove(&edit.path, bp.line);
            let mut next = bp;
            next.line = new_line;
            // 行变了 ⇒ adapter 那边的确认失效，等下一次 setBreakpoints 回填
            next.id = None;
            next.verified = false;
            next.message = None;
            store.set(edit.path.clone(), next);
            moved = true;
        }
        if moved {
            log::debug!(
                "dap-breakpoints: {} 的断点跟着编辑挪了行",
                edit.path.display()
            );
        }
        if moved && !touched.contains(&edit.path) {
            touched.push(edit.path.clone());
        }
    }
    touched
}

/// 行号重映射（纯函数，便于单测）：行首偏移 → `map_pos` → 新行号，越界一律夹回合法范围。
pub fn remap_line(old_text: &Rope, new_text: &Rope, changes: &ChangeSet, line: usize) -> usize {
    let old_line = line.min(old_text.len_lines().saturating_sub(1));
    let old_offset = old_text.line_to_char(old_line);
    let new_chars = new_text.len_chars();
    let new_offset = if new_chars == 0 {
        0
    } else {
        changes.map_pos(old_offset, Assoc::After).min(new_chars - 1)
    };
    new_text.char_to_line(new_offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_core::ChangeSet;
    use helix_dap::breakpoints::{Breakpoint, BreakpointStore};

    /// 按一组 `(from, to, insert)` 造出 `(old, new, changes)`（单次编辑场景足够）。
    fn edit(old: &str, from: usize, to: usize, insert: &str) -> (Rope, Rope, ChangeSet) {
        let old = Rope::from(old);
        let changes = ChangeSet::from_changes(
            &old,
            std::iter::once((from, to, Some(insert.to_string().into()))),
        );
        let mut new = old.clone();
        changes.apply(&mut new);
        (old, new, changes)
    }

    #[test]
    fn insert_line_above_shifts_breakpoint_down() {
        // 在文件头插一整行
        let (old, new, changes) = edit("a\nb\nc\n", 0, 0, "x\n");
        assert_eq!(remap_line(&old, &new, &changes, 0), 1);
        assert_eq!(remap_line(&old, &new, &changes, 1), 2);
        assert_eq!(remap_line(&old, &new, &changes, 2), 3);
    }

    #[test]
    fn delete_line_above_shifts_breakpoint_up() {
        // 删掉第一行 "a\n"
        let (old, new, changes) = edit("a\nb\nc\n", 0, 2, "");
        assert_eq!(remap_line(&old, &new, &changes, 0), 0, "夹到新文本第一行");
        assert_eq!(remap_line(&old, &new, &changes, 1), 0);
        assert_eq!(remap_line(&old, &new, &changes, 2), 1);
    }

    #[test]
    fn edit_below_does_not_move_breakpoint() {
        let (old, new, changes) = edit("a\nb\nc\n", 0, 1, "aa");
        assert_eq!(remap_line(&old, &new, &changes, 1), 1);
        assert_eq!(remap_line(&old, &new, &changes, 2), 2);
    }

    #[test]
    fn editing_the_breakpoint_line_keeps_it_there() {
        let (old, new, changes) = edit("fn main() {\n  x\n}\n", 15, 16, "y");
        assert_eq!(remap_line(&old, &new, &changes, 1), 1);
    }

    #[test]
    fn apply_pending_moves_and_invalidates_verified() {
        let (old, new, changes) = edit("a\nb\nc\n", 0, 0, "x\n");
        let mut store = BreakpointStore::new();
        let mut bp = Breakpoint::new(1);
        bp.condition = Some("i>1".into());
        bp.verified = true;
        bp.id = Some(7);
        store.set("/tmp/a.rs", bp);

        queue_edit(Path::new("/tmp/a.rs"), &old, &new, &changes);
        let touched = apply_pending(&mut store);
        assert_eq!(touched, vec![PathBuf::from("/tmp/a.rs")]);

        let moved = store.for_path(Path::new("/tmp/a.rs"));
        assert_eq!(moved.len(), 1, "断点没丢");
        assert_eq!(moved[0].line, 2, "跟着下移一行");
        assert_eq!(moved[0].condition.as_deref(), Some("i>1"), "条件不丢");
        assert!(!moved[0].verified, "行变了 ⇒ 确认失效");
        assert_eq!(moved[0].id, None);
    }

    #[test]
    fn edits_stacking_before_a_tick_are_applied_in_order() {
        let (old1, new1, ch1) = edit("a\nb\n", 0, 0, "x\n");
        let (old2, new2, ch2) = edit("x\na\nb\n", 0, 0, "y\n");
        let mut store = BreakpointStore::new();
        store.set("/tmp/s.rs", Breakpoint::new(0)); // 原第 0 行 "a"

        queue_edit(Path::new("/tmp/s.rs"), &old1, &new1, &ch1);
        queue_edit(Path::new("/tmp/s.rs"), &old2, &new2, &ch2);
        apply_pending(&mut store);
        assert_eq!(
            store.for_path(Path::new("/tmp/s.rs"))[0].line,
            2,
            "两次插行叠加：原 0 行 → 2 行"
        );
    }

    #[test]
    fn no_breakpoint_means_no_touch() {
        let (old, new, changes) = edit("a\nb\n", 0, 0, "x\n");
        let mut store = BreakpointStore::new();
        queue_edit(Path::new("/tmp/other.rs"), &old, &new, &changes);
        assert!(apply_pending(&mut store).is_empty());
        assert!(store.is_empty());
    }
}
