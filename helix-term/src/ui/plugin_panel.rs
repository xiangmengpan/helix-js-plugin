use crate::compositor::{Component, Compositor, Context, Event, EventResult};
use helix_js::{PopupKeyResult, StyledLine};
use helix_view::graphics::Rect;
use tui::buffer::Buffer as Surface;

use super::plugin_popup::key_to_plugin_key;

/// 侧边面板停靠边（与 open_panel 的 side 白名单对应）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSide {
    Right,
    Left,
    Bottom,
}

/// JS 插件侧边面板层：内容由 JS `render` 回调绘制（复用 render_popup 注册表），
/// 按键由 JS `onKey` 回调处理（未注册 onKey 时缺省全 Ignore——事件穿透给编辑器）。
pub struct PluginPanel {
    /// 共享骨架:焦点/可聚焦节点/行缓存/脏格 diff(见 `ui::js_surface`)
    surface: crate::ui::js_surface::JsSurface,
    side: PanelSide,
    /// 节点焦点路由开关（open_panel focusable;未启用零行为变化）
    focusable: bool,
}

impl PluginPanel {
    pub fn new(id: u64, side: PanelSide, focusable: bool) -> Self {
        Self {
            surface: crate::ui::js_surface::JsSurface::new(id),
            side,
            focusable,
        }
    }

    /// 共享层差异:只有 `focusable` 面板才收集焦点、才做焦点失效重置(未启用 = 零行为变化)。
    fn opts(&self) -> crate::ui::js_surface::SurfaceOpts {
        crate::ui::js_surface::SurfaceOpts {
            collect_focusables: self.focusable,
            reset_missing_focus: self.focusable,
            esc_releases_focus: true,
            label: "panel",
        }
    }

    /// 清空脏格 diff 状态（测试向不同 surface 渲染时需要重置）
    pub fn reset_render_state(&mut self) {
        self.surface.reset_render_state();
    }

    /// 测试/诊断访问器：当前布局行数
    pub fn lines(&self) -> &[StyledLine] {
        self.surface.lines()
    }

    /// 面板实例 id（open_panel 分配的 u64，与 render 注册表共用）；
    /// compositor 用它区分并存的多面板层（remove_panel / 排布收集）。
    pub(crate) fn id(&self) -> u64 {
        self.surface.id
    }

    /// 移动面板到另一侧（move_panel 请求；side 白名单在 JS 侧已校验）。
    /// compositor 每帧枚举面板重排，改后自动生效。
    pub(crate) fn set_side(&mut self, side: PanelSide) {
        self.side = side;
    }

    /// 面板在 JS 注册表丢失（reload/状态丢失的僵尸）：自动移除自愈，
    /// 否则渲染报错 + 按键穿透 → 面板无法关闭。
    fn zombie_selfheal(&self) -> EventResult {
        let id = self.surface.id;
        EventResult::Consumed(Some(Box::new(
            move |compositor: &mut Compositor, cx: &mut Context| {
                helix_js::close_panel_state(id);
                // popup id ≠ leaf id：按 popup id 找面板 leaf 再移除（remove_panel 只清 layers）
                if let Some(leaf) = compositor
                    .layout_tree()
                    .find_leaf_id::<PluginPanel>(|p| p.id() == id)
                {
                    // rail 面板:取除 rail(remove 对 rail 免疫)
                    if !compositor.close_rail(leaf) {
                        compositor.remove_leaf(leaf);
                    }
                }
                cx.editor.set_error(format!(
                    "面板 {id} 状态丢失，已自动关闭（:plugin-reload 后需重新打开）"
                ));
            },
        )))
    }
}

impl Component for PluginPanel {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        // 粘贴：焦点在 input → 批量插入整段 + 一次 onChange；否则忽略（冒泡给编辑器正文粘贴）
        if let Event::Paste(contents) = event {
            return self
                .surface
                .paste(contents, cx, self.opts())
                .unwrap_or(EventResult::Ignored(None));
        }
        let Event::Key(key_event) = event else {
            return EventResult::Ignored(None);
        };
        let key = key_to_plugin_key(key_event);
        // 节点焦点路由（focusable 面板启用）：Tab 移动焦点；焦点在节点时按键直达；
        // Esc 取消焦点回 onKey；无焦点时走 onKey（现状）。未启用 → 走下方既有逻辑。
        if self.focusable && !self.surface.focusables.is_empty() {
            let Some(key) = &key else {
                return EventResult::Ignored(None);
            };
            if key.name == "Tab" && self.surface.tab_focus(key.shift) {
                return EventResult::Consumed(None);
            }
            if let Some(result) = self.surface.node_key(key, cx, self.opts()) {
                return result;
            }
        }
        // 无 onKey 的面板：缺省全 Ignore，不调 popup_key（其缺省 Esc→Close 语义只适用于弹窗）。
        // 面板在 JS 注册表已丢失（reload/状态丢失的僵尸）：自动移除自愈
        if !helix_js::panel_has_onkey(self.surface.id) {
            if !helix_js::popup_exists(self.surface.id) {
                return self.zombie_selfheal();
            }
            return EventResult::Ignored(None);
        }
        let Some(key) = key else {
            return EventResult::Ignored(None);
        };
        // 构建当前文档快照（面板非模态，文档可编辑；每次按键重新序列化，同 PluginPopup）
        let ctx = crate::ui::js_surface::JsSurface::command_context(cx);
        let result = cx.js_popup_key(self.surface.id, &key, &ctx);
        // 应用编辑/光标/消息（在 popup_key 之后：onKey 入队的编辑在本次按键内同步应用）
        crate::ui::js_surface::JsSurface::drain_after_onkey(cx, self.opts());
        match result {
            Ok(PopupKeyResult::Close) => {
                let id = self.surface.id;
                // 先通知 JS（触发 onClose，echo 消息入队），再按实例 id 移除层，最后把
                // echo 消息刷成状态栏（与命令路径取消息的约定一致）。
                EventResult::Consumed(Some(Box::new(
                    move |compositor: &mut Compositor, cx: &mut Context| {
                        let _ = helix_js::close_popup(id);
                        compositor.close_panel_by_id(id);
                        let msgs = helix_js::take_messages();
                        if !msgs.is_empty() {
                            cx.editor.set_status(msgs.join(" "));
                        }
                    },
                )))
            }
            Ok(PopupKeyResult::Handled) => EventResult::Consumed(None),
            // 穿透给编辑器；Err 仅发生在 JS 侧异常时（popup 已不在 JS 注册表——热重载/状态
            // 丢失后的僵尸面板）：自动移除面板自愈，否则渲染报错 + 按键穿透无法关闭
            Ok(PopupKeyResult::Ignored) => EventResult::Ignored(None),
            Err(_) => self.zombie_selfheal(),
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        // focusable 面板:刷新时传焦点 + 提取可聚焦节点;未启用保持现状(传 None 不提取)
        self.surface
            .refresh(cx, (area.width, area.height), self.opts());
        // 脏格 diff 渲染：只重绘变化格
        self.surface.render_lines(area, surface, &cx.editor.theme);
    }

    // 无静态 id：多面板下各层需独立标识，移除/排布一律走 u64 实例 id
    //（compositor.remove_panel / layout_panels 收集）。
}
