//! `events_lsp` 域测试。

use super::*;

#[test]
fn event_handlers() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    assert!(!has_handlers("save"));

    load_script(
        r#"
    let order = [];
    helix.on("save", (doc) => { order.push("a"); });
    helix.on("save", (doc) => { order.push("b"); });
    helix.on("mode-change", (mode, doc) => { helix.echo("mode:" + mode); });
    "#,
    )
    .unwrap();

    assert!(has_handlers("save"));
    assert!(has_handlers("mode-change"));
    assert!(!has_handlers("buffer-open"));

    // emit 带编辑队列清空 + 多处理器按注册顺序
    let ctx = CommandContext {
        docs: vec![],
        path: Some("/tmp/e.rs".into()),
        text: "x".into(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    emit_event("save", &ctx, None).unwrap();
    assert!(take_edits().is_empty());

    // 事件名白名单校验 + 回调类型校验
    assert!(load_script(r#"helix.on("bogus", () => {});"#).is_err());
    assert!(load_script(r#"helix.on("save", 42);"#).is_err());

    // mode-change 处理器带 mode 参数 + echo
    emit_event("mode-change", &ctx, Some("insert")).unwrap();
    assert_eq!(take_messages(), vec!["mode:insert"]);
}

/// B-1:事件 payload **具名 + 版本化** —— 处理器在**位置参数之后**收到一个
/// `{api_version, type, …}` 对象;doc 对象也带同样两个字段。
#[test]
fn event_payload_is_named_and_versioned() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.on("save", (doc, payload) => {
        helix.echo(
            "v=" + payload.api_version + " t=" + payload.type
            + " path=" + payload.path
            + " cur=" + payload.cursor.row + ":" + payload.cursor.col
            + " sel=" + payload.selection.anchor.row + "-" + payload.selection.head.col
            + " docv=" + doc.api_version + " doct=" + doc.type
        );
    });
    "#,
    )
    .unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: Some("/tmp/e.rs".into()),
        text: "x".into(),
        cursor: (2, 3),
        selection: ((1, 1), (4, 5)),
    };
    emit_event("save", &ctx, None).unwrap();
    assert_eq!(
        take_messages(),
        vec!["v=1 t=save path=/tmp/e.rs cur=2:3 sel=1-5 docv=1 doct=save"]
    );
}

/// B-1:`layout-change` 的 kind 在 payload 里是显式 **null**(与位置参数同语义);
/// 钩子类事件(term-*)的 payload 字段是具名的(`pty_id`/`cmd` 等)。
#[test]
fn event_payload_kind_null_and_term_hook_fields() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.on("layout-change", (kind, doc, payload) => {
        helix.echo("kind=" + kind + " pkind=" + payload.kind + " t=" + payload.type);
    });
    helix.on("term-open", (id, cmd, payload) => {
        helix.echo("id=" + id + " cmd=" + cmd + " pty=" + payload.pty_id
            + " pcmd=" + payload.cmd + " t=" + payload.type + " v=" + payload.api_version);
    });
    "#,
    )
    .unwrap();
    emit_layout_change_kind(None).unwrap(); // 不带种类
    assert_eq!(
        take_messages(),
        vec!["kind=null pkind=null t=layout-change"]
    );
    emit_term_open(7, "bash");
    assert_eq!(
        take_messages(),
        vec!["id=7 cmd=bash pty=7 pcmd=bash t=term-open v=1"]
    );
}

