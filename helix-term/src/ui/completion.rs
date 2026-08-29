use crate::handlers::completion::{LspCompletionItem, SnippetCompletionItem};
use crate::ui::{menu, Markdown, Menu, Popup, PromptEvent};
use crate::{
    compositor::{Component, Context, Event, EventResult},
    handlers::completion::{
        trigger_auto_completion, CompletionItem, CompletionResponse, ResolveHandler,
    },
};
use helix_core::snippets::{ActiveSnippet, RenderedSnippet, Snippet, SnippetRenderCtx};
use helix_core::{self as core, chars, fuzzy::MATCHER, Change, Rope, Selection, Transaction};
use helix_lsp::{lsp, util, OffsetEncoding};
use helix_view::{
    editor::CompleteAction,
    handlers::lsp::SignatureHelpInvoked,
    theme::{Color, Modifier, Style},
    ViewId,
};
use helix_view::{graphics::Rect, Document, Editor};
use nucleo::{
    pattern::{Atom, AtomKind, CaseMatching, Normalization},
    Config, Utf32Str,
};
use tui::text::Spans;
use tui::{buffer::Buffer as Surface, text::Span};

use std::cmp::Reverse;

impl menu::Item for CompletionItem {
    type Data = Style;

    fn format(&mut self, dir_style: &Self::Data) -> menu::Row<'static> {
        let deprecated = match self {
            CompletionItem::Lsp(LspCompletionItem { item, .. }) => {
                item.deprecated.unwrap_or_default()
                    || item
                        .tags
                        .as_ref()
                        .is_some_and(|tags| tags.contains(&lsp::CompletionItemTag::DEPRECATED))
            }
            CompletionItem::Snippet(_) => false,
            CompletionItem::Other(_) => false,
        };

        // label/detail 取 owned 值:钩子命中后需 &mut self 清 match_indices,不能持有对 self 的借用
        let label_text = match self {
            CompletionItem::Lsp(LspCompletionItem { item, .. }) => item.label.to_string(),
            CompletionItem::Snippet(SnippetCompletionItem { label, .. }) => label.to_string(),
            CompletionItem::Other(core::CompletionItem { label, .. }) => label.to_string(),
        };

        let (kind_spans, kind_num) = match self {
            CompletionItem::Lsp(LspCompletionItem { item, .. }) => match item.kind {
                Some(lsp::CompletionItemKind::TEXT) => ("text".into(), 1),
                Some(lsp::CompletionItemKind::METHOD) => ("method".into(), 2),
                Some(lsp::CompletionItemKind::FUNCTION) => ("function".into(), 3),
                Some(lsp::CompletionItemKind::CONSTRUCTOR) => ("constructor".into(), 4),
                Some(lsp::CompletionItemKind::FIELD) => ("field".into(), 5),
                Some(lsp::CompletionItemKind::VARIABLE) => ("variable".into(), 6),
                Some(lsp::CompletionItemKind::CLASS) => ("class".into(), 7),
                Some(lsp::CompletionItemKind::INTERFACE) => ("interface".into(), 8),
                Some(lsp::CompletionItemKind::MODULE) => ("module".into(), 9),
                Some(lsp::CompletionItemKind::PROPERTY) => ("property".into(), 10),
                Some(lsp::CompletionItemKind::UNIT) => ("unit".into(), 11),
                Some(lsp::CompletionItemKind::VALUE) => ("value".into(), 12),
                Some(lsp::CompletionItemKind::ENUM) => ("enum".into(), 13),
                Some(lsp::CompletionItemKind::KEYWORD) => ("keyword".into(), 14),
                Some(lsp::CompletionItemKind::SNIPPET) => ("snippet".into(), 15),
                Some(lsp::CompletionItemKind::COLOR) => (
                    item.documentation
                        .as_ref()
                        .and_then(|docs| {
                            let text = match docs {
                                lsp::Documentation::String(text) => text,
                                lsp::Documentation::MarkupContent(lsp::MarkupContent {
                                    value,
                                    ..
                                }) => value,
                            };
                            // Language servers which send Color completion items tend to include a 6
                            // digit hex code at the end for the color. The extra 1 digit is for the '#'
                            text.get(text.len().checked_sub(7)?..)
                        })
                        .and_then(|c| Color::from_hex(c).ok())
                        .map_or("color".into(), |color| {
                            Spans::from(vec![
                                Span::raw("color "),
                                Span::styled("■", Style::default().fg(color)),
                            ])
                        }),
                    16,
                ),
                Some(lsp::CompletionItemKind::FILE) => ("file".into(), 17),
                Some(lsp::CompletionItemKind::REFERENCE) => ("reference".into(), 18),
                Some(lsp::CompletionItemKind::FOLDER) => ("folder".into(), 19),
                Some(lsp::CompletionItemKind::ENUM_MEMBER) => ("enum_member".into(), 20),
                Some(lsp::CompletionItemKind::CONSTANT) => ("constant".into(), 21),
                Some(lsp::CompletionItemKind::STRUCT) => ("struct".into(), 22),
                Some(lsp::CompletionItemKind::EVENT) => ("event".into(), 23),
                Some(lsp::CompletionItemKind::OPERATOR) => ("operator".into(), 24),
                Some(lsp::CompletionItemKind::TYPE_PARAMETER) => ("type_param".into(), 25),
                Some(kind) => {
                    log::error!("Received unknown completion item kind: {:?}", kind);
                    ("".into(), 0)
                }
                None => ("".into(), 0),
            },
            CompletionItem::Snippet(_) => ("snippet".into(), 15),
            CompletionItem::Other(core::CompletionItem { kind, .. }) => {
                (kind.to_string().into(), 0)
            }
        };

