//! `helix.plugin` —— 可调用函数 `plugin(name, deps)`,deps 自动加载。
//!
//! 它不是一个命名空间对象,而是**函数对象**;install/update/remove 由 `helix.plugin.*`
//! 之外的管理命令提供,不在这里。

use super::*;

use boa_engine::{
    object::{builtins::JsFunction, FunctionObjectBuilder},
    Context, NativeFunction,
};

pub(crate) fn build(engine: &mut Context) -> JsFunction {
    FunctionObjectBuilder::new(
        engine.realm(),
        NativeFunction::from_fn_ptr(commands::js_plugin),
    )
    .name("plugin")
    .length(2)
    .build()
}
