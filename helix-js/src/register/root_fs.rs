//! 根对象上的文件系统类 API(读写文件/目录、插件目录、watch…)。
//!
//! 从 `lib.rs` 的 `build_engine` 拆出(G6):`build_engine` 只剩"建命名空间 → 交给
//! 根对象 → 注册全局"三件事;成员注册表按命名空间/能力放在本目录。
//!
//! 裸路径(`commands::js_echo`)可用,是因为 `mod.rs` 把兄弟模块导入了本模块。

use super::*;

use boa_engine::{object::ObjectInitializer, JsString, NativeFunction};

pub(crate) fn register(builder: &mut ObjectInitializer<'_>) {
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_read_dir),
        JsString::from("read_dir"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_open_file),
        JsString::from("open_file"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_runtime_path),
        JsString::from("runtime_path"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_command_args),
        JsString::from("command_args"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_cwd),
        JsString::from("cwd"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_plugins_dir),
        JsString::from("plugins_dir"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_read_file),
        JsString::from("read_file"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(popup::js_write_file),
        JsString::from("write_file"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_write_plugin_file),
        JsString::from("write_plugin_file"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_loaded_plugins),
        JsString::from("loaded_plugins"),
        0,
    );
    builder.function(
        NativeFunction::from_fn_ptr(commands::js_remove_plugin_file),
        JsString::from("remove_plugin_file"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(shell::js_read_file_async),
        JsString::from("read_file_async"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(shell::js_write_file_async),
        JsString::from("write_file_async"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(shell::js_stat_async),
        JsString::from("stat_async"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(shell::js_glob_async),
        JsString::from("glob_async"),
        1,
    );
    builder.function(
        NativeFunction::from_fn_ptr(shell::js_read_tree),
        JsString::from("read_tree"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(watch::js_watch),
        JsString::from("watch"),
        2,
    );
    builder.function(
        NativeFunction::from_fn_ptr(watch::js_unwatch),
        JsString::from("unwatch"),
        1,
    );
}
