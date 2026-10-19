//! `draw_ops` 域测试:`helix.draw.*` 显示列表 → `Content::Ops` 的完整链路。
//!
//! 光栅化本身在 `crate::draw::tests`(纯函数);这里测的是 **JS → Rust 那一段**:
//! op 收集、`done()` 哨兵、`reset` 的跨帧隔离,以及 `helix.text.*` 的宽度口径。

use super::*;

fn ctx() -> CommandContext {
    CommandContext {
        docs: vec![],
        path: Some("/tmp/e.rs".into()),
        text: "x".into(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    }
}

/// 开一个弹窗,返回它的 id(取第一条 OpenPopup 请求)
fn open(render_js: &str) -> u64 {
    load_script(render_js).unwrap();
    let reqs = take_ui_requests();
    match reqs.first() {
        Some(UiRequest::OpenPopup(OpenPopupReq { id, .. })) => *id,
        other => panic!("expected OpenPopup, got {other:?}"),
    }
}

fn rows(content: &Content, w: u16, h: u16) -> Vec<String> {
    crate::draw::rasterize(
        match content {
            Content::Ops(ops) => ops,
            other => panic!("expected Content::Ops, got {other:?}"),
        },
        w,
        h,
    )
    .into_iter()
    .map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>())
    .collect()
}

/// render 里画一个带标题的矩形并 `return helix.draw.done()` ⇒ `Content::Ops`,光栅化出盒子
#[test]
fn draw_render_produces_ops() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let id = open(
        r#"
    helix.open_popup({
        render: () => {
            helix.draw.rect(0, 0, 8, 3, { border: "round", title: "hi", style: "ui.popup" });
            helix.draw.text(2, 1, "hi");
            return helix.draw.done();
        },
    });
    "#,
    );
    let content = render_popup(id, 8, 3, None).unwrap();
    let Content::Ops(ops) = &content else {
        panic!("expected Ops, got {content:?}")
    };
    assert_eq!(ops.len(), 2, "两条 op:rect + text");
    assert_eq!(
        rows(&content, 8, 3),
        vec!["╭ hi ──╮", "│ hi   │", "╰──────╯"]
    );
    // 样式跟着走到光栅化结果里(term 侧只认 scope 名)
    let line = crate::draw::rasterize(
        match &content {
            Content::Ops(o) => o,
            _ => unreachable!(),
        },
        8,
        3,
    );
    assert_eq!(line[0].spans[0].style.as_deref(), Some("ui.popup"));
    // 旧行 API 不受影响
    let _ = ctx();
}

/// 没调 `done()` 却返回了手写的 `{type:"draw"}` ⇒ 明确报错(不静默出半张图)
#[test]
fn hand_made_draw_sentinel_without_done_errors() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let id = open(
        r#"
    helix.open_popup({ render: () => ({ type: "draw" }) });
    "#,
    );
    let err = render_popup(id, 10, 3, None).unwrap_err().to_string();
    assert!(err.contains("helix.draw.done()"), "错误应指向 done():{err}");
}

/// 画了但 return 别的东西(数组)⇒ 仍按行渲染,op 被丢弃(下一次 render 也不会复活)
#[test]
fn drawing_without_done_returns_lines_and_ops_do_not_leak() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let id = open(
        r#"
    helix.open_popup({
        render: () => {
            helix.draw.rect(0, 0, 3, 3);   // 画了但不 done()
            return ["plain"];              // 认的是这个 ⇒ Lines
        },
    });
    "#,
    );
    assert_eq!(
        render_popup(id, 10, 2, None).unwrap(),
        Content::Lines(vec![StyledLine::plain("plain")])
    );
}

/// 跨帧隔离:上一帧的 op 不会被下一帧捡走(哪怕下一帧什么都没画)
#[test]
fn ops_do_not_leak_across_frames() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let id = open(
        r#"
    let n = 0;
    helix.open_popup({
        render: () => {
            n++;
            if (n === 1) helix.draw.rect(0, 0, 4, 3);
            return helix.draw.done();
        },
    });
    "#,
    );
    let first = render_popup(id, 6, 3, None).unwrap();
    assert_eq!(rows(&first, 6, 3)[0], "┌──┐");
    let second = render_popup(id, 6, 3, None).unwrap();
    let Content::Ops(ops) = &second else {
        panic!("expected Ops")
    };
    assert!(ops.is_empty(), "第二帧没画东西 ⇒ 空 op(不能捡到上一帧的)");
    assert_eq!(rows(&second, 6, 3), vec!["", "", ""]);
}

