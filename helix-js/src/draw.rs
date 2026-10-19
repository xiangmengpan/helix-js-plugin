//! 插件绘制原语(B 档:显示列表 / immediate mode)。
//!
//! ## 为什么有这层
//! 插件此前的"绘制"只能是**字符拼接**(`"─".repeat(w)` 之类),因为渲染管线是
//! **行模型**(`StyledLine` = 若干 `(text, style)` 段),没有绝对坐标概念。
//! 这里补中间那一层:JS 侧按坐标发几条 op,由 [`rasterize`] 光栅化成行 ——
//! 于是既有管线(`comp_layout::render` → `DiffRenderer` 的脏格 diff、宽字符写格)
//! 全部复用,**零新渲染路径**。
//!
//! ## 坐标与语义(定死,避免以后各处猜)
//! - `(0,0)` = 组件(弹窗/面板)可视区左上角;越界**自动裁剪**,不报错。
//! - 样式是**主题 scope 名**(与 `StyledLine`/`CompNode` 同一语义),这里不解析颜色;
//!   要自定义颜色先用 `helix.set_theme({scope: …})` 注册自己的 scope。
//! - 宽字符(CJK/全角)占 2 列:占位记 `Shadow`,后续写格若落在 Shadow 上,会把左邻的
//!   宽字符**清成空格** —— 否则会出现"半个汉字 + 一条边"的错位(实测过的形态)。
//!   宽度表与渲染器**同源**:`unicode-width` 版本必须与 `helix-tui`/`helix-core` 一致。
//! - 画布**不参与交互**(Tab 焦点/点击仍归 `button`/`input` 节点):这里是纯像素。

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::types::{StyledLine, TextSpan};

/// 边框字符集
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderKind {
    /// 无边框(矩形只用于填充)
    #[default]
    None,
    /// ┌ ┐ └ ┘ ─ │
    Plain,
    /// ╭ ╮ ╰ ╯ ─ │
    Round,
    /// ╔ ╗ ╚ ╝ ═ ║
    Double,
    /// ┏ ┓ ┗ ┛ ━ ┃
    Thick,
}

impl BorderKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "plain" => Some(Self::Plain),
            "round" => Some(Self::Round),
            "double" => Some(Self::Double),
            "thick" => Some(Self::Thick),
            _ => None,
        }
    }

    /// (左上, 右上, 左下, 右下, 横, 竖)
    fn chars(self) -> (char, char, char, char, char, char) {
        match self {
            Self::None => (' ', ' ', ' ', ' ', ' ', ' '),
            Self::Plain => ('┌', '┐', '└', '┘', '─', '│'),
            Self::Round => ('╭', '╮', '╰', '╯', '─', '│'),
            Self::Double => ('╔', '╗', '╚', '╝', '═', '║'),
            Self::Thick => ('┏', '┓', '┗', '┛', '━', '┃'),
        }
    }
}

/// 一条绘制指令。坐标/尺寸用 `u16`(终端量级),越界由光栅化裁剪。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawOp {
    /// 矩形:可带边框、可填充内部、可带标题(标题画在上边框上)
    Rect {
        x: u16,
        y: u16,
        w: u16,
        h: u16,
        style: Option<String>,
        border: BorderKind,
        /// 内部填充样式(写空格子);`None` = 不填
        fill: Option<String>,
        /// 标题(画在 `y` 行边框上,`x+2` 起;只取第一行)
        title: Option<Vec<TextSpan>>,
    },
    /// 水平线(单字符重复)
    HLine {
        x: u16,
        y: u16,
        len: u16,
        ch: char,
        style: Option<String>,
    },
    /// 垂直线(单字符重复)
    VLine {
        x: u16,
        y: u16,
        len: u16,
        ch: char,
        style: Option<String>,
    },
    /// Braille 半像素点阵:点在**绝对点坐标**里(1 列 = 2 点、1 行 = 4 点)。
    /// 同一格内的点 OR 成 U+2800..U+28FF 的盲文字符(2×4 点);后画的 op 覆盖先画的同格。
    Braille {
        pts: Vec<(u16, u16)>,
        style: Option<String>,
    },
    /// 显式裁剪区:内部 op 的写入被限制在矩形内(坐标仍是绝对的,不是相对)
    Clip {
        x: u16,
        y: u16,
        w: u16,
        h: u16,
        ops: Vec<DrawOp>,
    },
    /// 文字(多段样式);宽字符自动占 2 列
    Text {
        x: u16,
        y: u16,
        spans: Vec<TextSpan>,
        /// 最大显示宽度(列);`None` = 到右边界为止
        max_width: Option<u16>,
    },
}

/// 网格单元:宽字符的第二列是 `Shadow`(转行时**跳过**,否则整行右移一列)
#[derive(Debug, Clone, PartialEq, Eq)]
enum Cell {
    Blank,
    Ch(char, Option<String>),
    Shadow,
}

/// 显示列表 → 行(viewport 大小)。
///
/// 每行的**显示宽度**不超过 `width`;尾部无样式的空白被裁掉(调用方本来就把区域
/// 清成空白,省下的格子不必再画)。
pub fn rasterize(ops: &[DrawOp], width: u16, height: u16) -> Vec<StyledLine> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let mut grid = vec![vec![Cell::Blank; w]; h];
    let clip = Clip::full(w, h);
    for op in ops {
        apply(op, &mut grid, clip);
    }
    grid.iter().map(|row| row_to_line(row)).collect()
}

