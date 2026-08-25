//! 组件树布局引擎：把 JS render 返回的 CompNode 树渲染成样式化行（StyledLine）。
//! 样式名不在此解析（helix-js 只透传字符串），由调用方经 theme.get 映射。

use helix_js::{CompNode, Content, StyledLine, TextSpan};

/// 统一入口：Content::Lines 原样返回（旧行 API）；Content::Tree 走布局引擎。
pub fn render(content: Content, viewport: (u16, u16)) -> Vec<StyledLine> {
    match content {
        Content::Lines(lines) => lines,
        Content::Tree(node) => layout(&node, viewport),
    }
}

/// 弹性权重:flex 字段(Scroll 无 flex → None)。
fn flex_of(node: &CompNode) -> Option<u16> {
    match node {
        CompNode::Text { flex, .. } | CompNode::Row { flex, .. } | CompNode::Col { flex, .. }
        | CompNode::Button { flex, .. } | CompNode::Input { flex, .. } => *flex,
        CompNode::Scroll { .. } => None,
    }
}

/// 相邻相同 style 的 (char, style) 流合并成 TextSpan。
fn merge_spans(stream: &[(char, Option<String>)]) -> Vec<TextSpan> {
    let mut out: Vec<TextSpan> = Vec::new();
    for (ch, style) in stream {
        if let Some(last) = out.last_mut() {
            if last.style == *style {
                last.text.push(*ch);
                continue;
            }
        }
        out.push(TextSpan { text: ch.to_string(), style: style.clone() });
    }
    out
}