/// `fill` 两条路径:缺省铺带样式空格;给 `ch` 则逐行铺字符
#[test]
fn fill_and_hline_vline_from_js() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let id = open(
        r#"
    helix.open_popup({
        render: () => {
            helix.draw.fill(0, 0, 3, 2, { style: "ui.selection" });
            helix.draw.fill(0, 2, 3, 1, { ch: "=" });
            helix.draw.hline(0, 3, 3);
            helix.draw.vline(4, 0, 2);
            return helix.draw.done();
        },
    });
    "#,
    );
    let content = render_popup(id, 5, 4, None).unwrap();
    let Content::Ops(ops) = &content else {
        panic!("expected Ops")
    };
    // fill(空格) = 1 条 op;fill(ch) = 每行 1 条 HLine;+ hline + vline
    assert_eq!(ops.len(), 1 + 1 + 1 + 1, "ops: {ops:?}");
    assert_eq!(rows(&content, 5, 4), vec!["    │", "    │", "===", "───"]);
    // 无边框填充只有一份带样式的空格段
    let line0 = crate::draw::rasterize(
        match &content {
            Content::Ops(o) => o,
            _ => unreachable!(),
        },
        5,
        4,
    );
    assert_eq!(line0[0].spans[0].style.as_deref(), Some("ui.selection"));
}

/// `helix.text.*`:宽度口径与渲染器一致(CJK = 2 列),截断不切半个字
#[test]
fn text_helpers_width_truncate_pad() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.echo("w=" + helix.text.width("中文") + "," + helix.text.width("abc"));
    helix.echo("t=[" + helix.text.truncate("中文字", 3) + "]");
    helix.echo("p=[" + helix.text.pad("中文", 6) + "]");
    helix.echo("r=[" + helix.text.pad("ab", 4, { align: "right" }) + "]");
    "#,
    )
    .unwrap();
    assert_eq!(
        take_messages(),
        vec![
            "w=4,3",
            "t=[中]",     // 3 列装不下两个汉字(4 列)⇒ 只取一个
            "p=[中文  ]", // 4 列 + 2 空格
            "r=[  ab]",
        ]
    );
}

/// 参数形状错(opts 不是对象 / 数字是负数或 undefined)不应炸整帧
#[test]
fn bad_args_are_tolerated() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let id = open(
        r#"
    helix.open_popup({
        render: () => {
            helix.draw.rect(-5, undefined, "x", null, 42);   // 全都不合法 ⇒ 尺寸 0 的矩形
            helix.draw.text(0, 0, undefined);
            helix.draw.hline(0, 0, 2, "not-an-object");
            return helix.draw.done();
        },
    });
    "#,
    );
    let content = render_popup(id, 4, 2, None).unwrap();
    // 唯一有效的就是那条 hline(缺省字符 ─)
    assert_eq!(rows(&content, 4, 2), vec!["──", ""]);
    // 不认识的 border ⇒ 明确报错(不是静默画错)
    let id2 = open(
        r#"
    helix.open_popup({ render: () => { helix.draw.rect(0,0,2,2,{border:"wavy"}); return helix.draw.done(); } });
    "#,
    );
    let err = render_popup(id2, 4, 2, None).unwrap_err().to_string();
    assert!(err.contains("plain|round|double|thick|none"), "{err}");
}

/// A 档小步:`helix.el("box", …)` ⇒ `CompNode::Box`(边框容器,子节点照旧可聚焦)
#[test]
fn el_box_parses_into_comp_node() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let id = open(
        r#"
    helix.open_popup({
        render: () =>
            helix.el("box", [
                helix.el("button", "ok", { id: "b1" }),
                helix.el("text", "second"),
            ], { border: "double", title: "T", width: 10, height: 4, style: "ui.popup" }),
    });
    "#,
    );
    let content = render_popup(id, 20, 10, None).unwrap();
    let Content::Tree(CompNode::Box {
        border,
        title,
        width,
        height,
        style,
        children,
        ..
    }) = &content
    else {
        panic!("expected Tree(Box), got {content:?}")
    };
    assert_eq!(*border, crate::draw::BorderKind::Double);
    assert_eq!(title.len(), 1);
    assert_eq!(title[0].text, "T");
    assert_eq!((*width, *height), (Some(10), Some(4)));
    assert_eq!(style.as_deref(), Some("ui.popup"));
    assert_eq!(children.len(), 2, "button + text");
    // 边框名不认识 ⇒ 明确报错
    let id2 = open(
        r#"
    helix.open_popup({ render: () => helix.el("box", [], { border: "wavy" }) });
    "#,
    );
    let err = render_popup(id2, 10, 3, None).unwrap_err().to_string();
    assert!(err.contains("plain|round|double|thick|none"), "{err}");
}
