//! 折叠候选的自动推导（P0：缩进启发式）。
//!
//! 纯函数：只吃文本、吐**行号区间**，不依赖 `View`/`Editor`/`Document` ⇒ 可以单测，
//! 也是将来 tree-sitter `folds.scm` 与 LSP `foldingRange` 的公共出口形态
//! （三种来源都产出 `start..end` 行区间，再由 `helix-view` 的 `Folds::from_lines` 落地）。
//!
//! P0 的模型是**扁平**的（`helix-view/src/fold.rs` 里折叠不重叠、不建树），
//! 所以这里只产出**最外层**候选：遇到一个块就整块跳过去，不在块内再产嵌套候选。

use std::ops::Range;

use crate::indent::indent_level_for_line;
use crate::RopeSlice;

/// 由缩进推导折叠候选。
///
/// 规则（vim `foldmethod=indent` 的最小版）：
/// - 本行有内容、且下一行缩进**更深** ⇒ 本行是折叠头；
/// - 一直折到「下一行的缩进 ≤ 本行缩进」为止（空行不算边界，跟着块走）；
/// - 至少藏 `min_lines` 行才算候选（不足直接丢弃，避免折叠 1 行＝没意义）；
/// - 缩进按 `tab_width` 展开后除以 `indent_width`，与 `:indent-*` 同一套算法。
pub fn indent_folds(
    text: RopeSlice,
    tab_width: usize,
    indent_width: usize,
    min_lines: usize,
) -> Vec<Range<usize>> {
    let min_lines = min_lines.max(1);
    let line_count = text.len_lines();
    let mut folds = Vec::new();
    let mut line = 0;

    while line + 1 < line_count {
        // 空行不是头行（否则整个文件都会被一条空行折起来）
        let Some(header_indent) = line_indent(text, line, tab_width, indent_width) else {
            line += 1;
            continue;
        };
        // 下一行必须更深，否则这条线没有块
        let deeper = match line_indent(text, line + 1, tab_width, indent_width) {
            Some(next) => next > header_indent,
            None => false,
        };
        if !deeper {
            line += 1;
            continue;
        }

        // 找块末：第一个「非空且缩进 ≤ 头行缩进」的行
        let mut end = line + 2;
        while end < line_count {
            match line_indent(text, end, tab_width, indent_width) {
                // 文件末尾的空行（尾随换行产生的幻影行）不算块内
                None if end + 1 == line_count => break,
                // 其余空行跟着块走（不切断，与 vim 一致）
                None => end += 1,
                Some(indent) if indent > header_indent => end += 1,
                Some(_) => break,
            }
        }

        if end - line > min_lines {
            folds.push(line..end);
        }
        // 扁平模型：整块跳过去，块内不再产嵌套候选
        line = end.max(line + 1);
    }

    folds
}

/// 行缩进；**空行/纯空白行** 返回 `None`（它们既不是头行，也不当边界）。
fn line_indent(
    text: RopeSlice,
    line: usize,
    tab_width: usize,
    indent_width: usize,
) -> Option<usize> {
    let line = text.line(line);
    if line.chars().all(char::is_whitespace) {
        return None;
    }
    Some(indent_level_for_line(line, tab_width, indent_width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rope;

    fn folds(src: &str, min_lines: usize) -> Vec<Range<usize>> {
        let rope = Rope::from(src);
        indent_folds(rope.slice(..), 4, 1, min_lines)
    }

    #[test]
    fn folds_a_simple_block() {
        let src = "fn a() {\n    let x = 1;\n    let y = 2;\n}\n";
        // 头行 0，藏第 1、2 行，块后是第 3 行 ⇒ 0..3
        assert_eq!(folds(src, 2), vec![0..3]);
        // 要藏 3 行才算 ⇒ 不够，不折
        assert!(folds(src, 3).is_empty());
    }

    #[test]
    fn only_the_outermost_block_is_reported() {
        let src = "fn a() {\n    if x {\n        y();\n    }\n    z();\n}\n";
        // 扁平模型：外层 0..5，内层 if 不单独出现
        assert_eq!(folds(src, 2), vec![0..5]);
    }

    #[test]
    fn sibling_blocks_are_separate() {
        let src = "fn a() {\n    x();\n    y();\n}\n\nfn b() {\n    x();\n    y();\n}\n";
        assert_eq!(folds(src, 2), vec![0..3, 5..8]);
    }

    #[test]
    fn blank_lines_belong_to_the_block() {
        // 块中间的空行不切断块（end 一直走到缩进回到头行那一行）
        let src = "fn a() {\n    x();\n\n    y();\n}\n";
        // 空行跟着块走，`}` 不藏 ⇒ 0..4
        assert_eq!(folds(src, 2), vec![0..4]);
    }

    #[test]
    fn tabs_and_spaces_both_work() {
        let src = "a:\n\tb:\n\t\tc: 1\n\td: 2\ne: 3\n";
        // tab=4 ⇒ 头行 indent 0，藏 1..4
        assert_eq!(folds(src, 2), vec![0..4]);
    }

    #[test]
    fn flat_text_has_no_folds() {
        assert!(folds("one\ntwo\nthree\n", 2).is_empty());
    }

    #[test]
    fn dedent_ends_the_block() {
        let src = "outer\n    inner\n    inner2\nback\n    inner3\n    inner4\n";
        assert_eq!(folds(src, 2), vec![0..3, 3..6]);
    }
}
