//! 组件树布局引擎：把 JS render 返回的 CompNode 树渲染成样式化行（StyledLine）。
//! 样式名不在此解析（helix-js 只透传字符串），由调用方经 theme.get 映射。

use helix_js::{CompNode, Content, StyledLine};

/// 统一入口：Content::Lines 原样返回（旧行 API）；Content::Tree 走布局引擎。
pub fn render(content: Content, viewport: (u16, u16)) -> Vec<StyledLine> {
    match content {
        Content::Lines(lines) => lines,
        Content::Tree(node) => layout(&node, viewport),
    }
}

/// 组件树 → 样式化行：
/// - text → 单行（width 截断，viewport 宽度优先）
/// - col → 子节点自上而下堆叠（gap 空行；viewport 高度限制）
/// - row → 子节点各自布局成行集，第 i 行 = 各子第 i 行拼接（行数不足补空行；
///   gap 列间距；总宽超 viewport 截断）
/// - scroll → 按 col（无 gap）布局后保留最后 height 行（viewport 高度内）
pub fn layout(node: &CompNode, viewport: (u16, u16)) -> Vec<StyledLine> {
    match node {
        CompNode::Text { text, style, width } => {
            let limit = width.unwrap_or(u16::MAX).min(viewport.0) as usize;
            let text: String = text.chars().take(limit).collect();
            vec![StyledLine { text, style: style.clone() }]
        }
        CompNode::Col { children, gap } => {
            let mut out = Vec::new();
            for child in children {
                if out.len() >= viewport.1 as usize {
                    break;
                }
                // gap：子节点之间插空行（不超 viewport 高度）
                if !out.is_empty() && *gap > 0 && out.len() < viewport.1 as usize {
                    out.push(StyledLine { text: String::new(), style: None });
                }
                for line in layout(child, viewport) {
                    if out.len() >= viewport.1 as usize {
                        break;
                    }
                    out.push(line);
                }
            }
            out
        }
        CompNode::Row { children, gap } => {
            let parts: Vec<Vec<StyledLine>> = children.iter().map(|c| layout(c, viewport)).collect();
            let rows = parts.iter().map(Vec::len).max().unwrap_or(0);
            let mut out = Vec::with_capacity(rows);
            for i in 0..rows {
                let mut line = String::new();
                let mut width_used = 0usize;
                // 段样式：各非空段样式一致才保留，否则 None
                // ponytail: StyledLine 是单 (text, style) 对，row 混合样式会坍缩为 None；
                // 若需要逐段样式需把行模型升级为多 span。
                let mut first_style: Option<&str> = None;
                let mut saw_unstyled = false;
                let mut styles_agree = true;
                for (ci, part) in parts.iter().enumerate() {
                    if width_used >= viewport.0 as usize {
                        break;
                    }
                    if ci > 0 {
                        let gap = (*gap as usize).min(viewport.0 as usize - width_used);
                        line.push_str(&" ".repeat(gap));
                        width_used += gap;
                    }
                    let seg_style = part.get(i).and_then(|l| l.style.as_deref());
                    let seg: String = part
                        .get(i)
                        .map(|l| l.text.as_str())
                        .unwrap_or("")
                        .chars()
                        .take(viewport.0 as usize - width_used)
                        .collect();
                    width_used += seg.chars().count();
                    line.push_str(&seg);
                    if !seg.is_empty() {
                        // 首个非空样式为 baseline；后续段样式不同、从有到无、或从无到有 → 整行样式不一致
                        match (first_style, seg_style) {
                            (None, Some(s)) if !saw_unstyled => first_style = Some(s),
                            (None, Some(_)) => styles_agree = false,
                            (Some(a), Some(b)) if a != b => styles_agree = false,
                            (Some(_), None) => styles_agree = false,
                            (None, None) => saw_unstyled = true,
                            _ => {}
                        }
                    }
                }
                let style = if styles_agree { first_style.map(str::to_string) } else { None };
                out.push(StyledLine { text: line, style });
            }
            out
        }
        CompNode::Scroll { children, height } => {
            let h = (*height).min(viewport.1) as usize;
            if h == 0 {
                return Vec::new();
            }
            // 内容按无高度限制完整布局（scroll 语义：内容可溢出），再保留最后 h 行
            let col = layout(&CompNode::Col { children: children.clone(), gap: 0 }, (viewport.0, u16::MAX));
            let skip = col.len().saturating_sub(h);
            col.into_iter().skip(skip).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> CompNode {
        CompNode::Text { text: t.into(), style: None, width: None }
    }

    fn styled(t: &str, s: &str) -> CompNode {
        CompNode::Text { text: t.into(), style: Some(s.into()), width: None }
    }

    fn texts(ts: &[&str]) -> Vec<CompNode> {
        ts.iter().map(|t| text(t)).collect()
    }

    fn line(t: &str) -> StyledLine {
        StyledLine { text: t.into(), style: None }
    }

    #[test]
    fn row_mixed_styles_collapse_to_none() {
        // 无样式段在带样式段之前出现 → 整行样式为 None（对称性：从有到无同样不一致）
        let node = CompNode::Row {
            children: vec![
                CompNode::Text { text: "a".into(), style: None, width: None },
                CompNode::Text { text: "b".into(), style: Some("error".into()), width: None },
            ],
            gap: 0,
        };
        let out = layout(&node, (40, 10));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "ab");
        assert_eq!(out[0].style, None, "mixed styles must collapse to None");
    }

    #[test]
    fn text_single_line_and_width_truncation() {
        assert_eq!(layout(&text("hello"), (40, 10)), vec![line("hello")]);
        // width 截断
        assert_eq!(
            layout(&CompNode::Text { text: "hello".into(), style: None, width: Some(3) }, (40, 10)),
            vec![line("hel")]
        );
        // viewport 宽度优先于 width
        assert_eq!(
            layout(&CompNode::Text { text: "hello".into(), style: None, width: Some(10) }, (3, 10)),
            vec![line("hel")]
        );
    }

    #[test]
    fn text_style_kept() {
        assert_eq!(layout(&styled("err", "error"), (40, 10)), vec![StyledLine { text: "err".into(), style: Some("error".into()) }]);
    }

    #[test]
    fn col_stacks_and_clamps_to_viewport() {
        let node = CompNode::Col { children: texts(&["a", "b", "c"]), gap: 0 };
        assert_eq!(layout(&node, (40, 10)), vec![line("a"), line("b"), line("c")]);
        // viewport 高度限制
        assert_eq!(layout(&node, (40, 2)), vec![line("a"), line("b")]);
    }

    #[test]
    fn col_gap_inserts_blank_lines() {
        let node = CompNode::Col { children: texts(&["a", "b"]), gap: 1 };
        assert_eq!(layout(&node, (40, 10)), vec![line("a"), line(""), line("b")]);
    }

    #[test]
    fn row_side_by_side_with_gap() {
        let node = CompNode::Row { children: texts(&["left", "right"]), gap: 1 };
        assert_eq!(layout(&node, (40, 10)), vec![line("left right")]);
    }

    #[test]
    fn row_pads_shorter_children() {
        // 子节点高度不一致：短子补空行（第 2 行 = 长子第 2 行，短子为空）
        let node = CompNode::Row {
            children: vec![CompNode::Col { children: texts(&["a1", "a2"]), gap: 0 }, text("b")],
            gap: 0,
        };
        assert_eq!(layout(&node, (40, 10)), vec![line("a1b"), line("a2")]);
    }

    #[test]
    fn row_width_sum_and_viewport_truncation() {
        let node = CompNode::Row { children: texts(&["aaaa", "bbbb"]), gap: 1 };
        // 宽度求和 + gap
        assert_eq!(layout(&node, (40, 10)), vec![line("aaaa bbbb")]);
        // 超 viewport 截断
        assert_eq!(layout(&node, (6, 10)), vec![line("aaaa b")]);
    }

    #[test]
    fn scroll_keeps_last_height_lines() {
        let node = CompNode::Scroll { children: texts(&["s1", "s2"]), height: 1 };
        assert_eq!(layout(&node, (40, 10)), vec![line("s2")]);
        // height 超 viewport → clamp 到视口
        let node = CompNode::Scroll { children: texts(&["s1", "s2"]), height: 10 };
        assert_eq!(layout(&node, (40, 1)), vec![line("s2")]);
    }

    #[test]
    fn nested_col_row_text() {
        let node = CompNode::Col {
            children: vec![
                styled("title", "error"),
                CompNode::Row { children: texts(&["left", "right"]), gap: 1 },
                CompNode::Scroll { children: texts(&["s1", "s2"]), height: 1 },
            ],
            gap: 0,
        };
        assert_eq!(
            layout(&node, (40, 10)),
            vec![
                StyledLine { text: "title".into(), style: Some("error".into()) },
                line("left right"),
                line("s2"),
            ]
        );
    }

    #[test]
    fn render_passthrough_lines_and_tree() {
        let lines = vec![line("x")];
        assert_eq!(render(Content::Lines(lines.clone()), (40, 10)), lines);
        assert_eq!(
            render(Content::Tree(CompNode::Row { children: texts(&["a", "b"]), gap: 0 }), (40, 10)),
            vec![line("ab")]
        );
    }
}
