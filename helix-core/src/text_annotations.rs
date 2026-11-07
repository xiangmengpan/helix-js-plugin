use std::cell::Cell;
use std::cmp::Ordering;
use std::fmt::Debug;
use std::ops::{Deref, Range};
use std::ptr::NonNull;

use crate::doc_formatter::FormattedGrapheme;
use crate::syntax::{Highlight, OverlayHighlights};
use crate::{Position, Tendril};

/// An inline annotation is continuous text shown
/// on the screen before the grapheme that starts at
/// `char_idx`
#[derive(Debug, Clone)]
pub struct InlineAnnotation {
    pub text: Tendril,
    pub char_idx: usize,
}

impl InlineAnnotation {
    pub fn new(char_idx: usize, text: impl Into<Tendril>) -> Self {
        Self {
            char_idx,
            text: text.into(),
        }
    }
}

/// Represents a **single Grapheme** that is part of the document
/// that start at `char_idx` that will be replaced with
/// a different `grapheme`.
/// If `grapheme` contains multiple graphemes the text
/// will render incorrectly.
/// If you want to overlay multiple graphemes simply
/// use multiple `Overlays`.
///
/// # Examples
///
/// The following examples are valid overlays for the following text:
///
/// `aX͎̊͢͜͝͡bc`
///
/// ```
/// use helix_core::text_annotations::Overlay;
///
/// // replaces a
/// Overlay::new(0, "X");
///
/// // replaces X͎̊͢͜͝͡
/// Overlay::new(1, "\t");
///
/// // replaces b
/// Overlay::new(6, "X̢̢̟͖̲͌̋̇͑͝");
/// ```
///
/// The following examples are invalid uses
///
/// ```
/// use helix_core::text_annotations::Overlay;
///
/// // overlay is not aligned at grapheme boundary
/// Overlay::new(3, "x");
///
/// // overlay contains multiple graphemes
/// Overlay::new(0, "xy");
/// ```
#[derive(Debug, Clone)]
pub struct Overlay {
    pub char_idx: usize,
    pub grapheme: Tendril,
}

impl Overlay {
    pub fn new(char_idx: usize, grapheme: impl Into<Tendril>) -> Self {
        Self {
            char_idx,
            grapheme: grapheme.into(),
        }
    }
}

/// Line annotations allow inserting virtual text lines between normal text
/// lines.  These lines can be filled with text in the rendering code as their
/// contents have no effect beyond visual appearance.
///
/// The height of virtual text is usually not known ahead of time as virtual
/// text often requires softwrapping. Furthermore the height of some virtual
/// text like side-by-side diffs depends on the height of the text (again
/// influenced by softwrap) and other virtual text. Therefore line annotations
/// are computed on the fly instead of ahead of time like other annotations.
///
/// The core of this trait `insert_virtual_lines` function. It is called at the
/// end of every  visual line and allows the `LineAnnotation` to insert empty
/// virtual lines. Apart from that the `LineAnnotation` trait has multiple
/// methods that allow it to track anchors in the document.
///
/// When a new traversal of a document starts `reset_pos` is called. Afterwards
/// the other functions are called with indices that are larger then the
/// one passed to `reset_pos`. This allows performing a binary search (use
/// `partition_point`) in `reset_pos` once and then to only look at the next
/// anchor during each method call.
///
/// The `reset_pos`, `skip_conceal` and `process_anchor` functions all return a
/// `char_idx` anchor. This anchor is stored when transversing the document and
/// when the grapheme at the anchor is traversed the `process_anchor` function
/// is called.
///
/// # Note
///
/// All functions only receive immutable references to `self`.
/// `LineAnnotation`s that want to store an internal position or
/// state of some kind should use `Cell`. Using interior mutability for
/// caches is preferable as otherwise a lot of lifetimes become invariant
/// which complicates APIs a lot.
pub trait LineAnnotation {
    /// Resets the internal position to `char_idx`. This function is called
    /// when a new traversal of a document starts.
    ///
    /// All `char_idx` passed to `insert_virtual_lines` are strictly monotonically increasing
    /// with the first `char_idx` greater or equal to the `char_idx`
    /// passed to this function.
    ///
    /// # Returns
    ///
    /// The `char_idx` of the next anchor this `LineAnnotation` is interested in,
    /// replaces the currently registered anchor. Return `usize::MAX` to ignore
    fn reset_pos(&mut self, _char_idx: usize) -> usize {
        usize::MAX
    }

