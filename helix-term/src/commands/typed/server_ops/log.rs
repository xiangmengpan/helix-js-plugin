//! `:server log [name] [行数]` —— 把这个 server 相关的日志尾部开成 buffer。
//!
//! 为什么不新开文件：server 的 stderr 已经进了 `helix.log`（LSP/DAP 的 transport
//! 与 install 的进度都在里面），过滤比再写一份日志简单，也不会泄漏到别处。

use super::*;

const DEFAULT_LINES: usize = 300;

pub(super) fn run(
    editor: &mut Editor,
    name: Option<&str>,
    arg2: Option<&str>,
) -> anyhow::Result<()> {
    let path = helix_loader::log_file();
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow!("读日志 {} 失败: {e}", path.display()))?;
    let want = name.map(str::to_string);
    let limit = arg2
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_LINES);
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|l| match &want {
            Some(n) => l.contains(n.as_str()),
            None => true,
        })
        .collect();
    if lines.is_empty() {
        return Err(anyhow!(
            "{}里没有{}相关内容",
            path.display(),
            want.as_deref()
                .map(|n| format!(" '{n}' "))
                .unwrap_or_default()
        ));
    }
    let start = lines.len().saturating_sub(limit);
    lines = lines.split_off(start);
    let mut body = String::new();
    body.push_str(&format!(
        "// {} 条{}（来自 {}）—— `:bc` 关掉\n\n",
        lines.len(),
        want.as_deref()
            .map(|n| format!(" '{n}' 相关日志"))
            .unwrap_or_default(),
        path.display()
    ));
    body.push_str(&lines.join("\n"));
    body.push('\n');

    let reuse = editor
        .server_log_doc
        .filter(|id| editor.documents.contains_key(id));
    match reuse {
        Some(id) => editor.switch(id, helix_view::editor::Action::Replace),
        None => {
            editor.new_file(helix_view::editor::Action::Replace);
            let (_, doc) = helix_view::current!(editor);
            editor.server_log_doc = Some(doc.id());
        }
    }
    let (view, doc) = helix_view::current!(editor);
    let transaction = helix_core::Transaction::change(
        doc.text(),
        std::iter::once((0, 0, Some(body.as_str().into()))),
    );
    doc.apply(&transaction, view.id);
    doc.mark_saved_now();
    doc.set_selection(view.id, helix_core::Selection::point(0));
    Ok(())
}
