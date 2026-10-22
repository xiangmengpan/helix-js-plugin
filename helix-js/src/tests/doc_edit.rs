//! `doc_edit` 域测试。

use super::*;

/// `open_file` 的 `scratch` 选项:带它走 `OpenScratchFile`(不绑定路径),不带走原请求。
/// 这条路由是 `:tutor` 搬迁的**安全前提** —— 走错了,用户 `:w` 会覆盖 runtime 里的原文件。
#[test]
fn open_file_scratch_routes_to_scratch_request() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("ofs", () => {
        helix.open_file("/tmp/plain.txt");
        helix.open_file("/tmp/scratch.txt", { scratch: true });
    });
    "#,
    )
    .unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(run_command("ofs", &ctx).unwrap());
    let reqs = take_ui_requests();
    assert!(
        matches!(&reqs[0], UiRequest::OpenFile(OpenFileReq { path, .. }) if path == "/tmp/plain.txt"),
        "不带 scratch → 原 OpenFile 请求"
    );
    assert!(
        matches!(&reqs[1], UiRequest::OpenScratchFile(OpenScratchFileReq { path }) if path == "/tmp/scratch.txt"),
        "带 scratch → OpenScratchFile(不绑定路径)"
    );
}

#[test]
fn doc_edits_queue() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("edit", (ctx) => {
        ctx.doc.insert(1, 2, "ab");
        ctx.doc.replace(0, 0, 0, 5, "new");
        ctx.doc.delete(3, 0, 4, 0);
    });
    "#,
    )
    .unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (1, 2),
        selection: ((0, 0), (0, 0)),
    };
    run_command("edit", &ctx).unwrap();
    let edits = take_edits();
    assert_eq!(
        edits,
        vec![
            Edit {
                doc: None,
                start: (1, 2),
                end: (1, 2),
                insert: "ab".into()
            },
            Edit {
                doc: None,
                start: (0, 0),
                end: (0, 5),
                insert: "new".into()
            },
            Edit {
                doc: None,
                start: (3, 0),
                end: (4, 0),
                insert: String::new()
            },
        ]
    );
    // take_edits 清空
    assert!(take_edits().is_empty());
}

#[test]
fn doc_edit_validation() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 类型错误 → 命令失败（run_command 返回 Err），且队列被清空
    load_script(
        r#"
    helix.register_command("bad1", (ctx) => { ctx.doc.insert("x", 0, "a"); });
    helix.register_command("bad2", (ctx) => { ctx.doc.replace(0, 0, 0, 0, 42); });
    helix.register_command("bad3", (ctx) => { ctx.doc.delete(0, 0, 0); });
    "#,
    )
    .unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(run_command("bad1", &ctx).is_err());
    assert!(take_edits().is_empty());
    assert!(run_command("bad2", &ctx).is_err());
    assert!(run_command("bad3", &ctx).is_err());
    assert!(take_edits().is_empty());
    // 正常命令运行后队列仍有值（供 helix-term 消费）
    load_script(r#"helix.register_command("ok", (ctx) => { ctx.doc.insert(0, 0, "z"); });"#)
        .unwrap();
    run_command("ok", &ctx).unwrap();
    assert_eq!(take_edits().len(), 1);
}

#[test]
fn by_path_reads_other_buffer() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("read-other", () => {
        const d = helix.by_path("/tmp/other.rs");
        if (d === null) throw new Error("null");
        if (d.path !== "/tmp/other.rs") throw new Error("path mismatch");
        if (d.text !== "other text") throw new Error("text mismatch");
    });
    "#,
    )
    .unwrap();
    let ctx = CommandContext {
        path: Some("/tmp/current.rs".into()),
        text: "current".into(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: vec![DocSnapshot {
            path: "/tmp/other.rs".into(),
            text: "other text".into(),
        }],
    };
    run_command("read-other", &ctx).unwrap();
}

#[test]
fn by_path_edits_target_other_buffer() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("edit-other", () => {
        const d = helix.by_path("/tmp/other.rs");
        if (d === null) throw new Error("null");
        d.insert(0, 0, "X");
    });
    helix.register_command("edit-current", (ctx) => {
        ctx.doc.insert(0, 0, "Y");
    });
    "#,
    )
    .unwrap();
    let ctx = CommandContext {
        path: Some("/tmp/current.rs".into()),
        text: "current".into(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: vec![DocSnapshot {
            path: "/tmp/other.rs".into(),
            text: "other text".into(),
        }],
    };
    run_command("edit-other", &ctx).unwrap();
    let edits = take_edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].doc.as_deref(), Some("/tmp/other.rs"));
    run_command("edit-current", &ctx).unwrap();
    let edits = take_edits();
    assert_eq!(edits[0].doc, None, "ctx.doc 编辑仍指向当前 buffer");
}

