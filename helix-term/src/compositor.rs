// Each component declares its own size constraints and gets fitted based on its parent.
// Q: how does this work with popups?
// cursive does compositor.screen_mut().add_layer_at(pos::absolute(x, y), <component>)
use helix_core::Position;
use helix_view::graphics::{CursorKind, Rect};

use tui::buffer::Buffer as Surface;

pub type Callback = Box<dyn FnOnce(&mut Compositor, &mut Context)>;
pub type SyncCallback = Box<dyn FnOnce(&mut Compositor, &mut Context) + Sync>;

// Cursive-inspired
pub enum EventResult {
    Ignored(Option<Callback>),
    Consumed(Option<Callback>),
}

/// 全局窗口模式(C-w):任何叶子焦点下生效;模式键直调布局树。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WindowMode {
    Inactive,
    Active,
}

use crate::job::Jobs;
use crate::ui::picker;
use crate::ui::plugin_panel::{PanelSide, PluginPanel};
use helix_view::Editor;

pub use helix_view::input::Event;
use helix_view::document::Mode;

pub struct Context<'a> {
    pub editor: &'a mut Editor,
    pub scroll: Option<usize>,
    pub jobs: &'a mut Jobs,
}

impl Context<'_> {
    /// Waits on all pending jobs, and then tries to flush all pending write
    /// operations for all documents.
    pub fn block_try_flush_writes(&mut self) -> anyhow::Result<()> {
        tokio::task::block_in_place(|| helix_lsp::block_on(self.jobs.finish(self.editor, None)))?;
        tokio::task::block_in_place(|| helix_lsp::block_on(self.editor.flush_writes()))?;
        Ok(())
    }
}

pub trait Component: Any + AnyComponent {
    /// Process input events, return true if handled.
    fn handle_event(&mut self, _event: &Event, _ctx: &mut Context) -> EventResult {
        EventResult::Ignored(None)
    }
    // , args: ()

    /// Should redraw? Useful for saving redraw cycles if we know component didn't change.
    fn should_update(&self) -> bool {
        true
    }

    /// Render the component onto the provided surface.
    fn render(&mut self, area: Rect, frame: &mut Surface, ctx: &mut Context);

    /// Get cursor position and cursor kind.
    fn cursor(&self, _area: Rect, _ctx: &Editor) -> (Option<Position>, CursorKind) {
        (None, CursorKind::Hidden)
    }

    /// May be used by the parent component to compute the child area.
    /// viewport is the maximum allowed area, and the child should stay within those bounds.
    ///
    /// The returned size might be larger than the viewport if the child is too big to fit.
    /// In this case the parent can use the values to calculate scroll.
    fn required_size(&mut self, _viewport: (u16, u16)) -> Option<(u16, u16)> {
        None
    }

    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn id(&self) -> Option<&'static str> {
        None
    }
}

/// 布局标签条组件 id：JS 侧经 set_component_render(TABBAR_ID, fn) 注册视图回调，
/// compositor 渲染时在屏幕顶部画 1 行（未注册 → 树占满）。
pub const TABBAR_ID: u64 = 0x7ABB_0001;

pub struct Compositor {
    /// 瞬态覆盖层（弹窗/菜单/提示）——不参与布局，渲染在主区域之上
    layers: Vec<Box<dyn Component>>,
    /// 主区域布局树：编辑器/终端/面板都是叶子（tmux 式二分树）
    main_tree: crate::ui::layout::LayoutTree,
    area: Rect,

    pub(crate) last_picker: Option<Box<dyn Component>>,
    pub(crate) full_redraw: bool,
    pub(crate) window_mode: WindowMode,
    /// 标签条脏格 diff 渲染器
    tabbar_diff: crate::ui::comp_layout::DiffRenderer,
}

impl Compositor {
    pub fn new(area: Rect) -> Self {
        Self {
            main_tree: Default::default(),
            layers: Vec::new(),
            area,
            last_picker: None,
            full_redraw: false,
            window_mode: WindowMode::Inactive,
            tabbar_diff: Default::default(),
        }
    }

    /// 测试/状态栏访问:当前是否处于窗口模式
    pub fn window_mode_active(&self) -> bool {
        matches!(self.window_mode, WindowMode::Active)
    }

