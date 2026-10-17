//! `helix.icons.*` 命名空间 —— 成员来自 `crate::icons::native_fns()`(数据在 icons.rs)。
//!
//! 直接返回值,不走 UiRequest:图标是核心数据,没有异步往返的必要。

use boa_engine::{object::ObjectInitializer, Context, JsObject, JsString};

pub(crate) fn register(engine: &mut Context) -> JsObject {
    let mut icons_obj = ObjectInitializer::new(engine);
    for (name, f, arity) in crate::icons::native_fns() {
        // `function` 是 `&mut self` 原地改并返回 `&mut Self`,不能赋值回去
        icons_obj.function(f, JsString::from(name), arity);
    }
    icons_obj.build()
}
