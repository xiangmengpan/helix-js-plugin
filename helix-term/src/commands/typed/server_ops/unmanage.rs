//! `:server unmanage` 分支(从 `server_op` 拆出;见 architecture-audit §9 S2)。

use super::*;

pub(super) fn run(editor: &mut Editor, arg: Option<&str>) -> anyhow::Result<()> {
    let Some(name) = arg else {
        return Err(anyhow!("usage: server unmanage <name>"));
    };
    match server_manager::availability(name) {
        Availability::Local(_) => {
            let on = !server_manager::ignored_local().contains(&name.to_string());
            // set_ignored + rewrite 整 op 持写锁
            let _op = server_manager::op_lock();
            server_manager::set_ignored(name, on)?;
            let _ = server_manager::rewrite_languages_toml();
            editor.set_status(if on {
                format!("'{name}' 已停用自动挂接(本地工具保留);server unmanage {name} 恢复")
            } else {
                format!("'{name}' 已恢复自动挂接")
            });
        }
        Availability::Managed => {
            return Err(anyhow!("server '{name}' 是受管安装,用 remove 卸载"));
        }
        Availability::Missing => {
            return Err(anyhow!("server '{name}' 未安装/不在 PATH"));
        }
    }
    Ok(())
}