/// 组件树 → 样式化行：
/// - text → 单行（width 截断，viewport 宽度优先）
/// - col → 子节点自上而下堆叠（gap 空行；viewport 高度限制）
/// - row → 子节点各自布局成行集，第 i 行 = 各子第 i 行拼接（行数不足补空行；
///   gap 列间距；总宽超 viewport 截断）
/// - scroll → 按 col（无 gap）布局后保留最后 height 行（viewport 高度内）
pub fn layout(node: &CompNode, viewport: (u16, u16)) -> Vec<StyledLine> {
    match node {
        CompNode::Text { spans, width, wrap, .. } => {
            let limit = width.unwrap_or(viewport.0).min(viewport.0) as usize;
            if *wrap && spans.iter().map(|s| s.text.chars().count()).sum::<usize>() > limit && limit > 0 {
                // (char, style) 流按 limit 切行;每行合并相邻同 style 段
                let stream: Vec<(char, Option<String>)> = spans.iter()
                    .flat_map(|s| s.text.chars().map(|c| (c, s.style.clone())))
                    .collect();
                let mut lines: Vec<StyledLine> = Vec::with_capacity(stream.len() / limit + 1);
                let mut cur: Vec<(char, Option<String>)> = Vec::new();
                for item in stream {
                    if cur.len() == limit {
                        lines.push(StyledLine { spans: merge_spans(&cur) });
                        cur.clear();
                    }
                    cur.push(item);
                }
                if !cur.is_empty() {
                    lines.push(StyledLine { spans: merge_spans(&cur) });
                }
                lines
            } else {
                // 现有截断逻辑保持不动;显式 width 时不足补齐到 limit(固定列宽语义)
                let mut taken = 0usize;
                let mut out_spans = Vec::new();
                for span in spans {
                    if taken >= limit {
                        break;
                    }
                    let t: String = span
                        .text
                        .chars()
                        .take(limit - taken)
                        .collect();
                    taken += t.chars().count();
                    out_spans.push(TextSpan { text: t, style: span.style.clone() });
                }
                if width.is_some() && taken < limit {
                    out_spans.push(TextSpan { text: " ".repeat(limit - taken), style: None });
                }
                vec![StyledLine { spans: out_spans }]
            }
        }
        CompNode::Col { children, gap, .. } => {
            let total_flex: u16 = children.iter().filter_map(flex_of).sum();
            if total_flex == 0 {
                // ==== 现有逻辑(不动) ====
                let mut out = Vec::new();
                for child in children {
                    if out.len() >= viewport.1 as usize {
                        break;
                    }
                    // gap：子节点之间插空行（不超 viewport 高度）
                    if !out.is_empty() && *gap > 0 && out.len() < viewport.1 as usize {
                        out.push(StyledLine::plain(""));
                    }
                    for line in layout(child, viewport) {
                        if out.len() >= viewport.1 as usize {
                            break;
                        }
                        out.push(line);
                    }
                }
                out
            } else {
                // ==== 两遍分配(垂直) ====
                // 第一遍: flex=0 子节点正常布局并记录高度; flex>0 子节点留空
                let mut parts: Vec<Vec<StyledLine>> = Vec::with_capacity(children.len());
                let mut fixed_h = 0usize;
                for child in children {
                    match flex_of(child) {
                        None => {
                            let lines = layout(child, viewport);
                            fixed_h += lines.len();
                            parts.push(lines);
                        }
                        Some(_) => parts.push(Vec::new()),
                    }
                }
                // 剩余高度 = viewport 高 - 固定高 - gap 总高;按 flex 权重分配(阶梯法,无浮点)
                let gaps = children.len().saturating_sub(1) * *gap as usize;
                let remaining = (viewport.1 as usize).saturating_sub(fixed_h).saturating_sub(gaps);
                let mut alloc: Vec<usize> = Vec::with_capacity(children.len());
                let mut acc = 0usize;
                for child in children {
                    match flex_of(child) {
                        Some(f) => {
                            let lo = acc * remaining / total_flex as usize;
                            acc += f as usize;
                            let hi = acc * remaining / total_flex as usize;
                            alloc.push(hi - lo);
                        }
                        None => alloc.push(0),
                    }
                }
                // 第二遍: flex>0 子节点用分配高布局,不足补空行
                for (i, child) in children.iter().enumerate() {
                    if flex_of(child).is_some() {
                        let mut lines = layout(child, (viewport.0, alloc[i] as u16));
                        while lines.len() < alloc[i] {
                            lines.push(StyledLine::plain(""));
                        }
                        lines.truncate(alloc[i]); // 防止 flex 子节点溢出分配槽(嵌套 Col/wrap 多行场景)
                        parts[i] = lines;
                    }
                }
                // 拼接(带 gap 空行,clamp 到 viewport 高度)
                let mut out = Vec::new();
                for (i, part) in parts.into_iter().enumerate() {
                    if out.len() >= viewport.1 as usize {
                        break;
                    }
                    if !out.is_empty() && i > 0 && *gap > 0 && out.len() < viewport.1 as usize {
                        out.push(StyledLine::plain(""));
                    }
                    for line in part {
                        if out.len() >= viewport.1 as usize {
                            break;
                        }
                        out.push(line);
                    }
                }
                out
            }
        }
        CompNode::Row { children, gap, .. } => {
            let total_flex: u16 = children.iter().filter_map(flex_of).sum();
            let parts: Vec<Vec<StyledLine>> = if total_flex == 0 {
                // ==== 现有逻辑(不动) ====
                children.iter().map(|c| layout(c, viewport)).collect()
            } else {
                // ==== 两遍分配(水平) ====
                // 第一遍: flex=0 子节点正常布局并记录宽度; flex>0 子节点留空
                let mut parts: Vec<Vec<StyledLine>> = Vec::with_capacity(children.len());
                let mut fixed_w = 0usize;
                for child in children {
                    match flex_of(child) {
                        None => {
                            let lines = layout(child, viewport);
                            fixed_w += lines.iter().map(|l| l.width()).max().unwrap_or(0);
                            parts.push(lines);
                        }
                        Some(_) => parts.push(Vec::new()),
                    }
                }
                // 剩余空间 = viewport 宽 - 固定宽 - gap 总宽;按 flex 权重分配(阶梯法,无浮点)
                let gaps = children.len().saturating_sub(1) * *gap as usize;
                let remaining = (viewport.0 as usize).saturating_sub(fixed_w).saturating_sub(gaps);
                let mut alloc: Vec<usize> = Vec::with_capacity(children.len());
                let mut acc = 0usize;
                for child in children {
                    match flex_of(child) {
                        Some(f) => {
                            let lo = acc * remaining / total_flex as usize;
                            acc += f as usize;
                            let hi = acc * remaining / total_flex as usize;
                            alloc.push(hi - lo);
                        }
                        None => alloc.push(0),
                    }
                }
                // 第二遍: flex>0 子节点用分配宽布局(Text 补空格到分配宽)
                for (i, child) in children.iter().enumerate() {
                    if flex_of(child).is_some() {
                        let mut lines = layout(child, (alloc[i] as u16, viewport.1));
                        for line in &mut lines {
                            let pad = alloc[i].saturating_sub(line.width());
                            if pad > 0 {
                                line.spans.push(TextSpan { text: " ".repeat(pad), style: None });
                            }
                        }
                        parts[i] = lines;
                    }
                }
                parts
            };
            let rows = parts.iter().map(Vec::len).max().unwrap_or(0);
            let mut out = Vec::with_capacity(rows);
            for i in 0..rows {
                // 多 span 行：每段独立样式（不再坍缩），gap 作为普通空格段
                let mut spans: Vec<TextSpan> = Vec::new();
                let mut width_used = 0usize;
                for (ci, part) in parts.iter().enumerate() {
                    if width_used >= viewport.0 as usize {
                        break;
                    }
                    if ci > 0 {
                        let gap = (*gap as usize).min(viewport.0 as usize - width_used);
                        if gap > 0 {
                            spans.push(TextSpan { text: " ".repeat(gap), style: None });
                        }
                        width_used += gap;
                    }
                    if let Some(line) = part.get(i) {
                        let mut rem = viewport.0 as usize - width_used;
                        for span in &line.spans {
                            if rem == 0 {
                                break;
                            }
                            let t: String = span.text.chars().take(rem).collect();
                            rem -= t.chars().count();
                            width_used += t.chars().count();
                            spans.push(TextSpan { text: t, style: span.style.clone() });
                        }
                    }
                }
                out.push(StyledLine { spans });
            }
            out
        }
        CompNode::Button { label, width, .. } => {
            // 渲染为 [ label ]（焦点样式由 JS 侧 render(focus) 控制）
            let limit = width.unwrap_or(u16::MAX).min(viewport.0.saturating_sub(2)) as usize;
            let mut spans = vec![TextSpan { text: "[ ".into(), style: None }];
            let mut taken = 0usize;
            for span in label {
                if taken >= limit {
                    break;
                }
                let t: String = span.text.chars().take(limit - taken).collect();
                taken += t.chars().count();
                spans.push(TextSpan { text: t, style: span.style.clone() });
            }
            spans.push(TextSpan { text: " ]".into(), style: None });
            vec![StyledLine { spans }]
        }
        CompNode::Input { value, width, .. } => {
            let limit = width.unwrap_or(u16::MAX).min(viewport.0) as usize;
            let text: String = value.chars().take(limit).collect();
            vec![StyledLine::plain(text)]
        }
        CompNode::Scroll { children, height, .. } => {
            let h = (*height).min(viewport.1) as usize;
            if h == 0 {
                return Vec::new();
            }
            // 内容按无高度限制完整布局（scroll 语义：内容可溢出），再保留最后 h 行
            let col = layout(&CompNode::Col { children: children.clone(), gap: 0, flex: None }, (viewport.0, u16::MAX));
            let skip = col.len().saturating_sub(h);
            col.into_iter().skip(skip).collect()
        }
    }
}


