//! 面板/布局/堆叠/缩放类请求(24 个 arm)。
//!
//! 从 `typed.rs` 的 `apply_ui_requests` 拆出(G7):每个 arm 变成一个 handler。
//! G12 起 handler 不再自己 `job::dispatch_blocking`,而是返回
//! `ui_op::UiOp` 描述 —— 参数映射可单测,执行收敛在
//! `UiOp::apply` 一处。函数体里的 `bail!` 与参数白名单**未改**。

use super::*;

pub(crate) fn handle_open_panel(
    OpenPanelReq {
        id,
        side,
        size,
        focusable,
        rail,
    }: OpenPanelReq,
) -> anyhow::Result<Option<UiOp>> {
    // side 已在 JS 侧白名单校验，此处仅防御性映射
    let side = match side.as_str() {
        "right" => ui::PanelSide::Right,
        "left" => ui::PanelSide::Left,
        "bottom" => ui::PanelSide::Bottom,
        other => bail!("open_panel: unknown side '{other}'"),
    };
    Ok(Some(UiOp::OpenPanel {
        id,
        side,
        size,
        focusable,
        rail,
    }))
}

pub(crate) fn handle_close_panel(
    ClosePanelReq { id }: ClosePanelReq,
) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::ClosePanel { id }))
}

pub(crate) fn handle_move_panel(
    MovePanelReq { id, side }: MovePanelReq,
) -> anyhow::Result<Option<UiOp>> {
    // side 已在 JS 侧白名单校验，此处仅防御性映射（与 OpenPanel 同款）
    let side = match side.as_str() {
        "right" => ui::PanelSide::Right,
        "left" => ui::PanelSide::Left,
        "bottom" => ui::PanelSide::Bottom,
        other => bail!("move_panel: unknown side '{other}'"),
    };
    Ok(Some(UiOp::MovePanel { id, side }))
}

pub(crate) fn handle_split_leaf(
    SplitLeafReq {
        id,
        dir,
        kind,
        size,
        ..
    }: SplitLeafReq,
) -> anyhow::Result<Option<UiOp>> {
    use crate::ui::layout::SplitDir;
    let (dir, first) = match dir.as_str() {
        "right" => (SplitDir::H, false),
        "left" => (SplitDir::H, true),
        "bottom" => (SplitDir::V, false),
        "top" => (SplitDir::V, true),
        other => bail!("split: unknown dir '{other}'"),
    };
    Ok(Some(UiOp::SplitLeaf {
        id,
        dir,
        first,
        size,
        kind,
    }))
}

pub(crate) fn handle_close_leaf(CloseLeafReq { id }: CloseLeafReq) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::CloseLeaf { id }))
}

pub(crate) fn handle_pane_float(PaneFloatReq { id }: PaneFloatReq) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::SetFloat { id }))
}

pub(crate) fn handle_stack_create(
    StackCreateReq { id }: StackCreateReq,
) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::StackCreate { id }))
}

pub(crate) fn handle_stack_activate(
    StackActivateReq { id, member }: StackActivateReq,
) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::StackActivate { id, member }))
}

pub(crate) fn handle_stack_remove(
    StackRemoveReq { id }: StackRemoveReq,
) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::StackRemove { id }))
}

pub(crate) fn handle_pane_raise(PaneRaiseReq { id }: PaneRaiseReq) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::RaiseLeaf { id }))
}

pub(crate) fn handle_pane_pin(PanePinReq { id, on }: PanePinReq) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::PinLeaf { id, on }))
}

pub(crate) fn handle_pane_embed(PaneEmbedReq { id }: PaneEmbedReq) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::EmbedLeaf { id }))
}

pub(crate) fn handle_zoom_leaf(ZoomLeafReq { id }: ZoomLeafReq) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::Zoom { id }))
}

pub(crate) fn handle_unzoom() -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::Unzoom))
}

pub(crate) fn handle_resize_leaf(
    ResizeLeafReq { id, ratio }: ResizeLeafReq,
) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::ResizeLeaf { id, ratio }))
}

pub(crate) fn handle_resize_leaf_dir(
    ResizeLeafDirReq { id, dir, delta }: ResizeLeafDirReq,
) -> anyhow::Result<Option<UiOp>> {
    use crate::ui::layout::SplitDir;
    let dir = match dir.as_str() {
        "h" => SplitDir::H,
        "v" => SplitDir::V,
        other => bail!("layout_resize: unknown dir '{other}'"),
    };
    Ok(Some(UiOp::ResizeLeafDir { id, dir, delta }))
}

pub(crate) fn handle_swap_leaves(
    SwapLeavesReq { id1, id2 }: SwapLeavesReq,
) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::SwapLeaves { id1, id2 }))
}

pub(crate) fn handle_minimize_leaf(
    MinimizeLeafReq { id, minimized }: MinimizeLeafReq,
) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::Minimize { id, minimized }))
}

pub(crate) fn handle_layout_fix(
    LayoutFixReq { id, fixed }: LayoutFixReq,
) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::SetFixed { id, fixed }))
}

pub(crate) fn handle_focus_leaf_dir(
    FocusLeafDirReq { id, dir }: FocusLeafDirReq,
) -> anyhow::Result<Option<UiOp>> {
    use crate::ui::layout::SplitDir;
    let (dir, first) = match dir.as_str() {
        "left" => (SplitDir::H, true),
        "right" => (SplitDir::H, false),
        "up" => (SplitDir::V, true),
        "down" => (SplitDir::V, false),
        other => bail!("layout_focus: unknown dir '{other}'"),
    };
    Ok(Some(UiOp::FocusLeafDir { id, dir, first }))
}

pub(crate) fn handle_swap_leaf_dir(
    SwapLeafDirReq { id, dir }: SwapLeafDirReq,
) -> anyhow::Result<Option<UiOp>> {
    use crate::ui::layout::SplitDir;
    let (dir, first) = match dir.as_str() {
        "left" => (SplitDir::H, true),
        "right" => (SplitDir::H, false),
        "up" => (SplitDir::V, true),
        "down" => (SplitDir::V, false),
        other => bail!("layout_swap_dir: unknown dir '{other}'"),
    };
    Ok(Some(UiOp::SwapLeafDir { id, dir, first }))
}

pub(crate) fn handle_equalize_leaf(
    EqualizeLeafReq { id }: EqualizeLeafReq,
) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::Equalize { id }))
}

pub(crate) fn handle_focus_leaf(FocusLeafReq { id }: FocusLeafReq) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::FocusLeaf { id }))
}

pub(crate) fn handle_cache_layout(
    CacheLayoutReq { text: json }: CacheLayoutReq,
) -> anyhow::Result<Option<UiOp>> {
    helix_js::cache_layout(&json);
    // restore_layout 的「接线」:不只写缓存,真按 dump 重建树
    // (复用已有组件;已不存在的叶子收敛掉) —— 真正重建在 `UiOp::RestoreLayout::apply`
    Ok(Some(UiOp::RestoreLayout { json }))
}