        // JS 行渲染钩子：注册时自定义整行外观；未注册/抛错/返回不可用内容 → 回退原生两列。
        // 每行每帧调用（~20 行 × 钩子耗时），钩子里不要放重逻辑。
        let kind_text: String = kind_spans.0.iter().map(|s| s.content.as_ref()).collect();
        let provider_str = match self.provider() {
            core::completion::CompletionProvider::Lsp(_) => "lsp",
            core::completion::CompletionProvider::Path => "path",
            core::completion::CompletionProvider::Word => "word",
            core::completion::CompletionProvider::Snippet => "snippet",
        };
        let detail = match self {
            CompletionItem::Lsp(LspCompletionItem { item, .. }) => item.detail.clone(),
            _ => None,
        };
        if let Ok(content) = helix_js::render_completion_row(
            &label_text,
            &kind_text,
            kind_num,
            provider_str,
            detail.as_deref(),
            deprecated,
            self.match_indices().unwrap_or(&[]),
        ) {
            if let Some(row) = content_to_row(content) {
                // 钩子已完全自定义该行:清空匹配位置 → menu 渲染层跳过 highlight patch
                // (JS 内容与 label 的 match_indices 无对应关系,按索引 patch 会错位)
                self.set_match_indices(Vec::new());
                return row;
            }
        }

        let label = Span::styled(
            label_text,
            if deprecated {
                Style::default().add_modifier(Modifier::CROSSED_OUT)
            } else if kind_spans.0[0].content == "folder" {
                *dir_style
            } else {
                Style::default()
            },
        );

        // kind_num 0(非 LSP/未知 kind)也会调钩子——插件侧约定 0 返回空回退;规范 kind 为 1-25
        let kind_cell = match helix_js::completion_kind_icon(kind_num) {
            Some(icon) => menu::Cell::from(Span::raw(icon)),
            None => menu::Cell::from(kind_spans),
        };

        menu::Row::new([menu::Cell::from(label), kind_cell])
    }

    fn match_indices(&self) -> Option<&[u32]> {
        match self {
            CompletionItem::Lsp(LspCompletionItem { match_indices, .. }) => Some(match_indices),
            CompletionItem::Snippet(SnippetCompletionItem { match_indices, .. }) => {
                Some(match_indices)
            }
            CompletionItem::Other(core::CompletionItem { match_indices, .. }) => {
                Some(match_indices)
            }
        }
    }
}

/// 把 JS 行渲染钩子的返回内容转成 menu 行。仅支持单行单列：所有 span 拼进第一个 Cell，
/// 多 Cell/多行会破坏 Table 对齐（本批次不支持）。Tree / 空内容 → None（调用方回退原生两列）。
/// style 字符串经当前主题查表（参照 set_statusline 的 style 解析）；scope 未知 → 默认样式。
fn content_to_row(content: helix_js::Content) -> Option<menu::Row<'static>> {
    let helix_js::Content::Lines(lines) = content else {
        return None;
    };
    let theme = crate::commands::typed::current_theme_snapshot();
    let mut spans: Vec<Span<'static>> = Vec::new();
    for line in lines {
        for span in line.spans {
            let style = theme
                .as_ref()
                .and_then(|t| span.style.as_deref().and_then(|scope| t.try_get(scope)))
                .unwrap_or_default();
            spans.push(Span::styled(span.text, style));
        }
    }
    if spans.is_empty() {
        return None;
    }
    Some(menu::Row::new(vec![menu::Cell::from(Spans::from(spans))]))
}

