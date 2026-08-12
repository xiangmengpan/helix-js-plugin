//! 布局树：把编辑器主区域组织成 tmux 式二分树。
//! 叶子持有组件（编辑器/终端/面板），Split 节点切分区域。
//! 瞬态覆盖层（弹窗/菜单）不参与布局，仍由 Compositor 的 layers 管理。

use crate::compositor::{Component, Context, Event, EventResult};
use helix_view::graphics::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// 水平切分：左右两列
    H,
    /// 垂直切分：上下两行
    V,
}

#[derive(Debug)]
pub enum LayoutNode {
    Split {
        dir: SplitDir,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
    Leaf {
        id: u64,
    },
}

/// 布局树：二分树 + 叶子组件表 + 活动叶子 + 缩放状态。
pub struct LayoutTree {
    root: LayoutNode,
    components: std::collections::HashMap<u64, Box<dyn Component>>,
    active: u64,
    next_id: u64,
    /// 缩放中的叶子 id（占满全区，其他叶子隐藏）
    zoomed: Option<u64>,
}

/// 叶子布局结果：每个叶子的 id + Rect
type LeafRect = (u64, Rect);

fn layout_node(node: &LayoutNode, area: Rect, out: &mut Vec<LeafRect>) {
    match node {
        LayoutNode::Leaf { id } => out.push((*id, area)),
        LayoutNode::Split { dir, ratio, first, second } => {
            let ratio = ratio.clamp(0.05, 0.95);
            match dir {
                SplitDir::H => {
                    let w = (area.width as f32 * ratio) as u16;
                    layout_node(first, Rect::new(area.x, area.y, w, area.height), out);
                    layout_node(
                        second,
                        Rect::new(area.x + w, area.y, area.width.saturating_sub(w), area.height),
                        out,
                    );
                }
                SplitDir::V => {
                    let h = (area.height as f32 * ratio) as u16;
                    layout_node(first, Rect::new(area.x, area.y, area.width, h), out);
                    layout_node(
                        second,
                        Rect::new(area.x, area.y + h, area.width, area.height.saturating_sub(h)),
                        out,
                    );
                }
            }
        }
    }
}

impl Default for LayoutTree {
    fn default() -> Self {
        Self {
            root: LayoutNode::Leaf { id: 0 },
            components: Default::default(),
            active: 0,
            next_id: 1,
            zoomed: None,
        }
    }
}

impl LayoutTree {
    /// 设置主编辑器叶子（id=0，特殊：事件路由的兜底目标）
    pub fn set_editor(&mut self, component: Box<dyn Component>) {
        self.components.insert(0, component);
    }

    /// 活动叶子 id
    pub fn active(&self) -> u64 {
        self.zoomed.unwrap_or(self.active)
    }

    pub fn focus(&mut self, id: u64) {
        if self.components.contains_key(&id) {
            self.active = id;
        }
    }

    /// 把指定叶子切分为两个：原叶子（first）+ 新叶子（second，挂新组件）
    /// 返回新叶子 id
    pub fn split(
        &mut self,
        id: u64,
        dir: SplitDir,
        new_component: Box<dyn Component>,
    ) -> Option<u64> {
        self.split_side(id, dir, true, new_component)
    }

    pub fn next_id_for_split(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// 带侧向与比例的切分
    pub fn split_side_ratio(
        &mut self,
        id: u64,
        dir: SplitDir,
        new_first: bool,
        ratio: f32,
        new_component: Box<dyn Component>,
        new_id: u64,
    ) -> Option<u64> {
        if !self.components.contains_key(&id) {
            return None;
        }
        let (first, second) = if new_first { (new_id, id) } else { (id, new_id) };
        replace_leaf(&mut self.root, id, &|_| LayoutNode::Split {
            dir,
            ratio,
            first: Box::new(LayoutNode::Leaf { id: first }),
            second: Box::new(LayoutNode::Leaf { id: second }),
        });
        self.components.insert(new_id, new_component);
        self.active = new_id;
        Some(new_id)
    }

    /// 带侧向的切分：new_first=true 新叶子在 first（左/上），false 在 second（右/下）
    pub fn split_side(
        &mut self,
        id: u64,
        dir: SplitDir,
        new_first: bool,
        new_component: Box<dyn Component>,
    ) -> Option<u64> {
        if !self.components.contains_key(&id) {
            return None;
        }
        let new_id = self.next_id;
        self.next_id += 1;
        let (first, second) = if new_first {
            (new_id, id)
        } else {
            (id, new_id)
        };
        replace_leaf(&mut self.root, id, &|_| LayoutNode::Split {
            dir,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf { id: first }),
            second: Box::new(LayoutNode::Leaf { id: second }),
        });
        self.components.insert(new_id, new_component);
        self.active = new_id;
        Some(new_id)
    }

