//! 根对象上的 UI 类 API(弹窗/面板/组件/状态栏/输入/布局叶子操作)。
//!
//! 从 `lib.rs` 的 `build_engine` 拆出(G6):`build_engine` 只剩"建命名空间 → 交给
//! 根对象 → 注册全局"三件事;成员注册表按命名空间/能力放在本目录。
//!
//! 裸路径(`commands::js_echo`)可用,是因为 `mod.rs` 把兄弟模块导入了本模块。

use super::*;

use boa_engine::{object::ObjectInitializer, JsString, NativeFunction};

pub(crate) fn register(builder: &mut ObjectInitializer<'_>) {
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_open_popup),
        JsString::from("open_popup"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_set_component_render),
        JsString::from("set_component_render"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_close_popup),
        JsString::from("close_popup"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_popup_exists),
        JsString::from("popup_exists"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_open_panel),
        JsString::from("open_panel"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_close_panel),
        JsString::from("close_panel"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_viewport),
        JsString::from("viewport"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_picker_state),
        JsString::from("picker_state"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_move_panel),
        JsString::from("move_panel"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_set_completion_render),
        JsString::from("set_completion_render"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_el),
        JsString::from("el"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(input::js_set_input_value),
        JsString::from("set_input_value"),
        3,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_set_statusline),
        JsString::from("set_statusline"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_set_keymap_hint),
        JsString::from("set_keymap_hint"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(layout_api::js_resize_leaf_dir),
        JsString::from("layout_resize"),
        3,
    );
    builder.function(
        NativeFunction::from_fn_ptr(layout_api::js_swap_leaves),
        JsString::from("layout_swap"),
        2,
    );
}
