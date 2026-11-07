//! 折叠状态：挂 `Document` 上（同一文档多窗口共享），只存**世界坐标**（char_idx），
//! 不存屏幕行号 —— 屏幕行由 `helix-core` 的排版器从 `concealed_ranges()` 现算。
//!
//! 设计见 `docs/superpowers/plans/2026-11-07-code-folding.md`：
//! 折叠不是"渲染时跳行"，而是把隐藏区间喂给 `TextAnnotations`，
//! 让 `DocumentFormatter` 走字时跳过 ⇒ 滚动/点击/`:goto` 的行算术自动看到同一个世界。
//!
//! # 坐标系约定（最容易搞错）
//!
//! `Fold { start, end }` 用 **char_idx**：
//! - `start` = 折叠**头行**的**行首**字符（头行本身仍然显示）；
//! - `end`   = 折叠块**之后那一行**的行首字符（exclusive）。
//!
//! 真正隐藏的是 `[头行内容末尾, 折叠块最后一行内容末尾)` —— 见 [`Folds::concealed_ranges`]：
//! **保留折叠块最后一个换行**，头行才能独占一行、下一可见行从新行开始。

use std::ops::Range;

use helix_core::ChangeSet;
use helix_core::RopeSlice;

/// 一个折叠（坐标系见模块文档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Fold {
    pub start: usize,
    pub end: usize,
}

impl Fold {
    pub fn new(start: usize, end: usize) -> Option<Self> {
        (start < end).then_some(Self { start, end })
    }

    /// `char_idx` 是否被这个折叠藏住了（头行不算）。
    pub fn contains(&self, char_idx: usize) -> bool {
        self.start <= char_idx && char_idx < self.end
    }

    /// 折叠块藏起来的**文档行数**（不含头行）。
    pub fn hidden_lines(&self, text: RopeSlice) -> usize {
        text.char_to_line(self.end)
            .saturating_sub(text.char_to_line(self.start) + 1)
    }
}

/// 一个文档的折叠集合：按 `start` 有序、互不重叠。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Folds {
    folds: Vec<Fold>,
}

impl Folds {
    pub fn all(&self) -> &[Fold] {
        &self.folds
    }

    pub fn len(&self) -> usize {
        self.folds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.folds.is_empty()
    }

    pub fn clear(&mut self) {
        self.folds.clear();
    }

    /// 包含 `char_idx` 的折叠（头行也算"在这个折叠上"）。
    pub fn at(&self, char_idx: usize) -> Option<Fold> {
        let idx = self.folds.partition_point(|fold| fold.end <= char_idx);
        self.folds
            .get(idx)
            .copied()
            .filter(|fold| fold.start <= char_idx)
    }

    /// 这个位置是否**看不见**了（落在被隐藏的正文里，而不是折叠头行上）。
    ///
    /// `fold-jump` 的两个策略都只对"看不见的光标"动手：光标停在折叠**头行**上
    /// 不该触发任何动作（否则 `j` 一碰到折叠行的头行就自己展开了）。
    pub fn is_hidden(&self, text: RopeSlice, char_idx: usize) -> bool {
        self.at(char_idx)
            .is_some_and(|fold| char_idx > line_content_end(text, text.char_to_line(fold.start)))
    }

    /// 加一个折叠。与已有折叠重叠/相邻的会合并（P0 不建嵌套树，见计划 §1）。
    pub fn add(&mut self, fold: Fold) {
        let mut start = fold.start;
        let mut end = fold.end;
        self.folds.retain(|existing| {
            if existing.end < start || existing.start > end {
                return true;
            }
            // 新折叠可能夹在旧折叠中间 ⇒ 两端都要取并集
            start = start.min(existing.start);
            end = end.max(existing.end);
            false
        });
        let pos = self
            .folds
            .partition_point(|existing| existing.start < start);
        self.folds.insert(pos, Fold { start, end });
    }

    /// 去掉包含 `char_idx` 的折叠（`zo`）。返回是否真的去掉了。
    pub fn remove_at(&mut self, char_idx: usize) -> bool {
        let Some(fold) = self.at(char_idx) else {
            return false;
        };
        self.folds.retain(|existing| *existing != fold);
        true
    }

