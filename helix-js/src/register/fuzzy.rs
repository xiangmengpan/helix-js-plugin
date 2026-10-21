//! `helix.fuzzy.*` —— 进程内模糊匹配（nucleo）。实现与理由见 `crate::fuzzy`。

use super::*;

use boa_engine::{object::ObjectInitializer, Context, JsObject, JsString, NativeFunction};

pub(crate) fn register(engine: &mut Context) -> JsObject {
    let mut fuzzy_obj = ObjectInitializer::new(engine);
    fuzzy_obj.function(
        NativeFunction::from_fn_ptr(crate::fuzzy::js_fuzzy_session),
        JsString::from("session"),
        1,
    );
    fuzzy_obj.function(
        NativeFunction::from_fn_ptr(crate::fuzzy::js_fuzzy_run),
        JsString::from("run"),
        3,
    );
    fuzzy_obj.function(
        NativeFunction::from_fn_ptr(crate::fuzzy::js_fuzzy_drop),
        JsString::from("drop"),
        1,
    );
    fuzzy_obj.build()
}
