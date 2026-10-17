//! 根对象上的缓冲区与编辑类 API(by_path、光标/选区、虚拟文本、split…)。
//!
//! 从 `lib.rs` 的 `build_engine` 拆出(G6):`build_engine` 只剩"建命名空间 → 交给
//! 根对象 → 注册全局"三件事;成员注册表按命名空间/能力放在本目录。
//!
//! 裸路径(`commands::js_echo`)可用,是因为 `mod.rs` 把兄弟模块导入了本模块。

use super::*;

use boa_engine::{object::ObjectInitializer, JsString, NativeFunction};

pub(crate) fn register(builder: &mut ObjectInitializer<'_>) {
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_begin_edit),
        JsString::from("begin_edit"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_end_edit),
        JsString::from("end_edit"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_by_path),
        JsString::from("by_path"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_set_buffer_icon),
        JsString::from("set_buffer_icon"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_set_completion_icon),
        JsString::from("set_completion_icon"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_set_cursor),
        JsString::from("set_cursor"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_set_selection),
        JsString::from("set_selection"),
        4,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_set_virtual_text),
        JsString::from("set_virtual_text"),
        5,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_set_highlight),
        JsString::from("set_highlight"),
        7,
    );
    builder.function(
        NativeFunction::from_fn_ptr(layout_api::js_split),
        JsString::from("split"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(layout_api::js_buffer_open),
        JsString::from("buffer_open"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(layout_api::js_get_component_state),
        JsString::from("get_component_state"),
        1,
    );
}
