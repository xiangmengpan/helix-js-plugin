//! 文件监视请求(`Watch`/`Unwatch` 原先是一个 `ref request @ (A | B)` 组合 arm)。
//!
//! 从 `typed.rs` 的 `apply_ui_requests` 拆出(G7):每个 arm 变成一个 handler,
//! 顶层只留分派。请求结构体的字段解构写在 handler 签名里,handler 体内是原 arm 的原文。

pub(crate) fn apply_watch(request: helix_js::UiRequest) -> anyhow::Result<()> {
    crate::plugins_watch::apply_watch(&request)?;
    Ok(())
}
