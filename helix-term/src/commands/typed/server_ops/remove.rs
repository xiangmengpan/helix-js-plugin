//! `:server remove` 分支(从 `server_op` 拆出;见 architecture-audit §9 S2)。

use super::*;

pub(super) fn run(editor: &mut Editor, arg: Option<&str>) -> anyhow::Result<()> {
    let Some(name) = arg else {
        return Err(anyhow!("usage: server remove <name>"));
    };
    match server_manager::availability(name) {
        Availability::Managed => {
            // remove 内部含 languages.toml 重写:整 op 持写锁
            let _op = server_manager::op_lock();
            // 跨进程：另一个 hx 正在写同一棵 managed 目录 ⇒ 直接失败（不带病继续写）
            let _flock = server_manager::file_lock()?;
            server_manager::remove(name)?;
            editor.set_status(format!("removed '{name}';若正被 LSP 使用,重启后生效"));
        }
        Availability::Local(p) => {
            return Err(anyhow!(
                "server '{name}' 是本地工具({}),不能卸载;如需停用自动挂接用 server unmanage {name}",
                p.display()
            ));
        }
        Availability::Missing => {
            return Err(anyhow!("server '{name}' 未安装"));
        }
    }
    Ok(())
}