    /// `za`：有就开，没有就折。返回操作后的状态（`true` = 现在处于折叠态）。
    pub fn toggle_at(&mut self, char_idx: usize) -> bool {
        match self.at(char_idx) {
            Some(fold) => {
                self.folds.retain(|existing| *existing != fold);
                false
            }
            None => false,
        }
    }

    /// 由**行号区间**（`lines.start..lines.end`，`end` 不含，头行是 `start`）构造折叠，
    /// 并把两端对齐到行首（`from_lines` 是 indent/tree-sitter/LSP 三种来源的公共出口）。
    pub fn from_lines(text: RopeSlice, lines: Range<usize>) -> Option<Fold> {
        let last_line = text.len_lines().saturating_sub(1);
        let start_line = lines.start.min(last_line);
        let end_line = lines.end.min(last_line + 1);
        if end_line <= start_line + 1 {
            // 只有头行（或空区间）⇒ 没有可藏的
            return None;
        }
        Fold::new(text.line_to_char(start_line), text.line_to_char(end_line))
    }

    /// 喂给排版器的隐藏区间（`TextAnnotations::set_concealed`）。
    ///
    /// 每个折叠给一段 `[头行内容末尾, 折叠块最后一行内容末尾)`：
    /// 两处都是**换行字前后一致的边界**（`\n` 或 `\r\n` 的字边界），
    /// 于是折叠块最后一个换行被**保留** ⇒ 行数真的减少且下一行另起一行。
    pub fn concealed_ranges(&self, text: RopeSlice) -> Vec<Range<usize>> {
        self.folds
            .iter()
            .filter_map(|fold| {
                let header_line = text.char_to_line(fold.start);
                let end_line = text.char_to_line(fold.end);
                if end_line <= header_line + 1 {
                    return None; // 内容被编辑删空了
                }
                let start = line_content_end(text, header_line);
                let end = line_content_end(text, end_line - 1);
                (start < end).then_some(start..end)
            })
            .collect()
    }

    /// 文档变化后平移折叠：两端按 changeset 映射，再对齐回行首；
    /// 被删空（或塌到一行）的折叠直接丢弃。
    pub fn adjust(&mut self, changes: &ChangeSet, text: RopeSlice) {
        if self.folds.is_empty() || changes.changes().is_empty() {
            return;
        }
        let ops: Vec<(usize, usize, isize)> = changes
            .changes_iter()
            .map(|(from, to, inserted)| {
                let inserted = inserted.map(|t| t.chars().count()).unwrap_or(0) as isize;
                (from, to, inserted - (to - from) as isize)
            })
            .collect();

        let map = |pos: usize| -> usize {
            let mut delta = 0isize;
            for (from, to, op_delta) in &ops {
                if pos < *from {
                    break;
                }
                if pos >= *to {
                    delta += op_delta;
                    continue;
                }
                // 落在被替换的区间内部：钳到替换起点
                return (*from as isize + delta).max(0) as usize;
            }
            (pos as isize + delta).max(0) as usize
        };

        self.folds = self
            .folds
            .iter()
            .filter_map(|fold| {
                let start = map(fold.start).min(text.len_chars());
                let end = map(fold.end).min(text.len_chars());
                // 重新对齐到行首（编辑可能把边界推进一行中间）
                let start_line = text.char_to_line(start);
                let end_line = text.char_to_line(end);
                if end <= start || end_line < start_line + 2 {
                    // 塌成一行或更少 ⇒ 没有可藏的（头行 + 至少一行才有意义）
                    return None;
                }
                Fold::new(text.line_to_char(start_line), text.line_to_char(end_line))
            })
            .collect();
    }
}

