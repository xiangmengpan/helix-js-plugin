//! 弹窗请求(`OpenPopup`)—— 建浮层并交给事件循环。
//!
//! 从 `typed.rs` 的 `apply_ui_requests` 拆出(G7):每个 arm 变成一个 handler,
//! 顶层只留分派。请求结构体的字段解构写在 handler 签名里,handler 体内是原 arm 的原文。

use super::*;

pub(crate) fn handle_open_popup(
    OpenPopupReq {
        id,
        width,
        height,
        position,
        layer,
        center,
        size_pct,
    }: OpenPopupReq,
) -> anyhow::Result<()> {
    let lid = layer_id(&layer);
    // center → 居中浮层(layer 层可叠多张,同层替换);anchor → 现状(参照点/尺寸 hint)
    let popup = if center {
        ui::Popup::new(lid, ui::PluginPopup::new(id, None).with_layer(lid))
            .floating(width, height, size_pct)
            .auto_close(false)
    } else {
        ui::Popup::new(
            lid,
            ui::PluginPopup::new(id, width.zip(height)).with_layer(lid),
        )
        .position(position.map(|(row, col)| helix_core::Position::new(row as usize, col as usize)))
        .auto_close(false)
    };
    // 由事件循环在下一轮推层并渲染。
    job::dispatch_blocking(move |_editor, compositor, _js| {
        compositor.replace_or_push(lid, popup);
    });
    Ok(())
}

/// `helix.close_popup(id)`:主动关闭一个 JS 弹层。
///
/// 顺序与 onKey 的 "close" 路径**完全一致**:先 `helix_js::close_popup`(触发
/// `onClose` 并从注册表移除;幂等),再由宿主按实例 id 找到并移除对应层
/// (弹层可能不在栈顶,所以不能 `pop()`)。
pub(crate) fn handle_close_popup(ClosePopupReq { id }: ClosePopupReq) -> anyhow::Result<()> {
    let _ = helix_js::close_popup(id);
    job::dispatch_blocking(move |_editor, compositor, _js| {
        compositor.close_popup_by_id(id);
    });
    Ok(())
}
