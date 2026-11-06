use crate::compositor::{Component, Compositor, Context, Event, EventResult};
use helix_js::{PluginKey, PopupKeyResult, StyledLine};
use helix_view::graphics::Rect;
use helix_view::input::KeyEvent;
use helix_view::keyboard::{KeyCode, KeyModifiers};
use tui::buffer::Buffer as Surface;

/// JS 插件弹窗的内容组件：内容由 JS `render` 回调绘制，按键由 JS `onKey` 回调处理。
/// 作为 `ui::Popup` 的内容使用；图层弹出/边框/滚动由外层 Popup 负责。
pub struct PluginPopup {
    /// 共享骨架:焦点/可聚焦节点/行缓存/脏格 diff(见 `ui::js_surface`)
    surface: crate::ui::js_surface::JsSurface,
    /// open_popup 的 width/height 尺寸上限(两者都提供时才生效)
    size_hint: Option<(u16, u16)>,
    /// 本弹窗所在图层名(open_popup layer;None = 直构/单测,Close 时回退 pop)
    layer: Option<&'static str>,
    /// 绑定的键位作用域（B 阶段：状态栏徽标与 Esc 兜底）—— 由 `OpenPopupReq.scope` 带下来，
    /// **不**在渲染期反查宿主（多宿主下会查错表，实测踩过）
    scope: Option<String>,
    /// **核心弹窗**（`helix.dialog.*`）：布局期跳过 JS 预渲染（render 是 undefined）
    native: bool,
}

/// 弹窗的共享层差异:恒收集可聚焦节点;**不做**焦点失效重置(与面板不同,保持原行为)。
const OPTS: crate::ui::js_surface::SurfaceOpts = crate::ui::js_surface::SurfaceOpts {
    collect_focusables: true,
    reset_missing_focus: false,
    esc_releases_focus: false,
    // 弹窗：第一个 input/button 自动拿焦点 ⇒ `el("input")` 开箱能用（不然要先按 Tab）
    auto_focus_first: true,
    label: "popup",
};

impl PluginPopup {
    pub fn new(id: u64, size_hint: Option<(u16, u16)>) -> Self {
        Self {
            surface: crate::ui::js_surface::JsSurface::new(id),
            size_hint,
            scope: None,
            native: false,
            layer: None,
        }
    }

    /// 绑定键位作用域（`open_popup({scope})`）
    pub fn with_scope(mut self, scope: Option<String>) -> Self {
        self.scope = scope;
        self
    }

    /// 记录本弹窗图层名:JS onKey 返回 "close" 时按层名移除自身
    /// (异层并存场景顶层不一定是本弹窗,pop 会误伤)。
    pub fn with_layer(mut self, layer: &'static str) -> Self {
        self.layer = Some(layer);
        self
    }

    /// 标记为**核心弹窗**（`helix.dialog.*`）：布局期不预渲染 JS render。
    pub fn with_native(mut self, native: bool) -> Self {
        self.native = native;
        self
    }

    /// 弹层实例 id(`open_popup` 返回的那个)—— compositor 按它移除层。
    pub(crate) fn id(&self) -> u64 {
        self.surface.id
    }

    /// 核心弹窗的 Esc/Enter：调回调 → 关层（与 `PopupKeyResult::Close` 同一路径）。
    fn native_dialog_key(
        &mut self,
        key: &helix_js::PluginKey,
        cx: &mut Context,
    ) -> Option<EventResult> {
        let id = self.surface.id;
        let (action, submit, cancel) = cx
            .with_js(|h| helix_js::native_dialog_key(h.state(), id, &key.name))
            .flatten()?;
        // 列表里移动游标/勾选：只消费按键，下一帧重绘（不关层）
        if matches!(action, helix_js::DialogKey::Nav) {
            helix_event::request_redraw();
            return Some(EventResult::Consumed(None));
        }
        let cb = match &action {
            helix_js::DialogKey::Cancel => cancel,
            _ => submit,
        };
        if let Some(cb) = cb {
            let res = match &action {
                helix_js::DialogKey::Submit(v) => helix_js::dialog::call_dialog_callback(&cb, v),
                helix_js::DialogKey::SubmitMany(vs) => {
                    helix_js::dialog::call_dialog_callback_values(&cb, vs)
                }
                // 取消：回调不带参数（插件侧签名是 `on_cancel: () => …`）
                helix_js::DialogKey::Cancel => helix_js::dialog::call_dialog_callback(&cb, ""),
                helix_js::DialogKey::Nav => Ok(()),
            };
            if let Err(e) = res {
                cx.editor.set_error(e);
            }
        }
        // 与 onKey 返回 "close" 完全一致：先通知 JS，再移除层，最后刷状态栏
        let layer = self.layer;
        Some(EventResult::Consumed(Some(Box::new(
            move |compositor: &mut Compositor, cx: &mut Context| {
                let _ = helix_js::close_popup(id);
                if let Some(layer) = layer {
                    compositor.remove(layer);
                } else {
                    compositor.pop();
                }
                let msgs = cx
                    .with_js(|h| helix_js::take_messages_in(h.state()))
                    .unwrap_or_else(helix_js::take_messages);
                if !msgs.is_empty() {
                    cx.editor.set_status(msgs.join(" "));
                }
            },
        ))))
    }

    /// 清空脏格 diff 状态(测试向不同 surface 渲染时需要重置)
    pub fn reset_render_state(&mut self) {
        self.surface.reset_render_state();
    }

    /// 测试/调试访问器:当前布局行
    pub fn lines(&self) -> &[StyledLine] {
        self.surface.lines()
    }

