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

use crate::job::Jobs;
use crate::ui::picker;
use crate::ui::plugin_panel::{PanelSide, PluginPanel};
use helix_view::Editor;

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

pub struct Compositor {
    /// 瞬态覆盖层（弹窗/菜单/提示）——不参与布局，渲染在主区域之上
    layers: Vec<Box<dyn Component>>,
    /// 主区域布局树：编辑器/终端/面板都是叶子（tmux 式二分树）
    main_tree: crate::ui::layout::LayoutTree,
    area: Rect,

    pub(crate) last_picker: Option<Box<dyn Component>>,
    pub(crate) full_redraw: bool,
}

impl Compositor {
    pub fn new(area: Rect) -> Self {
        Self {
            main_tree: Default::default(),
            layers: Vec::new(),
            area,
            last_picker: None,
            full_redraw: false,
        }
    }

    pub fn size(&self) -> Rect {
        self.area
    }

    pub fn resize(&mut self, area: Rect) {
        self.area = area;
    }

    /// Add a layer to be rendered in front of all existing layers.
    pub fn push(&mut self, mut layer: Box<dyn Component>) {
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

    pub fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        // 主区域布局树：编辑器/终端/面板叶子各自在矩形里渲染
        self.main_tree.render(area, surface, cx);
        // 瞬态覆盖层（弹窗/菜单/提示）渲染在主区域之上
        for layer in &mut self.layers {
            layer.render(area, surface, cx);
        }
    }

    pub fn cursor(&self, area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        for layer in self.layers.iter().rev() {
            if let (Some(pos), kind) = layer.cursor(area, editor) {
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
        self.main_tree.split_side(active, dir, new_first, component)
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
        self.main_tree.split_side_ratio(active, dir, new_first, ratio, component, new_id)
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
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    pub fn remove_leaf(&mut self, id: u64) {
        self.main_tree.remove(id);
    }

    pub fn zoom_leaf(&mut self, id: u64) {
        self.main_tree.zoom(id);
    }

    pub fn unzoom(&mut self) {
        self.main_tree.unzoom();
    }

    pub fn resize_leaf(&mut self, id: u64, ratio: f32) {
        self.main_tree.resize(id, ratio);
    }

    pub fn focus_leaf(&mut self, id: u64) {
        self.main_tree.focus(id);
    }

    pub fn layout_tree(&mut self) -> &mut crate::ui::layout::LayoutTree {
        &mut self.main_tree
    }

    pub fn reset_plugin_diffs(&mut self) {
        for layer in &mut self.layers {
            if let Some(p) = layer.as_any_mut().downcast_mut::<crate::ui::PluginPanel>() {
                p.reset_render_state();
            }
            if let Some(p) = layer.as_any_mut().downcast_mut::<crate::ui::plugin_popup::PluginPopup>() {
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