/// 行内容末尾的 char_idx（**不含**行尾换行；`\n` 与 `\r\n` 都按整字边界处理）。
fn line_content_end(text: RopeSlice, line: usize) -> usize {
    let line_start = text.line_to_char(line);
    let slice = text.line(line);
    let mut len = slice.len_chars();
    if len > 0 && slice.char(len - 1) == '\n' {
        len -= 1;
        if len > 0 && slice.char(len - 1) == '\r' {
            len -= 1;
        }
    }
    line_start + len
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_core::Rope;

    fn text(s: &str) -> Rope {
        Rope::from(s)
    }

    /// 坐标约定：`start` 是头行行首，`end` 是块后那一行的行首。
    #[test]
    fn from_lines_aligns_to_line_starts() {
        let rope = text("head\nhidden1\nhidden2\nafter\n");
        let text = rope.slice(..);
        let fold = Folds::from_lines(text, 0..3).expect("两行可折");
        assert_eq!(fold.start, 0, "头行行首");
        assert_eq!(fold.end, 21, "块后一行（after）的行首");
        assert_eq!(fold.hidden_lines(text), 2);
        assert!(Folds::from_lines(text, 0..1).is_none(), "只有头行不折");
    }

    /// 隐藏区间保留块末换行：头行之后直接是块后的那一行。
    #[test]
    fn concealed_range_keeps_the_last_newline() {
        let rope = text("head\nhidden1\nhidden2\nafter\n");
        let text = rope.slice(..);
        let fold = Folds::from_lines(text, 0..3).unwrap();
        let mut folds = Folds::default();
        folds.add(fold);
        // "head"=0..4 内容末尾在 4；最后一行内容末尾在 20（换行在 20）
        assert_eq!(folds.concealed_ranges(text), vec![4..20]);
    }

    /// CRLF 也一样：两端都落在 `\r\n` 之前，保留的是整个 `\r\n` 字。
    #[test]
    fn concealed_range_handles_crlf() {
        let rope = text("head\r\nhidden1\r\nafter\r\n");
        let text = rope.slice(..);
        let fold = Folds::from_lines(text, 0..2).unwrap();
        let mut folds = Folds::default();
        folds.add(fold);
        // "head\r\n"=0..6（内容末尾 4），"hidden1\r\n"=6..15（内容末尾 13）
        assert_eq!(folds.concealed_ranges(text), vec![4..13]);
    }

    /// `at` 命中头行与块内，块外不命中。
    #[test]
    fn at_covers_header_and_body() {
        let rope = text("head\nhidden\nafter\n");
        let text = rope.slice(..);
        let mut folds = Folds::default();
        folds.add(Folds::from_lines(text, 0..2).unwrap());
        assert!(folds.at(0).is_some(), "头行");
        assert!(folds.at(6).is_some(), "块内");
        assert!(folds.at(13).is_none(), "块后那行不算");
    }

    /// `is_hidden`：头行不算隐藏，块内正文才算（fold-jump 的判据）。
    #[test]
    fn is_hidden_ignores_the_header_line() {
        let rope = text("head\nhidden\nafter\n");
        let text = rope.slice(..);
        let mut folds = Folds::default();
        folds.add(Folds::from_lines(text, 0..2).unwrap());
        assert!(!folds.is_hidden(text, 2), "头行内容是可见的");
        assert!(!folds.is_hidden(text, 4), "头行行尾换行仍可见");
        assert!(folds.is_hidden(text, 6), "块内正文看不见");
        assert!(!folds.is_hidden(text, 20), "块外不算");
    }

    /// 重叠/相邻的折叠合并（P0 不建嵌套树）。
    #[test]
    fn add_merges_overlapping() {
        let mut folds = Folds::default();
        folds.add(Fold::new(0, 20).unwrap());
        folds.add(Fold::new(10, 40).unwrap());
        folds.add(Fold::new(40, 60).unwrap());
        assert_eq!(folds.all(), &[Fold { start: 0, end: 60 }]);
    }

    /// 上面插行 ⇒ 整体后移；块内删空 ⇒ 丢弃。
    #[test]
    fn adjust_moves_and_drops_folds() {
        let before = text("aaa\nbbb\nccc\n");
        let mut folds = Folds::default();
        folds.add(Folds::from_lines(before.slice(..), 0..2).unwrap());

        // 在文档最前面插一行 ⇒ 两行都后移 4 个字符
        let after = text("zzz\naaa\nbbb\nccc\n");
        let txn = helix_core::Transaction::change(
            &before,
            std::iter::once((0, 0, Some(helix_core::Tendril::from("zzz\n")))),
        );
        folds.adjust(txn.changes(), after.slice(..));
        assert_eq!(folds.all(), &[Fold { start: 4, end: 12 }]);

        // 把被折叠的那一行删掉 ⇒ 塌成一行 ⇒ 丢弃
        let deleted = text("aaa\nccc\n");
        let txn = helix_core::Transaction::change(
            &before,
            std::iter::once((4, 8, Some(helix_core::Tendril::new()))),
        );
        let mut folds = Folds::default();
        folds.add(Folds::from_lines(before.slice(..), 0..2).unwrap());
        folds.adjust(txn.changes(), deleted.slice(..));
        assert!(folds.is_empty(), "删空后不该留空折叠");
    }
}

