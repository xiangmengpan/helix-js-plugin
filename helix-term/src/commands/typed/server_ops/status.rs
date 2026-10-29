//! `:server status` 分支(从 `server_op` 拆出;见 architecture-audit §9 S2)。

use super::*;

pub(super) fn run(editor: &mut Editor) -> anyhow::Result<()> {
    let all = registry::all();
    let (managed_rows, local_rows): (Vec<String>, Vec<String>) = all
        .iter()
        .filter_map(|s| match server_manager::availability(&s.name) {
            Availability::Managed => Some((
                format!(
                    "{}\u{2713}{}",
                    s.name,
                    s.version_detected()
                        .map(|v| format!(" v{v}"))
                        .unwrap_or_default()
                ),
                String::new(),
            )),
            Availability::Local(p) => Some((String::new(), format!("{}⊙{}", s.name, ver_path(&p)))),
            Availability::Missing => None,
        })
        .unzip();
    // 上次失败原因（安装/升级留下的）—— 比只报"没装"有用得多
    let mut last_errors: Vec<String> = Vec::new();
    for spec in &all {
        if let Some((msg, at)) = server_manager::last_error(&spec.name) {
            let when = if at == 0 {
                String::new()
            } else {
                format!(" @{at}")
            };
            last_errors.push(format!("{}{when}: {msg}", spec.name));
        }
    }
    let msg = format!(
        "server-manager: {}/{} 受管/配方;本机已有 {};managed dir: {}\n已装(受管): {}\n本机: {}\nlanguages.toml: {}",
        managed_rows.len(),
        all.len(),
        local_rows.len(),
        server_manager::managed_root().display(),
        if managed_rows.is_empty() {
            "(无)".to_string()
        } else {
            managed_rows.join(", ")
        },
        if local_rows.is_empty() {
            "(无)".to_string()
        } else {
            local_rows.join(", ")
        },
        server_manager::lang_config_file().display()
    );
    let msg = if last_errors.is_empty() {
        msg
    } else {
        format!("{msg}\n上次失败:\n  {}", last_errors.join("\n  "))
    };
    // 多行信息塞状态栏会截断 ⇒ 有失败记录时开成 buffer（复用同一个）
    if last_errors.is_empty() {
        editor.set_status(msg);
        return Ok(());
    }
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
    let body = format!("// server status —— `:bc` 关掉\n\n{msg}\n");
    let transaction = helix_core::Transaction::change(
        doc.text(),
        std::iter::once((0, 0, Some(body.as_str().into()))),
    );
    doc.apply(&transaction, view.id);
    doc.mark_saved_now();
    doc.set_selection(view.id, helix_core::Selection::point(0));
    Ok(())
}
