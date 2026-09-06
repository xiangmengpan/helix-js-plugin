use crate::{
    commands::Open,
    compositor::{Callback, Component, Context, Event, EventResult},
    ctrl, key,
};
use tui::{
    buffer::Buffer as Surface,
    widgets::{Block, Widget},
};

use helix_core::Position;
use helix_view::{
    graphics::{Margin, Rect},
    input::{MouseEvent, MouseEventKind},
    Editor,
};

const MIN_HEIGHT: u16 = 6;
const MAX_HEIGHT: u16 = 26;
const MAX_WIDTH: u16 = 120;

struct RenderInfo {
    area: Rect,
    child_height: u16,
    render_borders: bool,
    is_menu: bool,
}

/// 弹窗在屏幕上的布局方式。
/// Anchor = 既有锚点式(参照光标/position,按内容自适应尺寸);
/// Float = 居中浮层:无视 cursor/内容尺寸,按视口算固定矩形。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum PopupLayout {
    #[default]
    Anchor,
    Float {
        /// 视口百分比尺寸(78 → 视口宽 78%);两轴成对由 JS 侧保证
        width_pct: Option<u16>,
        height_pct: Option<u16>,
        /// 固定 px 尺寸(pct 优先)
        width_px: Option<u16>,
        height_px: Option<u16>,
    },
}

// TODO: share logic with Menu, it's essentially Popup(render_fn), but render fn needs to return
// a width/height hint. maybe Popup(Box<Component>)

pub struct Popup<T: Component> {
    contents: T,
    position: Option<Position>,
    area: Rect,
    position_bias: Open,
    scroll_half_pages: usize,
    auto_close: bool,
    ignore_escape_key: bool,
    id: &'static str,
    has_scrollbar: bool,
    layout: PopupLayout,
}

impl<T: Component> Popup<T> {
    pub fn new(id: &'static str, contents: T) -> Self {
        Self {
            contents,
            position: None,
            position_bias: Open::Below,
            area: Rect::new(0, 0, 0, 0),
            scroll_half_pages: 0,
            auto_close: false,
            ignore_escape_key: false,
            id,
            has_scrollbar: true,
            layout: PopupLayout::Anchor,
        }
    }

    /// 居中浮层模式:跳过锚点/内容自适应,按视口算固定居中矩形。
    /// 每维分辨率:width_pct/height_pct(视口百分比,优先)→ width_px/height_px(固定 px,
    /// 超视口钳到视口)→ 缺省 80%(约定:center 且无任何尺寸 = 80%×80%)。
    /// 边框在矩形内绘制(沿用 Anchor 的 border 绘制路径),不改变外框尺寸。
    pub fn floating(mut self, width: Option<u16>, height: Option<u16>, pct: Option<(u16, u16)>) -> Self {
        self.layout = PopupLayout::Float {
            width_pct: pct.map(|(w, _)| w),
            height_pct: pct.map(|(_, h)| h),
            width_px: width,
            height_px: height,
        };
        self
    }

    /// Set the anchor position next to which the popup should be drawn.
    ///
    /// Note that this is not the position of the top-left corner of the rendered popup itself,
    /// but rather the screen-space position of the information to which the popup refers.
    pub fn position(mut self, pos: Option<Position>) -> Self {
        self.position = pos;
        self
    }

    pub fn get_position(&self) -> Option<Position> {
        self.position
    }

    /// Set the popup to prefer to render above or below the anchor position.
    ///
    /// This preference will be ignored if the viewport doesn't have enough space in the
    /// chosen direction.
    pub fn position_bias(mut self, bias: Open) -> Self {
        self.position_bias = bias;
        self
    }

    pub fn auto_close(mut self, auto_close: bool) -> Self {
        self.auto_close = auto_close;
        self
    }

    /// Ignores an escape keypress event, letting the outer layer
    /// (usually the editor) handle it. This is useful for popups
    /// in insert mode like completion and signature help where
    /// the popup is closed on the mode change from insert to normal
    /// which is done with the escape key. Otherwise the popup consumes
    /// the escape key event and closes it, and an additional escape
    /// would be required to exit insert mode.
    pub fn ignore_escape_key(mut self, ignore: bool) -> Self {
        self.ignore_escape_key = ignore;
        self
    }

