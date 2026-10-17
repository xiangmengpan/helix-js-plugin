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
    editor.set_status(msg);
    Ok(())
}