/// 显示列表 + 一段已布局好的行(贴进 `at` 位置,按显示宽度裁剪) → 行。
///
/// 用途:`helix.el("box")` —— 边框是绘制原语,里面的内容是**布局结果**(可能含可聚焦节点),
/// 两者在同一格网格上合成。`at` 通常是边框内区的左上角。
pub fn rasterize_then_paste(
    ops: &[DrawOp],
    lines: &[StyledLine],
    at: (u16, u16),
    width: u16,
    height: u16,
) -> Vec<StyledLine> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let mut grid = vec![vec![Cell::Blank; w]; h];
    let clip = Clip::full(w, h);
    for op in ops {
        apply(op, &mut grid, clip);
    }
    let (px, py) = (at.0 as usize, at.1 as usize);
    for (i, line) in lines.iter().enumerate() {
        let row = py + i;
        if row >= h {
            break;
        }
        let mut cx = px;
        for span in &line.spans {
            for g in span.text.graphemes(true) {
                let gw = g.width();
                if gw == 0 {
                    continue;
                }
                if put_grapheme(&mut grid, clip, cx, row, g, &span.style) == 0 {
                    break;
                }
                cx += gw;
            }
        }
    }
    grid.iter().map(|row| row_to_line(row)).collect()
}

/// 显式裁剪区(半开区间 `[x0,x1) × [y0,y1)`,单位 = 字符格)。
///
/// 坐标永远是**绝对**的:裁剪只限制"能不能写",不改变 op 的坐标系 ——
/// 否则嵌套 clip、`rasterize_then_paste` 与 box 内区三者的换算会互相打架。
#[derive(Debug, Clone, Copy)]
struct Clip {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Clip {
    fn full(w: usize, h: usize) -> Self {
        Self {
            x0: 0,
            y0: 0,
            x1: w,
            y1: h,
        }
    }

    fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }

    /// 与一个矩形求交(嵌套 clip 就是连续求交)
    fn intersect(self, x: u16, y: u16, w: u16, h: u16) -> Self {
        Self {
            x0: self.x0.max(x as usize),
            y0: self.y0.max(y as usize),
            x1: self.x1.min((x as usize).saturating_add(w as usize)),
            y1: self.y1.min((y as usize).saturating_add(h as usize)),
        }
    }
}

/// 写一个**窄**字符(唯一的裁剪点 —— 所有写入都必须过这里,包括 unshadow 的清除)
fn put(grid: &mut [Vec<Cell>], clip: Clip, x: usize, y: usize, ch: char, style: &Option<String>) {
    if !clip.contains(x, y) {
        return;
    }
    unshadow(grid, clip, y, x, style);
    grid[y][x] = Cell::Ch(ch, style.clone());
}

/// 若 `x` 是宽字符的 Shadow,把左邻的宽字符清成空格(保持新字符的样式)。
/// 左邻不在裁剪区内时**不许**动它 —— 那是别人画的东西。
fn unshadow(grid: &mut [Vec<Cell>], clip: Clip, y: usize, x: usize, style: &Option<String>) {
    if grid[y][x] == Cell::Shadow && x > 0 && clip.contains(x - 1, y) {
        grid[y][x - 1] = Cell::Ch(' ', style.clone());
    }
}

/// 写一个 grapheme(可能是宽字符),返回占用的列数(`0` = 放不下,调用方应停下)。
/// 宽字符要求**两列都在裁剪区内**,否则整字丢弃(不能画半个汉字)。
fn put_grapheme(
    grid: &mut [Vec<Cell>],
    clip: Clip,
    x: usize,
    y: usize,
    g: &str,
    style: &Option<String>,
) -> usize {
    let gw = g.width();
    if gw == 0 || !clip.contains(x, y) || (gw == 2 && !clip.contains(x + 1, y)) {
        return 0;
    }
    unshadow(grid, clip, y, x, style);
    // 多 char 的 grapheme(组合字符)取首字符(格子模型的既有边界,与 comp_layout 同款)
    let Some(ch) = g.chars().next() else {
        return 0;
    };
    grid[y][x] = Cell::Ch(ch, style.clone());
    if gw == 2 {
        grid[y][x + 1] = Cell::Shadow;
    }
    gw
}

/// Braille 点阵上限(防御:病态输入不该把主线程按在 500k 个点上)
const MAX_BRAILLE_PTS: usize = 65_536;

