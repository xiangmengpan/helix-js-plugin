//! `pane_stack` 域测试。

use super::*;

/// `helix.pane.list()` 读 Rust 侧推来的 pane 清单(含浮窗)。
/// 之前 `get_layout()` 看不到浮窗,插件因此完全不知道浮窗存在。
#[test]
fn pane_list_api_reads_snapshot() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };

    // 未推送时:空数组(不报错)
    load_script(
        r#"
    helix.register_command("pl0", () => {
        helix.echo("n:" + helix.pane.list().length);
    });
    "#,
    )
    .unwrap();
    assert!(run_command("pl0", &ctx).unwrap());
    assert_eq!(take_messages()[0], "n:0");

    // 推入一份含浮窗的快照后应能读到 place/pinned/focused
    crate::state::cache_panes(
        r#"{"panes":[
            {"id":0,"kind":"EditorView","place":"tiled","focused":false,"fixed":false,"pinned":false},
            {"id":7,"kind":"PluginTerminal","place":"float","focused":true,"fixed":false,"pinned":true,"z":2,
             "rect":{"x":0.2,"y":0.2,"w":0.5,"h":0.5}}
        ]}"#,
    );
    load_script(
        r#"
    helix.register_command("pl1", () => {
        const l = helix.pane.list();
        helix.echo(
            "n:" + l.length + " p:" + l[1].place + " pin:" + l[1].pinned +
            " foc:" + l[1].focused + " tx:" + l[0].place + " k:" + l[1].kind + "/" + l[0].kind
        );
    });
    "#,
    )
    .unwrap();
    assert!(run_command("pl1", &ctx).unwrap());
    assert_eq!(
        take_messages()[0],
        "n:2 p:float pin:true foc:true tx:tiled k:PluginTerminal/EditorView"
    );
}

/// `helix.pane.float` / `helix.pane.embed` —— 平铺 ↔ 浮窗。
/// 词汇取自 zellij 插件 API 的 `float_multiple_panes` / `embed_multiple_panes`
/// (它的词汇是 float / embed),而不是自造的 `set_place`。
#[test]
fn pane_float_and_embed_push_requests() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("pf", () => {
        helix.pane.float(3);
        helix.pane.embed(4);
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
    assert!(run_command("pf", &ctx).unwrap());
    let reqs = take_ui_requests();
    assert!(
        matches!(&reqs[0], UiRequest::PaneFloat(PaneFloatReq { id }) if *id == 3),
        "pane.float(3) → PaneFloat{{id:3}}"
    );
    assert!(
        matches!(&reqs[1], UiRequest::PaneEmbed(PaneEmbedReq { id }) if *id == 4),
        "pane.embed(4) → PaneEmbed{{id:4}}"
    );
}

/// `helix.pane.*` / `helix.layout.*` —— ③ 的最终命名。
/// 这些是**同一批 UiRequest 的新名字**(旧名删除后照常工作),所以这里只验
/// 名字齐全 + 调用真的入队(各 variant 的行为已由旧名测试覆盖)。
#[test]
fn pane_namespace_is_wired() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("pn", () => {
        const need = ["list","float","embed","close","focus","focus_dir",
                      "move","resize","zoom","unzoom","minimize","equalize","fix",
                      "info","raise","pin"];
        const needBuf = ["list","current","focus"];
        const missingBuf = needBuf.filter((n) => typeof helix.buffer[n] !== "function");
        const missing = need.filter((n) => typeof helix.pane[n] !== "function").concat(missingBuf);
        helix.echo("missing:" + missing.join(","));
        helix.pane.close(5);
        helix.pane.focus(6);
        helix.echo("lg:" + typeof helix.layout.get + "/" + typeof helix.layout.restore);
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
    assert!(run_command("pn", &ctx).unwrap());
    let msgs = take_messages();
    assert_eq!(msgs[0], "missing:", "pane.* 名字必须齐全");
    assert_eq!(
        msgs[1], "lg:function/function",
        "layout.get/restore 必须存在"
    );
    let reqs = take_ui_requests();
    assert_eq!(reqs.len(), 2, "close/focus 各入队一个请求");
}

/// `helix.pane.info(id)` —— 单个 pane 信息;找不到 → null。
/// 复用 pane.list() 的同一份快照(不新增状态),所以两者必须一致。
#[test]
fn pane_info_reads_same_snapshot_as_list() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::state::cache_panes(
        r#"{"panes":[
            {"id":0,"kind":"EditorView","place":"tiled","focused":true,"fixed":false,"pinned":false},
            {"id":7,"kind":"PluginTerminal","place":"float","focused":false,"fixed":false,"pinned":true,"z":2}
        ]}"#,
    );
    crate::load_script(
        r##"
        helix.register_command("pi", () => {
            const a = helix.pane.info(7);
            helix.echo("k:" + (a ? a.kind : "null") + " place:" + (a ? a.place : "-") + " z:" + (a ? a.z : "-"));
            helix.echo("ed:" + helix.pane.info(0).kind);
            helix.echo("miss:" + helix.pane.info(999));
            helix.echo("same:" + (helix.pane.info(7).id === helix.pane.list()[1].id));
        });
        "##,
    )
    .unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(run_command("pi", &ctx).unwrap());
    let m = take_messages();
    assert_eq!(m[0], "k:PluginTerminal place:float z:2");
    assert_eq!(m[1], "ed:EditorView");
    assert_eq!(m[2], "miss:null", "找不到 → null(不是 undefined/报错)");
    assert_eq!(m[3], "same:true", "与 pane.list() 是同一份快照");

    // **复原全局缓存**:本测试注入过 panes,不清掉会让后续依赖"空缓存"的测试失败
    // (实测:不清时有 4 个无关测试挂了)
    crate::state::cache_panes(r#"{"panes":[]}"#);
}

