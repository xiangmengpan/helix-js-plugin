//! 光标/选区变化检测(插件 cursor-move / selection-change 事件的数据源)。
//! 检测点:渲染帧。主代理在 render 里调用 `cursor_change`(帧级节流,见
//! docs/superpowers/specs/2026-08-15-cursor-move-design.md)与 `selection_count`,
//! 变化后经 helix-js::cursor::emit_* 通知 JS。

use helix_view::Editor;

/// 取当前焦点视图所在文档的 primary cursor(行, 列),与 `last` 比较:
/// 变化 → 更新 `last` 并返回 Some((row, col));未变化 → None(零开销,调用方不 emit)。
/// 终端/无文档焦点时返回 None 且不改动 `last`。
pub fn cursor_change(editor: &Editor, last: &mut Option<(usize, usize)>) -> Option<(usize, usize)> {
    let view_id = editor.tree.focus;
    let doc_id = editor.tree.try_get(view_id).map(|view| view.doc)?;
    let doc = editor.documents.get(&doc_id)?;
    let pos = doc
        .selection(view_id)
        .primary()
        .cursor(doc.text().slice(..));
    let row = doc.text().char_to_line(pos);
    let col = pos - doc.text().line_to_char(row);
    let cur = (row, col);
    if *last == Some(cur) {
        None
    } else {
        *last = Some(cur);
        Some(cur)
    }
}

/// 当前焦点视图所在文档的选区数量。
pub fn selection_count(editor: &Editor) -> usize {
    let view_id = editor.tree.focus;
    let Some(doc_id) = editor.tree.try_get(view_id).map(|view| view.doc) else {
        return 0;
    };
    let Some(doc) = editor.documents.get(&doc_id) else {
        return 0;
    };
    doc.selection(view_id).len()
}