fn apply(op: &DrawOp, grid: &mut [Vec<Cell>], clip: Clip) {
    match op {
        DrawOp::Clip { x, y, w, h, ops } => {
            let inner = clip.intersect(*x, *y, *w, *h);
            for op in ops {
                apply(op, grid, inner);
            }
        }
        DrawOp::Braille { pts, style } => {
            // 同格的点先按位 OR(2×4 点 = 一个盲文字符),再一次性写格
            let mut cells: std::collections::BTreeMap<(usize, usize), u8> =
                std::collections::BTreeMap::new();
            for (dx, dy) in pts.iter().take(MAX_BRAILLE_PTS) {
                let (cx, cy) = (*dx as usize / 2, *dy as usize / 4);
                if !clip.contains(cx, cy) {
                    continue;
                }
                let bit = match ((*dx as usize) & 1, (*dy as usize) & 3) {
                    (0, 0) => 0x01,
                    (0, 1) => 0x02,
                    (0, 2) => 0x04,
                    (0, 3) => 0x40,
                    (1, 0) => 0x08,
                    (1, 1) => 0x10,
                    (1, 2) => 0x20,
                    (1, 3) => 0x80,
                    _ => continue,
                };
                *cells.entry((cx, cy)).or_insert(0u8) |= bit;
            }
            for ((cx, cy), mask) in cells {
                let ch = char::from_u32(0x2800 + mask as u32).unwrap_or(' ');
                put(grid, clip, cx, cy, ch, style);
            }
        }
        DrawOp::Rect {
            x,
            y,
            w: rw,
            h: rh,
            style,
            border,
            fill,
            title,
        } => {
            let (x, y, rw, rh) = (*x as usize, *y as usize, *rw as usize, *rh as usize);
            if rw == 0 || rh == 0 {
                return;
            }
            // 填充在先,边框最后画(边框不被填充覆盖)
            if let Some(fill_style) = fill {
                let fill_style = Some(fill_style.clone());
                let (fx, fy, fw, fh) = if *border == BorderKind::None {
                    (x, y, rw, rh)
                } else {
                    (x + 1, y + 1, rw.saturating_sub(2), rh.saturating_sub(2))
                };
                let (cx0, cx1) = (fx.max(clip.x0), (fx + fw).min(clip.x1));
                let (cy0, cy1) = (fy.max(clip.y0), (fy + fh).min(clip.y1));
                for row in cy0..cy1 {
                    for col in cx0..cx1 {
                        put(grid, clip, col, row, ' ', &fill_style);
                    }
                }
            }
            if *border == BorderKind::None {
                return;
            }
            let (tl, tr, bl, br, hor, ver) = border.chars();
            let right = x + rw - 1;
            let bottom = y + rh - 1;
            // 退化成一维:只画那一维(否则四角互相覆盖成乱码)
            if rh == 1 {
                for col in x.max(clip.x0)..(x + rw).min(clip.x1) {
                    put(grid, clip, col, y, hor, style);
                }
                return;
            }
            if rw == 1 {
                for row in y.max(clip.y0)..(y + rh).min(clip.y1) {
                    put(grid, clip, x, row, ver, style);
                }
                return;
            }
            // 四角 self-clip;四条边先与裁剪区求交再循环(避免 w=10000 的空转)
            put(grid, clip, x, y, tl, style);
            put(grid, clip, right, y, tr, style);
            put(grid, clip, x, bottom, bl, style);
            put(grid, clip, right, bottom, br, style);
            let (hx0, hx1) = ((x + 1).max(clip.x0), right.min(clip.x1));
            for col in hx0..hx1 {
                put(grid, clip, col, y, hor, style);
                if rh > 2 {
                    put(grid, clip, col, bottom, hor, style);
                }
            }
            let (vy0, vy1) = ((y + 1).max(clip.y0), bottom.min(clip.y1));
            for row in vy0..vy1 {
                put(grid, clip, x, row, ver, style);
                put(grid, clip, right, row, ver, style);
            }
            // 标题:`┌ title ─┐` —— 角后留一格空白,标题后也留一格(不压右边框)
            if let Some(spans) = title {
                let mut cx = x + 2;
                if cx < right {
                    put(grid, clip, x + 1, y, ' ', style);
                }
                'outer: for span in spans {
                    let own = span.style.clone().or_else(|| style.clone());
                    for g in span.text.graphemes(true) {
                        if cx + g.width() >= right {
                            break 'outer;
                        }
                        if put_grapheme(grid, clip, cx, y, g, &own) == 0 {
                            break 'outer;
                        }
                        cx += g.width();
                    }
                }
                if cx < right {
                    put(grid, clip, cx, y, ' ', style);
                }
            }
        }
        DrawOp::HLine {
            x,
            y,
            len,
            ch,
            style,
        } => {
            let (x, y, len) = (*x as usize, *y as usize, *len as usize);
            for col in x.max(clip.x0)..(x + len).min(clip.x1) {
                put(grid, clip, col, y, *ch, style);
            }
        }
        DrawOp::VLine {
            x,
            y,
            len,
            ch,
            style,
        } => {
            let (x, y, len) = (*x as usize, *y as usize, *len as usize);
            for row in y.max(clip.y0)..(y + len).min(clip.y1) {
                put(grid, clip, x, row, *ch, style);
            }
        }
        DrawOp::Text {
            x,
            y,
            spans,
            max_width,
        } => {
            let mut cx = *x as usize;
            let limit = match max_width {
                Some(m) => (cx + *m as usize).min(clip.x1),
                None => clip.x1,
            };
            for span in spans {
                for g in span.text.graphemes(true) {
                    let gw = g.width();
                    if gw == 0 {
                        continue;
                    }
                    if cx + gw > limit {
                        return; // 右边界装不下 ⇒ 这段文字到此为止
                    }
                    // 起笔落在裁剪区**左边** ⇒ 跳过它继续(裁剪只限制"能不能写",不能砍掉可见部分)
                    if !clip.contains(cx, *y as usize) {
                        cx += gw;
                        continue;
                    }
                    if put_grapheme(grid, clip, cx, *y as usize, g, &span.style) == 0 {
                        return; // 宽字符两列不全在区内 ⇒ 整字丢弃,别画半个
                    }
                    cx += gw;
                }
            }
        }
    }
}

/// 一格行 → StyledLine:相邻同 style 合并;尾部无样式空白裁掉;`Shadow` 跳过。
fn row_to_line(row: &[Cell]) -> StyledLine {
    let mut end = row.len();
    while end > 0 && row[end - 1] == Cell::Blank {
        end -= 1;
    }
    let mut spans: Vec<TextSpan> = Vec::new();
    for cell in &row[..end] {
        let (ch, style) = match cell {
            Cell::Shadow => continue,
            Cell::Blank => (' ', None),
            Cell::Ch(ch, style) => (*ch, style.clone()),
        };
        match spans.last_mut() {
            Some(last) if last.style == style => last.text.push(ch),
            _ => spans.push(TextSpan {
                text: ch.to_string(),
                style,
            }),
        }
    }
    StyledLine { spans }
}

