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
        /// 容器（组）id。与叶子共用同一个 id 空间 ⇒ 任何 u64 只可能是二者之一
        /// （`LayoutTree::node_kind` 判）。有了它，组才能被当成**切分/操作目标**：
        /// 在组上切一刀，新 pane 就跨满该组的整宽/整高（"地板的 term 横跨左右两个编辑区"）。
        id: u64,
        dir: SplitDir,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
    Leaf {
        id: u64,
    },
}

/// 组 id 的起点：叶子 id 由 `next_id` 从 1 递增分配，调用方还可能显式传 id
/// （`split_side_ratio(..., new_id)`），所以组**必须**用一段叶子碰不到的号段，
/// 否则"叶子 1 / 组 1"撞车会让 `find_node` 二义。1<<40 远在叶子增长之外，
/// 同时仍能精确放进 JS 的 double（< 2^53）。
pub const GROUP_ID_BASE: u64 = 1 << 40;

/// 一个 id 在树里是什么（不存在的 id → None）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// 叶子：持有组件（编辑器/终端/面板）
    Leaf,
    /// 组（容器节点）：只占区域，不持有组件；可作为切分/equalize/minimize 的目标
    Group,
}

/// 建一个组节点（省掉每处都写 Box::new）
fn group(id: u64, dir: SplitDir, ratio: f32, first: LayoutNode, second: LayoutNode) -> LayoutNode {
    LayoutNode::Split {
        id,
        dir,
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    }
}

/// 布局树：二分树 + 叶子组件表 + 活动叶子 + 缩放状态。
pub struct LayoutTree {
    root: LayoutNode,
    components: std::collections::HashMap<u64, Box<dyn Component>>,
    active: u64,
    next_id: u64,
    /// 下一个组 id（从 [`GROUP_ID_BASE`] 起；与叶子 id 不共享号段）
    next_group: u64,
    /// 缩放中的叶子 id（占满全区，其他叶子隐藏）
    zoomed: Option<u64>,
    /// 浮动叶子 id（终端 Floating 模式：不占 split 布局，渲染在视口中央浮窗，最上层）
    /// 浮动 pane 槽位(阶段②):多窗 + z 序 + 比例几何。
    /// 旧行为(终端 Floating 模式)= 只有一项、比例居中。
    floats: Vec<FloatSlot>,
    /// 堆叠组(阶段②)。key = 锚成员 id(= `members[0]` = 当前显示的那个)。
    /// 不变量:锚持有槽位且**就是 shown**;循环 = 轮转 `members`;成员必须是**兄弟**
    /// (否则被排除的成员不会把空间让给锚,轮转会跳位置 —— 见规格 A.7)。
    stacks: std::collections::HashMap<u64, StackGroup>,
    /// 最小化的**节点** id（叶子或组）：被最小化的子树不占布局，渲染为底部一条
    /// 标题横条；可同时有多个（按提交顺序从下往上叠）。
    minimized: Vec<u64>,
    /// 最近一次渲染的叶子区域（鼠标命中查询；每帧渲染更新）
    leaf_rects: std::collections::HashMap<u64, Rect>,
    /// 最近一次渲染的**组**区域（容器节点的包围盒；`pane.list()` / JS 读）。
    /// 组不占组件表，所以几何只能这样带出来。
    group_rects: std::collections::HashMap<u64, Rect>,
    /// 同上，但只在几何**变化**时置位（Compositor 用它决定要不要刷新 `pane.list()` 快照）
    rects_dirty: bool,
    /// 固定叶子 id 集合（fixed：不被模式操作 swap/resize/close/minimize/equalize，可被焦点穿过）
    fixed: std::collections::HashSet<u64>,
    /// 底部停靠叶(`register_dock_bottom`)：恒为 root 的直接子叶（V 分屏的下半，占全宽）。
    dock: Option<u64>,
    /// rail(侧栏轨道)叶子:恒为 root 的直接子叶(H 分屏边缘侧,占全高)。
    /// 不参与分裂/换位/最小化/放大等窗口操作;窗口操作只发生在 main 子树。
    rail: Option<u64>,
}

/// 堆叠组(阶段②)。
///
/// 不变量:`members[0]` 是**锚** —— 它持有树里的槽位,且它就是当前显示的那个;
/// `members[1..]` 被排除出布局(它们的位置空间由锚吸收),但它们**仍各自持有树位置**,
/// 所以解散/关闭时不需要重定键、也不会产生孤儿组件。
///
/// **成员必须是兄弟**(同一父 Split 的两个子):只有这样,排除一个才会让另一个占满
/// 同一块区域,循环时 rect 不变 —— 零映射的前提(规格 A.7)。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StackGroup {
    pub members: Vec<u64>,
}

impl StackGroup {
    pub fn anchor(&self) -> Option<u64> {
        self.members.first().copied()
    }
}

/// 浮动 pane 槽位(阶段②)。
///
/// 几何用**视口比例**(0..1)而不是绝对格:窗口改尺寸时浮窗跟着缩放,
/// 不会跑到视口外。旧行为(单个居中浮窗)= `x:0.2, y:0.15, w:0.6, h:0.7`
/// (与 `float_rect` 的 60%×70% 居中一致)。
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FloatSlot {
    pub id: u64,
    /// 左上角比例(相对视口)
    pub x: f32,
    pub y: f32,
    /// 宽高比例(相对视口)
    pub w: f32,
    pub h: f32,
    /// z 序:大者在上(渲染与事件路由都按降序)
    pub z: i32,
    /// pinned:置顶(上层语义,渲染只看 z)
    pub pinned: bool,
}

impl FloatSlot {
    /// 默认居中槽位(60%×70%,带旧实现的 40×10 最小尺寸)
    pub fn centered(id: u64) -> Self {
        Self {
            id,
            x: 0.2,
            y: 0.15,
            w: 0.6,
            h: 0.7,
            z: 0,
            pinned: false,
        }
    }

    /// 比例 → 实际矩形(与旧 `float_rect` 的尺寸约束一致:宽 ≥40、高 ≥10,不越视口)
    pub fn rect_of(&self, area: Rect) -> Rect {
        let max_w = area.width.max(1);
        let max_h = area.height.max(1);
        let w = ((area.width as f32 * self.w).round() as u16).clamp(40.min(max_w), max_w);
        let h = ((area.height as f32 * self.h).round() as u16).clamp(10.min(max_h), max_h);
        let x = (area.x as f32 + area.width as f32 * self.x).round() as i32;
        let y = (area.y as f32 + area.height as f32 * self.y).round() as i32;
        // 夹进视口(改尺寸/移动后不会跑出去)
        let x = x.clamp(area.x as i32, (area.x + max_w).saturating_sub(w) as i32) as u16;
        let y = y.clamp(area.y as i32, (area.y + max_h).saturating_sub(h) as i32) as u16;
        Rect::new(x, y, w, h)
    }
}

/// 叶子布局结果：每个叶子的 id + Rect
/// skip/minimized = 排除布局空间的叶子 id（float 浮窗 / minimized 横条）：
/// 其所在子树整体让位，其余叶子占满区域
type LeafRect = (u64, Rect);

/// 节点是否整体让位（不占布局空间）：自身 id 在排除集合里，或子树里所有叶子都排除。
fn node_excluded(node: &LayoutNode, excluded: &[u64]) -> bool {
    match node {
        LayoutNode::Leaf { id } => excluded.contains(id),
        LayoutNode::Split {
            id, first, second, ..
        } => {
            excluded.contains(id)
                || (node_excluded(first, excluded) && node_excluded(second, excluded))
        }
    }
}

