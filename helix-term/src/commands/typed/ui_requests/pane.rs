//! 面板/布局/堆叠/缩放类请求(24 个 arm)。
//!
//! 从 `typed.rs` 的 `apply_ui_requests` 拆出(G7):每个 arm 变成一个 handler,
//! 顶层只留分派。请求结构体的字段解构写在 handler 签名里,handler 体内是原 arm 的原文。

use super::*;

pub(crate) fn handle_open_panel(
    OpenPanelReq {
        id,
        side,
        size,
        focusable,
        rail,
    }: OpenPanelReq,
) -> anyhow::Result<()> {
    // side 已在 JS 侧白名单校验，此处仅防御性映射
    let side = match side.as_str() {
        "right" => ui::PanelSide::Right,
        "left" => ui::PanelSide::Left,
        "bottom" => ui::PanelSide::Bottom,
        other => bail!("open_panel: unknown side '{other}'"),
    };
    let panel = ui::PluginPanel::new(id, side, focusable);
    job::dispatch_blocking(move |_editor, compositor| {
        use crate::ui::layout::SplitDir;
        // rail:true + left/right → 注册 rail(贴边全高);否则维持普通叶分裂
        if rail && matches!(side, ui::PanelSide::Left | ui::PanelSide::Right) {
            compositor.register_panel(panel, side, size);
        } else {
            let (dir, first) = match side {
                ui::PanelSide::Right => (SplitDir::H, false),
                ui::PanelSide::Left => (SplitDir::H, true),
                ui::PanelSide::Bottom => (SplitDir::V, false),
            };
            let _ = compositor.split_leaf_with_ratio(dir, first, size, Box::new(panel));
        }
    });
    Ok(())
}

pub(crate) fn handle_close_panel(ClosePanelReq { id }: ClosePanelReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        // popup id ≠ 布局树 leaf id（split 分配）：由 close_panel_by_id 按它找叶
        // 先清 render 注册表（幂等），再按实例 id 关掉树里的面板叶
        let _ = helix_js::close_popup(id);
        compositor.close_panel_by_id(id);
    });
    Ok(())
}

pub(crate) fn handle_move_panel(MovePanelReq { id, side }: MovePanelReq) -> anyhow::Result<()> {
    // side 已在 JS 侧白名单校验，此处仅防御性映射（与 OpenPanel 同款）
    let side = match side.as_str() {
        "right" => ui::PanelSide::Right,
        "left" => ui::PanelSide::Left,
        "bottom" => ui::PanelSide::Bottom,
        other => bail!("move_panel: unknown side '{other}'"),
    };
    job::dispatch_blocking(move |editor, compositor| {
        if !compositor.set_panel_side(id, side) {
            editor.set_error(format!("move_panel: no panel with id {id}"));
        }
    });
    Ok(())
}

pub(crate) fn handle_split_leaf(
    SplitLeafReq {
        id,
        dir,
        kind,
        size,
        ..
    }: SplitLeafReq,
) -> anyhow::Result<()> {
    use crate::ui::layout::SplitDir;
    let (dir, first) = match dir.as_str() {
        "right" => (SplitDir::H, false),
        "left" => (SplitDir::H, true),
        "bottom" => (SplitDir::V, false),
        "top" => (SplitDir::V, true),
        other => bail!("split: unknown dir '{other}'"),
    };
    job::dispatch_blocking(move |_editor, compositor| {
        let component: Box<dyn Component> = match kind.as_str() {
            "terminal" => Box::new(crate::ui::plugin_terminal::PluginTerminal::new(
                id, id, size,
            )),
            _ => Box::new(crate::ui::PluginPanel::new(
                id,
                crate::ui::plugin_panel::PanelSide::Right,
                false,
            )),
        };
        // 用预分配 id 直接建叶子（split 的 id 由 JS 分配，回调已注册）
        compositor.split_leaf_prealloc(id, dir, first, size, component);
        // 标注变更种类（紧随其后的 sync_layout_cache 会把它带进 layout-change 事件）
        compositor.set_layout_change_kind("split");
        compositor.sync_layout_cache();
    });
    Ok(())
}

pub(crate) fn handle_close_leaf(CloseLeafReq { id }: CloseLeafReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |editor, compositor| {
        compositor.close_leaf_clean(editor, id);
        compositor.set_layout_change_kind("close");
        compositor.sync_layout_cache();
    });
    Ok(())
}

pub(crate) fn handle_pane_float(PaneFloatReq { id }: PaneFloatReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        // 复用 ②-1 的浮窗模型:set_float 给默认居中槽位
        compositor.set_float(id);
        compositor.set_layout_change_kind("float");
        compositor.sync_layout_cache();
    });
    Ok(())
}

pub(crate) fn handle_stack_create(StackCreateReq { id }: StackCreateReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        // 非兄弟 → None(无效操作);成功才刷新缓存
        if compositor
            .layout_tree()
            .stack_new_with_sibling(id)
            .is_some()
        {
            compositor.set_layout_change_kind("stack-create");
            compositor.sync_layout_cache();
        }
    });
    Ok(())
}

pub(crate) fn handle_stack_activate(
    StackActivateReq { id, member }: StackActivateReq,
) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        let lt = compositor.layout_tree();
        if lt.is_stacked(id) {
            // 轮转到 member 成为锚(= 当前显示)。2 人组最多一次;
            // 上限用成员数,便于将来 >2 成员时不改这里。
            for _ in 0..lt.stack_members(id).len() {
                if lt.stack_anchor_of(id) == Some(member) {
                    break;
                }
                lt.stack_rotate(id, true);
            }
            compositor.set_layout_change_kind("stack-activate");
            compositor.sync_layout_cache();
        }
    });
    Ok(())
}

