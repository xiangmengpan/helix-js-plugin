//! **`helix.*` 的唯一声明处**（签名/arity/能力/实现函数指针）。
//!
//! ## 为什么要有这张表
//!
//! 改这个仓库之前，同一个成员的事实散在四处：Rust 注册表（`.function(...)` 样板）、
//! `plugins/helix.d.ts`、`docs/api/*.md`、以及 `plugins/tools/check_*_surface.mjs` 里
//! **再解析一遍 Rust 源码**的逻辑。四份真相 + 三个正则对账脚本 ⇒ 漂移只能靠脚本兜，
//! 而脚本自己承认"只对名字，不查签名"。
//!
//! 现在：**注册、`describe()`、能力门控**都从这张表来；d.ts 与文档仍手写（签名里的类型
//! 是有价值的手工资产），但由 `describe()` 机器对账（名字 **+ arity + 能力**）。
//!
//! ## 改一个成员要动哪里
//!
//! 1. 这张表加/改一行（含 `caps`）；
//! 2. `plugins/helix.d.ts` 补类型（hand-written）；
//! 3. `docs/api/xxx.md` 补条目；
//! 4. `cargo xtask api-check`。
//!
//! ## 能力（caps）为什么在这里
//!
//! 以前每个受保护 API 的实现里各自 `check_js(...)`，忘了就是静默放权（那个
//! `check_capability_surface.mjs` 存在的唯一理由就是"人会忘"）。现在 `caps` 是表的字段，
//! **派发点**统一检查 ⇒ 新增 API 不可能漏（漏了就是没写 caps，而写 caps 是声明的一部分）。

use boa_engine::{
    native_function as native_fn,
    object::{builtins::JsFunction, ObjectInitializer},
    Context, JsObject, JsString, JsValue, NativeFunction,
};

/// 成员挂在哪里
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    /// `helix.<ns>.<name>`
    Ns(&'static str),
    /// `helix.<name>`（根上的函数/常量）
    Root,
    /// 引擎全局（不在 `helix` 上），如 `TABBAR_ID`
    Global,
}

/// 成员的种类
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemberKind {
    /// 普通原生函数
    Fn,
    /// 函数对象（`FunctionObjectBuilder`，目前只有 `helix.plugin`）
    FunctionObject,
    /// 常量（`api_version` / `TABBAR_ID`）
    Const,
    /// 由数据源**动态**注册（`helix.icons.*`）；表里只登记，供对账
    Dynamic,
}

pub struct Entry {
    pub target: Target,
    pub name: &'static str,
    pub arity: u32,
    /// `MemberKind::Fn` 时有效；其余种类为占位（`noop`）
    pub f: native_fn::NativeFunctionPointer,
    pub kind: MemberKind,
    /// 实现函数的**标识符**（对账脚本按它在源码里找函数体；`Dynamic`/`Const` 为空）
    pub fname: &'static str,
    /// 需要的能力（空 = 不需要）。**派发点**按它检查。
    pub caps: &'static [&'static str],
}

fn noop(_: &JsValue, _: &[JsValue], _: &mut Context) -> boa_engine::JsResult<JsValue> {
    Ok(JsValue::undefined())
}

const fn e(
    target: Target,
    name: &'static str,
    arity: u32,
    f: native_fn::NativeFunctionPointer,
    fname: &'static str,
    caps: &'static [&'static str],
) -> Entry {
    Entry { target, name, arity, f, kind: MemberKind::Fn, fname, caps }
}

const fn e_kind(
    target: Target,
    name: &'static str,
    arity: u32,
    kind: MemberKind,
    caps: &'static [&'static str],
) -> Entry {
    Entry { target, name, arity, f: noop, kind, fname: "", caps }
}