/// Wraps a Menu.
pub struct Completion {
    popup: Popup<Menu<CompletionItem>>,
    #[allow(dead_code)]
    trigger_offset: usize,
    filter: String,
    // TODO: move to helix-view/central handler struct in the future
    resolve_handler: ResolveHandler,
}

impl Completion {
    pub const ID: &'static str = "completion";

    pub fn new(editor: &Editor, items: Vec<CompletionItem>, trigger_offset: usize) -> Self {
        let preview_completion_insert = editor.config().preview_completion_insert;
        let replace_mode = editor.config().completion_replace;

        let dir_style = editor.theme.get("ui.text.directory");

        // Then create the menu
        let menu = Menu::new(items, dir_style, move |editor: &mut Editor, item, event| {
            let (view, doc) = current!(editor);

            macro_rules! language_server {
                ($item:expr) => {
                    match editor
                        .language_servers
                        .get_by_id($item.provider)
                    {
                        Some(ls) => ls,
                        None => {
                            editor.set_error("completions are outdated");
                            // TODO close the completion menu somehow,
                            // currently there is no trivial way to access the EditorView to close the completion menu
                            return;
                        }
                    }
                };
            }

            match event {
                PromptEvent::Abort => {}
                PromptEvent::Update if preview_completion_insert => {
                    // Update creates "ghost" transactions which are not sent to the
                    // lsp server to avoid messing up re-requesting completions. Once a
                    // completion has been selected (with tab, c-n or c-p) it's always accepted whenever anything
                    // is typed. The only way to avoid that is to explicitly abort the completion
                    // with c-c. This will remove the "ghost" transaction.
                    //
                    // The ghost transaction is modeled with a transaction that is not sent to the LS.
                    // (apply_temporary) and a savepoint. It's extremely important this savepoint is restored
                    // (also without sending the transaction to the LS) *before any further transaction is applied*.
                    // Otherwise incremental sync breaks (since the state of the LS doesn't match the state the transaction
                    // is applied to).
                    if matches!(editor.last_completion, Some(CompleteAction::Triggered)) {
                        editor.last_completion = Some(CompleteAction::Selected {
                            savepoint: doc.savepoint(view),
                        })
                    }
                    let item = item.unwrap();
                    let context = &editor.handlers.completions.active_completions[&item.provider()];
                    // if more text was entered, remove it
                    doc.restore(view, &context.savepoint, false);
                    // always present here

                    match item {
                        CompletionItem::Lsp(item) => {
                            let (transaction, _) = lsp_item_to_transaction(
                                doc,
                                view.id,
                                &item.item,
                                language_server!(item).offset_encoding(),
                                trigger_offset,
                                replace_mode,
                            );
                            doc.apply_temporary(&transaction, view.id)
                        }
                        CompletionItem::Snippet(_) => false,
                        CompletionItem::Other(core::CompletionItem { transaction, .. }) => {
                            doc.apply_temporary(transaction, view.id)
                        }
                    };
                }
                PromptEvent::Update => {}
                PromptEvent::Validate => {
                    if let Some(CompleteAction::Selected { savepoint }) =
                        editor.last_completion.take()
                    {
                        doc.restore(view, &savepoint, false);
                    }

                    let item = item.unwrap();
                    let context = &editor.handlers.completions.active_completions[&item.provider()];
                    // if more text was entered, remove it
                    doc.restore(view, &context.savepoint, true);
                    // save an undo checkpoint before the completion
                    doc.append_changes_to_history(view);

                    // item always present here
                    let (transaction, additional_edits, snippet) = match item.clone() {
                        CompletionItem::Lsp(mut item) => {
                            let language_server = language_server!(item);

                            // resolve item if not yet resolved
                            if !item.resolved {
                                if let Some(resolved_item) = Self::resolve_completion_item(
                                    language_server,
                                    item.item.clone(),
                                ) {
                                    item.item = resolved_item;
                                }
                            };

                            let encoding = language_server.offset_encoding();
                            let (transaction, snippet) = lsp_item_to_transaction(
                                doc,
                                view.id,
                                &item.item,
                                encoding,
                                trigger_offset,
                                replace_mode,
                            );
                            let add_edits = item.item.additional_text_edits;

                            (
                                transaction,
                                add_edits.map(|edits| (edits, encoding)),
                                snippet,
                            )
                        }
                        CompletionItem::Snippet(item) => {
                            let mut ctx = doc.snippet_ctx();
                            let (transaction, snippet) = snippet_item_to_transaction(
                                doc.text(),
                                doc.selection(view.id),
                                &item.body,
                                trigger_offset,
                                replace_mode,
                                &mut ctx,
                            );
                            (transaction, None, snippet)
                        }
                        CompletionItem::Other(core::CompletionItem { transaction, .. }) => {
                            (transaction, None, None)
                        }
                    };

                    doc.apply(&transaction, view.id);
                    let placeholder = snippet.is_some();
                    if let Some(snippet) = snippet {
                        doc.active_snippet = match doc.active_snippet.take() {
                            Some(active) => active.insert_subsnippet(snippet),
                            None => ActiveSnippet::new(snippet),
                        };
                    }

                    editor.last_completion = Some(CompleteAction::Applied {
                        trigger_offset,
                        changes: completion_changes(&transaction, trigger_offset),
                        placeholder,
                    });

                    // TODO: add additional _edits to completion_changes?
                    if let Some((additional_edits, offset_encoding)) = additional_edits {
                        if !additional_edits.is_empty() {
                            let transaction = util::generate_transaction_from_edits(
                                doc.text(),
                                additional_edits,
                                offset_encoding, // TODO: should probably transcode in Client
                            );
                            doc.apply(&transaction, view.id);
                        }
                    }
                    // we could have just inserted a trigger char (like a `crate::` completion for rust
                    // so we want to retrigger immediately when accepting a completion.
                    trigger_auto_completion(editor, true);
                }
            };

            // In case the popup was deleted because of an intersection w/ the auto-complete menu.
            if event != PromptEvent::Update {
                editor
                    .handlers
                    .trigger_signature_help(SignatureHelpInvoked::Automatic, editor);
            }
        });

        let popup = Popup::new(Self::ID, menu)
            .with_scrollbar(false)
            .ignore_escape_key(true);

        let (view, doc) = current_ref!(editor);
        let text = doc.text().slice(..);
        let cursor = doc.selection(view.id).primary().cursor(text);
        let offset = text
            .chars_at(cursor)
            .reversed()
            .take_while(|ch| chars::char_is_word(*ch))
            .count();
        let start_offset = cursor.saturating_sub(offset);

        let fragment = doc.text().slice(start_offset..cursor);
        let mut completion = Self {
            popup,
            trigger_offset,
            // TODO: expand nucleo api to allow moving straight to a Utf32String here
            // and avoid allocation during matching
            filter: String::from(fragment),
            resolve_handler: ResolveHandler::new(),
        };

        // need to recompute immediately in case start_offset != trigger_offset
        completion.score(false);

        completion
    }