    /// Called when a text is concealed that contains an anchor registered by this `LineAnnotation`.
    /// In this case the line decorations  **must** ensure that virtual text anchored within that
    /// char range is skipped.
    ///
    /// # Returns
    ///
    /// The `char_idx` of the next anchor this `LineAnnotation` is interested in,
    /// **after the end of conceal_end_char_idx**
    /// replaces the currently registered anchor. Return `usize::MAX` to ignore
    fn skip_concealed_anchors(&mut self, conceal_end_char_idx: usize) -> usize {
        self.reset_pos(conceal_end_char_idx)
    }

    /// Process an anchor (horizontal position is provided) and returns the next anchor.
    ///
    /// # Returns
    ///
    /// The `char_idx` of the next anchor this `LineAnnotation` is interested in,
    /// replaces the currently registered anchor. Return `usize::MAX` to ignore
    fn process_anchor(&mut self, _grapheme: &FormattedGrapheme) -> usize {
        usize::MAX
    }

    /// This function is called at the end of a visual line to insert virtual text
    ///
    /// # Returns
    ///
    /// The number of additional virtual lines to reserve
    ///
    /// # Note
    ///
    /// The `line_end_visual_pos` parameter indicates the visual vertical distance
    /// from the start of block where the traversal starts.  This includes the offset
    /// from other `LineAnnotations`. This allows inline annotations to consider
    /// the height of the text and "align" two different documents (like for side
    /// by side diffs).  These annotations that want to "align" two documents should
    /// therefore be added last so that other virtual text is also considered while aligning
    fn insert_virtual_lines(
        &mut self,
        line_end_char_idx: usize,
        line_end_visual_pos: Position,
        doc_line: usize,
    ) -> Position;
}

/// 注解数据来源:借用 doc 的切片(既有调用点,零拷贝)或渲染期构造的 owned Vec
/// (插件装饰按 style 分组后注入,数据随 TextAnnotations 一起存活)
#[derive(Debug)]
/// 注释层来源:借用切片或自有 `Vec` —— 之所以是 pub,是因为它出现在 `add_inline_annotations` 的公有签名里。
pub enum AnnotationSource<'a, A> {
    Borrowed(&'a [A]),
    Owned(Vec<A>),
}

impl<A> Deref for AnnotationSource<'_, A> {
    type Target = [A];

    fn deref(&self) -> &[A] {
        match self {
            AnnotationSource::Borrowed(s) => s,
            AnnotationSource::Owned(v) => v,
        }
    }
}

impl<'a, A> From<&'a [A]> for AnnotationSource<'a, A> {
    fn from(s: &'a [A]) -> Self {
        AnnotationSource::Borrowed(s)
    }
}

impl<'a, A> From<&'a Vec<A>> for AnnotationSource<'a, A> {
    fn from(v: &'a Vec<A>) -> Self {
        AnnotationSource::Borrowed(v)
    }
}

impl<A> From<Vec<A>> for AnnotationSource<'_, A> {
    fn from(v: Vec<A>) -> Self {
        AnnotationSource::Owned(v)
    }
}

#[derive(Debug)]
struct Layer<'a, A, M> {
    annotations: AnnotationSource<'a, A>,
    current_index: Cell<usize>,
    metadata: M,
}

impl<A: Clone, M: Clone> Clone for Layer<'_, A, M> {
    fn clone(&self) -> Self {
        Layer {
            annotations: match &self.annotations {
                AnnotationSource::Borrowed(s) => AnnotationSource::Borrowed(s),
                AnnotationSource::Owned(v) => AnnotationSource::Owned(v.clone()),
            },
            current_index: self.current_index.clone(),
            metadata: self.metadata.clone(),
        }
    }
}

impl<A, M> Layer<'_, A, M> {
    pub fn reset_pos(&self, char_idx: usize, get_char_idx: impl Fn(&A) -> usize) {
        let new_index = self
            .annotations
            .partition_point(|annot| get_char_idx(annot) < char_idx);
        self.current_index.set(new_index);
    }

    pub fn consume(&self, char_idx: usize, get_char_idx: impl Fn(&A) -> usize) -> Option<&A> {
        let annot = self.annotations.get(self.current_index.get())?;
        debug_assert!(get_char_idx(annot) >= char_idx);
        if get_char_idx(annot) == char_idx {
            self.current_index.set(self.current_index.get() + 1);
            Some(annot)
        } else {
            None
        }
    }
}

