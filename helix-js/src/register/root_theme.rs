//! 根对象上的主题/配置/图标/权限/诊断类 API。
//!
//! 从 `lib.rs` 的 `build_engine` 拆出(G6):`build_engine` 只剩"建命名空间 → 交给
//! 根对象 → 注册全局"三件事;成员注册表按命名空间/能力放在本目录。
//!
//! 裸路径(`commands::js_echo`)可用,是因为 `mod.rs` 把兄弟模块导入了本模块。

use super::*;

use boa_engine::{object::ObjectInitializer, JsString, NativeFunction};

pub(crate) fn register(builder: &mut ObjectInitializer<'_>) {
    builder.function(
        NativeFunction::from_fn_ptr(diagnostics::js_diagnostics),
        JsString::from("diagnostics"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(permissions::js_permissions),
        JsString::from("permissions"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(config::js_define_config),
        JsString::from("define_config"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(config::js_get_config),
        JsString::from("get_config"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(config::js_get_config_docs),
        JsString::from("get_config_docs"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(theme::js_set_theme),
        JsString::from("set_theme"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(theme::js_reset_theme),
        JsString::from("reset_theme"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(theme::js_get_style),
        JsString::from("get_style"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(theme::js_theme_info),
        JsString::from("theme_info"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(theme::js_set_theme_name),
        JsString::from("set_theme_name"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(crate::icons::js_set_diagnostic_icons),
        JsString::from("set_diagnostic_icons"),
        1,
    );
}