    fn score(&mut self, incremental: bool) {
        let pattern = &self.filter;
        let mut matcher = MATCHER.lock();
        matcher.config = Config::DEFAULT;
        // slight preference towards prefix matches
        matcher.config.prefer_prefix = true;
        let pattern = Atom::new(
            pattern,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
            false,
        );
        let mut buf = Vec::new();
        let (matches, options) = self.popup.contents_mut().update_options();
        if incremental {
            matches.retain_mut(|(index, score)| {
                let option = &mut options[*index as usize];
                let mut indices = Vec::new();
                let new_score = pattern.indices(
                    Utf32Str::new(option.filter_text(), &mut buf),
                    &mut matcher,
                    &mut indices,
                );
                match new_score {
                    Some(new_score) => {
                        *score = new_score as u32 / 2;
                        option.set_match_indices(indices);
                        true
                    }
                    None => false,
                }
            })
        } else {
            matches.clear();
            matches.extend(options.iter_mut().enumerate().filter_map(|(i, option)| {
                let mut indices = Vec::new();
                pattern
                    .indices(
                        Utf32Str::new(option.filter_text(), &mut buf),
                        &mut matcher,
                        &mut indices,
                    )
                    .map(|score| {
                        option.set_match_indices(indices);
                        (i as u32, score as u32 / 3)
                    })
            }));
        }
        // Nucleo is meant as an FZF-like fuzzy matcher and only hides matches that are truly
        // impossible - as in the sequence of characters just doesn't appear. That doesn't work
        // well for completions with multiple language servers where all completions of the next
        // server are below the current one (so you would get good suggestions from the second
        // server below those of the first). Setting a reasonable cutoff below which to move bad
        // completions out of the way helps with that.
        //
        // The score computation is a heuristic derived from Nucleo internal constants that may
        // move upstream in the future. I want to test this out here to settle on a good number.
        let min_score = (7 + pattern.needle_text().len() as u32 * 14) / 3;
        matches.sort_unstable_by_key(|&(i, score)| {
            let option = &options[i as usize];
            (
                score <= min_score,
                Reverse(option.preselect()),
                option.provider_priority(),
                Reverse(score),
                i,
            )
        });
    }