    pub fn scroll_half_page_down(&mut self) {
        self.scroll_half_pages += 1;
    }

    pub fn scroll_half_page_up(&mut self) {
        self.scroll_half_pages = self.scroll_half_pages.saturating_sub(1);
    }

    /// Toggles the Popup's scrollbar.
    /// Consider disabling the scrollbar in case the child
    /// already has its own.
    pub fn with_scrollbar(mut self, enable_scrollbar: bool) -> Self {
        self.has_scrollbar = enable_scrollbar;
        self
    }

    pub fn contents(&self) -> &T {
        &self.contents
    }

    pub fn contents_mut(&mut self) -> &mut T {
        &mut self.contents
    }

    pub fn area(&mut self, viewport: Rect, editor: &Editor) -> Rect {
        self.render_info(viewport, editor).area
    }

    fn render_info(&mut self, viewport: Rect, editor: &Editor) -> RenderInfo {
        match self.layout {
            PopupLayout::Anchor => self.render_info_anchor(viewport, editor),
            PopupLayout::Float {
                width_pct,
                height_pct,
                width_px,
                height_px,
            } => self.render_info_float(
                viewport,
                editor,
                width_pct,
                height_pct,
                width_px,
                height_px,
            ),
        }
    }

    /// 居中浮层几何:不读 cursor、不查内容 required_size,按视口算固定居中矩形。
    /// 每维:viewport*pct/100 → px(钳到视口)→ 缺省 80%;边框在框内,外框不变。
    fn render_info_float(
        &mut self,
        viewport: Rect,
        editor: &Editor,
        width_pct: Option<u16>,
        height_pct: Option<u16>,
        width_px: Option<u16>,
        height_px: Option<u16>,
    ) -> RenderInfo {
        let dim = |pct: Option<u16>, px: Option<u16>, total: u16| -> u16 {
            match (pct, px) {
                // 百分比:视口比例,整数除截断(120*78/100=93)
                (Some(p), _) => ((total as u32 * p as u32) / 100).min(total as u32) as u16,
                // 固定 px:超视口钳到视口
                (None, Some(x)) => x.min(total),
                // 约定:center 且该轴无尺寸 → 视口 80%(0 视口防御)
                (None, None) => (total as u32 * 80 / 100).max(1) as u16,
            }
        };
        let total_w = dim(width_pct, width_px, viewport.width);
        let total_h = dim(height_pct, height_px, viewport.height);
        // 边框沿用 Anchor 判定(popup_border 配置);太小不画;画在框内不占外框
        let render_borders = editor.popup_border() && total_w > 3 && total_h > 3;
        let border = u16::from(render_borders);
        let inner_h = total_h.saturating_sub(border * 2);
        // 居中:x=(vw-w)/2(不足时 saturating 防下溢)
        let area = Rect::new(
            viewport.width.saturating_sub(total_w) / 2,
            viewport.height.saturating_sub(total_h) / 2,
            total_w,
            total_h,
        );
        RenderInfo {
            area,
            // 内容区 = 框去边框;child_height 供外层滚动计算(浮层内容由 JS 侧 scroll 自理,
            // 固定视口内不产生外层滚动)
            child_height: inner_h,
            render_borders,
            is_menu: false,
        }
    }

