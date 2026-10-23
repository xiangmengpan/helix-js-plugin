//! `JsSurface` —— JS 组件(面板/弹窗)的共享骨架(G10)。
//!
//! 为什么有这层:`PluginPanel`(369 行)与 `PluginPopup`(401 行)有 73 个 20 行窗口是重复的
//! ——「粘贴 → 节点焦点路由 → 文档快照 → 调 JS onKey → drain 三类请求」这段骨架两边各写一遍,
//! 改一个键要改两处。这里把**骨架**收进来,把**语义**留在各自容器:
//!
//! - 收进来:状态(`id/focus/focusables/lines/diff`)、刷新渲染、Tab 焦点循环、
//!   节点按键路由(Enter/方向键/编辑键)、paste、drain(消息 + 游标/编辑/装饰 + UI 请求);
//! - 留在容器:**Esc 语义**(面板 = 取消焦点;弹窗 = 关闭)、关闭/自愈动作、`required_size`、
//!   面板的 `focusable` 开关、弹窗的 layer 语义。
//!
//! 设计约束:**这一层不实现 `Component`**,也不写 `if 是面板 / 是弹窗` —— 差异一律由
//! `SurfaceOpts` 或调用方显式传入。这样加第四个 JS 组件时,复制的是"调用顺序",不是 200 行。

use helix_view::graphics::Rect;

use crate::commands::typed::{
    apply_cursor_requests, apply_plugin_decorations, apply_plugin_edits, apply_ui_requests,
};
use crate::compositor::{Context, EventResult};
use crate::ui::comp_layout::{self, DiffRenderer};
use helix_js::{CommandContext, PluginKey, StyledLine};
use helix_view::theme::Theme;
use tui::buffer::Buffer as Surface;

/// 共享层的运行时差异(由容器提供,避免在这里写类型判断)。
#[derive(Clone, Copy)]
pub(crate) struct SurfaceOpts {
    /// 是否提取可聚焦节点。面板只在 `focusable` 时提取(未启用 = 零行为变化)。
    pub collect_focusables: bool,
    /// 焦点节点在新树里消失时是否清空焦点。面板要,弹窗不要(弹窗 focusables 恒在收集)。
    pub reset_missing_focus: bool,
    /// 焦点在节点时按 Esc 是否「取消焦点」而不是交给 onKey。
    /// 面板:是(面板非模态,Esc 只退出节点焦点);弹窗:否(Esc 走 onKey 默认关闭)。
    pub esc_releases_focus: bool,
    /// 错误文案里的组件名(`"panel"` / `"popup"`)。
    pub label: &'static str,
}

impl SurfaceOpts {
    fn error_line(&self, err: &str) -> StyledLine {
        StyledLine::plain(format!("<plugin {} error: {err}>", self.label))
    }
}

pub(crate) struct JsSurface {
    pub id: u64,
    /// 当前焦点节点 id(`None` = 无节点焦点,按键走容器级 onKey)
    pub focus: Option<String>,
    /// 可聚焦节点 id 列表(树序,渲染时刷新)
    pub focusables: Vec<String>,
    /// 最近一次布局出的行(按 viewport 裁好)
    pub lines: Vec<StyledLine>,
    /// 脏格 diff 渲染器(只重绘变化格)
    diff: DiffRenderer,
}

