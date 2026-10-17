//! `:server list` 分支(从 `server_op` 拆出;见 architecture-audit §9 S2)。

use super::*;

pub(super) fn run(editor: &mut Editor) -> anyhow::Result<()> {
    let all = registry::all();
    let mut parts = Vec::new();
    for spec in &all {
        let mark = match server_manager::availability(&spec.name) {
            Availability::Managed => {
                let v = spec
                    .version_detected()
                    .map(|s| format!("@{s}"))
                    .unwrap_or_default();
                format!("\u{2713}{}{v}", spec.name)
            }
            Availability::Local(p) => {
                format!("{}⊙(本机{})", spec.name, ver_path(&p))
            }
            Availability::Missing if spec.is_installable() => {
                format!("{}-（可装）", spec.name)
            }
            Availability::Missing => format!("{}-（无源）", spec.name),
        };
        parts.push(mark);
    }
    editor.set_status(format!("servers[{}]: {}", all.len(), parts.join("  ")));
    Ok(())
}
