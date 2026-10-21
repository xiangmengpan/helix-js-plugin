//! 服务器管理请求(3 个 arm;`ServerListRows` 是其中最大的一块,85 行)。
//!
//! 从 `typed.rs` 的 `apply_ui_requests` 拆出(G7):每个 arm 变成一个 handler,
//! 顶层只留分派。请求结构体的字段解构写在 handler 签名里,handler 体内是原 arm 的原文。

use super::*;

pub(crate) fn handle_server_op(ServerOpReq { op, arg }: ServerOpReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |editor, _compositor, mut _js| {
        if let Err(e) = server_op(editor, &op, arg.as_deref(), None) {
            editor.set_error(format!("server: {e}"));
        }
    });
    Ok(())
}

pub(crate) fn handle_server_list_rows(
    js: &mut Option<&mut helix_js::host::JsHost>,
    ServerListRowsReq { id }: ServerListRowsReq,
) -> anyhow::Result<()> {
    // 面板行数据:同步算好回投(无 editor 依赖);config 扩展配方先重读
    if let Err(e) =
        server_manager::apply_server_config(server_manager::disk_server_manager_table().as_ref())
    {
        log::error!("server rows config: {e:#}");
    }
    let rows: Vec<helix_js::ServerRow> = server_manager::registry::all()
        .into_iter()
        .map(|spec| {
            use server_manager::Availability;
            // 一次 availability 判定,分支内取齐 installed/local/version/upgradable/bin:
            // Managed → 受管已装(version=--version 检测串;upgradable 比较;bin=软链路径);
            // Local(p) → 本机已有(bin=PATH 绝对路径);Missing → 全空。
            let avail = server_manager::availability(&spec.name);
            // 版本串剥 "<bin_name> " 前缀(--version 常输出整行如
            // "rust-analyzer 1.82.0")只留版本号:状态列/信息弹窗不再重复拼 bin 名。
            // 非该前缀(如裸版本号)原样保留。
            let strip_bin_prefix = |v: Option<String>| -> Option<String> {
                let core = v
                    .as_deref()
                    .and_then(|s| s.strip_prefix(&spec.bin_name)?.strip_prefix(' '));
                core.map(str::to_string).or(v)
            };
            let (installed, local, version, upgradable, bin) = match avail {
                Availability::Managed => (
                    true,
                    false,
                    strip_bin_prefix(spec.version_detected()),
                    server_manager::is_upgradable(&spec),
                    Some(spec.bin_path().to_string_lossy().into_owned()),
                ),
                // Local(本机已有):只做 PATH 存在性判定,不 spawn `--version` 探测——
                // 本机工具可能很慢/挂起,rows 快照在主线程同步算,逐工具 spawn 会让
                // arsenal 打开/刷新长时间冻结 UI(detect_version 现带 2s 超时+缓存兜底)。
                // 状态列本机行显示 `local`,不带检测版本(与初版设计一致)。
                Availability::Local(p) => (
                    true,
                    true,
                    None,
                    false,
                    Some(p.to_string_lossy().into_owned()),
                ),
                Availability::Missing => (false, false, None, false, None),
            };
            // 下载源仅供显示:archive 有 url → 原文;Tool/惰性 → None
            let source = match &spec.install {
                server_manager::registry::Install::Archive { url_template, .. }
                    if !url_template.is_empty() =>
                {
                    Some(url_template.clone())
                }
                _ => None,
            };
            let kind = match spec.kind {
                server_manager::registry::Kind::Lsp => "lsp",
                server_manager::registry::Kind::Dap => "dap",
                server_manager::registry::Kind::Linter => "linter",
                server_manager::registry::Kind::Formatter => "formatter",
            }
            .to_string();
            helix_js::ServerRow {
                name: spec.name.clone(),
                kind,
                languages: spec.languages.clone(),
                installed,
                local,
                version,
                description: spec.description.clone(),
                homepage: spec.homepage.clone(),
                installable: spec.is_installable(),
                upgradable,
                needs_version: server_manager::needs_version(&spec),
                bin,
                source,
            }
        })
        .collect();
    let delivered = match js.as_mut() {
        Some(host) => host.deliver_server_rows(id, rows),
        None => helix_js::server_rows::deliver(id, rows),
    };
    if let Err(e) = delivered {
        log::error!("server rows deliver: {e:#}");
    }
    Ok(())
}

pub(crate) fn handle_server_task(
    ServerTaskReq { task_id, items }: ServerTaskReq,
) -> anyhow::Result<()> {
    // 后台任务批次:入队单 worker(串行)即返回,不阻塞主线程;事件异步回投
    let mapped: Vec<(String, String, Option<String>)> = items
        .into_iter()
        .map(|i| (i.op, i.name, i.version))
        .collect();
    crate::commands::server_tasks::submit(task_id, mapped);
    Ok(())
}
