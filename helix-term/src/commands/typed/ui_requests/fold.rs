//! 折叠类请求（`helix.fold.*`，计划 P3）。
//!
//! 插件给的是**行号**（0-based，与 `helix.fold.list()` 回来的形状一致），
//! 这里换算成世界坐标的 char_idx 再落到 `Document.folds` —— 与键位 `z f/o/a/M/R`
//! 共用同一份实现（`commands::fold_*_impl` / `fold_candidates`）。

use super::*;
use helix_js::FoldLineReq;

/// 行号 → 该行行首的 char_idx（越界 clamp 到最后一行）。
fn line_to_char(doc: &helix_view::Document, line: usize) -> usize {
    let text = doc.text().slice(..);
    let last = text.len_lines().saturating_sub(1);
    text.line_to_char(line.min(last))
}

fn status(editor: &mut helix_view::Editor, msg: impl Into<String>) {
    editor.set_status(msg.into());
}

pub(crate) fn handle_fold_open(FoldLineReq { line }: FoldLineReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |editor, _compositor, mut _js| {
        let (_view, doc) = current!(editor);
        let char_idx = line_to_char(doc, line);
        let opened = doc.folds.remove_at(char_idx);
        status(
            editor,
            if opened {
                "fold opened"
            } else {
                "fold.open: no fold on that line"
            },
        );
    });
    Ok(())
}

pub(crate) fn handle_fold_close(FoldLineReq { line }: FoldLineReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |editor, _compositor, mut _js| {
        let (fold_method, fold_min_lines) = {
            let config = editor.config();
            (config.fold_method, config.fold_min_lines)
        };
        let loader = editor.syn_loader.load_full();
        let (fold, hidden) = {
            let (_view, doc) = current!(editor);
            let char_idx = line_to_char(doc, line);
            let candidate =
                crate::commands::fold_candidates(doc, &loader, fold_method, fold_min_lines)
                    .0
                    .into_iter()
                    .find(|fold| fold.contains(char_idx));
            match candidate {
                Some(fold) => {
                    let hidden = fold.hidden_lines(doc.text().slice(..));
                    doc.folds.add(fold);
                    (Some(fold), hidden)
                }
                None => (None, 0),
            }
        };
        status(
            editor,
            match fold {
                Some(_) => format!("fold closed ({hidden} lines)"),
                None => "fold.close: nothing to fold on that line".to_string(),
            },
        );
    });
    Ok(())
}

pub(crate) fn handle_fold_toggle(FoldLineReq { line }: FoldLineReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |editor, _compositor, mut _js| {
        let opened = {
            let (_view, doc) = current!(editor);
            doc.folds.remove_at(line_to_char(doc, line))
        };
        if opened {
            status(editor, "fold opened");
            return;
        }
        // 没折着 ⇒ 按当前 fold-method 折上（与 `za` 同一条路）
        let (fold_method, fold_min_lines) = {
            let config = editor.config();
            (config.fold_method, config.fold_min_lines)
        };
        let loader = editor.syn_loader.load_full();
        let hidden = {
            let (_view, doc) = current!(editor);
            let char_idx = line_to_char(doc, line);
            crate::commands::fold_candidates(doc, &loader, fold_method, fold_min_lines)
                .0
                .into_iter()
                .find(|fold| fold.contains(char_idx))
                .map(|fold| {
                    let hidden = fold.hidden_lines(doc.text().slice(..));
                    doc.folds.add(fold);
                    hidden
                })
        };
        status(
            editor,
            match hidden {
                Some(hidden) => format!("fold closed ({hidden} lines)"),
                None => "fold.toggle: nothing to fold on that line".to_string(),
            },
        );
    });
    Ok(())
}

pub(crate) fn handle_fold_open_all() -> anyhow::Result<()> {
    job::dispatch_blocking(move |editor, _compositor, mut _js| {
        crate::commands::fold_open_all_impl(editor);
    });
    Ok(())
}

pub(crate) fn handle_fold_close_all() -> anyhow::Result<()> {
    job::dispatch_blocking(move |editor, _compositor, mut _js| {
        crate::commands::fold_close_all_impl(editor);
    });
    Ok(())
}

/// `layout.restore` 里带的折叠态：`{"<绝对路径>": [[start_line, end_line], ...]}`。
///
/// 只对**当前已打开**的文档生效（文档由布局恢复/插件自己打开，这里不主动开文件）；
/// 文本变短时 `Folds::from_lines` 会 clamp/丢弃 —— 与 `fold-state.json` 同一套语义。
pub(crate) fn apply_layout_folds(editor: &mut helix_view::Editor, dump: &serde_json::Value) {
    let Some(map) = dump.get("folds").and_then(|v| v.as_object()) else {
        return;
    };
    let mut applied = 0usize;
    for doc in editor.documents_mut() {
        let Some(path) = doc.path() else {
            continue;
        };
        let key = path.to_string_lossy();
        let Some(entries) = map.get(key.as_ref()).and_then(|v| v.as_array()) else {
            continue;
        };
        let text = doc.text().slice(..);
        let restored: Vec<_> = entries
            .iter()
            .filter_map(|entry| entry.as_array())
            .filter_map(|pair| {
                let start = pair.first()?.as_u64()? as usize;
                let end = pair.get(1)?.as_u64()? as usize;
                helix_view::fold::Folds::from_lines(text, start..end)
            })
            .collect();
        if restored.is_empty() {
            continue;
        }
        doc.folds.clear();
        for fold in restored {
            doc.folds.add(fold);
        }
        applied += 1;
    }
    if applied > 0 {
        editor.set_status(format!("layout: restored folds for {applied} buffer(s)"));
        helix_event::request_redraw();
    }
}
