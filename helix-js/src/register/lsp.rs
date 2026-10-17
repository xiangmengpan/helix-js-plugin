//! `helix.lsp.*` 命名空间(8 个成员,全部转发到 `crate::lsp`)。
//!
//! 从 `lib.rs` 的 `build_engine` 拆出(G6):`build_engine` 只剩"建命名空间 → 交给
//! 根对象 → 注册全局"三件事;成员注册表按命名空间/能力放在本目录。
//!
//! 裸路径(`commands::js_echo`)可用,是因为 `mod.rs` 把兄弟模块导入了本模块。

use boa_engine::{object::ObjectInitializer, Context, JsObject, JsString, NativeFunction};

pub(crate) fn register(engine: &mut Context) -> JsObject {
    let mut lsp_builder = ObjectInitializer::new(engine);
    lsp_builder
        .function(
            NativeFunction::from_fn_ptr(crate::lsp::js_lsp_hover),
            JsString::from("hover"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::lsp::js_lsp_completion),
            JsString::from("completion"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::lsp::js_lsp_goto_definition),
            JsString::from("goto_definition"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::lsp::js_lsp_document_symbols),
            JsString::from("document_symbols"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::lsp::js_lsp_format),
            JsString::from("format"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::lsp::js_lsp_rename),
            JsString::from("rename"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::lsp::js_lsp_code_actions),
            JsString::from("code_actions"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(crate::lsp::js_lsp_execute_code_action),
            JsString::from("execute_code_action"),
            1,
        );

    lsp_builder.build()
}