    /// 移除叶子：其父 Split 收缩为兄弟子树；组件随之销毁（Drop）
    pub fn remove(&mut self, id: u64) {
        if id == 0 {
            return; // 编辑器叶子不可移除
        }
        self.components.remove(&id);
        prune(&mut self.root, id);
        if self.active == id {
            self.active = 0;
        }
        if self.zoomed == Some(id) {
            self.zoomed = None;
        }
    }

    /// 缩放：叶子占满全区（其他叶子隐藏）；再次调用取消
    pub fn zoom(&mut self, id: u64) {
        if self.components.contains_key(&id) {
            self.zoomed = Some(id);
        }
    }

    pub fn unzoom(&mut self) {
        self.zoomed = None;
    }

    pub fn is_zoomed(&self) -> bool {
        self.zoomed.is_some()
    }

    /// 调整包含指定叶子的 Split 的分界比例（叶子在 first 侧则调该侧）
    pub fn resize(&mut self, id: u64, ratio: f32) {
        adjust_ratio(&mut self.root, id, ratio);
    }

    /// 是否有某类型组件（叶子内）
    pub fn has_component(&self, type_name: &str) -> bool {
        self.components
            .values()
            .any(|c| c.type_name() == type_name)
    }

    /// 某类型组件数量
    pub fn count_type(&self, type_name: &str) -> usize {
        self.components
            .values()
            .filter(|c| c.type_name() == type_name)
            .count()
    }

    /// 序列化布局（不包含组件内容，只含叶子 id 与类型标记）
    pub fn dump(&self) -> LayoutDump {
        fn dump_node(node: &LayoutNode) -> serde_json::Value {
            match node {
                LayoutNode::Leaf { id } => {
                    serde_json::json!({ "type": "leaf", "id": id })
                }
                LayoutNode::Split { dir, ratio, first, second } => serde_json::json!({
                    "type": "split",
                    "dir": match dir { SplitDir::H => "h", SplitDir::V => "v" },
                    "ratio": ratio,
                    "first": dump_node(first),
                    "second": dump_node(second),
                }),
            }
        }
        LayoutDump {
            tree: dump_node(&self.root),
            active: self.active,
            zoomed: self.zoomed,
        }
    }

    /// 渲染：每个叶子在自己的矩形里渲染组件；缩放时只有被缩放的叶子渲染
    pub fn render(&mut self, area: Rect, surface: &mut tui::buffer::Buffer, cx: &mut Context) {
        if let Some(zoomed) = self.zoomed {
            if let Some(comp) = self.components.get_mut(&zoomed) {
                comp.render(area, surface, cx);
            }
            return;
        }
        let mut rects = Vec::new();
        layout_node(&self.root, area, &mut rects);
        for (id, rect) in rects {
            if let Some(comp) = self.components.get_mut(&id) {
                comp.render(rect, surface, cx);
            }
        }
    }

    /// 事件路由：活动叶子优先；Ignored → 编辑器叶子（id=0）兜底
    pub fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let target = self.active();
        if let Some(comp) = self.components.get_mut(&target) {
            match comp.handle_event(event, cx) {
                EventResult::Ignored(cb) if target != 0 => {
                    // 非编辑器叶子忽略 → 交给编辑器
                    if let Some(editor) = self.components.get_mut(&0) {
                        let r = editor.handle_event(event, cx);
                        return if r.is_ignored() {
                            EventResult::Ignored(cb)
                        } else {
                            r
                        };
                    }
                    EventResult::Ignored(cb)
                }
                r => r,
            }
        } else {
            EventResult::Ignored(None)
        }
    }