impl<'a, A, M> From<(&'a [A], M)> for Layer<'a, A, M> {
    fn from((annotations, metadata): (&'a [A], M)) -> Layer<'a, A, M> {
        Layer {
            annotations: AnnotationSource::Borrowed(annotations),
            current_index: Cell::new(0),
            metadata,
        }
    }
}

impl<'a, A, M> From<(AnnotationSource<'a, A>, M)> for Layer<'a, A, M> {
    fn from((annotations, metadata): (AnnotationSource<'a, A>, M)) -> Layer<'a, A, M> {
        Layer {
            annotations,
            current_index: Cell::new(0),
            metadata,
        }
    }
}

fn reset_pos<A, M>(layers: &[Layer<A, M>], pos: usize, get_pos: impl Fn(&A) -> usize) {
    for layer in layers {
        layer.reset_pos(pos, &get_pos)
    }
}

/// Safety: We store LineAnnotation in a NonNull pointer. This is necessary to work
/// around an unfortunate inconsistency in rusts variance system that unnnecesarily
/// makes the lifetime invariant if implemented with safe code. This makes the
/// DocFormatter API very cumbersome/basically impossible to work with.
///
/// Normally object types `dyn Foo + 'a` are covariant so if we used `Box<dyn LineAnnotation + 'a>` below
/// everything would be alright. However we want to use `Cell<Box<dyn LineAnnotation + 'a>>`
/// to be able to call the mutable function on `LineAnnotation`. The problem is that
/// some types like `Cell` make all their arguments invariant. This is important for soundness
/// normally for the same reasons that `&'a mut T` is invariant over `T`
/// (see <https://doc.rust-lang.org/nomicon/subtyping.html>). However for `&'a mut` (`dyn Foo + 'b`)
/// there is a specical rule in the language to make `'b` covariant (otherwise trait objects would be
/// super annoying to use). See  <https://users.rust-lang.org/t/solved-variance-of-dyn-trait-a> for
/// why this is sound. Sadly that rule doesn't apply to `Cell<Box<(dyn Foo + 'a)>`
/// (or other invariant types like `UnsafeCell` or `*mut (dyn Foo + 'a)`).
///
/// We sidestep the problem by using `NonNull` which is covariant. In the
/// special case of trait objects this is sound (easily checked by adding a
/// `PhantomData<&'a mut Foo + 'a)>` field). We don't need an explicit `Cell`
/// type here because we never hand out any refereces to the trait objects. That
/// means any reference to the pointer can create a valid multable reference
/// that is covariant over `'a` (or in other words it's a raw pointer, as long as
/// we don't hand out references we are free to do whatever we want).
struct RawBox<T: ?Sized>(NonNull<T>);

impl<T: ?Sized> RawBox<T> {
    /// Safety: Only a single mutable reference
    /// created by this function may exist at a given time.
    #[allow(clippy::mut_from_ref)]
    unsafe fn get(&self) -> &mut T {
        &mut *self.0.as_ptr()
    }
}
impl<T: ?Sized> From<Box<T>> for RawBox<T> {
    fn from(box_: Box<T>) -> Self {
        // obviously safe because Box::into_raw never returns null
        unsafe { Self(NonNull::new_unchecked(Box::into_raw(box_))) }
    }
}

impl<T: ?Sized> Drop for RawBox<T> {
    fn drop(&mut self) {
        unsafe { drop(Box::from_raw(self.0.as_ptr())) }
    }
}

/// Annotations that change that is displayed when the document is render.
/// Also commonly called virtual text.
#[derive(Default)]
pub struct TextAnnotations<'a> {
    inline_annotations: Vec<Layer<'a, InlineAnnotation, Option<Highlight>>>,
    overlays: Vec<Layer<'a, Overlay, Option<Highlight>>>,
    line_annotations: Vec<(Cell<usize>, RawBox<dyn LineAnnotation + 'a>)>,
    /// 要**跳过不画**的 char 区间（折叠用；后续其它"隐藏"来源也走这里）。
    /// 已排序、已合并（见 `set_concealed`）。
    ///
    /// 为什么不另设一条通道：`TextAnnotations` 是 `DocumentFormatter` 唯一的外部输入
    /// （`helix-core/src/doc_formatter.rs`），隐藏放这里 ⇒ 渲染、`visual_offset_from_*`、
    /// `char_idx_at_visual_offset`（滚动/点击/`:goto`）自动看到同一个世界，
    /// 不会出现"屏幕是折后的、坐标系是折前的"。
    concealed: Vec<Range<usize>>,
}

impl Debug for TextAnnotations<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextAnnotations")
            .field("inline_annotations", &self.inline_annotations)
            .field("overlays", &self.overlays)
            .finish_non_exhaustive()
    }
}

