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

    /// 把全屏区切成 (面板区, 编辑器区)：面板占停靠边 size，编辑器占剩余部分。
    /// size 超界时面板 clamp 到全尺寸、编辑器区对应维度为 0（saturating 不 panic）。
    /// compositor.render 用此结果分流：面板层拿 panel，其余层拿 rest。
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

impl Component for PluginPanel {
    // 面板事件恒穿透：编辑器照常接收按键（仅渲染占位，不消费输入）
    fn handle_event(&mut self, _event: &Event, _cx: &mut Context) -> EventResult {
        EventResult::Ignored(None)
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
}
