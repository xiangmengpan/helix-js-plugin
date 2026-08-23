use crate::compositor::{Component, Context};
use helix_view::graphics::{Margin, Rect};
use helix_view::info::{Info, InfoPosition};
use tui::buffer::Buffer as Surface;
use tui::text::Text;
use tui::widgets::{Block, Paragraph, Widget};

impl Component for Info {
    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let text_style = cx.editor.theme.get("ui.text.info");
        let popup_style = cx.editor.theme.get("ui.popup.info");

        // Calculate the area of the terminal to modify. Because we want to
        // render at a fixed screen position (bottom-right by default), we use
        // the viewport's width and height which evaluate to the most bottom
        // right coordinate.
        let width = self.width + 2 + 2; // +2 for border, +2 for margin
        let height = self.height + 2; // +2 for border
        let (x, y) = match self.position {
            InfoPosition::BottomRight => (
                viewport.width.saturating_sub(width),
                viewport.height.saturating_sub(height + 2), // +2 for statusline
            ),
            InfoPosition::BottomLeft => (0, viewport.height.saturating_sub(height + 2)),
            InfoPosition::TopRight => (viewport.width.saturating_sub(width), 0),
            InfoPosition::TopLeft => (0, 0),
            InfoPosition::Center => (
                viewport.width.saturating_sub(width) / 2,
                viewport.height.saturating_sub(height) / 2,
            ),
        };
        let area = viewport.intersection(Rect::new(x, y, width, height));
        surface.clear_with(area, popup_style);

        let block = Block::bordered()
            .title(self.title.as_ref())
            .border_style(popup_style);

        let margin = Margin::horizontal(1);
        let inner = block.inner(area).inner(margin);
        block.render(area, surface);

        Paragraph::new(&Text::from(self.text.as_str()))
            .style(text_style)
            .render(inner, surface);
    }
}