    pub fn size(&self) -> Rect {
        self.area
    }

    pub fn resize(&mut self, area: Rect) {
        self.area = area;
    }

    /// Add a layer to be rendered in front of all existing layers.
    pub fn push(&mut self, mut layer: Box<dyn Component>) {
        // 窗口模式下打开弹窗/菜单:自动退模式(否则弹窗按键被模式键位吞掉)
        if self.window_mode_active() && layer.id().is_some() {
            self.window_mode = WindowMode::Inactive;
        }
        // immediately clear last_picker field to avoid excessive memory
        // consumption for picker with many items
        if layer.id() == Some(picker::ID) {
            self.last_picker = None;
        }
        let size = self.size();
        // trigger required_size on init
        layer.required_size((size.width, size.height));
        self.layers.push(layer);
    }

    /// Replace a component that has the given `id` with the new layer and if
    /// no component is found, push the layer normally.
    pub fn replace_or_push<T: Component>(&mut self, id: &'static str, layer: T) {
        if let Some(component) = self.find_id(id) {
            *component = layer;
        } else {
            self.push(Box::new(layer))
        }
    }

    pub fn pop(&mut self) -> Option<Box<dyn Component>> {
        self.layers.pop()
    }

    pub fn remove(&mut self, id: &'static str) -> Option<Box<dyn Component>> {
        let idx = self
            .layers
            .iter()
            .position(|layer| layer.id() == Some(id))?;
        Some(self.layers.remove(idx))
    }

