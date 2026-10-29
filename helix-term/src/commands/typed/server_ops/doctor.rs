//! `:server doctor [name]` —— 自检：能不能跑、被谁引用、为什么没生效。
//!
//! 为什么需要它：`:server status` 只回答"装没装"。用户真正的疑问是
//! **"我装了，为什么没生效？"** —— 绝大多数情况是"没有语言引用它"（配方装了、
//! `language-servers` 里没写），而这个信息以前只能靠手翻 `languages.toml`。

use super::*;

/// 一条自检结论。
struct Check {
    ok: bool,
    what: String,
    detail: String,
}

/// 一个语言配置的"引用快照"：谁挂了哪些 server / formatter / debugger。
struct Effective {
    /// (语言名, language-servers 里的名字)
    lsp_refs: Vec<(String, String)>,
    /// (语言名, formatter 命令的 basename)
    fmt_refs: Vec<(String, String)>,
    /// (语言名, debugger 名)
    dap_refs: Vec<(String, String)>,
}

fn effective(editor: &Editor) -> Effective {
    let loader: &helix_core::syntax::Loader = &editor.syn_loader.load();
    let mut out = Effective {
        lsp_refs: Vec::new(),
        fmt_refs: Vec::new(),
        dap_refs: Vec::new(),
    };
    for cfg in loader.language_configs() {
        for ls in &cfg.language_servers {
            out.lsp_refs
                .push((cfg.language_id.clone(), ls.name.clone()));
        }
        if let Some(f) = &cfg.formatter {
            let base = std::path::Path::new(&f.command)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.command.clone());
            out.fmt_refs.push((cfg.language_id.clone(), base));
        }
        if let Some(d) = &cfg.debugger {
            out.dap_refs.push((cfg.language_id.clone(), d.name.clone()));
        }
    }
    out
}

pub(super) fn run(editor: &mut Editor, name: Option<&str>) -> anyhow::Result<()> {
    let specs: Vec<registry::Spec> =
        match name {
            Some(n) => vec![registry::get(n)
                .ok_or_else(|| anyhow!("unknown server '{n}'（见 :server list）"))?],
            None => registry::all(),
        };
    if specs.is_empty() {
        editor.set_status("server registry is empty");
        return Ok(());
    }
    let langs_path = read_langs_path();
    let eff = effective(editor);
    let mut lines: Vec<String> = vec![format!(
        "// server doctor —— 读的是 {}",
        langs_path.display()
    )];
    let mut problems = 0usize;
    for spec in &specs {
        let checks = check_spec(spec, &eff);
        let bad = checks.iter().filter(|c| !c.ok).count();
        problems += bad;
        lines.push(String::new());
        lines.push(format!(
            "{} ({}){}",
            spec.name,
            kind_name(spec.kind),
            if bad == 0 { "" } else { "  ← 有问题" }
        ));
        for c in &checks {
            lines.push(format!(
                "  {} {:<14} {}",
                if c.ok { "✓" } else { "✗" },
                c.what,
                c.detail
            ));
        }
    }
    lines.push(String::new());
    lines.push(if problems == 0 {
        "// 全部通过".to_string()
    } else {
        format!("// {problems} 项需要处理（✗ 标出的那几行）")
    });

    // 复用同一个 buffer
    let text = lines.join("\n") + "\n";
    let reuse = editor
        .server_doctor_doc
        .filter(|id| editor.documents.contains_key(id));
    match reuse {
        Some(id) => editor.switch(id, helm_view_action()),
        None => {
            editor.new_file(helm_view_action());
            let (_, doc) = helix_view::current!(editor);
            editor.server_doctor_doc = Some(doc.id());
        }
    }
    let (view, doc) = helix_view::current!(editor);
    let transaction = helix_core::Transaction::change(
        doc.text(),
        std::iter::once((0, 0, Some(text.as_str().into()))),
    );
    doc.apply(&transaction, view.id);
    doc.mark_saved_now();
    doc.set_selection(view.id, helix_core::Selection::point(0));
    Ok(())
}

fn helm_view_action() -> helix_view::editor::Action {
    helix_view::editor::Action::Replace
}

fn kind_name(k: registry::Kind) -> &'static str {
    match k {
        registry::Kind::Lsp => "lsp",
        registry::Kind::Dap => "dap",
        registry::Kind::Linter => "linter",
        registry::Kind::Formatter => "formatter",
    }
}