/// B-1(纯 Rust):线格式是**扁平等层**对象 —— `api_version`/`type` 与具名字段同层,
/// 没有嵌套的 `hdr`;camelCase 的 `oldRange`/`newRange` 与 `doc.changes` 同形。
#[test]
fn event_payload_json_is_flat_and_named() {
    let payload = EventPayload::DocChange(DocChangePayload {
        hdr: BusHeader::new("doc-change"),
        doc_id: 0,
        path: None,
        cursor: (1, 2).into(),
        selection: ((0, 0), (1, 2)).into(),
        changes: vec![EvChange {
            old_range: EvRange {
                start: (0, 0).into(),
                end: (0, 1).into(),
            },
            new_range: EvRange {
                start: (0, 0).into(),
                end: (0, 2).into(),
            },
        }],
    });
    let v = payload.to_json();
    assert_eq!(v["api_version"], API_VERSION);
    assert_eq!(v["type"], "doc-change");
    assert_eq!(payload.event_name(), "doc-change");
    assert!(v.get("hdr").is_none(), "公共头必须摊平,不能嵌套");
    assert_eq!(v["changes"][0]["oldRange"]["start"]["row"], 0);
    assert_eq!(v["changes"][0]["newRange"]["end"]["col"], 2);
    // 单事件:startup = 公共头 + open_paths(命令行让打开的东西数量)
    let startup = EventPayload::Startup(StartupPayload {
        hdr: BusHeader::new("startup"),
        open_paths: 2,
    });
    let sv = startup.to_json();
    assert_eq!(sv["type"], "startup");
    assert_eq!(sv["api_version"], API_VERSION);
    assert_eq!(sv["open_paths"], 2);
    assert_eq!(sv.as_object().unwrap().len(), 3);
}

#[test]
fn server_api_pushes_server_ops() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // install/remove 缺 arg → 报错
    assert!(load_script(r#"helix.server.install();"#).is_err());
    assert!(load_script(r#"helix.server.remove();"#).is_err());
    crate::state::take_ui_requests(); // 清空
    load_script(r#"helix.server.list();"#).unwrap();
    load_script(r#"helix.server.install("rust-analyzer");"#).unwrap();
    load_script(r#"helix.server.update();"#).unwrap(); // 无参 = 全量
    load_script(r#"helix.server.remove("black");"#).unwrap();
    load_script(r#"helix.server.status();"#).unwrap();
    let reqs = crate::state::take_ui_requests();
    let ops: Vec<(&str, Option<&str>)> = reqs
        .iter()
        .filter_map(|r| match r {
            crate::types::UiRequest::ServerOp(ServerOpReq { op, arg }) => {
                Some((op.as_str(), arg.as_deref()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        ops,
        vec![
            ("list", None),
            ("install", Some("rust-analyzer")),
            ("update", None),
            ("remove", Some("black")),
            ("status", None),
        ]
    );
}

#[test]
fn lsp_enhance_request_shapes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 清残留队列(其它测试不入队,防御)
    let _ = take_lsp_requests();
    // rename newName 非字符串 / execute 非对象 → 命令失败且不入队
    // (同步命令:参数校验错误同步抛出;async 命令会包进 promise 拒绝)
    load_script(
        r#"
    helix.register_command("ren-bad", () => { helix.lsp.rename(42); });
    helix.register_command("exec-bad", () => { helix.lsp.execute_code_action("x"); });
    "#,
    )
    .unwrap();
    let ctx = CommandContext {
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: vec![],
    };
    assert!(run_command("ren-bad", &ctx).is_err());
    assert!(run_command("exec-bad", &ctx).is_err());
    assert!(take_lsp_requests().is_empty(), "校验失败不应入队");
    // 合法调用 → 请求形态正确(同步命令:四个调用都同步入队)
    load_script(
        r#"
    helix.register_command("ren-ok", () => {
        helix.lsp.rename("newName");
        helix.lsp.format();
        helix.lsp.code_actions({ row: 1, col: 2 });
        helix.lsp.execute_code_action({ title: "fix", kind: "quickfix" });
    });
    "#,
    )
    .unwrap();
    run_command("ren-ok", &ctx).unwrap();
    let reqs = take_lsp_requests();
    assert_eq!(reqs.len(), 4);
    assert!(matches!(reqs[0].method, crate::lsp::LspMethod::Rename));
    assert_eq!(
        reqs[0].params.as_ref().and_then(|v| v.as_str()),
        Some("newName")
    );
    assert!(matches!(reqs[1].method, crate::lsp::LspMethod::Format));
    assert!(matches!(reqs[2].method, crate::lsp::LspMethod::CodeActions));
    assert_eq!(reqs[2].pos, Some((1, 2)));
    assert!(matches!(
        reqs[3].method,
        crate::lsp::LspMethod::ExecuteCodeAction
    ));
    let action = reqs[3].params.as_ref().unwrap();
    assert_eq!(action["title"], serde_json::Value::String("fix".into()));
}