/// 便捷构造:一段文字
pub fn text(x: u16, y: u16, s: impl Into<String>, style: Option<String>) -> DrawOp {
    DrawOp::Text {
        x,
        y,
        spans: vec![TextSpan {
            text: s.into(),
            style,
        }],
        max_width: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_str(ops: &[DrawOp], w: u16, h: u16, row: usize) -> String {
        rasterize(ops, w, h)[row]
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect()
    }

    fn rect(x: u16, y: u16, w: u16, h: u16, border: BorderKind) -> DrawOp {
        DrawOp::Rect {
            x,
            y,
            w,
            h,
            style: None,
            border,
            fill: None,
            title: None,
        }
    }

    #[test]
    fn rect_frame() {
        let ops = vec![rect(0, 0, 4, 3, BorderKind::Plain)];
        assert_eq!(line_str(&ops, 10, 3, 0), "┌──┐");
        assert_eq!(line_str(&ops, 10, 3, 1), "│  │");
        assert_eq!(line_str(&ops, 10, 3, 2), "└──┘");
    }

    #[test]
    fn rect_at_offset_and_round_double() {
        let ops = vec![rect(2, 1, 3, 3, BorderKind::Round)];
        assert_eq!(line_str(&ops, 8, 4, 0), "");
        assert_eq!(line_str(&ops, 8, 4, 1), "  ╭─╮");
        assert_eq!(line_str(&ops, 8, 4, 2), "  │ │");
        assert_eq!(line_str(&ops, 8, 4, 3), "  ╰─╯");

        let ops = vec![rect(0, 0, 2, 2, BorderKind::Double)];
        assert_eq!(line_str(&ops, 4, 2, 0), "╔╗");
        assert_eq!(line_str(&ops, 4, 2, 1), "╚╝");
    }

    /// 退化尺寸:高度 1 只画横线,宽度 1 只画竖线(不出现四角乱码)
    #[test]
    fn rect_degenerate_sizes() {
        assert_eq!(
            line_str(&[rect(0, 0, 4, 1, BorderKind::Plain)], 6, 1, 0),
            "────"
        );
        let tall = [rect(1, 0, 1, 3, BorderKind::Plain)];
        assert_eq!(line_str(&tall, 3, 3, 0), " │");
        assert_eq!(line_str(&tall, 3, 3, 2), " │");
    }

    #[test]
    fn rect_title() {
        let mut op = rect(0, 0, 8, 3, BorderKind::Plain);
        if let DrawOp::Rect { title, .. } = &mut op {
            *title = Some(vec![TextSpan {
                text: "hi".into(),
                style: None,
            }]);
        }
        let ops = vec![op];
        assert_eq!(line_str(&ops, 8, 3, 0), "┌ hi ──┐");
        assert_eq!(line_str(&ops, 8, 3, 1), "│      │");
    }

    /// 填充:内部写带样式的空格;边框在最外层不被覆盖
    #[test]
    fn rect_fill_keeps_border() {
        let ops = vec![DrawOp::Rect {
            x: 0,
            y: 0,
            w: 4,
            h: 3,
            style: Some("ui.popup".into()),
            border: BorderKind::Plain,
            fill: Some("ui.statusline".into()),
            title: None,
        }];
        let lines = rasterize(&ops, 4, 3);
        // 行 1 = "│" + 两格 fill + "│"
        assert_eq!(lines[1].spans.len(), 3, "左竖/填充/右竖 = 3 段");
        assert_eq!(lines[1].spans[0].text, "│");
        assert_eq!(lines[1].spans[0].style.as_deref(), Some("ui.popup"));
        assert_eq!(lines[1].spans[1].text, "  ");
        assert_eq!(lines[1].spans[1].style.as_deref(), Some("ui.statusline"));
        assert_eq!(lines[1].spans[2].text, "│");
    }

    /// 无边框 + fill = 一块纯色区域(不需要边框就能铺底色)
    #[test]
    fn fill_without_border() {
        let ops = vec![DrawOp::Rect {
            x: 1,
            y: 0,
            w: 2,
            h: 1,
            style: None,
            border: BorderKind::None,
            fill: Some("ui.selection".into()),
            title: None,
        }];
        let lines = rasterize(&ops, 4, 1);
        // 尾部无样式空白被裁掉(调用方已先把区域清成空白)⇒ 2 段
        assert_eq!(lines[0].spans.len(), 2);
        assert_eq!(lines[0].spans[0].text, " ");
        assert_eq!(lines[0].spans[1].text, "  ");
        assert_eq!(lines[0].spans[1].style.as_deref(), Some("ui.selection"));
    }

    #[test]
    fn hline_vline() {
        let ops = vec![
            DrawOp::HLine {
                x: 1,
                y: 0,
                len: 3,
                ch: '─',
                style: None,
            },
            DrawOp::VLine {
                x: 0,
                y: 0,
                len: 2,
                ch: '│',
                style: None,
            },
        ];
        assert_eq!(line_str(&ops, 5, 2, 0), "│───");
        assert_eq!(line_str(&ops, 5, 2, 1), "│");
    }

    /// 宽字符占 2 列:后面的元素必须落在第 4 列(而不是第 3 列)
    #[test]
    fn wide_char_takes_two_columns() {
        let ops = vec![
            text(0, 0, "中文", None),
            DrawOp::HLine {
                x: 4,
                y: 0,
                len: 1,
                ch: '─',
                style: Some("ui.text".into()),
            },
        ];
        assert_eq!(line_str(&ops, 8, 1, 0), "中文─");
        // 两段没并成一段 ⇒ 横线确实落在显示第 4 列(若宽字符只占 1 列,它会落在第 2 列并与汉字同段)
        let spans = &rasterize(&ops, 8, 1)[0].spans;
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "中文");
        assert_eq!(spans[1].text, "─");
    }

    /// 后续写格落在宽字符的第二列 ⇒ 把宽字符清成空格(否则"半个汉字 + 边")
    #[test]
    fn overwriting_wide_shadow_clears_wide_char() {
        let ops = vec![
            text(0, 0, "中", None),
            DrawOp::HLine {
                x: 1,
                y: 0,
                len: 1,
                ch: '─',
                style: None,
            },
        ];
        assert_eq!(line_str(&ops, 4, 1, 0), " ─");
    }

    /// 越界裁剪:不 panic,右/下被裁掉
    #[test]
    fn clips_out_of_bounds() {
        let ops = vec![rect(2, 1, 10, 10, BorderKind::Plain)];
        let lines = rasterize(&ops, 4, 3);
        assert_eq!(lines.len(), 3);
        assert_eq!(line_str(&ops, 4, 3, 1), "  ┌─");
        assert_eq!(line_str(&ops, 4, 3, 2), "  │");
        // 宽字符在最后一列放不下 ⇒ 整字丢弃,不留半个
        let ops = vec![text(3, 0, "中", None)];
        assert_eq!(line_str(&ops, 4, 1, 0), "");
        // 零尺寸 viewport
        assert!(rasterize(&[rect(0, 0, 3, 3, BorderKind::Plain)], 0, 0).is_empty());
    }

    #[test]
    fn text_max_width_and_styles() {
        let op = DrawOp::Text {
            x: 0,
            y: 0,
            spans: vec![
                TextSpan {
                    text: "ab".into(),
                    style: Some("error".into()),
                },
                TextSpan {
                    text: "cd".into(),
                    style: Some("ui.text".into()),
                },
            ],
            max_width: Some(3),
        };
        let lines = rasterize(&[op], 10, 1);
        assert_eq!(lines[0].spans.len(), 2);
        assert_eq!(lines[0].spans[0].text, "ab");
        assert_eq!(lines[0].spans[0].style.as_deref(), Some("error"));
        assert_eq!(lines[0].spans[1].text, "c", "max_width=3 截断");
    }

    /// 后画的文字盖住先画的边框(顺序 = 图层序)
    #[test]
    fn later_ops_win() {
        let ops = vec![rect(0, 0, 4, 3, BorderKind::Plain), text(1, 1, "ab", None)];
        assert_eq!(line_str(&ops, 6, 3, 1), "│ab│");
    }

    /// `rasterize_then_paste`:框里贴布局好的行(Box 节点就靠它把边框与子内容合成)
    #[test]
    fn paste_lines_inside_frame() {
        let ops = vec![rect(0, 0, 6, 3, BorderKind::Round)];
        let inner = vec![StyledLine {
            spans: vec![TextSpan {
                text: "中文".into(),
                style: Some("ui.text".into()),
            }],
        }];
        let lines = rasterize_then_paste(&ops, &inner, (1, 1), 6, 3);
        let joined = |i: usize| {
            lines[i]
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>()
        };
        assert_eq!(joined(0), "╭────╮");
        assert_eq!(joined(1), "│中文│");
        assert_eq!(joined(2), "╰────╯");
        // 贴进来的内容保留自己的样式
        assert!(lines[1]
            .spans
            .iter()
            .any(|s| s.style.as_deref() == Some("ui.text") && s.text == "中文"));
    }

    /// 代价基准(**默认 ignore**:它只报数,不做断言,免得在慢机器上假红)。
    ///
    /// 跑法:`cargo test -p helix-js --release --lib draw_perf -- --ignored --nocapture`
    /// 参照:`docs/plugin-vs-core.md` 记的"JS 自绘 picker ≈ 3.2ms/帧"(行模型);
    /// 这里是同一量级的替代路径 —— op 光栅化本身应与"拼字符串"同阶,而不是更贵。
    #[test]
    #[ignore]
    fn draw_perf() {
        let mut ops = Vec::new();
        for i in 0..8u16 {
            ops.push(rect(i * 9, 0, 8, 6, BorderKind::Round));
            ops.push(text(i * 9 + 2, 2, "中文ab", Some("ui.text".into())));
            ops.push(DrawOp::HLine {
                x: i * 9,
                y: 7,
                len: 8,
                ch: '─',
                style: None,
            });
        }
        // 阶段二的两种 op 也要计入:一条 800 点的 braille 曲线 + 一个裁剪区
        let curve: Vec<(u16, u16)> = (0..800u16)
            .map(|dx| {
                let dy = 26 + (dx % 40) / 5; // 粗糙锯齿,只为压出真实工作量
                (dx, dy)
            })
            .collect();
        ops.push(DrawOp::Clip {
            x: 0,
            y: 10,
            w: 80,
            h: 20,
            ops: vec![
                DrawOp::Braille {
                    pts: curve,
                    style: Some("ui.text".into()),
                },
                text(2, 12, "clipped 中文", None),
            ],
        });
        let n = 500;
        let started = std::time::Instant::now();
        for _ in 0..n {
            let lines = rasterize(&ops, 80, 30);
            std::hint::black_box(lines);
        }
        let per = started.elapsed() / n;
        println!(
            "draw_perf: {} ops → 80x30, {:?}/frame ({:?} total)",
            ops.len(),
            per,
            started.elapsed()
        );
    }

    // ─────────────── 阶段二:braille 半像素 + 显式 clip ───────────────

    fn braille(pts: &[(u16, u16)]) -> DrawOp {
        DrawOp::Braille {
            pts: pts.to_vec(),
            style: None,
        }
    }

    #[test]
    fn braille_single_dots_and_full_cell() {
        // (0,0) = bit0 ⇒ U+2801;(1,3) = bit7 ⇒ U+2880;八点全亮 = U+28FF(⣿)
        assert_eq!(line_str(&[braille(&[(0, 0)])], 4, 2, 0), "⠁");
        assert_eq!(line_str(&[braille(&[(1, 3)])], 4, 1, 0), "⢀");
        let all = braille(&[
            (0, 0),
            (1, 0),
            (0, 1),
            (1, 1),
            (0, 2),
            (1, 2),
            (0, 3),
            (1, 3),
        ]);
        assert_eq!(line_str(&[all], 4, 1, 0), "⣿");
    }

    #[test]
    fn braille_merges_within_op_and_maps_to_cells() {
        // 同格两点 ⇒ 同一个字符;点 (2,4) 属第 2 列第 2 行的格(1 列=2 点、1 行=4 点)
        let ops = [braille(&[(0, 0), (1, 0)]), braille(&[(2, 4)])];
        let lines = rasterize(&ops, 4, 2);
        let row0: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        let row1: String = lines[1].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(row0, "⠉", "0x01|0x08 = U+2809");
        assert_eq!(row1, " ⠁", "第二格第二行 ⇒ (0,0) 位");
    }

    #[test]
    fn braille_style_and_later_op_wins() {
        let first = DrawOp::Braille {
            pts: vec![(0, 0)],
            style: Some("error".into()),
        };
        let lines = rasterize(&[first], 2, 1);
        assert_eq!(lines[0].spans[0].style.as_deref(), Some("error"));
        // 后画的 braille 覆盖同格(格子只能放一个字符 + 一份样式)
        let second = DrawOp::Braille {
            pts: vec![(1, 3)],
            style: Some("ui.text".into()),
        };
        let lines = rasterize(&[braille(&[(0, 0)]), second], 2, 1);
        assert_eq!(lines[0].spans[0].text, "⢀");
        assert_eq!(lines[0].spans[0].style.as_deref(), Some("ui.text"));
    }

    #[test]
    fn braille_is_clipped_like_everything_else() {
        let ops = [DrawOp::Clip {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
            ops: vec![braille(&[(0, 0), (4, 0)])], // 第二点落在第 3 列,被裁掉
        }];
        assert_eq!(line_str(&ops, 6, 1, 0), "⠁");
    }

    #[test]
    fn clip_limits_writes_but_keeps_absolute_coords() {
        let ops = [DrawOp::Clip {
            x: 1,
            y: 1,
            w: 2,
            h: 1,
            ops: vec![
                rect(0, 0, 6, 3, BorderKind::Plain),
                text(0, 1, "abcdef", None),
            ],
        }];
        assert_eq!(line_str(&ops, 8, 3, 0), "", "裁剪区之外(上边框)不画");
        assert_eq!(
            line_str(&ops, 8, 3, 1),
            " bc",
            "只留裁剪区内的两列,坐标不移位"
        );
        assert_eq!(line_str(&ops, 8, 3, 2), "");
    }

    #[test]
    fn nested_clips_intersect() {
        let inner = DrawOp::Clip {
            x: 2,
            y: 0,
            w: 4,
            h: 1,
            ops: vec![text(0, 0, "01234567", None)],
        };
        let outer = DrawOp::Clip {
            x: 3,
            y: 0,
            w: 2,
            h: 1,
            ops: vec![inner],
        };
        assert_eq!(line_str(&[outer], 10, 1, 0), "   34");
    }

    /// 宽字符跨裁剪边界 ⇒ 整字丢弃(不能只画半个)
    #[test]
    fn clip_does_not_split_wide_chars() {
        let ops = [DrawOp::Clip {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
            ops: vec![text(0, 0, "中", None)],
        }];
        assert_eq!(line_str(&ops, 4, 1, 0), "", "1 列宽装不下 2 列的汉字");
        let ops = [DrawOp::Clip {
            x: 1,
            y: 0,
            w: 3,
            h: 1,
            ops: vec![text(0, 0, "中", None)],
        }];
        assert_eq!(line_str(&ops, 4, 1, 0), "", "起笔在裁剪区外 ⇒ 也不画");
    }

    #[test]
    fn border_kind_parse() {
        assert_eq!(BorderKind::parse("plain"), Some(BorderKind::Plain));
        assert_eq!(BorderKind::parse("round"), Some(BorderKind::Round));
        assert_eq!(BorderKind::parse("double"), Some(BorderKind::Double));
        assert_eq!(BorderKind::parse("thick"), Some(BorderKind::Thick));
        assert_eq!(BorderKind::parse("none"), Some(BorderKind::None));
        assert_eq!(BorderKind::parse("bogus"), None);
    }
}

