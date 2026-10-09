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

/// zellij 式平级模式(阶段①)。
///
/// 与旧的单一 `C-w` 模式(已删)的差别:**平级、各有前缀键**。
/// 除 `Locked` 外,任一模式内按任一前缀键直接切换,按同一个键回 `Normal`。
/// 前缀:`C-g` Locked / `C-p` Pane / `C-n` Resize / `C-h` Move / `C-y` Scroll。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaneMode {
    Normal,
    /// 除 `C-g` 外全部放行给当前叶子(`C-p` 等不再被解释)
    Locked,
    /// 聚焦 / 分屏 / 关闭 / 全屏 / 最小化
    Pane,
    /// 尺寸增减(hjkl 增,HJKL 减)
    Resize,
    /// 与方向邻居交换
    Move,
    /// 滚动回看缓冲
    Scroll,
}

use crate::job::Jobs;
use crate::ui::picker;
use crate::ui::plugin_panel::PanelSide;
use helix_view::Editor;

use helix_view::document::Mode;
pub use helix_view::input::Event;

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
    /// zellij 式平级模式(C-g Locked / C-p Pane / C-n Resize / C-h Move / C-y Scroll)
    pub(crate) pane_mode: PaneMode,
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
            pane_mode: PaneMode::Normal,
            tabbar_diff: Default::default(),
        }
    }

    /// 当前平级模式(状态栏指示与集成测试用)
    pub fn pane_mode(&self) -> PaneMode {
        self.pane_mode
    }

    pub fn size(&self) -> Rect {
        self.area
    }

    pub fn resize(&mut self, area: Rect) {
        self.area = area;
        // 视口尺寸推给 JS 状态(插件要"真居中"就必须读得到 ✓;此前没有任何接口 ✗)
        // 这里是**唯一入口** —— 全部尺寸变化都会经过它 ✓
        helix_js::set_viewport(area.width, area.height);
    }

    /// Add a layer to be rendered in front of all existing layers.
    pub fn push(&mut self, mut layer: Box<dyn Component>) {
        // 打开弹窗/菜单时自动退模式:不退出的话弹窗按键会被 pane_mode_key 吞掉
        if self.pane_mode != PaneMode::Normal && layer.id().is_some() {
            self.pane_mode = PaneMode::Normal;
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

    /// 按插件面板实例 id 关闭面板(布局树路径)。
    /// 面板住在树里(rail 叶子或普通叶子),所以先找叶子再关 ——
    /// `close_panel` / 面板状态自愈 / Esc 关闭三条路径共用这一处。
    pub fn close_panel_by_id(&mut self, id: u64) -> bool {
        use crate::ui::plugin_panel::PluginPanel;
        let Some(leaf) = self.main_tree.find_leaf_id::<PluginPanel>(|p| p.id() == id) else {
            return false;
        };
        // rail 面板要走 close_rail(remove 对 rail 免疫)
        if !self.close_rail(leaf) {
            self.remove_leaf(leaf);
        }
        self.sync_layout_cache();
        true
    }

    /// 按面板实例 id 改停靠边(move_panel 请求)。**面板住在布局树里**,
    /// 所以走树:rail 叶子换边 = 取出组件→按新边重新注册(保住宽度比例)。
    /// 旧实现查 `compositor.layers`,而面板不在那儿 → 恒 false,
    /// 于是 `move_panel` 永远报 "no panel with id"(见 plugin_panel.rs 的钉住测试)。
    pub fn set_panel_side(&mut self, id: u64, side: PanelSide) -> bool {
        use crate::ui::plugin_panel::PluginPanel;
        let Some(leaf) = self.main_tree.find_leaf_id::<PluginPanel>(|p| p.id() == id) else {
            return false;
        };
        // 只有 rail 面板支持换边:非 rail 面板是普通叶分裂,换边会牵动布局树结构
        if !self.main_tree.is_rail(leaf) {
            return false;
        }
        let left = match side {
            PanelSide::Left => true,
            PanelSide::Right => false,
            // rail 只有左右两侧(register_panel 也只对 left/right 生效)
            PanelSide::Bottom => return false,
        };
        let ratio = self.main_tree.rail_ratio().unwrap_or(0.2);
        let Some(mut comp) = self.main_tree.take_rail_component() else {
            return false;
        };
        if let Some(panel) = comp.as_any_mut().downcast_mut::<PluginPanel>() {
            panel.set_side(side);
        } else {
            return false;
        }
        self.main_tree.register_rail(comp, left, ratio);
        self.sync_layout_cache();
        // 焦点跟到新的 rail 叶(旧 id 已被 prune 掉)
        if let Some(new_leaf) = self.main_tree.rail_leaf() {
            self.main_tree.focus(new_leaf);
        }
        true
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
        // ── zellij 式平级模式(阶段①)──────────────────────────────
        // 前缀:C-g Locked / C-p Pane / C-n Resize / C-h Move / C-y Scroll。
        // 拦截规则(设计 §4.2):`C-g` 在所有状态下都拦(否则从终端 insert 直通里
        // 根本进不了 Locked);其余前缀在 insert / 终端直通时放行给叶子
        // (`C-h` 在 insert 里是删词,不豁免会毁掉退格)。
        if self.pane_mode != PaneMode::Normal {
            if let Event::Key(key) = event {
                return self.pane_mode_key(key, cx);
            }
            return false;
        }
        if let Event::Key(key) = event {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let ch = match key.code {
                    KeyCode::Char(c) => Some(c),
                    _ => None,
                };
                if ch == Some('g') {
                    self.set_pane_mode(PaneMode::Locked, cx);
                    return true;
                }
                // 输入态层在前(命令行/提示/选择器/菜单/补全)时前缀键归该层:
                // C-n/C-p 在命令行与补全里是"下一个/上一个",不能被模式抢走。
                // (insert 豁免盖不住这种情况——命令行时编辑器不是 Insert 模式)
                if !self.input_layer_active()
                    && cx.editor.mode() != Mode::Insert
                    && !self.terminal_passthrough()
                {
                    let next = match ch {
                        Some('p') => Some(PaneMode::Pane),
                        Some('n') => Some(PaneMode::Resize),
                        Some('h') => Some(PaneMode::Move),
                        Some('y') => Some(PaneMode::Scroll),
                        _ => None,
                    };
                    if let Some(m) = next {
                        self.set_pane_mode(m, cx);
                        return true;
                    }
                }
            }
        }

        // 鼠标点击命中 → JS 视图层 component-event（标签条/有视图回调的叶子）
        if let Event::Mouse(mouse) = event {
            use helix_view::input::MouseEventKind;
            if let MouseEventKind::Down(_) = mouse.kind {
                let (x, y) = (mouse.column, mouse.row);
                // 标签条：顶部 1 行（有视图回调时）
                if helix_js::render_component(TABBAR_ID, 1, 1, None).is_ok()
                    && y == self.area.y
                    && helix_js::emit_component_event(TABBAR_ID, "click", x, y)
                {
                    return true;
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
            self.pane_mode != PaneMode::Normal,
            match self.pane_mode {
                PaneMode::Normal => None,
                PaneMode::Locked => Some("LOCKED"),
                PaneMode::Pane => Some("PANE"),
                PaneMode::Resize => Some("RESIZE"),
                PaneMode::Move => Some("MOVE"),
                PaneMode::Scroll => Some("SCROLL"),
            },
            active_leaf_type,
            active_leaf_path,
        );
        crate::ui::statusline::render(&mut context, statusline_area, surface);
        // keymap 前缀提示(Info):全屏坐标渲染,不随叶子区域漂移;位置由 Info.position 决定
        if cx.editor.config().auto_info {
            if let Some(mut info) = cx.editor.autoinfo.take() {
                info.render(area, surface, cx);
                cx.editor.autoinfo = Some(info)
            }
        }
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
                        let vid = bl.view_id;
                        if !cx.editor.tree.contains(vid) {
                            return None;
                        }
                        let doc = cx.editor.tree.get(vid).doc;
                        cx.editor
                            .document(doc)
                            .and_then(|d| d.path().map(|p| p.to_string_lossy().into_owned()))
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
        let tabbar = helix_js::render_component(TABBAR_ID, 1, 1, None).is_ok();
        let tree_area = area
            .clip_top(if tabbar { 1 } else { 0 })
            .clip_bottom(if bottom_ui { 2 } else { 1 });
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
        // 布局树活动叶子(编辑器 bar/underline 光标;终端/面板默认 Hidden)
        self.main_tree.cursor(tree_area, editor)
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
        let total = if dir == crate::ui::layout::SplitDir::H {
            self.area.width
        } else {
            self.area.height
        };
        let share = if new_first {
            new_size as f32 / total.max(1) as f32
        } else {
            1.0 - new_size as f32 / total.max(1) as f32
        };
        let ratio = share.clamp(0.1, 0.9);
        let new_id = self.main_tree.next_id_for_split();
        // 直接构造带 ratio 的 split
        let ret = self
            .main_tree
            .split_side_ratio(active, dir, new_first, ratio, component, new_id);
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
        let total = if dir == crate::ui::layout::SplitDir::H {
            self.area.width
        } else {
            self.area.height
        };
        let share = if new_first {
            new_size as f32 / total.max(1) as f32
        } else {
            1.0 - new_size as f32 / total.max(1) as f32
        };
        let ratio = share.clamp(0.1, 0.9);
        let _ = self
            .main_tree
            .split_side_ratio(active, dir, new_first, ratio, component, id);
        self.sync_layout_cache();
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    pub fn remove_leaf(&mut self, id: u64) {
        self.main_tree.remove(id);
        self.sync_layout_cache();
    }

    /// 同步 editor.tree.focus 到活动叶子承载的 view（叶=可编辑窗口的路由基础）：
    /// 活动叶为 BufferLeaf → focus 指向其 view；活动叶为编辑器(id=0) → focus 指向
    /// 未被任何 BufferLeaf 认领的 view（含旧 view-tree 遗留）；terminal/panel 无 view 不动。
    pub fn sync_editor_focus(&mut self, editor: &mut Editor) {
        let target = match self.main_tree.active() {
            0 => {
                let claimed = self.main_tree.claimed_view_ids();
                let focus_unclaimed = editor
                    .tree
                    .views()
                    .any(|(v, _)| v.id == editor.tree.focus && !claimed.contains(&v.id));
                if focus_unclaimed {
                    return; // 已指向合法 view,不动
                }
                editor
                    .tree
                    .views()
                    .find(|(v, _)| !claimed.contains(&v.id))
                    .map(|(v, _)| v.id)
            }
            id => self.main_tree.view_id_of(id),
        };
        if let Some(vid) = target {
            editor.tree.focus = vid;
        }
    }

    /// 关闭一个 BufferLeaf 叶子：同步移除其在 editor.tree 中注册的 view
    /// （否则 tree.traverse 仍视为“文档被其他窗口显示”，remove_empty_scratch 等误判）。
    pub fn remove_buffer_leaf(&mut self, editor: &mut Editor, id: u64) {
        if let Some(vid) = self.main_tree.view_id_of(id) {
            let doc_id = editor.tree.contains(vid).then(|| editor.tree.get(vid).doc);
            self.main_tree.remove(id);
            if editor.tree.contains(vid) {
                // editor.close 清理所有 doc 上该 view 的 selection + 树节点
                editor.close(vid);
            }
            // 空 scratch(无路径且未修改)失去最后一个 view → 随窗销毁,
            // 否则 ghost buffer 残留(bufferline 按文档数显示,gn 才触发回收)。
            if let Some(doc_id) = doc_id {
                let orphan_scratch = editor
                    .document(doc_id)
                    .map(|d| !d.is_modified() && d.path().is_none())
                    .unwrap_or(false)
                    && !editor.tree.views().any(|(v, _)| v.doc == doc_id);
                if orphan_scratch {
                    let _ = editor.close_document(doc_id, false);
                }
            }
            self.sync_layout_cache();
            self.sync_editor_focus(editor);
        } else {
            self.remove_leaf(id);
        }
    }

    /// 关闭任意叶(JS close_leaf 等入口):BufferLeaf 须同步清理其在 editor.tree 的 view
    /// (否则 tree.focus 悬空 → 每帧 serialize 的 tree.get(focus) panic);rail 走 take_rail。
    pub fn close_leaf_clean(&mut self, editor: &mut Editor, id: u64) {
        if self.main_tree.is_rail(id) {
            self.main_tree.take_rail();
            self.sync_layout_cache();
            self.sync_editor_focus(editor);
        } else if self.main_tree.view_id_of(id).is_some() {
            self.remove_buffer_leaf(editor, id);
        } else {
            self.remove_leaf(id);
            self.sync_editor_focus(editor);
        }
    }

    /// 关闭 rail(专用:x/ClosePanel 落在 rail 上;LayoutTree.remove 对 rail 免疫)
    pub fn close_rail(&mut self, id: u64) -> bool {
        if self.main_tree.is_rail(id) {
            self.main_tree.take_rail();
            self.sync_layout_cache();
            true
        } else {
            false
        }
    }

    /// 收编孤儿 view(旧 view-tree 分裂遗留/启动多文件等非叶 view)为 BufferLeaf 叶:
    /// 保留首个作为编辑器叶,其余逐个开叶。调用点:Application::new 启动末尾。
    pub fn adopt_orphan_views(&mut self, editor: &mut Editor) {
        let claimed = self.main_tree.claimed_view_ids();
        let views: Vec<helix_view::ViewId> = editor
            .tree
            .views()
            .map(|(v, _)| v.id)
            .filter(|id| !claimed.contains(id))
            .collect();
        if views.len() <= 1 {
            return; // 0 或仅编辑器自身 view,无需收编
        }
        for vid in views.iter().skip(1) {
            self.split_leaf(
                crate::ui::layout::SplitDir::H,
                false,
                Box::new(crate::ui::BufferLeaf { view_id: *vid }),
            );
        }
        self.sync_editor_focus(editor);
    }

    /// 把面板注册为 rail(侧栏):side left/right → LayoutTree 边缘全高;bottom 维持普通叶。
    /// size=面板期望列宽(整数值)。返回新叶 id。
    pub fn register_panel(
        &mut self,
        panel: crate::ui::PluginPanel,
        side: crate::ui::plugin_panel::PanelSide,
        size: u16,
    ) -> Option<u64> {
        let left = matches!(side, crate::ui::plugin_panel::PanelSide::Left);
        let width = self.area.width.max(1) as f32;
        let ratio = (size as f32 / width).clamp(0.05, 0.9);
        self.main_tree.register_rail(Box::new(panel), left, ratio);
        self.sync_layout_cache();
        self.main_tree.rail_leaf()
    }

    /// 打开路径到新叶（buffer_open/:vsplit path/gf 共用落点）：editor.open(Load) 建/取 doc
    /// → open_buffer_leaf 挂叶。失败 set_error。返回是否成功。
    pub fn open_doc_in_new_leaf(
        &mut self,
        editor: &mut Editor,
        path: std::path::PathBuf,
        dir: crate::ui::layout::SplitDir,
        new_first: bool,
    ) -> bool {
        use helix_view::editor::Action;
        let doc_id = match editor.open(&path, Action::Load) {
            Ok(id) => id,
            Err(e) => {
                editor.set_error(format!("open: failed to open {}: {e}", path.display()));
                return false;
            }
        };
        self.open_buffer_leaf(editor, doc_id, dir, new_first);
        true
    }

    /// 在活动叶子旁开新 BufferLeaf 叶显示 doc_id（同 doc 双视图/打开文件落点）：
    /// 若当前 view 正显示该 doc 则克隆它（复制光标/滚动，对齐 core switch-Split 语义），
    /// 否则新建 view；register_flat 注册 → split_leaf 挂载 → focus 同步。
    pub fn open_buffer_leaf(
        &mut self,
        editor: &mut Editor,
        doc_id: helix_view::DocumentId,
        dir: crate::ui::layout::SplitDir,
        new_first: bool,
    ) -> Option<u64> {
        let gutters = editor.config().gutters.clone();
        let cur = editor
            .tree
            .try_get(editor.tree.focus)
            .filter(|v| v.doc == doc_id)
            .cloned();
        let view = cur.unwrap_or_else(|| helix_view::view::View::new(doc_id, gutters));
        let vid = editor.tree.register_flat(view);
        if let Some(doc) = editor.document_mut(doc_id) {
            doc.ensure_view_init(vid);
            doc.mark_as_focused();
        }
        let leaf = self.split_leaf(
            dir,
            new_first,
            Box::new(crate::ui::BufferLeaf { view_id: vid }),
        );
        self.sync_editor_focus(editor);
        leaf
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
    pub fn resize_leaf_dir(
        &mut self,
        id: u64,
        dir: crate::ui::layout::SplitDir,
        delta: f32,
    ) -> bool {
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
    pub fn focus_leaf_dir(
        &mut self,
        id: u64,
        dir: crate::ui::layout::SplitDir,
        first: bool,
    ) -> Option<u64> {
        let ret = self.main_tree.focus_dir(id, dir, first);
        self.sync_layout_cache();
        ret
    }

    /// 与方向邻居交换内容（布局模式 H/J/K/L）；无邻居返回 false
    pub fn swap_leaf_dir(
        &mut self,
        id: u64,
        dir: crate::ui::layout::SplitDir,
        first: bool,
    ) -> bool {
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
    /// 终端 Insert 直通模式:前缀键放行给 pty(终端内 vim/emacs 需要),
    /// 模式不在直通态下进入;Normal(滚动)态的终端/编辑器/面板焦点照常进模式。
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

    fn window_mode_focus(&mut self, c: char, cx: &mut Context) {
        let (dir, side) = Self::window_dir(c);
        let a = self.main_tree.active();
        let _ = self.focus_leaf_dir(a, dir, side);
        self.sync_editor_focus(cx.editor);
    }

    /// 窗口模式:与方向邻居交换内容(H/J/K/L)。委托 swap_leaf_dir(含缓存同步)
    fn window_mode_swap(&mut self, c: char) {
        let (dir, side) = Self::window_dir(c);
        let a = self.main_tree.active();
        let _ = self.swap_leaf_dir(a, dir, side);
    }

    /// `p` / `P` / `Tab`:活动叶在堆叠组里 → **轮转堆叠**(组内容就地切换);
    /// 否则维持"切下一个/上一个窗口"。轮转后活动叶跟着新锚走
    /// (零映射:兄弟成员覆盖同一区域 —— 规格 A.6/A.7)。
    fn pane_cycle_or_stack(&mut self, forward: bool, cx: &mut Context) {
        let active = self.main_tree.active();
        if self.main_tree.is_stacked(active) {
            if let Some(new_anchor) = self.main_tree.stack_rotate(active, forward) {
                self.main_tree.focus(new_anchor);
                self.sync_editor_focus(cx.editor);
                self.sync_layout_cache();
                return;
            }
        }
        self.window_mode_cycle_focus(forward, cx);
    }

    /// 切到下一个/上一个叶子(按树中序环绕)。zellij 的 `p` / `Tab`。
    fn window_mode_cycle_focus(&mut self, forward: bool, cx: &mut Context) {
        let ids = self.main_tree.leaf_ids();
        if ids.len() < 2 {
            return;
        }
        let active = self.main_tree.active();
        let pos = ids.iter().position(|&i| i == active).unwrap_or(0);
        let next = if forward {
            (pos + 1) % ids.len()
        } else {
            (pos + ids.len() - 1) % ids.len()
        };
        self.main_tree.focus(ids[next]);
        self.sync_editor_focus(cx.editor);
    }

    /// 方向 resize(C-h/l 宽度 ∓5%;C-j/k 高度 ∓5%)。`sign` 取反即反方向。
    /// 委托 resize_leaf_dir(含缓存同步)
    fn window_mode_resize_signed(&mut self, c: char, sign: f32) {
        let (dir, delta) = match c {
            'h' => (crate::ui::layout::SplitDir::H, -0.05),
            'l' => (crate::ui::layout::SplitDir::H, 0.05),
            'j' => (crate::ui::layout::SplitDir::V, -0.05),
            'k' => (crate::ui::layout::SplitDir::V, 0.05),
            _ => unreachable!(),
        };
        let _ = self.resize_leaf_dir(self.main_tree.active(), dir, delta * sign);
    }

    /// 切换平级模式:置位 + 显示/清除键位提示(``Normal`` 时清掉)
    fn set_pane_mode(&mut self, mode: PaneMode, cx: &mut Context) {
        self.pane_mode = mode;
        // 插件侧可见性:helix.pane_mode.* + pane-mode-change 事件(单一来源)
        Self::publish_pane_mode(mode);
        if mode == PaneMode::Normal {
            cx.editor.autoinfo = None;
        } else {
            self.pane_mode_hint(mode, cx);
        }
    }

    /// 平级模式的键位表(单一来源):(模式显示名, [(键, 说明, 是否已实现)])。
    /// 内置提示与 `helix.pane_mode.keymap()` 都从这里取,避免两处硬编码漂移。
    fn pane_mode_entries(
        mode: PaneMode,
    ) -> (&'static str, Vec<(&'static str, &'static str, bool)>) {
        match mode {
            PaneMode::Normal => ("Normal", Vec::new()),
            PaneMode::Locked => (
                "C-g",
                vec![("C-g", "退出 locked(其余键原样交给当前窗口)", true)],
            ),
            PaneMode::Pane => (
                "C-p",
                vec![
                    ("h j k l", "聚焦(左/下/上/右)", true),
                    ("H J K L", "交换", true),
                    ("p / P / Tab", "切到下一个/上一个窗口", true),
                    ("n / d / r", "新分屏 / 下分 / 右分", true),
                    ("x", "关闭窗口", true),
                    ("f", "最大化/还原", true),
                    ("z", "最小化/还原", true),
                    ("w / e", "浮动 / 收回平铺", true),
                    ("i", "pin(浮动时置顶)", true),
                    ("s", "堆叠(与兄弟窗合并)", true),
                    ("Esc / C-p", "退出", true),
                ],
            ),
            PaneMode::Resize => (
                "C-n",
                vec![
                    ("h j k l", "向该方向增大(浮窗:改尺寸)", true),
                    ("H J K L", "向该方向减小", true),
                    ("= / - / +", "宽度 ±5%", true),
                    ("Esc / C-n", "退出", true),
                ],
            ),
            PaneMode::Move => (
                "C-h",
                vec![
                    ("h j k l", "与方向邻居交换(浮窗:搬位置)", true),
                    ("Esc / C-h", "退出", true),
                ],
            ),
            PaneMode::Scroll => (
                "C-y",
                vec![
                    ("j / k", "行滚动", true),
                    ("d / u", "半页", true),
                    ("C-f C-b / h l", "整页", true),
                    ("Esc / C-y", "退出", true),
                ],
            ),
        }
    }

    /// 把模式快照推给插件:helix.pane_mode.current()/keymap() 读它,
    /// 同时发 pane-mode-change 事件。单一来源 = pane_mode_entries。
    fn publish_pane_mode(mode: PaneMode) {
        let (name, entries) = Self::pane_mode_entries(mode);
        let keys: Vec<serde_json::Value> = entries
            .iter()
            .map(|(k, d, enabled)| {
                let mut o = serde_json::json!({ "key": k, "desc": d, "enabled": enabled });
                if !enabled {
                    o["reason"] = serde_json::json!("阶段② 余下");
                }
                o
            })
            .collect();
        let json = serde_json::json!({ "mode": name, "keys": keys }).to_string();
        helix_js::cache_pane_mode(&json);
        helix_js::emit_pane_mode_change(name);
    }

    /// 平级模式的键位提示(经 `keymap_hint` 可由 JS `set_keymap_hint` 接管,
    /// 缺省用内置文本)。各模式的条目与 `pane_mode_key` 里的键位表一一对应。
    fn pane_mode_hint(&mut self, mode: PaneMode, cx: &mut Context) {
        use helix_view::info::Info;
        let (key, entries) = Self::pane_mode_entries(mode);
        if entries.is_empty() {
            return;
        }
        let owned: Vec<(String, String)> = entries
            .iter()
            .map(|(k, d, enabled)| {
                let d = if *enabled {
                    (*d).to_string()
                } else {
                    format!("{d}(阶段② 余下)")
                };
                (k.to_string(), d)
            })
            .collect();
        let (text, position) = helix_js::keymap_hint(key, &owned).unwrap_or_else(|| {
            (
                owned
                    .iter()
                    .map(|(k, d)| format!("{k:<20} {d}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                "bottom-left".to_string(),
            )
        });
        let width = text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
        let height = text.lines().count() as u16;
        cx.editor.autoinfo = Some(Info {
            title: std::borrow::Cow::Borrowed(key),
            text,
            width: width.saturating_add(2),
            height,
            position: crate::keymap::hint_position(&position),
        });
    }

    /// 是否有"输入态"层在前(命令行/提示/选择器/菜单/补全)。
    /// 终端与面板层不算:它们是叶子式内容,模式要在其焦点下可用。
    fn input_layer_active(&self) -> bool {
        self.layers.iter().any(|l| {
            let t = l.type_name();
            !t.contains("::plugin_terminal::PluginTerminal")
                && !t.contains("::plugin_panel::PluginPanel")
        })
    }

    /// 平级模式内的按键。返回 `true` = 已消费;`false` = 放行给叶子(仅 `Locked`)。
    fn pane_mode_key(&mut self, key: &helix_view::input::KeyEvent, cx: &mut Context) -> bool {
        use helix_view::input::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let ch = match key.code {
            KeyCode::Char(c) => Some(c),
            _ => None,
        };

        // 前缀键在任一模式下直接切换;按同一个键回 Normal(zellij 语义)
        if ctrl {
            let next = match ch {
                Some('g') => Some(PaneMode::Locked),
                Some('p') => Some(PaneMode::Pane),
                Some('n') => Some(PaneMode::Resize),
                Some('h') => Some(PaneMode::Move),
                Some('y') => Some(PaneMode::Scroll),
                _ => None,
            };
            if let Some(m) = next {
                // Locked 只能靠 C-g 进出:其余前缀在 Locked 内不响应(全部放行给叶子)
                if self.pane_mode == PaneMode::Locked && m != PaneMode::Locked {
                    return false;
                }
                self.set_pane_mode(
                    if self.pane_mode == m {
                        PaneMode::Normal
                    } else {
                        m
                    },
                    cx,
                );
                return true;
            }
        }

        if self.pane_mode == PaneMode::Locked {
            return false; // 除 C-g 外全部原样交给当前叶子
        }
        if matches!(key.code, KeyCode::Esc) {
            self.set_pane_mode(PaneMode::Normal, cx);
            return true;
        }

        match self.pane_mode {
            PaneMode::Pane => match ch {
                Some(c @ ('h' | 'j' | 'k' | 'l')) => self.window_mode_focus(c, cx),
                Some('H') => self.window_mode_swap('h'),
                Some('J') => self.window_mode_swap('j'),
                Some('K') => self.window_mode_swap('k'),
                Some('L') => self.window_mode_swap('l'),
                Some('n') => self.window_mode_split('n', cx),
                Some('d') => self.window_mode_split('s', cx),
                Some('r') => self.window_mode_split('v', cx),
                Some('x') => self.window_mode_close(cx),
                Some('f') => self.window_mode_zoom(),
                Some('z') => self.window_mode_minimize(),
                // zellij 的浮动/嵌入/stack/pin(阶段②:浮动已接通,stack 待布局模型)
                Some('w') | Some('e') => {
                    let id = self.main_tree.active();
                    self.main_tree.float_toggle(id);
                    let now = self.main_tree.is_leaf_floating(id);
                    cx.editor.set_status(if now {
                        "浮动 pane(再按 w/e 收回平铺)".to_string()
                    } else {
                        "已收回平铺".to_string()
                    });
                }
                Some('i') => {
                    let id = self.main_tree.active();
                    if self.main_tree.is_leaf_floating(id) {
                        let now = !self.main_tree.float_is_pinned(id);
                        self.main_tree.float_set_pinned(id, now);
                        cx.editor.set_status(if now {
                            "已 pin(置顶)"
                        } else {
                            "已取消 pin"
                        });
                    } else {
                        cx.editor.set_status("pin 只对浮动 pane 有意义(w/e 先浮动)");
                    }
                }
                Some('s') => {
                    let id = self.main_tree.active();
                    if self.main_tree.stack_new_with_sibling(id).is_some() {
                        self.sync_layout_cache();
                        cx.editor
                            .set_status("已堆叠(p / P / Tab 在组内切换;再按 s 无效)");
                    } else if self.main_tree.is_stacked(id) {
                        cx.editor.set_status("已在堆叠组里(p / P / Tab 切换)");
                    } else {
                        cx.editor
                            .set_status("堆叠需要两个相邻的叶子窗(兄弟):当前无法堆叠");
                    }
                }
                // zellij 的切换焦点:p 下一个、P 上一个、Tab 下一个
                Some('p') => self.pane_cycle_or_stack(true, cx),
                Some('P') => self.pane_cycle_or_stack(false, cx),
                _ if matches!(key.code, KeyCode::Tab) => self.pane_cycle_or_stack(true, cx),
                _ => {}
            },
            PaneMode::Resize => {
                // 浮动的活动叶:调浮窗尺寸;否则调分界比例
                let id = self.main_tree.active();
                if self.main_tree.is_leaf_floating(id) {
                    if let Some(c) = ch {
                        let (dw, dh) = match c {
                            'h' => (-0.02, 0.0),
                            'l' => (0.02, 0.0),
                            'j' => (0.0, -0.02),
                            'k' => (0.0, 0.02),
                            'H' => (0.02, 0.0),
                            'L' => (-0.02, 0.0),
                            'J' => (0.0, 0.02),
                            'K' => (0.0, -0.02),
                            '=' | '+' => (0.02, 0.02),
                            '-' => (-0.02, -0.02),
                            _ => (0.0, 0.0),
                        };
                        self.main_tree.float_scale(id, dw, dh);
                    }
                } else {
                    match ch {
                        Some(c @ ('h' | 'j' | 'k' | 'l')) => self.window_mode_resize_signed(c, 1.0),
                        Some(c @ ('H' | 'J' | 'K' | 'L')) => {
                            self.window_mode_resize_signed(c.to_ascii_lowercase(), -1.0)
                        }
                        // zellij 的 `=`/`-`(整体增减):这里落到宽度 ±5%
                        Some('=') | Some('+') => self.window_mode_resize_signed('l', 1.0),
                        Some('-') => self.window_mode_resize_signed('h', 1.0),
                        _ => {}
                    }
                }
            }
            PaneMode::Move => {
                let id = self.main_tree.active();
                if self.main_tree.is_leaf_floating(id) {
                    // 浮动的:按方向平移浮窗(zellij 的 Move 模式对浮窗就是搬位置)
                    if let Some(c) = ch {
                        let (dx, dy) = match c {
                            'h' => (-0.02, 0.0),
                            'l' => (0.02, 0.0),
                            'j' => (0.0, 0.02),
                            'k' => (0.0, -0.02),
                            _ => (0.0, 0.0),
                        };
                        self.main_tree.float_nudge(id, dx, dy);
                    }
                } else if let Some(c @ ('h' | 'j' | 'k' | 'l')) = ch {
                    self.window_mode_swap(c);
                }
            }
            PaneMode::Scroll => match ch {
                Some('j') => self.pane_scroll(3),
                Some('k') => self.pane_scroll(-3),
                Some('d') => self.pane_scroll(12),
                Some('u') => self.pane_scroll(-12),
                Some('f') | Some('l') => self.pane_scroll(24),
                Some('b') | Some('h') => self.pane_scroll(-24),
                _ => match key.code {
                    KeyCode::Down => self.pane_scroll(3),
                    KeyCode::Up => self.pane_scroll(-3),
                    KeyCode::PageDown => self.pane_scroll(24),
                    KeyCode::PageUp => self.pane_scroll(-24),
                    _ => {}
                },
            },
            _ => {}
        }
        true // 模式内未绑定的键吞掉(不穿透)
    }

    /// Scroll 模式:滚动当前焦点叶子的回看缓冲。
    /// 编辑器叶子的视口滚动与 `s` 搜索尚未接(见阶段① 的未完成项)。
    fn pane_scroll(&mut self, lines: i32) {
        let active = self.main_tree.active();
        if let Some(t) =
            self.find_where::<crate::ui::plugin_terminal::PluginTerminal>(|t| t.view_id() == active)
        {
            t.scroll_by(lines);
        }
    }

    /// 窗口模式:创建新窗(v/s 同 doc 分屏,n 新空 buffer)。创建后保持模式可连续操作。
    fn window_mode_split(&mut self, c: char, cx: &mut Context) {
        use crate::ui::layout::SplitDir;
        if c == 'n' {
            let dir = SplitDir::V;
            let doc_id = cx.editor.create_scratch_document();
            self.open_buffer_leaf(cx.editor, doc_id, dir, false);
        } else {
            let dir = match c {
                'v' => SplitDir::H,
                's' => SplitDir::V,
                _ => unreachable!(),
            };
            if let Some(doc_id) = cx.editor.tree.try_get(cx.editor.tree.focus).map(|v| v.doc) {
                self.open_buffer_leaf(cx.editor, doc_id, dir, false);
            }
        }
    }

    /// 窗口模式:关闭活动窗口。优先关闭覆盖层(layers)中的终端/面板浮层;
    /// 否则关闭布局树活动叶子(编辑器叶子 id=0 不可关)。
    fn window_mode_close(&mut self, cx: &mut Context) {
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
            if self.main_tree.is_rail(a) {
                self.close_rail(a);
                self.sync_editor_focus(cx.editor);
            } else if self.main_tree.view_id_of(a).is_some() {
                self.remove_buffer_leaf(cx.editor, a);
            } else {
                self.remove_leaf(a);
            }
            self.sync_editor_focus(cx.editor);
        }
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
    pub(crate) fn sync_layout_cache(&mut self) {
        let dump = self.main_tree.dump();
        // pane.list():把树叶子与浮窗统一成"pane"视角(place/focused/pinned)。
        // ③ 会在此加 kind/rect 等字段;这里先把"看得见浮窗"这个缺口补上。
        let active = dump.active;
        let mut panes: Vec<serde_json::Value> = dump
            .leafs
            .iter()
            .map(|l| {
                // 堆叠:列出该叶所在组的成员(不在组里 → 空数组)。
                // 放进**同一份快照**的理由:stack.list() 因此无需新状态,也不会与 pane.list() 失配。
                // 不变量:成员 >1 时 `members[0]` 就是锚(= 当前显示的那个,见规格 A.6)。
                let members = self.main_tree.stack_members(l.id);
                let stack = if members.len() > 1 {
                    serde_json::json!(members)
                } else {
                    serde_json::json!([])
                };
                serde_json::json!({
                    "id": l.id,
                    "kind": self.main_tree.component_kind_of(l.id).unwrap_or(""),
                    "place": if l.rail { "rail" } else { "tiled" },
                    "focused": l.id == active,
                    "fixed": l.fixed,
                    "pinned": false,
                    "stack": stack,
                })
            })
            .collect();
        for f in &dump.floats {
            panes.push(serde_json::json!({
                "id": f.id,
                "kind": self.main_tree.component_kind_of(f.id).unwrap_or(""),
                "place": "float",
                "focused": f.id == active,
                "fixed": false,
                "pinned": f.pinned,
                "z": f.z,
                "rect": { "x": f.x, "y": f.y, "w": f.w, "h": f.h },
            }));
        }
        helix_js::cache_panes(&serde_json::json!({ "panes": panes }).to_string());
        let json = serde_json::to_string(&dump).unwrap_or_default();
        helix_js::cache_layout(&json);
        // #47:layout-change 事件 —— 发在这里即覆盖**所有**布局变更
        // (`sync_layout_cache` 是唯一汇聚点;调用者虽多,函数体只有这一处)
        // 暂不携带"变更种类":种类需从 LayoutTree 逐层串上来(≈10 处),留作后续增量 ✓
        if let Err(e) = helix_js::emit_layout_change() {
            log::warn!("layout-change event failed: {e}");
        }
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
            if let Some(p) = layer
                .as_any_mut()
                .downcast_mut::<crate::ui::plugin_popup::PluginPopup>()
            {
                p.reset_render_state();
            }
            if let Some(p) = layer
                .as_any_mut()
                .downcast_mut::<crate::ui::plugin_terminal::PluginTerminal>()
            {
                p.reset_render_state();
            }
        }
        // 布局树里的面板叶子（split 出的面板 diff 同样需重置，否则测试向新 surface 渲染不重画）
        self.main_tree.reset_plugin_diffs();
    }

    /// 按类型统计层数量（测试/诊断）
    pub fn count_type(&self, type_name: &str) -> usize {
        self.layers
            .iter()
            .filter(|l| l.type_name() == type_name)
            .count()
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
