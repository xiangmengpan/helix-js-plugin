//! `cargo xtask highlight-check` 的实现。
//!
//! 从 `main.rs` 的 `tasks` 模块拆出(G5):一个任务一个文件,统一入口
//! `run(args: Vec<String>)`,由 `tasks::TASKS` 表驱动 dispatch 与 help。

use crate::DynError;
use std::collections::HashSet;

pub fn run(args: Vec<String>) -> Result<(), DynError> {
    let args = args.into_iter();
    use helix_core::syntax::{HighlightEvent, Loader, Syntax};
    use helix_core::Language;
    use ropey::Rope;

    // The highlighter yields a `Highlight` index into the loader's scope
    // list. Feed it the capture names actually used across the bundled
    // queries, so every capture maps to itself and we read back the exact
    // `@capture` that won — no hand-maintained scope list. (Inheritance is
    // irrelevant here: scanning every language's queries unions all names.)
    fn capture_scopes() -> Vec<String> {
        let mut set = std::collections::BTreeSet::new();
        let Ok(langs) = std::fs::read_dir(crate::path::ts_queries()) else {
            return Vec::new();
        };
        for lang in langs.filter_map(Result::ok) {
            for q in ["highlights.scm", "locals.scm"] {
                let Ok(text) = std::fs::read_to_string(lang.path().join(q)) else {
                    continue;
                };
                let b = text.as_bytes();
                let mut i = 0;
                while i < b.len() {
                    if b[i] != b'@' {
                        i += 1;
                        continue;
                    }
                    let s = i + 1;
                    let mut j = s;
                    while j < b.len()
                        && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'.' | b'_' | b'-'))
                    {
                        j += 1;
                    }
                    let name = &text[s..j];
                    i = j;
                    if name.is_empty() || name.starts_with('_') {
                        continue;
                    }
                    // Locals: the highlight applied to a resolved reference is
                    // the class after `local.definition.`; the rest are structural.
                    if let Some(class) = name.strip_prefix("local.definition.") {
                        if !class.is_empty() {
                            set.insert(class.to_string());
                        }
                    } else if !name.starts_with("local.") && name != "local" {
                        set.insert(name.to_string());
                    }
                }
            }
        }
        set.into_iter().collect()
    }

    // Winning (top-of-stack) capture per byte range, via the real highlighter.
    fn spans(
        loader: &Loader,
        scopes: &[String],
        language: Language,
        source: &str,
    ) -> Option<Vec<(usize, usize, String)>> {
        let rope = Rope::from_str(source);
        // None when the grammar isn't built, so callers can skip rather than
        // panic (a no-language run only needs the corpus' grammars built).
        let syntax = Syntax::new(rope.slice(..), language, loader).ok()?;
        let mut hl = syntax.highlighter(rope.slice(..), loader, ..);
        let mut active: Vec<u32> = Vec::new();
        let mut start: u32 = 0;
        let len = source.len() as u32;
        let mut out = Vec::new();
        loop {
            let off = hl.next_event_offset();
            let cur = if off == u32::MAX { len } else { off };
            if cur > start {
                if let Some(idx) = active.last() {
                    out.push((start as usize, cur as usize, scopes[*idx as usize].clone()));
                }
            }
            if off == u32::MAX {
                break;
            }
            let (event, highlights) = hl.advance();
            let v: Vec<u32> = highlights.map(|h| h.get()).collect();
            match event {
                HighlightEvent::Refresh => active = v,
                HighlightEvent::Push => active.extend(v),
            }
            start = off;
        }
        Some(out)
    }

    let scopes = capture_scopes();
    let loader = helix_core::config::default_lang_loader();
    loader.set_scopes(scopes.clone());

    let args: Vec<String> = args.collect();

    // `--dump <language> <file>`: print the winning capture per span, for
    // discovering the exact `@capture` names when authoring assertions.
    if args.first().map(String::as_str) == Some("--dump") {
        let lang = args
            .get(1)
            .ok_or("usage: highlight-check --dump <language> <file>")?;
        let file = args
            .get(2)
            .ok_or("usage: highlight-check --dump <language> <file>")?;
        let language = loader
            .language_for_name(lang.as_str())
            .ok_or_else(|| format!("unknown language '{lang}'"))?;
        let source = std::fs::read_to_string(file)?;
        let sp = spans(&loader, &scopes, language, &source)
            .ok_or_else(|| format!("could not highlight (grammar for '{lang}' not built?)"))?;
        for (s, e, scope) in sp {
            let text = &source[s..e];
            if !text.trim().is_empty() {
                println!("{scope}\t{text:?}");
            }
        }
        return Ok(());
    }

    // Corpus mode: nvim-treesitter-style highlight tests laid out under
    // tests/query/highlights/<language-id>/<name>.<ext>. A comment line
    // below the code carries caret assertions whose columns line up with the
    // tokens above:
    //
    //     foo(bar)
    //     // ^ @function
    //     //     ^^^ @variable
    //
    // Each `^` checks the *winning* capture at the column above it; the
    // expected `@capture` must match exactly (a leading `!` negates). This
    // catches precedence bugs query-check cannot — e.g. a call captured on a
    // wrapper node that the inner `(identifier) @variable` then wins.
    let filter: HashSet<String> = args.into_iter().collect();
    let root = crate::path::tests_highlight();
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    for lang_dir in std::fs::read_dir(&root)?.filter_map(Result::ok) {
        if !lang_dir.path().is_dir() {
            continue;
        }
        let lang = lang_dir.file_name().to_string_lossy().into_owned();
        if !filter.is_empty() && !filter.contains(&lang) {
            continue;
        }
        for f in std::fs::read_dir(lang_dir.path())?.filter_map(Result::ok) {
            if f.path().is_file() {
                files.push((lang.clone(), f.path()));
            }
        }
    }
    files.sort();

    let mut errors = 0usize;
    let mut checks = 0usize;
    for (lang, path) in files {
        let language = match loader
            .languages()
            .find(|(_, d)| d.config().language_id == lang)
        {
            Some((language, _)) => language,
            None => {
                return Err(format!("{}: no configured language '{lang}'", path.display()).into())
            }
        };
        let source = std::fs::read_to_string(&path)?;
        let Some(sp) = spans(&loader, &scopes, language, &source) else {
            eprintln!("{}: skipped ('{lang}' grammar not built)", path.display());
            continue;
        };
        let at = |byte: usize| -> Option<&str> {
            sp.iter()
                .find(|(s, e, _)| *s <= byte && byte < *e)
                .map(|(_, _, n)| n.as_str())
        };

        let lines: Vec<&str> = source.lines().collect();
        let mut line_start = Vec::with_capacity(lines.len());
        let mut off = 0usize;
        for l in &lines {
            line_start.push(off);
            off += l.len() + 1;
        }

        let rel = path.strip_prefix(&root).unwrap_or(&path).display();
        let mut base: Option<usize> = None;
        for (li, line) in lines.iter().enumerate() {
            // An assertion line is a comment whose first `^` is preceded only
            // by the comment leader (no alphanumerics), so code like `a ^ b`
            // is never mistaken for one.
            let marker = line
                .find('^')
                .filter(|&c| !line[..c].bytes().any(|b| b.is_ascii_alphanumeric()));
            let Some(col) = marker else {
                if !line.trim().is_empty() {
                    base = Some(li);
                }
                continue;
            };
            let Some(brow) = base else { continue };
            let len = line[col..].bytes().take_while(|b| *b == b'^').count();
            let mut exp = line[col + len..].trim();
            let negate = exp.starts_with('!');
            if negate {
                exp = exp[1..].trim_start();
            }
            // First whitespace-delimited token after the caret, with a
            // leading `@` stripped — so a trailing comment close (OCaml
            // `*)`, HTML `-->`) isn't swallowed into the expected name.
            let exp = exp
                .trim_start_matches('@')
                .split_whitespace()
                .next()
                .unwrap_or("");
            if exp.is_empty() {
                continue;
            }
            let code = lines[brow];
            for c in col..col + len {
                // Skip carets that fall past the code line or over whitespace.
                match code.as_bytes().get(c) {
                    None => continue,
                    Some(b) if b.is_ascii_whitespace() => continue,
                    _ => {}
                }
                checks += 1;
                let got = at(line_start[brow] + c);
                if (got == Some(exp)) != negate {
                    continue;
                }
                errors += 1;
                eprintln!(
                    "{rel}:{}:{}: expected {}{} but got {}",
                    brow + 1,
                    c + 1,
                    if negate { "not " } else { "" },
                    exp,
                    got.unwrap_or("<unhighlighted>")
                );
            }
        }
    }

    if errors > 0 {
        return Err(
            format!("Highlight check failed: {errors} of {checks} assertion(s) wrong").into(),
        );
    }
    println!("Highlight check succeeded ({checks} assertions)");
    Ok(())
}
