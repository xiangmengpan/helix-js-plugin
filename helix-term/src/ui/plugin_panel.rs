use crate::commands::typed::{apply_cursor_requests, apply_plugin_edits};
use crate::compositor::{Component, Compositor, Context, Event, EventResult};
use crate::ui::comp_layout;
use helix_js::{CommandContext, PopupKeyResult, StyledLine};
use helix_view::current_ref;
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
    id: u64,
    side: PanelSide,
    lines: Vec<StyledLine>,
    /// 脏格 diff 渲染器（只重绘变化格）
    diff: crate::ui::comp_layout::DiffRenderer,
}

impl PluginPanel {
    pub fn new(id: u64, side: PanelSide) -> Self {
        Self { id, side, lines: Vec::new(), diff: Default::default() }
    }

    /// 清空脏格 diff 状态（测试向不同 surface 渲染时需要重置）
    pub fn reset_render_state(&mut self) {
        self.diff = Default::default();
    }

    /// 测试/诊断访问器：当前布局行数
    pub fn lines(&self) -> &[StyledLine] {
        &self.lines
    }

    /// 面板实例 id（open_panel 分配的 u64，与 render 注册表共用）；
    /// compositor 用它区分并存的多面板层（remove_panel / 排布收集）。
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// 移动面板到另一侧（move_panel 请求；side 白名单在 JS 侧已校验）。
    /// compositor 每帧枚举面板重排，改后自动生效。
    pub(crate) fn set_side(&mut self, side: PanelSide) {
        self.side = side;
    }

    /// 面板在 JS 注册表丢失（reload/状态丢失的僵尸）：自动移除自愈，
    /// 否则渲染报错 + 按键穿透 → 面板无法关闭。
    fn zombie_selfheal(&self) -> EventResult {
        let id = self.id;
        EventResult::Consumed(Some(Box::new(
            move |compositor: &mut Compositor, cx: &mut Context| {
                helix_js::close_panel_state(id);
                // popup id ≠ leaf id：按 popup id 找面板 leaf 再移除（remove_panel 只清 layers）
                if let Some(leaf) =
                    compositor.layout_tree().find_leaf_id::<PluginPanel>(|p| p.id() == id)
                {
                    compositor.remove_leaf(leaf);
                }
                compositor.remove_panel(id);
                cx.editor.set_error(format!(
                    "面板 {id} 状态丢失，已自动关闭（:plugin-reload 后需重新打开）"
                ));
            },
        )))
    }
}

impl Component for PluginPanel {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let Event::Key(key_event) = event else {
            return EventResult::Ignored(None);
        };
        // 无 onKey 的面板：缺省全 Ignore，不调 popup_key（其缺省 Esc→Close 语义只适用于弹窗）。
        // 面板在 JS 注册表已丢失（reload/状态丢失的僵尸）：自动移除自愈
        if !helix_js::panel_has_onkey(self.id) {
            if !helix_js::popup_exists(self.id) {
                return self.zombie_selfheal();
            }
            return EventResult::Ignored(None);
        }
        let Some(key) = key_to_plugin_key(key_event) else {
            return EventResult::Ignored(None);
        };
        // 构建当前文档快照（面板非模态，文档可编辑；每次按键重新序列化，同 PluginPopup）
        // ponytail: 与 PluginPopup::handle_event 重复的 ctx 构建/drain——提取公共辅助需动
        // plugin_popup.rs 的共享逻辑，并行 wave 冲突面上不值得；若第三次复用再提取。
        let ctx = {
            let (view, doc) = current_ref!(cx.editor);
            let text = doc.text();
            let primary = doc.selection(view.id).primary();
            let pos = primary.cursor(text.slice(..));
            let line = text.char_to_line(pos);
            let col = pos - text.line_to_char(line);
            let anchor_line = text.char_to_line(primary.anchor);
            let head_line = text.char_to_line(primary.head);
            CommandContext {
                path: doc.path().map(|p| p.to_string_lossy().into_owned()),
                text: text.to_string(),
                cursor: (line, col),
                selection: (
                    (anchor_line, primary.anchor - text.line_to_char(anchor_line)),
                    (head_line, primary.head - text.line_to_char(head_line)),
                ),
                docs: vec![],
            }
        };
        let result = helix_js::popup_key(self.id, &key, &ctx);
        // 应用编辑/光标/消息（在 popup_key 之后：onKey 入队的编辑在本次按键内同步应用）
        let cursor_reqs = helix_js::take_cursor_requests();
        if !cursor_reqs.is_empty() {
            if let Err(err) = apply_cursor_requests(cx.editor, &cursor_reqs) {
                cx.editor.set_error(format!("plugin panel cursor failed: {err}"));
            }
        }
        let edits = helix_js::take_edits();
        if !edits.is_empty() {
            if let Err(err) = apply_plugin_edits(cx.editor, &edits) {
                cx.editor.set_error(format!("plugin panel edit failed: {err}"));
            }
        }
        let msgs = helix_js::take_messages();
        if !msgs.is_empty() {
            cx.editor.set_status(msgs.join(" "));
        }
        // 面板/弹窗 onKey 里发起的 UI 请求（open_file / move_panel / set_terminal_mode 等）
        // 即时应用——否则请求搁置到下一个 :命令才被 drain
        if let Err(err) = crate::commands::typed::apply_ui_requests(helix_js::take_ui_requests()) {
            cx.editor.set_error(err.to_string());
        }
        match result {
            Ok(PopupKeyResult::Close) => {
                let id = self.id;
                // 先通知 JS（触发 onClose，echo 消息入队），再按实例 id 移除层，最后把
                // echo 消息刷成状态栏（与命令路径取消息的约定一致）。
                EventResult::Consumed(Some(Box::new(
                    move |compositor: &mut Compositor, cx: &mut Context| {
                        let _ = helix_js::close_popup(id);
                        compositor.remove_panel(id);
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
        match helix_js::render_popup(self.id, area.width, area.height, None) {
            Ok(content) => self.lines = comp_layout::render(content, (area.width, area.height)),
            Err(err) => self.lines = vec![StyledLine::plain(format!("<plugin panel error: {err}>"))],
        }
        // 脏格 diff 渲染：只重绘变化格（方案乙③）
        self.diff.render(&self.lines, area, surface, &cx.editor.theme);
    }

    // 无静态 id：多面板下各层需独立标识，移除/排布一律走 u64 实例 id
    //（compositor.remove_panel / layout_panels 收集）。
}


