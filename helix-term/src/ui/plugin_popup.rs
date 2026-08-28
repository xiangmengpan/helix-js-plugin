use crate::commands::typed::{apply_cursor_requests, apply_plugin_edits};
use crate::compositor::{Component, Compositor, Context, Event, EventResult};
use crate::ui::comp_layout;
use helix_js::{CommandContext, PluginKey, PopupKeyResult, StyledLine};
use helix_view::current_ref;
use helix_view::graphics::Rect;
use helix_view::input::KeyEvent;
use helix_view::keyboard::{KeyCode, KeyModifiers};
use tui::buffer::Buffer as Surface;

/// JS 插件弹窗的内容组件：内容由 JS `render` 回调绘制，按键由 JS `onKey` 回调处理。
/// 作为 `ui::Popup` 的内容使用；图层弹出/边框/滚动由外层 Popup 负责。
pub struct PluginPopup {
    id: u64,
    lines: Vec<StyledLine>,
    /// open_popup 的 width/height 尺寸上限（两者都提供时才生效）
    size_hint: Option<(u16, u16)>,
    /// 当前焦点节点 id（Tab/Shift-Tab 在可聚焦节点间移动；None = 无节点焦点）
    focus: Option<String>,
    /// 可聚焦节点 id 列表（树序，渲染时刷新）
    focusables: Vec<String>,
    /// 脏格 diff 渲染器（只重绘变化格）
    diff: crate::ui::comp_layout::DiffRenderer,
}

impl PluginPopup {
    pub fn new(id: u64, size_hint: Option<(u16, u16)>) -> Self {
        Self {
            id,
            lines: Vec::new(),
            size_hint,
            focus: None,
            focusables: Vec::new(),
            diff: Default::default(),
        }
    }

    /// 清空脏格 diff 状态（测试向不同 surface 渲染时需要重置）
    pub fn reset_render_state(&mut self) {
        self.diff = Default::default();
    }

    /// 测试/调试访问器：当前布局行
    pub fn lines(&self) -> &[StyledLine] {
        &self.lines
    }

    /// 调 JS render 并布局成行：Lines 原样；Tree 走 comp_layout（viewport 约束）。
    fn refresh(&mut self, viewport: (u16, u16)) {
        match helix_js::render_popup(self.id, viewport.0, viewport.1, self.focus.as_deref()) {
            Ok(content) => {
                // 收集可聚焦节点（Button/Input 的 id，树序）
                self.focusables.clear();
                if let helix_js::Content::Tree(node) = &content {
                    helix_js::focusable_node_ids(node, &mut self.focusables);
                }
                self.lines = comp_layout::render(content, viewport);
            }
            Err(err) => self.lines = vec![StyledLine::plain(format!("<plugin popup error: {err}>"))],
        }
    }
}

