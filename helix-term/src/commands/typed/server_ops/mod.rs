//! `:server` 子命令与 JS `helix.server.<op>` 的实现(S2:从 282 行的 `server_op` 拆出)。
//!
//! 拆法与 G7 的 `apply_ui_requests` 同构:**一个子命令一个文件**,arm 体逐行搬过去,
//! 顶层只留「重读配置 → 分派」。原始分支体没有改任何判断条件。
//!
//! 两个共用小工具(`resolve_version` / `ver_path`)原本是 `server_op` 里的闭包,搬成模块级
//! 函数 —— 它们本来就不捕获任何东西。

use super::*;

use server_manager::registry;
use server_manager::Availability;

mod doctor;
mod install;
mod list;
mod log;
mod remove;
mod search;
mod status;
mod sync;
mod unmanage;
mod update;

/// server 操作核心(`:server` 命令与 JS API `helix.server.<op>` 共用)。
/// 每次操作前从 config 的 `[server-manager]` 段重读 mirror/registry 配方(改配置即生效)。
pub(crate) fn server_op(
    editor: &mut Editor,
    sub: &str,
    arg: Option<&str>,
    arg2: Option<&str>,
) -> anyhow::Result<()> {
    server_manager::apply_server_config(server_manager::disk_server_manager_table().as_ref())
        .map_err(|e| anyhow!("config [server-manager]: {e:#}"))?;
    match sub {
        "list" => list::run(editor),
        "search" => search::run(editor, arg),
        "install" => install::run(editor, arg, arg2),
        "update" => update::run(editor, arg),
        "remove" => remove::run(editor, arg),
        "unmanage" | "ignore" => unmanage::run(editor, arg),
        "status" => status::run(editor),
        "doctor" => doctor::run(editor, arg),
        "log" => log::run(editor, arg, arg2),
        "sync" => sync::run(editor, arg, arg2),
        other => Err(anyhow!(
            "unknown server subcommand '{other}'(list|search|install|update|remove|unmanage|status|doctor|log|sync|panel)"
        )),
    }
}

/// 版本解析:显式参数 > 配方固定 version > 报错(要求显式给版本)。
fn resolve_version(spec: &registry::Spec, explicit: Option<&str>) -> anyhow::Result<String> {
    if let Some(v) = explicit {
        return Ok(v.to_string());
    }
    if let Some(v) = &spec.version {
        return Ok(v.clone());
    }
    Err(anyhow!(
        "server '{}': 配方未给固定 version;用 server install <name> <version> 或 config 加 version",
        spec.name
    ))
}

/// 本机二进制的可读版本(取不到就不显示)
fn ver_path(p: &std::path::Path) -> String {
    server_manager::detect_version(p)
        .map(|v| format!(" v{v}"))
        .unwrap_or_default()
}