    /// Synchronously resolve the given completion item. This is used when
    /// accepting a completion.
    fn resolve_completion_item(
        language_server: &helix_lsp::Client,
        completion_item: lsp::CompletionItem,
    ) -> Option<lsp::CompletionItem> {
        if !matches!(
            language_server.capabilities().completion_provider,
            Some(lsp::CompletionOptions {
                resolve_provider: Some(true),
                ..
            })
        ) {
            return None;
        }
        let future = language_server.resolve_completion_item(&completion_item);
        let response = helix_lsp::block_on(future);
        match response {
            Ok(item) => Some(item),
            Err(err) => {
                log::error!("Failed to resolve completion item: {}", err);
                None
            }
        }
    }

    /// Appends (`c: Some(c)`) or removes (`c: None`) a character to/from the filter
    /// this should be called whenever the user types or deletes a character in insert mode.
    pub fn update_filter(&mut self, c: Option<char>) {
        // recompute menu based on matches
        let menu = self.popup.contents_mut();
        match c {
            Some(c) => self.filter.push(c),
            None => {
                self.filter.pop();
                if self.filter.is_empty() {
                    menu.clear();
                    return;
                }
            }
        }
        self.score(c.is_some());
        self.popup.contents_mut().reset_cursor();
    }

    pub fn replace_provider_completions(
        &mut self,
        response: &mut CompletionResponse,
        is_incomplete: bool,
    ) {
        let menu = self.popup.contents_mut();
        let (_, options) = menu.update_options();
        if is_incomplete {
            options.retain(|item| item.provider() != response.provider)
        }
        response.take_items(options);
        self.score(false);
        let menu = self.popup.contents_mut();
        menu.ensure_cursor_in_bounds();
    }

    pub fn is_empty(&self) -> bool {
        self.popup.contents().is_empty()
    }

    pub fn replace_item(
        &mut self,
        old_item: &impl PartialEq<CompletionItem>,
        new_item: CompletionItem,
    ) {
        self.popup.contents_mut().replace_option(old_item, new_item);
    }

    pub fn area(&mut self, viewport: Rect, editor: &Editor) -> Rect {
        self.popup.area(viewport, editor)
    }
}

impl Component for Completion {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        self.popup.handle_event(event, cx)
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        self.popup.required_size(viewport)
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        self.popup.render(area, surface, cx);

        // if we have a selection, render a markdown popup on top/below with info
        let option = match self.popup.contents_mut().selection_mut() {
            Some(option) => option,
            None => return,
        };
        if let CompletionItem::Lsp(option) = option {
            self.resolve_handler.ensure_item_resolved(cx.editor, option);
        }
        // need to render:
        // option.detail
        // ---
        // option.documentation

        let Some(coords) = cx.editor.cursor().0 else {
            return;
        };
        let cursor_pos = coords.row as u16;
        let doc = doc!(cx.editor);
        let language = doc.language_name().unwrap_or("");

        let markdowned = |lang: &str, detail: Option<&str>, doc: Option<&str>| {
            let md = match (detail, doc) {
                (Some(detail), Some(doc)) => format!("```{lang}\n{detail}\n```\n{doc}"),
                (Some(detail), None) => format!("```{lang}\n{detail}\n```"),
                (None, Some(doc)) => doc.to_string(),
                (None, None) => String::new(),
            };
            Markdown::new(md, cx.editor.syn_loader.clone())
        };

        let mut markdown_doc = match option {
            CompletionItem::Lsp(option) => match &option.item.documentation {
                Some(lsp::Documentation::String(contents))
                | Some(lsp::Documentation::MarkupContent(lsp::MarkupContent {
                    kind: lsp::MarkupKind::PlainText,
                    value: contents,
                })) => {
                    // TODO: convert to wrapped text
                    markdowned(language, option.item.detail.as_deref(), Some(contents))
                }
                Some(lsp::Documentation::MarkupContent(lsp::MarkupContent {
                    kind: lsp::MarkupKind::Markdown,
                    value: contents,
                })) => {
                    // TODO: set language based on doc scope
                    markdowned(language, option.item.detail.as_deref(), Some(contents))
                }
                None if option.item.detail.is_some() => {
                    // TODO: set language based on doc scope
                    markdowned(language, option.item.detail.as_deref(), None)
                }
                None => return,
            },
            CompletionItem::Snippet(SnippetCompletionItem { description, .. }) => {
                let Some(doc) = description.as_deref() else {
                    return;
                };
                markdowned(language, None, Some(doc))
            }
            CompletionItem::Other(option) => {
                let Some(doc) = option.documentation.as_deref() else {
                    return;
                };
                markdowned(language, None, Some(doc))
            }
        };

