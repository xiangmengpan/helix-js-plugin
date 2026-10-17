//! JavaScript plugin runtime for the Helix editor (PoC).

mod commands;
pub mod host;
mod icons;
mod input;
mod layout;
mod lsp;
pub mod picker;
mod popup;
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
pub mod server_rows;
pub mod server_tasks;
pub mod watch;

pub use commands::*;
pub use icons::*;
pub use input::{
    clear_popup_inputs, dispatch_input_key, dispatch_input_paste, input_edit, input_has_state,
    input_is_multiline, js_set_input_value, with_input_states, InputState,
};
pub use lsp::*;
pub use popup::*;
pub use shell::*;
pub use state::*;
// `:layout` 类型化命令用的 Rust 层入口(与 helix.layout.* 共用内核与目录)
pub use host::set_editor_version;
pub use layout::{layout_delete, layout_list, layout_load, layout_save};
pub use permissions::{
    decide as decide_permission, decisions as permission_decisions, manifests as plugin_manifests,
    pending as pending_permissions,
};
pub use theme::*;
pub use types::DocChange;
pub use types::*;

use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsString, JsValue, NativeFunction};

/// 初始化线程局部运行时（幂等）：消息/UI 队列、事件通道、boa 引擎与全局 `helix` 对象。
pub fn init() {
    state::UI_REQUESTS.get_or_init(Default::default);
    let _ = state::LAST_LAYOUT.get_or_init(Default::default);
    state::with_term_events(|t| {
        if t.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            *t = Some(state::wake_sender(tx));
            state::with_term_events_rx(|r| *r = Some(rx));
        }
    });
    state::with_async_events(|t| {
        if t.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            *t = Some(state::wake_sender(tx));
            state::with_async_events_rx(|r| *r = Some(rx));
        }
    });
    state::with_lsp_results(|t| {
        if t.is_none() {
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
pub(crate) fn build_engine(engine: &mut Context) {
    // helix.lsp 命名空间：先独立构建对象（ObjectInitializer 独占 &mut Context），
    // 再作为属性挂到 helix 对象上
    let lsp_obj = {
        let mut lsp_builder = ObjectInitializer::new(engine);
        lsp_builder
            .function(
                NativeFunction::from_fn_ptr(lsp::js_lsp_hover),
                JsString::from("hover"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(lsp::js_lsp_completion),
                JsString::from("completion"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(lsp::js_lsp_goto_definition),
                JsString::from("goto_definition"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(lsp::js_lsp_document_symbols),
                JsString::from("document_symbols"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(lsp::js_lsp_format),
                JsString::from("format"),
                0,
            )
            .function(
                NativeFunction::from_fn_ptr(lsp::js_lsp_rename),
                JsString::from("rename"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(lsp::js_lsp_code_actions),
                JsString::from("code_actions"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(lsp::js_lsp_execute_code_action),
                JsString::from("execute_code_action"),
                1,
            );
        lsp_builder.build()
    };
    // helix.picker 命名空间：同 helix.lsp，独立构建后挂到 helix 对象上
    let picker_obj = {
        let mut picker_builder = ObjectInitializer::new(engine);
        picker_builder
            .function(
                NativeFunction::from_fn_ptr(picker::js_picker_define),
                JsString::from("define"),
                2,
            )
            .function(
                NativeFunction::from_fn_ptr(picker::js_picker_run),
                JsString::from("run"),
                1,
            );
        picker_builder.build()
    };
    // helix.plugin 命名空间:plugin(name, deps) 可调用函数 + install/update/remove 属性
    let plugin_fn = FunctionObjectBuilder::new(
        engine.realm(),
        NativeFunction::from_fn_ptr(commands::js_plugin),
    )
    .name("plugin")
    .length(2)
    .build(); // JsFunction(可调用)
              // 函数对象上加管理方法属性(Deref → JsObject::set;属性值为 JsFunction)
              // helix.server 命名空间:list/search/install/update/remove/status(镜像 :server)
    let server_obj = ObjectInitializer::new(engine)
        .function(
            NativeFunction::from_fn_ptr(commands::js_server_list),
            JsString::from("list"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_server_search),
            JsString::from("search"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_server_install),
            JsString::from("install"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_server_update),
            JsString::from("update"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_server_remove),
            JsString::from("remove"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_server_status),
            JsString::from("status"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(server_rows::js_server_rows),
            JsString::from("rows"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(server_tasks::js_server_task),
            JsString::from("task"),
            2,
        )
        .build();
    // helix.pane 命名空间:pane 清单(③ 会在此基础上加 open/set_place/…)
    // helix.pane.*(③ 的最终命名)。这些操作与旧 API 是**同一批 UiRequest**,
    // 所以直接复用同一批原生函数 —— 零重复实现,旧名删掉后新名照常工作。
    let pane_obj = ObjectInitializer::new(engine)
        .function(
            NativeFunction::from_fn_ptr(layout::js_pane_list),
            JsString::from("list"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_pane_info),
            JsString::from("info"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_pane_raise),
            JsString::from("raise"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_pane_pin),
            JsString::from("pin"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_pane_float),
            JsString::from("float"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_pane_embed),
            JsString::from("embed"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_close_leaf),
            JsString::from("close"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_focus_leaf),
            JsString::from("focus"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_focus_leaf_dir),
            JsString::from("focus_dir"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_swap_leaf_dir),
            JsString::from("move"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_resize_leaf),
            JsString::from("resize"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_zoom_leaf),
            JsString::from("zoom"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_unzoom),
            JsString::from("unzoom"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_minimize_leaf),
            JsString::from("minimize"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_equalize_leaf),
            JsString::from("equalize"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_layout_fix),
            JsString::from("fix"),
            2,
        )
        .build();
    // helix.layout.*(序列化;get/restore 已接线,save/load 待 ②-5 的 :layout 命令)
    let layout_obj = ObjectInitializer::new(engine)
        .function(
            NativeFunction::from_fn_ptr(layout::js_get_layout),
            JsString::from("get"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_restore_layout),
            JsString::from("restore"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_layout_save),
            JsString::from("save"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_layout_load),
            JsString::from("load"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_layout_list),
            JsString::from("list"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_layout_delete),
            JsString::from("delete"),
            1,
        )
        .build();
    // helix.pane_mode 命名空间:当前平级模式与它的键位表
    // (键位表由 Rust 侧单一来源提供,插件不必硬编码)
    let pane_mode_obj = ObjectInitializer::new(engine)
        .function(
            NativeFunction::from_fn_ptr(layout::js_pane_mode_current),
            JsString::from("current"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_pane_mode_keymap),
            JsString::from("keymap"),
            0,
        )
        .build();
    // ObjectInitializer 方法取 &mut self，链式必须在一个表达式内；
    // term_resize 是 cfg(unix) 的，拆成两步注册（builder 可变绑定）
    // helix.icons.* —— 图标表的**核心单一来源**(数据在 icons.rs,不再散在 JS 侧)。
    // 直接返回值,不走 UiRequest:图标是核心数据,没有异步往返的必要。
    // 必须在 `builder` **之前**构建:`ObjectInitializer::new` 会可变借用 engine,
    // 同一作用域内只能有一个(现有 pane/layout 等命名空间也都是这个顺序)。
    let icons_obj = {
        let mut o = ObjectInitializer::new(engine);
        for (name, f, arity) in icons::native_fns() {
            // `function` 是 `&mut self` 原地改并返回 `&mut Self`,不能赋值回去
            o.function(f, JsString::from(name), arity);
        }
        o.build()
    };
    // helix.buffer.*(③ 最终命名;与旧扁平 `buffers`/`current_buffer`/`focus_buffer`
    // 是同一批原生函数 —— 零重复实现,旧名删掉后新名照常工作)
    let buffer_obj = ObjectInitializer::new(engine)
        .function(
            NativeFunction::from_fn_ptr(layout::js_buffers),
            JsString::from("list"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_current_buffer),
            JsString::from("current"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_focus_buffer),
            JsString::from("focus"),
            1,
        )
        .build();
    // helix.stack.*(③ 最终命名)。目前只有只读的 `list` ——
    // `create/activate/remove` 需要先定"add 在兄弟约束下的去留"(见规格 A.7),
    // 所以本步只把**读**暴露出来(无需新状态、无需设计决策)。
    let stack_obj = ObjectInitializer::new(engine)
        .function(
            NativeFunction::from_fn_ptr(layout::js_stack_list),
            JsString::from("list"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_stack_create),
            JsString::from("create"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_stack_activate),
            JsString::from("activate"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_stack_remove),
            JsString::from("remove"),
            1,
        )
        .build();
    let mut builder = ObjectInitializer::new(engine);
    builder
        .function(
            NativeFunction::from_fn_ptr(commands::js_echo),
            JsString::from("echo"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_begin_edit),
            JsString::from("begin_edit"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_end_edit),
            JsString::from("end_edit"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_by_path),
            JsString::from("by_path"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_term_state),
            JsString::from("term_state"),
            3,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_register_command),
            JsString::from("register_command"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_open_popup),
            JsString::from("open_popup"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_set_component_render),
            JsString::from("set_component_render"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_open_panel),
            JsString::from("open_panel"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_close_panel),
            JsString::from("close_panel"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_read_dir),
            JsString::from("read_dir"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_open_file),
            JsString::from("open_file"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_runtime_path),
            JsString::from("runtime_path"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_command_args),
            JsString::from("command_args"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_viewport),
            JsString::from("viewport"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_picker_state),
            JsString::from("picker_state"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_quit),
            JsString::from("quit"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_plugins_dir),
            JsString::from("plugins_dir"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_read_file),
            JsString::from("read_file"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_write_file),
            JsString::from("write_file"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_write_plugin_file),
            JsString::from("write_plugin_file"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_loaded_plugins),
            JsString::from("loaded_plugins"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_remove_plugin_file),
            JsString::from("remove_plugin_file"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_move_panel),
            JsString::from("move_panel"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_set_buffer_icon),
            JsString::from("set_buffer_icon"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_set_completion_icon),
            JsString::from("set_completion_icon"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_set_completion_render),
            JsString::from("set_completion_render"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_el),
            JsString::from("el"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(input::js_set_input_value),
            JsString::from("set_input_value"),
            3,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_on),
            JsString::from("on"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_map),
            JsString::from("map"),
            3,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_set_cursor),
            JsString::from("set_cursor"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_set_selection),
            JsString::from("set_selection"),
            4,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_set_virtual_text),
            JsString::from("set_virtual_text"),
            5,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_set_highlight),
            JsString::from("set_highlight"),
            7,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_set_statusline),
            JsString::from("set_statusline"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_set_keymap_hint),
            JsString::from("set_keymap_hint"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_load),
            JsString::from("load"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_export),
            JsString::from("export"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_lazy),
            JsString::from("lazy"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_run_command),
            JsString::from("run_command"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_run),
            JsString::from("run"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(shell::js_run_async),
            JsString::from("run_async"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(shell::js_spawn),
            JsString::from("spawn"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(shell::js_term_write),
            JsString::from("term_write"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(shell::js_term_kill),
            JsString::from("term_kill"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(shell::js_read_file_async),
            JsString::from("read_file_async"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(shell::js_write_file_async),
            JsString::from("write_file_async"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(shell::js_stat_async),
            JsString::from("stat_async"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(shell::js_glob_async),
            JsString::from("glob_async"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(shell::js_read_tree),
            JsString::from("read_tree"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_open_terminal),
            JsString::from("open_terminal"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_term_feed),
            JsString::from("term_feed"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_set_terminal_mode),
            JsString::from("set_terminal_mode"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_term_clear),
            JsString::from("term_clear"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_term_save),
            JsString::from("term_save"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_resize_term),
            JsString::from("resize_term"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_split),
            JsString::from("split"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_buffer_open),
            JsString::from("buffer_open"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_resize_leaf_dir),
            JsString::from("layout_resize"),
            3,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_swap_leaves),
            JsString::from("layout_swap"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout::js_get_component_state),
            JsString::from("get_component_state"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(watch::js_watch),
            JsString::from("watch"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(watch::js_unwatch),
            JsString::from("unwatch"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(diagnostics::js_diagnostics),
            JsString::from("diagnostics"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(permissions::js_permissions),
            JsString::from("permissions"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(config::js_define_config),
            JsString::from("define_config"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(config::js_get_config),
            JsString::from("get_config"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(config::js_get_config_docs),
            JsString::from("get_config_docs"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(theme::js_set_theme),
            JsString::from("set_theme"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(theme::js_reset_theme),
            JsString::from("reset_theme"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(theme::js_get_style),
            JsString::from("get_style"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(theme::js_theme_info),
            JsString::from("theme_info"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(theme::js_set_theme_name),
            JsString::from("set_theme_name"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(icons::js_set_diagnostic_icons),
            JsString::from("set_diagnostic_icons"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_term_list),
            JsString::from("term_list"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(popup::js_term_close),
            JsString::from("term_close"),
            1,
        );
    #[cfg(unix)]
    builder.function(
        NativeFunction::from_fn_ptr(shell::js_term_resize),
        JsString::from("term_resize"),
        3,
    );
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