    /// 活动叶子的组件（可变）
    pub fn active_component(&mut self) -> Option<&mut Box<dyn Component>> {
        self.components.get_mut(&self.active())
    }

    /// 按类型找组件（树内所有叶子）
    pub fn find_component<T: 'static>(&mut self) -> Option<&mut T> {
        self.components
            .values_mut()
            .find_map(|c| c.as_any_mut().downcast_mut::<T>())
    }

    /// 按谓词找组件
    pub fn find_component_where<T: 'static>(
        &mut self,
        mut f: impl FnMut(&T) -> bool,
    ) -> Option<&mut T> {
        self.components
            .values_mut()
            .find_map(|c| c.as_any_mut().downcast_mut::<T>().filter(|t| f(t)))
    }

    /// 按类型移除组件（返回是否移除）
    pub fn remove_component_type<T: 'static>(&mut self) -> bool {
        let ids: Vec<u64> = self
            .components
            .iter()
            .filter(|(_, c)| c.as_any().downcast_ref::<T>().is_some())
            .map(|(id, _)| *id)
            .collect();
        let mut removed = false;
        for id in ids {
            if id != 0 {
                self.remove(id);
                removed = true;
            }
        }
        removed
    }



}

/// 把目标叶子替换为新节点（split 用）
fn replace_leaf(
    node: &mut LayoutNode,
    target: u64,
    f: &dyn Fn(&mut LayoutNode) -> LayoutNode,
) {
    match node {
        LayoutNode::Leaf { id } => {
            if *id == target {
                *node = f(node);
            }
        }
        LayoutNode::Split { first, second, .. } => {
            replace_leaf(first, target, f);
            replace_leaf(second, target, f);
        }
    }
}

/// 调整包含目标叶子的 Split 比例
fn adjust_ratio(node: &mut LayoutNode, target: u64, ratio: f32) {
    if let LayoutNode::Split { ratio: r, first, second, .. } = node {
        if contains_leaf(first, target) || contains_leaf(second, target) {
            *r = ratio.clamp(0.05, 0.95);
        }
        adjust_ratio(first, target, ratio);
        adjust_ratio(second, target, ratio);
    }
}

/// 从树中剪除目标叶子：其父 Split 收缩为兄弟子树。返回 true 表示该节点整体应被移除。
fn prune(node: &mut LayoutNode, target: u64) -> bool {
    match node {
        LayoutNode::Leaf { id } => *id == target,
        LayoutNode::Split { first, second, .. } => {
            if matches!(&**first, LayoutNode::Leaf { id } if *id == target) {
                let sibling = std::mem::replace(&mut **second, LayoutNode::Leaf { id: u64::MAX });
                *node = sibling;
                false
            } else if matches!(&**second, LayoutNode::Leaf { id } if *id == target) {
                let sibling = std::mem::replace(&mut **first, LayoutNode::Leaf { id: u64::MAX });
                *node = sibling;
                false
            } else if prune(first, target) || prune(second, target) {
                false // 一侧整体被移除（单子树退化），保持另一侧
            } else {
                false
            }
        }
    }
}

fn contains_leaf(node: &LayoutNode, target: u64) -> bool {
    match node {
        LayoutNode::Leaf { id } => *id == target,
        LayoutNode::Split { first, second, .. } => {
            contains_leaf(first, target) || contains_leaf(second, target)
        }
    }
}

/// 布局序列化结果（dump/restore 用）
#[derive(Clone)]
pub struct LayoutDump {
    pub tree: serde_json::Value,
    pub active: u64,
    pub zoomed: Option<u64>,
}

pub(crate) trait EventResultExt {
    fn is_ignored(&self) -> bool;
}
impl EventResultExt for EventResult {
    fn is_ignored(&self) -> bool {
        matches!(self, EventResult::Ignored(_))
    }
}
