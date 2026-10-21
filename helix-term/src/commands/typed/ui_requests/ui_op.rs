//! `UiOp` —— 一次 UI 变更的**描述**(G12)。
//!
//! 为什么改成值:原先 pane 族的 24 个 handler 各自把 UI 接线塞进
//! `job::dispatch_blocking(move |editor, compositor, _js| { … })` 闭包里 —— 想验证
//! 「这个请求到底要做什么」只能起一个 `Application`;而且
//! `set_layout_change_kind(..) + sync_layout_cache()` 这对调用在 24 处手写,
//! 漏一处就产出 `kind: null` 的 `layout-change` 事件(`docs/plugin-api.md` §4 记过这个坑)。
//!
//! 现在:handler 只负责「请求 → op」的映射与参数校验(可单测),执行收敛到
//! [`UiOp::apply`] 一处。
//!
//! **两条刻意的不变**:
//! 1. 一个请求仍然只 `dispatch` 一次(不合并成批)—— 合并会改变与其他 UI 工作的相对时序;
//! 2. `apply` **不补齐**缺失的 `set_layout_change_kind`:哪些操作发 `layout-change` 是既有行为,
//!    顺手补上属于行为变化(铁律 2)。这里只把"原先有的"照原样留住。

use super::*;

use crate::compositor::Compositor;
use crate::ui::layout::SplitDir;
use helix_view::Editor;

/// 一次 UI 变更。所有载荷都是**数据**(组件在这里才构造),所以能 `derive(PartialEq)`、
/// 能在单测里直接断言。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum UiOp {
    OpenPanel {
        id: u64,
        side: ui::PanelSide,
        size: u16,
        focusable: bool,
        rail: bool,
    },
    ClosePanel {
        id: u64,
    },
    MovePanel {
        id: u64,
        side: ui::PanelSide,
    },
    SplitLeaf {
        id: u64,
        dir: SplitDir,
        first: bool,
        size: u16,
        kind: String,
    },
    CloseLeaf {
        id: u64,
    },
    SetFloat {
        id: u64,
    },
    StackCreate {
        id: u64,
    },
    StackActivate {
        id: u64,
        member: u64,
    },
    StackRemove {
        id: u64,
    },
    RaiseLeaf {
        id: u64,
    },
    PinLeaf {
        id: u64,
        on: bool,
    },
    EmbedLeaf {
        id: u64,
    },
    Zoom {
        id: u64,
    },
    Unzoom,
    ResizeLeaf {
        id: u64,
        ratio: f32,
    },
    ResizeLeafDir {
        id: u64,
        dir: SplitDir,
        delta: f32,
    },
    SwapLeaves {
        id1: u64,
        id2: u64,
    },
    Minimize {
        id: u64,
        minimized: bool,
    },
    SetFixed {
        id: u64,
        fixed: bool,
    },
    FocusLeafDir {
        id: u64,
        dir: SplitDir,
        first: bool,
    },
    SwapLeafDir {
        id: u64,
        dir: SplitDir,
        first: bool,
    },
    Equalize {
        id: u64,
    },
    FocusLeaf {
        id: u64,
    },
    RestoreLayout {
        json: String,
    },
}