// ═══════════════════════ JS 侧 API(helix.draw.* / helix.text.*)═══════════════════════
//
// `helix.draw.*` 是**立即模式**:一次 render 里按顺序调几条,最后
// `return helix.draw.done()` 把这一批交给宿主。状态只有一份(线程局部),清空点只有两处、
// 各有职责:
//   · 进 render 前 `reset()` —— 上一次忘了 done() / 画完又返回别的内容,脏 op 不跨帧;
//   · `done()` 置 `armed` —— 没调 done 的 op **永远进不了 Content**,于是既有契约里
//     "忘了 return" 这类真 bug 仍会报错,而不是静默渲染出半张图。

use boa_engine::object::builtins::JsArray;
use boa_engine::{Context, JsObject, JsString, JsValue};

thread_local! {
    static DRAW: std::cell::RefCell<DrawState> = std::cell::RefCell::new(DrawState::default());
}

#[derive(Default)]
struct DrawState {
    ops: Vec<DrawOp>,
    armed: bool,
    /// 未闭合的裁剪区:`(x, y, w, h, ops 起点)`。
    /// `pop_clip()` 把起点之后新产生的 op 收进一个 `DrawOp::Clip`;`done()` 会兜底闭合。
    clips: Vec<(u16, u16, u16, u16, usize)>,
}