fn layout_node(
    node: &LayoutNode,
    area: Rect,
    out: &mut Vec<LeafRect>,
    groups: &mut Vec<LeafRect>,
    excluded: &[u64],
) {
    if node_excluded(node, excluded) {
        return; // 整棵子树让位（浮窗 / 堆叠非锚成员 / 被最小化的叶子或组）
    }
    match node {
        LayoutNode::Leaf { id } => {
            out.push((*id, area));
        }
        LayoutNode::Split {
            id,
            dir,
            ratio,
            first,
            second,
        } => {
            // 组的矩形 = 它管的那块区域（切分之前），JS/`pane.list()` 读这个
            groups.push((*id, area));
            // 一侧整棵让位 → 另一侧占满本区域（否则会留一块空白）
            let (first_gone, second_gone) = (
                node_excluded(first, excluded),
                node_excluded(second, excluded),
            );
            match (first_gone, second_gone) {
                (true, true) => return,
                (true, false) => return layout_node(second, area, out, groups, excluded),
                (false, true) => return layout_node(first, area, out, groups, excluded),
                _ => {}
            }
            let ratio = ratio.clamp(0.05, 0.95);
            match dir {
                SplitDir::H => {
                    let w = (area.width as f32 * ratio) as u16;
                    layout_node(
                        first,
                        Rect::new(area.x, area.y, w, area.height),
                        out,
                        groups,
                        excluded,
                    );
                    layout_node(
                        second,
                        Rect::new(
                            area.x + w,
                            area.y,
                            area.width.saturating_sub(w),
                            area.height,
                        ),
                        out,
                        groups,
                        excluded,
                    );
                }
                SplitDir::V => {
                    let h = (area.height as f32 * ratio) as u16;
                    layout_node(
                        first,
                        Rect::new(area.x, area.y, area.width, h),
                        out,
                        groups,
                        excluded,
                    );
                    layout_node(
                        second,
                        Rect::new(
                            area.x,
                            area.y + h,
                            area.width,
                            area.height.saturating_sub(h),
                        ),
                        out,
                        groups,
                        excluded,
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
            next_group: GROUP_ID_BASE,
            zoomed: None,
            floats: Vec::new(),
            stacks: std::collections::HashMap::new(),
            minimized: Vec::new(),
            fixed: Default::default(),
            dock: None,
            rail: None,
            leaf_rects: Default::default(),
            group_rects: Default::default(),
            rects_dirty: false,
        }
    }
}

impl LayoutTree {
    /// 设置/取消叶子的 fixed 标记（fixed 叶子不被 swap/resize/remove/minimize/equalize 操作）
    pub fn set_fixed(&mut self, id: u64, fixed: bool) {
        if fixed {
            self.fixed.insert(id);
        } else {
            self.fixed.remove(&id);
        }
    }

    /// 叶子是否 fixed
    pub fn is_fixed(&self, id: u64) -> bool {
        self.fixed.contains(&id)
    }

    /// 当前 rail 叶子(无 → None)
    pub fn rail_leaf(&self) -> Option<u64> {
        self.rail
    }

    /// id 是否为 rail
    pub fn is_rail(&self, id: u64) -> bool {
        self.rail == Some(id)
    }

    /// 把组件**停靠到底部**：整棵树包成上下两半（底部那半是新叶）。
    ///
    /// 与 `register_rail` 对称：rail 是"贴边的全高列"，dock 是"贴底的全宽行"。
    /// 为什么需要它：面板默认是**在当前活动叶上分裂**的 —— 焦点在右栏时开底部，
    /// 底部只会裂在编辑器那一列里（半宽），视觉上很怪。
    pub fn register_dock_bottom(&mut self, component: Box<dyn Component>, share: f32) -> u64 {
        let did = self.next_id_for_split();
        let gid = self.next_group_id();
        self.components.insert(did, component);
        let main_root = std::mem::replace(&mut self.root, LayoutNode::Leaf { id: did });
        self.root = group(
            gid,
            SplitDir::V,
            // ratio = **第一个子**（上面的 main）占的比例
            (1.0 - share).clamp(0.1, 0.95),
            main_root,
            LayoutNode::Leaf { id: did },
        );
        self.dock = Some(did);
        self.active = did;
        did
    }

    /// 这个叶是不是"底部停靠"。
    pub fn is_dock(&self, id: u64) -> bool {
        self.dock == Some(id)
    }

    /// 把组件注册为 rail(贴 root 边缘):先取除旧 rail(若有),再把当前 root 包成 main 子树。
    /// ratio=rail 占宽份额(0<ratio<1)。返回被替换的旧 rail id(组件已移除)。
    pub fn register_rail(
        &mut self,
        component: Box<dyn Component>,
        left: bool,
        ratio: f32,
    ) -> Option<u64> {
        let old = self.take_rail();
        let rid = self.next_id_for_split();
        let gid = self.next_group_id();
        self.components.insert(rid, component);
        let main_root = std::mem::replace(&mut self.root, LayoutNode::Leaf { id: rid });
        self.root = if left {
            group(
                gid,
                SplitDir::H,
                ratio,
                LayoutNode::Leaf { id: rid },
                main_root,
            )
        } else {
            group(
                gid,
                SplitDir::H,
                1.0 - ratio,
                main_root,
                LayoutNode::Leaf { id: rid },
            )
        };
        self.rail = Some(rid);
        // 注册即聚焦 rail(与普通面板打开后活动一致:按键直达浏览;Esc/l 或 C-\ 回 main)
        self.active = rid;
        old
    }

    /// 取除 rail:root 收缩回 main 子树;rail 组件移除。返回原 rail id。
    pub fn take_rail(&mut self) -> Option<u64> {
        let rid = self.rail.take()?;
        if self.components.contains_key(&rid) {
            self.components.remove(&rid);
            prune(&mut self.root, rid);
        }
        if self.active == rid {
            self.active = 0;
        }
        Some(rid)
    }

    /// 取出 rail 组件(**不丢弃**),用于换边后重新注册。
    /// 与 `take_rail` 的差别:后者移除并丢弃组件。
    pub fn take_rail_component(&mut self) -> Option<Box<dyn Component>> {
        let rid = self.rail.take()?;
        let comp = self.components.remove(&rid);
        if comp.is_some() {
            prune(&mut self.root, rid);
        }
        if self.active == rid {
            self.active = 0;
        }
        comp
    }

    /// 当前 rail **自身**的宽度比例(无 rail → None)。换边时保住宽度用。
    ///
    /// 注意 `Split.ratio` 是"**第一个子**的比例",所以 rail 在 second 侧时要取 `1 - ratio`
    /// —— 否则换边后读到的会是编辑器那一侧的比例(实测踩过:0.83 变成 0.17)。
    pub fn rail_ratio(&self) -> Option<f32> {
        let rid = self.rail?;
        match &self.root {
            LayoutNode::Split {
                ratio,
                first,
                second,
                ..
            } => {
                if matches!(&**first, LayoutNode::Leaf { id } if *id == rid) {
                    Some(*ratio)
                } else if matches!(&**second, LayoutNode::Leaf { id } if *id == rid) {
                    Some(1.0 - *ratio)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// main 子树的首个叶(跳过 rail);退化(整树只有 rail)回退 0
    fn main_first_leaf(&self) -> u64 {
        match &self.root {
            LayoutNode::Leaf { id } => {
                if self.rail == Some(*id) {
                    0
                } else {
                    *id
                }
            }
            LayoutNode::Split { first, second, .. } => first_leaf_skipping(first, self.rail)
                .or_else(|| first_leaf_skipping(second, self.rail))
                .unwrap_or(0),
        }
    }

    /// 活动叶若为 rail,分裂/操作落点 = main 首个叶
    pub fn operational_target(&self) -> u64 {
        if self.rail == Some(self.active) {
            self.main_first_leaf()
        } else {
            self.active
        }
    }

    // ── 堆叠(阶段②)──────────────────────────────────────────

    /// 该 id 所属堆叠组的锚(不在组里 → None)
    pub fn stack_anchor_of(&self, id: u64) -> Option<u64> {
        self.stacks
            .iter()
            .find(|(_, g)| g.members.contains(&id))
            .map(|(a, _)| *a)
    }

    /// 组内成员(不在组里 → 只含它自己)
    pub fn stack_members(&self, id: u64) -> Vec<u64> {
        match self.stack_anchor_of(id).and_then(|a| self.stacks.get(&a)) {
            Some(g) => g.members.clone(),
            None => vec![id],
        }
    }

    pub fn is_stacked(&self, id: u64) -> bool {
        self.stack_anchor_of(id).is_some()
    }

    /// `id` 的最小父 Split 里的**叶子**兄弟。堆叠 v1 只支持兄弟合并(A.7)。
    pub fn leaf_sibling_of(&self, id: u64) -> Option<u64> {
        fn go(node: &LayoutNode, id: u64) -> Option<u64> {
            let LayoutNode::Split { first, second, .. } = node else {
                return None;
            };
            let first_is = matches!(&**first, LayoutNode::Leaf { id: x } if *x == id);
            let second_is = matches!(&**second, LayoutNode::Leaf { id: x } if *x == id);
            if first_is {
                if let LayoutNode::Leaf { id: sib } = &**second {
                    return Some(*sib);
                }
            }
            if second_is {
                if let LayoutNode::Leaf { id: sib } = &**first {
                    return Some(*sib);
                }
            }
            go(first, id).or_else(|| go(second, id))
        }
        go(&self.root, id)
    }

    /// 建堆叠组:`id` 与其叶子兄弟组成 `[id, sibling]`(锚 = `id`)。
    /// 任一方已在组里 → 拒绝(避免把两个区域的成员塞进同一组,破坏不变量)。
    pub fn stack_new_with_sibling(&mut self, id: u64) -> Option<u64> {
        if !self.components.contains_key(&id) || self.is_stacked(id) {
            return None;
        }
        let sib = self.leaf_sibling_of(id)?;
        if self.is_stacked(sib) {
            return None;
        }
        self.stacks.insert(
            id,
            StackGroup {
                members: vec![id, sib],
            },
        );
        Some(id)
    }

    /// 轮转组内成员:新的 shown 成为新锚(换 key)。返回新锚。
    /// 兄弟成员覆盖同一区域,所以换 key 后 rect 不变 —— 零映射的前提。
    pub fn stack_rotate(&mut self, id: u64, forward: bool) -> Option<u64> {
        let anchor = self.stack_anchor_of(id)?;
        let mut g = self.stacks.remove(&anchor)?;
        if g.members.len() < 2 {
            self.stacks.insert(anchor, g);
            return None;
        }
        if forward {
            g.members.rotate_left(1);
        } else {
            g.members.rotate_right(1);
        }
        let new_anchor = g.members[0];
        self.stacks.insert(new_anchor, g);
        Some(new_anchor)
    }

    /// 从堆叠组里摘掉一个成员(关闭叶子时调用)。
    /// 锚被摘→换 key 到新首位;剩 1 个→解散(该成员恢复为普通 pane)。
    ///
    /// 注:在 v1 的**兄弟约束**下,一个 Split 只有两个子,所以组恒为 2 人 ——
    /// 摘任意一个都走"解散"分支;下面的换 key 分支是为将来支持 >2 成员留的(当前不可达)。
    pub fn stack_remove_member(&mut self, id: u64) {
        let anchors: Vec<u64> = self.stacks.keys().copied().collect();
        for a in anchors {
            let pos = self
                .stacks
                .get(&a)
                .and_then(|g| g.members.iter().position(|m| *m == id));
            let Some(pos) = pos else { continue };
            if let Some(g) = self.stacks.get_mut(&a) {
                g.members.remove(pos);
            }
            let len = self.stacks.get(&a).map(|g| g.members.len()).unwrap_or(0);
            if len <= 1 {
                self.stacks.remove(&a);
            } else if pos == 0 {
                if let Some(g) = self.stacks.remove(&a) {
                    let new_anchor = g.members[0];
                    self.stacks.insert(new_anchor, g);
                }
            }
            break;
        }
    }

    /// 不参与布局空间的 id:浮窗 + 堆叠组里**非锚**的成员 + 被最小化的节点(叶子或组)。
    ///
    /// (非锚成员的位置空间由锚吸收 —— 所以堆叠始终占满同一块区域;
    ///  最小化的**组**同理:它的整块区域由兄弟占满。)
    pub fn layout_skip_ids(&self) -> Vec<u64> {
        let mut v = self.float_ids();
        for g in self.stacks.values() {
            v.extend(g.members.iter().skip(1).copied());
        }
        v.extend(self.minimized.iter().copied());
        v
    }

    /// 最上层浮动叶子的 id(单浮窗时代的 `floating()` 语义)
    pub fn floating(&self) -> Option<u64> {
        self.floats.iter().max_by_key(|f| f.z).map(|f| f.id)
    }

    /// 全部浮动槽位(按 z 升序)
    pub fn floats(&self) -> Vec<FloatSlot> {
        let mut v = self.floats.clone();
        v.sort_by_key(|f| f.z);
        v
    }

    fn float_ids(&self) -> Vec<u64> {
        self.floats.iter().map(|f| f.id).collect()
    }

    /// 把叶子设为浮动(默认居中槽位;旧单浮窗语义)。仅当组件存在。
    pub fn set_float(&mut self, id: u64) {
        if self.components.contains_key(&id) {
            self.floats.retain(|f| f.id != id);
            self.floats.push(FloatSlot::centered(id));
            self.active = id;
        }
    }

    /// 按显式槽位加/替换一个浮动 pane(阶段② 多窗)
    pub fn add_float(&mut self, slot: FloatSlot) {
        if self.components.contains_key(&slot.id) {
            self.floats.retain(|f| f.id != slot.id);
            self.floats.push(slot);
        }
    }

    /// 取槽位可变引用(移动/缩放/pin 用)
    pub fn float_slot_mut(&mut self, id: u64) -> Option<&mut FloatSlot> {
        self.floats.iter_mut().find(|f| f.id == id)
    }

    /// 提到最上层(取当前最大 z + 1)
    pub fn float_raise(&mut self, id: u64) {
        let top = self.floats.iter().map(|f| f.z).max().unwrap_or(0);
        if let Some(f) = self.float_slot_mut(id) {
            f.z = top + 1;
        }
    }

    /// 关闭单个浮动槽位(组件保留,回到其 split 位置)
    pub fn remove_float(&mut self, id: u64) {
        self.floats.retain(|f| f.id != id);
    }

    /// 取消全部浮动(兼容旧 `unfloat()`;叶子回到各自 split 位置)
    pub fn unfloat(&mut self) {
        self.floats.clear();
    }

    /// 该叶子当前是否浮动
    pub fn is_leaf_floating(&self, id: u64) -> bool {
        self.floats.iter().any(|f| f.id == id)
    }

    /// 切换浮动态:浮动 → 回平铺;平铺 → 浮动(默认居中槽位)。
    /// zellij 的 `w`(ToggleFloating)/`e`(EmbedOrFloating)落到这里。
    pub fn float_toggle(&mut self, id: u64) {
        if self.is_leaf_floating(id) {
            self.remove_float(id);
        } else {
            self.set_float(id);
        }
    }

    /// 浮动 pane 按比例平移(增量为视口比例;夹在视口内)
    pub fn float_nudge(&mut self, id: u64, dx: f32, dy: f32) {
        if let Some(f) = self.float_slot_mut(id) {
            f.x = (f.x + dx).clamp(0.0, (1.0 - f.w).max(0.0));
            f.y = (f.y + dy).clamp(0.0, (1.0 - f.h).max(0.0));
        }
    }

    /// 浮动 pane 按比例改尺寸(增量为视口比例;不小于 10%,不越视口)
    pub fn float_scale(&mut self, id: u64, dw: f32, dh: f32) {
        if let Some(f) = self.float_slot_mut(id) {
            f.w = (f.w + dw).clamp(0.1, (1.0 - f.x).max(0.1));
            f.h = (f.h + dh).clamp(0.1, (1.0 - f.y).max(0.1));
        }
    }

    pub fn float_is_pinned(&self, id: u64) -> bool {
        self.floats
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.pinned)
            .unwrap_or(false)
    }

    /// pin 开关(zellij 的 `i`):置顶且不被焦点变化降级
    pub fn float_set_pinned(&mut self, id: u64, pinned: bool) {
        let top = self.floats.iter().map(|f| f.z).max().unwrap_or(0);
        if let Some(f) = self.float_slot_mut(id) {
            f.pinned = pinned;
            if pinned {
                f.z = top + 1;
            }
        }
    }

    pub fn is_float(&self) -> bool {
        !self.floats.is_empty()
    }

    /// 浮动浮窗矩形：视口居中，宽 60%、高 70%（带边框），最小 40×10
    pub fn float_rect(area: Rect) -> Rect {
        let w = (area.width as f32 * 0.6)
            .round()
            .clamp(40.0, area.width.max(1) as f32) as u16;
        let h = (area.height as f32 * 0.7)
            .round()
            .clamp(10.0, area.height.max(1) as f32) as u16;
        Rect::new(
            area.x + (area.width - w) / 2,
            area.y + (area.height - h) / 2,
            w,
            h,
        )
    }

    /// 按组件类型 + 谓词找叶子 id（终端 view_id → 叶子 id 映射用）
    pub fn find_leaf_id<T: 'static>(&self, mut f: impl FnMut(&T) -> bool) -> Option<u64> {
        self.components
            .iter()
            .find_map(|(id, c)| c.as_any().downcast_ref::<T>().filter(|t| f(t)).map(|_| *id))
    }
    /// 设置主编辑器叶子（id=0，特殊：事件路由的兜底目标）
    pub fn set_editor(&mut self, component: Box<dyn Component>) {
        self.components.insert(0, component);
    }

    /// 活动叶子 id。
    ///
    /// zoom 时返回**真正能收事件的那个叶子**：叶子 zoom → 它本身；组 zoom → 组内
    /// 当前聚焦的那个（不在组里就取组内第一个叶子）。
    pub fn active(&self) -> u64 {
        match self.zoomed {
            None => self.active,
            Some(z) => match self.node_kind(z) {
                Some(NodeKind::Leaf) => z,
                Some(NodeKind::Group) => {
                    if self.active != z && node_contains(&self.root, z, self.active) {
                        self.active
                    } else {
                        find_node(&self.root, z)
                            .and_then(first_leaf_of)
                            .unwrap_or(self.active)
                    }
                }
                None => self.active,
            },
        }
    }

    /// 全部叶子 id(树中序:左/上优先)。用于「切到下一个窗口」(Pane 模式 p/Tab)。
    pub fn leaf_ids(&self) -> Vec<u64> {
        self.leaf_ids_of(&self.root)
    }

    /// 某叶子的组件类型名(取末段,如 `PluginTerminal`/`PluginPanel`/`EditorView`)。
    /// 给 `helix.pane.list()` 的 `kind` 字段用 —— 插件拿到 id 得能区分是哪类 pane。
    pub fn component_kind_of(&self, id: u64) -> Option<&'static str> {
        self.components.get(&id).map(|c| {
            let t = c.type_name();
            t.rsplit("::").next().unwrap_or(t)
        })
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

    /// 分配一个**组**（容器节点）id（独立号段，绝不与叶子撞）
    fn next_group_id(&mut self) -> u64 {
        let id = self.next_group;
        self.next_group = self.next_group.saturating_add(1);
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
        // rail 不参与分裂:目标落 main
        let id = if self.rail == Some(id) {
            self.main_first_leaf()
        } else {
            id
        };
        let (first, second) = if new_first {
            (new_id, id)
        } else {
            (id, new_id)
        };
        let gid = self.next_group_id();
        replace_leaf(&mut self.root, id, &|_| {
            group(
                gid,
                dir,
                ratio,
                LayoutNode::Leaf { id: first },
                LayoutNode::Leaf { id: second },
            )
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
        // rail 不参与分裂:目标落 main
        let id = if self.rail == Some(id) {
            self.main_first_leaf()
        } else {
            id
        };
        let new_id = self.next_id;
        self.next_id += 1;
        let (first, second) = if new_first {
            (new_id, id)
        } else {
            (id, new_id)
        };
        let gid = self.next_group_id();
        replace_leaf(&mut self.root, id, &|_| {
            group(
                gid,
                dir,
                0.5,
                LayoutNode::Leaf { id: first },
                LayoutNode::Leaf { id: second },
            )
        });
        self.components.insert(new_id, new_component);
        self.active = new_id;
        Some(new_id)
    }

    /// 移除叶子：其父 Split 收缩为兄弟子树；组件随之销毁（Drop）。
    /// fixed 叶子 / 编辑器叶子(id=0) / 不存在的 id → false，不操作。
    pub fn remove(&mut self, id: u64) -> bool {
        if id == 0
            || self.fixed.contains(&id)
            || self.rail == Some(id)
            || !self.components.contains_key(&id)
        {
            return false; // 编辑器叶子不可移除；fixed/rail 免疫
        }
        self.components.remove(&id);
        prune(&mut self.root, id);
        if self.active == id {
            self.active = 0;
        }
        if self.zoomed == Some(id) {
            self.zoomed = None;
        }
        // 关闭浮动 pane：残留槽位会让事件路由指向已删叶子 → 全部 Ignored → 程序僵死
        self.floats.retain(|f| f.id != id);
        // 堆叠：摘掉成员；锚被摘则换 key，剩 1 个则解散（该成员恢复为普通 pane）
        self.stack_remove_member(id);
        // 最小化条目：被关掉的叶子、以及已经空了的组条目都清掉
        self.minimized
            .retain(|m| *m != id && find_node(&self.root, *m).is_some());
        true
    }

    /// 缩放：叶子占满全区（其他叶子隐藏）；再次调用取消
    /// 缩放：叶子占满全区；**组**占满全区（组内各叶子按原分割铺开）。
    /// rail 不参与（它恒为贴边列）。
    pub fn zoom(&mut self, id: u64) {
        if self.rail == Some(id) {
            return;
        }
        if self.node_kind(id).is_some() {
            self.zoomed = Some(id);
        }
    }

    /// 缩放中的叶子 id（None = 未缩放）
    pub fn zoomed_leaf(&self) -> Option<u64> {
        self.zoomed
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

    /// 让某个**节点**（组或叶子）在它的**直接父分割**里占 `share` 份额（0<share<1）。
    ///
    /// 与 [`Self::resize`] 的差别：后者会改路径上**所有**祖先的分界（老行为，留给叶子
    /// 兼容）；这个只动紧挨着它的那条分界 —— "把这一排窗口调大/调小"的语义，
    /// 也是组唯一讲得通的 resize（组没有"叶子比例"）。
    pub fn resize_share(&mut self, id: u64, share: f32) -> bool {
        fn set(node: &mut LayoutNode, id: u64, share: f32) -> bool {
            let LayoutNode::Split {
                ratio,
                first,
                second,
                ..
            } = node
            else {
                return false;
            };
            if node_id(first) == id {
                *ratio = share.clamp(0.05, 0.95);
                return true;
            }
            if node_id(second) == id {
                *ratio = 1.0 - share.clamp(0.05, 0.95);
                return true;
            }
            set(first, id, share) || set(second, id, share)
        }
        // rail/dock 不参与（贴边）
        if self.rail == Some(id) || self.dock == Some(id) {
            return false;
        }
        set(&mut self.root, id, share)
    }

    /// 按方向调整叶子份额：dir 必须匹配其直接父 Split 的方向（H=左右 / V=上下）
    /// 才生效；delta>0 增大该叶子、<0 减小（clamp 到 [0.05, 0.95]）。返回是否调整。
    pub fn resize_leaf_dir(&mut self, id: u64, dir: SplitDir, delta: f32) -> bool {
        if self.fixed.contains(&id) || self.rail == Some(id) {
            return false; // fixed/rail 不可 resize
        }
        fn adjust(node: &mut LayoutNode, id: u64, dir: SplitDir, delta: f32) -> bool {
            match node {
                LayoutNode::Leaf { .. } => false,
                LayoutNode::Split {
                    dir: sd,
                    ratio,
                    first,
                    second,
                    ..
                } => {
                    let first_target =
                        matches!(&**first, LayoutNode::Leaf { id: lid } if *lid == id);
                    let second_target =
                        matches!(&**second, LayoutNode::Leaf { id: lid } if *lid == id);
                    if (first_target || second_target) && *sd == dir {
                        let nr = if first_target {
                            *ratio + delta
                        } else {
                            *ratio - delta
                        };
                        *ratio = nr.clamp(0.05, 0.95);
                        return true;
                    }
                    if adjust(first, id, dir, delta) {
                        return true;
                    }
                    adjust(second, id, dir, delta)
                }
            }
        }
        if !self.components.contains_key(&id) {
            return false;
        }
        adjust(&mut self.root, id, dir, delta)
    }

    /// 交换两个叶子的组件引用：树结构/比例/id/焦点全不动，只换内容。
    /// 任一 id 不存在或相同 → false。
    pub fn swap(&mut self, id1: u64, id2: u64) -> bool {
        if id1 == id2
            || !self.components.contains_key(&id1)
            || !self.components.contains_key(&id2)
            || self.fixed.contains(&id1)
            || self.fixed.contains(&id2)
            || self.rail == Some(id1)
            || self.rail == Some(id2)
        {
            return false; // fixed/rail 不可 swap
        }
        let c1 = self.components.remove(&id1).unwrap();
        let c2 = self.components.remove(&id2).unwrap();
        self.components.insert(id1, c2);
        self.components.insert(id2, c1);
        true
    }

    /// 叶子在方向上的相邻叶子（H:first=左、second=右；V:first=上、second=下）。
    /// 从 target 的直接父开始向上回溯：遇到方向匹配的祖先 Split 时，若 target 在
    /// 对侧则返回该祖先对侧子树靠分割边的极值叶子；全程无匹配 → None（边界）。
    pub fn neighbor_leaf(&self, target: u64, dir: SplitDir, first_side: bool) -> Option<u64> {
        // 祖先链（从直接父到根），记录每层 target 在 first/second 侧
        let mut chain: Vec<(&LayoutNode, bool)> = Vec::new();
        fn collect<'a>(
            node: &'a LayoutNode,
            target: u64,
            chain: &mut Vec<(&'a LayoutNode, bool)>,
        ) -> bool {
            match node {
                LayoutNode::Leaf { id } => *id == target,
                LayoutNode::Split { first, second, .. } => {
                    if collect(first, target, chain) {
                        chain.push((node, true));
                        true
                    } else if collect(second, target, chain) {
                        chain.push((node, false));
                        true
                    } else {
                        false
                    }
                }
            }
        }
        if !collect(&self.root, target, &mut chain) {
            return None;
        }
        for (parent, target_in_first) in chain {
            let LayoutNode::Split {
                dir: sd,
                first,
                second,
                ..
            } = parent
            else {
                unreachable!()
            };
            if *sd != dir {
                continue; // 方向不匹配：继续向上层找
            }
            if first_side && !target_in_first {
                // 找 first 侧邻居：first 子树靠分割边的极值（沿 second 走到底）
                return Some(extreme_leaf(first, false));
            }
            if !first_side && target_in_first {
                // 找 second 侧邻居：second 子树靠分割边的极值（沿 first 走到底）
                return Some(extreme_leaf(second, true));
            }
            // 该层方向匹配但 target 已在目标侧：继续向上（上层可能跨过更大分割）
        }
        None
    }

    /// 聚焦方向邻居（布局模式 h/j/k/l）；无邻居返回 None。
    pub fn focus_dir(&mut self, id: u64, dir: SplitDir, first_side: bool) -> Option<u64> {
        let nb = self.neighbor_leaf(id, dir, first_side)?;
        self.active = nb;
        Some(nb)
    }

    /// 与方向邻居交换内容（布局模式 H/J/K/L）；无邻居返回 false。
    pub fn swap_dir(&mut self, id: u64, dir: SplitDir, first_side: bool) -> bool {
        let Some(nb) = self.neighbor_leaf(id, dir, first_side) else {
            return false;
        };
        self.swap(id, nb)
    }

    /// 目标叶子所在（最内层）Split 恢复 50/50。
    pub fn equalize(&mut self, id: u64) {
        if self.fixed.contains(&id) || self.rail == Some(id) {
            return; // fixed/rail 所在 Split 不可均衡
        }
        fn eq(node: &mut LayoutNode, id: u64) {
            match node {
                LayoutNode::Leaf { .. } => {}
                LayoutNode::Split {
                    ratio,
                    first,
                    second,
                    ..
                } => {
                    let first_is_target =
                        matches!(&**first, LayoutNode::Leaf { id: lid } if *lid == id);
                    let second_is_target =
                        matches!(&**second, LayoutNode::Leaf { id: lid } if *lid == id);
                    if first_is_target || second_is_target {
                        *ratio = 0.5;
                    } else if contains_leaf(first, id) {
                        eq(first, id);
                    } else if contains_leaf(second, id) {
                        eq(second, id);
                    }
                }
            }
        }
        if self.components.contains_key(&id) {
            eq(&mut self.root, id);
        }
    }

    /// 这个 id 在树里是什么（叶子 / 组 / 不存在）。
    pub fn node_kind(&self, id: u64) -> Option<NodeKind> {
        match find_node(&self.root, id)? {
            LayoutNode::Leaf { .. } => Some(NodeKind::Leaf),
            LayoutNode::Split { .. } => Some(NodeKind::Group),
        }
    }

    /// 全部组 id（树序：先根后子）。JS `pane.list()` 的组条目由此而来。
    pub fn group_ids(&self) -> Vec<u64> {
        fn walk(node: &LayoutNode, out: &mut Vec<u64>) {
            if let LayoutNode::Split {
                id, first, second, ..
            } = node
            {
                out.push(*id);
                walk(first, out);
                walk(second, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    /// 组的**绝对**屏幕矩形（最近一次渲染的包围盒；未渲染/zoom 中 → None）
    pub fn group_rect(&self, id: u64) -> Option<Rect> {
        self.group_rects.get(&id).copied()
    }

    /// 组的直接子节点 id（叶子或子组）
    pub fn group_children(&self, id: u64) -> Option<Vec<u64>> {
        match find_node(&self.root, id)? {
            LayoutNode::Split { first, second, .. } => Some(vec![node_id(first), node_id(second)]),
            LayoutNode::Leaf { .. } => None,
        }
    }

    /// 节点是否包含另一个节点（含自身）
    pub fn node_contains(&self, outer: u64, inner: u64) -> bool {
        node_contains(&self.root, outer, inner)
    }

    /// 组内所有叶子 id（树序）
    pub fn group_leaves(&self, id: u64) -> Vec<u64> {
        let mut out = Vec::new();
        if let Some(node) = find_node(&self.root, id) {
            fn walk(node: &LayoutNode, out: &mut Vec<u64>) {
                match node {
                    LayoutNode::Leaf { id } => out.push(*id),
                    LayoutNode::Split { first, second, .. } => {
                        walk(first, out);
                        walk(second, out);
                    }
                }
            }
            walk(node, &mut out);
        }
        out
    }

    /// 在**组**（容器节点）上切一刀：新叶子跨满该组的整宽/整高。
    ///
    /// `target = None` ⇒ 整窗切一刀（根），等价 tmux 的 `split-window -f` ——
    /// 这就是"term 横跨左右两个编辑区、占满整个底边"那条需求的一条命令。
    /// `target = Some(叶子)` ⇒ 退化成普通的叶子切分（区域就是那个叶子）。
    /// `target = Some(组)` ⇒ 区域是整个组的包围盒。
    pub fn split_at_group(
        &mut self,
        target: Option<u64>,
        dir: SplitDir,
        new_first: bool,
        ratio: f32,
        component: Box<dyn Component>,
    ) -> Option<u64> {
        let new_id = self.next_id_for_split();
        self.split_at_group_prealloc(new_id, target, dir, new_first, ratio, component)
    }

    /// 同 [`Self::split_at_group`]，但叶子 id 由调用方预分配
    /// （JS `helix.split` 已经按这个 id 注册了回调）。
    pub fn split_at_group_prealloc(
        &mut self,
        new_id: u64,
        target: Option<u64>,
        dir: SplitDir,
        new_first: bool,
        ratio: f32,
        component: Box<dyn Component>,
    ) -> Option<u64> {
        match target {
            // 叶子目标 → 就是普通叶子切分（区域 = 那个叶子）
            Some(t) if self.node_kind(t) == Some(NodeKind::Leaf) => {
                if self.rail == Some(t) {
                    return None; // rail 不参与分裂
                }
                return self.split_side_ratio(t, dir, new_first, ratio, component, new_id);
            }
            Some(t) if self.node_kind(t).is_none() => return None,
            _ => {}
        }
        self.components.insert(new_id, component);
        if self.wrap_leaf_at(target, new_id, dir, new_first, ratio) {
            Some(new_id)
        } else {
            None
        }
    }

    /// 把**已有的**叶子搬到指定位置（事后修形）：`target = None` 贴整窗边缘 /
    /// `Some(组)` 贴那个组的边缘 / `Some(叶子)` 与那个叶子并排。
    ///
    /// 用它能救回"已经切错"的布局：`ed1 | (ed2/term)` 里那个半宽的 term，
    /// `move_into(term, None, V, false)` 一次就变成横跨整宽的地板。
    pub fn move_into(
        &mut self,
        leaf: u64,
        target: Option<u64>,
        dir: SplitDir,
        new_first: bool,
        ratio: f32,
    ) -> bool {
        if !self.components.contains_key(&leaf) {
            return false;
        }
        // rail/dock 必须是 root 的边缘叶（贴边全高/全宽），搬走会破不变量
        if self.rail == Some(leaf) || self.dock == Some(leaf) {
            return false;
        }
        // 不能搬进自己/自己的子树
        if let Some(t) = target {
            if t == leaf || self.node_contains(leaf, t) {
                return false;
            }
        }
        // 整棵树只有它一个叶子 ⇒ 没地方可搬
        if self.leaf_ids().len() < 2 {
            return false;
        }
        // 1) 从原位摘下（父节点收缩成兄弟）
        prune(&mut self.root, leaf);
        // 2) 挪到目标位置（与 split 同一个"包一层组"的动作，只是放的是已有叶子）
        self.wrap_leaf_at(target, leaf, dir, new_first, ratio)
    }

    /// 把叶子 `leaf`（此刻**不在**树里，或已在树上但仅作为 target 的兄弟）包到
    /// `target`（None = 整窗）的某一侧。返回是否成功。
    ///
    /// 整窗这一档必须**剥掉边缘叶**（rail/dock）：它们的不变量是"恒为 root 的直接子叶"
    /// （`rail_edge`/`rail_ratio`/`is_dock` 都依赖它），跟着被包进去会让侧栏失去"贴边全高"。
    fn wrap_leaf_at(
        &mut self,
        target: Option<u64>,
        leaf: u64,
        dir: SplitDir,
        new_first: bool,
        ratio: f32,
    ) -> bool {
        if let Some(t) = target {
            match self.node_kind(t) {
                // 与某个叶子并排：在它原来的位置切一刀
                Some(NodeKind::Leaf) => {
                    let gid = self.next_group_id();
                    let (a, b) = if new_first { (leaf, t) } else { (t, leaf) };
                    // 上面已确认 t 是叶子 ⇒ replace_leaf 必定命中
                    replace_leaf(&mut self.root, t, &|_| {
                        group(
                            gid,
                            dir,
                            ratio,
                            LayoutNode::Leaf { id: a },
                            LayoutNode::Leaf { id: b },
                        )
                    });
                    self.active = leaf;
                    return true;
                }
                None => return false,
                Some(NodeKind::Group) => {}
            }
        }

        // 整窗 / 组：剥掉边缘叶 → 包住 main/组 → 原样挂回
        let rail = self.rail.and_then(|rid| self.detach_edge(SplitDir::H, rid));
        let dock = self.dock.and_then(|did| self.detach_edge(SplitDir::V, did));
        let gid = self.next_group_id();
        let old = match target {
            Some(t) => match take_node(&mut self.root, t) {
                Some(n) => n,
                None => {
                    // 复原边缘叶，别留下半截状态
                    if let Some((id, first, share)) = dock {
                        self.reattach_edge(SplitDir::V, id, first, share);
                    }
                    if let Some((id, first, share)) = rail {
                        self.reattach_edge(SplitDir::H, id, first, share);
                    }
                    return false;
                }
            },
            None => std::mem::replace(&mut self.root, LayoutNode::Leaf { id: u64::MAX }),
        };
        let node = if new_first {
            group(gid, dir, ratio, LayoutNode::Leaf { id: leaf }, old)
        } else {
            group(gid, dir, ratio, old, LayoutNode::Leaf { id: leaf })
        };
        let ok = match target {
            None => {
                self.root = node;
                true
            }
            Some(_) => fill_placeholder(&mut self.root, node),
        };
        if let Some((id, first, share)) = dock {
            self.reattach_edge(SplitDir::V, id, first, share);
        }
        if let Some((id, first, share)) = rail {
            self.reattach_edge(SplitDir::H, id, first, share);
        }
        if ok {
            self.active = leaf;
        }
        ok
    }

    /// 若 root 恰好是"贴边叶 + 其余"的二分（`dir` 轴向、贴边叶是直接子叶），
    /// 把 root 收缩为"其余"并返回 `(贴边叶 id, 是否 first 侧, 该侧份额)`。
    fn detach_edge(&mut self, dir: SplitDir, id: u64) -> Option<(u64, bool, f32)> {
        let LayoutNode::Split {
            dir: sd,
            ratio,
            first,
            second,
            ..
        } = &mut self.root
        else {
            return None;
        };
        if *sd != dir {
            return None;
        }
        let is_first = matches!(&**first, LayoutNode::Leaf { id: f } if *f == id);
        let is_second = matches!(&**second, LayoutNode::Leaf { id: s } if *s == id);
        if !is_first && !is_second {
            return None;
        }
        let share = if is_first { *ratio } else { 1.0 - *ratio };
        let sibling = if is_first {
            std::mem::replace(&mut **second, LayoutNode::Leaf { id: u64::MAX })
        } else {
            std::mem::replace(&mut **first, LayoutNode::Leaf { id: u64::MAX })
        };
        self.root = sibling;
        Some((id, is_first, share))
    }

    /// 把贴边叶重新挂回 root 边缘（与 `register_rail`/`register_dock_bottom` 同形，
    /// 但不改 `self.rail`/`self.dock`/`active`）。
    fn reattach_edge(&mut self, dir: SplitDir, id: u64, is_first: bool, share: f32) {
        let gid = self.next_group_id();
        let main = std::mem::replace(&mut self.root, LayoutNode::Leaf { id: u64::MAX });
        let leaf = LayoutNode::Leaf { id };
        self.root = if is_first {
            group(gid, dir, share, leaf, main)
        } else {
            group(gid, dir, 1.0 - share, main, leaf)
        };
    }

    /// 组级 equalize：把组内**所有**分界恢复成等分（递归 0.5）。
    /// 传叶子 id 时退化成旧行为（只均衡它最内层那个 Split）。返回是否动了。
    pub fn equalize_node(&mut self, id: u64) -> bool {
        if self.fixed.contains(&id) || self.rail == Some(id) {
            return false; // fixed/rail 不参与等分
        }
        match self.node_kind(id) {
            Some(NodeKind::Group) => {
                let rail = self.rail;
                if let Some(node) = find_node_mut(&mut self.root, id) {
                    equalize_subtree(node, rail);
                    return true;
                }
                false
            }
            Some(NodeKind::Leaf) => {
                self.equalize(id);
                true
            }
            None => false,
        }
    }

    /// 最小化一个**节点**（叶子或组）：被最小化的子树不占布局空间，渲染为底部一条
    /// 标题横条，其余叶子占满区域。可同时最小化多个（按提交顺序从下往上叠条）。
    ///
    /// 组级最小化是"整块折起来"：里面的所有 pane 都不占地方（它们仍在树里、
    /// 组件还活着），展开时原样回来。
    pub fn set_minimized(&mut self, id: u64, minimized: bool) {
        if self.node_kind(id).is_none() {
            return;
        }
        if minimized && (self.fixed.contains(&id) || self.rail == Some(id)) {
            return; // fixed/rail 不可被最小化
        }
        let root = &self.root;
        if minimized {
            // 祖先已在最小化列表里 → 再最小化它内部没有意义（整棵子树早就让位了）
            if self.minimized.iter().any(|m| node_contains(root, *m, id)) {
                return;
            }
            // 已经最小化了它内部的节点 → 换成更外层的这个（避免嵌套条目）
            self.minimized
                .retain(|m| *m == id || !node_contains(root, id, *m));
            if !self.minimized.contains(&id) {
                self.minimized.push(id);
            }
            // 焦点/缩放不能停在一个已经被折起来的节点上
            if node_contains(root, id, self.active) {
                self.active = 0;
            }
            if self.zoomed.is_some_and(|z| node_contains(root, id, z)) {
                self.zoomed = None;
            }
        } else {
            self.minimized.retain(|m| *m != id);
        }
    }

    /// 被最小化的节点 id（从下往上叠条的顺序）
    pub fn minimized_ids(&self) -> &[u64] {
        &self.minimized
    }

    /// 最近一次被最小化的节点 id（窗口模式的还原目标）
    pub fn minimized_last(&self) -> Option<u64> {
        self.minimized.last().copied()
    }

    /// 最小化节点 id（`None` = 没有任何最小化）
    pub fn minimized_leaf(&self) -> Option<u64> {
        self.minimized_last()
    }

    /// 最小化节点 id(窗口模式/测试别名;多个时给最近那个)
    pub fn minimized(&self) -> Option<u64> {
        self.minimized_last()
    }

    /// 缩放中的叶子 id(窗口模式/测试别名)
    pub fn zoomed(&self) -> Option<u64> {
        self.zoomed
    }

    /// 叶子类型名列表(树序;窗口模式测试断言用)
    pub fn leaf_types(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        fn walk(
            node: &LayoutNode,
            comps: &std::collections::HashMap<u64, Box<dyn Component>>,
            out: &mut Vec<&'static str>,
        ) {
            match node {
                LayoutNode::Leaf { id } => {
                    if let Some(c) = comps.get(id) {
                        out.push(c.type_name());
                    }
                }
                LayoutNode::Split { first, second, .. } => {
                    walk(first, comps, out);
                    walk(second, comps, out);
                }
            }
        }
        walk(&self.root, &self.components, &mut out);
        out
    }

    /// 是否有某类型组件（叶子内）
    pub fn has_component(&self, type_name: &str) -> bool {
        self.components.values().any(|c| c.type_name() == type_name)
    }

    /// 某类型组件数量
    pub fn count_type(&self, type_name: &str) -> usize {
        self.components
            .values()
            .filter(|c| c.type_name() == type_name)
            .count()
    }

    /// 序列化布局（不包含组件内容，只含叶子 id 与类型标记）
    /// 重置树内面板/终端的脏格 diff（测试向不同 surface 渲染时用）
    /// 活动叶子光标:转发给活动组件(编辑器 bar/underline 硬件光标),坐标转全屏
    pub fn cursor(
        &self,
        area: Rect,
        editor: &helix_view::Editor,
    ) -> (
        Option<helix_core::Position>,
        helix_view::graphics::CursorKind,
    ) {
        use helix_view::graphics::CursorKind;
        let active = self.active();
        let (Some(comp), Some(rect)) = (self.components.get(&active), self.leaf_rects.get(&active))
        else {
            return (None, CursorKind::Hidden);
        };
        let (pos, kind) = comp.cursor(*rect, editor);
        // 叶子 rect 相对 tree_area(area 参数 = tree_area);全屏 = area 原点 + rect + 组件内位置
        // Position::new(row, col):row 用叶子 y + 组件内 row,col 用叶子 x + 组件内 col
        let pos = pos.map(|p| {
            helix_core::Position::new(
                (area.y + rect.y) as usize + p.row,
                (area.x + rect.x) as usize + p.col,
            )
        });
        (pos, kind)
    }

    /// 鼠标命中:屏幕坐标 → 叶子 id(基于最近一次渲染的区域;无命中 → None)
    pub fn leaf_id_at(&self, x: u16, y: u16) -> Option<u64> {
        self.leaf_rects
            .iter()
            .find(|(_, r)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            .map(|(id, _)| *id)
    }

    /// 鼠标命中 → "这次点击该不该换焦点",是则给出目标叶子 id。
    ///
    /// 与 [`Self::leaf_id_at`] 的区别在于它排除两种情况:
    /// - **浮窗盖住的地方**:点击归浮窗(事件按 z 路由),把焦点送给被盖住的平铺叶子是反直觉的;
    /// - **zoom 期间**:`leaf_rects` 还是上一帧(未 zoom)的几何,拿它换焦点会把用户
    ///   甩到一个此刻根本看不见的叶子上。
    pub fn focus_target_at(&self, area: Rect, x: u16, y: u16) -> Option<u64> {
        if let Some(z) = self.zoomed {
            // 叶子 zoom：屏幕上只有它，点了也不该换焦点（几何是另一套）
            // 组 zoom：屏幕上就是这棵子树，`leaf_rects` 已经是 zoom 后的几何 ⇒ 可以点
            return match self.node_kind(z) {
                Some(NodeKind::Group) => self.leaf_id_at(x, y),
                _ => None,
            };
        }
        let in_float = self.floats().iter().any(|f| {
            let r = f.rect_of(area);
            x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
        });
        if in_float {
            return None;
        }
        self.leaf_id_at(x, y)
    }

    /// 某叶子的**绝对**屏幕矩形(最近一次渲染的几何;未渲染过 → None)。
    /// 与 `leaf_id_at` 同一份数据 ⇒ 插件拿到的 `pane.list()[i].area` 与鼠标坐标同系。
    pub fn leaf_rect(&self, id: u64) -> Option<Rect> {
        self.leaf_rects.get(&id).copied()
    }

    /// 几何自上次取走以来是否变过(取走脏标记)
    pub fn take_rects_dirty(&mut self) -> bool {
        std::mem::take(&mut self.rects_dirty)
    }

    pub fn reset_plugin_diffs(&mut self) {
        for comp in self.components.values_mut() {
            if let Some(p) = comp
                .as_any_mut()
                .downcast_mut::<crate::ui::plugin_panel::PluginPanel>()
            {
                p.reset_render_state();
            }
            if let Some(p) = comp
                .as_any_mut()
                .downcast_mut::<crate::ui::plugin_terminal::PluginTerminal>()
            {
                p.reset_render_state();
            }
        }
    }

    /// 从 `get_layout()` 的 dump 重建树。
    ///
    /// **重建而非重建组件**:终端/面板叶子无法凭空造出来(要 pty / 插件),
    /// 但组件本来就还在 `components` 里,所以按 id 复用即可。已不存在的叶子
    /// 连同它占的分支一起收敛;全部叶子都没了则报错(不把树搞空)。
    pub fn restore(&mut self, dump: &serde_json::Value) -> anyhow::Result<()> {
        use std::collections::HashSet;
        let live: HashSet<u64> = self.components.keys().copied().collect();

        fn parse(node: &serde_json::Value, live: &HashSet<u64>) -> Option<LayoutNode> {
            match node.get("type").and_then(|t| t.as_str()) {
                Some("leaf") => {
                    let id = node.get("id")?.as_u64()?;
                    live.contains(&id).then_some(LayoutNode::Leaf { id })
                }
                Some("split") => {
                    let dir = match node.get("dir").and_then(|d| d.as_str()) {
                        Some("v") => SplitDir::V,
                        _ => SplitDir::H,
                    };
                    let ratio = node.get("ratio").and_then(|r| r.as_f64()).unwrap_or(0.5) as f32;
                    let first = parse(node.get("first")?, live);
                    let second = parse(node.get("second")?, live);
                    match (first, second) {
                        (Some(f), Some(s)) => Some(group(
                            node.get("id").and_then(|i| i.as_u64()).unwrap_or(u64::MAX),
                            dir,
                            ratio.clamp(0.05, 0.95),
                            f,
                            s,
                        )),
                        (Some(f), None) => Some(f),
                        (None, Some(s)) => Some(s),
                        (None, None) => None,
                    }
                }
                _ => None,
            }
        }

        let tree = dump
            .get("tree")
            .ok_or_else(|| anyhow::anyhow!("restore_layout: dump 里缺 tree"))?;
        let root = parse(tree, &live)
            .ok_or_else(|| anyhow::anyhow!("restore_layout: dump 里没有任何仍存在的叶子"))?;

        self.root = root;

        // 组 id 修整：老 dump 没有 `id`（占位 u64::MAX）或重复 → 重新分配；
        // 顺带把 `next_id` 抬到最大已用 id 之上，否则新建的组/叶子会撞已有 id。
        {
            let mut used: std::collections::HashSet<u64> = live.clone();
            let mut next = GROUP_ID_BASE
                .max(max_group_id(&self.root).saturating_add(1))
                .max(self.next_group);
            fn fix(
                node: &mut LayoutNode,
                used: &mut std::collections::HashSet<u64>,
                next: &mut u64,
            ) {
                match node {
                    LayoutNode::Leaf { .. } => {}
                    LayoutNode::Split {
                        id, first, second, ..
                    } => {
                        if *id == u64::MAX || used.contains(id) {
                            *id = *next;
                            *next = next.saturating_add(1);
                        }
                        used.insert(*id);
                        fix(first, used, next);
                        fix(second, used, next);
                    }
                }
            }
            fix(&mut self.root, &mut used, &mut next);
            self.next_group = next;
        }

        // fixed / rail 按 dump 的 leafs 重建(只保留仍存在的 id)
        self.fixed.clear();
        self.rail = None;
        if let Some(leafs) = dump.get("leafs").and_then(|l| l.as_array()) {
            for l in leafs {
                let Some(id) = l.get("id").and_then(|v| v.as_u64()) else {
                    continue;
                };
                if !live.contains(&id) {
                    continue;
                }
                if l.get("fixed").and_then(|v| v.as_bool()).unwrap_or(false) {
                    self.fixed.insert(id);
                }
                if l.get("rail").and_then(|v| v.as_bool()).unwrap_or(false) {
                    self.rail = Some(id);
                }
            }
        }

        // 浮窗槽位:几何/z/pin 原样恢复,只保留仍存在的 id
        self.floats = dump
            .get("floats")
            .and_then(|f| serde_json::from_value::<Vec<FloatSlot>>(f.clone()).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|f| live.contains(&f.id))
            .collect();

        // 堆叠组:只保留仍存活的成员;成员少于 2 个则整组丢弃(兄弟约束下组恒 2 人)
        self.stacks.clear();
        if let Some(groups) = dump
            .get("stacks")
            .and_then(|v| serde_json::from_value::<Vec<StackGroup>>(v.clone()).ok())
        {
            for g in groups {
                let members: Vec<u64> =
                    g.members.into_iter().filter(|m| live.contains(m)).collect();
                if members.len() >= 2 {
                    let anchor = members[0];
                    self.stacks.insert(anchor, StackGroup { members });
                }
            }
        }

        // active / zoomed / minimized 只接受仍存在的 id,否则回落到树的首叶
        let live_of = |v: Option<u64>| v.filter(|id| live.contains(id));
        let first = self.leaf_ids().into_iter().next().unwrap_or(0);
        self.active = dump
            .get("active")
            .and_then(|v| v.as_u64())
            .and_then(|v| live_of(Some(v)))
            .unwrap_or(first);
        self.zoomed = live_of(dump.get("zoomed").and_then(|v| v.as_u64()));
        // minimized 老 dump 是单个数字，新 dump 是数组 —— 两种都吃
        let min_ids: Vec<u64> = match dump.get("minimized") {
            Some(serde_json::Value::Array(a)) => a.iter().filter_map(|v| v.as_u64()).collect(),
            Some(v) => v.as_u64().into_iter().collect(),
            None => Vec::new(),
        };
        self.minimized = min_ids
            .into_iter()
            .filter(|id| find_node(&self.root, *id).is_some())
            .collect();
        Ok(())
    }

    pub fn dump(&self) -> LayoutDump {
        let fixed = &self.fixed;
        let rail = self.rail;
        fn dump_node(
            node: &LayoutNode,
            fixed: &std::collections::HashSet<u64>,
            rail: Option<u64>,
        ) -> serde_json::Value {
            match node {
                LayoutNode::Leaf { id } => serde_json::json!({
                    "type": "leaf",
                    "id": id,
                    "fixed": fixed.contains(id),
                    "rail": Some(*id) == rail,
                }),
                LayoutNode::Split {
                    id,
                    dir,
                    ratio,
                    first,
                    second,
                } => serde_json::json!({
                    "type": "split",
                    "id": id,
                    "dir": match dir { SplitDir::H => "h", SplitDir::V => "v" },
                    "ratio": ratio,
                    "first": dump_node(first, fixed, rail),
                    "second": dump_node(second, fixed, rail),
                }),
            }
        }
        fn collect_leafs(
            node: &LayoutNode,
            fixed: &std::collections::HashSet<u64>,
            rail: Option<u64>,
            out: &mut Vec<LeafInfo>,
        ) {
            match node {
                LayoutNode::Leaf { id } => {
                    out.push(LeafInfo {
                        id: *id,
                        fixed: fixed.contains(id),
                        rail: Some(*id) == rail,
                    });
                }
                LayoutNode::Split { first, second, .. } => {
                    collect_leafs(first, fixed, rail, out);
                    collect_leafs(second, fixed, rail, out);
                }
            }
        }
        let mut leafs = Vec::new();
        collect_leafs(&self.root, fixed, rail, &mut leafs);
        LayoutDump {
            tree: dump_node(&self.root, fixed, rail),
            active: self.active,
            zoomed: self.zoomed,
            minimized: self.minimized.clone(),
            floats: self.floats.clone(),
            stacks: self.stacks.values().cloned().collect(),
            leafs,
        }
    }

    /// 渲染：每个叶子在自己的矩形里渲染组件；缩放时只有被缩放的叶子渲染；
    /// 浮动叶子最后画（最上层，居中浮窗 + 边框），其他叶子照常布局。
    /// rail 边缘几何:(id, 是否左侧)
    fn rail_edge(&self) -> Option<(u64, bool)> {
        let rid = self.rail?;
        match &self.root {
            LayoutNode::Split { first, second, .. } => {
                if matches!(&**first, LayoutNode::Leaf { id } if *id == rid) {
                    Some((rid, true))
                } else if matches!(&**second, LayoutNode::Leaf { id } if *id == rid) {
                    Some((rid, false))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// rail 占宽份额(0<share<1)
    fn rail_share(&self) -> f32 {
        match &self.root {
            LayoutNode::Split { ratio, first, .. } => {
                if let Some((rid, _)) = self.rail_edge() {
                    let first_has_rail = matches!(&**first, LayoutNode::Leaf { id } if *id == rid);
                    if first_has_rail {
                        *ratio
                    } else {
                        1.0 - *ratio
                    }
                } else {
                    0.0
                }
            }
            _ => 0.0,
        }
    }

    /// 把一批 (叶子, rect) 画出来：堆叠组顶部留 1 行 header；焦点叶子画边框（内容 inset 1）。
    fn draw_leaves(
        &mut self,
        rects: Vec<LeafRect>,
        focus: Option<u64>,
        surface: &mut tui::buffer::Buffer,
        cx: &mut Context,
    ) {
        for (id, rect) in rects {
            // 堆叠:占顶部 1 行作 header(成员列表,▶ 标记当前显示的那个),
            // 内容画在剩下的区域。锚 = members[0] = 当前显示的那个(A.6 不变量)。
            let group = self.stacks.get(&id).cloned();
            let rect = match &group {
                Some(g) if rect.height >= 3 => {
                    let names: Vec<String> = g
                        .members
                        .iter()
                        .enumerate()
                        .map(|(i, m)| {
                            let kind = self
                                .components
                                .get(m)
                                .map(|c| short_type_name(c.type_name()).to_string())
                                .unwrap_or_default();
                            if i == 0 {
                                format!("▶{kind}")
                            } else {
                                kind
                            }
                        })
                        .collect();
                    let text = format!("▸ {}", names.join("  "));
                    let style = cx.editor.theme.get("ui.statusline");
                    let chars: Vec<char> = text.chars().collect();
                    for x in 0..rect.width {
                        let ch = chars.get(x as usize).copied().unwrap_or(' ');
                        if let Some(cell) = surface.get_mut(rect.x + x, rect.y) {
                            cell.set_symbol(&ch.to_string());
                            cell.set_style(style);
                        }
                    }
                    Rect::new(rect.x, rect.y + 1, rect.width, rect.height - 1)
                }
                _ => rect,
            };
            if let Some(comp) = self.components.get_mut(&id) {
                if focus == Some(id) && rect.width >= 4 && rect.height >= 4 {
                    let inner = Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2);
                    comp.render(inner, surface, cx);
                    draw_focus_border(rect, surface, &cx.editor.theme);
                } else {
                    comp.render(rect, surface, cx);
                }
            }
        }
    }

    pub fn render(&mut self, area: Rect, surface: &mut tui::buffer::Buffer, cx: &mut Context) {
        if let Some(zoomed) = self.zoomed {
            // zoom 保留 rail:rail 占边缘窄条,zoomed 占其余 main 区
            let (rail_rect, zoom_area) = match self.rail_edge() {
                Some((rid, left)) => {
                    let share = self.rail_share().clamp(0.05, 0.9);
                    let rail_w = ((area.width as f32) * share) as u16;
                    let rail_w = rail_w.clamp(1, area.width.saturating_sub(1).max(1));
                    let (rr, za) = if left {
                        (
                            Some(Rect::new(area.x, area.y, rail_w, area.height)),
                            Rect::new(area.x + rail_w, area.y, area.width - rail_w, area.height),
                        )
                    } else {
                        (
                            Some(Rect::new(
                                area.x + area.width - rail_w,
                                area.y,
                                rail_w,
                                area.height,
                            )),
                            Rect::new(area.x, area.y, area.width - rail_w, area.height),
                        )
                    };
                    let _ = rid;
                    (rr, za)
                }
                None => (None, area),
            };
            if let Some(rr) = rail_rect {
                if let Some((rid, _)) = self.rail_edge() {
                    if let Some(comp) = self.components.get_mut(&rid) {
                        comp.render(rr, surface, cx);
                    }
                }
            }
            match self.node_kind(zoomed) {
                // 叶子 zoom：整块区域给它
                Some(NodeKind::Leaf) => {
                    self.group_rects.clear();
                    self.leaf_rects.clear();
                    self.leaf_rects.insert(zoomed, zoom_area);
                    let focus = if self.floats.is_empty() {
                        Some(zoomed)
                    } else {
                        None
                    };
                    self.draw_leaves(vec![(zoomed, zoom_area)], focus, surface, cx);
                }
                // 组 zoom：组内各叶子按原分割铺开占满 zoom 区（其它叶子不出现）
                Some(NodeKind::Group) => {
                    let skip = self.layout_skip_ids();
                    let mut rects = Vec::new();
                    let mut groups = Vec::new();
                    if let Some(node) = find_node(&self.root, zoomed) {
                        layout_node(node, zoom_area, &mut rects, &mut groups, &skip);
                    }
                    self.group_rects.clear();
                    for (id, r) in &groups {
                        self.group_rects.insert(*id, *r);
                    }
                    let changed = rects.len() != self.leaf_rects.len()
                        || rects
                            .iter()
                            .any(|(id, r)| self.leaf_rects.get(id) != Some(r));
                    self.leaf_rects.clear();
                    for (id, r) in &rects {
                        self.leaf_rects.insert(*id, *r);
                    }
                    if changed {
                        self.rects_dirty = true;
                    }
                    let focus = if self.floats.is_empty() {
                        let a = self.active;
                        rects.iter().any(|(id, _)| *id == a).then_some(a)
                    } else {
                        None
                    };
                    self.draw_leaves(rects, focus, surface, cx);
                }
                None => {}
            }
            return;
        }
        // 叶=窗口:leaf0(EditorView)渲染前告知被 BufferLeaf 认领的 view(渲染整树时排除,防双画)
        {
            let claimed = self.claimed_view_ids();
            if !claimed.is_empty() {
                if let Some(ev) = self.components.get_mut(&0).and_then(|c| {
                    c.as_any_mut()
                        .downcast_mut::<crate::ui::editor::EditorView>()
                }) {
                    ev.set_claimed_views(claimed);
                }
            }
        }
        let mut rects = Vec::new();
        let mut groups = Vec::new();
        // 浮动/最小化节点不占布局空间（其余叶子占满，无 dock 位置留白）
        let skip = self.layout_skip_ids();
        layout_node(&self.root, area, &mut rects, &mut groups, &skip);
        self.group_rects.clear();
        for (id, r) in &groups {
            self.group_rects.insert(*id, *r);
        }
        // 活动叶子（事件路由目标）：画高亮边框（内容 inset 1 格；浮窗已有自身边框不重复）
        let focus = if !self.floats.is_empty() || self.zoomed.is_some() {
            None
        } else {
            Some(self.active)
        };
        // 几何变化 → 置脏（`pane.list()` 快照由此刷新一次，不靠每帧同步）
        let changed = rects.len() != self.leaf_rects.len()
            || rects
                .iter()
                .any(|(id, r)| self.leaf_rects.get(id) != Some(r));
        self.leaf_rects.clear();
        for (id, rect) in &rects {
            self.leaf_rects.insert(*id, *rect);
        }
        if changed {
            self.rects_dirty = true;
        }
        self.draw_leaves(rects, focus, surface, cx);
        // 最小化的节点：底部一条标题横条（多条从下往上叠；最上层）
        let bar_style = cx.editor.theme.get("ui.popup");
        for (i, mid) in self.minimized.iter().enumerate() {
            let label = match find_node(&self.root, *mid) {
                Some(LayoutNode::Leaf { .. }) => self
                    .components
                    .get(mid)
                    .map(|c| short_type_name(c.type_name()).to_string())
                    .unwrap_or_default(),
                Some(node) => format!("组 {} 窗", count_leaves(node)),
                None => continue, // 节点已不在树里（关掉了）
            };
            let y = area.y + area.height.saturating_sub(1 + i as u16);
            if y < area.y {
                break;
            }
            let text = format!("─ {label} ─");
            let chars: Vec<char> = text.chars().collect();
            for x in 0..area.width {
                let ch = if (x as usize) < chars.len() {
                    chars[x as usize]
                } else {
                    '─'
                };
                if let Some(cell) = surface.get_mut(area.x + x, y) {
                    cell.set_symbol(&ch.to_string());
                    cell.set_style(bar_style);
                }
            }
        }
        // 浮动 pane(阶段②:多窗,按 z 升序叠画)
        for slot in self.floats() {
            let fid = slot.id;
            if let Some(comp) = self.components.get_mut(&fid) {
                let outer = slot.rect_of(area);
                let inner = Rect::new(
                    outer.x + 1,
                    outer.y + 1,
                    outer.width.saturating_sub(2),
                    outer.height.saturating_sub(2),
                );
                // 边框 + 背景（ui.popup 配色）
                let border_style = cx.editor.theme.get("ui.popup");
                let bg_style = cx.editor.theme.get("ui.background");
                for y in 0..outer.height {
                    for x in 0..outer.width {
                        let (ch, style) = if x == 0 && y == 0 {
                            ('┌', border_style)
                        } else if x == outer.width - 1 && y == 0 {
                            ('┐', border_style)
                        } else if x == 0 && y == outer.height - 1 {
                            ('└', border_style)
                        } else if x == outer.width - 1 && y == outer.height - 1 {
                            ('┘', border_style)
                        } else if y == 0 || y == outer.height - 1 {
                            ('─', border_style)
                        } else if x == 0 || x == outer.width - 1 {
                            ('│', border_style)
                        } else {
                            (' ', bg_style)
                        };
                        if let Some(cell) = surface.get_mut(outer.x + x, outer.y + y) {
                            cell.set_symbol(&ch.to_string());
                            cell.set_style(style);
                        }
                    }
                }
                comp.render(inner, surface, cx);
            }
        }
    }

    /// 事件路由：浮动叶子优先，其次活动叶子；Ignored → 编辑器叶子（id=0）兜底
    pub fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let target = self
            .floats
            .iter()
            .max_by_key(|f| f.z)
            .map(|f| f.id)
            .unwrap_or_else(|| self.active());
        // rail 焦点态:组件优先消费(浏览键);组件未消费时仅 Esc/plain-l 退出回 main,
        // 其余键(如 : 开命令)照旧穿透编辑器——filetree 等面板按"非模态"设计(未映射键穿透)。
        if target != 0 && self.rail == Some(target) {
            let is_esc =
                matches!(event, Event::Key(k) if k.code == helix_view::input::KeyCode::Esc);
            let is_plain_l = matches!(event, Event::Key(k)
                if k.code == helix_view::input::KeyCode::Char('l')
                    && !k.modifiers.contains(helix_view::input::KeyModifiers::CONTROL)
                    && !k.modifiers.contains(helix_view::input::KeyModifiers::ALT));
            if let Some(comp) = self.components.get_mut(&target) {
                let r = comp.handle_event(event, cx);
                if !r.is_ignored() {
                    return r; // 面板已消费:绝不二次分发
                }
                if is_esc || is_plain_l {
                    self.active = self.operational_target();
                    return EventResult::Consumed(None); // 退出 rail 焦点回 main
                }
                // 未消费:穿透给编辑器(非模态:冒号命令等),不再次触碰面板
                if let Some(editor) = self.components.get_mut(&0) {
                    let re = editor.handle_event(event, cx);
                    return if re.is_ignored() {
                        EventResult::Consumed(None) // rail 上吞掉,不落编辑器编辑键
                    } else {
                        re
                    };
                }
                return EventResult::Consumed(None);
            }
            return EventResult::Consumed(None);
        }
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

    /// 活动叶子的组件类型名（window mode hint / 测试用）
    pub fn active_type_name(&self) -> String {
        self.components
            .get(&self.active())
            .map(|c| c.type_name().to_string())
            .unwrap_or_default()
    }

    /// 活动叶子若为 BufferLeaf（持有独立 view），返回其 view id；否则 None。
    pub fn active_view_id(&self) -> Option<helix_view::ViewId> {
        self.view_id_of(self.active())
    }

    /// 指定叶子若为 BufferLeaf 返回其 view id。
    pub fn view_id_of(&self, id: u64) -> Option<helix_view::ViewId> {
        use crate::ui::buffer_leaf::BufferLeaf;
        self.components
            .get(&id)
            .and_then(|c| c.as_any().downcast_ref::<BufferLeaf>())
            .map(|b| b.view_id)
    }

    /// 所有 BufferLeaf 叶子持有的 view id（EditorView 渲染需排除，防双画）。
    pub fn claimed_view_ids(&self) -> Vec<helix_view::ViewId> {
        use crate::ui::buffer_leaf::BufferLeaf;
        self.components
            .iter()
            .filter(|(_, c)| c.as_any().downcast_ref::<BufferLeaf>().is_some())
            .filter_map(|(id, _)| self.view_id_of(*id))
            .collect()
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
fn replace_leaf(node: &mut LayoutNode, target: u64, f: &dyn Fn(&mut LayoutNode) -> LayoutNode) {
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

/// 子树靠分割边的极值叶子：prefer_first=true 沿 first 走到底（最左/最上），
/// false 沿 second 走到底（最右/最下）。
fn extreme_leaf(node: &LayoutNode, prefer_first: bool) -> u64 {
    match node {
        LayoutNode::Leaf { id } => *id,
        LayoutNode::Split { first, second, .. } => {
            if prefer_first {
                extreme_leaf(first, prefer_first)
            } else {
                extreme_leaf(second, prefer_first)
            }
        }
    }
}

/// 活动叶子高亮边框（ui.popup 色，与浮窗边框同风格）：┌ ┐ └ ┘ ─ │
fn draw_focus_border(
    rect: Rect,
    surface: &mut tui::buffer::Buffer,
    theme: &helix_view::theme::Theme,
) {
    let style = theme.get("ui.popup");
    let (x, y, w, h) = (rect.x, rect.y, rect.width, rect.height);
    let bottom = y + h - 1;
    let right = x + w - 1;
    if let Some(c) = surface.get_mut(x, y) {
        c.set_symbol("┌");
        c.set_style(style);
    }
    if let Some(c) = surface.get_mut(right, y) {
        c.set_symbol("┐");
        c.set_style(style);
    }
    if let Some(c) = surface.get_mut(x, bottom) {
        c.set_symbol("└");
        c.set_style(style);
    }
    if let Some(c) = surface.get_mut(right, bottom) {
        c.set_symbol("┘");
        c.set_style(style);
    }
    for col in (x + 1)..right {
        if let Some(c) = surface.get_mut(col, y) {
            c.set_symbol("─");
            c.set_style(style);
        }
        if let Some(c) = surface.get_mut(col, bottom) {
            c.set_symbol("─");
            c.set_style(style);
        }
    }
    for row in (y + 1)..bottom {
        if let Some(c) = surface.get_mut(x, row) {
            c.set_symbol("│");
            c.set_style(style);
        }
        if let Some(c) = surface.get_mut(right, row) {
            c.set_symbol("│");
            c.set_style(style);
        }
    }
}

/// 调整包含目标叶子的 Split 比例
fn adjust_ratio(node: &mut LayoutNode, target: u64, ratio: f32) {
    if let LayoutNode::Split {
        ratio: r,
        first,
        second,
        ..
    } = node
    {
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
            } else {
                // 递归剪除（父节点保留，子侧就地收缩）
                let _ = prune(first, target) || prune(second, target);
                false
            }
        }
    }
}

fn first_leaf_skipping(node: &LayoutNode, skip: Option<u64>) -> Option<u64> {
    match node {
        LayoutNode::Leaf { id } => {
            if Some(*id) == skip {
                None
            } else {
                Some(*id)
            }
        }
        LayoutNode::Split { first, second, .. } => {
            first_leaf_skipping(first, skip).or_else(|| first_leaf_skipping(second, skip))
        }
    }
}

/// 子树里最靠前的叶子 id（组 → 第一个叶子；叶子 → 它自己）
fn first_leaf_of(node: &LayoutNode) -> Option<u64> {
    match node {
        LayoutNode::Leaf { id } => Some(*id),
        LayoutNode::Split { first, second, .. } => {
            first_leaf_of(first).or_else(|| first_leaf_of(second))
        }
    }
}

/// 任意节点的 id（叶子 → 叶子 id；组 → 组 id）
fn node_id(node: &LayoutNode) -> u64 {
    match node {
        LayoutNode::Leaf { id } => *id,
        LayoutNode::Split { id, .. } => *id,
    }
}

/// 按 id 找节点（叶子或组）
fn find_node(node: &LayoutNode, id: u64) -> Option<&LayoutNode> {
    match node {
        LayoutNode::Leaf { id: lid } if *lid == id => Some(node),
        LayoutNode::Split {
            id: gid,
            first,
            second,
            ..
        } => {
            if *gid == id {
                Some(node)
            } else {
                find_node(first, id).or_else(|| find_node(second, id))
            }
        }
        _ => None,
    }
}

fn find_node_mut(node: &mut LayoutNode, id: u64) -> Option<&mut LayoutNode> {
    let self_is_target = match &*node {
        LayoutNode::Leaf { id: lid } => *lid == id,
        LayoutNode::Split { id: gid, .. } => *gid == id,
    };
    if self_is_target {
        return Some(node);
    }
    match node {
        LayoutNode::Leaf { .. } => None,
        LayoutNode::Split { first, second, .. } => {
            find_node_mut(first, id).or_else(|| find_node_mut(second, id))
        }
    }
}

/// `outer` 这棵子树里是否有 `inner`（含 outer == inner）
fn node_contains(node: &LayoutNode, outer: u64, inner: u64) -> bool {
    if outer == inner {
        return find_node(node, outer).is_some();
    }
    find_node(node, outer).is_some_and(|n| find_node(n, inner).is_some())
}

/// 子树里的叶子数（组折叠横条的标签用）
fn count_leaves(node: &LayoutNode) -> usize {
    match node {
        LayoutNode::Leaf { .. } => 1,
        LayoutNode::Split { first, second, .. } => count_leaves(first) + count_leaves(second),
    }
}

/// 树里出现过的最大**组** id（restore 后抬 `next_group`，否则新组会撞已有 id）
fn max_group_id(node: &LayoutNode) -> u64 {
    match node {
        LayoutNode::Leaf { .. } => 0,
        LayoutNode::Split {
            id, first, second, ..
        } => (*id).max(max_group_id(first)).max(max_group_id(second)),
    }
}

/// 把 id 匹配的节点整体取出（原位留一个 `u64::MAX` 占位叶子），配合 [`fill_placeholder`]。
fn take_node(node: &mut LayoutNode, target: u64) -> Option<LayoutNode> {
    match node {
        LayoutNode::Leaf { id } if *id == target => {
            Some(std::mem::replace(node, LayoutNode::Leaf { id: u64::MAX }))
        }
        LayoutNode::Split {
            id, first, second, ..
        } => {
            if *id == target {
                return Some(std::mem::replace(node, LayoutNode::Leaf { id: u64::MAX }));
            }
            take_node(first, target).or_else(|| take_node(second, target))
        }
        _ => None,
    }
}

/// 把占位叶子（`u64::MAX`）换成 `new`。
fn fill_placeholder(node: &mut LayoutNode, new: LayoutNode) -> bool {
    fn walk(node: &mut LayoutNode, slot: &mut Option<LayoutNode>) -> bool {
        match node {
            LayoutNode::Leaf { id } if *id == u64::MAX => match slot.take() {
                Some(n) => {
                    *node = n;
                    true
                }
                None => false,
            },
            LayoutNode::Split { first, second, .. } => walk(first, slot) || walk(second, slot),
            _ => false,
        }
    }
    walk(node, &mut Some(new))
}

/// 递归把所有分界设成等分（跳过直接挨着 rail 的那一层，否则侧栏宽度会被抹平）
fn equalize_subtree(node: &mut LayoutNode, rail: Option<u64>) {
    if let LayoutNode::Split {
        ratio,
        first,
        second,
        ..
    } = node
    {
        let touches_rail = matches!(&**first, LayoutNode::Leaf { id } if Some(*id) == rail)
            || matches!(&**second, LayoutNode::Leaf { id } if Some(*id) == rail);
        if !touches_rail {
            *ratio = 0.5;
        }
        equalize_subtree(first, rail);
        equalize_subtree(second, rail);
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
#[derive(Clone, serde::Serialize)]
pub struct LayoutDump {
    pub tree: serde_json::Value,
    pub active: u64,
    pub zoomed: Option<u64>,
    /// 最小化的节点 id（叶子或组；可多个）
    pub minimized: Vec<u64>,
    /// 浮动 pane 槽位(阶段②;空 = 无浮窗)。只含几何/z/pin,组件由 id 复用。
    pub floats: Vec<FloatSlot>,
    /// 堆叠组(阶段②)。每组 `members[0]` 是锚(持有槽位、也是当前显示的)。
    pub stacks: Vec<StackGroup>,
    /// 叶子扁平列表（树序；测试/JS 读 fixed 用）
    pub leafs: Vec<LeafInfo>,
}

/// 叶子信息（dump 的 leafs 元素）
#[derive(Clone, serde::Serialize)]
pub struct LeafInfo {
    pub id: u64,
    pub fixed: bool,
    pub rail: bool,
}

pub(crate) trait EventResultExt {
    fn is_ignored(&self) -> bool;
}
impl EventResultExt for EventResult {
    fn is_ignored(&self) -> bool {
        matches!(self, EventResult::Ignored(_))
    }
}

/// std::any::type_name 取最后一段（最小化横条标题用）
fn short_type_name(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::plugin_terminal::PluginTerminal;

    /// 鼠标点击换焦点的排除规则：浮窗盖住的点、zoom 期间一律不换（否则焦点被送到
    /// 被盖住的叶子，或送进一个此刻根本看不见的叶子）。
    #[test]
    fn focus_target_at_skips_floats_and_zoom() {
        let mut tree = LayoutTree::default();
        tree.set_editor(Box::new(PluginTerminal::new(0, 0, 80)));
        let tid = tree
            .split_side(
                0,
                SplitDir::H,
                false,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap();
        // 渲染过的几何（leaf_rects 平时由 render 填）
        tree.leaf_rects.insert(0, Rect::new(0, 0, 40, 24));
        tree.leaf_rects.insert(tid, Rect::new(40, 0, 40, 24));
        let area = Rect::new(0, 0, 80, 24);

        assert_eq!(tree.focus_target_at(area, 10, 5), Some(0), "落在左叶");
        assert_eq!(tree.focus_target_at(area, 50, 5), Some(tid), "落在右叶");
        assert_eq!(tree.focus_target_at(area, 200, 200), None, "落在树外");

        // 浮窗盖住中心：“底下压着哪个叶子都不重要，焦点不该跟着点击跑”
        tree.set_float(tid);
        let fr = tree.floats()[0].rect_of(area);
        assert_eq!(
            tree.focus_target_at(area, fr.x + fr.width / 2, fr.y + fr.height / 2),
            None,
            "落在浮窗上不换平铺叶子的焦点"
        );

        // zoom 期间 leaf_rects 是上一帧（未 zoom）的几何：不能用它换焦点
        tree.unfloat();
        tree.zoom(0);
        assert_eq!(
            tree.focus_target_at(area, 50, 5),
            None,
            "zoom 时不拿旧几何换焦点"
        );
    }

    #[test]
    fn remove_clears_float() {
        let mut tree = LayoutTree::default();
        // default 树的 components 是空的：先挂一个 id=0 占位组件（split 依赖它存在）
        tree.set_editor(Box::new(PluginTerminal::new(0, 0, 80)));
        let tid = tree
            .split_side(
                0,
                SplitDir::H,
                true,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap();
        tree.set_float(tid);
        assert_eq!(tree.floating(), Some(tid));
        // 关闭浮动终端（q）后 float 必须清空：残留会让事件路由指向已删叶子，
        // 所有按键被 Ignored(None) 吞掉 → 程序僵死
        tree.remove(tid);
        assert_eq!(tree.floating(), None);
        assert_eq!(tree.active(), 0, "active 回落编辑器叶子");
    }

    #[test]
    fn float_leaf_does_not_consume_layout_space() {
        // :term 浮动时 dock 位置不该留白：浮动叶子不占布局，其余叶子占满区域
        let mut tree = LayoutTree::default();
        tree.set_editor(Box::new(PluginTerminal::new(0, 0, 80)));
        let tid = tree
            .split_side(
                0,
                SplitDir::V,
                false,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap();
        let area = Rect::new(0, 0, 80, 24);
        // 未浮动：终端占底部，编辑器只剩上部
        let mut rects = Vec::new();
        layout_node(&tree.root, area, &mut rects, &mut Vec::new(), &[]);
        assert_eq!(rects.len(), 2);
        let editor_rect = rects.iter().find(|(id, _)| *id == 0).unwrap().1;
        assert!(editor_rect.height < area.height, "未浮动时编辑器被切分");
        // 浮动后：编辑器占满整个区域（无底部空白）
        tree.set_float(tid);
        let mut rects = Vec::new();
        layout_node(
            &tree.root,
            area,
            &mut rects,
            &mut Vec::new(),
            &tree.layout_skip_ids(),
        );
        assert_eq!(rects.len(), 1, "浮动叶子不产生布局 rect");
        assert_eq!(rects[0].0, 0);
        assert_eq!(rects[0].1, area, "编辑器占满整个区域");
    }

    #[test]
    fn neighbor_and_dir_ops() {
        // 树：editor(0) | [termA(1) / termB(2)]（外层 H，内层 V）
        let mut tree = LayoutTree::default();
        tree.set_editor(Box::new(PluginTerminal::new(0, 0, 80)));
        let a = tree
            .split_side(
                0,
                SplitDir::H,
                false,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap(); // termA 在 second
        let b = tree
            .split_side(
                a,
                SplitDir::V,
                false,
                Box::new(PluginTerminal::new(2, 2, 80)),
            )
            .unwrap(); // termB 在 termA 下方
                       // 结构：root H(editor | V(termA / termB))

        // termB 的右邻居：无（H 分割里 termB 在 root 的 second 子树，右邻居不存在）
        assert_eq!(
            tree.neighbor_leaf(b, SplitDir::H, false),
            None,
            "termB 右侧边界"
        );
        // termB 的左邻居：跨过 V 分割向上 → editor（H 的 first 子树靠 second 边极值）
        assert_eq!(
            tree.neighbor_leaf(b, SplitDir::H, true),
            Some(0),
            "termB 左侧 = editor"
        );
        // termB 的上邻居：termA（V 分割 first 侧）
        assert_eq!(
            tree.neighbor_leaf(b, SplitDir::V, true),
            Some(a),
            "termB 上方 = termA"
        );
        // termB 的下邻居：无
        assert_eq!(tree.neighbor_leaf(b, SplitDir::V, false), None);
        // editor 的右邻居：V(termA/termB) 子树靠 first 边极值 = termA
        assert_eq!(
            tree.neighbor_leaf(0, SplitDir::H, false),
            Some(a),
            "editor 右侧 = termA"
        );
        // editor 的左邻居：无
        assert_eq!(tree.neighbor_leaf(0, SplitDir::H, true), None);

        // focus_dir：聚焦 editor 右侧邻居
        assert_eq!(tree.focus_dir(0, SplitDir::H, false), Some(a));
        assert_eq!(tree.active(), a);
        // swap_dir：editor 与右侧邻居交换
        assert!(tree.swap_dir(0, SplitDir::H, false));
        assert!(
            tree.components
                .get(&0)
                .unwrap()
                .as_any()
                .downcast_ref::<PluginTerminal>()
                .is_some(),
            "swap 后 editor 位置是终端组件"
        );
        // 边界 swap 失败
        assert!(!tree.swap_dir(0, SplitDir::H, true), "editor 左侧无邻居");

        // equalize：termB 所在内层 V 分割恢复 50/50
        tree.resize_leaf_dir(a, SplitDir::V, 0.2);
        tree.equalize(a);
        let mut rects = Vec::new();
        layout_node(
            &tree.root,
            Rect::new(0, 0, 100, 40),
            &mut rects,
            &mut Vec::new(),
            &[],
        );
        let rect_a = rects.iter().find(|(id, _)| *id == a).unwrap().1;
        let rect_b = rects.iter().find(|(id, _)| *id == b).unwrap().1;
        assert_eq!(rect_a.height, rect_b.height, "equalize 后上下等高");
    }

    #[test]
    fn resize_leaf_dir_directional() {
        // editor|term 左右分割（term 在 second 侧），初始 ratio 0.5
        let mut tree = LayoutTree::default();
        tree.set_editor(Box::new(PluginTerminal::new(0, 0, 80)));
        let tid = tree
            .split_side(
                0,
                SplitDir::H,
                false,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap();
        // term 增大 0.2：second 侧 → ratio 0.5-0.2=0.3
        assert!(tree.resize_leaf_dir(tid, SplitDir::H, 0.2));
        let mut rects = Vec::new();
        layout_node(
            &tree.root,
            Rect::new(0, 0, 100, 10),
            &mut rects,
            &mut Vec::new(),
            &[],
        );
        let term_rect = rects.iter().find(|(id, _)| *id == tid).unwrap().1;
        assert_eq!(term_rect.x, 30, "term 占 30%");
        // editor 增大 0.1：first 侧 → ratio 0.3+0.1=0.4
        assert!(tree.resize_leaf_dir(0, SplitDir::H, 0.1));
        // 方向不匹配（V 对 H 分割）→ 不生效
        assert!(!tree.resize_leaf_dir(tid, SplitDir::V, 0.1));
        // 不存在 id → false
        assert!(!tree.resize_leaf_dir(999, SplitDir::H, 0.1));
    }

    #[test]
    fn swap_leaves_swaps_content() {
        // 两个叶子组件互换：树结构/焦点不动，渲染位置内容互换
        let mut tree = LayoutTree::default();
        tree.set_editor(Box::new(PluginTerminal::new(0, 0, 80)));
        let tid = tree
            .split_side(
                0,
                SplitDir::H,
                false,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap();
        assert!(tree.swap(0, tid));
        // id0 位置现在是 PluginTerminal（原 tid 的组件）
        assert!(
            tree.components
                .get(&0)
                .unwrap()
                .as_any()
                .downcast_ref::<PluginTerminal>()
                .is_some(),
            "swap 后 id0 位置应是终端组件"
        );
        // 无效 swap：不存在 id / 相同 id
        assert!(!tree.swap(0, 999));
        assert!(!tree.swap(tid, tid));
    }

    #[test]
    fn fixed_leaf_skipped_by_ops() {
        // fixed 叶子:swap/resize/remove 直接拒绝;焦点可穿过;取消标记后恢复可操作
        let mut tree = LayoutTree::default();
        tree.set_editor(Box::new(PluginTerminal::new(0, 0, 80)));
        let panel = tree
            .split_side(
                0,
                SplitDir::H,
                false,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap();
        tree.set_fixed(panel, true);
        assert!(
            !tree.resize_leaf_dir(panel, SplitDir::H, 0.1),
            "fixed 不可 resize"
        );
        assert!(!tree.remove(panel), "fixed 不可 remove");
        assert!(!tree.swap(0, panel), "fixed 不可 swap");
        // 焦点可穿过 fixed 叶子
        assert_eq!(
            tree.focus_dir(0, SplitDir::H, false),
            Some(panel),
            "焦点可移到 fixed"
        );
        // dump 输出含 fixed 字段
        let dump = tree.dump();
        let l = dump.leafs.iter().find(|l| l.id == panel).unwrap();
        assert!(l.fixed, "dump 含 fixed 字段");
        assert!(
            !dump.leafs.iter().find(|l| l.id == 0).unwrap().fixed,
            "编辑器默认不 fixed"
        );
        // 取消标记后恢复可操作
        tree.set_fixed(panel, false);
        assert!(tree.remove(panel), "取消 fixed 后可 remove");
    }

    #[test]
    fn minimize_excludes_leaf_and_restores() {
        // minimized 叶子不占布局空间，其余叶子占满；恢复后回到原分割
        let mut tree = LayoutTree::default();
        tree.set_editor(Box::new(PluginTerminal::new(0, 0, 80)));
        let tid = tree
            .split_side(
                0,
                SplitDir::V,
                false,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap();
        tree.set_minimized(tid, true);
        assert_eq!(tree.minimized_leaf(), Some(tid));
        assert_eq!(tree.active(), 0, "最小化时焦点回编辑器");
        let area = Rect::new(0, 0, 80, 24);
        let mut rects = Vec::new();
        layout_node(
            &tree.root,
            area,
            &mut rects,
            &mut Vec::new(),
            &tree.layout_skip_ids(),
        );
        assert_eq!(rects.len(), 1, "minimized 叶子不产生布局 rect");
        assert_eq!(rects[0].1, area, "其余叶子占满整个区域");
        // 恢复
        tree.set_minimized(tid, false);
        assert_eq!(tree.minimized_leaf(), None);
        let mut rects = Vec::new();
        layout_node(
            &tree.root,
            area,
            &mut rects,
            &mut Vec::new(),
            &tree.layout_skip_ids(),
        );
        assert_eq!(rects.len(), 2, "恢复后回到原分割");
    }
}

// ---- 组（容器节点）单元测试 ----

#[cfg(test)]
mod group_tests {
    use super::*;
    use crate::ui::plugin_terminal::PluginTerminal;

    fn term(n: u64) -> Box<dyn Component> {
        Box::new(PluginTerminal::new(n, n, 80))
    }

    /// `ed1 | ed2`（左右分）→ 在**根**上切一刀（下方）⇒ term 跨满整宽。
    /// 这就是"地板上的终端横跨左右两个编辑区"：切的不是叶子，是整棵树。
    #[test]
    fn split_at_root_spans_full_width() {
        let mut tree = LayoutTree::default();
        tree.set_editor(term(0));
        // ed1 | ed2
        let ed2 = tree
            .split_side(0, SplitDir::H, false, term(1))
            .expect("分出 ed2");
        let area = Rect::new(0, 0, 100, 30);
        // 旧行为对照：在 ed2 上切，term 只有 ed2 那一列的宽度
        let mut leafwise = LayoutTree::default();
        leafwise.set_editor(term(0));
        let ed2b = leafwise.split_side(0, SplitDir::H, false, term(1)).unwrap();
        leafwise
            .split_side(ed2b, SplitDir::V, false, term(2))
            .unwrap();
        let rects = |t: &LayoutTree| {
            let mut v = Vec::new();
            layout_node(&t.root, area, &mut v, &mut Vec::new(), &t.layout_skip_ids());
            v
        };
        let cramped = rects(&leafwise)
            .into_iter()
            .find(|(id, _)| *id == 2)
            .unwrap()
            .1;
        assert!(cramped.width < area.width, "叶子切分：term 只有半宽");

        // 新行为：在根上切 → term 整宽
        let tid = tree
            .split_at_group(None, SplitDir::V, false, 0.7, term(2))
            .expect("根上切一刀");
        let root_rect = rects(&tree)
            .into_iter()
            .find(|(id, _)| *id == tid)
            .unwrap()
            .1;
        assert_eq!(root_rect.width, area.width, "term 占满整宽");
        assert_eq!(root_rect.height, 9, "按 ratio 拿到底部 30% 的高度");
        // ed1/ed2 还在上面、宽度不变
        let ed1 = rects(&tree).into_iter().find(|(id, _)| *id == 0).unwrap().1;
        assert_eq!(ed1.width, 50, "ed1 宽度不变");
        assert_eq!(ed1.height, 21);
        assert_eq!(ed2, 1);
    }

    /// 在**组**上切：区域是该组的包围盒（比如"只把上面那一排"切开）
    #[test]
    fn split_at_group_uses_group_box() {
        let mut tree = LayoutTree::default();
        tree.set_editor(term(0));
        tree.split_side(0, SplitDir::H, false, term(1)).unwrap();
        tree.split_at_group(None, SplitDir::V, false, 0.7, term(2))
            .unwrap();
        // 现在根 = (上面的组) / term2；找上面那个组
        let top = tree
            .group_ids()
            .into_iter()
            .find(|g| tree.group_leaves(*g) == vec![0, 1])
            .expect("上面那排是一个组");
        let nid = tree
            .split_at_group(Some(top), SplitDir::V, false, 0.5, term(3))
            .expect("在组上切");
        let area = Rect::new(0, 0, 100, 30);
        let mut v = Vec::new();
        layout_node(&tree.root, area, &mut v, &mut Vec::new(), &[]);
        let r = v.iter().find(|(id, _)| *id == nid).unwrap().1;
        assert_eq!(r.width, 100, "新 pane 跨满那个组的整宽");
        assert_eq!(v.len(), 4, "4 个叶子都在布局里");
    }

    /// 组级 equalize：组内所有分界回到等分（旧 `equalize` 只管最内层那一层）
    #[test]
    fn equalize_group_flattens_every_split_inside() {
        let mut tree = LayoutTree::default();
        tree.set_editor(term(0));
        let a = tree.split_side(0, SplitDir::H, false, term(1)).unwrap();
        tree.split_side(a, SplitDir::V, false, term(2)).unwrap();
        // 根组 = 里面所有叶子的组
        let root_group = tree.group_ids()[0];
        let area = Rect::new(0, 0, 100, 40);
        let rects = |t: &LayoutTree| {
            let mut v = Vec::new();
            layout_node(&t.root, area, &mut v, &mut Vec::new(), &[]);
            v.into_iter()
                .collect::<std::collections::HashMap<u64, Rect>>()
        };
        tree.resize_leaf_dir(0, SplitDir::H, 0.3);
        assert_eq!(rects(&tree)[&0].width, 80, "先被拉宽到 80%");
        assert!(tree.equalize_node(root_group), "组级 equalize 生效");
        let after = rects(&tree);
        assert_eq!(after[&0].width, 50, "根分界回到 50/50");
        assert_eq!(after[&1].height, after[&2].height, "右列上下也回到等分");
        assert!(!tree.equalize_node(999), "不存在的 id → false");
    }

    /// 组级最小化：整组折成一条，兄弟占满；展开原样回来
    #[test]
    fn minimize_group_folds_whole_subtree() {
        let mut tree = LayoutTree::default();
        tree.set_editor(term(0));
        tree.split_side(0, SplitDir::H, false, term(1)).unwrap();
        let tid = tree
            .split_at_group(None, SplitDir::V, false, 0.7, term(2))
            .unwrap();
        let top = tree
            .group_ids()
            .into_iter()
            .find(|g| tree.group_leaves(*g) == vec![0, 1])
            .expect("上面那排是一个组");
        tree.set_minimized(top, true);
        assert_eq!(tree.minimized_ids(), &[top]);
        let area = Rect::new(0, 0, 100, 30);
        let mut v = Vec::new();
        layout_node(
            &tree.root,
            area,
            &mut v,
            &mut Vec::new(),
            &tree.layout_skip_ids(),
        );
        assert_eq!(v.len(), 1, "整组不占布局");
        assert_eq!(v[0].0, tid, "只剩底部的 term");
        assert_eq!(v[0].1, area, "它占满整个区域");
        // 展开
        tree.set_minimized(top, false);
        assert!(tree.minimized_ids().is_empty());
        let mut v = Vec::new();
        layout_node(&tree.root, area, &mut v, &mut Vec::new(), &[]);
        assert_eq!(v.len(), 3, "三个叶子都回来了");
    }

    /// 事后修形：把已经嵌在半宽列里的窗口搬到整窗底边
    #[test]
    fn move_into_lifts_nested_pane_to_full_width() {
        let mut tree = LayoutTree::default();
        tree.set_editor(term(0));
        // ed1 | (ed2 / term)：term 只有右半宽
        let ed2 = tree.split_side(0, SplitDir::H, false, term(1)).unwrap();
        let term_id = tree.split_side(ed2, SplitDir::V, false, term(2)).unwrap();
        let area = Rect::new(0, 0, 100, 30);
        let rects = |t: &LayoutTree| {
            let mut v = Vec::new();
            layout_node(&t.root, area, &mut v, &mut Vec::new(), &t.layout_skip_ids());
            v.into_iter()
                .collect::<std::collections::HashMap<u64, Rect>>()
        };
        assert!(rects(&tree)[&term_id].width < area.width, "前提：半宽");

        assert!(tree.move_into(term_id, None, SplitDir::V, false, 0.7));
        let after = rects(&tree);
        assert_eq!(after[&term_id].width, 100, "搬到整窗底边后横跨整宽");
        assert_eq!(after[&term_id].y + after[&term_id].height, 30, "贴底");
        assert_eq!(tree.active(), term_id, "搬完焦点跟着它");
        assert_eq!(tree.leaf_ids().len(), 3, "没有叶子丢失");
        // 原位收缩：ed2 现在独占上面那半边
        assert_eq!(after[&ed2].height, 21);
    }

    /// 搬到某个**组**的边缘 / 与某个叶子并排
    #[test]
    fn move_into_group_and_sibling() {
        let mut tree = LayoutTree::default();
        tree.set_editor(term(0));
        let b = tree.split_side(0, SplitDir::H, false, term(1)).unwrap();
        let c = tree.split_side(b, SplitDir::V, false, term(2)).unwrap();
        let area = Rect::new(0, 0, 100, 30);
        let rects = |t: &LayoutTree| {
            let mut v = Vec::new();
            layout_node(&t.root, area, &mut v, &mut Vec::new(), &t.layout_skip_ids());
            v.into_iter()
                .collect::<std::collections::HashMap<u64, Rect>>()
        };
        // 把 c 搬到根（整窗）右侧 ⇒ 整高
        assert!(tree.move_into(c, None, SplitDir::H, false, 0.3));
        assert_eq!(rects(&tree)[&c].height, 30, "贴右边 ⇒ 整高");
        // 把 c 再搬去和 0 并排（叶子目标）
        assert!(tree.move_into(c, Some(0), SplitDir::V, false, 0.5));
        let r = rects(&tree);
        assert_eq!(r[&c].width, r[&0].width, "与 0 并排 ⇒ 同宽");
        // 搬进自己的子树 / 搬 rail / 搬到不存在的目标 ⇒ false
        assert!(!tree.move_into(c, Some(c), SplitDir::H, false, 0.5));
        assert!(!tree.move_into(999, None, SplitDir::H, false, 0.5));
    }

    /// 整窗切分不能把 rail 卷进去：rail 必须仍是 root 的直接子叶（贴边全高）
    #[test]
    fn root_split_keeps_rail_as_direct_edge_child() {
        let mut tree = LayoutTree::default();
        tree.set_editor(term(0));
        tree.register_rail(term(9), true, 0.2);
        let rid = tree.rail_leaf().expect("有 rail");
        let area = Rect::new(0, 0, 100, 30);
        // 整窗切一刀：新窗口横跨**main 区**整宽，rail 仍是整高的一列
        let tid = tree
            .split_at_group(None, SplitDir::V, false, 0.7, term(2))
            .expect("根上切");
        let mut v = Vec::new();
        layout_node(
            &tree.root,
            area,
            &mut v,
            &mut Vec::new(),
            &tree.layout_skip_ids(),
        );
        let r: std::collections::HashMap<u64, Rect> = v.into_iter().collect();
        assert_eq!(r[&rid].height, 30, "rail 仍占满全高");
        assert_eq!(r[&tid].x, r[&rid].width, "新窗口在 rail 右侧起");
        assert_eq!(r[&tid].width, 100 - r[&rid].width, "横跨 main 区整宽");
        assert!(
            tree.rail_edge().is_some(),
            "rail 仍在 root 边缘（rail_edge 可用）"
        );
    }

    /// 组 zoom：组内各叶子按原分割铺开占满全区，组外叶子不占布局，事件仍落在真叶子上
    #[test]
    fn zoom_group_lays_out_subtree_full_area() {
        let mut tree = LayoutTree::default();
        tree.set_editor(term(0));
        let b = tree.split_side(0, SplitDir::H, false, term(1)).unwrap();
        let top = tree
            .group_ids()
            .into_iter()
            .find(|g| tree.group_leaves(*g) == vec![0, b])
            .expect("两个编辑器组成一个组");
        let bottom = tree
            .split_at_group(None, SplitDir::V, false, 0.7, term(2))
            .unwrap();
        // 缩小一下组（模拟 ratio 不等）
        tree.resize_leaf_dir(0, SplitDir::H, 0.3);
        assert_ne!(tree.active(), 0, "活动叶是刚切出来的");

        tree.zoom(top);
        assert_eq!(tree.zoomed(), Some(top), "组可以 zoom");
        // 组 zoom 时 active() 必须是组内的真叶子（事件路由要它）
        let a = tree.active();
        assert!(
            tree.group_leaves(top).contains(&a),
            "active()={a} 必须落在组内（否则事件路由会指向组）"
        );
        let area = Rect::new(0, 0, 100, 30);
        let mut v = Vec::new();
        let mut g = Vec::new();
        layout_node(
            find_node(&tree.root, top).unwrap(),
            area,
            &mut v,
            &mut g,
            &tree.layout_skip_ids(),
        );
        let r: std::collections::HashMap<u64, Rect> = v.into_iter().collect();
        assert_eq!(r.len(), 2, "只有组内两个叶子");
        assert!(!r.contains_key(&bottom), "组外叶子不出现在 zoom 里");
        assert_eq!(r[&0].height, 30, "组内叶子铺满整高");
        // 上面 resize 过 → 组内是 80/20，zoom 不改变组内分界
        assert_eq!(r[&0].width, 80, "组内分界保持（缩放不改比例）");
        assert_eq!(r[&b].width, 20);
        // 取消 zoom 后回到原状
        tree.unzoom();
        assert_eq!(tree.zoomed(), None);
    }

    /// 组级 resize：只改它自己那条分界（"这一排占多少"），不动祖先
    #[test]
    fn resize_share_moves_only_the_group_boundary() {
        let mut tree = LayoutTree::default();
        tree.set_editor(term(0));
        tree.split_side(0, SplitDir::H, false, term(1)).unwrap();
        let top = tree
            .group_ids()
            .into_iter()
            .find(|g| tree.group_leaves(*g) == vec![0, 1])
            .expect("上排是组");
        tree.split_at_group(None, SplitDir::V, false, 0.5, term(2))
            .unwrap();
        let area = Rect::new(0, 0, 100, 40);
        let rects = |t: &LayoutTree| {
            let mut v = Vec::new();
            layout_node(&t.root, area, &mut v, &mut Vec::new(), &t.layout_skip_ids());
            v.into_iter()
                .collect::<std::collections::HashMap<u64, Rect>>()
        };
        assert_eq!(rects(&tree)[&0].height, 20, "初始上下一半");
        assert!(tree.resize_share(top, 0.75), "组级 resize 生效");
        let after = rects(&tree);
        assert_eq!(after[&0].height, 30, "这一排拿到 75% 的整高");
        assert_eq!(after[&2].height, 10, "底部让出空间");
        assert_eq!(after[&0].width, 50, "组内左右分割不受影响");
        assert!(!tree.resize_share(999, 0.5), "不存在的 id → false");
    }

    /// 节点查询：kind / children / contains，以及"组 id 不与叶子 id 撞"
    #[test]
    fn node_kind_and_children() {
        let mut tree = LayoutTree::default();
        tree.set_editor(term(0));
        let b = tree.split_side(0, SplitDir::H, false, term(1)).unwrap();
        let groups = tree.group_ids();
        assert_eq!(groups.len(), 1, "一次切分产生一个组");
        let g = groups[0];
        assert!(g >= GROUP_ID_BASE, "组 id 走独立号段");
        assert_eq!(tree.node_kind(g), Some(NodeKind::Group));
        assert_eq!(tree.node_kind(0), Some(NodeKind::Leaf));
        assert_eq!(tree.node_kind(4242), None);
        assert_eq!(tree.node_kind(b), Some(NodeKind::Leaf));
        assert_eq!(tree.group_children(g), Some(vec![0, b]));
        assert_eq!(tree.group_leaves(g), vec![0, b]);
        assert!(tree.node_contains(g, b), "组包含它的叶子");
        assert!(!tree.node_contains(b, g), "叶子不包含组");
    }
}

// ---- rail 单元测试 ----

#[cfg(test)]
mod rail_tests {
    use super::*;
    use crate::ui::plugin_terminal::PluginTerminal;

    fn base() -> LayoutTree {
        let mut tree = LayoutTree::default();
        tree.set_editor(Box::new(PluginTerminal::new(0, 0, 80)));
        tree
    }

    #[test]
    fn register_rail_wraps_root_and_marks_dump() {
        let mut tree = base();
        tree.register_rail(Box::new(PluginTerminal::new(9, 0, 80)), true, 0.2);
        let rid = tree.rail_leaf().expect("注册后有 rail");
        assert_eq!(tree.rail_leaf(), Some(rid));
        assert!(tree.is_rail(rid));
        // root = Split(rail(1) | main(0))
        match &tree.root {
            LayoutNode::Split { first, second, .. } => {
                assert_eq!(tree.leaf_ids_of(first), vec![rid], "rail 在 first(左侧)");
                assert!(tree.leaf_ids_of(second).contains(&0), "main 含编辑器");
            }
            _ => panic!("注册 rail 后 root 应为 Split"),
        }
        let dump = tree.dump();
        let rail_leaf = dump.leafs.iter().find(|l| l.id == rid).unwrap();
        assert!(rail_leaf.rail, "dump 标记 rail");
        let editor_leaf = dump.leafs.iter().find(|l| l.id == 0).unwrap();
        assert!(!editor_leaf.rail, "编辑器非 rail");
        // 右侧 rail 对称:root=Split(main|rail)
        let mut tree2 = base();
        tree2.register_rail(Box::new(PluginTerminal::new(8, 0, 80)), false, 0.2);
        let rid2 = tree2.rail_leaf().expect("注册后有 rail");
        match &tree2.root {
            LayoutNode::Split { first, second, .. } => {
                assert!(
                    tree2.leaf_ids_of(second).contains(&rid2),
                    "rail 在 second(右侧)"
                );
                assert!(tree2.leaf_ids_of(first).contains(&0));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn rail_replace_and_take() {
        let mut tree = base();
        tree.register_rail(Box::new(PluginTerminal::new(9, 0, 80)), true, 0.2);
        let r1 = tree.rail_leaf().unwrap();
        // 替换:旧 rail 被取除,新 rail 就位,root 仍单层 Split(rail|main)
        let replaced = tree
            .register_rail(Box::new(PluginTerminal::new(10, 0, 80)), true, 0.3)
            .unwrap();
        let r2 = tree.rail_leaf().unwrap();
        assert_eq!(r1, replaced, "返回被替换的旧 rail id");
        assert!(!tree.is_rail(r1) || r1 != r2);
        match &tree.root {
            LayoutNode::Split { first, second, .. } => {
                assert!(tree.leaf_ids_of(first).contains(&r2));
                assert!(tree.leaf_ids_of(second).contains(&0));
            }
            _ => panic!(),
        }
        // 取除:root 收缩回编辑器叶
        tree.take_rail();
        assert_eq!(tree.rail_leaf(), None);
        assert!(matches!(tree.root, LayoutNode::Leaf { id: 0 }));
        assert!(!tree.components.contains_key(&r2), "rail 组件已移除");
    }

    /// `restore_layout` 接线:dump → restore 往返,形状/比例/活动叶子一致
    #[test]
    fn restore_roundtrip_keeps_shape() {
        let mut t = base();
        let a = t
            .split_side_ratio(
                0,
                SplitDir::H,
                false,
                0.3,
                Box::new(PluginTerminal::new(1, 1, 80)),
                1,
            )
            .unwrap();
        t.split_side_ratio(
            a,
            SplitDir::V,
            false,
            0.7,
            Box::new(PluginTerminal::new(2, 2, 80)),
            2,
        )
        .unwrap();
        let json = serde_json::to_value(t.dump()).unwrap();
        let ids = t.leaf_ids();
        let active = t.active();

        // 同组组件、但形状不同的另一棵树:restore 后应完全对齐 dump
        let mut t2 = base();
        let a2 = t2
            .split_side_ratio(
                0,
                SplitDir::H,
                false,
                0.9,
                Box::new(PluginTerminal::new(1, 1, 80)),
                1,
            )
            .unwrap();
        t2.split_side_ratio(
            a2,
            SplitDir::V,
            true,
            0.1,
            Box::new(PluginTerminal::new(2, 2, 80)),
            2,
        )
        .unwrap();
        t2.restore(&json).unwrap();

        assert_eq!(t2.leaf_ids(), ids, "叶子集合与树序");
        assert_eq!(t2.active(), active, "活动叶子");
        assert_eq!(
            serde_json::to_value(t2.dump()).unwrap()["tree"],
            json["tree"],
            "树形状(含 ratio 与 first/second 次序)"
        );
    }

    /// 堆叠组也要随 dump 往返(否则重启/restore 后堆叠丢失,两个 pane 变回独立)
    #[test]
    fn restore_roundtrip_keeps_stacks() {
        let mut t = base();
        let b = t
            .split_side(
                0,
                SplitDir::H,
                false,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap();
        t.stack_new_with_sibling(0).unwrap();
        t.stack_rotate(0, true).unwrap(); // 锚换成 b,考验顺序也被保留
        let json = serde_json::to_value(t.dump()).unwrap();

        let mut t2 = base();
        t2.split_side(
            0,
            SplitDir::H,
            false,
            Box::new(PluginTerminal::new(1, 1, 80)),
        )
        .unwrap();
        assert!(!t2.is_stacked(0) && !t2.is_stacked(b), "重建前无堆叠");
        t2.restore(&json).unwrap();

        assert!(t2.is_stacked(b), "锚(轮转后的)回来了");
        assert_eq!(t2.stack_members(b), vec![b, 0], "成员顺序也保留");
        assert_eq!(t2.stack_anchor_of(0), Some(b));

        // 成员已不存在 → 整组丢弃(否则 skip 集会指向已删叶子)
        let mut t3 = base();
        t3.restore(&json).unwrap();
        assert!(
            !t3.is_stacked(0) && !t3.is_stacked(b),
            "仅剩一个存活成员 → 组丢弃"
        );
    }

    /// 浮窗也要随 dump 往返(否则 restore 后浮窗会消失)
    #[test]
    fn restore_roundtrip_keeps_floats() {
        let mut t = base();
        t.split_side_ratio(
            0,
            SplitDir::H,
            false,
            0.5,
            Box::new(PluginTerminal::new(1, 1, 80)),
            1,
        )
        .unwrap();
        t.add_float(FloatSlot {
            x: 0.7,
            y: 0.6,
            w: 0.25,
            h: 0.3,
            z: 3,
            pinned: true,
            ..FloatSlot::centered(1)
        });
        let json = serde_json::to_value(t.dump()).unwrap();

        let mut t2 = base();
        t2.split_side_ratio(
            0,
            SplitDir::H,
            false,
            0.5,
            Box::new(PluginTerminal::new(1, 1, 80)),
            1,
        )
        .unwrap();
        assert!(t2.floats().is_empty());
        t2.restore(&json).unwrap();

        let f = t2.floats();
        assert_eq!(f.len(), 1, "浮窗槽位回来了");
        assert_eq!(
            (f[0].x, f[0].y, f[0].w, f[0].h),
            (0.7, 0.6, 0.25, 0.3),
            "几何"
        );
        assert_eq!(f[0].z, 3);
        assert!(f[0].pinned);

        // 组件已不在时,浮窗槽位要被丢掉(否则路由指向已删叶子)
        let mut t3 = LayoutTree::default();
        t3.set_editor(Box::new(PluginTerminal::new(0, 0, 80)));
        t3.restore(&json).unwrap();
        assert!(t3.floats().is_empty(), "id=1 不存活 → 槽位丢弃");
    }

    /// 已不存在的叶子应被丢弃、空分支收敛;全空则报错
    #[test]
    fn restore_drops_missing_leafs() {
        let mut t = base();
        t.split_side_ratio(
            0,
            SplitDir::H,
            false,
            0.4,
            Box::new(PluginTerminal::new(1, 1, 80)),
            1,
        )
        .unwrap();
        let json = serde_json::to_value(t.dump()).unwrap();

        // 新树只有 id=0(1 已不在)→ 退化为单叶
        let mut t2 = base();
        t2.restore(&json).unwrap();
        assert_eq!(t2.leaf_ids(), vec![0]);
        assert_eq!(t2.active(), 0);

        // 一个活的叶子都没有 → 报错,不把树搞空
        let mut t3 = LayoutTree::default();
        assert!(t3.restore(&json).is_err());
    }

    /// `component_kind_of`:给 `pane.list()` 的 kind 用(取类型名末段)
    #[test]
    fn component_kind_of_short_name() {
        use crate::ui::plugin_panel::PanelSide;
        use crate::ui::PluginPanel;
        let mut tree = base();
        assert_eq!(tree.component_kind_of(0), Some("PluginTerminal"));
        let id = tree
            .split_side(
                0,
                SplitDir::H,
                false,
                Box::new(PluginPanel::new(9, PanelSide::Left, false)),
            )
            .expect("split_side 应返回新叶 id");
        assert_eq!(tree.component_kind_of(id), Some("PluginPanel"));
        assert_eq!(tree.component_kind_of(9999), None, "不存在的 id → None");
    }

    /// A.7 指定的第一件事:兄弟合并 + 轮转前后锚的 rect 必须完全相同。
    /// 这条同时验证 A.6 的整个零映射论证 —— 不过就不要往下写。
    #[test]
    fn stack_siblings_keep_rect_across_rotation() {
        let mut tree = base(); // 编辑器 id=0
        let b = tree
            .split_side(
                0,
                SplitDir::H,
                false,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap();
        assert_eq!(tree.leaf_sibling_of(0), Some(b), "0 与 b 是叶子兄弟");

        assert_eq!(tree.stack_new_with_sibling(0), Some(0), "建组,锚 = 0");
        assert_eq!(tree.stack_members(0), vec![0, b]);
        assert!(tree.is_stacked(b), "b 也算在组内");

        let area = Rect::new(0, 0, 100, 30);
        let rects_of = |t: &LayoutTree| -> std::collections::HashMap<u64, Rect> {
            let mut v = Vec::new();
            layout_node(&t.root, area, &mut v, &mut Vec::new(), &t.layout_skip_ids());
            v.into_iter().collect()
        };
        let before = rects_of(&tree);
        assert!(
            before.contains_key(&0) && !before.contains_key(&b),
            "只有锚占布局空间(非锚被吸收)"
        );
        let anchor_rect = before[&0];
        assert_eq!(
            anchor_rect.width, area.width,
            "兄弟被排除后锚占满整个 split 区域"
        );

        // 轮转 → 新锚 = b,被排除的变成 0
        let new_anchor = tree.stack_rotate(0, true).unwrap();
        assert_eq!(new_anchor, b, "轮转后新锚 = 兄弟");
        let after = rects_of(&tree);
        assert!(
            after.contains_key(&b) && !after.contains_key(&0),
            "新锚独占空间"
        );
        assert_eq!(
            after[&b], anchor_rect,
            "A.7:轮转前后锚的 rect 必须相同 —— 零映射的前提"
        );

        // 反向轮转回到原状
        assert_eq!(tree.stack_rotate(b, false).unwrap(), 0);
        assert_eq!(rects_of(&tree)[&0], anchor_rect);
    }

    /// A.7 的硬约束:非兄弟(另一子是 Split)不能合并;已在组里不重复建组
    #[test]
    fn stack_rejects_non_siblings_and_is_idempotent() {
        let mut tree = base();
        let b = tree
            .split_side(
                0,
                SplitDir::H,
                false,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap();
        // 再把 b 劈开 → 0 的兄弟变成一个 Split,不再是叶子
        let c = tree
            .split_side(
                b,
                SplitDir::V,
                false,
                Box::new(PluginTerminal::new(2, 2, 80)),
            )
            .unwrap();
        assert_eq!(tree.leaf_sibling_of(0), None, "非叶兄弟 → 不认");
        assert_eq!(tree.stack_new_with_sibling(0), None, "非兄弟 → 拒绝建组");
        assert!(!tree.is_stacked(0));

        // b 与 c 是叶子兄弟 → 可以建组
        assert_eq!(tree.stack_new_with_sibling(b), Some(b));
        assert_eq!(tree.stack_members(c), vec![b, c]);
        assert_eq!(
            tree.stack_new_with_sibling(b),
            None,
            "已在组里 → 幂等拒绝,不重复入组"
        );

        // 摘掉非锚成员 → 组解散(该成员恢复为普通 pane)
        tree.stack_remove_member(c);
        assert!(!tree.is_stacked(b) && !tree.is_stacked(c), "剩 1 个 → 解散");
    }

    /// 摘成员:兄弟约束下组恒为 2 人,所以摘掉任意一个都必然解散
    /// (该成员各自恢复为普通 pane —— 它本来就有自己的树位置,不会成孤儿)
    #[test]
    fn stack_removing_member_dissolves_two_member_group() {
        let mut tree = base();
        let b = tree
            .split_side(
                0,
                SplitDir::H,
                false,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap();
        tree.stack_new_with_sibling(0).unwrap();
        tree.stack_remove_member(0); // 摘掉锚
        assert!(!tree.is_stacked(0) && !tree.is_stacked(b), "剩 1 个 → 解散");
        // 解散后两个叶子都重新占布局空间
        let mut v = Vec::new();
        layout_node(
            &tree.root,
            Rect::new(0, 0, 100, 30),
            &mut v,
            &mut Vec::new(),
            &tree.layout_skip_ids(),
        );
        assert_eq!(v.len(), 2, "解散后两叶各占自己的位置");
    }

    /// 阶段②:多个浮动 pane 并存,z 大者在上(渲染与事件路由共用此序)
    #[test]
    fn multiple_floats_stack_by_z() {
        let mut tree = base();
        let a = tree
            .split_side(
                0,
                SplitDir::H,
                false,
                Box::new(PluginTerminal::new(1, 1, 80)),
            )
            .unwrap();
        tree.split_side(
            a,
            SplitDir::H,
            false,
            Box::new(PluginTerminal::new(2, 2, 80)),
        )
        .unwrap();

        tree.add_float(FloatSlot {
            z: 1,
            ..FloatSlot::centered(1)
        });
        tree.add_float(FloatSlot {
            z: 5,
            ..FloatSlot::centered(2)
        });
        assert_eq!(tree.floats().len(), 2, "两个浮窗并存");
        assert_eq!(tree.floating(), Some(2), "z 最大的在上");

        // 抬升 → 变最上
        tree.float_raise(1);
        assert_eq!(tree.floating(), Some(1), "raise 后成为最上");

        // 关闭单个 → 只少那一个,组件保留在树里
        tree.remove_float(1);
        assert_eq!(tree.floats().len(), 1);
        assert_eq!(tree.floating(), Some(2));
        assert_eq!(
            tree.leaf_ids().len(),
            3,
            "浮窗叶子仍在树里(不占布局空间而已)"
        );
    }

    /// 浮窗几何用比例:任一视口尺寸下都不越界(改窗口不会把浮窗甩出去)
    #[test]
    fn float_rect_scales_and_stays_inside_viewport() {
        let slot = FloatSlot {
            x: 0.9,
            y: 0.9,
            w: 0.6,
            h: 0.7,
            ..FloatSlot::centered(7)
        };
        for area in [
            Rect::new(0, 0, 200, 60),
            Rect::new(3, 2, 80, 24),
            Rect::new(0, 0, 41, 11),
        ] {
            let r = slot.rect_of(area);
            assert!(r.x >= area.x && r.y >= area.y, "{area:?} 左上不越界");
            assert!(r.x + r.width <= area.x + area.width, "{area:?} 右不越界");
            assert!(r.y + r.height <= area.y + area.height, "{area:?} 下不越界");
        }
    }

    #[test]
    fn rail_immune_to_window_ops() {
        let mut tree = base();
        tree.register_rail(Box::new(PluginTerminal::new(9, 0, 80)), true, 0.2);
        let rid = tree.rail_leaf().unwrap();
        let mid = tree
            .split_side(
                0,
                SplitDir::H,
                true,
                Box::new(PluginTerminal::new(2, 0, 80)),
            )
            .unwrap();
        // 分裂在 main 内:rail 仍在 root first,root 不增层
        match &tree.root {
            LayoutNode::Split { first, second, .. } => {
                assert!(tree.leaf_ids_of(first).contains(&rid));
                assert!(tree.leaf_ids_of(second).contains(&mid), "main 含新分裂叶");
            }
            _ => panic!(),
        }
        // 对 rail 的 remove/zoom/swap/minimize 全部免疫
        assert!(!tree.remove(rid), "rail 不可 remove");
        let before = tree.zoomed;
        tree.zoom(rid);
        assert_eq!(tree.zoomed, before, "rail 不可 zoom");
        assert!(!tree.swap(rid, mid), "rail 不可 swap");
        tree.set_minimized(rid, true);
        assert_ne!(tree.minimized_leaf(), Some(rid), "rail 不可最小化");
        // 活动在 rail 上时,分裂落 main(operational_target)
        tree.focus(rid);
        let target = tree.operational_target();
        assert_ne!(target, rid, "rail 上操作目标落 main");
        let nid = tree
            .split_side(
                target,
                SplitDir::H,
                true,
                Box::new(PluginTerminal::new(3, 0, 80)),
            )
            .unwrap();
        assert!(tree.is_rail(rid) && !tree.is_rail(nid));
        // rail 仍直接贴 root
        match &tree.root {
            LayoutNode::Split { first, .. } => {
                assert!(tree.leaf_ids_of(first).contains(&rid));
            }
            _ => panic!(),
        }
    }
}

// LayoutTree::leaf_ids_of 测试辅助(递归收集)
impl LayoutTree {
    fn leaf_ids_of(&self, node: &LayoutNode) -> Vec<u64> {
        fn go(n: &LayoutNode, out: &mut Vec<u64>) {
            match n {
                LayoutNode::Leaf { id } => out.push(*id),
                LayoutNode::Split { first, second, .. } => {
                    go(first, out);
                    go(second, out);
                }
            }
        }
        let mut v = Vec::new();
        go(node, &mut v);
        v
    }
}