/// 全部成员（顺序即属性插入顺序；`register_*` 依赖它）
pub static API: &[Entry] = &[
    e(Target::Ns("lsp"), "hover", 1, crate::lsp::js_lsp_hover, "js_lsp_hover", &[]),
    e(Target::Ns("lsp"), "completion", 1, crate::lsp::js_lsp_completion, "js_lsp_completion", &[]),
    e(Target::Ns("lsp"), "goto_definition", 1, crate::lsp::js_lsp_goto_definition, "js_lsp_goto_definition", &[]),
    e(Target::Ns("lsp"), "document_symbols", 1, crate::lsp::js_lsp_document_symbols, "js_lsp_document_symbols", &[]),
    e(Target::Ns("lsp"), "format", 0, crate::lsp::js_lsp_format, "js_lsp_format", &[]),
    e(Target::Ns("lsp"), "rename", 1, crate::lsp::js_lsp_rename, "js_lsp_rename", &[]),
    e(Target::Ns("lsp"), "code_actions", 1, crate::lsp::js_lsp_code_actions, "js_lsp_code_actions", &[]),
    e(Target::Ns("lsp"), "execute_code_action", 1, crate::lsp::js_lsp_execute_code_action, "js_lsp_execute_code_action", &[]),
    e(Target::Ns("picker"), "define", 2, crate::picker::js_picker_define, "js_picker_define", &[]),
    e(Target::Ns("picker"), "run", 1, crate::picker::js_picker_run, "js_picker_run", &[]),
    e(Target::Ns("server"), "list", 1, crate::commands::js_server_list, "js_server_list", &[crate::permissions::CAP_PROCESS]),
    e(Target::Ns("server"), "search", 1, crate::commands::js_server_search, "js_server_search", &[crate::permissions::CAP_PROCESS]),
    e(Target::Ns("server"), "install", 1, crate::commands::js_server_install, "js_server_install", &[crate::permissions::CAP_PROCESS]),
    e(Target::Ns("server"), "update", 1, crate::commands::js_server_update, "js_server_update", &[crate::permissions::CAP_PROCESS]),
    e(Target::Ns("server"), "remove", 1, crate::commands::js_server_remove, "js_server_remove", &[crate::permissions::CAP_PROCESS]),
    e(Target::Ns("server"), "status", 1, crate::commands::js_server_status, "js_server_status", &[crate::permissions::CAP_PROCESS]),
    e(Target::Ns("server"), "rows", 1, crate::server_rows::js_server_rows, "js_server_rows", &[crate::permissions::CAP_PROCESS]),
    e(Target::Ns("server"), "task", 2, crate::server_tasks::js_server_task, "js_server_task", &[crate::permissions::CAP_PROCESS]),
    e(Target::Ns("pane"), "list", 0, crate::layout_api::js_pane_list, "js_pane_list", &[]),
    e(Target::Ns("pane"), "info", 1, crate::layout_api::js_pane_info, "js_pane_info", &[]),
    e(Target::Ns("pane"), "raise", 1, crate::layout_api::js_pane_raise, "js_pane_raise", &[]),
    e(Target::Ns("pane"), "pin", 2, crate::layout_api::js_pane_pin, "js_pane_pin", &[]),
    e(Target::Ns("pane"), "float", 1, crate::layout_api::js_pane_float, "js_pane_float", &[]),
    e(Target::Ns("pane"), "embed", 1, crate::layout_api::js_pane_embed, "js_pane_embed", &[]),
    e(Target::Ns("pane"), "close", 1, crate::layout_api::js_close_leaf, "js_close_leaf", &[]),
    e(Target::Ns("pane"), "focus", 1, crate::layout_api::js_focus_leaf, "js_focus_leaf", &[]),
    e(Target::Ns("pane"), "focus_dir", 2, crate::layout_api::js_focus_leaf_dir, "js_focus_leaf_dir", &[]),
    e(Target::Ns("pane"), "move", 2, crate::layout_api::js_swap_leaf_dir, "js_swap_leaf_dir", &[]),
    e(Target::Ns("pane"), "resize", 2, crate::layout_api::js_resize_leaf, "js_resize_leaf", &[]),
    e(Target::Ns("pane"), "zoom", 1, crate::layout_api::js_zoom_leaf, "js_zoom_leaf", &[]),
    e(Target::Ns("pane"), "unzoom", 0, crate::layout_api::js_unzoom, "js_unzoom", &[]),
    e(Target::Ns("pane"), "minimize", 2, crate::layout_api::js_minimize_leaf, "js_minimize_leaf", &[]),
    e(Target::Ns("pane"), "equalize", 1, crate::layout_api::js_equalize_leaf, "js_equalize_leaf", &[]),
    e(Target::Ns("pane"), "fix", 2, crate::layout_api::js_layout_fix, "js_layout_fix", &[]),
    e(Target::Ns("layout"), "get", 0, crate::layout_api::js_get_layout, "js_get_layout", &[]),
    e(Target::Ns("layout"), "restore", 1, crate::layout_api::js_restore_layout, "js_restore_layout", &[]),
    e(Target::Ns("layout"), "save", 1, crate::layout_api::js_layout_save, "js_layout_save", &[]),
    e(Target::Ns("layout"), "load", 1, crate::layout_api::js_layout_load, "js_layout_load", &[]),
    e(Target::Ns("layout"), "list", 0, crate::layout_api::js_layout_list, "js_layout_list", &[]),
    e(Target::Ns("layout"), "delete", 1, crate::layout_api::js_layout_delete, "js_layout_delete", &[]),
    e(Target::Ns("pane_mode"), "current", 0, crate::layout_api::js_pane_mode_current, "js_pane_mode_current", &[]),
    e(Target::Ns("pane_mode"), "keymap", 0, crate::layout_api::js_pane_mode_keymap, "js_pane_mode_keymap", &[]),
    e(Target::Ns("fuzzy"), "session", 1, crate::fuzzy::js_fuzzy_session, "js_fuzzy_session", &[]),
    e(Target::Ns("fuzzy"), "run", 3, crate::fuzzy::js_fuzzy_run, "js_fuzzy_run", &[]),
    e(Target::Ns("fuzzy"), "drop", 1, crate::fuzzy::js_fuzzy_drop, "js_fuzzy_drop", &[]),
    e(Target::Ns("buffer"), "list", 0, crate::layout_api::js_buffers, "js_buffers", &[]),
    e(Target::Ns("buffer"), "current", 0, crate::layout_api::js_current_buffer, "js_current_buffer", &[]),
    e(Target::Ns("buffer"), "focus", 1, crate::layout_api::js_focus_buffer, "js_focus_buffer", &[]),
    e(Target::Ns("stack"), "list", 0, crate::layout_api::js_stack_list, "js_stack_list", &[]),
    e(Target::Ns("stack"), "create", 1, crate::layout_api::js_stack_create, "js_stack_create", &[]),
    e(Target::Ns("stack"), "activate", 2, crate::layout_api::js_stack_activate, "js_stack_activate", &[]),
    e(Target::Ns("stack"), "remove", 1, crate::layout_api::js_stack_remove, "js_stack_remove", &[]),
    e(Target::Ns("draw"), "rect", 5, crate::draw::js_draw_rect, "js_draw_rect", &[]),
    e(Target::Ns("draw"), "fill", 5, crate::draw::js_draw_fill, "js_draw_fill", &[]),
    e(Target::Ns("draw"), "hline", 4, crate::draw::js_draw_hline, "js_draw_hline", &[]),
    e(Target::Ns("draw"), "vline", 4, crate::draw::js_draw_vline, "js_draw_vline", &[]),
    e(Target::Ns("draw"), "text", 4, crate::draw::js_draw_text, "js_draw_text", &[]),
    e(Target::Ns("draw"), "braille", 2, crate::draw::js_draw_braille, "js_draw_braille", &[]),
    e(Target::Ns("draw"), "push_clip", 4, crate::draw::js_draw_push_clip, "js_draw_push_clip", &[]),
    e(Target::Ns("draw"), "push_scroll", 5, crate::draw::js_draw_push_scroll, "js_draw_push_scroll", &[]),
    e(Target::Ns("draw"), "pop", 0, crate::draw::js_draw_pop, "js_draw_pop", &[]),
    e(Target::Ns("draw"), "done", 0, crate::draw::js_draw_done, "js_draw_done", &[]),
    e(Target::Ns("text"), "width", 1, crate::draw::js_text_width, "js_text_width", &[]),
    e(Target::Ns("text"), "truncate", 2, crate::draw::js_text_truncate, "js_text_truncate", &[]),
    e(Target::Ns("text"), "pad", 3, crate::draw::js_text_pad, "js_text_pad", &[]),
    e(Target::Root, "echo", 1, crate::commands::js_echo, "js_echo", &[]),
    e(Target::Root, "register_command", 2, crate::commands::js_register_command, "js_register_command", &[]),
    e(Target::Root, "quit", 0, crate::commands::js_quit, "js_quit", &[]),
    e(Target::Root, "reload_plugins", 0, crate::commands::js_reload_plugins, "js_reload_plugins", &[]),
    e(Target::Root, "on", 2, crate::commands::js_on, "js_on", &[]),
    e(Target::Root, "map", 3, crate::commands::js_map, "js_map", &[]),
    e(Target::Root, "load", 1, crate::commands::js_load, "js_load", &[]),
    e(Target::Root, "export", 1, crate::commands::js_export, "js_export", &[]),
    e(Target::Root, "lazy", 2, crate::commands::js_lazy, "js_lazy", &[]),
    e(Target::Root, "run_command", 1, crate::commands::js_run_command, "js_run_command", &[]),
    e(Target::Root, "run", 1, crate::commands::js_run, "js_run", &[crate::permissions::CAP_PROCESS]),
    e(Target::Root, "read_dir", 1, crate::popup::js_read_dir, "js_read_dir", &[crate::permissions::CAP_FS_READ]),
    e(Target::Root, "open_file", 1, crate::popup::js_open_file, "js_open_file", &[]),
    e(Target::Root, "runtime_path", 1, crate::commands::js_runtime_path, "js_runtime_path", &[]),
    e(Target::Root, "command_args", 0, crate::commands::js_command_args, "js_command_args", &[]),
    e(Target::Root, "cwd", 0, crate::commands::js_cwd, "js_cwd", &[]),
    e(Target::Root, "plugins_dir", 0, crate::commands::js_plugins_dir, "js_plugins_dir", &[]),
    e(Target::Root, "read_file", 1, crate::popup::js_read_file, "js_read_file", &[crate::permissions::CAP_FS_READ]),
    e(Target::Root, "write_file", 2, crate::popup::js_write_file, "js_write_file", &[crate::permissions::CAP_FS_WRITE]),
    e(Target::Root, "write_plugin_file", 2, crate::commands::js_write_plugin_file, "js_write_plugin_file", &[crate::permissions::CAP_FS_WRITE]),
    e(Target::Root, "loaded_plugins", 0, crate::commands::js_loaded_plugins, "js_loaded_plugins", &[]),
    e(Target::Root, "remove_plugin_file", 1, crate::commands::js_remove_plugin_file, "js_remove_plugin_file", &[crate::permissions::CAP_FS_WRITE]),
    e(Target::Root, "read_file_async", 1, crate::shell::js_read_file_async, "js_read_file_async", &[crate::permissions::CAP_FS_READ]),
    e(Target::Root, "write_file_async", 2, crate::shell::js_write_file_async, "js_write_file_async", &[crate::permissions::CAP_FS_WRITE]),
    e(Target::Root, "stat_async", 1, crate::shell::js_stat_async, "js_stat_async", &[crate::permissions::CAP_FS_READ]),
    e(Target::Root, "glob_async", 1, crate::shell::js_glob_async, "js_glob_async", &[crate::permissions::CAP_FS_READ]),
    e(Target::Root, "read_tree", 2, crate::shell::js_read_tree, "js_read_tree", &[crate::permissions::CAP_FS_READ]),
    e(Target::Root, "watch", 2, crate::watch::js_watch, "js_watch", &[crate::permissions::CAP_FS_READ]),
    e(Target::Root, "unwatch", 1, crate::watch::js_unwatch, "js_unwatch", &[]),
    e(Target::Root, "begin_edit", 0, crate::commands::js_begin_edit, "js_begin_edit", &[]),
    e(Target::Root, "end_edit", 0, crate::commands::js_end_edit, "js_end_edit", &[]),
    e(Target::Root, "by_path", 1, crate::commands::js_by_path, "js_by_path", &[]),
    e(Target::Root, "set_buffer_icon", 1, crate::popup::js_set_buffer_icon, "js_set_buffer_icon", &[]),
    e(Target::Root, "set_completion_icon", 1, crate::popup::js_set_completion_icon, "js_set_completion_icon", &[]),
    e(Target::Root, "set_cursor", 2, crate::commands::js_set_cursor, "js_set_cursor", &[]),
    e(Target::Root, "set_selection", 4, crate::commands::js_set_selection, "js_set_selection", &[]),
    e(Target::Root, "set_virtual_text", 5, crate::commands::js_set_virtual_text, "js_set_virtual_text", &[]),
    e(Target::Root, "set_highlight", 7, crate::commands::js_set_highlight, "js_set_highlight", &[]),
    e(Target::Root, "split", 2, crate::layout_api::js_split, "js_split", &[]),
    e(Target::Root, "buffer_open", 2, crate::layout_api::js_buffer_open, "js_buffer_open", &[]),
    e(Target::Root, "get_component_state", 1, crate::layout_api::js_get_component_state, "js_get_component_state", &[]),
    e(Target::Root, "term_state", 3, crate::commands::js_term_state, "js_term_state", &[]),
    e(Target::Root, "run_async", 1, crate::shell::js_run_async, "js_run_async", &[crate::permissions::CAP_PROCESS]),
    e(Target::Root, "spawn", 1, crate::shell::js_spawn, "js_spawn", &[crate::permissions::CAP_PROCESS]),
    e(Target::Root, "term_write", 2, crate::shell::js_term_write, "js_term_write", &[]),
    e(Target::Root, "term_kill", 1, crate::shell::js_term_kill, "js_term_kill", &[]),
    e(Target::Root, "open_terminal", 1, crate::popup::js_open_terminal, "js_open_terminal", &[crate::permissions::CAP_PROCESS]),
    e(Target::Root, "term_feed", 2, crate::popup::js_term_feed, "js_term_feed", &[]),
    e(Target::Root, "set_terminal_mode", 2, crate::popup::js_set_terminal_mode, "js_set_terminal_mode", &[]),
    e(Target::Root, "term_clear", 1, crate::popup::js_term_clear, "js_term_clear", &[]),
    e(Target::Root, "term_save", 2, crate::popup::js_term_save, "js_term_save", &[]),
    e(Target::Root, "resize_term", 2, crate::popup::js_resize_term, "js_resize_term", &[]),
    e(Target::Root, "term_list", 0, crate::popup::js_term_list, "js_term_list", &[]),
    e(Target::Root, "term_close", 1, crate::popup::js_term_close, "js_term_close", &[]),
    e(Target::Root, "term_resize", 3, crate::shell::js_term_resize, "js_term_resize", &[]),
    e(Target::Root, "open_popup", 1, crate::popup::js_open_popup, "js_open_popup", &[]),
    e(Target::Root, "set_component_render", 2, crate::popup::js_set_component_render, "js_set_component_render", &[]),
    e(Target::Root, "close_popup", 1, crate::popup::js_close_popup, "js_close_popup", &[]),
    e(Target::Root, "popup_exists", 1, crate::popup::js_popup_exists, "js_popup_exists", &[]),
    e(Target::Root, "open_panel", 1, crate::popup::js_open_panel, "js_open_panel", &[]),
    e(Target::Root, "close_panel", 1, crate::popup::js_close_panel, "js_close_panel", &[]),
    e(Target::Root, "viewport", 0, crate::commands::js_viewport, "js_viewport", &[]),
    e(Target::Root, "picker_state", 0, crate::commands::js_picker_state, "js_picker_state", &[]),
    e(Target::Root, "move_panel", 2, crate::popup::js_move_panel, "js_move_panel", &[]),
    e(Target::Root, "set_completion_render", 1, crate::popup::js_set_completion_render, "js_set_completion_render", &[]),
    e(Target::Root, "el", 2, crate::popup::js_el, "js_el", &[]),
    e(Target::Root, "set_input_value", 3, crate::input::js_set_input_value, "js_set_input_value", &[]),
    e(Target::Root, "set_statusline", 1, crate::popup::js_set_statusline, "js_set_statusline", &[]),
    e(Target::Root, "set_keymap_hint", 1, crate::popup::js_set_keymap_hint, "js_set_keymap_hint", &[]),
    e(Target::Root, "layout_resize", 3, crate::layout_api::js_resize_leaf_dir, "js_resize_leaf_dir", &[]),
    e(Target::Root, "layout_swap", 2, crate::layout_api::js_swap_leaves, "js_swap_leaves", &[]),
    e(Target::Root, "diagnostics", 0, crate::diagnostics::js_diagnostics, "js_diagnostics", &[]),
    e(Target::Root, "permissions", 0, crate::permissions::js_permissions, "js_permissions", &[]),
    e(Target::Root, "define_config", 2, crate::config::js_define_config, "js_define_config", &[]),
    e(Target::Root, "get_config", 1, crate::config::js_get_config, "js_get_config", &[]),
    e(Target::Root, "get_config_docs", 1, crate::config::js_get_config_docs, "js_get_config_docs", &[]),
    e(Target::Root, "set_theme", 1, crate::theme::js_set_theme, "js_set_theme", &[]),
    e(Target::Root, "reset_theme", 0, crate::theme::js_reset_theme, "js_reset_theme", &[]),
    e(Target::Root, "get_style", 1, crate::theme::js_get_style, "js_get_style", &[]),
    e(Target::Root, "theme_info", 0, crate::theme::js_theme_info, "js_theme_info", &[]),
    e(Target::Root, "set_theme_name", 1, crate::theme::js_set_theme_name, "js_set_theme_name", &[]),
    e(Target::Root, "set_diagnostic_icons", 1, crate::icons::js_set_diagnostic_icons, "js_set_diagnostic_icons", &[]),
    // ── icons：**动态注册**（`icons::native_fns()` 是唯一数据源）。这里的行只服务
    //    describe()/dts 对账，不参与注册（注册走 `register::icons`）。
    e_kind(Target::Ns("icons"), "file", 1, MemberKind::Dynamic, &[]),
    e_kind(Target::Ns("icons"), "dir", 1, MemberKind::Dynamic, &[]),
    e_kind(Target::Ns("icons"), "mode", 1, MemberKind::Dynamic, &[]),
    e_kind(Target::Ns("icons"), "diagnostic", 1, MemberKind::Dynamic, &[]),
    e_kind(Target::Ns("icons"), "git", 1, MemberKind::Dynamic, &[]),
    e_kind(Target::Ns("icons"), "completion", 1, MemberKind::Dynamic, &[]),
    e_kind(Target::Ns("icons"), "enabled", 1, MemberKind::Dynamic, &[]),
    // ── 根上的两个非表驱动成员：函数对象 `helix.plugin` 与常量 `helix.api_version`
    //    （挂载顺序见 `ATTACH`；这里只登记，供 describe()/dts 对账）
    e_kind(Target::Root, "plugin", 2, MemberKind::FunctionObject, &[]),
    e_kind(Target::Root, "api_version", 0, MemberKind::Const, &[]),
    // ── 全局量（不是 helix 的属性）
    e_kind(Target::Global, "TABBAR_ID", 0, MemberKind::Const, &[]),
];