/// 进入一次 render 前重置(渲染入口调用,见 `popup::render_popup`)
pub fn reset() {
    DRAW.with(|d| {
        let mut d = d.borrow_mut();
        d.ops.clear();
        d.armed = false;
        d.clips.clear();
    });
}

/// 取出本帧的显示列表;`None` = 插件没调 `helix.draw.done()`(调用方报错)
pub(crate) fn take_armed() -> Option<Vec<DrawOp>> {
    DRAW.with(|d| {
        let mut d = d.borrow_mut();
        if !d.armed {
            return None;
        }
        d.armed = false;
        Some(std::mem::take(&mut d.ops))
    })
}

fn push_op(op: DrawOp) {
    DRAW.with(|d| d.borrow_mut().ops.push(op));
}

/// 闭合最内层裁剪区:把 `start..` 期间产生的 op 收进一个 `DrawOp::Clip`。
/// 返回是否真的闭了一个(栈空 = no-op,便于 `done()` 兜底)。
fn close_clip() -> bool {
    DRAW.with(|d| {
        let mut d = d.borrow_mut();
        let Some((x, y, w, h, start)) = d.clips.pop() else {
            return false;
        };
        let inner: Vec<DrawOp> = d.ops.drain(start..).collect();
        d.ops.push(DrawOp::Clip {
            x,
            y,
            w,
            h,
            ops: inner,
        });
        true
    })
}

// ── opts 读取(形状不对一律当"没给",绘制 API 不该因为 ops 里多一个字段就炸整帧)──

fn prop(obj: &JsObject, key: &str, ctx: &mut Context) -> Option<JsValue> {
    obj.get(JsString::from(key), ctx).ok()
}

fn as_string(v: &JsValue) -> Option<String> {
    v.as_string()
        .map(|s| s.to_std_string_escaped())
        .filter(|s| !s.is_empty())
}