impl<'a> TextAnnotations<'a> {
    /// Prepare the TextAnnotations for iteration starting at char_idx
    pub fn reset_pos(&self, char_idx: usize) {
        reset_pos(&self.inline_annotations, char_idx, |annot| annot.char_idx);
        reset_pos(&self.overlays, char_idx, |annot| annot.char_idx);
        for (next_anchor, layer) in &self.line_annotations {
            next_anchor.set(unsafe { layer.get().reset_pos(char_idx) });
        }
    }

    pub fn collect_overlay_highlights(&self, char_range: Range<usize>) -> OverlayHighlights {
        let mut highlights = Vec::new();
        self.reset_pos(char_range.start);
        for char_idx in char_range {
            if let Some((_, Some(highlight))) = self.overlay_at(char_idx) {
                // we don't know the number of chars the original grapheme takes
                // however it doesn't matter as highlight boundaries are automatically
                // aligned to grapheme boundaries in the rendering code
                highlights.push((highlight, char_idx..char_idx + 1));
            }
        }

        OverlayHighlights::Heterogenous { highlights }
    }

    /// Add new inline annotations.
    ///
    /// The annotations grapheme will be rendered with `highlight`
    /// patched on top of `ui.text`.
    ///
    /// The annotations **must be sorted** by their `char_idx`.
    /// Multiple annotations with the same `char_idx` are allowed,
    /// they will be display in the order that they are present in the layer.
    ///
    /// If multiple layers contain annotations at the same position
    /// the annotations that belong to the layers added first will be shown first.
    pub fn add_inline_annotations(
        &mut self,
        layer: impl Into<AnnotationSource<'a, InlineAnnotation>>,
        highlight: Option<Highlight>,
    ) -> &mut Self {
        let layer = layer.into();
        if !layer.is_empty() {
            self.inline_annotations.push((layer, highlight).into());
        }
        self
    }

    /// Add new grapheme overlays.
    ///
    /// The overlaid grapheme will be rendered with `highlight`
    /// patched on top of `ui.text`.
    ///
    /// The overlays **must be sorted** by their `char_idx`.
    /// Multiple overlays with the same `char_idx` **are allowed**.
    ///
    /// If multiple layers contain overlay at the same position
    /// the overlay from the layer added last will be show.
    pub fn add_overlay(&mut self, layer: &'a [Overlay], highlight: Option<Highlight>) -> &mut Self {
        if !layer.is_empty() {
            self.overlays.push((layer, highlight).into());
        }
        self
    }

    /// Add new annotation lines.
    ///
    /// The line annotations **must be sorted** by their `char_idx`.
    /// Multiple line annotations with the same `char_idx` **are not allowed**.
    pub fn add_line_annotation(&mut self, layer: Box<dyn LineAnnotation + 'a>) -> &mut Self {
        self.line_annotations
            .push((Cell::new(usize::MAX), layer.into()));
        self
    }

    /// Removes all line annotations, useful for vertical motions
    /// so that virtual text lines are automatically skipped.
    pub fn clear_line_annotations(&mut self) {
        self.line_annotations.clear();
    }

