//! JavaScript plugin runtime for the Helix editor (PoC).

mod commands;
mod icons;
mod input;
mod layout;
mod lsp;
pub mod picker;
mod popup;
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
pub use layout::{layout_delete, layout_list, layout_load, layout_save};
pub use theme::*;
pub use types::DocChange;
pub use types::*;

use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsString, JsValue, NativeFunction};

/// 初始化线程局部运行时（幂等）：消息/UI 队列、事件通道、boa 引擎与全局 `helix` 对象。
pub fn init() {
    state::MESSAGES.get_or_init(Default::default);
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
    state::with_engine_slot(|slot| {
        if slot.is_none() {
            let engine = Box::leak(Box::new(Context::default()));
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
            for (name, f) in [
                (
                    "install",
                    commands::js_plugin_install
                        as boa_engine::native_function::NativeFunctionPointer,
                ),
                (
                    "update",
                    commands::js_plugin_update
                        as boa_engine::native_function::NativeFunctionPointer,
                ),
                (
                    "remove",
                    commands::js_plugin_remove
                        as boa_engine::native_function::NativeFunctionPointer,
                ),
            ] {
                let f = FunctionObjectBuilder::new(
                    engine.realm(),
                    boa_engine::NativeFunction::from_fn_ptr(f),
                )
                .name(name)
                .length(1)
                .build();
                plugin_fn
                    .set(JsString::from(name), f, false, engine)
                    .expect("set plugin method");
            }
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
            *slot = Some(engine);
        }
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use boa_engine::JsValue;
    use std::sync::Mutex;

    // 多个测试共享全局运行时，用锁串行化避免消息队列竞争
    pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// v13 任务简报验证测试：read_dir 排序/is_dir、open_file、move_panel 入队 + 校验。
    /// 简报原文断言 count:2，但设置创建 3 个条目（a.txt、b.js、sub/）→ 按实际调整为 count:3；
    /// 排序断言 entries[0].name < entries[1].name 不受影响（a.txt < b.js）。
    #[test]
    fn term_wake_fires_on_event() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fired2 = fired.clone();
        set_term_wake(Box::new(move || {
            fired2.store(true, std::sync::atomic::Ordering::SeqCst);
        }));
        load_script(
            r#"
            helix.register_command("wk", () => {
                helix.spawn({ pty: false, cmd: "echo wake-test", onChunk: () => {}, onExit: () => {} });
            });
            "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("wk", &ctx).unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !fired.load(std::sync::atomic::Ordering::SeqCst)
            && std::time::Instant::now() < deadline
        {
            // 消费事件（让 worker 继续/完成），wake 在 send 时触发
            let _ = drain_term_events();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            fired.load(std::sync::atomic::Ordering::SeqCst),
            "wake should fire on event send"
        );
    }

    /// `helix.pane.list()` 读 Rust 侧推来的 pane 清单(含浮窗)。
    /// 之前 `get_layout()` 看不到浮窗,插件因此完全不知道浮窗存在。
    #[test]
    fn pane_list_api_reads_snapshot() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };

        // 未推送时:空数组(不报错)
        load_script(
            r#"
        helix.register_command("pl0", () => {
            helix.echo("n:" + helix.pane.list().length);
        });
        "#,
        )
        .unwrap();
        assert!(run_command("pl0", &ctx).unwrap());
        assert_eq!(take_messages()[0], "n:0");

        // 推入一份含浮窗的快照后应能读到 place/pinned/focused
        crate::state::cache_panes(
            r#"{"panes":[
                {"id":0,"kind":"EditorView","place":"tiled","focused":false,"fixed":false,"pinned":false},
                {"id":7,"kind":"PluginTerminal","place":"float","focused":true,"fixed":false,"pinned":true,"z":2,
                 "rect":{"x":0.2,"y":0.2,"w":0.5,"h":0.5}}
            ]}"#,
        );
        load_script(
            r#"
        helix.register_command("pl1", () => {
            const l = helix.pane.list();
            helix.echo(
                "n:" + l.length + " p:" + l[1].place + " pin:" + l[1].pinned +
                " foc:" + l[1].focused + " tx:" + l[0].place + " k:" + l[1].kind + "/" + l[0].kind
            );
        });
        "#,
        )
        .unwrap();
        assert!(run_command("pl1", &ctx).unwrap());
        assert_eq!(
            take_messages()[0],
            "n:2 p:float pin:true foc:true tx:tiled k:PluginTerminal/EditorView"
        );
    }

    /// `helix.pane.float` / `helix.pane.embed` —— 平铺 ↔ 浮窗。
    /// 词汇取自 zellij 插件 API 的 `float_multiple_panes` / `embed_multiple_panes`
    /// (它的词汇是 float / embed),而不是自造的 `set_place`。
    #[test]
    fn pane_float_and_embed_push_requests() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("pf", () => {
            helix.pane.float(3);
            helix.pane.embed(4);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("pf", &ctx).unwrap());
        let reqs = take_ui_requests();
        assert!(
            matches!(&reqs[0], UiRequest::PaneFloat { id } if *id == 3),
            "pane.float(3) → PaneFloat{{id:3}}"
        );
        assert!(
            matches!(&reqs[1], UiRequest::PaneEmbed { id } if *id == 4),
            "pane.embed(4) → PaneEmbed{{id:4}}"
        );
    }

    /// `helix.pane.*` / `helix.layout.*` —— ③ 的最终命名。
    /// 这些是**同一批 UiRequest 的新名字**(旧名删除后照常工作),所以这里只验
    /// 名字齐全 + 调用真的入队(各 variant 的行为已由旧名测试覆盖)。
    #[test]
    fn pane_namespace_is_wired() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("pn", () => {
            const need = ["list","float","embed","close","focus","focus_dir",
                          "move","resize","zoom","unzoom","minimize","equalize","fix",
                          "info","raise","pin"];
            const needBuf = ["list","current","focus"];
            const missingBuf = needBuf.filter((n) => typeof helix.buffer[n] !== "function");
            const missing = need.filter((n) => typeof helix.pane[n] !== "function").concat(missingBuf);
            helix.echo("missing:" + missing.join(","));
            helix.pane.close(5);
            helix.pane.focus(6);
            helix.echo("lg:" + typeof helix.layout.get + "/" + typeof helix.layout.restore);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("pn", &ctx).unwrap());
        let msgs = take_messages();
        assert_eq!(msgs[0], "missing:", "pane.* 名字必须齐全");
        assert_eq!(
            msgs[1], "lg:function/function",
            "layout.get/restore 必须存在"
        );
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 2, "close/focus 各入队一个请求");
    }

    /// 多根查找(对齐 Neovim runtimepath 语义):**后加的根覆盖先加的**。
    /// 这是"把功能做成随软件分发的内置插件"的地基 —— 内置放前、用户放后即可覆盖。
    #[test]
    fn plugin_roots_resolve_with_later_overriding_earlier() {
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let mk = |dir: &std::path::Path, rel: &str, tag: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, tag).unwrap();
        };
        // 两边都有同名:应取 user 那份
        mk(bundled.path(), "features/x/index.js", "bundled");
        mk(user.path(), "features/x/index.js", "user");
        // 只有内置有:应能找到
        mk(bundled.path(), "lib/only_bundled.js", "b");
        let roots = vec![bundled.path().to_path_buf(), user.path().to_path_buf()];

        let got = crate::state::resolve_in(&roots, "features/x/index.js").unwrap();
        assert_eq!(
            std::fs::read_to_string(&got).unwrap(),
            "user",
            "后加的根(user)必须覆盖先加的(bundled)"
        );
        assert!(
            crate::state::resolve_in(&roots, "lib/only_bundled.js").is_some(),
            "只在前面根里的相对名也要能找到"
        );
        // 都不存在 → 回落到**最后一个根**(报错信息贴近用户目录)
        let miss = crate::state::resolve_in(&roots, "nope.js").unwrap();
        assert!(
            miss.starts_with(user.path()),
            "回落应指向最后一个根: {miss:?}"
        );
        // 绝对路径原样透传
        let abs = bundled.path().join("lib/only_bundled.js");
        assert_eq!(
            crate::state::resolve_in(&roots, abs.to_str().unwrap()).unwrap(),
            abs
        );
    }

    /// `helix.pane.info(id)` —— 单个 pane 信息;找不到 → null。
    /// 复用 pane.list() 的同一份快照(不新增状态),所以两者必须一致。
    #[test]
    fn pane_info_reads_same_snapshot_as_list() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        crate::state::cache_panes(
            r#"{"panes":[
                {"id":0,"kind":"EditorView","place":"tiled","focused":true,"fixed":false,"pinned":false},
                {"id":7,"kind":"PluginTerminal","place":"float","focused":false,"fixed":false,"pinned":true,"z":2}
            ]}"#,
        );
        crate::load_script(
            r##"
            helix.register_command("pi", () => {
                const a = helix.pane.info(7);
                helix.echo("k:" + (a ? a.kind : "null") + " place:" + (a ? a.place : "-") + " z:" + (a ? a.z : "-"));
                helix.echo("ed:" + helix.pane.info(0).kind);
                helix.echo("miss:" + helix.pane.info(999));
                helix.echo("same:" + (helix.pane.info(7).id === helix.pane.list()[1].id));
            });
            "##,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("pi", &ctx).unwrap());
        let m = take_messages();
        assert_eq!(m[0], "k:PluginTerminal place:float z:2");
        assert_eq!(m[1], "ed:EditorView");
        assert_eq!(m[2], "miss:null", "找不到 → null(不是 undefined/报错)");
        assert_eq!(m[3], "same:true", "与 pane.list() 是同一份快照");

        // **复原全局缓存**:本测试注入过 panes,不清掉会让后续依赖"空缓存"的测试失败
        // (实测:不清时有 4 个无关测试挂了)
        crate::state::cache_panes(r#"{"panes":[]}"#);
    }

    /// `pane.raise` / `pane.pin` —— 入队对应请求;`pin` **必须显式 on**。
    #[test]
    fn pane_raise_and_pin_push_requests() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("pr", () => {
            helix.pane.raise(7);
            helix.pane.pin(7, true);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("pr", &ctx).unwrap());
        let reqs = take_ui_requests();
        assert!(
            matches!(&reqs[0], UiRequest::PaneRaise { id } if *id == 7),
            "pane.raise(7) → PaneRaise{{id:7}}"
        );
        assert!(
            matches!(&reqs[1], UiRequest::PanePin { id, on } if *id == 7 && *on),
            "pane.pin(7,true) → PanePin{{id:7,on:true}}"
        );

        // 不显式给 on → 报错(而不是隐式取反:快照可能落后一帧,取反会抖动)
        assert!(
            load_script(r#"helix.pane.pin(7);"#).is_err(),
            "pin 缺 on 应当报错"
        );
    }

    /// `helix.stack.list()` —— 从 pane 快照分组;**按锚去重**是关键
    /// (组内每个成员都会报同一份 members,不去重就会重复列出同一组)。
    #[test]
    fn stack_list_groups_and_dedupes_by_anchor() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        crate::state::cache_panes(
            r#"{"panes":[
                {"id":0,"kind":"EditorView","place":"tiled","stack":[0,1]},
                {"id":1,"kind":"PluginTerminal","place":"tiled","stack":[0,1]},
                {"id":5,"kind":"PluginPanel","place":"tiled","stack":[]},
                {"id":7,"kind":"PluginTerminal","place":"float"}
            ]}"#,
        );
        crate::load_script(
            r##"
            helix.register_command("sl", () => {
                const g = helix.stack.list();
                helix.echo("n:" + g.length);
                helix.echo("a:" + g[0].anchor + " m:" + g[0].members.join(","));
                helix.echo("t:" + typeof helix.stack.list);
            });
            "##,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("sl", &ctx).unwrap());
        let m = take_messages();
        assert_eq!(m[0], "n:1", "两个成员只应产出**一组**(按锚去重)");
        assert_eq!(m[1], "a:0 m:0,1", "锚 = members[0](= 当前显示的那个)");
        assert_eq!(m[2], "t:function");
        // 复原全局缓存(本测试注入过 panes;不清会让依赖"空快照"的测试挂)
        crate::state::cache_panes(r#"{"panes":[]}"#);
    }

    /// `helix.stack.*` —— 名字齐全 + 三个写操作各入队对应请求。
    /// 注意:本套 API 只支持**2 人组**(规格 A.7 兄弟约束),所以**没有 `add`**
    /// —— 一个 Split 只有两个子,组不可能多于 2 人。这条注释就是"为什么没有 add"的答案。
    #[test]
    fn stack_namespace_and_write_ops() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("sw", () => {
            const need = ["list","create","activate","remove"];
            const missing = need.filter((n) => typeof helix.stack[n] !== "function");
            helix.echo("missing:" + missing.join(","));
            helix.echo("add:" + typeof helix.stack.add);
            helix.stack.create(3);
            helix.stack.activate(3, 9);
            helix.stack.remove(3);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("sw", &ctx).unwrap());
        let m = take_messages();
        assert_eq!(m[0], "missing:", "四个名字必须齐全");
        assert_eq!(m[1], "add:undefined", "**没有 add** —— 2 人组的模型约束");
        let reqs = take_ui_requests();
        assert!(matches!(&reqs[0], UiRequest::StackCreate { id } if *id == 3));
        assert!(
            matches!(&reqs[1], UiRequest::StackActivate { id, member } if *id == 3 && *member == 9)
        );
        assert!(matches!(&reqs[2], UiRequest::StackRemove { id } if *id == 3));

        // activate 缺 member → 报错(要显式说清"显示哪个")
        assert!(
            load_script(r#"helix.stack.activate(3);"#).is_err(),
            "缺 member 应报错"
        );
    }

    /// `open_file` 的 `scratch` 选项:带它走 `OpenScratchFile`(不绑定路径),不带走原请求。
    /// 这条路由是 `:tutor` 搬迁的**安全前提** —— 走错了,用户 `:w` 会覆盖 runtime 里的原文件。
    #[test]
    fn open_file_scratch_routes_to_scratch_request() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("ofs", () => {
            helix.open_file("/tmp/plain.txt");
            helix.open_file("/tmp/scratch.txt", { scratch: true });
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("ofs", &ctx).unwrap());
        let reqs = take_ui_requests();
        assert!(
            matches!(&reqs[0], UiRequest::OpenFile { path, .. } if path == "/tmp/plain.txt"),
            "不带 scratch → 原 OpenFile 请求"
        );
        assert!(
            matches!(&reqs[1], UiRequest::OpenScratchFile { path } if path == "/tmp/scratch.txt"),
            "带 scratch → OpenScratchFile(不绑定路径)"
        );
    }

    #[test]
    fn open_terminal_api() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("ot", () => {
            const pid = helix.open_terminal({ cmd: "cat", side: "right", size: 40 });
            helix.echo("pid:" + pid);
            helix.term_feed(pid, "abc");
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("ot", &ctx).unwrap());
        assert!(take_messages()[0].starts_with("pid:"));
        let reqs = take_ui_requests();
        assert!(
            matches!(&reqs[0], UiRequest::OpenTerminal { side, size, .. } if side == "right" && *size == 40)
        );
        assert!(matches!(&reqs[1], UiRequest::TermFeed { chunk, .. } if chunk == "abc"));
        // 校验
        assert!(
            load_script(r#"helix.open_terminal({ cmd: "x", side: "top", size: 10 });"#).is_err()
        );
        assert!(load_script(r#"helix.open_terminal({ cmd: "x", side: "right" });"#).is_err());
        // 缺 size
    }

    /// 交互 bash 启动不应报 "cannot set terminal process group"（缺 setsid/控制终端）。
    /// 修复前：bash job control 初始化失败向 stderr 打印该错误。
    #[test]
    fn pty_bash_interactive_no_error() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("bi", () => {
            const pid = helix.open_terminal({ cmd: "bash -i", side: "bottom", size: 10 });
            helix.echo("pid:" + pid);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("bi", &ctx).unwrap());
        let _ = take_messages();
        let pty_id = match &take_ui_requests()[0] {
            UiRequest::OpenTerminal { pty_id, .. } => *pty_id,
            other => panic!("expected OpenTerminal, got {other:?}"),
        };
        // 收集 bash 启动输出直到报错或超时（提示符正常出现即好）。
        // 注意：必须 resolve 事件才能触发 bridge 回调（term_feed → UI 请求），
        // 只 drain 不 resolve 会让断言假绿（输出为空）。
        let mut output = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while std::time::Instant::now() < deadline {
            for ev in drain_term_events() {
                let eid = match &ev {
                    TermEvent::Chunk(id, _) => *id,
                    TermEvent::Exit(id, _, _) => *id,
                };
                let _ = resolve_term_event(eid, ev);
            }
            for req in take_ui_requests() {
                if let UiRequest::TermFeed { chunk, .. } = req {
                    output.push_str(&chunk);
                }
            }
            if output.contains("cannot set terminal process group") {
                break;
            }
            if output.contains('$') {
                break; // 提示符出现
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = term_kill(pty_id);
        // 消费 Exit 事件，清理回调注册
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !with_terms(|m| m.is_empty()) && std::time::Instant::now() < deadline {
            let _ = drain_term_events();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            !output.contains("cannot set terminal process group"),
            "交互 bash 不应报 job control 错误，实际输出: {output:?}"
        );
    }

    #[test]
    fn open_terminal_after_kill_repro() {
        // 关闭（term_kill，模拟 Esc/q 关闭终端）后再 open_terminal：应能正常打开。
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("t3", () => {
            const pid = helix.open_terminal({ cmd: "cat", side: "bottom", size: 10 });
            helix.echo("pid:" + pid);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("t3", &ctx).unwrap(), "第一次 open_terminal");
        let _ = take_messages();
        let pty_id = match &take_ui_requests()[0] {
            UiRequest::OpenTerminal { pty_id, .. } => *pty_id,
            other => panic!("expected OpenTerminal, got {other:?}"),
        };
        // 模拟关闭：Esc/q → remove_type → PluginTerminal::drop → term_kill(pty_id)
        term_kill(pty_id).unwrap();
        // 消费 Exit 事件（worker 退出回调），与事件循环 drain 一致
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !with_terms(|m| m.is_empty()) && std::time::Instant::now() < deadline {
            let _ = drain_term_events();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            run_command("t3", &ctx).unwrap(),
            "kill 后再 open_terminal 应成功"
        );
        let _ = take_messages();
        let _ = take_ui_requests(); // 清空第二次 open 的请求，避免污染后续测试
    }

    #[test]
    fn sidecar_apis() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = std::env::temp_dir().join(format!("helix-js-sidecar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("b.js"), "b").unwrap();

        // 路径经 {:?}（JSON 字符串转义）注入脚本
        let dir_str = dir.to_string_lossy();
        let file_path = dir.join("a.txt");
        let file_str = file_path.to_string_lossy();
        let script = format!(
            r#"
        helix.register_command("sc", () => {{
            const entries = helix.read_dir({dir:?});
            helix.echo("count:" + entries.length + " sorted:" + (entries[0].name < entries[1].name));
            const dirs = entries.filter(e => e.is_dir);
            helix.echo("dirs:" + dirs.length + ":" + dirs[0].name);
            helix.open_file({file:?});
            helix.move_panel(7, "left");
        }});
        "#,
            dir = dir_str,
            file = file_str,
        );
        load_script(&script).unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("sc", &ctx).unwrap());
        let msgs = take_messages();
        assert!(msgs[0].starts_with("count:3 sorted:true"), "{msgs:?}");
        assert_eq!(msgs[1], "dirs:1:sub");
        let reqs = take_ui_requests();
        assert!(
            matches!(&reqs[0], UiRequest::OpenFile { path, row: None, col: None } if path.ends_with("a.txt"))
        );
        assert!(matches!(&reqs[1], UiRequest::MovePanel { id: 7, side } if side == "left"));

        // 校验：read_dir 不存在路径 → 抛错；move_panel 非法 side → 抛错；open_file 非字符串 → 抛错
        load_script(
            r#"
        helix.register_command("bad1", () => { helix.read_dir("/nonexistent-helix-js-xyz"); });
        helix.register_command("bad2", () => { helix.move_panel(7, "top"); });
        helix.register_command("bad3", () => { helix.open_file(42); });
        "#,
        )
        .unwrap();
        assert!(run_command("bad1", &ctx).is_err());
        assert!(run_command("bad2", &ctx).is_err());
        assert!(run_command("bad3", &ctx).is_err());
    }

    #[test]
    fn theme_overrides_api() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 清残留脏位（本测试是唯一 set_theme 的测试，防御顺序依赖）
        let _ = take_theme_dirty();
        assert!(theme_overrides().is_empty());

        // set_theme：整体替换 + 置脏；字符串 = fg，对象 = {fg, bg, modifiers}
        load_script(
            r##"
        helix.set_theme({
            "ui.popup": "#ff00aa",
            "error": "red",
            "ui.window": { fg: "#112233", bg: "#445566", modifiers: ["italic", "bold"] },
            "ui.linenr": 42,
        });
        "##,
        )
        .unwrap();
        assert!(take_theme_dirty());
        let ov = theme_overrides();
        let popup = ov.get("ui.popup").unwrap();
        assert_eq!(popup.fg.as_deref(), Some("#ff00aa"));
        assert_eq!(popup.bg, None);
        assert!(popup.modifiers.is_empty(), "字符串值只有 fg");
        assert_eq!(ov.get("error").unwrap().fg.as_deref(), Some("red"));
        let win = ov.get("ui.window").unwrap();
        assert_eq!(win.fg.as_deref(), Some("#112233"));
        assert_eq!(win.bg.as_deref(), Some("#445566"));
        assert_eq!(win.modifiers, vec!["italic", "bold"]);
        assert!(!ov.contains_key("ui.linenr"), "非字符串非对象值应被忽略");

        // 再次 set_theme：替换而非累积
        load_script(r#"helix.set_theme({ "error": "blue" });"#).unwrap();
        assert!(take_theme_dirty());
        let ov = theme_overrides();
        assert_eq!(ov.len(), 1);
        assert_eq!(ov.get("error").unwrap().fg.as_deref(), Some("blue"));

        // 空对象 → 清空覆盖（等价的 reset）
        load_script(r#"helix.set_theme({});"#).unwrap();
        assert!(theme_overrides().is_empty());

        // reset_theme：清空 + 置脏
        load_script(r#"helix.set_theme({ "error": "red" });"#).unwrap();
        assert!(take_theme_dirty());
        load_script(r#"helix.reset_theme();"#).unwrap();
        assert!(take_theme_dirty());
        assert!(theme_overrides().is_empty());

        // 非法参数（非对象/缺参）→ JS 报错
        assert!(load_script(r#"helix.set_theme("red");"#).is_err());
        assert!(load_script(r#"helix.set_theme();"#).is_err());
    }

    #[test]
    fn echo_captures_message() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(r#"helix.echo("hello from js");"#).unwrap();
        assert_eq!(take_messages(), vec!["hello from js"]);
    }

    #[test]
    fn loaded_scripts_list() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script_named("a.js", r#"helix.register_command("a", () => {});"#).unwrap();
        load_script_named("b.js", r#"helix.register_command("b", () => {});"#).unwrap();
        let names = loaded_scripts();
        assert!(names.iter().any(|n| n == "a.js"));
        assert!(names.iter().any(|n| n == "b.js"));
    }

    #[test]
    fn register_and_run_command() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("where", (ctx) => {
            helix.echo("cursor: " + ctx.cursor.row + "," + ctx.cursor.col);
        });
        "#,
        )
        .unwrap();

        let ctx = CommandContext {
            path: Some("/tmp/demo.rs".to_string()),
            text: "hello\nworld".to_string(),
            cursor: (1, 2),
            selection: ((0, 0), (0, 0)),
            docs: vec![],
        };
        assert!(run_command("where", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["cursor: 1,2"]);

        // 未注册的命令返回 false
        assert!(!run_command("nope", &ctx).unwrap());
        // 非法命令名（含空白）注册时报错
        assert!(load_script(r#"helix.register_command("bad name", () => {});"#).is_err());
    }

    #[test]
    fn panel_api() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        const pid = helix.open_panel({ side: "right", size: 30, render: () => ["p1", "p2"] });
        helix.echo("id:" + pid);
        helix.close_panel(pid);
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        // 编译器建议：matches! 守卫未用 id 绑定 → id: _（简报原文绑了 id，clippy 要求 0 告警）
        assert!(
            matches!(&reqs[0], UiRequest::OpenPanel { id: _, side, size, .. } if side == "right" && *size == 30)
        );
        assert!(matches!(&reqs[1], UiRequest::ClosePanel { id: _ }));
        assert!(take_messages()[0].starts_with("id:"));
        // 校验：side 白名单 / size / render
        assert!(
            load_script(r#"helix.open_panel({ side: "top", size: 10, render: () => [] });"#)
                .is_err()
        );
        assert!(load_script(r#"helix.open_panel({ side: "right", size: 10 });"#).is_err()); // 缺 render
        assert!(load_script(
            r#"helix.open_panel({ side: "right", size: "big", render: () => [] });"#
        )
        .is_err());
        crate::state::with_open_panels(|p| p.clear()); // 清 OPEN_PANELS，防污染后续测试
    }

    #[test]
    fn panel_onkey() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        const pid = helix.open_panel({
            side: "right", size: 20,
            render: () => ["p"],
            onKey: (key) => { helix.echo("panel-key:" + key.name); return key.name === "Esc" ? "close" : "handled"; },
        });
        helix.echo("pid:" + pid);
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        // 编译器建议：matches! 未用 id 绑定 → id: _（简报原文绑了 id，clippy 要求 0 告警）
        assert!(matches!(&reqs[0], UiRequest::OpenPanel { id: _, .. }));
        // 编译器要求：单臂 match 非穷尽 → 改 let-else（与 popup_lifecycle 同款）
        let UiRequest::OpenPanel { id, .. } = reqs[0] else {
            unreachable!("expected OpenPanel")
        };
        assert!(take_messages()[0].starts_with("pid:"));
        // popup_key 走同一注册表：Esc → close，其他 → handled
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        let esc = PluginKey {
            name: "Esc".into(),
            shift: false,
            ctrl: false,
            alt: false,
        };
        assert_eq!(popup_key(id, &esc, &ctx).unwrap(), PopupKeyResult::Close);
        assert_eq!(take_messages(), vec!["panel-key:Esc"]);
        // panel_has_onkey：有 onKey → true（简报测试的补充断言）
        assert!(panel_has_onkey(id));
        // 无 onKey 的面板：false（helix-term 侧据此全 Ignore 穿透，不调 popup_key）
        load_script(r#"helix.open_panel({ side: "left", size: 10, render: () => ["x"] });"#)
            .unwrap();
        let UiRequest::OpenPanel { id: id2, .. } = take_ui_requests()[0] else {
            unreachable!("expected OpenPanel")
        };
        assert!(!panel_has_onkey(id2));
        crate::state::with_open_panels(|p| p.clear()); // 清 OPEN_PANELS（两个面板未 close），防污染
        let _ = take_ui_requests();
    }

    #[test]
    fn popup_lifecycle() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        let rendered = null;
        helix.open_popup({
            render: () => ["a", "b", "c"],
            onKey: (key) => key.name === "Down" ? "handled" : "close",
            onClose: () => helix.echo("closed:" + rendered),
        });
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 1);
        let UiRequest::OpenPopup { id, .. } = reqs[0] else {
            unreachable!("expected OpenPopup")
        };
        assert_eq!(id, 1); // 自增从 1 开始

        let lines = render_popup(id, 40, 10, None).unwrap();
        assert_eq!(
            lines,
            Content::Lines(vec![
                StyledLine::plain("a"),
                StyledLine::plain("b"),
                StyledLine::plain("c"),
            ])
        );

        let key = PluginKey {
            name: "Down".into(),
            shift: false,
            ctrl: false,
            alt: false,
        };
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Handled);
        let key = PluginKey {
            name: "Esc".into(),
            shift: false,
            ctrl: false,
            alt: false,
        };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Close);

        close_popup(id).unwrap();
        assert_eq!(take_messages(), vec!["closed:null"]);
        assert!(render_popup(id, 40, 10, None).is_err()); // 已关闭，注册表移除
    }

    #[test]
    fn popup_default_keys_and_validation() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 未提供 onKey：Esc 默认关闭，其他穿透
        load_script(r#"helix.open_popup({ render: () => ["x"] });"#).unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        let key = PluginKey {
            name: "Enter".into(),
            shift: false,
            ctrl: false,
            alt: false,
        };
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Ignored);
        let key = PluginKey {
            name: "Esc".into(),
            shift: false,
            ctrl: false,
            alt: false,
        };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Close);
        close_popup(id).unwrap();

        // render 非数组 → Err
        load_script(r#"helix.open_popup({ render: () => "not an array" });"#).unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        assert!(render_popup(id, 40, 10, None).is_err());
        close_popup(id).unwrap();

        // 参数缺失/类型错误 → JS 报错
        assert!(load_script(r#"helix.open_popup({});"#).is_err());
        assert!(load_script(r#"helix.open_popup({ render: 42 });"#).is_err());
    }

    #[test]
    fn popup_size_position() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.open_popup({ render: () => ["x"], width: 40, height: 10, position: { row: 3, col: 4 } });
        helix.open_popup({ render: () => ["y"] });
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 2);
        match &reqs[0] {
            UiRequest::OpenPopup {
                width,
                height,
                position,
                ..
            } => {
                assert_eq!(*width, Some(40));
                assert_eq!(*height, Some(10));
                assert_eq!(*position, Some((3, 4)));
            }
            other => panic!("expected OpenPopup, got {other:?}"),
        }
        match &reqs[1] {
            UiRequest::OpenPopup {
                width,
                height,
                position,
                ..
            } => {
                assert_eq!(*width, None);
                assert_eq!(*height, None);
                assert_eq!(*position, None);
            }
            other => panic!("expected OpenPopup, got {other:?}"),
        }
        // 非法类型
        assert!(load_script(r#"helix.open_popup({ render: () => [], width: "big" });"#).is_err());
    }

    #[test]
    fn popup_styled_lines() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.open_popup({
            render: () => [
                { text: "err: ", style: "error" },
                "plain",
                { text: "warn" },
            ],
        });
        "#,
        )
        .unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        let lines = render_popup(id, 40, 10, None).unwrap();
        assert_eq!(
            lines,
            Content::Lines(vec![
                StyledLine::styled("err: ", "error"),
                StyledLine::plain("plain"),
                StyledLine::plain("warn"),
            ])
        );
        close_popup(id).unwrap();
        // 非法元素（缺 text / 非字符串非对象）→ Err
        load_script(r#"helix.open_popup({ render: () => [{ style: "error" }] });"#).unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        assert!(render_popup(id, 40, 10, None).is_err());
        close_popup(id).unwrap();
    }

    #[test]
    fn popup_onkey_edits_doc() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.open_popup({
            render: () => ["a", "b"],
            onKey: (key, doc) => {
                if (key.name === "Enter") {
                    doc.insert(doc.cursor.row, doc.cursor.col, "XYZ");
                    return "close";
                }
                return "handled";
            },
        });
        "#,
        )
        .unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        let ctx = CommandContext {
            path: Some("/tmp/p.rs".into()),
            text: "line1\nline2".into(),
            cursor: (1, 2),
            selection: ((0, 0), (0, 0)),
            docs: vec![],
        };
        let key = PluginKey {
            name: "Enter".into(),
            shift: false,
            ctrl: false,
            alt: false,
        };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Close);
        assert_eq!(
            take_edits(),
            vec![Edit {
                doc: None,
                start: (1, 2),
                end: (1, 2),
                insert: "XYZ".into()
            }]
        );
        close_popup(id).unwrap();
    }

    #[test]
    fn buffer_icon_hook() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        assert_eq!(bufferline_icon(Some("a.rs")), None); // 未注册 → None

        load_script(
            r#"
        helix.set_buffer_icon((path) => path && path.endsWith(".rs") ? "🦀" : null);
        "#,
        )
        .unwrap();
        assert_eq!(bufferline_icon(Some("main.rs")), Some("🦀".to_string()));
        assert_eq!(bufferline_icon(Some("main.py")), None);
        assert_eq!(bufferline_icon(None), None);
    }

    #[test]
    fn doc_edits_queue() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("edit", (ctx) => {
            ctx.doc.insert(1, 2, "ab");
            ctx.doc.replace(0, 0, 0, 5, "new");
            ctx.doc.delete(3, 0, 4, 0);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (1, 2),
            selection: ((0, 0), (0, 0)),
        };
        run_command("edit", &ctx).unwrap();
        let edits = take_edits();
        assert_eq!(
            edits,
            vec![
                Edit {
                    doc: None,
                    start: (1, 2),
                    end: (1, 2),
                    insert: "ab".into()
                },
                Edit {
                    doc: None,
                    start: (0, 0),
                    end: (0, 5),
                    insert: "new".into()
                },
                Edit {
                    doc: None,
                    start: (3, 0),
                    end: (4, 0),
                    insert: String::new()
                },
            ]
        );
        // take_edits 清空
        assert!(take_edits().is_empty());
    }

    #[test]
    fn doc_edit_validation() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 类型错误 → 命令失败（run_command 返回 Err），且队列被清空
        load_script(
            r#"
        helix.register_command("bad1", (ctx) => { ctx.doc.insert("x", 0, "a"); });
        helix.register_command("bad2", (ctx) => { ctx.doc.replace(0, 0, 0, 0, 42); });
        helix.register_command("bad3", (ctx) => { ctx.doc.delete(0, 0, 0); });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("bad1", &ctx).is_err());
        assert!(take_edits().is_empty());
        assert!(run_command("bad2", &ctx).is_err());
        assert!(run_command("bad3", &ctx).is_err());
        assert!(take_edits().is_empty());
        // 正常命令运行后队列仍有值（供 helix-term 消费）
        load_script(r#"helix.register_command("ok", (ctx) => { ctx.doc.insert(0, 0, "z"); });"#)
            .unwrap();
        run_command("ok", &ctx).unwrap();
        assert_eq!(take_edits().len(), 1);
    }

    #[test]
    fn by_path_reads_other_buffer() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("read-other", () => {
            const d = helix.by_path("/tmp/other.rs");
            if (d === null) throw new Error("null");
            if (d.path !== "/tmp/other.rs") throw new Error("path mismatch");
            if (d.text !== "other text") throw new Error("text mismatch");
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            path: Some("/tmp/current.rs".into()),
            text: "current".into(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
            docs: vec![DocSnapshot {
                path: "/tmp/other.rs".into(),
                text: "other text".into(),
            }],
        };
        run_command("read-other", &ctx).unwrap();
    }

    #[test]
    fn by_path_edits_target_other_buffer() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("edit-other", () => {
            const d = helix.by_path("/tmp/other.rs");
            if (d === null) throw new Error("null");
            d.insert(0, 0, "X");
        });
        helix.register_command("edit-current", (ctx) => {
            ctx.doc.insert(0, 0, "Y");
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            path: Some("/tmp/current.rs".into()),
            text: "current".into(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
            docs: vec![DocSnapshot {
                path: "/tmp/other.rs".into(),
                text: "other text".into(),
            }],
        };
        run_command("edit-other", &ctx).unwrap();
        let edits = take_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].doc.as_deref(), Some("/tmp/other.rs"));
        run_command("edit-current", &ctx).unwrap();
        let edits = take_edits();
        assert_eq!(edits[0].doc, None, "ctx.doc 编辑仍指向当前 buffer");
    }

    #[test]
    fn event_handlers() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        assert!(!has_handlers("save"));

        load_script(
            r#"
        let order = [];
        helix.on("save", (doc) => { order.push("a"); });
        helix.on("save", (doc) => { order.push("b"); });
        helix.on("mode-change", (mode, doc) => { helix.echo("mode:" + mode); });
        "#,
        )
        .unwrap();

        assert!(has_handlers("save"));
        assert!(has_handlers("mode-change"));
        assert!(!has_handlers("buffer-open"));

        // emit 带编辑队列清空 + 多处理器按注册顺序
        let ctx = CommandContext {
            docs: vec![],
            path: Some("/tmp/e.rs".into()),
            text: "x".into(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        emit_event("save", &ctx, None).unwrap();
        assert!(take_edits().is_empty());

        // 事件名白名单校验 + 回调类型校验
        assert!(load_script(r#"helix.on("bogus", () => {});"#).is_err());
        assert!(load_script(r#"helix.on("save", 42);"#).is_err());

        // mode-change 处理器带 mode 参数 + echo
        emit_event("mode-change", &ctx, Some("insert")).unwrap();
        assert_eq!(take_messages(), vec!["mode:insert"]);
    }

    #[test]
    fn doc_change_event() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.on("doc-change", (doc) => { helix.echo("changed:" + doc.cursor.row); });
        "#,
        )
        .unwrap();
        assert!(has_handlers("doc-change"));
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: "x".into(),
            cursor: (2, 0),
            selection: ((0, 0), (0, 0)),
        };
        emit_event("doc-change", &ctx, None).unwrap();
        assert_eq!(take_messages(), vec!["changed:2"]);
        // 未注册的事件名仍然报错
        assert!(load_script(r#"helix.on("bogus", () => {});"#).is_err());
    }

    #[test]
    fn doc_change_changes_merged_range() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.on("doc-change", (doc) => {
            if (doc.changes.length === 0) { helix.echo("empty"); return; }
            const c = doc.changes[0];
            helix.echo(c.oldRange.start.row + "," + c.oldRange.start.col + "-" + c.oldRange.end.row + "," + c.oldRange.end.col + "|" + c.newRange.start.row + "," + c.newRange.start.col + "-" + c.newRange.end.row + "," + c.newRange.end.col);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: "hello world".into(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        // 两次插入:a 在 char0、b 在 char1 → 合并 old = (0,0)-(1,1),new = (0,0)-(1,2);行列按当前文本换算
        let changes = vec![((0, 0), (0, 1)), ((1, 1), (1, 2))];
        emit_doc_change(&ctx, &changes).unwrap();
        assert_eq!(take_messages(), vec!["0,0-0,1|0,0-0,2"]);
        // 无变更 → changes 为空数组
        emit_doc_change(&ctx, &[]).unwrap();
        assert_eq!(take_messages(), vec!["empty"]);
    }

    #[test]
    fn keymap_registration() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();

        // 字符串命令 → MapKey 入队
        load_script(r#"helix.map("normal", "gd", "goto-def");"#).unwrap();
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 1);
        match &reqs[0] {
            UiRequest::MapKey { mode, key, command } => {
                assert_eq!(mode, "normal");
                assert_eq!(key, "gd");
                assert_eq!(command, "goto-def");
            }
            other => panic!("expected MapKey, got {other:?}"),
        }

        // 回调 → 注册 __mapped_N + MapKey 入队
        load_script(
            r#"
        helix.map("insert", "C-n", () => { helix.echo("cb"); });
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 1);
        let command = match &reqs[0] {
            UiRequest::MapKey { command, .. } => command.clone(),
            other => panic!("expected MapKey, got {other:?}"),
        };
        assert!(command.starts_with("__mapped_"), "command: {command}");
        // 注册的命令可以运行（与普通插件命令同机制）
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command(&command, &ctx).unwrap());
        assert_eq!(take_messages(), vec!["cb"]);

        // 校验失败
        assert!(load_script(r#"helix.map("bogus", "x", "y");"#).is_err());
        assert!(load_script(r#"helix.map("normal", 42, "y");"#).is_err());
        assert!(load_script(r#"helix.map("normal", "x", 42);"#).is_err());
        assert!(load_script(r#"helix.map("normal", "", "y");"#).is_err());
    }

    #[test]
    fn statusline_hook() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let ctx = StatuslineCtx {
            path: Some("/tmp/a.rs".into()),
            mode: "insert".into(),
            cursor: (3, 7),
            total_lines: 100,
            diagnostics_error: 2,
            diagnostics_warning: 1,
            window_mode: false,
            active_leaf_type: "editor".into(),
            active_leaf_path: None,
            modified: false,
            selections: 1,
            selections_primary: 0,
        };
        assert_eq!(statusline_parts(&ctx), None);

        // 字符串 → 单段（style None）
        load_script(r#"helix.set_statusline((ctx) => ctx.mode + ":" + ctx.cursor.row);"#).unwrap();
        assert_eq!(
            statusline_parts(&ctx),
            Some(vec![StatuslinePart {
                text: "insert:3".into(),
                style: None,
                zone: None
            }])
        );

        // 数组 → 多段（字符串项 / {text, style} 项混用；style 透传）
        load_script(
            r#"
        helix.set_statusline((ctx) => [
            { text: " N ", style: "ui.statusline.normal" },
            ctx.mode + ":" + ctx.cursor.row,
            { text: String(ctx.diagnostics_error), style: "error", zone: "right" },
        ]);
        "#,
        )
        .unwrap();
        assert_eq!(
            statusline_parts(&ctx),
            Some(vec![
                StatuslinePart {
                    text: " N ".into(),
                    style: Some("ui.statusline.normal".into()),
                    zone: None
                },
                StatuslinePart {
                    text: "insert:3".into(),
                    style: None,
                    zone: None
                },
                StatuslinePart {
                    text: "2".into(),
                    style: Some("error".into()),
                    zone: Some("right".into())
                },
            ])
        );

        // 返回 null → None
        load_script(r#"helix.set_statusline(() => null);"#).unwrap();
        assert_eq!(statusline_parts(&ctx), None);
        // 抛错 → None
        load_script(r#"helix.set_statusline(() => { throw new Error("boom"); });"#).unwrap();
        assert_eq!(statusline_parts(&ctx), None);
        // set_statusline(null) 清除
        load_script(r#"helix.set_statusline(null);"#).unwrap();
        assert_eq!(statusline_parts(&ctx), None);
        // 非法参数 → JS 报错
        assert!(load_script(r#"helix.set_statusline(42);"#).is_err());
    }

    #[test]
    fn command_docs() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("doc1", () => {}, "first doc");
        helix.register_command("nodoc", () => {});
        helix.register_command("nulldoc", () => {}, undefined);
        "#,
        )
        .unwrap();
        assert_eq!(command_doc("doc1"), Some("first doc".to_string()));
        assert_eq!(command_doc("nodoc"), None);
        assert_eq!(command_doc("nulldoc"), None);
        assert_eq!(command_doc("missing"), None);
        // 非法 doc 类型 → 报错
        assert!(load_script(r#"helix.register_command("bad", () => {}, 42);"#).is_err());
    }

    #[test]
    fn plugin_reload() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script_named(
            "a.js",
            r#"helix.register_command("reload-cmd", () => { helix.echo("v1"); });"#,
        )
        .unwrap();
        load_script_named("b.js", r#"helix.on("save", () => {});"#).unwrap();

        assert!(has_handlers("save"));
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("reload-cmd", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["v1"]);

        // reload：清空状态后重跑全部已记录脚本
        reload_all().unwrap();
        assert!(has_handlers("save"), "handlers re-registered after reload");
        assert!(run_command("reload-cmd", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["v1"]);
        let _ = take_ui_requests(); // 清 reload_all 入队的 ClosePanel
    }

    /// 热重载不继承旧装饰:reload_all 在 reset 后 push 全局 Clear(doc: None),
    /// 使 term 侧已应用的旧装饰在下一帧被清空。
    #[test]
    fn decorations_cleared_on_reload() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(r#"helix.register_command("dec-dummy", () => {});"#).unwrap();
        // load_script 自身会 push 一个全局 Clear(见 decorations_cleared_on_script_load),先取走
        assert_eq!(take_decorations().len(), 1);
        assert!(take_decorations().is_empty());
        reload_all().unwrap();
        let reqs = take_decorations();
        assert!(
            reqs.iter()
                .any(|r| r.doc.is_none() && matches!(r.kind, crate::types::DecorationKind::Clear)),
            "reload 后应有全局 Clear: {reqs:?}"
        );
    }

    #[test]
    fn reload_keeps_plugins_with_deps_and_hooks() {
        // 模拟真实 init.js 场景:相对名 + deps 声明 + set_statusline + 命令 + 事件
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_path_buf();
        std::mem::forget(dir); // 线程池复用,目录泄漏保确定性(同文件其他测试惯例)
        set_plugins_dir(d.clone());
        std::fs::create_dir_all(d.join("lib")).unwrap();
        std::fs::create_dir_all(d.join("features")).unwrap();
        std::fs::write(
            d.join("lib/icons.js"),
            r#"helix.plugin("icons", { deps: [] }); helix.export({ src: "icons" });"#,
        )
        .unwrap();
        std::fs::write(
            d.join("features/statusline.js"),
            r#"helix.plugin("statusline", { deps: ["lib/icons.js"] });
               const IC = helix.load("lib/icons.js");
               helix.set_statusline(() => "SL" + (IC ? "-" + IC.src : ""));"#,
        )
        .unwrap();
        std::fs::write(
            d.join("features/filetree.js"),
            r#"helix.plugin("filetree", { deps: ["lib/icons.js"] });
               helix.register_command("ft", () => helix.echo("ft-ok"));"#,
        )
        .unwrap();
        // 模拟 init.js:绝对路径名加载(application.rs 同款),脚本内依赖也用绝对路径(PLUGINS_DIR 全局共享,不依赖)
        let init_path = d.join("init.js");
        let icons_abs = d.join("lib/icons.js").to_string_lossy().into_owned();
        let init_src = format!(
            r#"helix.load({statusline:?}); helix.load({filetree:?});"#,
            statusline = d.join("features/statusline.js").to_string_lossy(),
            filetree = d.join("features/filetree.js").to_string_lossy(),
        );
        // 脚本内依赖改绝对路径:先重写两个 feature 文件的 load 行
        let sl = std::fs::read_to_string(d.join("features/statusline.js")).unwrap();
        std::fs::write(
            d.join("features/statusline.js"),
            sl.replace("\"lib/icons.js\"", &format!("\"{icons_abs}\"")),
        )
        .unwrap();
        let ft = std::fs::read_to_string(d.join("features/filetree.js")).unwrap();
        std::fs::write(
            d.join("features/filetree.js"),
            ft.replace("\"lib/icons.js\"", &format!("\"{icons_abs}\"")),
        )
        .unwrap();
        load_script_named(&init_path.display().to_string(), &init_src).unwrap();
        // 模拟启动时的相对名记录:load 已把 features/* 记录为相对名
        let ctx = CommandContext {
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
            docs: vec![],
        };
        assert!(run_command("ft", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["ft-ok"]);
        let _ = take_ui_requests();
        // reload:重跑全部(init.js + features/*)
        reload_all().unwrap();
        assert!(
            run_command("ft", &ctx).unwrap(),
            "filetree 命令 reload 后仍可用"
        );
        assert_eq!(take_messages(), vec!["ft-ok"]);
        let _ = take_ui_requests();
    }
    #[test]
    fn reload_after_init_midway_failure_keeps_dependent_plugins() {
        // 模拟用户场景:init.js 第一个插件(terminal)reload 失败 → init.js eval 中断,
        // 但循环继续单独 eval 后续 features/*——它们依赖 icons(缓存被 reset 清空),
        // 重新 eval icons = 嵌套 eval,可能污染后续闭包。验证命令回调完好。
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_path_buf();
        std::mem::forget(dir);
        set_plugins_dir(d.clone());
        std::fs::create_dir_all(d.join("lib")).unwrap();
        std::fs::create_dir_all(d.join("features")).unwrap();
        std::fs::write(
            d.join("lib/icons.js"),
            r#"helix.plugin("icons", { deps: [] }); helix.export({ src: "icons" });"#,
        )
        .unwrap();
        std::fs::write(
            d.join("features/bad.js"),
            r#"helix.register_command("bad", () => {});"#,
        )
        .unwrap();
        std::fs::write(
            d.join("features/plugin_a.js"),
            r#"helix.plugin("plugin_a", { deps: ["lib/icons.js"] });
               const IC = helix.load("lib/icons.js");
               helix.register_command("pa", () => helix.echo("pa:" + (IC && IC.src)));"#,
        )
        .unwrap();
        let bad_abs = d.join("features/bad.js").to_string_lossy().into_owned();
        let a_abs = d
            .join("features/plugin_a.js")
            .to_string_lossy()
            .into_owned();
        // PLUGINS_DIR 若已被其他测试占用,plugin_a 内部相对名 load 会失败——改写为绝对路径
        let icons_abs = d.join("lib/icons.js").to_string_lossy().into_owned();
        let a = std::fs::read_to_string(d.join("features/plugin_a.js")).unwrap();
        std::fs::write(
            d.join("features/plugin_a.js"),
            a.replace("\"lib/icons.js\"", &format!("\"{icons_abs}\"")),
        )
        .unwrap();
        let init_path = d.join("init.js");
        let init_src = format!(
            r#"helix.load({bad:?}); helix.load({a:?});"#,
            bad = bad_abs,
            a = a_abs
        );
        load_script_named(&init_path.display().to_string(), &init_src).unwrap();
        let ctx = CommandContext {
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
            docs: vec![],
        };
        assert!(run_command("pa", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["pa:icons"]);
        let _ = take_ui_requests();
        // 磁盘 bad 改坏版 → reload:init.js 在 bad 处失败,plugin_a 单独 eval
        std::fs::write(
            d.join("features/bad.js"),
            r#"helix.register_command("bad", () => { BAD SYNTAX"#,
        )
        .unwrap();
        let err = reload_all().unwrap_err();
        assert!(err.to_string().contains("failed"), "应有失败汇总: {err}");
        let _ = take_messages();
        let _ = take_ui_requests();
        assert!(
            run_command("pa", &ctx).is_ok(),
            "plugin_a 命令 reload 后应可调用"
        );
        let msgs = take_messages();
        assert_eq!(
            msgs.first().map(|s| s.as_str()),
            Some("pa:icons"),
            "回调结果应正确(未被污染),got {msgs:?}"
        );
    }

    #[test]
    fn reload_users_real_plugins_dir() {
        // 复制用户真实 ~/.config/helix/plugins 到 tempdir,用用户 init.js 序列 reload。
        // 用户 filetree 是改版(offset scroll)+ 有 test.js——最接近真实环境。
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // PLUGINS_DIR 被占则相对名解析失效——跳过(核心行为由其他 reload 测试覆盖)
        if crate::state::PLUGIN_ROOTS.get().is_some() {
            return;
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let user_plugins = std::path::PathBuf::from(&home).join(".config/helix/plugins");
        if !user_plugins.join("features/filetree/index.js").is_file() {
            return; // 无用户环境(CI)跳过
        }
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_path_buf();
        std::mem::forget(dir);
        fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
            for e in std::fs::read_dir(src).unwrap() {
                let e = e.unwrap();
                let to = dst.join(e.file_name());
                if e.file_type().unwrap().is_dir() {
                    std::fs::create_dir_all(&to).unwrap();
                    copy_dir(&e.path(), &to);
                } else {
                    std::fs::copy(e.path(), to).unwrap();
                }
            }
        }
        copy_dir(&user_plugins, &d);
        set_plugins_dir(d.clone());
        // 用户 init.js(无 terminal——用户已禁用)
        let init_path = d.join("init.js");
        let init_src = r#"helix.load("features/statusline.js");
helix.load("features/filetree/index.js");
helix.load("features/which-key.js");
helix.load("features/picker.js")
helix.map("normal", "space-f", () => helix.picker.run("files"));"#;
        let r = load_script_named(&init_path.display().to_string(), init_src);
        if let Err(e) = &r {
            panic!("init load failed: {e}");
        }
        let _ = take_messages();
        let _ = take_ui_requests();
        if let Err(e) = reload_all() {
            panic!("reload_all failed: {e}");
        }
        let _ = take_messages();
        let _ = take_ui_requests();
        let ctx = CommandContext {
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
            docs: vec![],
        };
        assert!(
            run_command("filetree", &ctx).is_ok(),
            "用户 filetree 命令 reload 后应可调用"
        );
    }

    #[test]
    fn reload_missing_relative_load_then_dependent_pollution() {
        // 用户场景:init.js 里 load 一个不存在的相对名文件(如"禁用"后仍留 load 行的 terminal.js)
        // → init.js eval 失败 → 循环单独 eval 后续 features/* → 它们相对名 load icons(重新 eval=嵌套)
        // → 后续注册的闭包可能被 boa 嵌套 eval 污染。验证命令回调结果。
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        if crate::state::PLUGIN_ROOTS.get().is_some() {
            return; // 需要独占 PLUGINS_DIR(相对名解析)
        }
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_path_buf();
        std::mem::forget(dir);
        set_plugins_dir(d.clone());
        std::fs::create_dir_all(d.join("lib")).unwrap();
        std::fs::create_dir_all(d.join("features")).unwrap();
        std::fs::write(
            d.join("lib/icons.js"),
            r#"helix.plugin("icons", { deps: [] }); helix.export({ src: "icons" });"#,
        )
        .unwrap();
        // plugin_a:相对名依赖 icons,注册命令回调读 ICONS
        std::fs::write(
            d.join("features/plugin_a.js"),
            r#"helix.plugin("plugin_a", { deps: ["lib/icons.js"] });
               const IC = helix.load("lib/icons.js");
               helix.register_command("pa", () => helix.echo("pa:" + (IC && IC.src)));"#,
        )
        .unwrap();
        // init.js:load 不存在的 terminal.js(用户"禁用"方式)+ plugin_a
        let init_path = d.join("init.js");
        std::fs::write(
            &init_path,
            r#"helix.load("features/terminal.js"); helix.load("features/plugin_a.js");"#,
        )
        .unwrap();
        // 启动模拟:load_script_named 绝对路径 init.js —— 但 init.js 内部 load 相对名,
        // 依赖 PLUGINS_DIR(独占,有效)
        let init_abs = init_path.display().to_string();
        // 启动时 terminal.js 不存在 → load 失败 → init.js 加载失败(用户启动时 terminal 存在,reload 时删了?)
        // 更贴近:先让 terminal 存在(启动 OK),reload 前删掉 → reload 时 init.js 重跑失败
        std::fs::write(
            d.join("features/terminal.js"),
            r#"helix.register_command("term", () => {});"#,
        )
        .unwrap();
        let init_src = std::fs::read_to_string(&init_path).unwrap();
        load_script_named(&init_abs, &init_src).unwrap();
        let ctx = CommandContext {
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
            docs: vec![],
        };
        assert!(run_command("pa", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["pa:icons"]);
        let _ = take_ui_requests();
        // reload 前删 terminal.js(用户"禁用"= 删文件但 init.js 留着 load)→ reload 重跑 init.js 失败
        std::fs::remove_file(d.join("features/terminal.js")).unwrap();
        let err = reload_all().unwrap_err();
        assert!(
            err.to_string().contains("terminal.js"),
            "应报 terminal.js 缺失: {err}"
        );
        let _ = take_messages();
        let _ = take_ui_requests();
        // plugin_a 单独 eval(init.js 中断后)命令回调应完好
        assert!(run_command("pa", &ctx).is_ok());
        let msgs = take_messages();
        assert_eq!(
            msgs.first().map(|s| s.as_str()),
            Some("pa:icons"),
            "回调未被污染, got {msgs:?}"
        );
    }

    fn reload_failed_script_does_not_block_others() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_path_buf();
        std::mem::forget(dir);
        set_plugins_dir(d.clone());
        // 先写好版并加载;之后磁盘改坏版 → reload 重读磁盘时失败
        std::fs::write(
            d.join("bad.js"),
            r#"helix.register_command("bad", () => {});"#,
        )
        .unwrap();
        std::fs::write(
            d.join("good.js"),
            r#"helix.register_command("good", () => helix.echo("still-here"));"#,
        )
        .unwrap();
        // 绝对路径加载(PLUGINS_DIR 全局共享不可靠)
        let bad_abs = d.join("bad.js").to_string_lossy().into_owned();
        let good_abs = d.join("good.js").to_string_lossy().into_owned();
        load_script_named(
            &bad_abs,
            &std::fs::read_to_string(d.join("bad.js")).unwrap(),
        )
        .unwrap();
        load_script_named(
            &good_abs,
            &std::fs::read_to_string(d.join("good.js")).unwrap(),
        )
        .unwrap();
        let ctx = CommandContext {
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
            docs: vec![],
        };
        assert!(run_command("good", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["still-here"]);
        // 坏脚本名:bad.js(相对名判断用;磁盘读按绝对路径)
        std::fs::write(
            d.join("bad.js"),
            r#"helix.register_command("bad", () => { THIS IS NOT VALID"#,
        )
        .unwrap();
        // reload:bad 磁盘版失败,不中止 good
        let err = reload_all().unwrap_err();
        assert!(
            err.to_string().contains("1 plugin(s) failed"),
            "汇总错误: {err}"
        );
        assert!(err.to_string().contains("bad.js"), "错误含脚本名: {err}");
        let _ = take_ui_requests();
        assert!(
            run_command("good", &ctx).is_ok(),
            "good 脚本 reload 后仍可用(不被 bad 阻塞)"
        );
    }

    #[test]
    fn circular_runtime_load_errors_not_crash() {
        // 回归:跨脚本运行时 helix.load 循环(A load B,B load A)曾无限嵌套 eval 递归
        // → 栈溢出 SIGABRT。js_load 每次新建空栈导致循环检测失效;现在共享加载栈,
        // 循环应报错退出而非崩溃。用绝对路径避免占用全局 PLUGINS_DIR(与其他测试并行)。
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_path_buf();
        std::mem::forget(dir);
        std::fs::create_dir_all(d.join("features")).unwrap();
        let a_abs = d.join("features/a.js").to_string_lossy().into_owned();
        let b_abs = d.join("features/b.js").to_string_lossy().into_owned();
        let a_src = format!(r#"const B = helix.load("{b_abs}"); helix.export(() => B);"#);
        let b_src = format!(r#"const A = helix.load("{a_abs}"); helix.export(() => A);"#);
        std::fs::write(d.join("features/a.js"), a_src).unwrap();
        std::fs::write(d.join("features/b.js"), b_src).unwrap();
        let init_path = d.join("init.js");
        let init_src = format!(r#"helix.load("{a_abs}");"#);
        std::fs::write(&init_path, init_src).unwrap();
        let r = load_script_named(
            &init_path.display().to_string(),
            &std::fs::read_to_string(&init_path).unwrap(),
        );
        let msg = format!("{r:?}");
        assert!(r.is_err(), "循环 load 应报错退出而非崩溃, got {msg}");
        assert!(msg.contains("circular"), "错误应说明循环依赖, got {msg}");
    }

    #[test]
    fn reload_all_real_plugins_keeps_commands() {
        // 加载仓库全部真实插件(init.js 同款序列)+ reload,验证命令/钩子保留
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // PLUGINS_DIR 是全局 OnceLock:被其他测试(如 plugin_deps_auto_load,字母序在前)占住后
        // 本测试的相对名解析失效——跳过(核心 reload 行为由 reload_keeps/reload_failed 覆盖)
        if crate::state::PLUGIN_ROOTS.get().is_some() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_path_buf();
        std::mem::forget(dir);
        // 复制仓库 plugins/ 到临时目录
        fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
            for e in std::fs::read_dir(src).unwrap() {
                let e = e.unwrap();
                let to = dst.join(e.file_name());
                if e.file_type().unwrap().is_dir() {
                    std::fs::create_dir_all(&to).unwrap();
                    copy_dir(&e.path(), &to);
                } else {
                    std::fs::copy(e.path(), to).unwrap();
                }
            }
        }
        copy_dir(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../plugins")),
            &d,
        );
        set_plugins_dir(d.clone());
        // 模拟 init.js:加载 features(含 terminal/which-key/tabbar/picker + map 回调)
        let init_src = r#"helix.load("features/terminal.js");
helix.load("features/statusline.js");
helix.load("features/filetree/index.js");
helix.load("features/which-key.js");
helix.load("features/tabbar.js");
helix.load("features/picker.js")
helix.map("normal", "space-f", () => helix.picker.run("files"));"#;
        let init_path = d.join("init.js");
        let r = load_script_named(&init_path.display().to_string(), init_src);
        if let Err(e) = &r {
            panic!("init load failed: {e}");
        }
        let _ = take_messages();
        let _ = take_ui_requests();
        // reload
        if let Err(e) = reload_all() {
            panic!("reload_all failed: {e}");
        }
        let _ = take_messages();
        let _ = take_ui_requests();
        // 验证:filetree 命令在(真实插件注册的命令)
        let ctx = CommandContext {
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
            docs: vec![],
        };
        assert!(
            run_command("filetree", &ctx).is_ok(),
            "filetree 命令 reload 后应可调用"
        );
    }

    #[test]
    #[test]
    fn reload_real_statusline_render_ok() {
        // 用户场景:statusline.js 依赖 icons,reload 后 render(状态栏渲染)不应抛错/返回空
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        if crate::state::PLUGIN_ROOTS.get().is_some() {
            return; // 需要独占 PLUGINS_DIR(相对名 icons)
        }
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_path_buf();
        std::mem::forget(dir);
        set_plugins_dir(d.clone());
        std::fs::create_dir_all(d.join("lib")).unwrap();
        std::fs::create_dir_all(d.join("features")).unwrap();
        // 复制仓库 statusline.js + icons.js
        let repo = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../plugins"));
        std::fs::copy(
            repo.join("features/statusline.js"),
            d.join("features/statusline.js"),
        )
        .unwrap();
        std::fs::copy(repo.join("lib/icons.js"), d.join("lib/icons.js")).unwrap();
        let src = std::fs::read_to_string(d.join("features/statusline.js")).unwrap();
        let r = load_script_named("features/statusline.js", &src);
        eprintln!("load statusline result: {r:?}");
        r.unwrap();
        let _ = take_messages();
        let hook_set = crate::state::with_statusline_hook(|h| h.is_some());
        eprintln!("statusline hook registered: {hook_set}");
        crate::state::with_engine(|e| {
            let callable = crate::state::with_statusline_hook(|h| {
                h.as_ref().map(|v| v.as_callable().is_some())
            });
            eprintln!("hook callable: {callable:?}");
        });
        let ctx = StatuslineCtx {
            path: Some("/tmp/a.rs".into()),
            mode: "insert".into(),
            cursor: (3, 7),
            total_lines: 100,
            diagnostics_error: 2,
            diagnostics_warning: 1,
            window_mode: false,
            active_leaf_type: "editor".into(),
            active_leaf_path: None,
            modified: false,
            selections: 1,
            selections_primary: 0,
        };
        // reload 前 render 正常
        let before = statusline_parts(&ctx);
        eprintln!("reload 前 statusline_parts: {before:?}");
        assert!(
            before.is_some() && !before.unwrap().is_empty(),
            "reload 前 statusline 应有内容"
        );
        // reload
        reload_all().unwrap();
        let _ = take_messages();
        let _ = take_ui_requests();
        // reload 后 render 应正常(非空,不抛错)
        let after = statusline_parts(&ctx);
        assert!(after.is_some(), "reload 后 statusline render 不应抛错");
        assert!(!after.unwrap().is_empty(), "reload 后 statusline 应有内容");
    }

    fn reload_closes_open_panel_layer() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 面板由命令打开（不在加载时），保证 reload 重跑脚本不会自动重开面板
        load_script(
            r#"helix.register_command("open-panel", () => { helix.open_panel({ side: "right", size: 30, render: () => ["p1"] }); });"#,
        )
        .unwrap();
        let ctx = CommandContext {
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
            docs: vec![],
        };
        assert!(run_command("open-panel", &ctx).unwrap());
        assert!(matches!(take_ui_requests()[0], UiRequest::OpenPanel { .. }));

        // reload：面板层还挂在 compositor 上 → 必须入队 ClosePanel 供 apply_ui_requests 移除
        reload_all().unwrap();
        let reqs = take_ui_requests();
        assert!(
            matches!(&reqs[0], UiRequest::ClosePanel { .. }),
            "layer removed after reload"
        );
        // LAST_PANEL_ID 已清空 → :panel-close 不再误报有面板
        assert!(close_last_panel().is_err());
    }

    #[test]
    fn selection_and_cursor() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("selcmd", (ctx) => {
            helix.set_cursor(2, 3);
            helix.set_selection(0, 1, 0, 5);
            helix.echo("sel:" + ctx.selection.anchor.row + "," + ctx.selection.anchor.col + "-" + ctx.selection.head.row + "," + ctx.selection.head.col);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            path: None,
            text: "abc\ndef\nghi".into(),
            cursor: (0, 0),
            selection: ((0, 1), (0, 5)),
            docs: vec![],
        };
        assert!(run_command("selcmd", &ctx).unwrap());
        let reqs = take_cursor_requests();
        assert_eq!(
            reqs,
            vec![
                CursorRequest::SetCursor { row: 2, col: 3 },
                CursorRequest::SetSelection {
                    anchor: (0, 1),
                    head: (0, 5)
                },
            ]
        );
        assert_eq!(take_messages(), vec!["sel:0,1-0,5"]);

        // 类型错误 → 命令失败（run_command 返回 Err），请求队列被清空
        load_script(r#"helix.register_command("badsel", (ctx) => { helix.set_cursor("x", 0); });"#)
            .unwrap();
        assert!(run_command("badsel", &ctx).is_err());
        assert!(take_cursor_requests().is_empty());
    }

    #[test]
    fn set_selection_array() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 数组形态 → SetSelections 入队
        load_script(
            r#"
        helix.set_selection([{ anchor: {row: 0, col: 0}, head: {row: 0, col: 2} },
                             { anchor: {row: 2, col: 1}, head: {row: 2, col: 4} }]);
        "#,
        )
        .unwrap();
        let reqs = take_cursor_requests();
        assert!(matches!(&reqs[0], CursorRequest::SetSelections(v) if v.len() == 2));
        // 空数组 → JS 报错
        assert!(load_script(r#"helix.set_selection([]);"#).is_err());
        // 缺字段 → JS 报错
        assert!(load_script(r#"helix.set_selection([{ anchor: {row:0,col:0} }]);"#).is_err());
        // 4 参兼容
        load_script(r#"helix.set_selection(0, 0, 1, 1);"#).unwrap();
        let reqs = take_cursor_requests();
        assert!(matches!(&reqs[0], CursorRequest::SetSelection { .. }));
    }

    #[test]
    fn shell_run() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 成功路径
        load_script(
            r#"helix.register_command("r1", () => { helix.echo(helix.run("echo hi")); });"#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("r1", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["hi\n"]);

        // 非零退出码 → 错误
        load_script(
            r#"helix.register_command("r2", () => { helix.run("echo boom >&2; exit 3"); });"#,
        )
        .unwrap();
        let err = run_command("r2", &ctx).unwrap_err().to_string();
        assert!(err.contains("command failed"), "err: {err}");
        assert!(err.contains("boom"), "stderr should be included: {err}");

        // 类型错误
        load_script(r#"helix.register_command("r3", () => { helix.run(42); });"#).unwrap();
        assert!(run_command("r3", &ctx).is_err());

        // 截断：输出超限 → 返回长度 ≤ 65536
        load_script(
            r#"
        helix.register_command("r4", () => {
            const out = helix.run("head -c 100000 /dev/zero | tr '\\0' 'x'");
            helix.echo("len:" + out.length + " tail:" + out.slice(-11));
        });
        "#,
        )
        .unwrap();
        assert!(run_command("r4", &ctx).unwrap());
        let msg = take_messages();
        assert!(
            msg[0].contains("tail:(truncated)"),
            "marker expected: {:?}",
            msg[0]
        );
        let len: usize = msg[0]
            .strip_prefix("len:")
            .unwrap()
            .split(" tail:")
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            len <= 65536 + "(truncated)".len(),
            "truncated output, len={len}"
        );
    }

    /// 轮询 drain_term_events 直到谓词命中或超时（async 测试需要）。
    /// 累积自调用以来的全部事件返回：进程事件是 Chunk(s)→Exit 的顺序流，
    /// 命中谓词的那次 drain 之前可能已有事件被取走，须一并保留按序 resolve。
    fn wait_for_term_event(pred: impl Fn(&TermEvent) -> bool) -> Vec<TermEvent> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut all = Vec::new();
        loop {
            all.extend(drain_term_events());
            if all.iter().any(&pred) {
                return all;
            }
            if std::time::Instant::now() > deadline {
                panic!("timed out waiting for term event");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// 轮询 drain_async_events 直到谓词命中或超时，事件累积进调用方传入的 vec
    /// （跨调用共享累积：异步 fs 四个操作并发发送，后几次 wait 必须能看到先前已 drain 的事件）。
    fn wait_for_async(all: &mut Vec<AsyncEvent>, pred: impl Fn(&AsyncEvent) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            all.extend(drain_async_events());
            if all.iter().any(&pred) {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!("timed out waiting for async event");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn async_run_and_spawn() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();

        // run_async:Promise 模式 → Exit 事件 → resolve → pump_jobs 执行 .then
        load_script(
            r#"
        helix.run_async("echo async-hello").then((out) => {
            helix.echo("cb:ok:" + out.trim());
        });
        "#,
        )
        .unwrap();
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        let TermEvent::Exit(id, code, stdout) = &events[0] else {
            unreachable!()
        };
        assert_eq!(*code, 0);
        resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap();
        pump_jobs().unwrap();
        assert_eq!(take_messages(), vec!["cb:ok:async-hello"]);

        // run_async 错误路径:非零退出 → reject → .catch
        load_script(
            r#"
        helix.run_async("echo boom >&2; exit 3").catch((e) => {
            helix.echo("raerr:" + e.message);
        });
        "#,
        )
        .unwrap();
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        let TermEvent::Exit(id2, code2, _) = &events[0] else {
            unreachable!()
        };
        assert_ne!(*code2, 0);
        resolve_term_event(*id2, TermEvent::Exit(*id2, *code2, None)).unwrap();
        pump_jobs().unwrap();
        let msgs = take_messages();
        assert!(msgs[0].contains("raerr:"), "{msgs:?}");

        // spawn 流式：cat 回显
        load_script(
            r#"
        helix.register_command("sp", () => {
            const id = helix.spawn({ cmd: "cat", onChunk: (c) => helix.echo("chunk:" + c), onExit: (code) => helix.echo("exit:" + code) });
            helix.term_write(id, "hello-term\n");
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("sp", &ctx).unwrap());
        let events = wait_for_term_event(
            |e| matches!(e, TermEvent::Chunk(_, c) if c.contains("hello-term")),
        );
        let ev = events
            .iter()
            .find(|e| matches!(e, TermEvent::Chunk(_, c) if c.contains("hello-term")))
            .expect("chunk event");
        let TermEvent::Chunk(id, chunk) = ev else {
            unreachable!()
        };
        resolve_term_event(*id, TermEvent::Chunk(*id, chunk.clone())).unwrap();
        assert!(take_messages().contains(&format!("chunk:{chunk}")));

        // term_kill：sleep 100 → kill → Exit 快到达
        load_script(
            r#"
        helix.register_command("kp", () => {
            const id = helix.spawn({ cmd: "sleep 100", onChunk: () => {}, onExit: (code) => helix.echo("killed:" + code) });
            helix.term_kill(id);
        });
        "#,
        )
        .unwrap();
        assert!(run_command("kp", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        let TermEvent::Exit(id, code, _) = &events[0] else {
            unreachable!()
        };
        assert!(*code != 0, "killed process should have non-zero exit");
        resolve_term_event(*id, TermEvent::Exit(*id, *code, None)).unwrap();
        assert!(take_messages()[0].starts_with("killed:"));

        // 类型校验：run_async/spawn 参数错误在 load 时即报错
        // 注:原 helix.run_async("x", 42) 断言是回调校验报错,现 JS 多参天然容忍 → 删除
        assert!(load_script(r#"helix.run_async(42);"#).is_err());
        assert!(load_script(r#"helix.spawn({ cmd: "x" });"#).is_err()); // 缺 onChunk
                                                                        // term_write 未知 id 在 load 时不会执行（命令体），须放进命令里跑
        load_script(r#"helix.register_command("badid", () => { helix.term_write(999, "x"); });"#)
            .unwrap();
        assert!(run_command("badid", &ctx).is_err());
    }

    #[test]
    fn async_utf8_across_chunks() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();

        // run_async 聚合：19999 字节 CJK 输出（"中文\n"×2857，7 字节/行）跨多个 4096 块，
        // 每块边界都可能切开 3 字节字符——修复前逐块 from_utf8_lossy 会产出 U+FFFD。
        // head -c 19999 恰好截在行边界（19999 = 7×2857），整流是合法 UTF-8。
        load_script(
            r#"
        helix.run_async("yes 中文 | head -c 19999").then((out) => {
            helix.echo("len:" + out.length + " tail:" + out.slice(-11));
        });
        "#,
        )
        .unwrap();
        // 只等聚合模式的 Exit（携带 Some(stdout)），遗漏的遗留 Exit 一并 resolve 清理
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, Some(_))));
        let ev = events
            .iter()
            .find(|e| matches!(e, TermEvent::Exit(_, _, Some(_))))
            .expect("run_async exit");
        let TermEvent::Exit(_, code, stdout) = ev else {
            unreachable!()
        };
        assert_eq!(*code, 0);
        let out = stdout.clone().unwrap_or_default();
        assert_eq!(out.len(), 19999, "aggregated bytes intact across chunks");
        assert!(
            !out.contains('\u{FFFD}'),
            "no replacement chars in aggregated output"
        );
        assert_eq!(
            out.matches("中文").count(),
            2857,
            "CJK lines preserved (7 bytes/line)"
        );
        for ev in &events {
            if let TermEvent::Exit(id, code, stdout) = ev {
                resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap();
            }
        }
        pump_jobs().unwrap();
        let msg = take_messages();
        let m = msg
            .iter()
            .find(|m| m.starts_with("len:"))
            .expect("run_async echo");
        // JS 收到完整输出：19999 字节 = 2857 行 × 3 个 BMP 字符 = 8571 个 UTF-16 单元
        assert!(m.starts_with("len:8571 tail:"), "full output length: {m}");
        assert!(m.contains("中文"), "tail should be CJK: {m}");

        // spawn 流式：同一输出经 onChunk 增量解码拼接，块边界不得产生 U+FFFD
        load_script(
            r#"
        helix.register_command("spcjk", () => {
            const parts = [];
            const id = helix.spawn({
                cmd: "yes 中文 | head -c 19999",
                onChunk: (c) => parts.push(c),
                onExit: (code) => helix.echo("spawn:" + code + ":" + parts.length + ":" + parts.join("")),
            });
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("spcjk", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        // 按序 resolve：先 Chunk 后 Exit，JS 的 parts 才能拼全
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, chunk) => {
                    resolve_term_event(*id, TermEvent::Chunk(*id, chunk.clone())).unwrap();
                }
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap();
                }
            }
        }
        let msg = take_messages();
        let m = msg
            .iter()
            .find(|m| m.starts_with("spawn:0:"))
            .expect("spawn echo");
        let mut it = m.splitn(4, ':');
        assert_eq!(it.next(), Some("spawn"));
        assert_eq!(it.next(), Some("0"));
        let chunks: usize = it.next().unwrap().parse().expect("chunk count");
        assert!(
            chunks >= 4,
            "streaming should split into multiple chunks, got {chunks}"
        );
        let joined = it.next().unwrap();
        assert_eq!(joined.len(), 19999, "streamed bytes intact across chunks");
        assert!(
            !joined.contains('\u{FFFD}'),
            "no replacement chars in streamed output"
        );
        assert_eq!(
            joined.matches("中文").count(),
            2857,
            "CJK lines preserved in streamed output"
        );
    }

    /// 任务简报验证测试：四个异步 fs API（read/write/stat/glob）Promise → echo；
    /// 错误路径 reject（e.message）；参数类型校验。
    /// 注（相对简报的测试侧调整）：wait_for_async 把事件累积进共享 vec（四个 worker
    /// 并发发送，四次顺序 wait 需共享累积）；简报注释 "resolve 全部" 落实为逐事件 resolve。
    #[test]
    fn async_fs() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = std::env::temp_dir().join(format!("helix-js-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello fs").unwrap();
        std::fs::write(dir.join("b.js"), "x").unwrap();

        load_script(&format!(
            r#"
        helix.register_command("fsd", () => {{
            helix.read_file_async("{dir}/a.txt").then((content) => {{
                helix.echo("read::" + content);
            }});
            helix.write_file_async("{dir}/out.txt", "written").then(() => {{
                helix.echo("write:ok");
            }});
            helix.stat_async("{dir}/a.txt").then((st) => {{
                helix.echo("stat:" + st.size + ":" + st.is_dir);
            }});
            helix.glob_async("{dir}/*.js").then((paths) => {{
                helix.echo("glob:" + paths.length);
            }});
        }});
    "#,
            dir = dir.display()
        ))
        .unwrap();

        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("fsd", &ctx).unwrap());
        // 轮询 drain_async_events 直到四个回调都到（wait_for_async 辅助，仿 wait_for_term_event）
        let mut events = Vec::new();
        wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsRead(_, _)));
        wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsWrite(_, _)));
        wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsStat(_, _)));
        wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsGlob(_, _)));
        // resolve 全部（事件里带 id）→ 断言回调 echo
        for ev in events {
            let id = match &ev {
                AsyncEvent::FsRead(id, _)
                | AsyncEvent::FsWrite(id, _)
                | AsyncEvent::FsStat(id, _)
                | AsyncEvent::FsGlob(id, _)
                | AsyncEvent::FsTree(id, _) => *id,
            };
            resolve_async_event(id, ev).unwrap();
        }
        pump_jobs().unwrap();
        let msgs = take_messages();
        assert!(msgs.iter().any(|m| m == "read::hello fs"), "{msgs:?}");
        assert!(msgs.iter().any(|m| m == "write:ok"), "{msgs:?}");
        assert!(
            msgs.iter().any(|m| m.starts_with("stat:8:false")),
            "{msgs:?}"
        );
        assert!(msgs.iter().any(|m| m == "glob:1"), "{msgs:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join("out.txt")).unwrap(),
            "written"
        );

        // review: ** 中缀（`dir/**/*.js`）——嵌套目录也要命中（独立回合：wait_for_async 为
        // any 语义，同一事件类型不能连续 wait 两次）
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/deep.js"), "d").unwrap();
        load_script(&format!(
            r#"
        helix.register_command("fsd2", () => {{
            helix.glob_async("{dir}/**/*.js").then((paths) => {{
                helix.echo("glob2::" + paths.length);
            }});
        }});
    "#,
            dir = dir.display()
        ))
        .unwrap();
        assert!(run_command("fsd2", &ctx).unwrap());
        let mut g2 = Vec::new();
        wait_for_async(&mut g2, |e| matches!(e, AsyncEvent::FsGlob(_, _)));
        for ev in g2 {
            let id = match &ev {
                AsyncEvent::FsRead(id, _)
                | AsyncEvent::FsWrite(id, _)
                | AsyncEvent::FsStat(id, _)
                | AsyncEvent::FsGlob(id, _)
                | AsyncEvent::FsTree(id, _) => *id,
            };
            resolve_async_event(id, ev).unwrap();
        }
        pump_jobs().unwrap();
        let msgs2 = take_messages();
        assert!(
            msgs2.iter().any(|m| m == "glob2::2"),
            "** 应命中根目录+嵌套: {msgs2:?}"
        );

        // 错误路径：读不存在 → err 非空
        load_script(&format!(
            r#"
        helix.register_command("fsbad", () => {{
            helix.read_file_async("{dir}/nope.txt").catch((e) => {{
                helix.echo("bad:" + (e !== null ? "err" : "noerr"));
            }});
        }});
    "#,
            dir = dir.display()
        ))
        .unwrap();
        assert!(run_command("fsbad", &ctx).unwrap());
        let mut bad = Vec::new();
        wait_for_async(&mut bad, |e| matches!(e, AsyncEvent::FsRead(_, _)));
        // resolve → 断言
        for ev in bad {
            let id = match &ev {
                AsyncEvent::FsRead(id, _)
                | AsyncEvent::FsWrite(id, _)
                | AsyncEvent::FsStat(id, _)
                | AsyncEvent::FsGlob(id, _)
                | AsyncEvent::FsTree(id, _) => *id,
            };
            resolve_async_event(id, ev).unwrap();
        }
        pump_jobs().unwrap();
        assert!(take_messages().iter().any(|m| m == "bad:err"));

        // 类型校验：path 参数错误在 load 时即报错（回调校验已随 async_cb 删除，多参天然容忍）
        assert!(load_script(r#"helix.read_file_async(42);"#).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// review 回归：裸模式 `*.js`（基目录 = `.`，cwd 内匹配）——验证字面前缀基目录 +
    /// 前导 `./` 归一化。chdir 受 TEST_LOCK 保护（同进程单测串行，本 crate 全部测试取锁）。
    #[test]
    fn glob_bare_pattern() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = std::env::temp_dir().join(format!("helix-js-glob-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("x.js"), "x").unwrap();
        std::fs::write(dir.join("y.txt"), "y").unwrap();
        std::fs::write(dir.join("sub/z.js"), "z").unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let bare = glob_matches("*.js");
        let dbl = glob_matches("**/*.js");
        let explicit = glob_matches("./*.js"); // ./ 归一化与裸模式一致
        std::env::set_current_dir(cwd).unwrap();
        assert_eq!(bare.unwrap(), vec!["x.js"], "裸 * 不跨目录分隔符");
        assert_eq!(
            dbl.unwrap(),
            vec!["sub/z.js", "x.js"],
            "** 跨目录（含零层）"
        );
        assert_eq!(explicit.unwrap(), vec!["x.js"], "前导 ./ 归一化");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 临时任务简报的验证测试。wait 条件用 Exit（chunk 是它的先导），
    /// 返回的全部事件按序 resolve（先 Chunk 后 Exit），保证通道不残留。
    /// 注意：编译报错调整——简报原文 `events[0]` 按值取会 move，改为 `&events[0]`；
    /// `assert!(true, ...)` 触发 clippy::assertions_on_constants，删除。
    #[test]
    fn node_events_and_focusables() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
            helix.open_popup({
                render: (focus) => helix.el("col", [
                    helix.el("button", "run", { id: "btn1", onPress: () => helix.echo("pressed"), style: focus === "btn1" ? "error" : null }),
                    helix.el("input", { id: "in1", value: "abc", onKey: (k) => helix.echo("key:" + k) }),
                ]),
            });
            "#,
        )
        .unwrap();
        let id = match &take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => *id,
            other => panic!("expected OpenPopup, got {other:?}"),
        };
        // render 传 focus → JS render(focus, ctx) 收到（focus 样式分支由 JS 处理，这里验证渲染不崩 + focusables 收集）
        let content = render_popup(id, 60, 20, Some("btn1")).unwrap();
        let mut focusables = Vec::new();
        if let Content::Tree(node) = &content {
            focusable_node_ids(node, &mut focusables);
        }
        assert_eq!(focusables, vec!["btn1".to_string(), "in1".to_string()]);
        // 事件分发：button onPress
        assert!(dispatch_node_event(id, "btn1", None).is_ok());
        // input onKey
        assert!(dispatch_node_event(id, "in1", Some("a")).is_ok());
        let msgs = take_messages();
        assert!(msgs.contains(&"pressed".to_string()), "{msgs:?}");
        assert!(msgs.contains(&"key:a".to_string()), "{msgs:?}");
        // 未知节点 → Ok（无处理器）
        assert!(dispatch_node_event(id, "nope", None).is_ok());
        close_popup(id).unwrap();
    }

    #[test]
    fn terminal_modes_api() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
            helix.register_command("tm", () => {
                const id = helix.open_terminal({ cmd: "cat", side: "right", size: 40 });
                helix.set_terminal_mode(id, "fullscreen");
                helix.set_terminal_mode(id, "minimized");
                helix.term_clear(id);
                helix.resize_term(id, 60);
            });
            "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("tm", &ctx).unwrap());
        let reqs = take_ui_requests();
        assert!(matches!(&reqs[0], UiRequest::OpenTerminal { .. }));
        assert!(matches!(&reqs[1], UiRequest::TermMode { mode, .. } if mode == "fullscreen"));
        assert!(matches!(&reqs[2], UiRequest::TermMode { mode, .. } if mode == "minimized"));
        assert!(matches!(&reqs[3], UiRequest::TermClear { .. }));
        assert!(matches!(&reqs[4], UiRequest::TermResize { size, .. } if *size == 60));
        // 非法 mode → 命令报错
        load_script(r#"helix.register_command("tm-bad2", () => { helix.set_terminal_mode(1, "sideways"); });"#).unwrap();
        assert!(run_command("tm-bad2", &ctx).is_err());
        // 类型校验（load 时即报错）
        assert!(load_script(r#"helix.term_clear("x");"#).is_err());
        assert!(load_script(r#"helix.resize_term(1, "wide");"#).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn pty_spawn() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("pty-tty", () => {
            const id = helix.spawn({ pty: true, cmd: "tty", onChunk: (c) => helix.echo("out:" + c.trim()), onExit: (code) => helix.echo("exit:" + code) });
        });
        helix.register_command("pty-size", () => {
            const id = helix.spawn({ pty: true, cmd: "stty size", onChunk: (c) => helix.echo("size:" + c.trim()), onExit: (code) => helix.echo("sizeexit:" + code) });
        });
        helix.register_command("pty-cat", () => {
            const id = helix.spawn({ pty: true, cmd: "cat", onChunk: (c) => { helix.echo("pty:" + c.trim()); helix.term_kill(id); }, onExit: (code) => helix.echo("ptyexit:" + code) });
            helix.term_write(id, "hello-pty\n"); // raw mode 下 \u{4} 不是 EOF——echo 到达后 kill
        });
        helix.register_command("pty-badresize", () => { helix.term_resize(999, 1, 1); });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };

        // tty：stdin 是 pty → 输出 /dev/pts/N（CRLF 行尾，用 contains 断言）
        assert!(run_command("pty-tty", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, c) => {
                    resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap()
                }
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap()
                }
            }
        }
        assert!(
            take_messages().iter().any(|m| m.contains("/dev/pts/")),
            "tty command sees a pty"
        );

        // stty size：默认 winsize 24×80（输出顺序 rows cols）
        assert!(run_command("pty-size", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, c) => {
                    resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap()
                }
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap()
                }
            }
        }
        assert!(
            take_messages().iter().any(|m| m.contains("24 80")),
            "default winsize 24x80"
        );

        // 写 master → 子进程 stdin：cat 回显；raw mode 下 \u{4} 非 EOF——onChunk 里 kill。
        // 轮询 drain + resolve（让 onChunk 的 kill 生效）直到 Exit。
        assert!(run_command("pty-cat", &ctx).unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut got_exit = false;
        while !got_exit && std::time::Instant::now() < deadline {
            for e in drain_term_events() {
                let is_exit = matches!(e, TermEvent::Exit(_, _, _));
                let id = match &e {
                    TermEvent::Chunk(id, _) | TermEvent::Exit(id, _, _) => *id,
                };
                let _ = resolve_term_event(id, e);
                if is_exit {
                    got_exit = true;
                }
            }
            if !got_exit {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        assert!(got_exit, "cat should exit after kill");
        let msgs = take_messages();
        assert!(
            msgs.iter().any(|m| m.contains("hello-pty")),
            "cat echo missing: {msgs:?}"
        );
        assert!(
            msgs.iter().any(|m| m.contains("ptyexit:-1")),
            "kill exits cat: {msgs:?}"
        );

        // 校验：pty 非布尔 / resize 未知 id → 报错
        assert!(
            load_script(r#"helix.spawn({ pty: "yes", cmd: "tty", onChunk: () => {} });"#).is_err()
        );
        assert!(run_command("pty-badresize", &ctx).is_err());
    }

    /// spawn 后立即 term_resize → 子进程 stty size 读到新值。
    /// 实现用 master fd 直连 ioctl（spawn 返回时已注册），无消息时序问题。
    #[test]
    #[cfg(unix)]
    fn pty_resize() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("pty-resize", () => {
            const id = helix.spawn({ pty: true, cmd: "stty size", onChunk: (c) => helix.echo("size:" + c.trim()), onExit: (code) => helix.echo("resizeexit:" + code) });
            helix.term_resize(id, 40, 100);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("pty-resize", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, c) => {
                    resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap()
                }
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap()
                }
            }
        }
        let msgs = take_messages();
        // stty size 输出顺序是 rows cols（简报写 "100 40"，实为行列反了）：
        // term_resize(40, 100) → "40 100"，以实际输出为准
        assert!(
            msgs.iter().any(|m| m.contains("40 100")),
            "resize applied before stty runs: {msgs:?}"
        );
    }

    #[test]
    fn plugin_reload_top_level_const() {
        // 顶层 const/let 脚本 reload 必须成功：IIFE 包裹下重跑获得全新词法作用域
        // （无 IIFE 时全局词法环境重复声明会抛 SyntaxError: duplicate lexical declaration）
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script_named(
            "c.js",
            r#"const X = 1; helix.register_command("ccmd", () => helix.echo("ok"));"#,
        )
        .unwrap();

        reload_all().unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("ccmd", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["ok"]);
    }

    /// 方案 2 依赖清单：helix.plugin deps 自动拓扑加载（先依赖后自身）、
    /// 已加载去重、循环依赖报错。不碰 set_plugins_dir（OnceLock 全局，污染后续测试），
    /// 全用绝对路径。
    #[test]
    fn plugin_deps_auto_load() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = tempfile::tempdir().unwrap();
        // ponytail: 泄漏 tempdir（同文件其他测试同款）——线程池复用线程，
        // 目录被删会误伤后续 load；泄漏几个 /tmp 小文件换确定性。
        let d = dir.path().to_path_buf();
        std::mem::forget(dir);
        std::fs::create_dir_all(d.join("features")).unwrap();
        std::fs::create_dir_all(d.join("lib")).unwrap();
        std::fs::write(
            d.join("lib/icons.js"),
            r#"helix.plugin("icons", { deps: [] }); helix.export({ src: "icons" });"#,
        )
        .unwrap();
        std::fs::write(
            d.join("features/feat.js"),
            format!(
                r#"helix.plugin("feat", {{ deps: [{icons:?}] }}); helix.export({{ src: "feat" }});"#,
                icons = d.join("lib/icons.js").to_string_lossy()
            ),
        )
        .unwrap();
        let icons_abs = d.join("lib/icons.js").to_string_lossy().into_owned();
        let feat_abs = d.join("features/feat.js").to_string_lossy().into_owned();
        let script = format!(
            r#"
        helix.register_command("deps-run", () => {{
            const feat = helix.load({feat:?});
            const icons = helix.load({icons:?});
            helix.echo("feat:" + feat.src + " icons:" + icons.src + " same:" + (feat === helix.load({feat:?})));
        }});
        "#,
            feat = feat_abs,
            icons = icons_abs,
        );
        load_script_named("driver.js", &script).unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("deps-run", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["feat:feat icons:icons same:true"]);

        // 循环依赖：a deps b，b deps a → 报错
        let a_abs = d.join("a.js").to_string_lossy().into_owned();
        let b_abs = d.join("b.js").to_string_lossy().into_owned();
        let a_script = format!(r#"helix.plugin("a", {{ deps: [{b:?}] }});"#, b = b_abs);
        let b_script = format!(r#"helix.plugin("b", {{ deps: [{a:?}] }});"#, a = a_abs);
        std::fs::write(d.join("a.js"), a_script).unwrap();
        std::fs::write(d.join("b.js"), b_script).unwrap();
        let cyc_script = format!(
            r#"
        helix.register_command("cyc-run", () => {{
            try {{
                helix.load({a:?});
                helix.echo("no-cycle");
            }} catch (e) {{
                helix.echo("cycle:" + String(e));
            }}
        }});
        "#,
            a = a_abs,
        );
        load_script_named("cyc.js", &cyc_script).unwrap();
        assert!(run_command("cyc-run", &ctx).unwrap());
        assert!(
            take_messages()[0].contains("circular dependency"),
            "循环依赖应报错"
        );
    }

    #[test]
    fn plugin_api_arg_validation() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // install/remove 缺 arg → 报错
        assert!(load_script(r#"helix.plugin.install();"#).is_err());
        assert!(load_script(r#"helix.plugin.remove();"#).is_err());
        // 合法调用 → push PluginOp 请求
        crate::state::take_ui_requests(); // 清空
        load_script(r#"helix.plugin.remove("nope");"#).unwrap();
        load_script(r#"helix.plugin.update();"#).unwrap(); // arg 可选
        let reqs = crate::state::take_ui_requests();
        assert!(reqs.iter().any(|r| matches!(
            r,
            crate::types::UiRequest::PluginOp { op, arg }
                if op == "remove" && arg.as_deref() == Some("nope")
        )));
        assert!(reqs
            .iter()
            .any(|r| matches!(r, crate::types::UiRequest::PluginOp { op, arg } if op == "update" && arg.is_none())));
    }

    #[test]
    fn server_api_pushes_server_ops() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // install/remove 缺 arg → 报错
        assert!(load_script(r#"helix.server.install();"#).is_err());
        assert!(load_script(r#"helix.server.remove();"#).is_err());
        crate::state::take_ui_requests(); // 清空
        load_script(r#"helix.server.list();"#).unwrap();
        load_script(r#"helix.server.install("rust-analyzer");"#).unwrap();
        load_script(r#"helix.server.update();"#).unwrap(); // 无参 = 全量
        load_script(r#"helix.server.remove("black");"#).unwrap();
        load_script(r#"helix.server.status();"#).unwrap();
        let reqs = crate::state::take_ui_requests();
        let ops: Vec<(&str, Option<&str>)> = reqs
            .iter()
            .filter_map(|r| match r {
                crate::types::UiRequest::ServerOp { op, arg } => {
                    Some((op.as_str(), arg.as_deref()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            ops,
            vec![
                ("list", None),
                ("install", Some("rust-analyzer")),
                ("update", None),
                ("remove", Some("black")),
                ("status", None),
            ]
        );
    }

    /// 统一入口：load/export 往返 + 缓存、lazy 桩、run_command 带 ctx、未知文件报错。
    /// set_plugins_dir 是进程全局——测试用临时目录隔离。
    // ponytail: tempdir 不 drop（std::mem::forget）——线程池复用线程，后续 reload 测试
    // 会在本线程重跑 LOADED_SCRIPTS（含 init.js→load("exp.js")），目录被删会误伤；
    // 泄漏几个 /tmp 小文件换确定性。
    #[test]
    fn entry_load_export_lazy() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = tempfile::tempdir().unwrap();
        set_plugins_dir(dir.path().to_path_buf());

        // 导出 + 加载往返
        std::fs::write(
            dir.path().join("exp.js"),
            r#"helix.export({ a: 1, b: "x" });"#,
        )
        .unwrap();
        load_script_named("init.js", r#"helix.load("exp.js");"#).unwrap();
        // init.js 的 load 本身无法断言返回值——直接测 js_load 路径：
        // 用 helix.run_command 间接：注册命令调用 load 并把结果 echo 出来
        load_script_named(
            "driver.js",
            r#"
        helix.register_command("load-exp", () => {
            const mod = helix.load("exp.js");
            helix.echo("a:" + mod.a + " b:" + mod.b);
        });
        helix.register_command("load-cached", () => {
            const m1 = helix.load("exp.js");
            const m2 = helix.load("exp.js");
            helix.echo("same:" + (m1 === m2));
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("load-exp", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["a:1 b:x"]);
        assert!(run_command("load-cached", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["same:true"]);

        // lazy：桩首次调用时加载 + 转执行
        std::fs::write(
            dir.path().join("lazy.js"),
            r#"helix.register_command("lazy-cmd", () => { helix.echo("lazy-ran"); });"#,
        )
        .unwrap();
        load_script_named("lazy-driver.js", r#"helix.lazy("lazy.js", "lazy-cmd");"#).unwrap();
        assert!(run_command("lazy-cmd", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["lazy-ran"]);

        // run_command 带 ctx：命令读 ctx.cursor
        load_script_named(
            "rc.js",
            r#"
        helix.register_command("where", (c) => { helix.echo("at:" + c.cursor.row + "," + c.cursor.col); });
        "#,
        )
        .unwrap();
        load_script_named(
            "rc-driver.js",
            r#"helix.register_command("call-where", () => { helix.run_command("where", { cursor: { row: 3, col: 7 } }); });"#,
        )
        .unwrap();
        assert!(run_command("call-where", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["at:3,7"]);

        // 校验：未知文件 → 抛错
        load_script_named(
            "bad-driver.js",
            r#"helix.register_command("bad-load", () => { helix.load("nope.js"); });"#,
        )
        .unwrap();
        assert!(run_command("bad-load", &ctx).is_err());

        std::mem::forget(dir);
    }

    /// 简报验证测试：el 构造节点、render 返回树 → Content::Tree、非法节点类型 → Err
    #[test]
    fn component_nodes() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.open_popup({
            render: () => helix.el("col", [
                helix.el("text", "title", { style: "error" }),
                helix.el("row", [
                    helix.el("text", "left"),
                    helix.el("text", "right", { width: 5 }),
                ], { gap: 1 }),
                helix.el("scroll", [helix.el("text", "s1"), helix.el("text", "s2")], { height: 1 }),
            ]),
        });
        helix.register_command("tree", (ctx) => {
            const n = helix.el("text", "hello", { width: 10 });
            helix.echo("type:" + n.type + " text:" + n.text + " w:" + n.width);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("tree", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["type:text text:hello w:10"]);

        let reqs = take_ui_requests();
        let id = match &reqs[0] {
            UiRequest::OpenPopup { id, .. } => *id,
            _ => unreachable!("expected OpenPopup"),
        };
        match render_popup(id, 40, 10, None).unwrap() {
            Content::Tree(root) => {
                assert!(matches!(&root, CompNode::Col { .. }));
                match &root {
                    CompNode::Col { children, .. } => {
                        assert_eq!(children.len(), 3);
                        assert!(
                            matches!(&children[0], CompNode::Text { spans, .. } if spans.len() == 1 && spans[0].text == "title" && spans[0].style.as_deref() == Some("error"))
                        );
                        assert!(matches!(&children[1], CompNode::Row { .. }));
                        assert!(matches!(&children[2], CompNode::Scroll { .. }));
                    }
                    _ => panic!(),
                }
            }
            _ => panic!("expected tree"),
        }
        close_popup(id).unwrap();
        // 非法节点类型 → Err
        load_script(r#"helix.open_popup({ render: () => ({ type: "bogus" }) });"#).unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        assert!(render_popup(id, 40, 10, None).is_err());
        close_popup(id).unwrap();
    }

    /// 布局增强任务 1 验证：el/parse_node 透传 flex/wrap/offset 新 opts（含缺省与类型错误）
    #[test]
    fn el_new_opts_parse() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.open_popup({
            render: () => helix.el("col", [
                helix.el("scroll", [helix.el("text", "x")], { height: 4, offset: 2 }),
                helix.el("text", "y", { flex: 2, wrap: true }),
                helix.el("text", "z"),
                helix.el("row", [helix.el("text", "a", { flex: 1 })], { gap: 1, flex: 3 }),
                helix.el("button", "run", { id: "b1", flex: 1 }),
                helix.el("input", { id: "i1", value: "v", flex: 2 }),
            ]),
        });
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        let id = match &reqs[0] {
            UiRequest::OpenPopup { id, .. } => *id,
            _ => unreachable!("expected OpenPopup"),
        };
        match render_popup(id, 40, 10, None).unwrap() {
            Content::Tree(CompNode::Col { children, .. }) => {
                assert_eq!(children.len(), 6);
                assert!(matches!(
                    &children[0],
                    CompNode::Scroll {
                        height: 4,
                        offset: Some(2),
                        ..
                    }
                ));
                assert!(matches!(
                    &children[1],
                    CompNode::Text {
                        flex: Some(2),
                        wrap: true,
                        ..
                    }
                ));
                assert!(matches!(
                    &children[2],
                    CompNode::Text {
                        flex: None,
                        wrap: false,
                        ..
                    }
                ));
                assert!(
                    matches!(&children[3], CompNode::Row { gap: 1, flex: Some(3), children, .. }
                    if matches!(&children[0], CompNode::Text { flex: Some(1), .. }))
                );
                assert!(matches!(
                    &children[4],
                    CompNode::Button { flex: Some(1), .. }
                ));
                assert!(matches!(
                    &children[5],
                    CompNode::Input { flex: Some(2), .. }
                ));
            }
            _ => panic!("expected tree"),
        }
        close_popup(id).unwrap();
        // 类型错误：flex 传字符串 → Err（el 构造时即抛）
        load_script(
            r#"helix.open_popup({ render: () => helix.el("text", "x", { flex: "big" }) });"#,
        )
        .unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        assert!(render_popup(id, 40, 10, None).is_err());
        close_popup(id).unwrap();
        // 类型错误：wrap 传数字 → Err
        load_script(r#"helix.open_popup({ render: () => helix.el("text", "x", { wrap: 1 }) });"#)
            .unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        assert!(render_popup(id, 40, 10, None).is_err());
        close_popup(id).unwrap();
    }

    /// 终端钩子:emit_hook 返回第一个非 undefined;无 handler → None
    #[test]
    fn term_hook_emit_and_term_key() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 无 handler:None
        assert_eq!(emit_hook("term-key", &[JsValue::from(1_i32)]), None);
        assert_eq!(emit_term_key(1, "esc", false, false, false), None);
        assert!(!emit_term_close(1, "esc"), "无 handler 不阻止");

        load_script(
            r#"
            helix.on("term-close", () => false);
            helix.on("term-key", (id, key) => {
                if (key.code === "esc") return "minimize";
                if (key.code === "x" && key.ctrl) return "close";
                return "pass";
            });
            "#,
        )
        .unwrap();
        use crate::commands::TermKeyDecision;
        // term-key 映射
        assert!(matches!(
            emit_term_key(1, "esc", false, false, false),
            Some(TermKeyDecision::Minimize)
        ));
        assert!(matches!(
            emit_term_key(1, "x", false, true, false),
            Some(TermKeyDecision::Close)
        ));
        assert!(matches!(
            emit_term_key(1, "a", false, false, false),
            Some(TermKeyDecision::Pass)
        ));
        // term-close:false → 阻止
        assert!(emit_term_close(7, "esc"), "handler 返回 false → 阻止关闭");
        // 通知型钩子(参数不经 boa 也可构造)
        emit_hook(
            "term-resize",
            &[
                JsValue::from(1_i32),
                JsValue::from(24_i32),
                JsValue::from(80_i32),
            ],
        );
    }

    /// term_state 持久化读写(临时 HOME 隔离)
    #[test]
    fn term_state_persist_roundtrip() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", dir.path());
        load_script(
            r#"
            globalThis.__st = helix.term_state(42, "cwd");   // 不存在 → undefined
            helix.term_state(42, "cwd", "/tmp/proj");
            globalThis.__st2 = helix.term_state(42, "cwd");  // 写后读回
            "#,
        )
        .unwrap();
        crate::state::with_engine(|engine| {
            let v1 = engine
                .global_object()
                .get(JsString::from("__st"), engine)
                .unwrap();
            assert!(v1.is_undefined(), "不存在 → undefined");
            let v2 = engine
                .global_object()
                .get(JsString::from("__st2"), engine)
                .unwrap();
            assert_eq!(
                v2.as_string().unwrap().to_std_string_escaped(),
                "/tmp/proj",
                "写后读回"
            );
        });
        if let Some(old) = old_home {
            std::env::set_var("HOME", old);
        }
        drop(dir);
    }
    /// 组件视图回调:set_component_render(id, fn) → render_component 读取
    #[test]
    fn component_render_registration() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 非法参数
        assert!(load_script(r#"helix.set_component_render("x", () => []);"#).is_err());
        assert!(load_script(r#"helix.set_component_render(3, 42);"#).is_err());
        // 注册后 render_component 可读
        load_script(r#"helix.set_component_render(3, () => [{ type: "text", text: "hi-view" }]);"#)
            .unwrap();
        let content = render_component(3, 10, 1, None).unwrap();
        // 数组返回 → Lines;组件树对象 → Tree;两种都支持(与 popup render 同语义)
        let joined = match &content {
            Content::Lines(lines) => lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.text.as_str())
                .collect::<String>(),
            Content::Tree(_) => String::new(),
        };
        assert_eq!(joined, "hi-view", "set_component_render 回调内容可读");
        // 未注册 → Err
        assert!(render_component(999, 10, 1, None).is_err());
    }

    /// keymap 前缀提示:set_keymap_hint 回调 → keymap_hint 调用;null/无注册 → None
    #[test]
    fn keymap_hint_hook() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 未注册 → None
        assert_eq!(keymap_hint("g", &[]), None);
        // 注册:字符串 → (文本, 默认位置);对象 → 指定位置
        load_script(
            r#"
            helix.set_keymap_hint((ctx) => {
                if (ctx.title === "g") return "h 左移\ng 分组";
                if (ctx.title === "x") return { text: "x 提示", position: "bottom-left" };
                return null;
            });
            "#,
        )
        .unwrap();
        let entries = vec![("h".to_string(), "move_char_left".to_string())];
        let (text, pos) = keymap_hint("g", &entries).unwrap();
        assert!(text.contains("h 左移"), "回调返回文本: {text:?}");
        assert_eq!(pos, "bottom-right", "字符串返回默认位置");
        // 对象返回 → 指定位置
        let (text, pos) = keymap_hint("x", &entries).unwrap();
        assert_eq!(text, "x 提示");
        assert_eq!(pos, "bottom-left", "对象指定位置");
        // 其他前缀 → null → None
        assert_eq!(keymap_hint("z", &entries), None);
        // 清除
        load_script(r#"helix.set_keymap_hint(null);"#).unwrap();
        assert_eq!(keymap_hint("g", &entries), None);
    }

    /// buffer 遍历:cache_buffers → buffers/current_buffer 解析;focus_buffer 入队
    #[test]
    fn buffer_traversal_api() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 无缓存 → null/undefined
        load_script(
            r#"globalThis.__b = helix.buffer.list(); globalThis.__c = helix.buffer.current();"#,
        )
        .unwrap();
        crate::state::with_engine(|engine| {
            let v = engine
                .global_object()
                .get(JsString::from("__b"), engine)
                .unwrap();
            assert!(v.is_null(), "无缓存 buffers → null");
        });
        // 缓存写入 → buffers 数组 / current id
        crate::state::cache_buffers(
            r#"{"current":3,"buffers":[{"id":3,"path":"/a.rs","name":"a.rs","dirty":false,"language":"rust"},{"id":7,"path":null,"name":"[scratch]","dirty":true,"language":null}]}"#,
        );
        load_script(
            r#"
            globalThis.__bs = helix.buffer.list();
            globalThis.__cur = helix.buffer.current();
            helix.buffer.focus(7);
            "#,
        )
        .unwrap();
        crate::state::with_engine(|engine| {
            let arr = engine
                .global_object()
                .get(JsString::from("__bs"), engine)
                .unwrap();
            let arr_obj = arr.as_object().unwrap();
            let arr = boa_engine::object::builtins::JsArray::from_object(arr_obj.clone()).unwrap();
            let len: usize = arr.length(engine).unwrap() as usize;
            assert_eq!(len, 2, "buffers 数组长度");
            let cur = engine
                .global_object()
                .get(JsString::from("__cur"), engine)
                .unwrap();
            assert_eq!(cur.as_number().unwrap() as u64, 3, "current_buffer id");
        });
        // focus_buffer 入队 FocusBuffer
        let reqs = take_ui_requests();
        assert!(
            reqs.iter()
                .any(|r| matches!(r, UiRequest::FocusBuffer { id: 7 })),
            "focus_buffer 入队 FocusBuffer"
        );
    }

    /// 组件状态通道:register_component_state(JSON) → get_component_state(对象);未注册 → null
    #[test]
    fn component_state_roundtrip() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 未注册 → null
        load_script(r#"globalThis.__cs = helix.get_component_state(7);"#).unwrap();
        crate::state::with_engine(|engine| {
            let v = engine
                .global_object()
                .get(JsString::from("__cs"), engine)
                .unwrap();
            assert!(v.is_null(), "未注册 → null");
        });
        // 注册 JSON 提供者 → 对象
        crate::state::register_component_state(7, |_| {
            r#"{"mode":"insert","title":"bash"}"#.to_string()
        });
        load_script(r#"globalThis.__cs2 = helix.get_component_state(7);"#).unwrap();
        crate::state::with_engine(|engine| {
            let v = engine
                .global_object()
                .get(JsString::from("__cs2"), engine)
                .unwrap();
            let obj = v.as_object().unwrap();
            let mode = obj.get(JsString::from("mode"), engine).unwrap();
            assert_eq!(mode.as_string().unwrap().to_std_string_escaped(), "insert");
            let title = obj.get(JsString::from("title"), engine).unwrap();
            assert_eq!(title.as_string().unwrap().to_std_string_escaped(), "bash");
        });
        crate::state::unregister_component_state(7);
    }

    /// term-exit 钩子:resolve_term_event(Exit) → emit_term_exit(消息)
    #[test]
    fn term_exit_hook_via_resolve() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(r#"helix.on("term-exit", (id, code) => helix.echo("exit:" + code));"#).unwrap();
        let id = crate::state::next_term_id();
        // 注册回调(否则 resolve 直接 return)
        crate::state::with_terms(|m| {
            m.insert(
                id,
                crate::types::TermCallbacks {
                    on_chunk: JsValue::undefined(),
                    on_exit: None,
                },
            )
        });
        crate::shell::resolve_term_event(id, crate::types::TermEvent::Exit(id, 3, None)).unwrap();
        let msgs = take_messages();
        assert!(
            msgs.iter().any(|m| m == "exit:3"),
            "term-exit 钩子触发: {msgs:?}"
        );
    }

    /// 批量编辑事务:begin_edit 后编辑积压(take_edits 返回空),end_edit 后一次取走。
    /// Rust 侧直接驱动(简报备选方案):编辑队列用 with_edits 注入,begin/end 走 JS API。
    #[test]
    fn batch_edit_transaction() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 触发 builder 注册 begin_edit/end_edit
        load_script(r#"helix.register_command("noop", () => {});"#).unwrap();

        // 1. 未开启事务:编辑入队后可取走
        crate::state::with_edits(|e| {
            e.push(crate::types::Edit {
                doc: None,
                start: (0, 0),
                end: (0, 0),
                insert: "a".into(),
            })
        });
        assert_eq!(take_edits().len(), 1);

        // 2. begin 后编辑入队,take_edits 返回空(积压)
        load_script("helix.begin_edit();").unwrap();
        crate::state::with_edits(|e| {
            e.push(crate::types::Edit {
                doc: None,
                start: (0, 0),
                end: (0, 0),
                insert: "b".into(),
            })
        });
        assert!(take_edits().is_empty(), "txn open should hold edits");

        // 3. end 后 take_edits 取到积压
        load_script("helix.end_edit();").unwrap();
        let edits = take_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].insert, "b");

        // 4. 命令内 begin/end 包住编辑:命令返回后一次取走
        load_script(
            r#"
        helix.register_command("be", (ctx) => {
            helix.begin_edit();
            ctx.doc.insert(0, 0, "c");
            helix.end_edit();
        });
        "#,
        )
        .unwrap();
        let ctx = crate::CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("be", &ctx).unwrap());
        let edits = take_edits();
        assert_eq!(edits.len(), 1, "begin/end 包住的编辑合并取走");
        assert_eq!(edits[0].insert, "c");

        // 5. 嵌套 begin/end:深度计数,内层 end 不释放外层
        load_script("helix.begin_edit(); helix.begin_edit();").unwrap();
        crate::state::with_edits(|e| {
            e.push(crate::types::Edit {
                doc: None,
                start: (0, 0),
                end: (0, 0),
                insert: "d".into(),
            })
        });
        assert!(take_edits().is_empty(), "嵌套:深度 2 仍积压");
        load_script("helix.end_edit();").unwrap(); // 深度 2→1
        assert!(take_edits().is_empty(), "嵌套:外层事务仍开启");
        load_script("helix.end_edit();").unwrap(); // 深度 1→0
        let edits = take_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].insert, "d");

        // 6. 多余 end:深度 0 饱和,不吞后续编辑
        load_script("helix.end_edit(); helix.end_edit(); helix.end_edit();").unwrap();
        crate::state::with_edits(|e| {
            e.push(crate::types::Edit {
                doc: None,
                start: (0, 0),
                end: (0, 0),
                insert: "e".into(),
            })
        });
        let edits = take_edits();
        assert_eq!(edits.len(), 1, "多余 end 后编辑正常取走");
        assert_eq!(edits[0].insert, "e");

        // 7. begin 不 end(未配对):编辑积压;下一命令入口复位深度+清队列,编辑丢弃不吞后续
        load_script(
            r#"
        helix.register_command("leak", (ctx) => {
            helix.begin_edit();
            ctx.doc.insert(0, 0, "f");
        });
        "#,
        )
        .unwrap();
        assert!(run_command("leak", &ctx).unwrap());
        assert!(take_edits().is_empty(), "未配对事务:编辑积压");
        // 下一命令入口应复位 txn 深度并清队列(丢弃积压编辑)
        assert!(run_command("noop", &ctx).unwrap());
        assert_eq!(
            crate::state::with_txn_depth(|d| *d),
            0,
            "命令入口应复位 txn 深度"
        );
        crate::state::with_edits(|e| {
            e.push(crate::types::Edit {
                doc: None,
                start: (0, 0),
                end: (0, 0),
                insert: "g".into(),
            })
        });
        let edits = take_edits();
        assert_eq!(edits.len(), 1, "复位后编辑正常取走");
        assert_eq!(edits[0].insert, "g");
    }

    /// 装饰请求队列:set_virtual_text / set_highlight 入队,take_decorations 一次取走;
    /// set_virtual_text 只传 path(text 省略)→ Clear。路径已 canonicalize(绝对路径断言原样)。
    #[test]
    fn decorations_queue_and_take() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("dec1", () => {
            helix.set_virtual_text("/tmp/a.rs", 0, 0, "hi", "ui.help");
            helix.set_highlight("/tmp/a.rs", 0, 0, 1, 2, "ui.selection");
            helix.set_virtual_text("/tmp/nope.rs");
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
            docs: vec![],
        };
        run_command("dec1", &ctx).unwrap();
        let reqs = take_decorations();
        assert_eq!(reqs.len(), 3);
        // 路径已 canonicalize(相对→绝对:用绝对路径输入,断言原样)
        assert_eq!(reqs[0].doc.as_deref(), Some("/tmp/a.rs"));
        match &reqs[0].kind {
            crate::types::DecorationKind::VirtualText {
                row,
                col,
                text,
                style,
            } => {
                assert_eq!((*row, *col), (0, 0));
                assert_eq!(text, "hi");
                assert_eq!(style.as_deref(), Some("ui.help"));
            }
            other => panic!("expected VirtualText, got {other:?}"),
        }
        match &reqs[1].kind {
            crate::types::DecorationKind::Highlight {
                sr,
                sc,
                er,
                ec,
                style,
            } => {
                assert_eq!((*sr, *sc, *er, *ec), (0, 0, 1, 2));
                assert_eq!(style.as_deref(), Some("ui.selection"));
            }
            other => panic!("expected Highlight, got {other:?}"),
        }
        assert!(matches!(reqs[2].kind, crate::types::DecorationKind::Clear));
    }

    /// 事务门控与复位:begin/end 包住装饰 → end 后取到;命令入口清残留(不跨命令);
    /// path 非字符串 → 命令失败(类型校验)。
    #[test]
    fn decorations_txn_holds_and_reset() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("dec-txn", () => {
            helix.begin_edit();
            helix.set_virtual_text("/tmp/t.rs", 0, 0, "x");
            helix.end_edit();
        });
        helix.register_command("dec-bad", () => { helix.set_virtual_text(42); });
        helix.register_command("dec-bad2", () => { helix.set_virtual_text("/tmp/t.rs", 0, 0); });
    "#,
        )
        .unwrap();
        let ctx = CommandContext {
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
            docs: vec![],
        };
        // 命令内 begin/end 包住:命令返回后取到(证明 end 后放行)
        run_command("dec-txn", &ctx).unwrap();
        assert_eq!(take_decorations().len(), 1);
        // 类型校验:path 非字符串 → 命令失败
        assert!(run_command("dec-bad", &ctx).is_err());
        // 给了坐标却缺 text → 报错而非静默 Clear(避免误清空;清除 = 只传 path)
        assert!(run_command("dec-bad2", &ctx).is_err());
        // 命令开始复位:上一命令残留不跨命令(dec-bad 报错后队列也应被清)
        assert!(take_decorations().is_empty());
    }

    /// 脚本(重)加载不继承旧装饰:load_script_named 求值前 push 全局 Clear(doc: None)。
    #[test]
    fn decorations_cleared_on_script_load() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        assert!(take_decorations().is_empty());
        load_script("helix.register_command('x', () => {});").unwrap();
        let reqs = take_decorations();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].doc, None);
        assert!(matches!(reqs[0].kind, crate::types::DecorationKind::Clear));
    }

    #[test]
    fn lsp_enhance_request_shapes() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 清残留队列(其它测试不入队,防御)
        let _ = take_lsp_requests();
        // rename newName 非字符串 / execute 非对象 → 命令失败且不入队
        // (同步命令:参数校验错误同步抛出;async 命令会包进 promise 拒绝)
        load_script(
            r#"
        helix.register_command("ren-bad", () => { helix.lsp.rename(42); });
        helix.register_command("exec-bad", () => { helix.lsp.execute_code_action("x"); });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
            docs: vec![],
        };
        assert!(run_command("ren-bad", &ctx).is_err());
        assert!(run_command("exec-bad", &ctx).is_err());
        assert!(take_lsp_requests().is_empty(), "校验失败不应入队");
        // 合法调用 → 请求形态正确(同步命令:四个调用都同步入队)
        load_script(
            r#"
        helix.register_command("ren-ok", () => {
            helix.lsp.rename("newName");
            helix.lsp.format();
            helix.lsp.code_actions({ row: 1, col: 2 });
            helix.lsp.execute_code_action({ title: "fix", kind: "quickfix" });
        });
        "#,
        )
        .unwrap();
        run_command("ren-ok", &ctx).unwrap();
        let reqs = take_lsp_requests();
        assert_eq!(reqs.len(), 4);
        assert!(matches!(reqs[0].method, crate::lsp::LspMethod::Rename));
        assert_eq!(
            reqs[0].params.as_ref().and_then(|v| v.as_str()),
            Some("newName")
        );
        assert!(matches!(reqs[1].method, crate::lsp::LspMethod::Format));
        assert!(matches!(reqs[2].method, crate::lsp::LspMethod::CodeActions));
        assert_eq!(reqs[2].pos, Some((1, 2)));
        assert!(matches!(
            reqs[3].method,
            crate::lsp::LspMethod::ExecuteCodeAction
        ));
        let action = reqs[3].params.as_ref().unwrap();
        assert_eq!(action["title"], serde_json::Value::String("fix".into()));
    }
}