        let popup_area = self.popup.area(area, cx.editor);
        let doc_width_available = area.width.saturating_sub(popup_area.right());
        let doc_area = if doc_width_available > 30 {
            let mut doc_width = doc_width_available;
            let mut doc_height = area.height.saturating_sub(popup_area.top());
            let x = popup_area.right();
            let y = popup_area.top();

            if let Some((rel_width, rel_height)) =
                markdown_doc.required_size((doc_width, doc_height))
            {
                doc_width = rel_width.min(doc_width);
                doc_height = rel_height.min(doc_height);
            }
            Rect::new(x, y, doc_width, doc_height)
        } else {
            // Documentation should not cover the cursor or the completion popup
            // Completion popup could be above or below the current line
            let avail_height_above = cursor_pos.min(popup_area.top()).saturating_sub(1);
            let avail_height_below = area
                .height
                .saturating_sub(cursor_pos.max(popup_area.bottom()) + 1 /* padding */);
            let (y, avail_height) = if avail_height_below >= avail_height_above {
                (
                    area.height.saturating_sub(avail_height_below),
                    avail_height_below,
                )
            } else {
                (0, avail_height_above)
            };
            if avail_height <= 1 {
                return;
            }

            Rect::new(0, y, area.width, avail_height.min(15))
        };

        // clear area
        let background = cx.editor.theme.get("ui.popup");
        surface.clear_with(doc_area, background);

        if cx.editor.popup_border() {
            use tui::widgets::{Block, Widget};
            Widget::render(Block::bordered(), doc_area, surface);
        }

        markdown_doc.render(doc_area, surface, cx);
    }
}
fn lsp_item_to_transaction(
    doc: &Document,
    view_id: ViewId,
    item: &lsp::CompletionItem,
    offset_encoding: OffsetEncoding,
    trigger_offset: usize,
    replace_mode: bool,
) -> (Transaction, Option<RenderedSnippet>) {
    let selection = doc.selection(view_id);
    let text = doc.text().slice(..);
    let primary_cursor = selection.primary().cursor(text);

    let (edit_offset, new_text) = if let Some(edit) = &item.text_edit {
        let edit = match edit {
            lsp::CompletionTextEdit::Edit(edit) => edit.clone(),
            lsp::CompletionTextEdit::InsertAndReplace(item) => {
                let range = if replace_mode {
                    item.replace
                } else {
                    item.insert
                };
                lsp::TextEdit::new(range, item.new_text.clone())
            }
        };

        let Some(range) = util::lsp_range_to_range(doc.text(), edit.range, offset_encoding) else {
            return (Transaction::new(doc.text()), None);
        };

        let start_offset = range.anchor as i128 - primary_cursor as i128;
        let end_offset = range.head as i128 - primary_cursor as i128;

        (Some((start_offset, end_offset)), edit.new_text)
    } else {
        let new_text = item
            .insert_text
            .clone()
            .unwrap_or_else(|| item.label.clone());
        // check that we are still at the correct savepoint
        // we can still generate a transaction regardless but if the
        // document changed (and not just the selection) then we will
        // likely delete the wrong text (same if we applied an edit sent by the LS)
        debug_assert!(primary_cursor == trigger_offset);
        (None, new_text)
    };

    if matches!(item.kind, Some(lsp::CompletionItemKind::SNIPPET))
        || matches!(
            item.insert_text_format,
            Some(lsp::InsertTextFormat::SNIPPET)
        )
    {
        let Ok(snippet) = Snippet::parse(&new_text) else {
            log::error!("Failed to parse snippet: {new_text:?}",);
            return (Transaction::new(doc.text()), None);
        };
        let (transaction, snippet) = util::generate_transaction_from_snippet(
            doc.text(),
            selection,
            edit_offset,
            replace_mode,
            snippet,
            &mut doc.snippet_ctx(),
        );
        (transaction, Some(snippet))
    } else {
        let transaction = util::generate_transaction_from_completion_edit(
            doc.text(),
            selection,
            edit_offset,
            replace_mode,
            new_text,
        );
        (transaction, None)
    }
}

