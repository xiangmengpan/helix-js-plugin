//! JavaScript plugin runtime for the Helix editor (PoC).

pub mod api;
mod commands;
mod fuzzy;
pub mod host;
mod icons;
mod input;
mod layout_api;
mod lsp;
pub mod picker;
mod popup;
mod register;
// pty.rs 是纯 POSIX 实现(openpt / ioctl / winsize + 裸 fd):只在 unix 编译。
// 调用点(shell.rs / popup.rs)各自已有 cfg(unix) / cfg(not(unix)) 分支 ✓
#[cfg(unix)]
mod permissions;
mod pty;
mod shell;
mod state;
// 快照按需序列化的门控(helix-term 的帧路径用):`snapshot_wanted` 问"有人读吗",
// `SnapshotKind` 是位标记的种类,`snapshot_stats` 是"做了几次"的可观测计数。
pub use state::{snapshot_stats, snapshot_wanted, SnapshotKind};
mod theme;
mod types;

pub mod config;
pub mod cursor;
pub mod diagnostics;
pub mod draw;
pub mod server_rows;
pub mod server_tasks;
pub mod watch;

pub use commands::*;
pub use icons::*;
pub use input::{
    dispatch_input_key, dispatch_input_paste, input_edit, input_has_state, input_is_multiline,
    with_input_states, with_input_states_current, InputState,
};
pub use lsp::*;
pub use popup::*;
pub use shell::*;
// G3:`state` 的导出面**显式列出**(原来是 `pub use state::*`)。
//
// 为什么不 glob:glob 让「进程级状态访问器」和「业务 API」在 `helix_js::` 顶层平铺,
// 外部无法分辨边界,而且新增一个内部访问器就会自动变成公共 API。
// 现在新增导出必须在这里加一行 —— 边界是可见的。
pub use state::{
    cache_buffers, cache_configs, cache_diagnostics, cache_layout, cache_pane_mode, cache_panes,
    cache_plugin_overrides, clear_command_args, config_schemas, configs_populated,
    register_component_state, seed_config_schema, set_command_args, set_layouts_dir,
    set_picker_state, set_plugin_roots, set_runtime_dirs, set_term_wake, set_viewport,
    statusline_replaces, statusline_zones, take_messages, take_ui_requests,
    unregister_component_state,
};
// **L3 过渡入口**:helix-term 的"无宿主分支"与 `Drop` 实现拿不到 `&HostState` ⇒
// 这些 `*_current` 是同一批访问器的隐式版本。它们**只**给 helix-term 的边界用,
// L4 删 `ACTIVE` 前必须逐个消灭(`grep -rn "_current(" helix-js/src helix-term/src`)。
pub use state::{
    cache_buffers_current, cache_diagnostics_current, register_component_state_current,
    set_layouts_dir_current, set_plugin_roots_current, set_runtime_dirs_current,
    unregister_component_state_current,
};
// 插件配置合并(纯函数):schema 注册表 + 已缓存用户覆盖 → 合并 JSON。
// 插件 load/reload 之后必须重建一次(见 helix-term `refresh_plugin_configs`)。
pub use config::rebuild_configs;
// `:layout` 类型化命令用的 Rust 层入口(与 helix.layout.* 共用内核与目录)
pub use host::set_editor_version;
// `pub use state::*` 换成本文件里的显式清单后,原先被 glob 顺带带出的 `pub(crate)`
// 项(如 `state::with_terms` / `host::with_engine`)不再出现在 crate 根,单元测试改成
// 从各自模块导入(`src/tests/mod.rs`)。
//
// ⚠ 不要在本行以上写 `#[cfg(test)] pub(crate) use ...`:`cargo xtask api-check` 的解析器
// 把 lib.rs 从**第一个** `#[cfg(test)]` 处截断来跳过测试代码,这里放一个就等于把注册表清零
// (本轮亲手踩过:members=0,报成"d.ts 多出一堆")。
pub use permissions::{
    decide as decide_permission, decisions as permission_decisions, manifests as plugin_manifests,
    pending as pending_permissions,
};
pub use theme::*;
pub use types::DocChange;
pub use types::*;

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsObject, JsString, JsValue};