    pub fn remove_type<T: 'static>(&mut self) {
        let type_name = std::any::type_name::<T>();
        self.layers
            .retain(|component| component.type_name() != type_name);
        // 布局树里的同类组件（如终端叶子）也移除
        self.main_tree.remove_component_type::<T>();
        self.sync_layout_cache();
    }

    /// 按面板实例 id（PluginPanel::id，open_panel 分配的 u64）移除对应层；
    /// 多面板并存时各层以 u64 实例 id 区分（静态 id 只适用于单面板）。
    pub fn remove_panel(&mut self, id: u64) -> Option<Box<dyn Component>> {
        let panel_type = std::any::type_name::<PluginPanel>();
        let idx = self.layers.iter().position(|layer| {
            layer.type_name() == panel_type
                && layer
                    .as_any()
                    .downcast_ref::<PluginPanel>()
                    .map(|panel| panel.id() == id)
                    .unwrap_or(false)
        })?;
        Some(self.layers.remove(idx))
    }

    /// 按面板实例 id 改停靠边（move_panel 请求；找不到该 id 返回 false）
    pub fn set_panel_side(&mut self, id: u64, side: PanelSide) -> bool {
        let panel_type = std::any::type_name::<PluginPanel>();
        let Some(layer) = self.layers.iter_mut().find(|layer| {
            layer.type_name() == panel_type
                && layer
                    .as_any()
                    .downcast_ref::<PluginPanel>()
                    .map(|panel| panel.id() == id)
                    .unwrap_or(false)
        }) else {
            return false;
        };
        layer
            .as_any_mut()
            .downcast_mut::<PluginPanel>()
            .map(|panel| {
                panel.set_side(side);
                true
            })
            .unwrap_or(false)
    }
    pub fn handle_event(&mut self, event: &Event, cx: &mut Context) -> bool {
        // If it is a key event, a macro is being recorded, and a macro isn't being replayed,
        // push the key event to the recording.
        if let (Event::Key(key), Some((_, keys))) = (event, &mut cx.editor.macro_recording) {
            if cx.editor.macro_replaying.is_empty() {
                keys.push(*key);
            }
        }

        use helix_view::input::{KeyCode, KeyModifiers};
        // 窗口模式:normal/select 模式 C-w 进入;insert 模式 C-w 保留原义(删词,vim 惯例)
        if self.window_mode_active() {
            if let Event::Key(key) = event {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let ch = match key.code {
                    KeyCode::Char(c) => Some(c),
                    _ => None,
                };
                // 键位表:C-hjkl resize > Esc/C-w 退出 > hjkl 聚焦 / HJKL 交换 / x 关闭 / z 最小化 / f 最大化
                match ch {
                    Some(c) if ctrl && matches!(c, 'h' | 'j' | 'k' | 'l') => {
                        self.window_mode_resize(c);
                        return true;
                    }
                    Some(c) if c == 'w' && ctrl => {
                        self.window_mode = WindowMode::Inactive;
                        cx.editor.autoinfo = None;
                        return true;
                    }
                    _ if matches!(key.code, KeyCode::Esc) => {
                        self.window_mode = WindowMode::Inactive;
                        cx.editor.autoinfo = None;
                        return true;
                    }
                    _ if matches!(key.code, KeyCode::Enter) => {
                        // Enter:确认当前窗口 → 进入其 buffer 并退出窗口模式回 normal
                        self.window_mode = WindowMode::Inactive;
                        cx.editor.autoinfo = None;
                        return true;
                    }
                    Some('h') => {
                        self.window_mode_focus('h');
                        return true;
                    }
                    Some('j') => {
                        self.window_mode_focus('j');
                        return true;
                    }
                    Some('k') => {
                        self.window_mode_focus('k');
                        return true;
                    }
                    Some('l') => {
                        self.window_mode_focus('l');
                        return true;
                    }
                    Some('H') => {
                        self.window_mode_swap('h');
                        return true;
                    }
                    Some('J') => {
                        self.window_mode_swap('j');
                        return true;
                    }
                    Some('K') => {
                        self.window_mode_swap('k');
                        return true;
                    }
                    Some('L') => {
                        self.window_mode_swap('l');
                        return true;
                    }
                    Some('x') => {
                        self.window_mode_close();
                        return true;
                    }
                    Some('z') => {
                        self.window_mode_minimize();
                        return true;
                    }
                    Some('f') => {
                        self.window_mode_zoom();
                        return true;
                    }
                    _ => return true, // 模式吞掉未知键(保持模式)
                }
            }
            return true;
        }
        if let Event::Key(key) = event {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('w'))
                && cx.editor.mode() != Mode::Insert
                && !self.terminal_passthrough()
            {
                self.window_mode = WindowMode::Active;
                self.window_mode_hint(cx);
                return true;
            }
        }

        // 鼠标点击命中 → JS 视图层 component-event（标签条/有视图回调的叶子）
        if let Event::Mouse(mouse) = event {
            use helix_view::input::MouseEventKind;
            if let MouseEventKind::Down(_) = mouse.kind {
                let (x, y) = (mouse.column, mouse.row);
                // 标签条：顶部 1 行（有视图回调时）
                if helix_js::render_component(TABBAR_ID, 1, 1, None).is_ok() && y == self.area.y {
                    if helix_js::emit_component_event(TABBAR_ID, "click", x, y) {
                        return true;
                    }
                }
                // 叶子命中：有视图回调的叶子把点击交给 JS 视图层
                if let Some(id) = self.main_tree.leaf_id_at(x, y) {
                    if helix_js::render_component(id, 1, 1, None).is_ok()
                        && helix_js::emit_component_event(id, "click", x, y)
                    {
                        return true;
                    }
                }
            }
        }

        let mut callbacks = Vec::new();
        let mut consumed = false;

        // propagate events through the layers until we either find a layer that consumes it or we
        // run out of layers (event bubbling), starting at the front layer and then moving to the
        // background.
        for layer in self.layers.iter_mut().rev() {
            match layer.handle_event(event, cx) {
                EventResult::Consumed(Some(callback)) => {
                    callbacks.push(callback);
                    consumed = true;
                    break;
                }
                EventResult::Consumed(None) => {
                    consumed = true;
                    break;
                }
                EventResult::Ignored(Some(callback)) => {
                    callbacks.push(callback);
                }
                EventResult::Ignored(None) => {}
            };
        }

        // 瞬态层未消费 → 布局树（活动叶子优先，忽略则编辑器叶子兜底）
        if !consumed {
            match self.main_tree.handle_event(event, cx) {
                EventResult::Consumed(Some(callback)) => {
                    callbacks.push(callback);
                    consumed = true;
                }
                EventResult::Consumed(None) => {
                    consumed = true;
                }
                EventResult::Ignored(Some(callback)) => {
                    callbacks.push(callback);
                }
                EventResult::Ignored(None) => {}
            }
        }

        for callback in callbacks {
            callback(self, cx)
        }

        consumed
    }

    /// 底部 UI 层（命令/搜索 prompt、picker）：commandline 画在状态栏上一行
    fn is_bottom_layer(layer: &dyn Component) -> bool {
        let t = layer.type_name();
        t.contains("::prompt::Prompt") || t.contains("picker::Picker") || t.contains("Picker")
    }

    fn bottom_ui_active(&self) -> bool {
        self.layers
            .iter()
            .any(|l| Self::is_bottom_layer(l.as_ref()))
    }

    pub fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        // commandline（prompt/picker）活跃时占状态栏上一行：树多让 1 行；
        // 状态栏永远在最底 1 行（commandline 关闭后自动恢复）
        let bottom_ui = self.bottom_ui_active();
        // 标签条（JS 视图,顶部 1 行；未注册回调 → 树占满）
        let tabbar = helix_js::render_component(TABBAR_ID, area.width, 1, None).is_ok();
        let tree_area = area
            .clip_top(if tabbar { 1 } else { 0 })
            .clip_bottom(if bottom_ui { 2 } else { 1 });
        if tabbar {
            if let Ok(content) = helix_js::render_component(TABBAR_ID, area.width, 1, None) {
                let lines = crate::ui::comp_layout::render(content, (area.width, 1));
                self.tabbar_diff
                    .render(&lines, area.with_height(1), surface, &cx.editor.theme);
            }
        }
        // 主区域布局树：编辑器/终端/面板叶子各自在矩形里渲染
        self.main_tree.render(tree_area, surface, cx);
        // 瞬态覆盖层（弹窗/菜单/提示）渲染在主区域之上；
        // 底部 UI 层用缩一行的区域（commandline 画在状态栏上一行，不重叠）
        for layer in &mut self.layers {
            let layer_area = if bottom_ui && Self::is_bottom_layer(layer.as_ref()) {
                area.clip_bottom(1)
            } else {
                area
            };
            layer.render(layer_area, surface, cx);
        }
        // 全局状态栏：永远屏幕最底 1 行
        let statusline_area = area.clip_top(area.height.saturating_sub(1)).clip_bottom(1);
        let spinners = crate::ui::ProgressSpinners::default();
        let is_focused = self.main_tree.active() == 0;
        let (active_leaf_type, active_leaf_path) = self.active_leaf_info(cx);
        let (view, doc) = current_ref!(cx.editor);
        let mut context = crate::ui::statusline::RenderContext::new(
            cx.editor,
            doc,
            view,
            is_focused,
            &spinners,
            self.window_mode_active(),
            active_leaf_type,
            active_leaf_path,
        );
        crate::ui::statusline::render(&mut context, statusline_area, surface);
    }

    /// 活动窗口类型与路径(状态栏窗口图标用):editor/buffer 带文件路径,terminal/panel 无
    fn active_leaf_info(&mut self, cx: &mut Context) -> (&'static str, Option<String>) {
        let active = self.main_tree.active();
        if active == 0 {
            let (_, doc) = current_ref!(cx.editor);
            return (
                "editor",
                doc.path().map(|p| p.to_string_lossy().into_owned()),
            );
        }
        if let Some(id) = self
            .main_tree
            .find_leaf_id::<crate::ui::BufferLeaf>(|_| true)
        {
            if id == active {
                let path = self
                    .main_tree
                    .find_component::<crate::ui::BufferLeaf>()
                    .and_then(|bl| {
                        cx.editor.document(bl.view.doc).and_then(|d| {
                            d.path().map(|p| p.to_string_lossy().into_owned())
                        })
                    });
                return ("buffer", path);
            }
        }
        if let Some(id) = self
            .main_tree
            .find_leaf_id::<crate::ui::plugin_terminal::PluginTerminal>(|_| true)
        {
            if id == active {
                return ("terminal", None);
            }
        }
        ("panel", None)
    }

    pub fn cursor(&self, area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        let bottom_ui = self.bottom_ui_active();
        for layer in self.layers.iter().rev() {
            // 底部 UI 层（commandline）光标与渲染同用缩一行的区域
            let layer_area = if bottom_ui && Self::is_bottom_layer(layer.as_ref()) {
                area.clip_bottom(1)
            } else {
                area
            };
            if let (Some(pos), kind) = layer.cursor(layer_area, editor) {
                return (Some(pos), kind);
            }
        }
        (None, CursorKind::Hidden)
    }

    pub fn has_component(&self, type_name: &str) -> bool {
        self.layers
            .iter()
            .any(|component| component.type_name() == type_name)
    
        || self.main_tree.has_component(type_name)
}

    pub fn find<T: 'static>(&mut self) -> Option<&mut T> {
        let type_name = std::any::type_name::<T>();
        if let Some(c) = self
            .layers
            .iter_mut()
            .find(|component| component.type_name() == type_name)
            .and_then(|component| component.as_any_mut().downcast_mut())
        {
            return Some(c);
        }
        self.main_tree.find_component::<T>()
    }

    pub fn find_id<T: 'static>(&mut self, id: &'static str) -> Option<&mut T> {
        self.layers
            .iter_mut()
            .find(|component| component.id() == Some(id))
            .and_then(|component| component.as_any_mut().downcast_mut())
    }

    /// 按谓词找层（多实例同类型区分：终端层按 view_id 等自定义键查找，
    /// 避免每事件构造 &'static id 字符串）
    pub fn find_where<T: 'static>(&mut self, mut f: impl FnMut(&T) -> bool) -> Option<&mut T> {
        if let Some(c) = self
            .layers
            .iter_mut()
            .filter_map(|component| component.as_any_mut().downcast_mut::<T>())
            .find(|t| f(t))
        {
            return Some(c);
        }
        self.main_tree.find_component_where::<T>(f)
    }

    /// 重置全部插件面板/弹窗的脏格 diff 状态（测试向不同 surface 渲染时用）
    /// 设置主编辑器（布局树的 id=0 叶子；替换原来的 push EditorView）
    pub fn set_main_editor(&mut self, component: Box<dyn Component>) {
        self.main_tree.set_editor(component);
    }

    /// 把活动叶子按方向切分，新叶子挂 component（new_first=false：右/下侧）；返回新叶子 id
    pub fn split_leaf(
        &mut self,
        dir: crate::ui::layout::SplitDir,
        new_first: bool,
        component: Box<dyn Component>,
    ) -> Option<u64> {
        let active = self.main_tree.active();
        let ret = self.main_tree.split_side(active, dir, new_first, component);
        self.sync_layout_cache();
        ret
    }

    /// 同上，但新叶子占新叶子 side 的 ratio 份额（按区域尺寸换算成 first 的 ratio）
    pub fn split_leaf_with_ratio(
        &mut self,
        dir: crate::ui::layout::SplitDir,
        new_first: bool,
        new_size: u16,
        component: Box<dyn Component>,
    ) -> Option<u64> {
        let active = self.main_tree.active();
        let total = if dir == crate::ui::layout::SplitDir::H { self.area.width } else { self.area.height };
        let share = if new_first {
            new_size as f32 / total.max(1) as f32
        } else {
            1.0 - new_size as f32 / total.max(1) as f32
        };
        let ratio = share.clamp(0.1, 0.9);
        let new_id = self.main_tree.next_id_for_split();
        // 直接构造带 ratio 的 split
        let ret = self.main_tree.split_side_ratio(active, dir, new_first, ratio, component, new_id);
        self.sync_layout_cache();
        ret
    }

    /// 用预分配 id 切分（JS 已注册回调；id 由 JS 侧分配）
    pub fn split_leaf_prealloc(
        &mut self,
        id: u64,
        dir: crate::ui::layout::SplitDir,
        new_first: bool,
        new_size: u16,
        component: Box<dyn Component>,
    ) {
        let active = self.main_tree.active();
        let total = if dir == crate::ui::layout::SplitDir::H { self.area.width } else { self.area.height };
        let share = if new_first {
            new_size as f32 / total.max(1) as f32
        } else {
            1.0 - new_size as f32 / total.max(1) as f32
        };
        let ratio = share.clamp(0.1, 0.9);
        let _ = self.main_tree.split_side_ratio(active, dir, new_first, ratio, component, id);
        self.sync_layout_cache();
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    pub fn remove_leaf(&mut self, id: u64) {
        self.main_tree.remove(id);
        self.sync_layout_cache();
    }

    pub fn zoom_leaf(&mut self, id: u64) {
        self.main_tree.zoom(id);
        self.sync_layout_cache();
    }

    pub fn unzoom(&mut self) {
        self.main_tree.unzoom();
        self.sync_layout_cache();
    }

    pub fn resize_leaf(&mut self, id: u64, ratio: f32) {
        self.main_tree.resize(id, ratio);
        self.sync_layout_cache();
    }

    /// 按方向调整叶子份额（H=左右 / V=上下；delta>0 增大该叶子）。返回是否调整。
    pub fn resize_leaf_dir(&mut self, id: u64, dir: crate::ui::layout::SplitDir, delta: f32) -> bool {
        let ret = self.main_tree.resize_leaf_dir(id, dir, delta);
        self.sync_layout_cache();
        ret
    }

    /// 交换两个叶子的内容（组件引用互换，树结构/焦点不变）。
    pub fn swap_leaves(&mut self, id1: u64, id2: u64) -> bool {
        let ret = self.main_tree.swap(id1, id2);
        self.sync_layout_cache();
        ret
    }

    /// 最小化/恢复叶子（不占布局，渲染为底部标题横条）
    pub fn minimize_leaf(&mut self, id: u64, minimized: bool) {
        self.main_tree.set_minimized(id, minimized);
        self.sync_layout_cache();
    }

    /// 聚焦方向邻居（布局模式 h/j/k/l）；无邻居返回 None
    pub fn focus_leaf_dir(&mut self, id: u64, dir: crate::ui::layout::SplitDir, first: bool) -> Option<u64> {
        let ret = self.main_tree.focus_dir(id, dir, first);
        self.sync_layout_cache();
        ret
    }

    /// 与方向邻居交换内容（布局模式 H/J/K/L）；无邻居返回 false
    pub fn swap_leaf_dir(&mut self, id: u64, dir: crate::ui::layout::SplitDir, first: bool) -> bool {
        let ret = self.main_tree.swap_dir(id, dir, first);
        self.sync_layout_cache();
        ret
    }

    /// 叶子所在 Split 恢复 50/50
    pub fn equalize_leaf(&mut self, id: u64) {
        self.main_tree.equalize(id);
        self.sync_layout_cache();
    }

    /// 最小化叶子 id
    pub fn minimized_leaf(&self) -> Option<u64> {
        self.main_tree.minimized_leaf()
    }

    /// 方向字符 → (SplitDir, 是否 first 侧)。h/k → first;l/j → second。
    fn window_dir(c: char) -> (crate::ui::layout::SplitDir, bool) {
        match c {
            'h' => (crate::ui::layout::SplitDir::H, true),
            'l' => (crate::ui::layout::SplitDir::H, false),
            'k' => (crate::ui::layout::SplitDir::V, true),
            'j' => (crate::ui::layout::SplitDir::V, false),
            _ => unreachable!(),
        }
    }

    /// 窗口模式:方向聚焦(h/j/k/l)。委托 focus_leaf_dir(内部 neighbor_leaf+focus,含缓存同步)
    /// 终端 Insert 直通模式:C-w 放行给 pty(终端内 vim/emacs 需要;基线版本直通,
    /// 本分支恢复);Normal(滚动)模式的终端/编辑器/面板焦点照常进窗口模式。
    fn terminal_passthrough(&mut self) -> bool {
        let active = self.main_tree.active();
        self.find_where::<crate::ui::plugin_terminal::PluginTerminal>(|t| {
            t.view_id() == active
                && matches!(
                    t.input_mode(),
                    crate::ui::plugin_terminal::TermInputMode::Insert
                )
        })
        .is_some()
    }

    fn window_mode_focus(&mut self, c: char) {
        let (dir, side) = Self::window_dir(c);
        let a = self.main_tree.active();
        let _ = self.focus_leaf_dir(a, dir, side);
    }

    /// 窗口模式:与方向邻居交换内容(H/J/K/L)。委托 swap_leaf_dir(含缓存同步)
    fn window_mode_swap(&mut self, c: char) {
        let (dir, side) = Self::window_dir(c);
        let a = self.main_tree.active();
        let _ = self.swap_leaf_dir(a, dir, side);
    }

    /// 窗口模式:方向 resize(C-h/l 宽度 ∓5%;C-j/k 高度 ∓5%)。委托 resize_leaf_dir(含缓存同步)
    fn window_mode_resize(&mut self, c: char) {
        let (dir, delta) = match c {
            'h' => (crate::ui::layout::SplitDir::H, -0.05),
            'l' => (crate::ui::layout::SplitDir::H, 0.05),
            'j' => (crate::ui::layout::SplitDir::V, -0.05),
            'k' => (crate::ui::layout::SplitDir::V, 0.05),
            _ => unreachable!(),
        };
        let _ = self.resize_leaf_dir(self.main_tree.active(), dir, delta);
    }

    /// 窗口模式:关闭活动窗口。优先关闭覆盖层(layers)中的终端/面板浮层;
    /// 否则关闭布局树活动叶子(编辑器叶子 id=0 不可关)。
    fn window_mode_close(&mut self) {
        let layer_has_term = self
            .layers
            .iter()
            .any(|l| l.type_name().contains("::plugin_terminal::PluginTerminal"));
        let layer_has_panel = self
            .layers
            .iter()
            .any(|l| l.type_name().contains("::plugin_panel::PluginPanel"));
        if layer_has_term || layer_has_panel {
            // 只清 layers 中的组件(Drop → 杀 pty);布局树不受影响
            self.layers.retain(|l| {
                let t = l.type_name();
                !t.contains("::plugin_terminal::PluginTerminal")
                    && !t.contains("::plugin_panel::PluginPanel")
            });
            self.sync_layout_cache();
            return;
        }
        let a = self.main_tree.active();
        if a != 0 {
            self.remove_leaf(a);
        }
    }

    /// 窗口模式提示:进入时经 keymap_hint(JS set_keymap_hint)或内置文本显示键位表
    fn window_mode_hint(&mut self, cx: &mut Context) {
        use helix_view::info::Info;
        let entries: Vec<(String, String)> = [
            ("h j k l", "聚焦(左/下/上/右)"),
            ("H J K L", "交换"),
            ("C-h C-j C-k C-l", "尺寸 ∓5%"),
            ("x", "关闭窗口"),
            ("z", "最小化/还原"),
            ("f", "最大化/还原"),
            ("Esc / C-w", "退出"),
        ]
        .iter()
        .map(|(k, d)| (k.to_string(), d.to_string()))
        .collect();
        let text = helix_js::keymap_hint("C-w", &entries).unwrap_or_else(|| {
            entries
                .iter()
                .map(|(k, d)| format!("{k:<20} {d}"))
                .collect::<Vec<_>>()
                .join("\n")
        });
        let width = text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
        let height = text.lines().count() as u16;
        cx.editor.autoinfo = Some(Info {
            title: std::borrow::Cow::Borrowed("C-w"),
            text,
            width: width.saturating_add(2),
            height,
        });
    }

    /// 窗口模式:最小化/还原(z)。已最小化 → 还原;否则最小化活动叶子(编辑器除外)。
    /// 委托 minimize_leaf(含缓存同步)
    fn window_mode_minimize(&mut self) {
        if let Some(m) = self.main_tree.minimized() {
            self.minimize_leaf(m, false);
        } else {
            let a = self.main_tree.active();
            if a != 0 {
                self.minimize_leaf(a, true);
            }
        }
    }

    /// 窗口模式:最大化/还原(f)
    fn window_mode_zoom(&mut self) {
        let a = self.main_tree.active();
        if self.main_tree.zoomed() == Some(a) {
            self.unzoom();
        } else {
            self.zoom_leaf(a);
        }
    }

    /// 设置叶子 fixed 标记（fixed 叶子不被模式操作 swap/resize/close/minimize/equalize，可被焦点穿过）
    pub fn set_leaf_fixed(&mut self, id: u64, fixed: bool) {
        self.main_tree.set_fixed(id, fixed);
        self.sync_layout_cache();
    }

    /// 叶子是否 fixed
    pub fn leaf_fixed(&self, id: u64) -> bool {
        self.main_tree.is_fixed(id)
    }

    /// 布局树变更后同步 dump 缓存（get_layout 实时性；否则返回 null/旧值）
    fn sync_layout_cache(&mut self) {
        let json = serde_json::to_string(&self.main_tree.dump()).unwrap_or_default();
        helix_js::cache_layout(&json);
    }

    pub fn focus_leaf(&mut self, id: u64) {
        self.main_tree.focus(id);
        self.sync_layout_cache();
    }

    pub fn layout_tree(&mut self) -> &mut crate::ui::layout::LayoutTree {
        &mut self.main_tree
    }
    /// 浮动叶子（终端 Floating 模式）：返回当前浮动叶子 id
    pub fn floating(&self) -> Option<u64> {
        self.main_tree.floating()
    }

    /// 设置浮动叶子（渲染在最上层浮窗）；组件不存在时 no-op
    pub fn set_float(&mut self, id: u64) {
        self.main_tree.set_float(id);
        self.sync_layout_cache();
    }

    /// 取消浮动（终端回其 split 位置）
    pub fn unfloat(&mut self) {
        self.main_tree.unfloat();
        self.sync_layout_cache();
    }

    pub fn reset_plugin_diffs(&mut self) {
        self.tabbar_diff = Default::default();
        for layer in &mut self.layers {
            if let Some(p) = layer.as_any_mut().downcast_mut::<crate::ui::PluginPanel>() {
                p.reset_render_state();
            }
            if let Some(p) = layer.as_any_mut().downcast_mut::<crate::ui::plugin_popup::PluginPopup>() {
                p.reset_render_state();
            }
            if let Some(p) = layer.as_any_mut().downcast_mut::<crate::ui::plugin_terminal::PluginTerminal>() {
                p.reset_render_state();
            }
        }
        // 布局树里的面板叶子（split 出的面板 diff 同样需重置，否则测试向新 surface 渲染不重画）
        self.main_tree.reset_plugin_diffs();
    }

    /// 按类型统计层数量（测试/诊断）
    pub fn count_type(&self, type_name: &str) -> usize {
        self.layers.iter().filter(|l| l.type_name() == type_name).count()
    
        + self.main_tree.count_type(type_name)
}

    pub fn need_full_redraw(&mut self) {
        self.full_redraw = true;
    }

    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
}