/// Snippet 候选 → transaction + RenderedSnippet。edit_offset 覆盖光标前已输入单词(删除前缀)。
fn snippet_item_to_transaction(
    text: &Rope,
    selection: &Selection,
    body: &str,
    // 保留以对齐 lsp_item_to_transaction 签名;前缀删除由 move_prev_word_start 从 primary_cursor 计算,不依赖 trigger_offset
    _trigger_offset: usize,
    replace_mode: bool,
    snippet_ctx: &mut SnippetRenderCtx,
) -> (Transaction, Option<RenderedSnippet>) {
    let primary_cursor = selection.primary().cursor(text.slice(..));
    // 光标前单词范围(删除已输入 prefix);无单词则不替换
    let edit_offset = {
        // gate:前一字符非 word char(光标在单词起点/空白后/行首)→ 纯插入不删词
        // (move_prev_word_start 会跨空白/换行删到前一个单词,与 LSP 路径 find_completion_range 语义不一致)
        let prev_is_word = text
            .chars_at(primary_cursor)
            .reversed()
            .next()
            .is_some_and(helix_core::chars::char_is_word);
        if !prev_is_word {
            None
        } else {
            let cursor = helix_core::movement::move_prev_word_start(
                text.slice(..),
                core::Range::point(primary_cursor),
                1,
            );
            if cursor.head == primary_cursor {
                None
            } else {
                Some((cursor.head as i128 - primary_cursor as i128, 0))
            }
        }
    };
    let Ok(snippet) = Snippet::parse(body) else {
        log::error!("Failed to parse snippet: {body:?}");
        return (Transaction::new(text), None);
    };
    let (transaction, snippet) = util::generate_transaction_from_snippet(
        text,
        selection,
        edit_offset,
        replace_mode,
        snippet,
        snippet_ctx,
    );
    (transaction, Some(snippet))
}

