use crate::commands::typed::{apply_cursor_requests, apply_plugin_edits};
use crate::compositor::{Component, Compositor, Context, Event, EventResult};
use helix_js::{CommandContext, PopupKeyResult, StyledLine};
use helix_view::current_ref;
use helix_view::graphics::Rect;
use tui::buffer::Buffer as Surface;
use tui::text::{Span, Spans, Text as TuiText};
use tui::widgets::{Paragraph, Widget, Wrap};

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
    size: u16,
    lines: Vec<StyledLine>,
}

impl PluginPanel {
    pub fn new(id: u64, side: PanelSide, size: u16) -> Self {
        Self { id, side, size, lines: Vec::new() }
    }

    /// 面板实例 id（open_panel 分配的 u64，与 render 注册表共用）；
    /// compositor 用它区分并存的多面板层（remove_panel / 排布收集）。
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn side(&self) -> PanelSide {
        self.side
    }

    pub(crate) fn size(&self) -> u16 {
        self.size
    }

    /// 移动面板到另一侧（move_panel 请求；side 白名单在 JS 侧已校验）。
    /// compositor 每帧枚举面板重排，改后自动生效。
    pub(crate) fn set_side(&mut self, side: PanelSide) {
        self.side = side;
    }

    /// 按停靠边从全屏区切出面板区域：size 超界时 clamp 到 area 尺寸。
    /// 单面板场景用（N=1 特例；多面板排布走 layout_panels）。
    fn panel_area(&self, area: Rect) -> Rect {
        match self.side {
            PanelSide::Right => Rect::new(
                area.right().saturating_sub(self.size),
                area.y,
                self.size.min(area.width),
                area.height,
            ),
            PanelSide::Left => Rect::new(area.x, area.y, self.size.min(area.width), area.height),
            PanelSide::Bottom => Rect::new(
                area.x,
                area.bottom().saturating_sub(self.size),
                area.width,
                self.size.min(area.height),
            ),
        }
    }

    /// 把全屏区切成 (面板区, 编辑器区)：面板占停靠边 size，编辑器占剩余部分。
    /// size 超界时面板 clamp 到全尺寸、编辑器区对应维度为 0（saturating 不 panic）。
    /// 单面板场景用（N=1 特例；多面板排布走 layout_panels）。
    pub fn split_area(&self, area: Rect) -> (Rect, Rect) {
        let panel = self.panel_area(area);
        let rest = match self.side {
            PanelSide::Right => Rect::new(
                area.x,
                area.y,
                area.width.saturating_sub(self.size),
                area.height,
            ),
            PanelSide::Left => Rect::new(
                area.x.saturating_add(self.size),
                area.y,
                area.width.saturating_sub(self.size),
                area.height,
            ),
            PanelSide::Bottom => Rect::new(
                area.x,
                area.y,
                area.width,
                area.height.saturating_sub(self.size),
            ),
        };
        (panel, rest)
    }
}

/// 多面板排布（纯函数，可单测）：给定全屏区与面板清单（按开层顺序），
/// 返回每面板的矩形 + 各侧收缩后的剩余区。
/// 排布规则：right 从右缘向内、left 从左缘向内、bottom 从底缘向上，先开的靠边；
/// 各侧独立叠放（bottom 面板横跨全宽，不与左/右面板嵌套）。
/// size 总和超界时 saturating clamp（不 panic）。
/// N=1 时结果与 PluginPanel::split_area 一致（单面板 = 特例）。
pub(crate) fn layout_panels(
    area: Rect,
    panels: &[(u64, PanelSide, u16)],
) -> (Vec<(u64, Rect)>, Rect) {
    let mut rects = Vec::with_capacity(panels.len());
    let mut right_used = 0u16;
    let mut left_used = 0u16;
    let mut bottom_used = 0u16;
    for (id, side, size) in panels {
        let rect = match side {
            PanelSide::Right => {
                let w = (*size).min(area.width.saturating_sub(right_used));
                let r = Rect::new(
                    area.right().saturating_sub(right_used).saturating_sub(w),
                    area.y,
                    w,
                    area.height,
                );
                right_used = right_used.saturating_add(*size);
                r
            }
            PanelSide::Left => {
                let w = (*size).min(area.width.saturating_sub(left_used));
                let r = Rect::new(area.x.saturating_add(left_used), area.y, w, area.height);
                left_used = left_used.saturating_add(*size);
                r
            }
            PanelSide::Bottom => {
                let h = (*size).min(area.height.saturating_sub(bottom_used));
                let r = Rect::new(
                    area.x,
                    area.bottom().saturating_sub(bottom_used).saturating_sub(h),
                    area.width,
                    h,
                );
                bottom_used = bottom_used.saturating_add(*size);
                r
            }
        };
        rects.push((*id, rect));
    }
    let rest = Rect::new(
        left_used,
        area.y,
        area.width.saturating_sub(left_used).saturating_sub(right_used),
        area.height.saturating_sub(bottom_used),
    );
    (rects, rest)
}