// ── 候选推导（`fold-method` → 折叠集合）─────────────────────────────────────
//
// 这份逻辑原先在 `helix-term/src/commands.rs`；gutter 也要画"可折但没折"的 `▼`，
// 于是下沉到 view 层：`Document`/`Editor` 自带推导 + 缓存，命令层与渲染层共用。

use crate::editor::FoldMethod;
use crate::Document;

/// 缓存键：文档版本 + 折叠方法（任一变化就重算）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldCandidateCache {
    pub revision: usize,
    pub method: FoldMethod,
    pub folds: Vec<Fold>,
}

/// 按 `fold-method` 推导候选折叠。
///
/// 返回 `(候选, 是否需要补一次 LSP 请求)`：`lsp` 的缓存为空时后者为 `true`
/// （请求本身只能在 `helix-term` 发，那里才有 `Editor`/语言服务器）。
/// `tree-sitter` 没有该语言的 `folds.scm`、或没语法 ⇒ 退 indent；`lsp` 同理。
pub fn candidates(
    doc: &Document,
    loader: &helix_core::syntax::Loader,
    method: FoldMethod,
    min_lines: usize,
) -> (Vec<Fold>, bool) {
    if method == FoldMethod::Manual {
        return (Vec::new(), false);
    }
    let text = doc.text().slice(..);
    let tab_width = doc.tab_width();
    let indent_width = doc.indent_style.indent_width(tab_width).max(1);
    let indent_lines = || helix_core::fold::indent_folds(text, tab_width, indent_width, min_lines);

    let mut needs_lsp = false;
    let line_ranges: Vec<std::ops::Range<usize>> = match method {
        FoldMethod::Manual => Vec::new(),
        FoldMethod::TreeSitter => match tree_sitter_lines(doc, loader) {
            Some(ranges) if !ranges.is_empty() => ranges,
            _ => indent_lines(),
        },
        FoldMethod::Lsp => {
            let lsp_lines = doc.lsp_fold_lines();
            if lsp_lines.is_empty() {
                needs_lsp = true;
                indent_lines()
            } else {
                lsp_lines
            }
        }
        FoldMethod::Indent => indent_lines(),
    };

    let folds = line_ranges
        .into_iter()
        .filter_map(|lines| Folds::from_lines(text, lines))
        .collect();
    (folds, needs_lsp)
}

/// `folds.scm` 给出的候选（行号区间）。没语法 / 没该语言的 query ⇒ `None`。
fn tree_sitter_lines(
    doc: &Document,
    loader: &helix_core::syntax::Loader,
) -> Option<Vec<std::ops::Range<usize>>> {
    let language_config = doc.language.as_ref()?;
    let query = loader.fold_query(language_config.language())?;
    let syntax = doc.syntax()?;
    let text = doc.text().slice(..);
    let mut lines = Vec::new();
    for range in query.fold_byte_ranges(syntax.tree().root_node(), text) {
        let start = text.byte_to_char(range.start.min(text.len_bytes()));
        let last = text.byte_to_char(range.end.saturating_sub(1).min(text.len_bytes()));
        let start_line = text.char_to_line(start);
        let end_line = text.char_to_line(last) + 1;
        if end_line > start_line + 1 {
            lines.push(start_line..end_line);
        }
    }
    Some(lines)
}

impl Document {
    /// 带缓存的候选推导：版本 + `fold-method` 都没变就复用。
    ///
    /// 缓存是必要的：gutter 每帧都要画 `▼`，而 tree-sitter 查询/indent 扫描都是整篇成本。
    pub fn fold_candidates_cached(
        &self,
        loader: &helix_core::syntax::Loader,
        method: FoldMethod,
        min_lines: usize,
    ) -> Vec<Fold> {
        let revision = self.version() as usize;
        if let Some(cache) = self.fold_candidate_cache.borrow().as_ref() {
            if cache.revision == revision && cache.method == method {
                return cache.folds.clone();
            }
        }
        let (folds, _) = candidates(self, loader, method, min_lines);
        *self.fold_candidate_cache.borrow_mut() = Some(FoldCandidateCache {
            revision,
            method,
            folds: folds.clone(),
        });
        folds
    }
}
