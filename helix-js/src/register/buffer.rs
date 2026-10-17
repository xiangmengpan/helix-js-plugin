//! `helix.buffer.*` 与 `helix.stack.*` 命名空间。
//!
//! 从 `lib.rs` 的 `build_engine` 拆出(G6):`build_engine` 只剩"建命名空间 → 交给
//! 根对象 → 注册全局"三件事;成员注册表按命名空间/能力放在本目录。
//!
//! 裸路径(`commands::js_echo`)可用,是因为 `mod.rs` 把兄弟模块导入了本模块。

use super::*;

use boa_engine::{object::ObjectInitializer, Context, JsObject, JsString, NativeFunction};

pub(crate) fn register(engine: &mut Context) -> JsObject {
    let mut buffer_obj = ObjectInitializer::new(engine);
    buffer_obj
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_buffers),
            JsString::from("list"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_current_buffer),
            JsString::from("current"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_focus_buffer),
            JsString::from("focus"),
            1,
        )
        .build()
}

/// `helix.stack.*`(堆叠组)。
pub(crate) fn register_stack(engine: &mut Context) -> JsObject {
    let mut stack_obj = ObjectInitializer::new(engine);
    stack_obj
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_stack_list),
            JsString::from("list"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_stack_create),
            JsString::from("create"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_stack_activate),
            JsString::from("activate"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_stack_remove),
            JsString::from("remove"),
            1,
        )
        .build()
}
