//! 取词器请求(1 个 arm)。
//!
//! 从 `typed.rs` 的 `apply_ui_requests` 拆出(G7):每个 arm 变成一个 handler,
//! 顶层只留分派。请求结构体的字段解构写在 handler 签名里,handler 体内是原 arm 的原文。

use super::*;

pub(crate) fn handle_open_picker(
    OpenPickerReq {
        source,
        columns,
        rows,
    }: OpenPickerReq,
) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        ui::js_picker::open_picker(compositor, &source, rows, columns);
    });
    Ok(())
}