#[test]
fn doc_change_event() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.on("doc-change", (doc) => { helix.echo("changed:" + doc.cursor.row); });
    "#,
    )
    .unwrap();
    assert!(has_handlers("doc-change"));
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: "x".into(),
        cursor: (2, 0),
        selection: ((0, 0), (0, 0)),
    };
    emit_event("doc-change", &ctx, None).unwrap();
    assert_eq!(take_messages(), vec!["changed:2"]);
    // 未注册的事件名仍然报错
    assert!(load_script(r#"helix.on("bogus", () => {});"#).is_err());
}

#[test]
fn doc_change_changes_merged_range() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.on("doc-change", (doc) => {
        if (doc.changes.length === 0) { helix.echo("empty"); return; }
        const c = doc.changes[0];
        helix.echo(c.oldRange.start.row + "," + c.oldRange.start.col + "-" + c.oldRange.end.row + "," + c.oldRange.end.col + "|" + c.newRange.start.row + "," + c.newRange.start.col + "-" + c.newRange.end.row + "," + c.newRange.end.col);
    });
    "#,
    )
    .unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: "hello world".into(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    // 两次插入:a 在 char0、b 在 char1 → 合并 old = (0,0)-(1,1),new = (0,0)-(1,2);行列按当前文本换算
    let changes = vec![((0, 0), (0, 1)), ((1, 1), (1, 2))];
    emit_doc_change(&ctx, &changes).unwrap();
    assert_eq!(take_messages(), vec!["0,0-0,1|0,0-0,2"]);
    // 无变更 → changes 为空数组
    emit_doc_change(&ctx, &[]).unwrap();
    assert_eq!(take_messages(), vec!["empty"]);
}

#[test]
fn selection_and_cursor() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("selcmd", (ctx) => {
        helix.set_cursor(2, 3);
        helix.set_selection(0, 1, 0, 5);
        helix.echo("sel:" + ctx.selection.anchor.row + "," + ctx.selection.anchor.col + "-" + ctx.selection.head.row + "," + ctx.selection.head.col);
    });
    "#,
    )
    .unwrap();
    let ctx = CommandContext {
        path: None,
        text: "abc\ndef\nghi".into(),
        cursor: (0, 0),
        selection: ((0, 1), (0, 5)),
        docs: vec![],
    };
    assert!(run_command("selcmd", &ctx).unwrap());
    let reqs = take_cursor_requests();
    assert_eq!(
        reqs,
        vec![
            CursorRequest::SetCursor { row: 2, col: 3 },
            CursorRequest::SetSelection {
                anchor: (0, 1),
                head: (0, 5)
            },
        ]
    );
    assert_eq!(take_messages(), vec!["sel:0,1-0,5"]);

    // 类型错误 → 命令失败（run_command 返回 Err），请求队列被清空
    load_script(r#"helix.register_command("badsel", (ctx) => { helix.set_cursor("x", 0); });"#)
        .unwrap();
    assert!(run_command("badsel", &ctx).is_err());
    assert!(take_cursor_requests().is_empty());
}