/// 初始化线程局部运行时（幂等）：消息/UI 队列、事件通道、boa 引擎与全局 `helix` 对象。
pub fn init() {
    state::UI_REQUESTS.get_or_init(Default::default);
    state::with_term_events(|t| {
        if t.is_none() {
            state::js_dbg(format_args!("init channel term"));
            let (tx, rx) = std::sync::mpsc::channel();
            *t = Some(state::wake_sender(tx));
            state::with_term_events_rx(|r| *r = Some(rx));
        }
    });
    state::with_async_events(|t| {
        if t.is_none() {
            state::js_dbg(format_args!("init channel async"));
            let (tx, rx) = std::sync::mpsc::channel();
            *t = Some(state::wake_sender(tx));
            state::with_async_events_rx(|r| *r = Some(rx));
        }
    });
    state::with_lsp_results(|t| {
        if t.is_none() {
            state::js_dbg(format_args!("init channel lsp"));
            let (tx, rx) = std::sync::mpsc::channel();
            *t = Some(state::wake_sender(tx));
            state::with_lsp_results_rx(|r| *r = Some(rx));
        }
    });
    if !crate::host::engine_exists() {
        crate::host::install_default(crate::host::JsHost::new());
    }
}

/// 建引擎并在其中注册 `helix` 全局(逐字搬自 `init()`,仅去掉槽相关的 3 行)。
/// 由 [`crate::host::JsHost::new`] 调用。
/// JS 侧可见的错误:`throw new Error(msg)`(opaque 的 Error 对象)。
///
/// 为什么收成一个函数:这个形状原先在本 crate 出现 **238 次**
/// (`JsError::from_opaque(JsValue::from(JsString::from(msg)))`),占了参数校验代码
/// 一半的宽度,把「校验什么」挤出视线。语义完全不变。
pub(crate) fn js_err(msg: impl Into<String>) -> boa_engine::JsError {
    boa_engine::JsError::from_opaque(boa_engine::JsValue::from(boa_engine::JsString::from(
        msg.into(),
    )))
}

pub(crate) fn build_engine(engine: &mut Context) {
    // 成员与顺序全部来自 `api.rs` 的一张表（`API` + `ATTACH`）。
    // 这里只做三件事：建命名空间对象 → 交给根对象 → 注册全局。
    let attr = Attribute::READONLY | Attribute::NON_ENUMERABLE;

    // 命名空间对象先各自构建（`ObjectInitializer` 独占 `&mut Context`）
    let mut ns_objs: Vec<JsObject> = Vec::new();
    for a in api::ATTACH {
        if let api::Attach::Ns(ns) = a {
            ns_objs.push(api::register_ns(engine, ns));
        }
    }
    let plugin_fn = api::build_plugin(engine);

    let mut builder = ObjectInitializer::new(engine);
    api::register_root(&mut builder);

    let mut ns_iter = ns_objs.into_iter();
    for a in api::ATTACH {
        match a {
            api::Attach::Ns(ns) => {
                let obj = ns_iter.next().expect("命名空间对象与 ATTACH 顺序一致");
                builder.property(JsString::from(*ns), obj, attr);
            }
            api::Attach::Plugin => {
                builder.property(JsString::from("plugin"), plugin_fn.clone(), attr);
            }
            api::Attach::ApiVersion => {
                builder.property(
                    JsString::from("api_version"),
                    JsValue::from(crate::types::API_VERSION),
                    attr,
                );
            }
        }
    }

    let helix = builder.build();
    engine
        .register_global_property(JsString::from("helix"), helix, attr)
        .expect("register helix object");
    api::register_globals(engine);
}

#[cfg(test)]
pub(crate) mod tests;