fn opt_obj(args: &[JsValue], i: usize) -> Option<JsObject> {
    args.get(i).and_then(|v| v.as_object())
}

fn opt_str_prop(obj: &Option<JsObject>, key: &str, ctx: &mut Context) -> Option<String> {
    obj.as_ref()
        .and_then(|o| prop(o, key, ctx))
        .and_then(|v| as_string(&v))
}

/// 数字参数 → u16(负数/NaN 视作 0;越界本来就靠裁剪,不必在这里报错)
fn num(args: &[JsValue], i: usize) -> u16 {
    args.get(i)
        .and_then(|v| v.as_number())
        .map(|n| n.clamp(0.0, u16::MAX as f64) as u16)
        .unwrap_or(0)
}

fn num_prop(obj: &Option<JsObject>, key: &str, ctx: &mut Context) -> Option<u16> {
    obj.as_ref()
        .and_then(|o| prop(o, key, ctx))
        .and_then(|v| v.as_number())
        .map(|n| n.clamp(0.0, u16::MAX as f64) as u16)
}

/// 字符串 / `[{text, style}, …]` → 多段文本(`style` 是缺省样式)
fn spans_from_value(v: &JsValue, style: Option<String>, ctx: &mut Context) -> Vec<TextSpan> {
    if let Some(s) = as_string(v) {
        return vec![TextSpan { text: s, style }];
    }
    let Some(arr) = v
        .as_object()
        .and_then(|o| JsArray::from_object(o.clone()).ok())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut i = 0u32;
    while i < 4096 {
        // 防御:病态输入(无限数组)别把主线程挂住
        let Ok(item) = arr.get(i, ctx) else { break };
        if item.is_null_or_undefined() {
            break;
        }
        let mut text = as_string(&item).unwrap_or_default();
        let mut st = style.clone();
        if text.is_empty() {
            if let Some(o) = item.as_object() {
                if let Some(t) = prop(&o, "text", ctx) {
                    text = as_string(&t).unwrap_or_default();
                }
                if let Some(s) = prop(&o, "style", ctx) {
                    if let Some(s) = as_string(&s) {
                        st = Some(s);
                    }
                }
            }
        }
        if !text.is_empty() {
            out.push(TextSpan { text, style: st });
        }
        i += 1;
    }
    out
}

/// `helix.draw.rect(x, y, w, h, opts?)`
///
/// opts: `style`(边框样式 scope)、`border`(`plain|round|double|thick|none`,缺省 plain)、
/// `fill`(`true` = 用 `style` 填内部;字符串 = 用该 scope 填)、`title`(串或 `[{text,style}]`)。
pub(crate) fn js_draw_rect(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let opts = opt_obj(args, 4);
    let style = opt_str_prop(&opts, "style", ctx);
    let border = match opt_str_prop(&opts, "border", ctx) {
        Some(s) => BorderKind::parse(&s).ok_or_else(|| {
            crate::js_err("draw.rect: border must be plain|round|double|thick|none")
        })?,
        None => BorderKind::Plain,
    };
    let fill = opts
        .as_ref()
        .and_then(|o| prop(o, "fill", ctx))
        .and_then(|v| match v.as_boolean() {
            Some(true) => style.clone(),
            _ => as_string(&v),
        });
    let title = opts
        .as_ref()
        .and_then(|o| prop(o, "title", ctx))
        .map(|v| spans_from_value(&v, style.clone(), ctx))
        .filter(|v| !v.is_empty());
    push_op(DrawOp::Rect {
        x: num(args, 0),
        y: num(args, 1),
        w: num(args, 2),
        h: num(args, 3),
        style,
        border,
        fill,
        title,
    });
    Ok(JsValue::undefined())
}

/// `helix.draw.fill(x, y, w, h, opts?)` —— 无边框填充块
///
/// 缺省铺带样式的**空格**(TUI 里"上底色"的唯一手段);`opts.ch` 给单字符纹理。
pub(crate) fn js_draw_fill(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let opts = opt_obj(args, 4);
    let style = opt_str_prop(&opts, "style", ctx);
    let (x, y, w, h) = (num(args, 0), num(args, 1), num(args, 2), num(args, 3));
    match opt_str_prop(&opts, "ch", ctx).and_then(|s| s.chars().next()) {
        Some(ch) => {
            for row in 0..h {
                push_op(DrawOp::HLine {
                    x,
                    y: y.saturating_add(row),
                    len: w,
                    ch,
                    style: style.clone(),
                });
            }
        }
        None => push_op(DrawOp::Rect {
            x,
            y,
            w,
            h,
            style: None,
            border: BorderKind::None,
            fill: Some(style.unwrap_or_default()),
            title: None,
        }),
    }
    Ok(JsValue::undefined())
}

/// `helix.draw.hline(x, y, len, opts?)` —— `opts.ch` 换字符(缺省 `─`),`opts.style` 给样式
pub(crate) fn js_draw_hline(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let opts = opt_obj(args, 3);
    push_op(DrawOp::HLine {
        x: num(args, 0),
        y: num(args, 1),
        len: num(args, 2),
        ch: opt_str_prop(&opts, "ch", ctx)
            .and_then(|s| s.chars().next())
            .unwrap_or('─'),
        style: opt_str_prop(&opts, "style", ctx),
    });
    Ok(JsValue::undefined())
}

/// `helix.draw.vline(x, y, len, opts?)` —— `opts.ch` 换字符(缺省 `│`),`opts.style` 给样式
pub(crate) fn js_draw_vline(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let opts = opt_obj(args, 3);
    push_op(DrawOp::VLine {
        x: num(args, 0),
        y: num(args, 1),
        len: num(args, 2),
        ch: opt_str_prop(&opts, "ch", ctx)
            .and_then(|s| s.chars().next())
            .unwrap_or('│'),
        style: opt_str_prop(&opts, "style", ctx),
    });
    Ok(JsValue::undefined())
}