impl Component for PluginPanel {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let Event::Key(key_event) = event else {
            return EventResult::Ignored(None);
        };
        // 无 onKey 的面板：缺省全 Ignore，不调 popup_key（其缺省 Esc→Close 语义只适用于弹窗）
        if !helix_js::panel_has_onkey(self.id) {
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
            // 穿透给编辑器；Err 仅发生在 JS 侧异常时，同样放行
            Ok(PopupKeyResult::Ignored) | Err(_) => EventResult::Ignored(None),
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        match helix_js::render_popup(self.id, area.width, area.height) {
            Ok(lines) => self.lines = lines,
            Err(err) => self.lines = vec![StyledLine { text: format!("<plugin panel error: {err}>"), style: None }],
        }
        // 与 PluginPopup 同款样式逻辑：style 名映射主题 scope（未知 scope 返回默认 Style）
        let spans: Vec<Spans> = self
            .lines
            .iter()
            .map(|l| {
                let span = match &l.style {
                    Some(s) => Span::styled(l.text.clone(), cx.editor.theme.get(s)),
                    None => Span::raw(l.text.clone()),
                };
                Spans::from(span)
            })
            .collect();
        let text = TuiText::from(spans);
        let par = Paragraph::new(&text).wrap(Wrap { trim: false });
        par.render(area, surface);
    }

    // 无静态 id：多面板下各层需独立标识，移除/排布一律走 u64 实例 id
    //（compositor.remove_panel / layout_panels 收集）。
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_area_by_side() {
        let area = Rect::new(0, 0, 100, 40);
        // Right：面板在右侧 20 列，编辑器剩左侧 80 列
        let (panel, rest) = PluginPanel::new(1, PanelSide::Right, 20).split_area(area);
        assert_eq!(panel, Rect::new(80, 0, 20, 40));
        assert_eq!(rest, Rect::new(0, 0, 80, 40));
        // Left：面板在左侧 20 列，编辑器剩右侧 80 列
        let (panel, rest) = PluginPanel::new(1, PanelSide::Left, 20).split_area(area);
        assert_eq!(panel, Rect::new(0, 0, 20, 40));
        assert_eq!(rest, Rect::new(20, 0, 80, 40));
        // Bottom：面板在底部 10 行，编辑器剩顶部 30 行
        let (panel, rest) = PluginPanel::new(1, PanelSide::Bottom, 10).split_area(area);
        assert_eq!(panel, Rect::new(0, 30, 100, 10));
        assert_eq!(rest, Rect::new(0, 0, 100, 30));
    }

    #[test]
    fn split_area_clamps_oversized() {
        // size 超屏 → 面板 clamp 到全屏，编辑器区对应维度为 0（saturating 不 panic）
        let area = Rect::new(0, 0, 100, 40);
        let (panel, rest) = PluginPanel::new(1, PanelSide::Right, 500).split_area(area);
        assert_eq!(panel, Rect::new(0, 0, 100, 40));
        assert_eq!(rest, Rect::new(0, 0, 0, 40));
        let (panel, rest) = PluginPanel::new(1, PanelSide::Bottom, 500).split_area(area);
        assert_eq!(panel, Rect::new(0, 0, 100, 40));
        assert_eq!(rest, Rect::new(0, 0, 100, 0));
    }

    #[test]
    fn layout_panels_right_side() {
        let area = Rect::new(0, 0, 100, 40);
        // 同侧多面板：先开的靠右缘，后开的向内叠
        let (rects, rest) = layout_panels(area, &[(1, PanelSide::Right, 20), (2, PanelSide::Right, 10)]);
        assert_eq!(rects, vec![(1, Rect::new(80, 0, 20, 40)), (2, Rect::new(70, 0, 10, 40))]);
        assert_eq!(rest, Rect::new(0, 0, 70, 40));
        // N=1 特例：与 split_area 一致
        let (rects, rest) = layout_panels(area, &[(1, PanelSide::Right, 20)]);
        assert_eq!(rects, vec![(1, Rect::new(80, 0, 20, 40))]);
        assert_eq!(rest, Rect::new(0, 0, 80, 40));
    }

    #[test]
    fn layout_panels_mixed_sides() {
        let area = Rect::new(0, 0, 100, 40);
        // 各侧独立叠放：left 从左上、bottom 从底缘（横跨全宽）；剩余区各侧收缩
        let (rects, rest) = layout_panels(
            area,
            &[(1, PanelSide::Left, 10), (2, PanelSide::Bottom, 5), (3, PanelSide::Right, 15)],
        );
        assert_eq!(rects, vec![
            (1, Rect::new(0, 0, 10, 40)),
            (2, Rect::new(0, 35, 100, 5)),
            (3, Rect::new(85, 0, 15, 40)),
        ]);
        assert_eq!(rest, Rect::new(10, 0, 75, 35));
        // 无面板：剩余区 = 全屏
        let (rects, rest) = layout_panels(area, &[]);
        assert!(rects.is_empty());
        assert_eq!(rest, area);
    }

    #[test]
    fn layout_panels_clamps_oversized() {
        // size 总和超屏 → saturating clamp 不 panic：首面板吃满全宽，后续宽度为 0
        let area = Rect::new(0, 0, 100, 40);
        let (rects, rest) = layout_panels(area, &[(1, PanelSide::Right, 500), (2, PanelSide::Right, 10)]);
        assert_eq!(rects[0].1, Rect::new(0, 0, 100, 40));
        assert_eq!(rects[1].1.width, 0);
        assert_eq!(rest, Rect::new(0, 0, 0, 40));
    }
}