fn completion_changes(transaction: &Transaction, trigger_offset: usize) -> Vec<Change> {
    transaction
        .changes_iter()
        .filter(|(start, end, _)| (*start..=*end).contains(&trigger_offset))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::menu::Item;

    fn lsp_item(kind: Option<lsp::CompletionItemKind>) -> CompletionItem {
        CompletionItem::Lsp(LspCompletionItem {
            item: lsp::CompletionItem {
                label: "foo".into(),
                kind,
                ..Default::default()
            },
            provider: core::diagnostic::LanguageServerId::default(),
            resolved: false,
            provider_priority: 0,
            match_indices: Vec::new(),
        })
    }

    fn cells(row: &menu::Row<'_>) -> Vec<String> {
        row.cell_text().collect()
    }

    #[test]
    fn snippet_item_kind_and_priority() {
        use crate::handlers::completion::{CompletionItem, SnippetCompletionItem};
        use helix_core::completion::CompletionProvider;
        let mut item = CompletionItem::Snippet(SnippetCompletionItem {
            label: "fn".into(),
            body: "function ${1:name}(${2:params}) {\n\t${0}\n}".into(),
            description: Some("Function declaration".into()),
            provider_priority: 0,
            match_indices: Vec::new(),
        });
        assert_eq!(item.provider(), CompletionProvider::Snippet);
        assert_eq!(item.provider_priority(), 0);
        // format:kind 文本 "snippet"、kind_num 15
        let row = CompletionItem::format(&mut item, &Style::default());
        assert_eq!(cells(&row), vec!["fn".to_string(), "snippet".to_string()]);
    }

    #[test]
    fn highlight_matched_indices() {
        // match_indices 存进候选;format 输出不因高亮字段变化
        let mut item = CompletionItem::Other(core::CompletionItem {
            transaction: Transaction::new(&core::Rope::from("x")),
            label: "formatName".into(),
            kind: "word".into(),
            documentation: None,
            provider: core::completion::CompletionProvider::Word,
            match_indices: Vec::new(),
        });
        item.set_match_indices(vec![0, 1, 2]);
        assert_eq!(item.match_indices(), Some(&[0, 1, 2][..]));
        let row = CompletionItem::format(&mut item, &Style::default());
        let joined: String = row.cells[0].content.lines[0]
            .0
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(joined, "formatName");
    }

    #[test]
    fn hook_rendered_row_clears_match_indices() {
        // 行渲染钩子生效时,JS 已完全自定义该行:按 label 的 match_indices 去 patch 会错位——
        // format 命中钩子后应清空 match_indices,menu 渲染层据此跳过高亮 patch
        helix_js::init();
        helix_js::load_script(
            r#"helix.set_completion_render((ctx) => [{ type: "text", text: ctx.label + "|" + ctx.provider, style: "ui.completion" }]);"#,
        )
        .unwrap();
        let mut item = CompletionItem::Other(core::CompletionItem {
            transaction: Transaction::new(&core::Rope::from("x")),
            label: "formatName".into(),
            kind: "word".into(),
            documentation: None,
            provider: core::completion::CompletionProvider::Word,
            match_indices: vec![0, 1, 2],
        });
        let row = CompletionItem::format(&mut item, &Style::default());
        assert_eq!(row.cells.len(), 1); // 钩子行单列(原生两列)
        assert!(item.match_indices().map_or(true, |i| i.is_empty())); // 清空 → 渲染层跳过 patch
                                                                      // 收尾:重注册无害钩子,防线程复用残留影响后续断言
        helix_js::load_script(r#"helix.set_completion_render(() => []);"#).unwrap();
    }

    #[test]
    fn format_kind_text_without_hook() {
        // 未注册钩子 → kind 文本
        let mut item = lsp_item(Some(lsp::CompletionItemKind::METHOD));
        let row = CompletionItem::format(&mut item, &Style::default());
        assert_eq!(cells(&row), vec!["foo".to_string(), "method".to_string()]);
    }

    #[test]
    fn format_kind_icon_with_hook() {
        // 注册钩子 → kind cell 是图标字符
        helix_js::init();
        helix_js::load_script(r#"helix.set_completion_icon((k) => "i" + k);"#).unwrap();
        let mut item = lsp_item(Some(lsp::CompletionItemKind::METHOD));
        let row = CompletionItem::format(&mut item, &Style::default());
        assert_eq!(cells(&row), vec!["foo".to_string(), "i2".to_string()]);
    }

    #[test]
    fn snippet_item_to_transaction_empty_prefix_does_not_delete_previous_word() {
        use helix_core::{indent::IndentStyle, snippets::SnippetRenderCtx};
        let mut rope = core::Rope::from("abc fn");
        let selection = Selection::point(4); // 光标在空白后(空前缀 accept)
        let body = "function ${1:name}() {\n\t${0}\n}";
        let mut ctx = SnippetRenderCtx {
            resolve_var: Box::new(|_| None),
            tab_width: 4,
            indent_style: IndentStyle::Spaces(4),
            line_ending: "\n",
        };
        let (transaction, snippet) =
            snippet_item_to_transaction(&rope, &selection, body, 4, false, &mut ctx);
        assert!(snippet.is_some());
        transaction.apply(&mut rope);
        let out = rope.to_string();
        assert!(out.starts_with("abc function")); // 纯插入:前面 "abc " 不被删
    }

    #[test]
    fn snippet_item_to_transaction_replaces_prefix() {
        use helix_core::{indent::IndentStyle, snippets::SnippetRenderCtx};
        let mut rope = core::Rope::from("fn");
        let selection = Selection::point(2); // 光标在 "fn" 后
        let body = "function ${1:name}() {\n\t${0}\n}";
        let mut ctx = SnippetRenderCtx {
            resolve_var: Box::new(|_| None),
            tab_width: 4,
            indent_style: IndentStyle::Spaces(4),
            line_ending: "\n",
        };
        let (transaction, snippet) =
            snippet_item_to_transaction(&rope, &selection, body, 2, false, &mut ctx);
        assert!(snippet.is_some());
        transaction.apply(&mut rope);
        let out = rope.to_string();
        assert!(out.starts_with("function")); // 前缀 "fn" 被 body 替换
    }

    #[test]
    fn format_other_item_ignores_hook() {
        // 非 LSP 候选:无数字 kind,kind_num=0;钩子只在 1-25 返回 → 0 回退到 kind 字符串
        helix_js::init();
        helix_js::load_script(
            r#"helix.set_completion_icon((k) => k >= 1 && k <= 25 ? "i" + k : "");"#,
        )
        .unwrap();
        let mut item = CompletionItem::Other(core::CompletionItem {
            transaction: Transaction::new(&core::Rope::from("foo")),
            label: "foo".into(),
            kind: "word".into(),
            documentation: None,
            provider: core::completion::CompletionProvider::Word,
            match_indices: Vec::new(),
        });
        let row = CompletionItem::format(&mut item, &Style::default());
        assert_eq!(cells(&row), vec!["foo".to_string(), "word".to_string()]);
    }

    #[test]
    fn format_render_hook_merges_into_single_cell() {
        // 注册行渲染钩子 → 返回内容拼进第一个 Cell(单列,不破坏 Table 对齐)
        helix_js::init();
        helix_js::load_script(
            r#"helix.set_completion_render((ctx) => [{ type: "text", text: ctx.label + "[" + ctx.kind + "]", style: "ui.completion" }]);"#,
        )
        .unwrap();
        let mut item = lsp_item(Some(lsp::CompletionItemKind::METHOD));
        let row = CompletionItem::format(&mut item, &Style::default());
        assert_eq!(cells(&row), vec!["foo[method]".to_string()]);
        // 抛错 → 回退原生两列
        helix_js::load_script(r#"helix.set_completion_render(() => { throw new Error("x"); });"#)
            .unwrap();
        let row = CompletionItem::format(&mut item, &Style::default());
        assert_eq!(cells(&row), vec!["foo".to_string(), "method".to_string()]);
        // 收尾重注册无害钩子(避免 thread_local 残留抛错钩子影响后续断言)
        helix_js::load_script(r#"helix.set_completion_render(() => []);"#).unwrap();
    }
}