impl Component for PluginPopup {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let Event::Key(key_event) = event else {
            return EventResult::Ignored(None);
        };
        let Some(key) = key_to_plugin_key(key_event) else {
            return EventResult::Ignored(None);
        };
        // 节点焦点路由（方案乙）：Tab 移动焦点；焦点在 button/input 时按键直接路由，
        // 不经过弹窗级 onKey。
        if !self.focusables.is_empty() {
            if key.name == "Tab" {
                let idx = self.focusables.iter().position(|f| Some(f) == self.focus.as_ref());
                let next = idx.map(|i| (i + 1) % self.focusables.len()).unwrap_or(0);
                self.focus = Some(self.focusables[next].clone());
                return EventResult::Consumed(None);
            }
            if let Some(fid) = self.focus.as_ref() {
                let drain_msgs = |cx: &mut Context| {
                    let msgs = helix_js::take_messages();
                    if !msgs.is_empty() {
                        cx.editor.set_status(msgs.join(" "));
                    }
                    // 节点事件（button onPress 等）里发起的 UI 请求同样即时应用
                    if let Err(err) =
                        crate::commands::typed::apply_ui_requests(helix_js::take_ui_requests())
                    {
                        cx.editor.set_error(err.to_string());
                    }
                };
                match key.name.as_str() {
                    // Enter：input 有状态 → onKey("Enter")；否则（button）→ onPress
                    // （空格键是单字符，走下方编辑臂插入输入框；key_to_plugin_key 无 "Space" 键名）
                    "Enter" => {
                        let event = if helix_js::input_has_state(self.id, fid) {
                            Some(key.name.as_str())
                        } else {
                            None
                        };
                        if helix_js::dispatch_node_event(self.id, fid, event).is_ok() {
                            drain_msgs(cx);
                            return EventResult::Consumed(None);
                        }
                    }
                    // 水平方向键/Home/End：input 光标移动（input_edit 对这些键返回 None，不触发 onChange）
                    "Left" | "Right" | "Home" | "End" => {
                        if helix_js::input_has_state(self.id, fid)
                            && helix_js::dispatch_input_key(self.id, fid, &key.name).is_ok()
                        {
                            drain_msgs(cx);
                            return EventResult::Consumed(None);
                        }
                    }
                    // Up/Down：候选导航（走 onKey）
                    "Up" | "Down" => {
                        if helix_js::input_has_state(self.id, fid)
                            && helix_js::dispatch_node_event(self.id, fid, Some(&key.name)).is_ok()
                        {
                            drain_msgs(cx);
                            return EventResult::Consumed(None);
                        }
                    }
                    // 编辑键：input → dispatch_input_key（改值 + onChange）；否则（button）→ onKey
                    key_name if key_name.chars().count() == 1
                        || key_name == "Backspace"
                        || key_name == "Delete" =>
                    {
                        if helix_js::input_has_state(self.id, fid) {
                            if helix_js::dispatch_input_key(self.id, fid, &key.name).is_ok() {
                                drain_msgs(cx);
                                return EventResult::Consumed(None);
                            }
                        } else if helix_js::dispatch_node_event(self.id, fid, Some(&key.name)).is_ok() {
                            drain_msgs(cx);
                            return EventResult::Consumed(None);
                        }
                    }
                    _ => {}
                }
            }
        }
        // 构建当前文档快照（弹窗是模态层，打开期间文档不变；每次按键重新序列化）
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
                cx.editor.set_error(format!("plugin popup cursor failed: {err}"));
            }
        }
        let edits = helix_js::take_edits();
        if !edits.is_empty() {
            if let Err(err) = apply_plugin_edits(cx.editor, &edits) {
                cx.editor.set_error(format!("plugin popup edit failed: {err}"));
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
                // 先通知 JS（触发 onClose，echo 消息入队），再弹掉图层，最后把
                // echo 消息刷成状态栏（与命令路径取消息的约定一致）。
                EventResult::Consumed(Some(Box::new(
                    move |compositor: &mut Compositor, cx: &mut Context| {
                        let _ = helix_js::close_popup(id);
                        // ponytail: pop() 假定弹窗层在栈顶；若未来有叠加图层场景，改用 compositor.remove("plugin-popup")
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

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        self.refresh((area.width, area.height));
        // 脏格 diff 渲染：只重绘变化格（方案乙③）
        self.diff.render(&self.lines, area, surface, &cx.editor.theme);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        // 首帧 lines 为空时预布局（树内容需真实 viewport 才能算出尺寸；
        // 否则 Popup 先按 required_size 得到 0×0，树布局永远被裁成空）
        if self.lines.is_empty() {
            self.refresh(viewport);
        }
        let width = self
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.text.chars().count() as u16).sum())
            .max()
            .unwrap_or(0)
            .min(viewport.0);
        let height = self.lines.len() as u16;
        let (width, height) = match self.size_hint {
            Some((w, h)) => (width.min(w), height.min(h)),
            None => (width, height),
        };
        Some((width, height))
    }
}

/// 把编辑器按键转成 JS onKey 可见的快照；无法表示的键返回 None（不转交 JS）。
/// 本树 KeyEvent 只有 code+modifiers（无 kind，termina 已过滤 Release），
// ponytail: 若 KeyEvent 将来增加 kind 字段，需在此过滤 KeyEventKind::Press。
/// KeyCode 无 BackTab，故与简报实现相比删去这两处分支。
pub(crate) fn key_to_plugin_key(key: &KeyEvent) -> Option<PluginKey> {
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
        KeyCode::Insert => "Insert".into(),
        KeyCode::F(n) => format!("F{n}"),
        _ => return None,
    };
    Some(PluginKey {
        name,
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::Component;

    #[test]
    fn required_size_clamps_to_size_hint() {
        let mut p = PluginPopup::new(1, Some((10, 4)));
        p.lines = vec![StyledLine::plain("W".repeat(20)); 10];
        assert_eq!(p.required_size((120, 30)), Some((10, 4)));
    }

    #[test]
    fn required_size_without_hint_uses_content() {
        let mut p = PluginPopup::new(1, None);
        p.lines = vec![
            StyledLine::plain("abc"),
            StyledLine::plain("a"),
        ];
        assert_eq!(p.required_size((120, 30)), Some((3, 2)));
    }

    #[test]
    fn required_size_never_exceeds_viewport_width() {
        let mut p = PluginPopup::new(1, None);
        p.lines = vec![StyledLine::plain("x".repeat(200))];
        assert_eq!(p.required_size((50, 30)), Some((50, 1)));
    }
}