#[test]
fn set_selection_array() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 数组形态 → SetSelections 入队
    load_script(
        r#"
    helix.set_selection([{ anchor: {row: 0, col: 0}, head: {row: 0, col: 2} },
                         { anchor: {row: 2, col: 1}, head: {row: 2, col: 4} }]);
    "#,
    )
    .unwrap();
    let reqs = take_cursor_requests();
    assert!(matches!(&reqs[0], CursorRequest::SetSelections(v) if v.len() == 2));
    // 空数组 → JS 报错
    assert!(load_script(r#"helix.set_selection([]);"#).is_err());
    // 缺字段 → JS 报错
    assert!(load_script(r#"helix.set_selection([{ anchor: {row:0,col:0} }]);"#).is_err());
    // 4 参兼容
    load_script(r#"helix.set_selection(0, 0, 1, 1);"#).unwrap();
    let reqs = take_cursor_requests();
    assert!(matches!(&reqs[0], CursorRequest::SetSelection { .. }));
}

/// 批量编辑事务:begin_edit 后编辑积压(take_edits 返回空),end_edit 后一次取走。
/// Rust 侧直接驱动(简报备选方案):编辑队列用 with_edits 注入,begin/end 走 JS API。
#[test]
fn batch_edit_transaction() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 触发 builder 注册 begin_edit/end_edit
    load_script(r#"helix.register_command("noop", () => {});"#).unwrap();

    // 1. 未开启事务:编辑入队后可取走
    crate::state::with_edits_current(|e: &mut Vec<crate::types::Edit>| {
        e.push(crate::types::Edit {
            doc: None,
            start: (0, 0),
            end: (0, 0),
            insert: "a".into(),
        })
    });
    assert_eq!(take_edits().len(), 1);

    // 2. begin 后编辑入队,take_edits 返回空(积压)
    load_script("helix.begin_edit();").unwrap();
    crate::state::with_edits_current(|e: &mut Vec<crate::types::Edit>| {
        e.push(crate::types::Edit {
            doc: None,
            start: (0, 0),
            end: (0, 0),
            insert: "b".into(),
        })
    });
    assert!(take_edits().is_empty(), "txn open should hold edits");

    // 3. end 后 take_edits 取到积压
    load_script("helix.end_edit();").unwrap();
    let edits = take_edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].insert, "b");

    // 4. 命令内 begin/end 包住编辑:命令返回后一次取走
    load_script(
        r#"
    helix.register_command("be", (ctx) => {
        helix.begin_edit();
        ctx.doc.insert(0, 0, "c");
        helix.end_edit();
    });
    "#,
    )
    .unwrap();
    let ctx = crate::CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(run_command("be", &ctx).unwrap());
    let edits = take_edits();
    assert_eq!(edits.len(), 1, "begin/end 包住的编辑合并取走");
    assert_eq!(edits[0].insert, "c");

    // 5. 嵌套 begin/end:深度计数,内层 end 不释放外层
    load_script("helix.begin_edit(); helix.begin_edit();").unwrap();
    crate::state::with_edits_current(|e: &mut Vec<crate::types::Edit>| {
        e.push(crate::types::Edit {
            doc: None,
            start: (0, 0),
            end: (0, 0),
            insert: "d".into(),
        })
    });
    assert!(take_edits().is_empty(), "嵌套:深度 2 仍积压");
    load_script("helix.end_edit();").unwrap(); // 深度 2→1
    assert!(take_edits().is_empty(), "嵌套:外层事务仍开启");
    load_script("helix.end_edit();").unwrap(); // 深度 1→0
    let edits = take_edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].insert, "d");

    // 6. 多余 end:深度 0 饱和,不吞后续编辑
    load_script("helix.end_edit(); helix.end_edit(); helix.end_edit();").unwrap();
    crate::state::with_edits_current(|e: &mut Vec<crate::types::Edit>| {
        e.push(crate::types::Edit {
            doc: None,
            start: (0, 0),
            end: (0, 0),
            insert: "e".into(),
        })
    });
    let edits = take_edits();
    assert_eq!(edits.len(), 1, "多余 end 后编辑正常取走");
    assert_eq!(edits[0].insert, "e");

    // 7. begin 不 end(未配对):编辑积压;下一命令入口复位深度+清队列,编辑丢弃不吞后续
    load_script(
        r#"
    helix.register_command("leak", (ctx) => {
        helix.begin_edit();
        ctx.doc.insert(0, 0, "f");
    });
    "#,
    )
    .unwrap();
    assert!(run_command("leak", &ctx).unwrap());
    assert!(take_edits().is_empty(), "未配对事务:编辑积压");
    // 下一命令入口应复位 txn 深度并清队列(丢弃积压编辑)
    assert!(run_command("noop", &ctx).unwrap());
    assert_eq!(
        crate::state::with_txn_depth_current(|d: &mut usize| *d),
        0,
        "命令入口应复位 txn 深度"
    );
    crate::state::with_edits_current(|e: &mut Vec<crate::types::Edit>| {
        e.push(crate::types::Edit {
            doc: None,
            start: (0, 0),
            end: (0, 0),
            insert: "g".into(),
        })
    });
    let edits = take_edits();
    assert_eq!(edits.len(), 1, "复位后编辑正常取走");
    assert_eq!(edits[0].insert, "g");
}
