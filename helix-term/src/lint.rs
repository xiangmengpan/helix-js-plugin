//! `:lint` —— 跑**已装的 linter 配方**，把输出解析成编辑器诊断。
//!
//! 为什么在核心：诊断通道原来只有 LSP（`DiagnosticProvider` 只有 `Lsp` 变体），
//! 于是 `kind = "linter"` 的配方"装完没有任何东西用它"。这里补上最小可用通道：
//!
//! - 只跑**当前文件语言**匹配、且**已安装**的 linter 配方（`[server-manager.registry.*]`
//!   的 `kind = "linter"` + `lint-cmd`）；
//! - 命令是 shell 串，支持编辑器变量展开（`%{file_path_absolute}` 等）；
//! - 输出按 **gcc / parsable** 家族解析：`path:line:col: [severity:] message`
//!   （ruff `--output-format=concise`、flake8、shellcheck `-f gcc`、yamllint `-f parsable`、
//!   golangci-lint `--out-format=line-number`、hadolint `--format tty` 都是这一族）；
//! - 诊断用 `DiagnosticProvider::Linter{name}` ⇒ **不会**把 LSP 的诊断清掉。

use helix_core::diagnostic::Range;
use helix_core::diagnostic::{Diagnostic, DiagnosticProvider, Severity};
use helix_view::editor::Editor;
use std::sync::Arc;

/// 跑一次 lint。`only` = 只跑指定配方名（`None` = 当前语言的全部已装 linter）。
pub fn run(editor: &mut Editor, only: Option<&str>) -> anyhow::Result<String> {
    use crate::server_manager::{availability, registry, Availability};

    let (path, lang) = {
        let doc_id = helix_view::doc!(editor).id();
        let doc = editor
            .document(doc_id)
            .ok_or_else(|| anyhow::anyhow!("没有当前文档"))?;
        let path = doc
            .path()
            .ok_or_else(|| anyhow::anyhow!("当前 buffer 没有文件路径（先保存）"))?
            .to_path_buf();
        let lang = doc
            .language_config()
            .map(|c| c.language_id.clone())
            .unwrap_or_default();
        (path, lang)
    };

    let mut ran: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for spec in registry::all() {
        if spec.kind != registry::Kind::Linter {
            continue;
        }
        if let Some(want) = only {
            if spec.name != want {
                continue;
            }
        } else if !spec.languages.iter().any(|l| l == &lang || l == "any") {
            continue;
        }
        if !matches!(
            availability(&spec.name),
            Availability::Managed | Availability::Local(_)
        ) {
            skipped.push(format!("{}（未安装）", spec.name));
            continue;
        }
        let Some(cmd_tpl) = spec.lint_cmd.as_deref() else {
            skipped.push(format!("{}（配方没给 lint-cmd）", spec.name));
            continue;
        };
        // 变量展开（与 formatter args 同一套 `%{…}`，单个 token 整体展开）
        let cmd = expand_vars(editor, cmd_tpl);
        let cwd = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        // **受管 bin 目录进 PATH**：`lint-cmd` 里写的是裸命令名（`ruff check …`），
        // 受管安装的 bin 只挂在 managed/bin 下、不在用户 PATH 里 ⇒ 不加就会
        // "sh: ruff: command not found" 然后被当成"0 条问题"（实测踩到）。
        let managed_bin = crate::server_manager::managed_root().join("bin");
        let path_env = match std::env::var("PATH") {
            Ok(p) if !p.is_empty() => format!("{}:{}", managed_bin.display(), p),
            _ => managed_bin.to_string_lossy().into_owned(),
        };
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .current_dir(&cwd)
            .env("PATH", path_env)
            .output();
        let out = match out {
            Ok(o) => o,
            Err(e) => {
                skipped.push(format!("{}（起不来: {e}）", spec.name));
                continue;
            }
        };
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        // 命令根本没跑起来（没装到 PATH / 名字写错）⇒ 直接说清楚，别报"0 条问题"
        if out.status.code() == Some(127) || text.contains("command not found") {
            skipped.push(format!(
                "{}（命令没找到：{}）",
                spec.name,
                cmd.split_whitespace().next().unwrap_or("?")
            ));
            continue;
        }
        let n = attach_diagnostics(editor, &spec.name, &path, &text);
        ran.push(format!("{}：{n} 条", spec.name));
    }

    // 没跑任何东西时也清掉上次的 lint 诊断（避免"改了还是旧提示"）
    if ran.is_empty() {
        return Ok(format!(
            "没有可跑的 linter（语言 {lang}）{}",
            if skipped.is_empty() {
                String::new()
            } else {
                format!("：{}", skipped.join("、"))
            }
        ));
    }
    let mut msg = format!("lint: {}", ran.join(" · "));
    if !skipped.is_empty() {
        msg.push_str(&format!("（跳过 {}）", skipped.join("、")));
    }
    Ok(msg)
}

/// 展开 `%{file_path_absolute}` 这类编辑器变量（不引号解析，整串当一个 token）。
fn expand_vars(editor: &mut Editor, tpl: &str) -> String {
    use helix_core::command_line::Token;
    match helix_view::expansion::expand(editor, Token::expand(tpl)) {
        Ok(s) => s.into_owned(),
        Err(e) => {
            log::warn!("lint-cmd 变量展开失败（按原文执行）: {e}");
            tpl.to_string()
        }
    }
}

