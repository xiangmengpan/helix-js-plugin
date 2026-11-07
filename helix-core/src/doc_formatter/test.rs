use crate::doc_formatter::{DocumentFormatter, TextFormat};
use crate::text_annotations::{InlineAnnotation, Overlay, TextAnnotations};

impl TextFormat {
    fn new_test(softwrap: bool) -> Self {
        TextFormat {
            soft_wrap: softwrap,
            tab_width: 2,
            max_wrap: 3,
            max_indent_retain: 4,
            wrap_indicator: ".".into(),
            wrap_indicator_highlight: None,
            // use a prime number to allow lining up too often with repeat
            viewport_width: 17,
            soft_wrap_at_text_width: false,
        }
    }
}

impl<'t> DocumentFormatter<'t> {
    fn collect_to_str(&mut self) -> String {
        use std::fmt::Write;
        let mut res = String::new();
        let viewport_width = self.text_fmt.viewport_width;
        let soft_wrap_at_text_width = self.text_fmt.soft_wrap_at_text_width;
        let mut line = 0;

        for grapheme in self {
            if grapheme.visual_pos.row != line {
                line += 1;
                assert_eq!(grapheme.visual_pos.row, line);
                write!(res, "\n{}", ".".repeat(grapheme.visual_pos.col)).unwrap();
            }
            if !soft_wrap_at_text_width {
                assert!(
                    grapheme.visual_pos.col <= viewport_width as usize,
                    "softwrapped failed {}<={viewport_width}",
                    grapheme.visual_pos.col
                );
            }
            write!(res, "{}", grapheme.raw).unwrap();
        }

        res
    }
}

fn softwrap_text(text: &str) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(true),
        &TextAnnotations::default(),
        0,
    )
    .collect_to_str()
}

#[test]
fn basic_softwrap() {
    assert_eq!(
        softwrap_text(&"foo ".repeat(10)),
        "foo foo foo foo \n.foo foo foo foo \n.foo foo  "
    );
    assert_eq!(
        softwrap_text(&"fooo ".repeat(10)),
        "fooo fooo fooo \n.fooo fooo fooo \n.fooo fooo fooo \n.fooo  "
    );

    // check that we don't wrap unnecessarily
    assert_eq!(softwrap_text("\t\txxxx1xxxx2xx\n"), "    xxxx1xxxx2xx \n ");
}

#[test]
fn softwrap_indentation() {
    assert_eq!(
        softwrap_text("\t\tfoo1 foo2 foo3 foo4 foo5 foo6\n"),
        "    foo1 foo2 \n.....foo3 foo4 \n.....foo5 foo6 \n "
    );
    assert_eq!(
        softwrap_text("\t\t\tfoo1 foo2 foo3 foo4 foo5 foo6\n"),
        "      foo1 foo2 \n.foo3 foo4 foo5 \n.foo6 \n "
    );
}

#[test]
fn long_word_softwrap() {
    assert_eq!(
        softwrap_text("\t\txxxx1xxxx2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxxx2xxx\n.....x3xxxx4xxxx5\n.....xxxx6xxxx7xx\n.....xx8xxxx9xxx \n "
    );
    assert_eq!(
        softwrap_text("xxxxxxxx1xxxx2xxx\n"),
        "xxxxxxxx1xxxx2xxx\n. \n "
    );
    assert_eq!(
        softwrap_text("\t\txxxx1xxxx 2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxxx \n.....2xxxx3xxxx4x\n.....xxx5xxxx6xxx\n.....x7xxxx8xxxx9\n.....xxx \n "
    );
    assert_eq!(
        softwrap_text("\t\txxxx1xxx 2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxx 2xxx\n.....x3xxxx4xxxx5\n.....xxxx6xxxx7xx\n.....xx8xxxx9xxx \n "
    );
}

#[test]
fn softwrap_multichar_grapheme() {
    assert_eq!(
        softwrap_text("xxxx xxxx xxx a\u{0301}bc\n"),
        "xxxx xxxx xxx \n.ábc \n "
    )
}

fn softwrap_text_at_text_width(text: &str) -> String {
    let mut text_fmt = TextFormat::new_test(true);
    text_fmt.soft_wrap_at_text_width = true;
    let annotations = TextAnnotations::default();
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &text_fmt, &annotations, 0);
    formatter.collect_to_str()
}
#[test]
fn long_word_softwrap_text_width() {
    assert_eq!(
        softwrap_text_at_text_width("xxxxxxxx1xxxx2xxx\nxxxxxxxx1xxxx2xxx"),
        "xxxxxxxx1xxxx2xxx \nxxxxxxxx1xxxx2xxx "
    );
}

fn overlay_text(text: &str, char_pos: usize, softwrap: bool, overlays: &[Overlay]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_overlay(overlays, None),
        char_pos,
    )
    .collect_to_str()
}

#[test]
fn overlay() {
    assert_eq!(
        overlay_text(
            "foobar",
            0,
            false,
            &[Overlay::new(0, "X"), Overlay::new(2, "\t")],
        ),
        "Xo  bar "
    );
    assert_eq!(
        overlay_text(
            &"foo ".repeat(10),
            0,
            true,
            &[
                Overlay::new(2, "\t"),
                Overlay::new(5, "\t"),
                Overlay::new(16, "X"),
            ]
        ),
        "fo   f  o foo \n.foo Xoo foo foo \n.foo foo foo  "
    );
}

fn annotate_text(text: &str, softwrap: bool, annotations: &[InlineAnnotation]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_inline_annotations(annotations, None),
        0,
    )
    .collect_to_str()
}

