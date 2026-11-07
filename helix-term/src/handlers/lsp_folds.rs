//! LSP 折叠范围（`textDocument/foldingRange`，计划 P2）。
//!
//! 时机：**文档打开**时请求一次（与 document_links / document_colors 同款），
//! `fold-method = "lsp"` 的 `zM` 发现缓存为空时再补请求一次（见 `request_lsp_folds`），
//! 结果缓存进 `Document::lsp_fold_ranges`。
//!
//! 为什么不做"编辑即请求"：`foldingRange` 没有 push 能力，而 LSP 给的是**行号区间**，
//! 小幅编辑下仍然可用（缩进/结构没变），保存时刷新就够；按 `didChange` 频繁请求会
//! 把服务器打满，也会让 `zM` 一直读到"刚被清空"的缓存。
//!
//! 只在 `[editor] fold-method = "lsp"` 时请求，避免给不用折叠的用户白跑一趟。

use std::collections::HashSet;

use futures_util::stream::{FuturesUnordered, StreamExt};
use helix_event::{cancelable_future, register_hook};
use helix_lsp::lsp;
use helix_view::{DocumentId, Editor};

use helix_view::editor::FoldMethod;
use helix_view::events::{ConfigDidChange, DocumentDidOpen, LanguageServerInitialized};
use helix_view::handlers::Handlers;

use crate::job;

/// 发起一次折叠范围请求（`zM` 发现缓存为空时也会调它）。
pub fn request_lsp_folds(editor: &mut Editor, doc_id: DocumentId) {
    if !matches!(editor.config().fold_method, FoldMethod::Lsp) {
        return;
    }
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };

    let cancel = doc.pull_diagnostic_controller.restart();
    let identifier = doc.identifier();

    let futures: FuturesUnordered<_> = {
        let mut seen: HashSet<_> = HashSet::new();
        doc.language_servers()
            .filter(|server| seen.insert(server.id()))
            .filter_map(|server| server.text_document_folding_range(identifier.clone()))
            .collect()
    };
    if futures.is_empty() {
        return;
    }

    tokio::spawn(async move {
        let mut all = Vec::new();
        let mut futures = futures;
        loop {
            match cancelable_future(futures.next(), &cancel).await {
                // LSP 结果是 `Option<Vec<FoldingRange>>`（有些服务器回 null）
                Some(Some(Ok(Some(ranges)))) => all.extend(ranges),
                Some(Some(Ok(None))) => {}
                Some(Some(Err(err))) => log::error!("foldingRange request failed: {err}"),
                Some(None) => break,
                None => return, // 被取消（文档关闭/新请求覆盖）
            }
        }
        job::dispatch(move |editor, _, mut _js| attach_fold_ranges(editor, doc_id, all)).await;
    });
}

fn attach_fold_ranges(editor: &mut Editor, doc_id: DocumentId, mut ranges: Vec<lsp::FoldingRange>) {
    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return;
    };
    // 同一份区间可能被多个服务器重复给出 ⇒ 去重后按起点排序
    ranges.sort_by_key(|range| (range.start_line, range.end_line));
    ranges.dedup_by_key(|range| (range.start_line, range.end_line));
    let changed = ranges.len() != doc.lsp_fold_ranges.len()
        || ranges
            .iter()
            .zip(&doc.lsp_fold_ranges)
            .any(|(a, b)| (a.start_line, a.end_line) != (b.start_line, b.end_line));
    doc.lsp_fold_ranges = ranges;
    if changed {
        helix_event::request_redraw();
    }
}

pub(super) fn register_hooks(handlers: &Handlers) {
    let _ = handlers;
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        let doc_id = event.doc;
        request_lsp_folds(event.editor, doc_id);
        Ok(())
    });

    // `[editor] fold-method` 从别的值切到 `lsp` ⇒ 给所有已打开文档补一次请求
    // （否则用户必须重开文件才生效；`:config-reload` 走的就是这条）。
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        if !matches!(event.old.fold_method, FoldMethod::Lsp)
            && matches!(event.new.fold_method, FoldMethod::Lsp)
        {
            let docs: Vec<DocumentId> = event.editor.documents().map(|doc| doc.id()).collect();
            for doc_id in docs {
                request_lsp_folds(event.editor, doc_id);
            }
        }
        Ok(())
    });

    // 文档往往在 LSP 握手**之前**就打开了（那时 `doc.language_servers()` 还空着）
    // ⇒ 服务器初始化完成后给它的所有文档补一次请求（与 did_open 那条路同款）。
    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        let server_id = event.server_id;
        let docs: Vec<DocumentId> = event
            .editor
            .documents()
            .filter(|doc| doc.supports_language_server(server_id))
            .map(|doc| doc.id())
            .collect();
        for doc_id in docs {
            request_lsp_folds(event.editor, doc_id);
        }
        Ok(())
    });
}
