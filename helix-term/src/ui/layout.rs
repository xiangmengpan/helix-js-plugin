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
    /// 浮动叶子 id（终端 Floating 模式：不占 split 布局，渲染在视口中央浮窗，最上层）
    float: Option<u64>,
    /// 最小化叶子 id（不占布局，渲染为底部一条标题横条；单例）
    minimized: Option<u64>,
    /// 最近一次渲染的叶子区域（鼠标命中查询；每帧渲染更新）
    leaf_rects: std::collections::HashMap<u64, Rect>,
    /// 固定叶子 id 集合（fixed：不被模式操作 swap/resize/close/minimize/equalize，可被焦点穿过）
    fixed: std::collections::HashSet<u64>,
    /// rail(侧栏轨道)叶子:恒为 root 的直接子叶(H 分屏边缘侧,占全高)。
    /// 不参与分裂/换位/最小化/放大等窗口操作;窗口操作只发生在 main 子树。
    rail: Option<u64>,
}

/// 叶子布局结果：每个叶子的 id + Rect
/// skip/minimized = 排除布局空间的叶子 id（float 浮窗 / minimized 横条）：
/// 其所在子树整体让位，其余叶子占满区域
type LeafRect = (u64, Rect);

/// 子树是否所有叶子都属于排除集合（float/minimized 让位判断）
fn subtree_all_excluded(node: &LayoutNode, skip: Option<u64>, minimized: Option<u64>) -> bool {
    match node {
        LayoutNode::Leaf { id } => Some(*id) == skip || Some(*id) == minimized,
        LayoutNode::Split { first, second, .. } => {
            subtree_all_excluded(first, skip, minimized)
                && subtree_all_excluded(second, skip, minimized)
        }
    }
}

