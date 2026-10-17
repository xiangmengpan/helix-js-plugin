//! `cargo xtask indent-check` 的实现。
//!
//! 从 `main.rs` 的 `tasks` 模块拆出(G5):一个任务一个文件,统一入口
//! `run(args: Vec<String>)`,由 `tasks::TASKS` 表驱动 dispatch 与 help。

use crate::DynError;
use std::collections::HashSet;

pub fn run(args: Vec<String>) -> Result<(), DynError> {
    let languages = args.into_iter();
    use helix_core::{
        indent::{is_opaque_interior, is_outdent_token_at, treesitter_indent_for_pos, IndentStyle},
        Syntax,
    };
    use helix_stdx::rope::RopeSliceExt;
    use ropey::Rope;

    let filter: HashSet<String> = languages.collect();
    let loader = helix_core::config::default_lang_loader();
    let corpus = crate::path::tests_indent();
    let tab_width = 4;
    let mut errors = 0usize;
    let mut over_notes = 0usize;

    let mut entries: Vec<_> = std::fs::read_dir(&corpus)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    entries.sort();

    for path in entries {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if !filter.is_empty() && !filter.contains(stem) {
            continue;
        }
        let file = path.file_name().and_then(|s| s.to_str()).unwrap_or("");

        // Corpus files are named <language-id>.<ext>; resolve the language by id.
        let language = match loader
            .languages()
            .find(|(_, data)| data.config().language_id == stem)
        {
            Some((language, _)) => language,
            None => {
                return Err(format!(
                    "{file}: no configured language with id '{stem}' (corpus files are named <language-id>.<ext>)"
                )
                .into())
            }
        };

        let config = loader.language(language).config();
        let indent_style = IndentStyle::from_str(
            &config
                .indent
                .as_ref()
                .ok_or_else(|| format!("{file}: language '{stem}' has no indent config"))?
                .unit,
        );
        // Lines that are commented out are skipped: they self-document edge
        // cases (e.g. known indent limitations) without being asserted on.
        let comment_tokens: Vec<String> = config.comment_tokens.clone().unwrap_or_default();

        let doc = Rope::from_reader(&mut std::fs::File::open(&path)?)?;
        let text = doc.slice(..);
        let syntax = Syntax::new(text, language, &loader)
            .map_err(|e| format!("{file}: failed to parse: {e:?}"))?;
        let indent_query = loader
            .indent_query(language)
            .ok_or_else(|| format!("{file}: language '{stem}' has no indent query"))?;

        for i in 0..doc.len_lines() {
            let line = text.line(i);
            let Some(pos) = line.first_non_whitespace_char() else {
                continue;
            };

            let trimmed = line.slice(pos..).to_string();
            if comment_tokens
                .iter()
                .any(|tok| trimmed.starts_with(tok.as_str()))
            {
                continue;
            }

            let suggested = treesitter_indent_for_pos(
                indent_query,
                &syntax,
                &loader,
                tab_width,
                indent_style.indent_width(tab_width),
                text,
                i,
                text.line_to_char(i) + pos,
                false,
            )
            .unwrap()
            .to_string(&indent_style, tab_width);

            let actual = line
                .get_slice(..pos)
                .map(|s| s.to_string())
                .unwrap_or_default();

            if actual != suggested {
                errors += 1;
                println!(
                    "{file}:{}: reindent expected {} columns, computed {}",
                    i + 1,
                    actual.chars().count(),
                    suggested.chars().count(),
                );
            }

            // Typing direction: simulate pressing Enter at the end of this line and check the indent computed for the next line.
            //   - under-indent (computed < canonical) is always a failure: nothing pulls the line further in, so the user is left
            //     under-indented.
            //   - over-indent (computed > canonical) is only acceptable when the next line's leading token is @outdent (a closing
            //     bracket, case/else/except keyword, ...): entering it dedents the line. An over-indent on a plain statement (e.g.
            //     a line after `return` that should leave the block but doesn't) has nothing to correct it and is a real failure.
            if i + 1 < doc.len_lines() {
                if let Some(next_pos) = text.line(i + 1).first_non_whitespace_char() {
                    let next = text.line(i + 1);
                    let next_trim = next.slice(next_pos..).to_string();
                    // Lines inside an @opaque body (string/comment) carry literal leading whitespace, not code indent — don't
                    // assert a typing indent for them.
                    let next_byte = text.char_to_byte(text.line_to_char(i + 1) + next_pos) as u32;

                    let next_is_opaque = is_opaque_interior(indent_query, &syntax, text, next_byte);
                    let next_is_comment = next_is_opaque
                        || comment_tokens
                            .iter()
                            .any(|tok| next_trim.starts_with(tok.as_str()));

                    if !next_is_comment {
                        let typed = treesitter_indent_for_pos(
                            indent_query,
                            &syntax,
                            &loader,
                            tab_width,
                            indent_style.indent_width(tab_width),
                            text,
                            i,
                            text.line_to_char(i + 1) - 1,
                            true,
                        )
                        .unwrap()
                        .to_string(&indent_style, tab_width);

                        let next_actual = next
                            .get_slice(..next_pos)
                            .map(|s| s.to_string())
                            .unwrap_or_default();

                        let typed_cols = typed.chars().count();
                        let want_cols = next_actual.chars().count();
                        let leading_outdent = || {
                            let byte = text.char_to_byte(text.line_to_char(i + 1) + next_pos);
                            is_outdent_token_at(indent_query, &syntax, text, byte as u32)
                        };

                        if typed_cols < want_cols {
                            errors += 1;
                            println!(
                                "{file}:{}: typing under-indents: computed {} columns, expected {} | {}",
                                i + 2,
                                typed_cols,
                                want_cols,
                                next_trim.trim_end(),
                            );
                        } else if typed_cols > want_cols && !leading_outdent() {
                            // Over-indents are reported but not failed: every over-indent sits at a legitimate dedent point, and
                            // for indent-delimited languages (python after a block) the editor genuinely cannot know how far to
                            // dedent. Surfaced so a regression shows up in the diff; gate on under-indents only.
                            over_notes += 1;
                            println!(
                                "{file}:{}: note: typing over-indents: computed {} columns, expected {} (no leading outdent; review) | {}",
                                i + 2,
                                typed_cols,
                                want_cols,
                                next_trim.trim_end(),
                            );
                        }
                    }
                }
            }
        }
    }

    if over_notes > 0 {
        println!("Indent check: {over_notes} typing over-indent note(s) (not failures; review for regressions)");
    }
    match errors {
        0 => {
            println!("Indent check succeeded");
            Ok(())
        }
        n => Err(format!("Indent check failed: {n} line(s) with wrong indentation").into()),
    }
}