/// 命名空间对象的**挂载顺序**（与测试/插件看到的 `Object.keys(helix)` 顺序一致）
pub enum Attach {
    Ns(&'static str),
    /// `helix.plugin`（函数对象）
    Plugin,
    /// `helix.api_version`（常量）
    ApiVersion,
}

pub static ATTACH: &[Attach] = &[
    Attach::Ns("lsp"),
    Attach::Ns("picker"),
    Attach::Plugin,
    Attach::Ns("server"),
    Attach::Ns("pane"),
    Attach::Ns("icons"),
    Attach::Ns("fuzzy"),
    Attach::Ns("stack"),
    Attach::Ns("draw"),
    Attach::Ns("text"),
    Attach::Ns("buffer"),
    Attach::Ns("layout"),
    Attach::Ns("pane_mode"),
    Attach::ApiVersion,
];

/// 按表构建一个命名空间对象（`icons` 例外：数据源在 `icons::native_fns`）
pub(crate) fn register_ns(engine: &mut Context, ns: &'static str) -> JsObject {
    if ns == "icons" {
        return crate::register::icons::register(engine);
    }
    let mut builder = ObjectInitializer::new(engine);
    for (idx, entry) in API.iter().enumerate() {
        if entry.target != Target::Ns(ns) || entry.kind != MemberKind::Fn {
            continue;
        }
        builder.function(native(idx, entry), JsString::from(entry.name), entry.arity as usize);
    }
    builder.build()
}

/// 根对象上的普通函数成员（在命名空间挂载**之前**插入；顺序 = 表序）
pub(crate) fn register_root(builder: &mut ObjectInitializer<'_>) {
    for (idx, entry) in API.iter().enumerate() {
        if entry.target != Target::Root || entry.kind != MemberKind::Fn {
            continue;
        }
        builder.function(native(idx, entry), JsString::from(entry.name), entry.arity as usize);
    }
}

/// 命名空间之后的两个特殊成员
pub(crate) fn build_plugin(engine: &mut Context) -> JsFunction {
    use boa_engine::{object::FunctionObjectBuilder, NativeFunction};
    FunctionObjectBuilder::new(
        engine.realm(),
        NativeFunction::from_fn_ptr(crate::commands::js_plugin),
    )
    .name("plugin")
    .length(2)
    .build()
}

/// 引擎全局量
pub(crate) fn register_globals(engine: &mut Context) {
    engine
        .register_global_property(
            JsString::from("TABBAR_ID"),
            JsValue::from(0x7ABB_0001_u64),
            boa_engine::property::Attribute::READONLY
                | boa_engine::property::Attribute::NON_ENUMERABLE,
        )
        .expect("register TABBAR_ID");
}

/// 把一个表项变成可注册的 `NativeFunction`：**能力在派发点检查**
fn native(idx: usize, entry: &'static Entry) -> NativeFunction {
    if entry.caps.is_empty() {
        return NativeFunction::from_fn_ptr(entry.f);
    }
    NativeFunction::from_copy_closure_with_captures(
        |this: &JsValue, args: &[JsValue], idx: &usize, ctx: &mut Context| {
            let entry = &API[*idx];
            for cap in entry.caps {
                crate::permissions::check(cap).map_err(|err| {
                    crate::js_err(format!("{}: {err}", api_name(entry.target, entry.name)))
                })?;
            }
            (entry.f)(this, args, ctx)
        },
        idx,
    )
}

fn api_name(target: Target, name: &str) -> String {
    match target {
        Target::Ns(ns) => format!("helix.{ns}.{name}"),
        Target::Root => format!("helix.{name}"),
        Target::Global => name.to_string(),
    }
}

/// 机器可读的 API 面：`api-check` 的唯一数据源（顺序 = 表序，diff 稳定）
pub fn describe() -> String {
    let members: Vec<serde_json::Value> = API
        .iter()
        .enumerate()
        .filter(|(_, entry)| !(entry.target == Target::Root && entry.kind == MemberKind::Const))
        .map(|(idx, entry)| {
            let ns = match entry.target {
                Target::Ns(ns) => Some(ns),
                _ => None,
            };
            serde_json::json!({
                "ns": ns,
                "name": entry.name,
                "where": format!("helix-js/src/api.rs → API[{idx}] ({})", api_name(entry.target, entry.name)),
                "arity": entry.arity,
                "fname": entry.fname,
                "kind": match entry.kind {
                    MemberKind::Fn => "fn",
                    MemberKind::FunctionObject => "function_object",
                    MemberKind::Const => "const",
                    MemberKind::Dynamic => "dynamic",
                },
                "global": entry.target == Target::Global,
                "caps": entry.caps,
            })
        })
        .collect();
    let properties: Vec<&'static str> = API
        .iter()
        .filter(|e| e.target == Target::Root && e.kind == MemberKind::Const)
        .map(|e| e.name)
        .collect();
    serde_json::json!({
        "members": members,
        // 根上的**属性**（不是函数）：d.ts 里以 `name: Type;` 形式声明，
        // 与函数成员分开对账（`api_version` 属此列）
        "properties": properties,
        // 引擎全局量（不是 helix 的属性）：`helix` 本身由 build_engine 注册，
        // 其余来自表里 `Target::Global` 的行。
        "globals": ["helix"],
        "events": crate::commands::event_whitelist(),
    })
    .to_string()
}
