//! 命名空间 `helix.draw.*`(显示列表绘制)与 `helix.text.*`(显示宽度工具)。
//!
//! `draw` 的语义见 `crate::draw` 顶部注释(`(0,0)` = 组件左上角、越界裁剪、样式 = 主题 scope)。
//! `text` 是 D 档零件:各插件手里重复的 `dw()`/`cut()` 收进核心 —— 与渲染器**同一张宽度表**,
//! 不会出现"插件算 1 列、渲染器占 2 列"的错位。
//!
//! ⚠ 改这里之后跑 `cargo xtask api-check`:它按源码文本解析注册表
//! (容器名 = `let draw_builder = ObjectInitializer::new` ⇒ 命名空间 `draw`)。

use boa_engine::{object::ObjectInitializer, Context, JsObject, JsString, NativeFunction};

pub(crate) fn register(engine: &mut Context) -> JsObject {
    let mut draw_builder = ObjectInitializer::new(engine);
    draw_builder
        .function(
            NativeFunction::from_fn_ptr(crate::draw::js_draw_rect),
            JsString::from("rect"),
            5,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::draw::js_draw_fill),
            JsString::from("fill"),
            5,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::draw::js_draw_hline),
            JsString::from("hline"),
            4,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::draw::js_draw_vline),
            JsString::from("vline"),
            4,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::draw::js_draw_text),
            JsString::from("text"),
            4,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::draw::js_draw_braille),
            JsString::from("braille"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::draw::js_draw_push_clip),
            JsString::from("push_clip"),
            4,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::draw::js_draw_pop_clip),
            JsString::from("pop_clip"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::draw::js_draw_done),
            JsString::from("done"),
            0,
        );
    draw_builder.build()
}

pub(crate) fn register_text(engine: &mut Context) -> JsObject {
    let mut text_builder = ObjectInitializer::new(engine);
    text_builder
        .function(
            NativeFunction::from_fn_ptr(crate::draw::js_text_width),
            JsString::from("width"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::draw::js_text_truncate),
            JsString::from("truncate"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::draw::js_text_pad),
            JsString::from("pad"),
            3,
        );
    text_builder.build()
}
