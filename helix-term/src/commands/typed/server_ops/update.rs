//! `:server update` 分支(从 `server_op` 拆出;见 architecture-audit §9 S2)。

use super::*;

pub(super) fn run(editor: &mut Editor, arg: Option<&str>) -> anyhow::Result<()> {
    let all = registry::all();
    let targets: Vec<registry::Spec> = match arg {
        Some(name) => {
            let spec = registry::get(name).ok_or_else(|| anyhow!("unknown server '{name}'"))?;
            match server_manager::availability(name) {
                Availability::Managed => vec![spec],
                Availability::Local(p) => {
                    return Err(anyhow!(
                        "server '{name}' 由本地提供({}),不走 server manager 升级(受管安装才可 update)",
                        p.display()
                    ));
                }
                Availability::Missing => {
                    return Err(anyhow!("server '{name}' 未安装,用 install"));
                }
            }
        }
        None => all
            .into_iter()
            .filter(|s| {
                matches!(server_manager::availability(&s.name), Availability::Managed)
                    && s.is_installable()
            })
            .collect(),
    };
    if targets.is_empty() {
        editor.set_status("nothing to update");
        return Ok(());
    }
    let (mut updated, mut failed) = (0u32, Vec::new());
    // 写互斥:全量/单个 update(含末尾 rewrite)与后台 worker 写流不交错
    let _op = server_manager::op_lock();
    for spec in targets {
        match resolve_version(&spec, None).and_then(|v| server_manager::update(&spec.name, &v)) {
            Ok(()) => updated += 1,
            Err(e) => failed.push(format!("{}: {e:#}", spec.name)),
        }
    }
    if updated > 0 {
        let _ = server_manager::rewrite_languages_toml();
    }
    if failed.is_empty() {
        editor.set_status(format!("updated {updated} server(s);重启后生效"));
    } else {
        editor.set_error(format!(
            "server update: updated {updated}, failed {} ({})",
            failed.len(),
            failed.join(" | ")
        ));
    }
    Ok(())
}
