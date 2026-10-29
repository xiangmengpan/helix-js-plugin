//! `:server update check` —— 问上游"有没有新版本"。
//!
//! 与 `update` 分开：`update` 是**执行**（下载安装，需版本号），`check` 只**查询**
//! （网络 + 缓存 24h），之后 arsenal/`server rows` 的 `upgradable` 就能显示 ▲。
//!
//! 覆盖面受配方限制：只有写了 `latest = "github:owner/repo"`（或返回纯文本版本号的 URL）
//! 的配方能查；其它会被明确列为"配方未给 latest 源"。

use super::*;

pub(super) fn run(editor: &mut Editor) -> anyhow::Result<()> {
    let installed: Vec<registry::Spec> = registry::all()
        .into_iter()
        .filter(|s| matches!(server_manager::availability(&s.name), Availability::Managed))
        .collect();
    if installed.is_empty() {
        editor.set_status("没有受管安装的 server（先 :server install）");
        return Ok(());
    }
    let mut lines: Vec<String> = Vec::new();
    let mut upgradable = 0usize;
    let mut checked = 0usize;
    for spec in &installed {
        let have = server_manager::installed_version(&spec.name)
            .or_else(|| spec.version_detected())
            .unwrap_or_else(|| "?".to_string());
        if spec.latest_source.is_none() {
            lines.push(format!(
                "· {} 已装 {have}：配方未给 `latest` 源，无法查上游",
                spec.name
            ));
            continue;
        }
        match server_manager::fetch_latest(spec) {
            Ok(Some(latest)) => {
                checked += 1;
                let newer = !have.contains(&latest);
                if newer {
                    upgradable += 1;
                }
                lines.push(format!(
                    "{} {} 已装 {have} → 上游 {latest}{}",
                    if newer { "▲" } else { "✓" },
                    spec.name,
                    if newer {
                        "（可升级：:server update "
                    } else {
                        ""
                    }
                ));
                if newer {
                    // 上一行补上闭合提示（保持简短）
                    let last = lines.last_mut().unwrap();
                    last.push_str(&format!("{})", spec.name));
                }
            }
            Ok(None) => lines.push(format!("· {} 已装 {have}：上游没给版本号", spec.name)),
            Err(e) => lines.push(format!("✗ {} 查询失败：{e:#}", spec.name)),
        }
    }
    let head = format!("server update check：{checked} 个查到上游，{upgradable} 个可升级");
    editor.set_status(format!("{head}（详见 buffer）"));

    let reuse = editor
        .server_status_doc
        .filter(|id| editor.documents.contains_key(id));
    match reuse {
        Some(id) => editor.switch(id, helix_view::editor::Action::Replace),
        None => {
            editor.new_file(helix_view::editor::Action::Replace);
            let (_, doc) = helix_view::current!(editor);
            editor.server_status_doc = Some(doc.id());
        }
    }
    let (view, doc) = helix_view::current!(editor);
    let body = format!("// {head} —— `:bc` 关掉\n\n{}\n", lines.join("\n"));
    let transaction = helix_core::Transaction::change(
        doc.text(),
        std::iter::once((0, 0, Some(body.as_str().into()))),
    );
    doc.apply(&transaction, view.id);
    doc.mark_saved_now();
    doc.set_selection(view.id, helix_core::Selection::point(0));
    Ok(())
}