/// 报告头里显示的那个路径（`:server` 会往这里写）。
///
/// 注意：**判断"有没有被引用"用的是 `effective()`（已加载的语言配置）**，
/// 而不是读文件 —— 很多人没有 `~/.config/helix/languages.toml`，
/// adapter/formatter 全来自发行版自带那份，只看用户文件会误报。
fn read_langs_path() -> PathBuf {
    server_manager::lang_config_file()
}

fn check_spec(spec: &registry::Spec, eff: &Effective) -> Vec<Check> {
    let mut out = Vec::new();
    // ① 可用性
    let (bin, avail) = match server_manager::availability(&spec.name) {
        server_manager::Availability::Managed => {
            (Some(spec.bin_path()), "受管（server-manager 装的）")
        }
        server_manager::Availability::Local(p) => (Some(p), "本机已有（PATH 里）"),
        server_manager::Availability::Missing => (None, "未安装"),
    };
    out.push(Check {
        ok: bin.is_some(),
        what: "可执行".into(),
        detail: match &bin {
            Some(p) => format!("{}（{avail}）", p.display()),
            None => format!(
                "{avail}{}",
                if spec.is_installable() {
                    format!("；可 :server install {}", spec.name)
                } else {
                    "；配方未配置下载源".to_string()
                }
            ),
        },
    });
    // ② 能不能执行（**不**以 `--version` 为判据：lldb-dap 这类服务器会一直等 stdin）
    if let Some(p) = &bin {
        let exec = is_executable(p);
        let v = if exec {
            server_manager::detect_version(p)
        } else {
            None
        };
        out.push(Check {
            ok: exec,
            what: "可执行位".into(),
            detail: if !exec {
                format!("{} 不可执行（perm 0644？挂载 noexec？）", p.display())
            } else if let Some(v) = v {
                format!("`--version` → {v}")
            } else {
                "可执行（它不报版本 —— 服务器类二进制正常，如 lldb-dap）".to_string()
            },
        });
    }
    // ③ 有没有被配置引用（这才是"装了不生效"的头号原因）
    match spec.kind {
        registry::Kind::Lsp => {
            let langs: Vec<&str> = eff
                .lsp_refs
                .iter()
                .filter(|(_, n)| n == &spec.name)
                .map(|(l, _)| l.as_str())
                .collect();
            let referenced = !langs.is_empty();
            out.push(Check {
                ok: referenced,
                what: "被引用".into(),
                detail: if referenced {
                    format!("{} 的 language-servers 里挂了它", langs.join(", "))
                } else {
                    "没有任何语言的 language-servers 指向它 ⇒ 打开文件也不会启动它（:server unmanage 可停用自动挂接）"
                        .to_string()
                },
            });
        }
        registry::Kind::Dap => {
            let langs: Vec<&str> = eff
                .dap_refs
                .iter()
                .filter(|(_, n)| n == &spec.name)
                .map(|(l, _)| l.as_str())
                .collect();
            let referenced = !langs.is_empty();
            out.push(Check {
                ok: referenced,
                what: "被引用".into(),
                detail: if referenced {
                    format!("{} 的 debugger 指向它", langs.join(", "))
                } else {
                    "没有语言的 debugger 指向它 ⇒ `:debug-start` 找不到 adapter".into()
                },
            });
        }
        registry::Kind::Formatter => {
            let langs: Vec<&str> = eff
                .fmt_refs
                .iter()
                .filter(|(_, b)| b == &spec.bin_name || b == &spec.name)
                .map(|(l, _)| l.as_str())
                .collect();
            let referenced = !langs.is_empty();
            out.push(Check {
                ok: referenced,
                what: "被引用".into(),
                detail: if referenced {
                    format!("{} 的 formatter 指向它", langs.join(", "))
                } else {
                    "没有语言的 formatter 指向它 ⇒ `:format` 不会用它（装完会自动写，若仍缺失看 :server status 的 conflicts）".into()
                },
            });
        }
        registry::Kind::Linter => {
            // linter 不是"没接线"，而是核心没有 lint 通道 —— 说清楚，别让人等
            out.push(Check {
                ok: false,
                what: "接线方式".into(),
                detail: "核心没有\"跑外部 linter → 诊断\"的通道；请用它的 LSP 模式（多数 linter 都支持，如 ruff/biome/deno）接到 language-servers".into(),
            });
        }
    }
    out
}

/// 文件存在且有执行位（Unix）；其它平台只看存在。
fn is_executable(p: &std::path::Path) -> bool {
    match std::fs::metadata(p) {
        Ok(m) => {
            if !m.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        }
        Err(_) => false,
    }
}
