//! `helix.layout.*` 命名空间(序列化式布局树读写)。
//!
//! 从 `lib.rs` 的 `build_engine` 拆出(G6):`build_engine` 只剩"建命名空间 → 交给
//! 根对象 → 注册全局"三件事;成员注册表按命名空间/能力放在本目录。
//!
//! 裸路径(`commands::js_echo`)可用,是因为 `mod.rs` 把兄弟模块导入了本模块。

use super::*;

use boa_engine::{object::ObjectInitializer, Context, JsObject, JsString, NativeFunction};

pub(crate) fn register(engine: &mut Context) -> JsObject {
    let mut layout_obj = ObjectInitializer::new(engine);
    layout_obj
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_get_layout),
            JsString::from("get"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_restore_layout),
            JsString::from("restore"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_layout_save),
            JsString::from("save"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_layout_load),
            JsString::from("load"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_layout_list),
            JsString::from("list"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_layout_delete),
            JsString::from("delete"),
            1,
        )
        .build()
}

/// `helix.pane_mode.*`(当前平级模式与其键位表)。
pub(crate) fn register_pane_mode(engine: &mut Context) -> JsObject {
    let mut pane_mode_obj = ObjectInitializer::new(engine);
    pane_mode_obj
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_pane_mode_current),
            JsString::from("current"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_pane_mode_keymap),
            JsString::from("keymap"),
            0,
        )
        .build()
}
