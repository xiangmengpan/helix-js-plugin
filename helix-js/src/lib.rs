//! JavaScript plugin runtime for the Helix editor (PoC).

mod commands;
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
    with_input_states, InputState,
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
    cache_plugin_overrides, clear_command_args, config_schemas, register_component_state,
    seed_config_schema, set_command_args, set_layouts_dir, set_picker_state, set_plugin_roots,
    set_runtime_dirs, set_term_wake, set_viewport, statusline_replaces, statusline_zones,
    take_messages, take_ui_requests, unregister_component_state,
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
use boa_engine::{Context, JsString, JsValue};

/// 初始化线程局部运行时（幂等）：消息/UI 队列、事件通道、boa 引擎与全局 `helix` 对象。
pub fn init() {
    state::UI_REQUESTS.get_or_init(Default::default);
    let _ = state::LAST_LAYOUT.get_or_init(Default::default);
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
    // 各命名空间先独立构建(`ObjectInitializer` 独占 `&mut Context`),再挂到 helix 上。
    // 成员注册表在 `register/*`:这里只负责**拼装顺序**,不再写 134 份样板。
    let lsp_obj = register::lsp::register(engine);
    let picker_obj = register::picker::register(engine);
    let plugin_fn = register::plugin::build(engine);
    let server_obj = register::server::register(engine);
    let pane_obj = register::pane::register(engine);
    let layout_obj = register::pane_layout::register(engine);
    let pane_mode_obj = register::pane_layout::register_pane_mode(engine);
    let icons_obj = register::icons::register(engine);
    let buffer_obj = register::buffer::register(engine);
    let stack_obj = register::buffer::register_stack(engine);
    let draw_obj = register::draw::register(engine);
    let text_obj = register::draw::register_text(engine);

    // 根对象成员按能力分族注册(每族一个文件,顺序即属性插入顺序)
    let mut builder = ObjectInitializer::new(engine);
    register::root_doc::register(&mut builder);
    register::root_fs::register(&mut builder);
    register::root_buffer::register(&mut builder);
    register::root_term::register(&mut builder);
    register::root_ui::register(&mut builder);
    register::root_theme::register(&mut builder);

    builder.property(
        JsString::from("lsp"),
        lsp_obj,
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    builder.property(
        JsString::from("picker"),
        picker_obj,
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    builder.property(
        JsString::from("plugin"),
        plugin_fn,
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    builder.property(
        JsString::from("server"),
        server_obj,
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    builder.property(
        JsString::from("pane"),
        pane_obj,
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    builder.property(
        JsString::from("icons"),
        icons_obj,
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    builder.property(
        JsString::from("stack"),
        stack_obj,
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    builder.property(
        JsString::from("draw"),
        draw_obj,
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    builder.property(
        JsString::from("text"),
        text_obj,
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    builder.property(
        JsString::from("buffer"),
        buffer_obj,
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    builder.property(
        JsString::from("layout"),
        layout_obj,
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    builder.property(
        JsString::from("pane_mode"),
        pane_mode_obj,
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    // 边界协议的线格式版本(B-1/B-2 同一份):插件可在运行时分支
    // (`if (helix.api_version >= 2) …`),而不必靠"猜哪个版本引入了哪个字段"。
    builder.property(
        JsString::from("api_version"),
        JsValue::from(crate::types::API_VERSION),
        Attribute::READONLY | Attribute::NON_ENUMERABLE,
    );
    let helix = builder.build();
    engine
        .register_global_property(
            JsString::from("helix"),
            helix,
            Attribute::READONLY | Attribute::NON_ENUMERABLE,
        )
        .expect("register helix object");
    // 布局标签条组件 id(与 helix-term::compositor::TABBAR_ID 一致)
    engine
        .register_global_property(
            JsString::from("TABBAR_ID"),
            JsValue::from(0x7ABB_0001_u64),
            Attribute::READONLY | Attribute::NON_ENUMERABLE,
        )
        .expect("register TABBAR_ID");
}

#[cfg(test)]
pub(crate) mod tests;
