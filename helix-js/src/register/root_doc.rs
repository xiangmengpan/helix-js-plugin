//! 根对象上的文档/命令/插件加载类 API(echo、on、map、load、run…)。
//!
//! 从 `lib.rs` 的 `build_engine` 拆出(G6):`build_engine` 只剩"建命名空间 → 交给
//! 根对象 → 注册全局"三件事;成员注册表按命名空间/能力放在本目录。
//!
//! 裸路径(`commands::js_echo`)可用,是因为 `mod.rs` 把兄弟模块导入了本模块。

use super::*;

use boa_engine::{object::ObjectInitializer, JsString, NativeFunction};

pub(crate) fn register(builder: &mut ObjectInitializer<'_>) {
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_echo),
        JsString::from("echo"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_register_command),
        JsString::from("register_command"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_quit),
        JsString::from("quit"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_on),
        JsString::from("on"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_map),
        JsString::from("map"),
        3,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_load),
        JsString::from("load"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_export),
        JsString::from("export"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_lazy),
        JsString::from("lazy"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_run_command),
        JsString::from("run_command"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_run),
        JsString::from("run"),
        1,
    );
}