#[test]
fn annotation() {
    assert_eq!(
        annotate_text("bar", false, &[InlineAnnotation::new(0, "foo")]),
        "foobar "
    );
    assert_eq!(
        annotate_text(
            &"foo ".repeat(10),
            true,
            &[InlineAnnotation::new(0, "foo ")]
        ),
        "foo foo foo foo \n.foo foo foo foo \n.foo foo foo  "
    );
}

#[test]
fn annotation_and_overlay() {
    let annotations = [InlineAnnotation {
        char_idx: 0,
        text: "fooo".into(),
    }];
    let overlay = [Overlay {
        char_idx: 0,
        grapheme: "\t".into(),
    }];
    assert_eq!(
        DocumentFormatter::new_at_prev_checkpoint(
            "bbar".into(),
            &TextFormat::new_test(false),
            TextAnnotations::default()
                .add_inline_annotations(annotations.as_slice(), None)
                .add_overlay(overlay.as_slice(), None),
            0,
        )
        .collect_to_str(),
        "fooo  bar "
    );
}

fn folded_text(text: &str, concealed: impl IntoIterator<Item = std::ops::Range<usize>>) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(false),
        TextAnnotations::default().set_concealed(concealed),
        0,
    )
    .collect_to_str()
}

fn folded_text_softwrap(
    text: &str,
    concealed: impl IntoIterator<Item = std::ops::Range<usize>>,
) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(true),
        TextAnnotations::default().set_concealed(concealed),
        0,
    )
    .collect_to_str()
}

/// 折叠的核心：隐藏行**不占屏幕行**，头行独占一行，块后那一行接在下面。
///
/// 区间约定（见 `docs/superpowers/plans/2026-11-07-code-folding.md` §3）：
/// conceal = `[头行行尾换行, 块最后一个换行)` ⇒ 最后的换行保留，行数才真的减。
#[test]
fn concealed_lines_do_not_take_a_row() {
    let text = "head\nhidden1\nhidden2\nafter\n";
    // chars: "head\n"=0..5, "hidden1\n"=5..13, "hidden2\n"=13..21, "after\n"=21..27
    assert_eq!(
        folded_text(text, []),
        "head \nhidden1 \nhidden2 \nafter \n ",
        "换行字渲染成空格（`Grapheme::Newline` 的 Display）"
    );
    assert_eq!(folded_text(text, [4..20]), "head \nafter \n ");
}

/// 折叠后行算术必须跟着变：`visual_offset_from_block` 的行号少 2。
#[test]
fn concealed_lines_shrink_visual_rows() {
    use crate::position::visual_offset_from_block;

    let text = std::borrow::Cow::Borrowed("head\nhidden1\nhidden2\nafter\n");
    let text = crate::Rope::from(text);
    let text = text.slice(..);
    let fmt = TextFormat::new_test(false);
    let after = 21; // "after" 的首字符

    let plain = TextAnnotations::default();
    assert_eq!(
        visual_offset_from_block(text, 0, after, &fmt, &plain).0.row,
        3,
        "不折叠时 after 在第 4 行（row=3）"
    );

    let mut folded = TextAnnotations::default();
    folded.set_concealed([4..20]);
    assert_eq!(
        visual_offset_from_block(text, 0, after, &fmt, &folded)
            .0
            .row,
        1,
        "折叠 2 行后 after 落到第 2 行（row=1）"
    );
}

/// 不变量：任意可见行 row ⇒ 反解出的 char 再正解回来，仍是同一行。
/// 这条测试是为了抓「屏幕是折后的、坐标系是折前的」这类看不到的裂缝。
#[test]
fn concealed_rows_round_trip() {
    use crate::position::{char_idx_at_visual_offset, visual_offset_from_block};

    let rope = crate::Rope::from("aaa\nbbbb\nccccc\ndd\neee\n");
    let text = rope.slice(..);
    let fmt = TextFormat::new_test(false);
    let mut annotations = TextAnnotations::default();
    annotations.set_concealed([4..15]);

    // 折掉 "bbbb"、"ccccc" 两行（第 1、2 行）
    for row in 0..3 {
        let (char_idx, _virtual) = char_idx_at_visual_offset(text, 0, row, 0, &fmt, &annotations);
        let back = visual_offset_from_block(text, 0, char_idx, &fmt, &annotations)
            .0
            .row;
        assert_eq!(
            back, row as usize,
            "row {row} 往返不一致（char_idx={char_idx}）"
        );
    }
}

/// soft wrap 开着时结果与关着时一致（覆盖 `word_buf` / `peeked_grapheme` 的 lookahead）。
#[test]
fn concealed_lines_with_softwrap() {
    // 关掉软折后就是两行文本；软折开着但每行都短 ⇒ 依旧两行
    assert_eq!(
        folded_text_softwrap("head\nhidden1\nhidden2\nafter\n", [4..20]),
        "head \nafter \n "
    );
}