/// `pane.raise` / `pane.pin` —— 入队对应请求;`pin` **必须显式 on**。
#[test]
fn pane_raise_and_pin_push_requests() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("pr", () => {
        helix.pane.raise(7);
        helix.pane.pin(7, true);
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
    assert!(run_command("pr", &ctx).unwrap());
    let reqs = take_ui_requests();
    assert!(
        matches!(&reqs[0], UiRequest::PaneRaise(PaneRaiseReq { id }) if *id == 7),
        "pane.raise(7) → PaneRaise{{id:7}}"
    );
    assert!(
        matches!(&reqs[1], UiRequest::PanePin(PanePinReq { id, on }) if *id == 7 && *on),
        "pane.pin(7,true) → PanePin{{id:7,on:true}}"
    );

    // 不显式给 on → 报错(而不是隐式取反:快照可能落后一帧,取反会抖动)
    assert!(
        load_script(r#"helix.pane.pin(7);"#).is_err(),
        "pin 缺 on 应当报错"
    );
}

/// `helix.stack.list()` —— 从 pane 快照分组;**按锚去重**是关键
/// (组内每个成员都会报同一份 members,不去重就会重复列出同一组)。
#[test]
fn stack_list_groups_and_dedupes_by_anchor() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::state::cache_panes(
        r#"{"panes":[
            {"id":0,"kind":"EditorView","place":"tiled","stack":[0,1]},
            {"id":1,"kind":"PluginTerminal","place":"tiled","stack":[0,1]},
            {"id":5,"kind":"PluginPanel","place":"tiled","stack":[]},
            {"id":7,"kind":"PluginTerminal","place":"float"}
        ]}"#,
    );
    crate::load_script(
        r##"
        helix.register_command("sl", () => {
            const g = helix.stack.list();
            helix.echo("n:" + g.length);
            helix.echo("a:" + g[0].anchor + " m:" + g[0].members.join(","));
            helix.echo("t:" + typeof helix.stack.list);
        });
        "##,
    )
    .unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(run_command("sl", &ctx).unwrap());
    let m = take_messages();
    assert_eq!(m[0], "n:1", "两个成员只应产出**一组**(按锚去重)");
    assert_eq!(m[1], "a:0 m:0,1", "锚 = members[0](= 当前显示的那个)");
    assert_eq!(m[2], "t:function");
    // 复原全局缓存(本测试注入过 panes;不清会让依赖"空快照"的测试挂)
    crate::state::cache_panes(r#"{"panes":[]}"#);
}

