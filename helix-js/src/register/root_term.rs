//! 根对象上的终端/进程类 API(term_*、run_async、spawn…)。
//!
//! 从 `lib.rs` 的 `build_engine` 拆出(G6):`build_engine` 只剩"建命名空间 → 交给
//! 根对象 → 注册全局"三件事;成员注册表按命名空间/能力放在本目录。
//!
//! 裸路径(`commands::js_echo`)可用,是因为 `mod.rs` 把兄弟模块导入了本模块。

use super::*;

use boa_engine::{object::ObjectInitializer, JsString, NativeFunction};

pub(crate) fn register(builder: &mut ObjectInitializer<'_>) {
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_term_state),
        JsString::from("term_state"),
        3,
    );
    builder.function(
        NativeFunction::from_fn_ptr(shell::js_run_async),
        JsString::from("run_async"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(shell::js_spawn),
        JsString::from("spawn"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(shell::js_term_write),
        JsString::from("term_write"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(shell::js_term_kill),
        JsString::from("term_kill"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_open_terminal),
        JsString::from("open_terminal"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_term_feed),
        JsString::from("term_feed"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_set_terminal_mode),
        JsString::from("set_terminal_mode"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_term_clear),
        JsString::from("term_clear"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_term_save),
        JsString::from("term_save"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_resize_term),
        JsString::from("resize_term"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_term_list),
        JsString::from("term_list"),
        0,
    );
    builder.function(
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
}