    fn render_info_anchor(&mut self, viewport: Rect, editor: &Editor) -> RenderInfo {
        let mut position = editor.cursor().0.unwrap_or_default();
        if let Some(old_position) = self
            .position
            .filter(|old_position| old_position.row == position.row)
        {
            position = old_position;
        } else {
            self.position = Some(position);
        }

        let is_menu = self
            .contents
            .type_name()
            .starts_with("helix_term::ui::menu::Menu");

        let mut render_borders = if is_menu {
            editor.menu_border()
        } else {
            editor.popup_border()
        };

        // -- make sure frame doesn't stick out of bounds
        let mut rel_x = position.col as u16;
        let mut rel_y = position.row as u16;

        // if there's a orientation preference, use that
        // if we're on the top part of the screen, do below
        // if we're on the bottom part, do above
        let can_put_below = viewport.height > rel_y + MIN_HEIGHT;
        let can_put_above = rel_y.checked_sub(MIN_HEIGHT).is_some();
        let final_pos = match self.position_bias {
            Open::Below => match can_put_below {
                true => Open::Below,
                false => Open::Above,
            },
            Open::Above => match can_put_above {
                true => Open::Above,
                false => Open::Below,
            },
        };

        // compute maximum space available for child
        let mut max_height = match final_pos {
            Open::Above => rel_y,
            Open::Below => viewport.height.saturating_sub(1 + rel_y),
        };
        max_height = max_height.min(MAX_HEIGHT);
        let mut max_width = viewport.width.saturating_sub(2).min(MAX_WIDTH);
        render_borders = render_borders && max_height > 3 && max_width > 3;
        if render_borders {
            max_width -= 2;
            max_height -= 2;
        }

        // compute required child size and reclamp
        let (mut width, child_height) = self
            .contents
            .required_size((max_width, max_height))
            .expect("Component needs required_size implemented in order to be embedded in a popup");

        width = width.min(MAX_WIDTH);
        let height = if render_borders {
            (child_height + 2).min(MAX_HEIGHT)
        } else {
            child_height.min(MAX_HEIGHT)
        };
        if render_borders {
            width += 2;
        }
        if viewport.width <= rel_x + width + 2 {
            rel_x = viewport.width.saturating_sub(width + 2);
            width = viewport.width.saturating_sub(rel_x + 2)
        }

        let area = match final_pos {
            Open::Above => {
                rel_y = rel_y.saturating_sub(height);
                Rect::new(rel_x, rel_y, width, position.row as u16 - rel_y)
            }
            Open::Below => {
                rel_y += 1;
                let y_max = viewport.bottom().min(height + rel_y);
                Rect::new(rel_x, rel_y, width, y_max - rel_y)
            }
        };
        RenderInfo {
            area,
            child_height,
            render_borders,
            is_menu,
        }
    }

    fn handle_mouse_event(
        &mut self,
        &MouseEvent {
            kind,
            column: x,
            row: y,
            ..
        }: &MouseEvent,
    ) -> EventResult {
        if self.auto_close && matches!(kind, MouseEventKind::Down(_)) {
            let close_fn: Callback = Box::new(|compositor, _| {
                // remove the layer
                compositor.remove(self.id.as_ref());
            });

            return EventResult::Ignored(Some(close_fn));
        }

        let mouse_is_within_popup = x >= self.area.left()
            && x < self.area.right()
            && y >= self.area.top()
            && y < self.area.bottom();

        if !mouse_is_within_popup {
            return EventResult::Ignored(None);
        }

        match kind {
            MouseEventKind::ScrollDown if self.has_scrollbar => {
                self.scroll_half_page_down();
                EventResult::Consumed(None)
            }
            MouseEventKind::ScrollUp if self.has_scrollbar => {
                self.scroll_half_page_up();
                EventResult::Consumed(None)
            }
            _ => EventResult::Ignored(None),
        }
    }
}

impl<T: Component> Component for Popup<T> {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let key = match event {
            Event::Key(event) => *event,
            Event::Mouse(event) => return self.handle_mouse_event(event),
            Event::Resize(_, _) => {
                // TODO: calculate inner area, call component's handle_event with that area
                return EventResult::Ignored(None);
            }
            _ => return EventResult::Ignored(None),
        };

        if key!(Esc) == key && self.ignore_escape_key {
            return EventResult::Ignored(None);
        }

        let close_fn: Callback = Box::new(|compositor, _| {
            // remove the layer
            compositor.remove(self.id.as_ref());
        });