pub(crate) fn handle_stack_remove(StackRemoveReq { id }: StackRemoveReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        compositor.layout_tree().stack_remove_member(id);
        compositor.set_layout_change_kind("stack-remove");
        compositor.sync_layout_cache();
    });
    Ok(())
}

pub(crate) fn handle_pane_raise(PaneRaiseReq { id }: PaneRaiseReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        compositor.layout_tree().float_raise(id);
        compositor.set_layout_change_kind("raise");
        compositor.sync_layout_cache();
    });
    Ok(())
}

pub(crate) fn handle_pane_pin(PanePinReq { id, on }: PanePinReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        compositor.layout_tree().float_set_pinned(id, on);
        compositor.set_layout_change_kind("pin");
        compositor.sync_layout_cache();
    });
    Ok(())
}

pub(crate) fn handle_pane_embed(PaneEmbedReq { id }: PaneEmbedReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        compositor.layout_tree().remove_float(id);
        compositor.set_layout_change_kind("embed");
        compositor.sync_layout_cache();
    });
    Ok(())
}

pub(crate) fn handle_zoom_leaf(ZoomLeafReq { id }: ZoomLeafReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        compositor.zoom_leaf(id);
    });
    Ok(())
}

pub(crate) fn handle_unzoom() -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        compositor.unzoom();
    });
    Ok(())
}

pub(crate) fn handle_resize_leaf(ResizeLeafReq { id, ratio }: ResizeLeafReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        compositor.resize_leaf(id, ratio);
    });
    Ok(())
}

pub(crate) fn handle_resize_leaf_dir(
    ResizeLeafDirReq { id, dir, delta }: ResizeLeafDirReq,
) -> anyhow::Result<()> {
    use crate::ui::layout::SplitDir;
    let dir = match dir.as_str() {
        "h" => SplitDir::H,
        "v" => SplitDir::V,
        other => bail!("layout_resize: unknown dir '{other}'"),
    };
    job::dispatch_blocking(move |editor, compositor| {
        if !compositor.resize_leaf_dir(id, dir, delta) {
            editor.set_error("layout_resize: 该叶子此方向不可调（父分割方向不匹配）");
        }
    });
    Ok(())
}

pub(crate) fn handle_swap_leaves(SwapLeavesReq { id1, id2 }: SwapLeavesReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |editor, compositor| {
        if !compositor.swap_leaves(id1, id2) {
            editor.set_error("layout_swap: 叶子不存在");
        }
    });
    Ok(())
}

pub(crate) fn handle_minimize_leaf(
    MinimizeLeafReq { id, minimized }: MinimizeLeafReq,
) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        compositor.minimize_leaf(id, minimized);
    });
    Ok(())
}

pub(crate) fn handle_layout_fix(LayoutFixReq { id, fixed }: LayoutFixReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        compositor.set_leaf_fixed(id, fixed);
    });
    Ok(())
}

pub(crate) fn handle_focus_leaf_dir(
    FocusLeafDirReq { id, dir }: FocusLeafDirReq,
) -> anyhow::Result<()> {
    use crate::ui::layout::SplitDir;
    let (dir, first) = match dir.as_str() {
        "left" => (SplitDir::H, true),
        "right" => (SplitDir::H, false),
        "up" => (SplitDir::V, true),
        "down" => (SplitDir::V, false),
        other => bail!("layout_focus: unknown dir '{other}'"),
    };
    job::dispatch_blocking(move |editor, compositor| {
        if compositor.focus_leaf_dir(id, dir, first).is_none() {
            editor.set_error("layout_focus: 该方向无邻居（边界）");
        }
    });
    Ok(())
}

pub(crate) fn handle_swap_leaf_dir(
    SwapLeafDirReq { id, dir }: SwapLeafDirReq,
) -> anyhow::Result<()> {
    use crate::ui::layout::SplitDir;
    let (dir, first) = match dir.as_str() {
        "left" => (SplitDir::H, true),
        "right" => (SplitDir::H, false),
        "up" => (SplitDir::V, true),
        "down" => (SplitDir::V, false),
        other => bail!("layout_swap_dir: unknown dir '{other}'"),
    };
    job::dispatch_blocking(move |editor, compositor| {
        if !compositor.swap_leaf_dir(id, dir, first) {
            editor.set_error("layout_swap_dir: 该方向无邻居");
        }
    });
    Ok(())
}

pub(crate) fn handle_equalize_leaf(EqualizeLeafReq { id }: EqualizeLeafReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        compositor.equalize_leaf(id);
    });
    Ok(())
}

pub(crate) fn handle_focus_leaf(FocusLeafReq { id }: FocusLeafReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor| {
        compositor.focus_leaf(id);
    });
    Ok(())
}

pub(crate) fn handle_cache_layout(
    CacheLayoutReq { text: json }: CacheLayoutReq,
) -> anyhow::Result<()> {
    helix_js::cache_layout(&json);
    // restore_layout 的「接线」:不只写缓存,真按 dump 重建树
    // (复用已有组件;已不存在的叶子收敛掉)
    job::dispatch_blocking(move |editor, compositor| {
        match serde_json::from_str::<serde_json::Value>(&json) {
            Ok(v) => {
                if let Err(e) = compositor.layout_tree().restore(&v) {
                    editor.set_error(format!("restore_layout: {e}"));
                } else {
                    compositor.sync_editor_focus(editor);
                }
            }
            Err(e) => editor.set_error(format!("restore_layout: 非法 JSON: {e}")),
        }
    });
    Ok(())
}
