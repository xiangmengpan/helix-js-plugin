//! `:server sync [--check] [--prune]` —— 按 `.helix/servers.toml` 对齐环境。
//!
//! 这是"命令式安装器"与"可复现环境"的分界线：
//! - `--check`：只报告差异，不动磁盘；
//! - 默认：装缺的（版本取 `[version]` → 配方固定版本），然后重写 `languages.toml`；
//! - `--prune`：额外卸掉"装了但清单里没有"的受管 server（默认**不**卸，避免误删）。

use super::*;

pub(super) fn run(
    editor: &mut Editor,
    arg: Option<&str>,
    arg2: Option<&str>,
) -> anyhow::Result<()> {
    let mut check_only = false;
    let mut prune = false;
    // 用**子词**而不是 `--flag`：`:server` 的 signature 不允许任意 flag
    // （`unknown flag '--check'`），而且和 `install/update` 的位置参数风格一致。
    for a in [arg, arg2].into_iter().flatten() {
        match a {
            "check" => check_only = true,
            "prune" => prune = true,
            other => {
                return Err(anyhow!(
                    "server sync: 未知参数 '{other}'（用法: :server sync [check] [prune]）"
                ))
            }
        }
    }
    let Some(desired) = server_manager::project::desired()? else {
        editor.set_status(format!(
            "没有 {} —— 在项目里写 `server = [\"rust-analyzer\", …]` 再 sync",
            server_manager::project::config_path().display()
        ));
        return Ok(());
    };
    // 已有的受管 server
    let installed: Vec<String> = registry::all()
        .into_iter()
        .filter(|s| matches!(server_manager::availability(&s.name), Availability::Managed))
        .map(|s| s.name)
        .collect();

    let mut report: Vec<String> = Vec::new();
    let mut install_list: Vec<(String, String)> = Vec::new();
    for name in &desired.servers {
        let Some(spec) = registry::get(name) else {
            report.push(format!(
                "✗ {name}: 配方里没有这个名字（:server search {name}）"
            ));
            continue;
        };
        if installed.contains(name) {
            report.push(format!("✓ {name}: 已安装"));
            continue;
        }
        if let server_manager::Availability::Local(p) = server_manager::availability(name) {
            report.push(format!("✓ {name}: 本机已有（{}），无需装", p.display()));
            continue;
        }
        let version = desired
            .versions
            .get(name)
            .cloned()
            .or_else(|| spec.version.clone());
        match version {
            Some(v) => {
                install_list.push((name.clone(), v.clone()));
                report.push(format!("＋ {name}: 待安装 v{v}"));
            }
            None => report.push(format!(
                "✗ {name}: 配方未固定版本 ⇒ 在 .helix/servers.toml 的 [version] 里写 {name} = \"…\""
            )),
        }
    }
    let mut remove_list: Vec<String> = Vec::new();
    if prune {
        remove_list = installed
            .iter()
            .filter(|n| !desired.servers.contains(n))
            .cloned()
            .collect();
        for n in &remove_list {
            report.push(format!("－ {n}: 待卸载（清单里没有）"));
        }
    } else {
        for n in installed.iter().filter(|n| !desired.servers.contains(n)) {
            report.push(format!("· {n}: 已装但清单没有（要卸加 --prune）"));
        }
    }

    let head = if check_only {
        "server sync --check"
    } else {
        "server sync"
    };
    let changed = !install_list.is_empty() || !remove_list.is_empty();
    if check_only || !changed {
        editor.set_status(format!(
            "{head}: {} 项；{}",
            report.len(),
            if changed {
                "有差异（去掉 check 执行）"
            } else {
                "已对齐"
            }
        ));
        show(editor, head, &report, true)?;
        return Ok(());
    }

    // 真正执行：先卸（避免同一目录被两边写），再装，最后重写 languages.toml
    let _op = server_manager::op_lock();
    let _flock = server_manager::file_lock()?;
    for n in &remove_list {
        match server_manager::remove(n) {
            Ok(_) => report.push(format!("✓ {n}: 已卸载")),
            Err(e) => report.push(format!("✗ {n}: 卸载失败 {e:#}")),
        }
    }
    for (n, v) in &install_list {
        match server_manager::install(n, v) {
            Ok(()) => {
                server_manager::clear_error(n);
                report.push(format!("✓ {n}: 已安装 v{v}"));
            }
            Err(e) => {
                server_manager::record_error(n, &format!("{e:#}"));
                report.push(format!("✗ {n}: 安装失败 {e:#}"));
            }
        }
    }
    match server_manager::rewrite_languages_toml() {
        Ok(rw) => {
            if rw.written {
                report.push("✓ languages.toml 已重写（重启后生效）".to_string());
            }
            report.extend(rw.conflicts.iter().map(|c| format!("! {c}")));
        }
        Err(e) => report.push(format!("✗ 写 languages.toml 失败: {e:#}")),
    }
    editor.set_status(format!("{head}: 完成（{} 项，详见 buffer）", report.len()));
    show(editor, head, &report, false)?;
    Ok(())
}

/// 结果开成 buffer（复用同一个）—— 多行结果塞状态栏会被截断。
fn show(editor: &mut Editor, head: &str, lines: &[String], check: bool) -> anyhow::Result<()> {
    let reuse = editor
        .server_sync_doc
        .filter(|id| editor.documents.contains_key(id));
    match reuse {
        Some(id) => editor.switch(id, helix_view::editor::Action::Replace),
        None => {
            editor.new_file(helix_view::editor::Action::Replace);
            let (_, doc) = helix_view::current!(editor);
            editor.server_sync_doc = Some(doc.id());
        }
    }
    let (view, doc) = helix_view::current!(editor);
    let body = format!(
        "// {head} —— 清单：{}{}\n\n{}\n",
        server_manager::project::config_path().display(),
        if check {
            "（check：没有改动磁盘）"
        } else {
            ""
        },
        lines.join("\n")
    );
    let transaction = helix_core::Transaction::change(
        doc.text(),
        std::iter::once((0, 0, Some(body.as_str().into()))),
    );
    doc.apply(&transaction, view.id);
    doc.mark_saved_now();
    doc.set_selection(view.id, helix_core::Selection::point(0));
    Ok(())
}
