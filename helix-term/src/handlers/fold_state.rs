//! 折叠态跨会话持久化（计划 P3）。
//!
//! 存储：`~/.local/state/helix/fold-state.json`，形如
//! `{"<绝对路径>": [[start_line, end_line], ...]}`（行号 0-based，`end` 不含）——
//! 与 `term-state.json` 同一个目录、同一套"小 JSON 直接读写"的做法。
//!
//! 为什么存**行号**而不是 char_idx：跨会话时文档内容可能已经变了，
//! 行号是唯一还能对上的锚点（文件被大改时折叠会落到别处，可接受；
//! 行数不够时 `Folds::from_lines` 会 clamp 或丢弃）。
//!
//! 开关：`[editor] fold-persist`（默认开；测试关掉，否则会写真实 HOME）。

use std::path::PathBuf;

use helix_view::{DocumentId, Editor};

fn state_path() -> PathBuf {
    helix_stdx::path::home_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".local/state/helix")
        .join("fold-state.json")
}

fn load_all() -> serde_json::Map<String, serde_json::Value> {
    let content = std::fs::read_to_string(state_path()).unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn save_all(map: &serde_json::Map<String, serde_json::Value>) {
    let path = state_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(map) {
        if let Err(err) = std::fs::write(&path, json) {
            log::warn!("fold-state: 写入 {path:?} 失败: {err}");
        }
    }
}

/// 把当前文档的折叠写进状态文件（命令层每次折叠变更后调用）。
pub fn persist_current(editor: &Editor) {
    if !editor.config().fold_persist {
        return;
    }
    let Some(doc_id) = editor.tree.try_get(editor.tree.focus).map(|view| view.doc) else {
        return;
    };
    persist(editor, doc_id);
}

pub fn persist(editor: &Editor, doc_id: DocumentId) {
    let Some(doc) = editor.document(doc_id) else {
        return;
    };
    let Some(path) = doc.path() else {
        return; // 无名 buffer 没有稳定锚点
    };
    let folds: Vec<serde_json::Value> = encode_folds(doc)
        .into_iter()
        .map(|pair| serde_json::json!(pair))
        .collect();

    let mut all = load_all();
    let key = path.to_string_lossy().into_owned();
    if folds.is_empty() {
        all.remove(&key);
    } else {
        all.insert(key, serde_json::Value::Array(folds));
    }
    save_all(&all);
}

/// 打开文档时恢复折叠（行号对齐；行数不够的自然被丢弃）。
pub fn restore(editor: &mut Editor, doc_id: DocumentId) {
    if !editor.config().fold_persist {
        return;
    }
    let all = load_all();
    if all.is_empty() {
        return;
    }
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };
    let Some(path) = doc.path() else {
        return;
    };
    let key = path.to_string_lossy().into_owned();
    let Some(entries) = all.get(&key).and_then(|v| v.as_array()) else {
        return;
    };

    let pairs: Vec<[usize; 2]> = entries
        .iter()
        .filter_map(|entry| {
            let pair = entry.as_array()?;
            let start = pair.first()?.as_u64()? as usize;
            let end = pair.get(1)?.as_u64()? as usize;
            Some([start, end])
        })
        .collect();
    let restored = decode_folds(doc.text().slice(..), &pairs);
    if restored.is_empty() {
        return;
    }
    for fold in restored {
        doc.folds.add(fold);
    }
}

pub(super) fn register_hooks(_handlers: &helix_view::handlers::Handlers) {
    use helix_event::register_hook;
    use helix_view::events::DocumentDidOpen;

    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        let doc_id = event.doc;
        restore(event.editor, doc_id);
        Ok(())
    });
}

/// 文档的折叠 → 行号对（写盘形状）。纯函数，便于单测。
pub(crate) fn encode_folds(doc: &helix_view::Document) -> Vec<[usize; 2]> {
    let text = doc.text().slice(..);
    doc.folds
        .all()
        .iter()
        .map(|fold| [text.char_to_line(fold.start), text.char_to_line(fold.end)])
        .collect()
}

/// 行号对 → 折叠（读盘形状）。行数不够 / 只剩一行的自然被 `Folds::from_lines` 丢掉。
pub(crate) fn decode_folds(
    text: helix_core::RopeSlice<'_>,
    pairs: &[[usize; 2]],
) -> Vec<helix_view::fold::Fold> {
    pairs
        .iter()
        .filter_map(|[start, end]| helix_view::fold::Folds::from_lines(text, *start..*end))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_core::Rope;

    #[test]
    fn encode_decode_roundtrip() {
        let rope = Rope::from("fn a() {\n    x();\n    y();\n}\n");
        let text = rope.slice(..);
        let folds = decode_folds(text, &[[0, 4]]);
        assert_eq!(folds.len(), 1);
        assert_eq!(
            (
                text.char_to_line(folds[0].start),
                text.char_to_line(folds[0].end)
            ),
            (0, 4)
        );
    }

    #[test]
    fn decode_drops_degenerate_entries() {
        let rope = Rope::from("a\nb\nc\n");
        let text = rope.slice(..);
        assert!(decode_folds(text, &[[0, 1]]).is_empty(), "只藏 0 行 ⇒ 丢");
        assert!(decode_folds(text, &[[2, 2]]).is_empty(), "空区间 ⇒ 丢");
        // 越界 end 会被 clamp 到文件末尾 ⇒ 仍然是个合法折叠（文件变短了就该这样）
        assert_eq!(decode_folds(text, &[[0, 99]]).len(), 1, "end clamp 到末尾");
        assert_eq!(decode_folds(text, &[[0, 3]]).len(), 1, "0..3 合法");
        // 起点越界 ⇒ clamp 到最后一行 ⇒ 没有可藏的
        assert!(decode_folds(text, &[[99, 100]]).is_empty(), "起点越界 ⇒ 丢");
    }
}
