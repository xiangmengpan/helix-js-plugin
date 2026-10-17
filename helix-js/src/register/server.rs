//! `helix.server.*` 命名空间(镜像 `:server` 子命令 + 行渲染/任务入口)。
//!
//! 从 `lib.rs` 的 `build_engine` 拆出(G6):`build_engine` 只剩"建命名空间 → 交给
//! 根对象 → 注册全局"三件事;成员注册表按命名空间/能力放在本目录。
//!
//! 裸路径(`commands::js_echo`)可用,是因为 `mod.rs` 把兄弟模块导入了本模块。

use super::*;

use boa_engine::{object::ObjectInitializer, Context, JsObject, JsString, NativeFunction};

pub(crate) fn register(engine: &mut Context) -> JsObject {
    let mut server_obj = ObjectInitializer::new(engine);
    server_obj
        .function(
            NativeFunction::from_fn_ptr(commands::js_server_list),
            JsString::from("list"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_server_search),
            JsString::from("search"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_server_install),
            JsString::from("install"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_server_update),
            JsString::from("update"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_server_remove),
            JsString::from("remove"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(commands::js_server_status),
            JsString::from("status"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(server_rows::js_server_rows),
            JsString::from("rows"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(server_tasks::js_server_task),
            JsString::from("task"),
            2,
        )
        .build()
}