// View casting, taken straight from Cursive

use std::any::Any;

/// A view that can be downcasted to its concrete type.
///
/// This trait is automatically implemented for any `T: Component`.
pub trait AnyComponent {
    /// Downcast self to a `Any`.
    fn as_any(&self) -> &dyn Any;

    /// Downcast self to a mutable `Any`.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Returns a boxed any from a boxed self.
    ///
    /// Can be used before `Box::downcast()`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use helix_term::{ui::Text, compositor::Component};
    /// let boxed: Box<dyn Component> = Box::new(Text::new("text".to_string()));
    /// let text: Box<Text> = boxed.as_boxed_any().downcast().unwrap();
    /// ```
    fn as_boxed_any(self: Box<Self>) -> Box<dyn Any>;
}

impl<T: Component> AnyComponent for T {
    /// Downcast self to a `Any`.
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// Downcast self to a mutable `Any`.
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_boxed_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl dyn AnyComponent {
    /// Attempts to downcast `self` to a concrete type.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.as_any().downcast_ref()
    }

    /// Attempts to downcast `self` to a concrete type.
    pub fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.as_any_mut().downcast_mut()
    }

    /// Attempts to downcast `Box<Self>` to a concrete type.
    pub fn downcast<T: Any>(self: Box<Self>) -> Result<Box<T>, Box<Self>> {
        // Do the check here + unwrap, so the error
        // value is `Self` and not `dyn Any`.
        if self.as_any().is::<T>() {
            Ok(self.as_boxed_any().downcast().unwrap())
        } else {
            Err(self)
        }
    }

    /// Checks if this view is of type `T`.
    pub fn is<T: Any>(&mut self) -> bool {
        self.as_any().is::<T>()
    }
}