/// `helix.draw.text(x, y, textOrParts, opts?)` —— `opts.style` 缺省样式,`opts.maxWidth` 限宽(列)
pub(crate) fn js_draw_text(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let opts = opt_obj(args, 3);
    let style = opt_str_prop(&opts, "style", ctx);
    let spans = match args.get(2) {
        Some(v) => spans_from_value(v, style, ctx),
        None => Vec::new(),
    };
    push_op(DrawOp::Text {
        x: num(args, 0),
        y: num(args, 1),
        spans,
        max_width: num_prop(&opts, "maxWidth", ctx),
    });
    Ok(JsValue::undefined())
}

/// `helix.draw.braille(pts, opts?)` —— 半像素点阵(`pts` = `[[dx,dy], …]` 或 `[{x,y}, …]`)
///
/// 点坐标是**绝对点坐标**:1 列 = 2 点、1 行 = 4 点 ⇒ 分辨率是普通字符格的 8 倍。
/// 同一格内的点 OR 成一个盲文字符(U+2800..U+28FF);`opts.style` 给样式。
pub(crate) fn js_draw_braille(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let opts = opt_obj(args, 1);
    let style = opt_str_prop(&opts, "style", ctx);
    let mut pts: Vec<(u16, u16)> = Vec::new();
    if let Some(obj) = args.first().and_then(|v| v.as_object()) {
        if obj.is_array() {
            // 按 `length` 迭代(不看元素是否为 null —— 跳过的坏点不该中断整批)
            let len: u32 = prop(&obj, "length", ctx)
                .and_then(|v| v.as_number())
                .unwrap_or(0.0) as u32;
            if let Ok(arr) = JsArray::from_object(obj.clone()) {
                for i in 0..len.min(MAX_BRAILLE_PTS as u32) {
                    let Ok(item) = arr.get(i, ctx) else { continue };
                    let (dx, dy) = match item.as_object() {
                        // `[x, y]`
                        Some(o) if o.is_array() => (
                            JsArray::from_object(o.clone())
                                .ok()
                                .and_then(|p| p.get(0, ctx).ok()),
                            JsArray::from_object(o)
                                .ok()
                                .and_then(|p| p.get(1, ctx).ok()),
                        ),
                        // `{x, y}`
                        Some(o) => (prop(&o, "x", ctx), prop(&o, "y", ctx)),
                        // 标量/null ⇒ 跳过这一点
                        None => (None, None),
                    };
                    if let (Some(dx), Some(dy)) = (dx, dy) {
                        if let (Some(dx), Some(dy)) = (dx.as_number(), dy.as_number()) {
                            // NaN/-1 之类 clamps 成 0(绘制 API 不因坏坐标炸整帧)
                            let c = |n: f64| n.clamp(0.0, u16::MAX as f64) as u16;
                            pts.push((c(dx), c(dy)));
                        }
                    }
                }
            }
        }
    }
    push_op(DrawOp::Braille { pts, style });
    Ok(JsValue::undefined())
}

/// `helix.draw.push_clip(x, y, w, h)` —— 开始一个裁剪区(与 `pop_clip` 配对)
pub(crate) fn js_draw_push_clip(
    _this: &JsValue,
    args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    DRAW.with(|d| {
        let mut d = d.borrow_mut();
        let start = d.ops.len();
        d.clips.push((
            num(args, 0),
            num(args, 1),
            num(args, 2),
            num(args, 3),
            start,
        ));
    });
    Ok(JsValue::undefined())
}

/// `helix.draw.pop_clip()` —— 闭合最近一个裁剪区(栈空 = 无害 no-op)
pub(crate) fn js_draw_pop_clip(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    close_clip();
    Ok(JsValue::undefined())
}

/// `helix.draw.done()` —— 把本帧的 op 交给宿主(`render` 的返回值)
pub(crate) fn js_draw_done(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    // 忘了 pop_clip 也不该让内容整个丢掉(未闭合的区按"到 done 为止"闭)
    while close_clip() {}
    DRAW.with(|d| d.borrow_mut().armed = true);
    let obj = boa_engine::object::ObjectInitializer::new(ctx)
        .property(
            JsString::from("type"),
            JsValue::from(JsString::from("draw")),
            boa_engine::property::Attribute::all(),
        )
        .build();
    Ok(JsValue::from(obj))
}

// ── `helix.text.*`(D 档零件:把各插件手里重复的 dw()/cut() 收进核心)──

fn first_string(args: &[JsValue]) -> String {
    args.first()
        .and_then(|v| v.as_string())
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default()
}

/// `helix.text.width(s)` —— 显示列宽(CJK/全角 = 2),与渲染器**同一张宽度表**
pub(crate) fn js_text_width(
    _this: &JsValue,
    args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    Ok(JsValue::from(first_string(args).width() as f64))
}

/// `helix.text.truncate(s, width)` —— 按显示宽度截断(不会切出半个宽字符)
pub(crate) fn js_text_truncate(
    _this: &JsValue,
    args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let s = first_string(args);
    let limit = num(args, 1) as usize;
    let mut out = String::new();
    let mut used = 0usize;
    for g in s.graphemes(true) {
        let gw = g.width();
        if used + gw > limit {
            break;
        }
        out.push_str(g);
        used += gw;
    }
    Ok(JsValue::from(JsString::from(out)))
}

/// `helix.text.pad(s, width, opts?)` —— `opts.align`: `left`(缺省) / `right` / `center`
pub(crate) fn js_text_pad(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let s = first_string(args);
    let width = num(args, 1) as usize;
    let align = opt_str_prop(&opt_obj(args, 2), "align", ctx).unwrap_or_else(|| "left".into());
    let used = s.width();
    if used >= width {
        return Ok(JsValue::from(JsString::from(s)));
    }
    let pad = width - used;
    let (left, right) = match align.as_str() {
        "right" => (pad, 0),
        "center" => (pad / 2, pad - pad / 2),
        _ => (0, pad),
    };
    Ok(JsValue::from(JsString::from(format!(
        "{}{}{}",
        " ".repeat(left),
        s,
        " ".repeat(right)
    ))))
}