/// 解析 gcc 家族输出并挂到对应文档上（**只挂已打开的文件**；其它文件只计数）。
fn attach_diagnostics(
    editor: &mut Editor,
    name: &str,
    current: &std::path::Path,
    text: &str,
) -> usize {
    // path → 诊断
    let mut by_file: std::collections::BTreeMap<
        std::path::PathBuf,
        Vec<(usize, usize, String, Severity)>,
    > = std::collections::BTreeMap::new();
    for line in text.lines() {
        let Some(parsed) = parse_gcc_line(line) else {
            continue;
        };
        let p = if parsed.0.is_absolute() {
            parsed.0
        } else {
            current
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(parsed.0)
        };
        by_file
            .entry(normalize(&p))
            .or_default()
            .push((parsed.1, parsed.2, parsed.3, parsed.4));
    }
    let mut total = 0usize;
    let provider = DiagnosticProvider::Linter {
        name: Arc::from(name),
    };
    // 先收集 (doc_id, path, rope, 行数)——Rope 是 Arc 的，clone 很便宜；
    // 边遍历边 `document_mut` 会撞借用。
    let docs: Vec<(
        helix_view::DocumentId,
        std::path::PathBuf,
        helix_core::Rope,
        usize,
    )> = editor
        .documents()
        .map(|doc| {
            (
                doc.id(),
                doc.path().map(normalize).unwrap_or_default(),
                doc.text().clone(),
                doc.text().len_lines(),
            )
        })
        .collect();
    for (doc_id, path, text, lines) in docs {
        let Some(items) = by_file.get(&path) else {
            continue;
        };
        let diags: Vec<Diagnostic> = items
            .iter()
            .filter_map(|(line, col, msg, sev)| {
                // DAP 里同一个 1 起始 → 字符偏移的换算（`Diagnostic.range` 用的是**字符偏移**，
                // 不是 LSP 的 (line,col)）
                let start = helix_view::handlers::dap::dap_pos_to_pos(&text, *line, *col)?;
                let line0 = line.saturating_sub(1).min(lines.saturating_sub(1));
                Some(Diagnostic {
                    range: Range {
                        start,
                        end: (start + 1).min(text.len_chars()),
                    },
                    ends_at_word: false,
                    starts_at_word: false,
                    zero_width: false,
                    line: line0,
                    message: msg.clone(),
                    severity: Some(*sev),
                    code: None,
                    provider: provider.clone(),
                    tags: Vec::new(),
                    source: Some(name.to_string()),
                    data: None,
                })
            })
            .collect();
        total += diags.len();
        if let Some(doc) = editor.document_mut(doc_id) {
            doc.replace_diagnostics(diags, &[], Some(&provider));
        }
    }
    total
}

fn normalize(p: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// `path:line:col: [severity:] message` → (path, line, col, message, severity)
fn parse_gcc_line(line: &str) -> Option<(std::path::PathBuf, usize, usize, String, Severity)> {
    let (path, rest) = line.split_once(':')?;
    let (lineno, rest) = rest.split_once(':')?;
    let (colno, mut msg) = rest.split_once(':')?;
    let line_no: usize = lineno.trim().parse().ok()?;
    let col_no: usize = colno.trim().parse().ok()?;
    msg = msg.trim();
    let mut severity = Severity::Warning;
    // `[warning] …` / `warning: …` 家族
    let lower = msg.to_ascii_lowercase();
    if let Some(stripped) = lower
        .strip_prefix("[warning]")
        .or_else(|| lower.strip_prefix("[error]"))
        .or_else(|| lower.strip_prefix("[info]"))
    {
        severity = match lower.split(']').next().unwrap_or("") {
            "[error]" => Severity::Error,
            "[info]" => Severity::Info,
            _ => Severity::Warning,
        };
        msg = msg[msg.len() - stripped.len()..].trim();
    } else if let Some((head, tail)) = msg.split_once(':') {
        let h = head.trim().to_ascii_lowercase();
        let sev = match h.as_str() {
            "error" => Some(Severity::Error),
            "warning" => Some(Severity::Warning),
            "info" => Some(Severity::Info),
            "note" => Some(Severity::Hint),
            _ => None,
        };
        if let Some(s) = sev {
            severity = s;
            msg = tail.trim();
        }
    }
    if msg.is_empty() {
        return None;
    }
    Some((
        std::path::PathBuf::from(path.trim()),
        line_no,
        col_no,
        msg.to_string(),
        severity,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ruff_concise() {
        let got = parse_gcc_line("a.py:3:5: F401 [*] `os` imported but unused").unwrap();
        assert_eq!(got.1, 3);
        assert_eq!(got.2, 5);
        assert!(got.3.contains("F401"));
        assert_eq!(got.4, Severity::Warning);
    }

    #[test]
    fn parses_yamllint_parsable() {
        let got = parse_gcc_line("/tmp/x.yaml:2:1: [warning] wrong indentation").unwrap();
        assert_eq!(got.4, Severity::Warning);
        assert_eq!(got.3, "wrong indentation");
    }

    #[test]
    fn parses_shellcheck_gcc_error() {
        let got = parse_gcc_line("s.sh:10:3: error: Quote this to prevent word splitting").unwrap();
        assert_eq!(got.4, Severity::Error);
        assert!(got.3.starts_with("Quote this"));
    }

    #[test]
    fn ignores_non_diagnostic_lines() {
        assert!(parse_gcc_line("ruff: checking 3 files").is_none());
        assert!(parse_gcc_line("").is_none());
    }
}