/// `helix.stack.*` —— 名字齐全 + 三个写操作各入队对应请求。
/// 注意:本套 API 只支持**2 人组**(规格 A.7 兄弟约束),所以**没有 `add`**
/// —— 一个 Split 只有两个子,组不可能多于 2 人。这条注释就是"为什么没有 add"的答案。
#[test]
fn stack_namespace_and_write_ops() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("sw", () => {
        const need = ["list","create","activate","remove"];
        const missing = need.filter((n) => typeof helix.stack[n] !== "function");
        helix.echo("missing:" + missing.join(","));
        helix.echo("add:" + typeof helix.stack.add);
        helix.stack.create(3);
        helix.stack.activate(3, 9);
        helix.stack.remove(3);
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
    assert!(run_command("sw", &ctx).unwrap());
    let m = take_messages();
    assert_eq!(m[0], "missing:", "四个名字必须齐全");
    assert_eq!(m[1], "add:undefined", "**没有 add** —— 2 人组的模型约束");
    let reqs = take_ui_requests();
    assert!(matches!(&reqs[0], UiRequest::StackCreate(StackCreateReq { id }) if *id == 3));
    assert!(
        matches!(&reqs[1], UiRequest::StackActivate(StackActivateReq { id, member }) if *id == 3 && *member == 9)
    );
    assert!(matches!(&reqs[2], UiRequest::StackRemove(StackRemoveReq { id }) if *id == 3));

    // activate 缺 member → 报错(要显式说清"显示哪个")
    assert!(
        load_script(r#"helix.stack.activate(3);"#).is_err(),
        "缺 member 应报错"
    );
}

/// `layout-change` 的参数形状**恒定**为 `(kind | null, doc)`:缺种类时第一参是显式 `null`,
/// 第二参仍是 doc(旧实现缺种类时只传 `[doc]` ⇒ `(kind)=>…` 静默拿到事件对象)。
#[test]
fn layout_change_args_are_kind_or_null_then_doc() {
    let _g = crate::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    crate::init();
    let _ = crate::take_messages(); // 清掉同线程此前的 echo,免得断言受污染
    load_script(
        r#"helix.on("layout-change", (kind, doc) => {
             const k = kind === null ? "null" : (typeof kind === "string" ? kind : "NOT-A-KIND:" + typeof kind);
             const d = (doc && typeof doc === "object" && typeof doc.text === "string") ? "doc" : "NOT-DOC:" + typeof doc;
             helix.echo("lc " + k + " " + d);
           });"#,
    )
    .unwrap();

    crate::emit_layout_change_kind(Some("split")).unwrap();
    crate::emit_layout_change_kind(None).unwrap(); // 不带种类

    let msgs = crate::take_messages();
    assert!(
        msgs.iter().any(|m| m == "lc split doc"),
        "带种类应为 (kind, doc);实得 {msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m == "lc null doc"),
        "无种类应为 (null, doc)(第二参仍是 doc);实得 {msgs:?}"
    );
}

/// buffer 遍历:cache_buffers → buffers/current_buffer 解析;focus_buffer 入队
#[test]
fn buffer_traversal_api() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 无缓存 → null/undefined
    load_script(
        r#"globalThis.__b = helix.buffer.list(); globalThis.__c = helix.buffer.current();"#,
    )
    .unwrap();
    crate::state::with_engine(|engine| {
        let v = engine
            .global_object()
            .get(JsString::from("__b"), engine)
            .unwrap();
        assert!(v.is_null(), "无缓存 buffers → null");
    });
    // 缓存写入 → buffers 数组 / current id
    crate::state::cache_buffers(
        r#"{"current":3,"buffers":[{"id":3,"path":"/a.rs","name":"a.rs","dirty":false,"language":"rust"},{"id":7,"path":null,"name":"[scratch]","dirty":true,"language":null}]}"#,
    );
    load_script(
        r#"
        globalThis.__bs = helix.buffer.list();
        globalThis.__cur = helix.buffer.current();
        helix.buffer.focus(7);
        "#,
    )
    .unwrap();
    crate::state::with_engine(|engine| {
        let arr = engine
            .global_object()
            .get(JsString::from("__bs"), engine)
            .unwrap();
        let arr_obj = arr.as_object().unwrap();
        let arr = boa_engine::object::builtins::JsArray::from_object(arr_obj.clone()).unwrap();
        let len: usize = arr.length(engine).unwrap() as usize;
        assert_eq!(len, 2, "buffers 数组长度");
        let cur = engine
            .global_object()
            .get(JsString::from("__cur"), engine)
            .unwrap();
        assert_eq!(cur.as_number().unwrap() as u64, 3, "current_buffer id");
    });
    // focus_buffer 入队 FocusBuffer
    let reqs = take_ui_requests();
    assert!(
        reqs.iter()
            .any(|r| matches!(r, UiRequest::FocusBuffer(FocusBufferReq { id: 7 }))),
        "focus_buffer 入队 FocusBuffer"
    );
}