impl JsSurface {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            focus: None,
            focusables: Vec::new(),
            lines: Vec::new(),
            diff: Default::default(),
        }
    }

    /// 清空脏格 diff 状态(测试向不同 surface 渲染时需要重置)
    pub fn reset_render_state(&mut self) {
        self.diff = Default::default();
    }

    /// 测试/诊断访问器:当前布局行
    pub fn lines(&self) -> &[StyledLine] {
        &self.lines
    }

    /// 用**当前宿主**刷新(有 `Context` 的路径:渲染)。
    pub fn refresh(&mut self, cx: &mut Context, viewport: (u16, u16), opts: SurfaceOpts) {
        let rendered = cx.js_render_popup(self.id, viewport.0, viewport.1, self.focus.as_deref());
        self.apply_content(rendered.map_err(|e| e.to_string()), viewport, opts);
    }

    /// 用**环境宿主**刷新(无 `Context` 的路径:`Component::required_size` 在布局期被调用,
    /// 拿不到 `&mut Context`,只能走全局解析)。
    pub fn refresh_global(&mut self, viewport: (u16, u16), opts: SurfaceOpts) {
        // L4-④:布局期没有 `&mut Context` ⇒ 用**过渡捕获入口**(无宿主时先 `init`,
        // 与旧行为一致 —— 以前 `render_popup` 自己会 `init()` 装一个默认宿主)。
        let st = helix_js::host::state_handle_or_init();
        let rendered =
            helix_js::render_popup(&st, self.id, viewport.0, viewport.1, self.focus.as_deref());
        self.apply_content(rendered.map_err(|e| e.to_string()), viewport, opts);
    }

    fn apply_content(
        &mut self,
        rendered: Result<helix_js::Content, String>,
        viewport: (u16, u16),
        opts: SurfaceOpts,
    ) {
        match rendered {
            Ok(content) => {
                if opts.collect_focusables {
                    self.focusables.clear();
                    if let helix_js::Content::Tree(node) = &content {
                        helix_js::focusable_node_ids(node, &mut self.focusables);
                    }
                    // 焦点失效重置:焦点节点不在新列表 → 清空。content 非 Tree 时列表为空 ⇒ 同样清空。
                    // 先传旧 focus 供 JS 样式,提取后再重置。
                    if opts.reset_missing_focus {
                        if let Some(f) = &self.focus {
                            if !self.focusables.contains(f) {
                                self.focus = None;
                            }
                        }
                    }
                }
                self.lines = comp_layout::render(content, viewport)
            }
            Err(err) => self.lines = vec![opts.error_line(&err)],
        }
    }

    /// 把 `lines` 画到 area(脏格 diff:只重绘变化格)
    pub fn render_lines(&mut self, area: Rect, surface: &mut Surface, theme: &Theme) {
        self.diff.render(&self.lines, area, surface, theme);
    }

    /// Tab/Shift-Tab:焦点在可聚焦节点间循环(`back` = 反向)。
    /// 返回是否真的移动了(`false` = 没有可聚焦节点,调用方应把键让给 onKey)。
    ///
    /// 说明:`Shift-Tab` 在键码层**没有**独立变体 —— `helix-view/src/input.rs`
    /// 把 `termina::KeyCode::BackTab` 归一化成 `Tab + SHIFT`,所以方向只能靠
    /// `PluginKey.shift` 传进来。
    pub fn tab_focus(&mut self, back: bool) -> bool {
        if self.focusables.is_empty() {
            return false;
        }
        let n = self.focusables.len();
        let idx = self
            .focusables
            .iter()
            .position(|f| Some(f) == self.focus.as_ref());
        let next = match (idx, back) {
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
            (None, false) => 0,
            (None, true) => n - 1,
        };
        self.focus = Some(self.focusables[next].clone());
        true
    }

    /// 粘贴:焦点在 input → 批量插入整段 + 一次 onChange;否则 `None`(冒泡给编辑器正文粘贴)。
    ///
    /// 注意:input 状态已变更后才调 onChange —— 失败绝不能冒泡(否则编辑器正文再插一份)。
    pub fn paste(
        &mut self,
        contents: &str,
        cx: &mut Context,
        opts: SurfaceOpts,
    ) -> Option<EventResult> {
        let fid = self.focus.as_ref()?;
        if !helix_js::input_has_state(self.id, fid) {
            return None;
        }
        match cx.js_dispatch_input_paste(self.id, fid, contents) {
            Ok(()) => {
                drain_light(cx);
                Some(EventResult::Consumed(None))
            }
            Err(e) => {
                cx.editor
                    .set_error(format!("plugin {} input paste failed: {e}", opts.label));
                Some(EventResult::Consumed(None))
            }
        }
    }

    /// 节点焦点路由:焦点在 button/input 时按键直接路由,不经过容器级 onKey。
    ///
    /// 返回 `None` = 该键不归节点处理(调用方继续自己的 onKey)。
    pub fn node_key(
        &mut self,
        key: &PluginKey,
        cx: &mut Context,
        opts: SurfaceOpts,
    ) -> Option<EventResult> {
        let fid = self.focus.as_ref()?;
        // 面板:Esc 只退出节点焦点(非模态);弹窗没有这条,走 onKey 的默认关闭语义。
        if key.name == "Esc" && opts.esc_releases_focus {
            self.focus = None;
            return Some(EventResult::Consumed(None));
        }
        match key.name.as_str() {
            // Enter:multiline input → 插入换行(dispatch_input_key 消费,不触发 onKey/提交);
            // 否则 input → onKey("Enter");button → onPress
            "Enter" => {
                if helix_js::input_has_state(self.id, fid)
                    && helix_js::input_is_multiline(self.id, fid)
                    && cx.js_dispatch_input_key(self.id, fid, &key.name).is_ok()
                {
                    drain_light(cx);
                    return Some(EventResult::Consumed(None));
                }
                let event = if helix_js::input_has_state(self.id, fid) {
                    Some(key.name.as_str())
                } else {
                    None
                };
                match cx.js_dispatch_node_event(self.id, fid, event) {
                    Ok(()) => {
                        drain_light(cx);
                        return Some(EventResult::Consumed(None));
                    }
                    Err(err) => {
                        // 失败可见:dispatch 的调用点用的是 is_ok()(失败 = 不消费按键),
                        // 错误若只往上传就会被**静默吞掉** —— 权限拒绝/插件抛错的表现
                        // 会变成"按了没反应"。这里上屏(Error)后继续冒泡。
                        drain_light(cx);
                        cx.editor.set_error(format!("{err}"));
                    }
                }
            }
            // 水平方向键/Home/End:input 光标移动(input_edit 对这些键返回 None,不触发 onChange)
            "Left" | "Right" | "Home" | "End" => {
                if helix_js::input_has_state(self.id, fid)
                    && cx.js_dispatch_input_key(self.id, fid, &key.name).is_ok()
                {
                    drain_light(cx);
                    return Some(EventResult::Consumed(None));
                }
            }
            // Up/Down:multiline input → 光标行间移动;否则候选导航(走 onKey)
            "Up" | "Down" => {
                if helix_js::input_has_state(self.id, fid)
                    && helix_js::input_is_multiline(self.id, fid)
                {
                    if cx.js_dispatch_input_key(self.id, fid, &key.name).is_ok() {
                        drain_light(cx);
                        return Some(EventResult::Consumed(None));
                    }
                } else if helix_js::input_has_state(self.id, fid)
                    && cx
                        .js_dispatch_node_event(self.id, fid, Some(&key.name))
                        .is_ok()
                {
                    drain_light(cx);
                    return Some(EventResult::Consumed(None));
                } else if !helix_js::input_has_state(self.id, fid) {
                    // 失败可见(同上):非 input 节点的 onKey 抛错/被权限拒绝时也要上屏
                    drain_light(cx);
                }
            }
            // 编辑键:input → dispatch_input_key(改值 + onChange);否则(button)→ onKey
            key_name
                if key_name.chars().count() == 1
                    || key_name == "Backspace"
                    || key_name == "Delete" =>
            {
                if helix_js::input_has_state(self.id, fid) {
                    if cx.js_dispatch_input_key(self.id, fid, &key.name).is_ok() {
                        drain_light(cx);
                        return Some(EventResult::Consumed(None));
                    }
                } else if cx
                    .js_dispatch_node_event(self.id, fid, Some(&key.name))
                    .is_ok()
                {
                    drain_light(cx);
                    return Some(EventResult::Consumed(None));
                } else {
                    drain_light(cx);
                }
            }
            _ => {}
        }
        None
    }

    /// 当前文档快照。弹窗是模态层(打开期间文档不变),面板非模态(每次按键重新序列化)。
    pub fn command_context(cx: &mut Context) -> CommandContext {
        let (view, doc) = helix_view::current_ref!(cx.editor);
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
    }

    /// onKey 之后的收尾:应用游标/编辑/装饰、刷消息、即时应用 UI 请求。
    ///
    /// 顺序不能变:编辑在 `popup_key` 之后同步应用,否则会拖到下一次按键。
    pub fn drain_after_onkey(cx: &mut Context, opts: SurfaceOpts) {
        let cursor_reqs = helix_js::take_cursor_requests();
        if !cursor_reqs.is_empty() {
            if let Err(err) = apply_cursor_requests(cx.editor, &cursor_reqs) {
                cx.editor
                    .set_error(format!("plugin {} cursor failed: {err}", opts.label));
            }
        }
        let edits = helix_js::take_edits();
        if !edits.is_empty() {
            if let Err(err) = apply_plugin_edits(cx.editor, &edits) {
                cx.editor
                    .set_error(format!("plugin {} edit failed: {err}", opts.label));
            }
        }
        let decorations = helix_js::take_decorations();
        if !decorations.is_empty() {
            if let Err(err) = apply_plugin_decorations(cx.editor, &decorations) {
                cx.editor
                    .set_error(format!("plugin {} decorations failed: {err}", opts.label));
            }
        }
        let msgs = helix_js::take_messages();
        if !msgs.is_empty() {
            cx.editor.set_status(msgs.join(" "));
        }
        // onKey 里发起的 UI 请求(open_file / move_panel / set_terminal_mode 等)即时应用——
        // 否则请求搁置到下一个 :命令才被 drain
        if let Err(err) = apply_ui_requests(cx.js.as_deref_mut(), helix_js::take_ui_requests()) {
            cx.editor.set_error(err.to_string());
        }
    }
}

/// 节点事件/粘贴之后的轻量 drain:只刷消息 + 即时应用 UI 请求(不动游标/编辑/装饰)。
fn drain_light(cx: &mut Context) {
    let msgs = helix_js::take_messages();
    if !msgs.is_empty() {
        cx.editor.set_status(msgs.join(" "));
    }
    if let Err(err) = apply_ui_requests(cx.js.as_deref_mut(), helix_js::take_ui_requests()) {
        cx.editor.set_error(err.to_string());
    }
}
