use crate::compositor::{Component, Context, Event, EventResult};
use helix_js::StyledLine;
use helix_view::graphics::Rect;
use tui::buffer::Buffer as Surface;
use tui::text::{Span, Spans, Text as TuiText};
use tui::widgets::{Paragraph, Widget, Wrap};

/// 侧边面板停靠边（与 open_panel 的 side 白名单对应）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSide {
    Right,
    Left,
    Bottom,
}

/// JS 插件侧边面板层：内容由 JS `render` 回调绘制（复用 render_popup 注册表），
/// 不拦截任何按键——handle_event 恒 Ignored，事件穿透给编辑器。
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

    /// 按停靠边从全屏区切出面板区域：size 超界时 clamp 到 area 尺寸。
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
}

impl Component for PluginPanel {
    // 面板事件恒穿透：编辑器照常接收按键（仅渲染占位，不消费输入）
    fn handle_event(&mut self, _event: &Event, _cx: &mut Context) -> EventResult {
        EventResult::Ignored(None)
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let area = self.panel_area(area);
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

    // 静态 id：compositor.remove("plugin-panel") 按 id 移除层
    // ponytail: 单面板 PoC——id 不区分实例，再次 open 会 replace_or_push 替换旧层
    fn id(&self) -> Option<&'static str> {
        Some("plugin-panel")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_area_by_side() {
        let area = Rect::new(0, 0, 100, 40);
        assert_eq!(PluginPanel::new(1, PanelSide::Right, 20).panel_area(area), Rect::new(80, 0, 20, 40));
        assert_eq!(PluginPanel::new(1, PanelSide::Left, 20).panel_area(area), Rect::new(0, 0, 20, 40));
        assert_eq!(PluginPanel::new(1, PanelSide::Bottom, 10).panel_area(area), Rect::new(0, 30, 100, 10));
    }

    #[test]
    fn panel_area_clamps_oversized() {
        // size 超屏宽 → clamp 到全宽，x 回退到 0（saturating 不 panic）
        let area = Rect::new(0, 0, 100, 40);
        assert_eq!(PluginPanel::new(1, PanelSide::Right, 500).panel_area(area), Rect::new(0, 0, 100, 40));
        assert_eq!(PluginPanel::new(1, PanelSide::Bottom, 500).panel_area(area), Rect::new(0, 0, 100, 40));
    }
}
