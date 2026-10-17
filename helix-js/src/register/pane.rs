//! `helix.pane.*` 命名空间(清单/焦点/浮动/嵌入/缩放/均分…)。
//!
//! 从 `lib.rs` 的 `build_engine` 拆出(G6):`build_engine` 只剩"建命名空间 → 交给
//! 根对象 → 注册全局"三件事;成员注册表按命名空间/能力放在本目录。
//!
//! 裸路径(`commands::js_echo`)可用,是因为 `mod.rs` 把兄弟模块导入了本模块。

use super::*;

use boa_engine::{object::ObjectInitializer, Context, JsObject, JsString, NativeFunction};

pub(crate) fn register(engine: &mut Context) -> JsObject {
    let mut pane_obj = ObjectInitializer::new(engine);
    pane_obj
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_pane_list),
            JsString::from("list"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_pane_info),
            JsString::from("info"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_pane_raise),
            JsString::from("raise"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_pane_pin),
            JsString::from("pin"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_pane_float),
            JsString::from("float"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_pane_embed),
            JsString::from("embed"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_close_leaf),
            JsString::from("close"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_focus_leaf),
            JsString::from("focus"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_focus_leaf_dir),
            JsString::from("focus_dir"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_swap_leaf_dir),
            JsString::from("move"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_resize_leaf),
            JsString::from("resize"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_zoom_leaf),
            JsString::from("zoom"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_unzoom),
            JsString::from("unzoom"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_minimize_leaf),
            JsString::from("minimize"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_equalize_leaf),
            JsString::from("equalize"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(layout_api::js_layout_fix),
            JsString::from("fix"),
            2,
        )
        .build()
}
