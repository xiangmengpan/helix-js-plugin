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
        if id == 0 || self.fixed.contains(&id) || !self.components.contains_key(&id) {
            return false; // 编辑器叶子不可移除；fixed 叶子免疫
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
        if self.fixed.contains(&id) {
            return false; // fixed 叶子不可 resize
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
        {
            return false; // fixed 叶子不可 swap
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
        if self.fixed.contains(&id) {
            return; // fixed 叶子所在 Split 不可均衡
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
        if minimized && self.fixed.contains(&id) {
            return; // fixed 叶子不可被最小化;还原不受限(否则 z 后设 fixed 的叶子无恢复路径)
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

    pub fn dump(&self) -> LayoutDump {
        let fixed = &self.fixed;
        fn dump_node(
            node: &LayoutNode,
            fixed: &std::collections::HashSet<u64>,
        ) -> serde_json::Value {
            match node {
                LayoutNode::Leaf { id } => {
                    serde_json::json!({ "type": "leaf", "id": id, "fixed": fixed.contains(id) })
                }
                LayoutNode::Split {
                    dir,
                    ratio,
                    first,
                    second,
                } => serde_json::json!({
                    "type": "split",
                    "dir": match dir { SplitDir::H => "h", SplitDir::V => "v" },
                    "ratio": ratio,
                    "first": dump_node(first, fixed),
                    "second": dump_node(second, fixed),
                }),
            }
        }
        fn collect_leafs(
            node: &LayoutNode,
            fixed: &std::collections::HashSet<u64>,
            out: &mut Vec<LeafInfo>,
        ) {
            match node {
                LayoutNode::Leaf { id } => {
                    out.push(LeafInfo {
                        id: *id,
                        fixed: fixed.contains(id),
                    });
                }
                LayoutNode::Split { first, second, .. } => {
                    collect_leafs(first, fixed, out);
                    collect_leafs(second, fixed, out);
                }
            }
        }
        let mut leafs = Vec::new();
        collect_leafs(&self.root, fixed, &mut leafs);
        LayoutDump {
            tree: dump_node(&self.root, fixed),
            active: self.active,
            zoomed: self.zoomed,
            minimized: self.minimized,
            leafs,
        }
    }

    /// 渲染：每个叶子在自己的矩形里渲染组件；缩放时只有被缩放的叶子渲染；
    /// 浮动叶子最后画（最上层，居中浮窗 + 边框），其他叶子照常布局。
    pub fn render(&mut self, area: Rect, surface: &mut tui::buffer::Buffer, cx: &mut Context) {
        if let Some(zoomed) = self.zoomed {
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