    /// 无 `Context` 的预布局路径(`Component::required_size` 在布局期被调用)。
    fn refresh(&mut self, viewport: (u16, u16)) {
        // 布局期的预渲染（`required_size` 拿不到 Context）：核心弹窗的尺寸来自
        // `width`/`height`，由 `with_native` 标记后**跳过**预渲染 —— JS render 对它是
        // undefined，走那条路只会渲染出一行错误。
        if self.native {
            return;
        }
        self.surface.refresh_global(viewport, OPTS);
    }
}

impl Component for PluginPopup {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        // 粘贴:焦点在 input → 批量插入整段 + 一次 onChange;否则忽略(冒泡给编辑器正文粘贴)
        if let Event::Paste(contents) = event {
            return self
                .surface
                .paste(contents, cx, OPTS)
                .unwrap_or(EventResult::Ignored(None));
        }
        let Event::Key(key_event) = event else {
            return EventResult::Ignored(None);
        };
        let Some(key) = key_to_plugin_key(key_event) else {
            return EventResult::Ignored(None);
        };
        // 节点焦点路由:Tab 移动焦点;焦点在 button/input 时按键直接路由,不经过弹窗级 onKey。
        if key.name == "Tab" && self.surface.tab_focus(key.shift) {
            return EventResult::Consumed(None);
        }
        // 核心弹窗：Esc/Enter 在这里判（**必须在节点路由之前** —— 焦点在单行 input 时
        // 核心会把 Enter 交给该节点的 onKey，弹窗的 onKey 根本收不到）。
        if let Some(result) = self.native_dialog_key(&key, cx) {
            return result;
        }
        if !self.surface.focusables.is_empty() {
            if let Some(result) = self.surface.node_key(&key, cx, OPTS) {
                return result;
            }
        }
        // 构建当前文档快照(弹窗是模态层,打开期间文档不变;每次按键重新序列化)
        let ctx = crate::ui::js_surface::JsSurface::command_context(cx);
        let result = cx.js_popup_key(self.surface.id, &key, &ctx);
        // 应用编辑/光标/消息(在 popup_key 之后:onKey 入队的编辑在本次按键内同步应用)
        crate::ui::js_surface::JsSurface::drain_after_onkey(cx, OPTS);
        match result {
            Ok(PopupKeyResult::Close) => {
                let id = self.surface.id;
                let layer = self.layer;
                // 先通知 JS(触发 onClose,echo 消息入队),再弹掉图层,最后把
                // echo 消息刷成状态栏(与命令路径取消息的约定一致)。
                EventResult::Consumed(Some(Box::new(
                    move |compositor: &mut Compositor, cx: &mut Context| {
                        let _ = helix_js::close_popup(id);
                        // 按 layer 名移除自身:同层替换/异层并存下顶层不一定是本弹窗;
                        // 直构(单测)无层名时回退 pop。
                        if let Some(layer) = layer {
                            compositor.remove(layer);
                        } else {
                            compositor.pop();
                        }
                        let msgs = helix_js::take_messages();
                        if !msgs.is_empty() {
                            cx.editor.set_status(msgs.join(" "));
                        }
                    },
                )))
            }
            Ok(PopupKeyResult::Handled) => EventResult::Consumed(None),
            // 穿透给编辑器;Err 仅发生在 JS 侧异常时,同样放行
            Ok(PopupKeyResult::Ignored) | Err(_) => EventResult::Ignored(None),
        }
    }

    fn key_scope(&self) -> Option<String> {
        self.scope.clone()
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let id = self.surface.id;
        // 核心弹窗（`helix.dialog.*`）：内容由核心给，跳过 JS render
        let native = cx
            .with_js(|h| helix_js::native_dialog_content(h.state(), id))
            .unwrap_or_else(|| helix_js::native_dialog_content_env(id));
        match native {
            Some(content) => self
                .surface
                .refresh_native(content, (area.width, area.height), OPTS),
            None => self.surface.refresh(cx, (area.width, area.height), OPTS),
        }
        // 脏格 diff 渲染:只重绘变化格
        self.surface.render_lines(area, surface, &cx.editor.theme);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        // 首帧 lines 为空时预布局(树内容需真实 viewport 才能算出尺寸;
        // 否则 Popup 先按 required_size 得到 0×0,树布局永远被裁成空)
        if self.surface.lines.is_empty() {
            self.refresh(viewport);
        }
        let width = self
            .surface
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|sp| sp.text.chars().count() as u16)
                    .sum()
            })
            .max()
            .unwrap_or(0)
            .min(viewport.0);
        let height = self.surface.lines.len() as u16;
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
// 注：不存在 `KeyCode::BackTab` —— `input.rs` 把 termina 的 BackTab 归一化成
// `Tab + SHIFT`，所以 Shift-Tab 在 JS 侧就是 `{name:"Tab", shift:true}`。
pub fn key_to_plugin_key(key: &KeyEvent) -> Option<PluginKey> {
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
        p.surface.lines = vec![StyledLine::plain("W".repeat(20)); 10];
        assert_eq!(p.required_size((120, 30)), Some((10, 4)));
    }

    #[test]
    fn required_size_without_hint_uses_content() {
        let mut p = PluginPopup::new(1, None);
        p.surface.lines = vec![StyledLine::plain("abc"), StyledLine::plain("a")];
        assert_eq!(p.required_size((120, 30)), Some((3, 2)));
    }

    #[test]
    fn required_size_never_exceeds_viewport_width() {
        let mut p = PluginPopup::new(1, None);
        p.surface.lines = vec![StyledLine::plain("x".repeat(200))];
        assert_eq!(p.required_size((50, 30)), Some((50, 1)));
    }
}