fn layout_node(
    node: &LayoutNode,
    area: Rect,
    out: &mut Vec<LeafRect>,
    skip: Option<u64>,
    minimized: Option<u64>,
) {
    match node {
        LayoutNode::Leaf { id } => {
            if Some(*id) != skip && Some(*id) != minimized {
                out.push((*id, area));
            }
        }
        LayoutNode::Split {
            dir,
            ratio,
            first,
            second,
        } => {
            // 排除叶子所在子树整体不占空间：另一侧占满本区域（浮动/最小化时
            // 不留白——否则 dock 位置留白 → 其余叶子出现一块空白）
            if skip.is_some() || minimized.is_some() {
                let (first_gone, second_gone) = (
                    subtree_all_excluded(first, skip, minimized),
                    subtree_all_excluded(second, skip, minimized),
                );
                match (first_gone, second_gone) {
                    (true, false) => return layout_node(second, area, out, skip, minimized),
                    (false, true) => return layout_node(first, area, out, skip, minimized),
                    _ => {}
                }
            }
            let ratio = ratio.clamp(0.05, 0.95);
            match dir {
                SplitDir::H => {
                    let w = (area.width as f32 * ratio) as u16;
                    layout_node(
                        first,
                        Rect::new(area.x, area.y, w, area.height),
                        out,
                        skip,
                        minimized,
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
                        skip,
                        minimized,
                    );
                }
                SplitDir::V => {
                    let h = (area.height as f32 * ratio) as u16;
                    layout_node(
                        first,
                        Rect::new(area.x, area.y, area.width, h),
                        out,
                        skip,
                        minimized,
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
                        skip,
                        minimized,
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
            float: None,
            minimized: None,
            fixed: Default::default(),
            rail: None,
            leaf_rects: Default::default(),
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
        self.components.insert(rid, component);
        let main_root = std::mem::replace(&mut self.root, LayoutNode::Leaf { id: rid });
        self.root = if left {
            LayoutNode::Split {
                dir: SplitDir::H,
                ratio,
                first: Box::new(LayoutNode::Leaf { id: rid }),
                second: Box::new(main_root),
            }
        } else {
            LayoutNode::Split {
                dir: SplitDir::H,
                ratio: 1.0 - ratio,
                first: Box::new(main_root),
                second: Box::new(LayoutNode::Leaf { id: rid }),
            }
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

    /// 浮动叶子 id
    pub fn floating(&self) -> Option<u64> {
        self.float
    }

    /// 把叶子设为浮动（渲染在最上层浮窗；其他叶子照常布局）。仅当组件存在。
    pub fn set_float(&mut self, id: u64) {
        if self.components.contains_key(&id) {
            self.float = Some(id);
            self.active = id;
        }
    }

    /// 取消浮动（叶子回到其 split 位置）
    pub fn unfloat(&mut self) {
        self.float = None;
    }

    pub fn is_float(&self) -> bool {
        self.float.is_some()
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

    /// 活动叶子 id
    pub fn active(&self) -> u64 {
        self.zoomed.unwrap_or(self.active)
    }

    /// 全部叶子 id(树中序:左/上优先)。用于「切到下一个窗口」(Pane 模式 p/Tab)。
    pub fn leaf_ids(&self) -> Vec<u64> {
        self.leaf_ids_of(&self.root)
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
        if self.float == Some(id) {
            // 残留会让事件路由指向已删叶子（handle_event 优先 float）→ 全部 Ignored → 程序僵死
            self.float = None;
        }
        if self.minimized == Some(id) {
            self.minimized = None;
        }
        true
    }

    /// 缩放：叶子占满全区（其他叶子隐藏）；再次调用取消
    pub fn zoom(&mut self, id: u64) {
        if self.rail == Some(id) {
            return; // rail 不参与 zoom
        }
        if self.components.contains_key(&id) {
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

    /// 最小化叶子：不占布局空间，渲染为底部一条标题横条（单例：再调用切换到新叶子）。
    /// minimized=false 恢复。最小化时若该叶子正被聚焦/缩放，焦点回编辑器。
    pub fn set_minimized(&mut self, id: u64, minimized: bool) {
        if !self.components.contains_key(&id) {
            return;
        }
        if minimized && (self.fixed.contains(&id) || self.rail == Some(id)) {
            return; // fixed/rail 不可被最小化
        }
        if minimized {
            self.minimized = Some(id);
            if self.zoomed == Some(id) {
                self.zoomed = None;
            }
            if self.active == id {
                self.active = 0;
            }
        } else if self.minimized == Some(id) {
            self.minimized = None;
        }
    }

    /// 最小化叶子 id
    pub fn minimized_leaf(&self) -> Option<u64> {
        self.minimized
    }

    /// 最小化叶子 id(窗口模式/测试别名)
    pub fn minimized(&self) -> Option<u64> {
        self.minimized
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
                        (Some(f), Some(s)) => Some(LayoutNode::Split {
                            dir,
                            ratio: ratio.clamp(0.05, 0.95),
                            first: Box::new(f),
                            second: Box::new(s),
                        }),
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

        // active / zoomed / minimized 只接受仍存在的 id,否则回落到树的首叶
        let live_of = |v: Option<u64>| v.filter(|id| live.contains(id));
        let first = self.leaf_ids().into_iter().next().unwrap_or(0);
        self.active = dump
            .get("active")
            .and_then(|v| v.as_u64())
            .and_then(|v| live_of(Some(v)))
            .unwrap_or(first);
        self.zoomed = live_of(dump.get("zoomed").and_then(|v| v.as_u64()));
        self.minimized = live_of(dump.get("minimized").and_then(|v| v.as_u64()));
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
                    dir,
                    ratio,
                    first,
                    second,
                } => serde_json::json!({
                    "type": "split",
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
            minimized: self.minimized,
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
            LayoutNode::Split {
                ratio,
                first,
                second,
                ..
            } => {
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

    pub fn render(&mut self, area: Rect, surface: &mut tui::buffer::Buffer, cx: &mut Context) {
        if let Some(zoomed) = self.zoomed {
            // zoom 保留 rail:rail 占边缘窄条,zoomed 占其余 main 区
            if let Some((rid, left)) = self.rail_edge() {
                let share = self.rail_share().clamp(0.05, 0.9);
                let rail_w = ((area.width as f32) * share) as u16;
                let rail_w = rail_w.clamp(1, area.width.saturating_sub(1).max(1));
                let (rail_rect, zoom_area) = if left {
                    (
                        Rect::new(area.x, area.y, rail_w, area.height),
                        Rect::new(area.x + rail_w, area.y, area.width - rail_w, area.height),
                    )
                } else {
                    (
                        Rect::new(area.x + area.width - rail_w, area.y, rail_w, area.height),
                        Rect::new(area.x, area.y, area.width - rail_w, area.height),
                    )
                };
                let mut comps = std::mem::take(&mut self.components);
                if let Some(rc) = comps.get_mut(&rid) {
                    rc.render(rail_rect, surface, cx);
                }
                if let Some(zc) = comps.get_mut(&zoomed) {
                    zc.render(zoom_area, surface, cx);
                }
                self.components = comps;
                return;
            }
            if let Some(comp) = self.components.get_mut(&zoomed) {
                comp.render(area, surface, cx);
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
        // 浮动/最小化叶子不占布局空间（其余叶子占满，无 dock 位置留白）
        layout_node(&self.root, area, &mut rects, self.float, self.minimized);
        // 活动叶子（事件路由目标）：画高亮边框（内容 inset 1 格；浮窗已有自身边框不重复）
        let focus = if self.float.is_some() || self.zoomed.is_some() {
            None
        } else {
            Some(self.active)
        };
        self.leaf_rects.clear();
        for (id, rect) in &rects {
            self.leaf_rects.insert(*id, *rect);
        }
        for (id, rect) in rects {
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
        // 最小化叶子：底部一条标题横条（最上层）
        if let Some(mid) = self.minimized {
            if let Some(comp) = self.components.get(&mid) {
                let title = short_type_name(comp.type_name());
                let style = cx.editor.theme.get("ui.popup");
                let y = area.y + area.height.saturating_sub(1);
                let text = format!("─ {title} ─");
                let chars: Vec<char> = text.chars().collect();
                for x in 0..area.width {
                    let ch = if (x as usize) < chars.len() {
                        chars[x as usize]
                    } else {
                        '─'
                    };
                    if let Some(cell) = surface.get_mut(area.x + x, y) {
                        cell.set_symbol(&ch.to_string());
                        cell.set_style(style);
                    }
                }
            }
        }
        // 浮动叶子：最上层浮窗（边框 + 内区）
        if let Some(fid) = self.float {
            if let Some(comp) = self.components.get_mut(&fid) {
                let outer = Self::float_rect(area);
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
        let target = self.float.unwrap_or_else(|| self.active());
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
    pub minimized: Option<u64>,
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
        layout_node(&tree.root, area, &mut rects, None, None);
        assert_eq!(rects.len(), 2);
        let editor_rect = rects.iter().find(|(id, _)| *id == 0).unwrap().1;
        assert!(editor_rect.height < area.height, "未浮动时编辑器被切分");
        // 浮动后：编辑器占满整个区域（无底部空白）
        tree.set_float(tid);
        let mut rects = Vec::new();
        layout_node(&tree.root, area, &mut rects, tree.float, tree.minimized);
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
        layout_node(&tree.root, Rect::new(0, 0, 100, 40), &mut rects, None, None);
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
        layout_node(&tree.root, Rect::new(0, 0, 100, 10), &mut rects, None, None);
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
        layout_node(&tree.root, area, &mut rects, tree.float, tree.minimized);
        assert_eq!(rects.len(), 1, "minimized 叶子不产生布局 rect");
        assert_eq!(rects[0].1, area, "其余叶子占满整个区域");
        // 恢复
        tree.set_minimized(tid, false);
        assert_eq!(tree.minimized_leaf(), None);
        let mut rects = Vec::new();
        layout_node(&tree.root, area, &mut rects, tree.float, tree.minimized);
        assert_eq!(rects.len(), 2, "恢复后回到原分割");
    }
}

// ---- rail 单元测试 ----

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
                assert_eq!(tree.leaf_ids_of(&*first), vec![rid], "rail 在 first(左侧)");
                assert!(tree.leaf_ids_of(&*second).contains(&0), "main 含编辑器");
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
                    tree2.leaf_ids_of(&*second).contains(&rid2),
                    "rail 在 second(右侧)"
                );
                assert!(tree2.leaf_ids_of(&*first).contains(&0));
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
                assert!(tree.leaf_ids_of(&*first).contains(&r2));
                assert!(tree.leaf_ids_of(&*second).contains(&0));
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
                assert!(tree.leaf_ids_of(&*first).contains(&rid));
                assert!(tree.leaf_ids_of(&*second).contains(&mid), "main 含新分裂叶");
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
                assert!(tree.leaf_ids_of(&*first).contains(&rid));
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