use helix_view::graphics::Style;

// ── 脏格增量渲染（方案乙③）──

/// 插件面板/弹窗渲染器：全量渲染（每帧清区域 + 写全部行）。
/// 不做跨帧 diff——终端外部清空 surface（切 tab、OS 重绘、resize）时 diff 状态失效
/// 会导致内容不重画/错位；面板区域小，全量成本可忽略，termina 后端自带终端级 diff。
#[derive(Default)]
pub struct DiffRenderer;

impl DiffRenderer {
    /// 全量渲染 lines 到 surface（area 左上角起）：先清空区域，再写全部行。
    ///
    /// 样式全量解析：scope 样式缺 fg → 回退 ui.text（bg-only 样式/无样式行文字可读），
    /// 缺 bg → 回退 ui.background（面板/弹窗背景与编辑器一致）。写格前先 reset，
    /// 清掉残留颜色——否则 Cell::set_style 只覆盖 Some 字段，被取消选中的行会
    /// 永远留着旧高亮背景（残影），且 fg=Reset 的文字在暗背景下不可见。
    pub fn render(
        &mut self,
        lines: &[StyledLine],
        area: helix_view::graphics::Rect,
        surface: &mut tui::buffer::Buffer,
        theme: &helix_view::Theme,
    ) {
        let default_fg = theme.get("ui.text").fg;
        let default_bg = theme.get("ui.background").bg;
        let resolve = |mut style: Style| -> Style {
            if style.fg.is_none() {
                style.fg = default_fg;
            }
            if style.bg.is_none() {
                style.bg = default_bg;
            }
            style
        };
        let width = area.width as usize;
        let max_rows = area.height as usize;
        let blank = resolve(Style::default());
        // 全量：先清区域（背景），再写内容——无跨帧状态，外部清空 surface 后必然完整重画
        for y in 0..max_rows {
            for x in 0..width {
                let cell = &mut surface[(area.x + x as u16, area.y + y as u16)];
                cell.reset();
                cell.set_symbol(" ");
                cell.set_style(blank);
            }
        }
        for (y, line) in lines.iter().take(max_rows).enumerate() {
            let mut x = 0usize;
            for span in &line.spans {
                let style = resolve(
                    span.style
                        .as_ref()
                        .map(|s| theme.get(s))
                        .unwrap_or_default(),
                );
                for ch in span.text.chars() {
                    if x >= width {
                        break;
                    }
                    let cell = &mut surface[(area.x + x as u16, area.y + y as u16)];
                    cell.reset();
                    cell.set_symbol(&ch.to_string());
                    cell.set_style(style);
                    x += 1;
                }
                if x >= width {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_js::TextSpan;

    fn text(t: &str) -> CompNode {
        CompNode::Text { spans: vec![TextSpan { text: t.into(), style: None }], width: None, id: None, flex: None, wrap: false }
    }

    fn styled(t: &str, s: &str) -> CompNode {
        CompNode::Text { spans: vec![TextSpan { text: t.into(), style: Some(s.into()) }], width: None, id: None, flex: None, wrap: false }
    }

    fn texts(ts: &[&str]) -> Vec<CompNode> {
        ts.iter().map(|t| text(t)).collect()
    }

    fn line(t: &str) -> StyledLine {
        StyledLine::plain(t)
    }

    fn line_text(l: &StyledLine) -> String {
        l.spans.iter().map(|sp| sp.text.as_str()).collect()
    }

    #[test]
    fn scroll_full_window_outputs_all_rows() {
        // 模拟 filetree：viewport (32, 40)，scroll height 39，children 39 行
        let children: Vec<CompNode> = (0..39).map(|i| text(&format!("row{i:02}"))).collect();
        let node = CompNode::Scroll { children, height: 39, offset: None };
        let out = layout(&node, (32, 40));
        assert_eq!(out.len(), 39, "scroll 应输出全部 39 行，实际 {}", out.len());
        // 行内容应是前 39 行（skip=0）
        assert_eq!(line_text(&out[0]), "row00");
        assert_eq!(line_text(&out[38]), "row38");
    }

    #[test]
    fn scroll_truncates_to_viewport() {
        // children 100 行，viewport 高度 40，height 39 → 保留最后 39 行
        let children: Vec<CompNode> = (0..100).map(|i| text(&format!("r{i:03}"))).collect();
        let node = CompNode::Scroll { children, height: 39, offset: None };
        let out = layout(&node, (32, 40));
        assert_eq!(out.len(), 39, "scroll 应截断到 39 行");
        assert_eq!(line_text(&out[0]), "r061", "保留最后 39 行的第一行");
    }

    #[test]
    fn row_mixed_styles_keep_spans() {
        // 多 span 行模型：row 混合样式不再坍缩——每段独立保留
        let node = CompNode::Row {
            children: vec![
                CompNode::Text { spans: vec![TextSpan { text: "a".into(), style: None }], width: None, id: None, flex: None, wrap: false },
                CompNode::Text { spans: vec![TextSpan { text: "b".into(), style: Some("error".into()) }], width: None, id: None, flex: None, wrap: false },
            ],
            gap: 0,
            flex: None,
        };
        let out = layout(&node, (40, 10));
        assert_eq!(out.len(), 1);
        assert_eq!(line_text(&out[0]), "ab");
        assert_eq!(out[0].spans[0].style, None);
        assert_eq!(out[0].spans[1].style.as_deref(), Some("error"));
    }

    #[test]
    fn text_single_line_and_width_truncation() {
        assert_eq!(layout(&text("hello"), (40, 10)), vec![line("hello")]);
        // width 截断
        assert_eq!(
            layout(&CompNode::Text { spans: vec![TextSpan { text: "hello".into(), style: None }], width: Some(3), id: None, flex: None, wrap: false }, (40, 10)),
            vec![line("hel")]
        );
        // viewport 宽度优先于 width
        assert_eq!(
            layout(&CompNode::Text { spans: vec![TextSpan { text: "hello".into(), style: None }], width: Some(10), id: None, flex: None, wrap: false }, (3, 10)),
            vec![line("hel")]
        );
    }

    #[test]
    fn text_style_kept() {
        assert_eq!(layout(&styled("err", "error"), (40, 10)), vec![StyledLine::styled("err", "error")]);
    }

    #[test]
    fn col_stacks_and_clamps_to_viewport() {
        let node = CompNode::Col { children: texts(&["a", "b", "c"]), gap: 0, flex: None };
        assert_eq!(layout(&node, (40, 10)), vec![line("a"), line("b"), line("c")]);
        // viewport 高度限制
        assert_eq!(layout(&node, (40, 2)), vec![line("a"), line("b")]);
    }

    #[test]
    fn col_gap_inserts_blank_lines() {
        let node = CompNode::Col { children: texts(&["a", "b"]), gap: 1, flex: None };
        assert_eq!(layout(&node, (40, 10)), vec![line("a"), line(""), line("b")]);
    }

    #[test]
    fn row_side_by_side_with_gap() {
        let node = CompNode::Row { children: texts(&["left", "right"]), gap: 1, flex: None };
        let out = layout(&node, (40, 10));
        assert_eq!(line_text(&out[0]), "left right");
        assert_eq!(out[0].spans.len(), 3); // left + gap + right（每段独立 span）
    }

    #[test]
    fn row_pads_shorter_children() {
        // 子节点高度不一致：短子补空行（第 2 行 = 长子第 2 行，短子为空）
        let node = CompNode::Row {
            children: vec![CompNode::Col { children: texts(&["a1", "a2"]), gap: 0, flex: None }, text("b")],
            gap: 0,
            flex: None,
        };
        let out = layout(&node, (40, 10));
        assert_eq!(line_text(&out[0]), "a1b");
        assert_eq!(line_text(&out[1]), "a2");
    }

    #[test]
    fn row_width_sum_and_viewport_truncation() {
        let node = CompNode::Row { children: texts(&["aaaa", "bbbb"]), gap: 1, flex: None };
        // 宽度求和 + gap
        let out = layout(&node, (40, 10));
        assert_eq!(line_text(&out[0]), "aaaa bbbb");
        // 超 viewport 截断
        let out = layout(&node, (6, 10));
        assert_eq!(line_text(&out[0]), "aaaa b");
    }

    #[test]
    fn scroll_keeps_last_height_lines() {
        let node = CompNode::Scroll { children: texts(&["s1", "s2"]), height: 1, offset: None };
        assert_eq!(layout(&node, (40, 10)), vec![line("s2")]);
        // height 超 viewport → clamp 到视口
        let node = CompNode::Scroll { children: texts(&["s1", "s2"]), height: 10, offset: None };
        assert_eq!(layout(&node, (40, 1)), vec![line("s2")]);
    }

    #[test]
    fn nested_col_row_text() {
        let node = CompNode::Col {
            children: vec![
                styled("title", "error"),
                CompNode::Row { children: texts(&["left", "right"]), gap: 1, flex: None },
                CompNode::Scroll { children: texts(&["s1", "s2"]), height: 1, offset: None },
            ],
            gap: 0,
            flex: None,
        };
        let out = layout(&node, (40, 10));
        assert_eq!(line_text(&out[0]), "title");
        assert_eq!(out[0].spans[0].style.as_deref(), Some("error"));
        assert_eq!(line_text(&out[1]), "left right");
        assert_eq!(out[1].spans.len(), 3, "left + gap + right");
        assert_eq!(line_text(&out[2]), "s2");
    }

    #[test]
    fn row_flex_allocates_ratio() {
        // viewport (100, 10); flex:1 + flex:2 → 两列按 1:2 分(减 1 个 gap)
        let node = CompNode::Row {
            children: vec![
                CompNode::Text { spans: vec![TextSpan { text: "a".into(), style: None }], width: None, id: None, flex: Some(1), wrap: false },
                CompNode::Text { spans: vec![TextSpan { text: "b".into(), style: None }], width: None, id: None, flex: Some(2), wrap: false },
            ],
            gap: 1,
            flex: None,
        };
        let lines = layout(&node, (100, 10));
        let text = lines[0].spans.iter().map(|s| s.text.as_str()).collect::<String>();
        // 内容 "a" + 空格 + "b" + 空格填充: 总宽 100, a 列 33, b 列 66(减 gap 1)
        assert_eq!(text.chars().count(), 100);
        let a = text.find('a').unwrap();
        let b = text.find('b').unwrap();
        assert!(b - a > 30 && b - a < 36, "1:2 比例: b 距 a {}(期望 ~33)", b - a);
    }

    #[test]
    fn row_flex_zero_keeps_content_width() {
        // flex:0(缺省) + 固定 width:8 → 总宽 = 内容 + 8 + gap
        let node = CompNode::Row {
            children: vec![
                CompNode::Text { spans: vec![TextSpan { text: "hello".into(), style: None }], width: None, id: None, flex: None, wrap: false },
                CompNode::Text { spans: vec![TextSpan { text: "size".into(), style: None }], width: Some(8), id: None, flex: None, wrap: false },
            ],
            gap: 1,
            flex: None,
        };
        let lines = layout(&node, (100, 10));
        let text = lines[0].spans.iter().map(|s| s.text.as_str()).collect::<String>();
        assert_eq!(text.chars().count(), 5 + 1 + 8);
    }

    #[test]
    fn col_flex_allocates_height_ratio() {
        // flex:1 + flex:2 的 Col → 剩余高度按 1:2 分: a 槽 2 行(内容 1 + 空行 1), b 槽 4 行
        let node = CompNode::Col {
            children: vec![
                CompNode::Text { spans: vec![TextSpan { text: "a".into(), style: None }], width: None, id: None, flex: Some(1), wrap: false },
                CompNode::Text { spans: vec![TextSpan { text: "b".into(), style: None }], width: None, id: None, flex: Some(2), wrap: false },
            ],
            gap: 0,
            flex: None,
        };
        let out = layout(&node, (10, 6));
        let texts: Vec<String> = out.iter().map(line_text).collect();
        assert_eq!(out.len(), 6, "a 槽 2 行 + b 槽 4 行");
        assert_eq!(texts[0], "a");
        assert_eq!(texts[1], "");
        assert_eq!(texts[2], "b");
        assert!(texts[3..].iter().all(|t| t.is_empty()), "b 槽余下为空行");
    }

    #[test]
    fn col_flex_truncates_overflow_to_slot() {
        // wrap 子节点行数 > 分配高时截断到槽内,不溢入下一兄弟槽
        let node = CompNode::Col {
            children: vec![
                CompNode::Text { spans: vec![TextSpan { text: "abcdefghijklmnop".into(), style: None }], width: None, id: None, flex: Some(1), wrap: true },
                CompNode::Text { spans: vec![TextSpan { text: "x".into(), style: None }], width: None, id: None, flex: Some(1), wrap: false },
            ],
            gap: 0,
            flex: None,
        };
        // 16 字符 limit 4 → wrap 4 行,但两槽各只分 2 行:必须截断,不能把 "x" 挤出视口
        let out = layout(&node, (4, 4));
        let texts: Vec<String> = out.iter().map(line_text).collect();
        assert_eq!(out.len(), 4, "两槽各 2 行,总行数 = 分配和");
        assert_eq!(texts[0], "abcd");
        assert_eq!(texts[1], "efgh");
        assert_eq!(texts[2], "x");
        assert_eq!(texts[3], "");
    }

    #[test]
    fn text_wrap_multiline() {
        // wrap=true, width=5: "abcdefghij" → 两行 "abcde" / "fghij"
        let node = CompNode::Text { spans: vec![TextSpan { text: "abcdefghij".into(), style: None }], width: Some(5), id: None, flex: None, wrap: true };
        let lines = layout(&node, (5, 10));
        assert_eq!(lines.len(), 2);
        let joined = lines.iter().map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>()).collect::<Vec<_>>();
        assert_eq!(joined, vec!["abcde", "fghij"]);
    }

    #[test]
    fn text_wrap_cjk_keeps_chars() {
        // 中文 6 字符 width=4 → 两行,不切坏字符
        let node = CompNode::Text { spans: vec![TextSpan { text: "中文测试文本".into(), style: None }], width: Some(4), id: None, flex: None, wrap: true };
        let lines = layout(&node, (4, 10));
        let joined: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.text.as_str())).collect();
        assert_eq!(joined, "中文测试文本");
        assert!(lines.iter().all(|l| l.width() <= 4));
    }

    #[test]
    fn text_wrap_false_truncates() {
        // 回归: 无 wrap → 现状截断
        let node = CompNode::Text { spans: vec![TextSpan { text: "hello world".into(), style: None }], width: Some(5), id: None, flex: None, wrap: false };
        assert_eq!(layout(&node, (100, 10)).len(), 1);
    }

    #[test]
    fn render_passthrough_lines_and_tree() {
        let lines = vec![line("x")];
        assert_eq!(render(Content::Lines(lines.clone()), (40, 10)), lines);
        let out = render(
            Content::Tree(CompNode::Row { children: texts(&["a", "b"]), gap: 0, flex: None }),
            (40, 10),
        );
        assert_eq!(line_text(&out[0]), "ab");
    }
}