impl UiOp {
    /// 唯一的执行点。函数体逐段搬自各 handler 原来的闭包,**没有改任何判断条件**。
    pub(crate) fn apply(self, editor: &mut Editor, compositor: &mut Compositor) {
        match self {
            UiOp::OpenPanel {
                id,
                side,
                size,
                focusable,
                rail,
            } => {
                let panel = ui::PluginPanel::new(id, side, focusable);
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
            }
            UiOp::ClosePanel { id } => {
                // popup id ≠ 布局树 leaf id(split 分配):由 close_panel_by_id 按它找叶
                // 先清 render 注册表(幂等),再按实例 id 关掉树里的面板叶
                let _ = helix_js::close_popup(id);
                compositor.close_panel_by_id(id);
            }
            UiOp::MovePanel { id, side } => {
                if !compositor.set_panel_side(id, side) {
                    editor.set_error(format!("move_panel: no panel with id {id}"));
                }
            }
            UiOp::SplitLeaf {
                id,
                dir,
                first,
                size,
                kind,
            } => {
                let component: Box<dyn Component> = match kind.as_str() {
                    "terminal" => Box::new(crate::ui::plugin_terminal::PluginTerminal::new(
                        id, id, size,
                    )),
                    _ => Box::new(ui::PluginPanel::new(id, ui::PanelSide::Right, false)),
                };
                // 用预分配 id 直接建叶子(split 的 id 由 JS 分配,回调已注册)
                compositor.split_leaf_prealloc(id, dir, first, size, component);
                // 标注变更种类(紧随其后的 sync_layout_cache 会把它带进 layout-change 事件)
                compositor.set_layout_change_kind("split");
                compositor.sync_layout_cache();
            }
            UiOp::CloseLeaf { id } => {
                compositor.close_leaf_clean(editor, id);
                compositor.set_layout_change_kind("close");
                compositor.sync_layout_cache();
            }
            UiOp::SetFloat { id } => {
                // 复用 ②-1 的浮窗模型:set_float 给默认居中槽位
                compositor.set_float(id);
                compositor.set_layout_change_kind("float");
                compositor.sync_layout_cache();
            }
            UiOp::StackCreate { id } => {
                // 非兄弟 → None(无效操作);成功才刷新缓存
                if compositor
                    .layout_tree()
                    .stack_new_with_sibling(id)
                    .is_some()
                {
                    compositor.set_layout_change_kind("stack-create");
                    compositor.sync_layout_cache();
                }
            }
            UiOp::StackActivate { id, member } => {
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
            }
            UiOp::StackRemove { id } => {
                compositor.layout_tree().stack_remove_member(id);
                compositor.set_layout_change_kind("stack-remove");
                compositor.sync_layout_cache();
            }
            UiOp::RaiseLeaf { id } => {
                compositor.layout_tree().float_raise(id);
                compositor.set_layout_change_kind("raise");
                compositor.sync_layout_cache();
            }
            UiOp::PinLeaf { id, on } => {
                compositor.layout_tree().float_set_pinned(id, on);
                compositor.set_layout_change_kind("pin");
                compositor.sync_layout_cache();
            }
            UiOp::EmbedLeaf { id } => {
                compositor.layout_tree().remove_float(id);
                compositor.set_layout_change_kind("embed");
                compositor.sync_layout_cache();
            }
            UiOp::Zoom { id } => compositor.zoom_leaf(id),
            UiOp::Unzoom => compositor.unzoom(),
            UiOp::ResizeLeaf { id, ratio } => compositor.resize_leaf(id, ratio),
            UiOp::ResizeLeafDir { id, dir, delta } => {
                if !compositor.resize_leaf_dir(id, dir, delta) {
                    editor.set_error("layout_resize: 该叶子此方向不可调（父分割方向不匹配）");
                }
            }
            UiOp::SwapLeaves { id1, id2 } => {
                if !compositor.swap_leaves(id1, id2) {
                    editor.set_error("layout_swap: 叶子不存在");
                }
            }
            UiOp::Minimize { id, minimized } => compositor.minimize_leaf(id, minimized),
            UiOp::SetFixed { id, fixed } => compositor.set_leaf_fixed(id, fixed),
            UiOp::FocusLeafDir { id, dir, first } => {
                if compositor.focus_leaf_dir(id, dir, first).is_none() {
                    editor.set_error("layout_focus: 该方向无邻居（边界）");
                }
            }
            UiOp::SwapLeafDir { id, dir, first } => {
                if !compositor.swap_leaf_dir(id, dir, first) {
                    editor.set_error("layout_swap_dir: 该方向无邻居");
                }
            }
            UiOp::Equalize { id } => compositor.equalize_leaf(id),
            UiOp::FocusLeaf { id } => compositor.focus_leaf(id),
            UiOp::RestoreLayout { json } => {
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
            }
        }
    }
}

/// 分派:保持「一个请求一次 dispatch」(与改造前逐请求一次的时序一致)。
pub(crate) fn dispatch(op: Option<UiOp>) {
    if let Some(op) = op {
        job::dispatch_blocking(move |editor, compositor, _js| op.apply(editor, compositor));
    }
}

#[cfg(test)]
mod tests {
    use super::super::pane::*;
    use super::*;

    /// 映射逻辑的单测:不起 `Application`,直接断言「这个请求要做什么」。
    /// 真正执行仍由集成测试覆盖(377 条),这里只钉参数映射与白名单。
    #[test]
    fn zoom_leaf_builds_zoom_op() {
        let op = handle_zoom_leaf(ZoomLeafReq { id: 7 }).unwrap();
        assert_eq!(op, Some(UiOp::Zoom { id: 7 }));
    }

    #[test]
    fn split_leaf_maps_all_directions() {
        let mk = |dir: &str| SplitLeafReq {
            id: 1,
            dir: dir.to_string(),
            kind: "panel".into(),
            cmd: None,
            size: 30,
        };
        assert_eq!(
            handle_split_leaf(mk("right")).unwrap(),
            Some(UiOp::SplitLeaf {
                id: 1,
                dir: SplitDir::H,
                first: false,
                size: 30,
                kind: "panel".into()
            })
        );
        assert_eq!(
            handle_split_leaf(mk("top")).unwrap(),
            Some(UiOp::SplitLeaf {
                id: 1,
                dir: SplitDir::V,
                first: true,
                size: 30,
                kind: "panel".into()
            })
        );
        let err = handle_split_leaf(mk("diagonal")).unwrap_err().to_string();
        assert!(err.contains("split: unknown dir"), "{err}");
    }

    #[test]
    fn open_panel_maps_side_and_keeps_rail() {
        let mk = |side: &str, rail: bool| OpenPanelReq {
            id: 3,
            side: side.to_string(),
            size: 20,
            focusable: true,
            rail,
        };
        assert_eq!(
            handle_open_panel(mk("left", true)).unwrap(),
            Some(UiOp::OpenPanel {
                id: 3,
                side: ui::PanelSide::Left,
                size: 20,
                focusable: true,
                rail: true
            })
        );
        let err = handle_open_panel(mk("center", false))
            .unwrap_err()
            .to_string();
        assert!(err.contains("open_panel: unknown side"), "{err}");
    }

    #[test]
    fn cache_layout_carries_json_verbatim() {
        let json = r#"{"leaves":[]}"#.to_string();
        let op = handle_cache_layout(CacheLayoutReq { text: json.clone() }).unwrap();
        assert_eq!(op, Some(UiOp::RestoreLayout { json }));
    }

    #[test]
    fn close_leaf_has_no_editor_side_effect_at_build_time() {
        // 建 op 阶段不碰 editor/compositor:只是把请求翻译成描述
        let op = handle_close_leaf(CloseLeafReq { id: 9 }).unwrap();
        assert_eq!(op, Some(UiOp::CloseLeaf { id: 9 }));
    }
}
