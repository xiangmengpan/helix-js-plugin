use crate::compositor::{Component, Compositor, Context, Event, EventResult};
use helix_js::{PluginKey, PopupKeyResult};
use helix_view::graphics::Rect;
use helix_view::input::KeyEvent;
use helix_view::keyboard::{KeyCode, KeyModifiers};
use tui::buffer::Buffer as Surface;
use tui::text::Text as TuiText;
use tui::widgets::{Paragraph, Widget, Wrap};

/// JS 插件弹窗的内容组件：内容由 JS `render` 回调绘制，按键由 JS `onKey` 回调处理。
/// 作为 `ui::Popup` 的内容使用；图层弹出/边框/滚动由外层 Popup 负责。
pub struct PluginPopup {
    id: u64,
    lines: Vec<String>,
}

impl PluginPopup {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            lines: Vec::new(),
        }
    }
}

impl Component for PluginPopup {
    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        let Event::Key(key_event) = event else {
            return EventResult::Ignored(None);
        };
        let Some(key) = key_to_plugin_key(key_event) else {
            return EventResult::Ignored(None);
        };
        match helix_js::popup_key(self.id, &key) {
            Ok(PopupKeyResult::Close) => {
                let id = self.id;
                // 先通知 JS（触发 onClose，echo 消息入队），再弹掉图层，最后把
                // echo 消息刷成状态栏（与命令路径取消息的约定一致）。
                EventResult::Consumed(Some(Box::new(
                    move |compositor: &mut Compositor, cx: &mut Context| {
                        let _ = helix_js::close_popup(id);
                        compositor.pop();
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

    fn render(&mut self, area: Rect, surface: &mut Surface, _cx: &mut Context) {
        match helix_js::render_popup(self.id, area.width, area.height) {
            Ok(lines) => self.lines = lines,
            Err(err) => self.lines = vec![format!("<plugin popup error: {err}>")],
        }
        let text = TuiText::from(self.lines.join("\n"));
        let par = Paragraph::new(&text).wrap(Wrap { trim: false });
        par.render(area, surface);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        let width = self
            .lines
            .iter()
            .map(|l| l.chars().count() as u16)
            .max()
            .unwrap_or(0)
            .min(viewport.0);
        let height = self.lines.len() as u16;
        Some((width, height))
    }
}

/// 把编辑器按键转成 JS onKey 可见的快照；无法表示的键返回 None（不转交 JS）。
/// 本树 KeyEvent 只有 code+modifiers（无 kind，termina 已过滤 Release），
/// KeyCode 无 BackTab，故与简报实现相比删去这两处分支。
fn key_to_plugin_key(key: &KeyEvent) -> Option<PluginKey> {
    let name = match key.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Delete => "Delete".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PageUp".into(),
        KeyCode::PageDown => "PageDown".into(),
        _ => return None,
    };
    Some(PluginKey {
        name,
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
    })
}
