//! `:server install` 分支(从 `server_op` 拆出;见 architecture-audit §9 S2)。

use super::*;

pub(super) fn run(
    editor: &mut Editor,
    arg: Option<&str>,
    arg2: Option<&str>,
) -> anyhow::Result<()> {
    let Some(name) = arg else {
        return Err(anyhow!("usage: server install <name> [version]"));
    };
    let spec =
        registry::get(name).ok_or_else(|| anyhow!("unknown server '{name}'(见 server list)"))?;
    match server_manager::availability(name) {
        Availability::Managed => {
            let v = spec
                .version_detected()
                .map(|s| format!(" v{s}"))
                .unwrap_or_default();
            return Err(anyhow!(
                "server '{name}' 已安装({v});升级用 server update {name}"
            ));
        }
        Availability::Local(p) => {
            // 本地已可用:直接使用,不下载安装
            editor.set_status(format!(
                "server '{name}' 本地已可用({}){},无需安装;卸载本地工具不受 server manager 管",
                p.display(),
                ver_path(&p)
            ));
            return Ok(());
        }
        Availability::Missing => {}
    }
    if !spec.is_installable() {
        return Err(anyhow!(
            "server '{name}': 下载源未配置;内置配方待录,或配 [server-manager.registry.{name}] url/version"
        ));
    }
    let version = resolve_version(&spec, arg2)?;
    // 写互斥(整 op 含 rewrite):与后台 worker / :server 其它写流不交错
    let _op = server_manager::op_lock();
    server_manager::install(name, &version)?;
    let rw = server_manager::rewrite_languages_toml()?;
    let mut msg = format!(
        "installed '{name}' {version} -> {}",
        spec.bin_path().display()
    );
    if rw.written {
        msg.push_str(" | languages.toml 已更新(重启后生效)");
    }
    if !rw.conflicts.is_empty() {
        msg.push_str(" | 提示: ");
        msg.push_str(&rw.conflicts.join("; "));
    }
    editor.set_status(msg);
    Ok(())
}
