//! `helix.picker.*` 命名空间(define / run)。
//!
//! 从 `lib.rs` 的 `build_engine` 拆出(G6):`build_engine` 只剩"建命名空间 → 交给
//! 根对象 → 注册全局"三件事;成员注册表按命名空间/能力放在本目录。
//!
//! 裸路径(`commands::js_echo`)可用,是因为 `mod.rs` 把兄弟模块导入了本模块。

use boa_engine::{object::ObjectInitializer, Context, JsObject, JsString, NativeFunction};

pub(crate) fn register(engine: &mut Context) -> JsObject {
    let mut picker_builder = ObjectInitializer::new(engine);
    picker_builder
        .function(
            NativeFunction::from_fn_ptr(crate::picker::js_picker_define),
            JsString::from("define"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::picker::js_picker_run),
            JsString::from("run"),
            1,
        );

    picker_builder.build()
}