        // Code completion handles arrows and page up/down itself,
        // but code lens does not. First check whether content knows
        // about the key event. When not, check the default keys.
        match self.contents.handle_event(event, cx) {
            EventResult::Ignored(fn_once) => {
                match key {
                    // esc or ctrl-c aborts the completion and closes the menu
                    key!(Esc) | ctrl!('c') => {
                        let _ = self.contents.handle_event(event, cx);
                        EventResult::Consumed(Some(close_fn))
                    }
                    key!(PageDown) | ctrl!('d') => {
                        self.scroll_half_page_down();
                        EventResult::Consumed(None)
                    }
                    key!(PageUp) | ctrl!('u') => {
                        self.scroll_half_page_up();
                        EventResult::Consumed(None)
                    }
                    _ => {
                        // for some events, we want to process them but send ignore, specifically all input except
                        // tab/enter/ctrl-k or whatever will confirm the selection/ ctrl-n/ctrl-p for scroll.

                        if self.auto_close {
                            EventResult::Ignored(Some(close_fn))
                        } else {
                            EventResult::Ignored(fn_once)
                        }
                    }
                }
            }
            ev => ev,
        }
    }

    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let RenderInfo {
            area,
            child_height,
            render_borders,
            is_menu,
        } = self.render_info(viewport, cx.editor);
        self.area = area;

        // clear area
        let background = if is_menu {
            // TODO: consistently style menu
            cx.editor
                .theme
                .try_get("ui.menu")
                .unwrap_or_else(|| cx.editor.theme.get("ui.text"))
        } else {
            cx.editor.theme.get("ui.popup")
        };
        surface.clear_with(area, background);

        let mut inner = area;
        if render_borders {
            inner = area.inner(Margin::all(1));
            Widget::render(Block::bordered(), area, surface);
        }
        let border = usize::from(render_borders);

        let max_offset = child_height.saturating_sub(inner.height) as usize;
        let half_page_size = (inner.height / 2) as usize;
        let scroll = max_offset.min(self.scroll_half_pages * half_page_size);
        self.scroll_half_pages = scroll
            .checked_div(half_page_size)
            .unwrap_or(self.scroll_half_pages);
        cx.scroll = Some(scroll);
        self.contents.render(inner, surface, cx);

        // render scrollbar if contents do not fit
        if self.has_scrollbar {
            let win_height = inner.height as usize;
            let len = child_height as usize;
            let fits = len <= win_height;
            let scroll_style = cx.editor.theme.get("ui.menu.scroll");

            if !fits {
                let scroll_height = win_height.pow(2).div_ceil(len).min(win_height);
                let scroll_line = (win_height - scroll_height) * scroll
                    / std::cmp::max(1, len.saturating_sub(win_height));

                let mut cell;
                for i in 0..win_height {
                    cell =
                        &mut surface[(inner.right() - 1 + border as u16, inner.top() + i as u16)];

                    let half_block = if render_borders { "▌" } else { "▐" };

                    if scroll_line <= i && i < scroll_line + scroll_height {
                        // Draw scroll thumb
                        cell.set_symbol(half_block);
                        cell.set_fg(scroll_style.fg.unwrap_or(helix_view::theme::Color::Reset));
                    } else if !render_borders {
                        // Draw scroll track
                        cell.set_symbol(half_block);
                        cell.set_fg(scroll_style.bg.unwrap_or(helix_view::theme::Color::Reset));
                    }
                }
            }
        }
    }

    fn id(&self) -> Option<&'static str> {
        Some(self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_swap::{access::Map, ArcSwap};
    use helix_core::config::default_lang_loader;
    use helix_view::handlers::{completion::CompletionHandler, word_index};
    use helix_view::theme;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    /// 无内容桩组件:浮层几何只算外框/居中,不触内容渲染
    struct Noop;
    impl Component for Noop {
        fn render(&mut self, _area: Rect, _surface: &mut Surface, _cx: &mut Context) {}
    }

    /// 测试 Editor:仿 helix-view editor.rs tests::test_editor——channel 直建 Handlers
    /// (只 spawn word_index),不注册任何 hook。cursor 空树 → None,anchor 分支不受影响。
    fn test_editor(popup_border: helix_view::editor::PopupBorderConfig) -> Editor {
        let theme_loader = Arc::new(theme::Loader::new(&[]));
        let syn_loader = Arc::new(ArcSwap::from_pointee(default_lang_loader()));
        let mut cfg = helix_view::editor::Config::default();
        cfg.popup_border = popup_border;
        let config = Arc::new(ArcSwap::from_pointee(cfg));
        let (completions_tx, _) = mpsc::channel(32);
        let (signature_hints, _) = mpsc::channel(32);
        let (auto_save, _) = mpsc::channel(32);
        let (document_colors, _) = mpsc::channel(32);
        let (document_links, _) = mpsc::channel(32);
        let (pull_diagnostics, _) = mpsc::channel(32);
        let (pull_all_documents_diagnostics, _) = mpsc::channel(32);
        let (code_action_hint, _) = mpsc::channel(32);
        let handlers = helix_view::handlers::Handlers {
            completions: CompletionHandler::new(completions_tx),
            signature_hints,
            auto_save,
            document_colors,
            document_links,
            word_index: word_index::Handler::spawn(),
            pull_diagnostics,
            pull_all_documents_diagnostics,
            code_action_hint,
        };
        Editor::new(
            Rect::new(0, 0, 120, 40),
            theme_loader,
            syn_loader,
            Arc::new(Map::new(config, |c: &helix_view::editor::Config| c)),
            handlers,
            helix_loader::workspace_trust::WorkspaceTrust::fully_trusted(),
        )
    }

    fn float_popup(w: Option<u16>, h: Option<u16>, pct: Option<(u16, u16)>) -> Popup<Noop> {
        Popup::new("test-layer", Noop).floating(w, h, pct)
    }

    /// 浮层 78%×75% 在 120×40 视口 → 居中矩形:
    /// 宽 = 120*78/100 = 93(整数除),高 = 40*75/100 = 30;x = (120-93)/2 = 13,y = (40-30)/2 = 5
    #[tokio::test(flavor = "multi_thread")]
    async fn float_pct_area_centered_and_proportional() {
        let editor = test_editor(helix_view::editor::PopupBorderConfig::None);
        let mut p = float_popup(None, None, Some((78, 75)));
        let viewport = Rect::new(0, 0, 120, 40);
        assert_eq!(p.area(viewport, &editor), Rect::new(13, 5, 93, 30));
    }

    /// center 且无任何尺寸 → 约定 80%×80%:120*80/100=96,40*80/100=32;x=(120-96)/2=12,y=4
    #[tokio::test(flavor = "multi_thread")]
    async fn float_no_sizes_defaults_80pct() {
        let editor = test_editor(helix_view::editor::PopupBorderConfig::None);
        let mut p = float_popup(None, None, None);
        let viewport = Rect::new(0, 0, 120, 40);
        assert_eq!(p.area(viewport, &editor), Rect::new(12, 4, 96, 32));
    }

    /// px 尺寸超视口 → 钳到视口(0,0 满屏);单轴 px + 另一轴缺省 → 缺省轴 80%
    #[tokio::test(flavor = "multi_thread")]
    async fn float_px_clamps_and_defaults_missing_axis() {
        let editor = test_editor(helix_view::editor::PopupBorderConfig::None);
        let viewport = Rect::new(0, 0, 120, 40);
        let mut p = float_popup(Some(300), Some(200), None);
        assert_eq!(p.area(viewport, &editor), Rect::new(0, 0, 120, 40));
        let mut p = float_popup(Some(30), None, None);
        assert_eq!(p.area(viewport, &editor), Rect::new(45, 4, 30, 32));
    }

    /// 边框在浮层框内绘制(不改变外框):popup_border=All 时外框仍与无边框一致
    #[tokio::test(flavor = "multi_thread")]
    async fn float_borders_stay_inside_box() {
        let editor = test_editor(helix_view::editor::PopupBorderConfig::All);
        let mut p = float_popup(None, None, Some((78, 75)));
        let viewport = Rect::new(0, 0, 120, 40);
        assert_eq!(p.area(viewport, &editor), Rect::new(13, 5, 93, 30));
    }
}