    /// 设置要**隐藏**的 char 区间（折叠）。
    ///
    /// 接受乱序 / 重叠 / 相邻的区间，内部排序并合并；空迭代器 = 取消隐藏。
    ///
    /// **行对齐由调用方保证**：折叠区间从「头行行尾的换行符」开始、
    /// 到「折叠块最后一个换行符」之前结束（保留最后一个换行 ⇒ 头行独占一行，
    /// 下一个可见行从新行开始）。区间不对齐行边界时行为仍然自洽，
    /// 但会把两行拼到同一显示行上。
    pub fn set_concealed(&mut self, ranges: impl IntoIterator<Item = Range<usize>>) -> &mut Self {
        let mut ranges: Vec<Range<usize>> = ranges
            .into_iter()
            .filter(|range| range.start < range.end)
            .collect();
        ranges.sort_by_key(|range| range.start);
        // 合并重叠与相邻（`end == next.start`）：合并后每个位置只做一次二分，
        // 且 `concealed_end` 对"连续跳"能一次跳到底。
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for range in ranges {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        self.concealed = merged;
        self
    }

    /// 是否存在隐藏区间（formatter 的快路径）。
    pub fn has_concealed(&self) -> bool {
        !self.concealed.is_empty()
    }

    /// 隐藏区间（已排序合并；给测试与调试看）。
    pub fn concealed(&self) -> &[Range<usize>] {
        &self.concealed
    }

    /// `char_idx` 落在隐藏区间里 ⇒ 返回该区间结束位置（连续隐藏区的末尾）。
    ///
    /// formatter 据此"一次跳到底"：`while let Some(end) = concealed_end(pos)`。
    pub(crate) fn concealed_end(&self, char_idx: usize) -> Option<usize> {
        if self.concealed.is_empty() {
            return None;
        }
        // 第一个 `end > char_idx` 的区间，若它 `start <= char_idx` 则命中。
        let idx = self
            .concealed
            .partition_point(|range| range.end <= char_idx);
        self.concealed
            .get(idx)
            .filter(|range| range.start <= char_idx)
            .map(|range| range.end)
    }

    pub(crate) fn next_inline_annotation_at(
        &self,
        char_idx: usize,
    ) -> Option<(&InlineAnnotation, Option<Highlight>)> {
        self.inline_annotations.iter().find_map(|layer| {
            let annotation = layer.consume(char_idx, |annot| annot.char_idx)?;
            Some((annotation, layer.metadata))
        })
    }

    pub(crate) fn overlay_at(&self, char_idx: usize) -> Option<(&Overlay, Option<Highlight>)> {
        let mut overlay = None;
        for layer in &self.overlays {
            while let Some(new_overlay) = layer.consume(char_idx, |annot| annot.char_idx) {
                overlay = Some((new_overlay, layer.metadata));
            }
        }
        overlay
    }

    pub(crate) fn process_virtual_text_anchors(&self, grapheme: &FormattedGrapheme) {
        for (next_anchor, layer) in &self.line_annotations {
            loop {
                match next_anchor.get().cmp(&grapheme.char_idx) {
                    Ordering::Less => next_anchor
                        .set(unsafe { layer.get().skip_concealed_anchors(grapheme.char_idx) }),
                    Ordering::Equal => {
                        next_anchor.set(unsafe { layer.get().process_anchor(grapheme) })
                    }
                    Ordering::Greater => break,
                };
            }
        }
    }

    pub(crate) fn virtual_lines_at(
        &self,
        char_idx: usize,
        line_end_visual_pos: Position,
        doc_line: usize,
    ) -> usize {
        let mut virt_off = Position::new(0, 0);
        for (_, layer) in &self.line_annotations {
            virt_off += unsafe {
                layer
                    .get()
                    .insert_virtual_lines(char_idx, line_end_visual_pos + virt_off, doc_line)
            };
        }
        virt_off.row
    }
}

#[cfg(test)]
mod concealed_tests {
    use super::TextAnnotations;

    /// 空集：任何查询都不命中，`has_concealed` 为 false（formatter 的快路径依据）。
    #[test]
    fn empty_conceal_never_hits() {
        let annot = TextAnnotations::default();
        assert!(!annot.has_concealed());
        assert_eq!(annot.concealed_end(0), None);
        assert_eq!(annot.concealed_end(usize::MAX), None);
    }

    /// 区间前 / 区间内 / 区间后（含边界）：只有 `[start, end)` 内的位置命中。
    #[test]
    fn concealed_end_bounds_are_half_open() {
        let mut annot = TextAnnotations::default();
        annot.set_concealed([10..20]);
        assert!(annot.has_concealed());
        assert_eq!(annot.concealed_end(9), None, "区间前");
        assert_eq!(annot.concealed_end(10), Some(20), "区间首字符命中");
        assert_eq!(annot.concealed_end(19), Some(20), "区间末字符前一格命中");
        assert_eq!(annot.concealed_end(20), None, "end 是开的");
        assert_eq!(annot.concealed_end(21), None, "区间后");
    }

    /// 乱序 + 重叠 + 相邻：统一排序合并，且合并后能一次跳到连续区的末尾。
    #[test]
    fn set_concealed_sorts_and_merges() {
        let mut annot = TextAnnotations::default();
        annot.set_concealed([30..40, 0..5, 3..8, 8..12, 50..50]);
        assert_eq!(
            annot.concealed().to_vec(),
            vec![0..12, 30..40],
            "相邻也算连续"
        );
        assert_eq!(annot.concealed_end(4), Some(12));
        assert_eq!(annot.concealed_end(11), Some(12));
        assert_eq!(annot.concealed_end(12), None);
        assert_eq!(annot.concealed_end(35), Some(40));
    }

    /// 重设会整体替换（每帧重建的 `TextAnnotations` 用不到，但 `:config-reload` 类路径要确定）。
    #[test]
    fn set_concealed_replaces_previous() {
        let mut annot = TextAnnotations::default();
        annot.set_concealed([0..10]);
        annot.set_concealed([100..110]);
        assert_eq!(annot.concealed().to_vec(), vec![100..110]);
        assert_eq!(annot.concealed_end(5), None);
        annot.set_concealed([]);
        assert!(!annot.has_concealed(), "空迭代器 = 取消隐藏");
    }
}
