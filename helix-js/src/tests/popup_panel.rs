//! `popup_panel` 域测试。

use super::*;

#[test]
fn theme_overrides_api() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 清残留脏位（本测试是唯一 set_theme 的测试，防御顺序依赖）
    let _ = take_theme_dirty();
    assert!(theme_overrides().is_empty());

    // set_theme：**合并** + 置脏；字符串 = fg，对象 = {fg, bg, modifiers}
    load_script(
        r##"
    helix.set_theme({
        "ui.popup": "#ff00aa",
        "error": "red",
        "ui.window": { fg: "#112233", bg: "#445566", modifiers: ["italic", "bold"] },
        "ui.linenr": 42,
    });
    "##,
    )
    .unwrap();
    assert!(take_theme_dirty());
    let ov = theme_overrides();
    let popup = ov.get("ui.popup").unwrap();
    assert_eq!(popup.fg.as_deref(), Some("#ff00aa"));
    assert_eq!(popup.bg, None);
    assert!(popup.modifiers.is_empty(), "字符串值只有 fg");
    assert_eq!(ov.get("error").unwrap().fg.as_deref(), Some("red"));
    let win = ov.get("ui.window").unwrap();
    assert_eq!(win.fg.as_deref(), Some("#112233"));
    assert_eq!(win.bg.as_deref(), Some("#445566"));
    assert_eq!(win.modifiers, vec!["italic", "bold"]);
    assert!(!ov.contains_key("ui.linenr"), "非字符串非对象值应被忽略");

    // 再次 set_theme：**合并**而非替换 —— 没提到的 scope 原地不动
    // （以前这里是整体替换：一个插件调 set_theme 会抹掉别的插件的覆盖，详见 theme.rs 的注释）
    load_script(r#"helix.set_theme({ "error": "blue" });"#).unwrap();
    assert!(take_theme_dirty());
    let ov = theme_overrides();
    assert_eq!(
        ov.len(),
        3,
        "3 个合法 scope 都应还在（ui.linenr 是非法值，被忽略）: {ov:?}"
    );
    assert_eq!(
        ov.get("error").unwrap().fg.as_deref(),
        Some("blue"),
        "同名覆盖生效"
    );
    assert!(
        ov.contains_key("ui.popup") && ov.contains_key("ui.window"),
        "没提到的 scope 不该被清掉: {ov:?}"
    );

    // 空对象 → 清空覆盖（等价的 reset）
    load_script(r#"helix.set_theme({});"#).unwrap();
    assert!(theme_overrides().is_empty());

    // reset_theme：清空 + 置脏
    load_script(r#"helix.set_theme({ "error": "red" });"#).unwrap();
    assert!(take_theme_dirty());
    load_script(r#"helix.reset_theme();"#).unwrap();
    assert!(take_theme_dirty());
    assert!(theme_overrides().is_empty());

    // 非法参数（非对象/缺参）→ JS 报错
    assert!(load_script(r#"helix.set_theme("red");"#).is_err());
    assert!(load_script(r#"helix.set_theme();"#).is_err());
}

#[test]
fn panel_api() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    const pid = helix.open_panel({ side: "right", size: 30, render: () => ["p1", "p2"] });
    helix.echo("id:" + pid);
    helix.close_panel(pid);
    "#,
    )
    .unwrap();
    let reqs = take_ui_requests();
    // 编译器建议：matches! 守卫未用 id 绑定 → id: _（简报原文绑了 id，clippy 要求 0 告警）
    assert!(
        matches!(&reqs[0], UiRequest::OpenPanel(OpenPanelReq { id: _, side, size, .. }) if side == "right" && *size == 30)
    );
    assert!(matches!(
        &reqs[1],
        UiRequest::ClosePanel(ClosePanelReq { id: _ })
    ));
    assert!(take_messages()[0].starts_with("id:"));
    // 校验：side 白名单 / size / render
    assert!(
        load_script(r#"helix.open_panel({ side: "top", size: 10, render: () => [] });"#).is_err()
    );
    assert!(load_script(r#"helix.open_panel({ side: "right", size: 10 });"#).is_err()); // 缺 render
    assert!(
        load_script(r#"helix.open_panel({ side: "right", size: "big", render: () => [] });"#)
            .is_err()
    );
    crate::host::with_state_or_init(|st| crate::state::with_open_panels(st, |p| p.clear()));
    // 清 OPEN_PANELS，防污染后续测试
}

#[test]
fn panel_onkey() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    const pid = helix.open_panel({
        side: "right", size: 20,
        render: () => ["p"],
        onKey: (key) => { helix.echo("panel-key:" + key.name); return key.name === "Esc" ? "close" : "handled"; },
    });
    helix.echo("pid:" + pid);
    "#,
    )
    .unwrap();
    let reqs = take_ui_requests();
    // 编译器建议：matches! 未用 id 绑定 → id: _（简报原文绑了 id，clippy 要求 0 告警）
    assert!(matches!(
        &reqs[0],
        UiRequest::OpenPanel(OpenPanelReq { id: _, .. })
    ));
    // 编译器要求：单臂 match 非穷尽 → 改 let-else（与 popup_lifecycle 同款）
    let UiRequest::OpenPanel(OpenPanelReq { id, .. }) = reqs[0] else {
        unreachable!("expected OpenPanel")
    };
    assert!(take_messages()[0].starts_with("pid:"));
    // popup_key 走同一注册表：Esc → close，其他 → handled
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    let esc = PluginKey {
        name: "Esc".into(),
        shift: false,
        ctrl: false,
        alt: false,
    };
    crate::host::with_state_or_init(|st| {
        assert_eq!(
            popup_key(st, id, &esc, &ctx).unwrap(),
            PopupKeyResult::Close
        )
    });
    assert_eq!(take_messages(), vec!["panel-key:Esc"]);
    // panel_has_onkey：有 onKey → true（简报测试的补充断言）
    assert!(panel_has_onkey(id));
    // 无 onKey 的面板：false（helix-term 侧据此全 Ignore 穿透，不调 popup_key）
    load_script(r#"helix.open_panel({ side: "left", size: 10, render: () => ["x"] });"#).unwrap();
    let UiRequest::OpenPanel(OpenPanelReq { id: id2, .. }) = take_ui_requests()[0] else {
        unreachable!("expected OpenPanel")
    };
    assert!(!panel_has_onkey(id2));
    crate::host::with_state_or_init(|st| crate::state::with_open_panels(st, |p| p.clear())); // 清 OPEN_PANELS（两个面板未 close），防污染
    let _ = take_ui_requests();
}

/// `helix.popup_exists(id)`:插件自查弹层是否还活着(B2)。
///
/// 由来:JS 侧没有主动关闭入口,插件记的 popup 句柄会在层被外部移除后永久失效
/// (症状:`:arsenal` 一直回 "already open")。这条 API 是自愈的唯一手段。
#[test]
fn popup_exists_reports_live_layers() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(r#"helix.open_popup({ render: () => [{ text: "x" }] });"#).unwrap();
    let id = crate::host::with_state_or_init(|st| {
        crate::state::with_popups(st, |p| p.keys().copied().max())
    })
    .expect("open_popup 应登记一层");
    assert!(crate::popup::popup_exists(id), "刚打开的层必须存在");

    // JS 侧读到的结论必须与 Rust 一致(用 throw 让不一致变成 Result::Err)
    load_script(&format!(
        r#"if (!helix.popup_exists({id})) throw new Error("should exist");"#
    ))
    .unwrap();

    // 关闭(Rust 侧等价于 onKey 的 "close"/reload)→ 必须变 false
    crate::popup::close_popup(id).unwrap();
    assert!(!crate::popup::popup_exists(id));
    load_script(&format!(
        r#"if (helix.popup_exists({id})) throw new Error("should be gone");"#
    ))
    .unwrap();

    // 参数非法 ⇒ 明确报错,不静默给 false
    assert!(
        load_script(r#"helix.popup_exists("nope");"#).is_err(),
        "非法 id 应报错"
    );
}

/// `helix.close_popup(id)` 的 JS 侧:**入队一条 ClosePopup 请求**(宿主侧移除由集成测试覆盖)。
#[test]
fn close_popup_enqueues_request() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let _ = crate::state::take_ui_requests();
    load_script(r#"helix.close_popup(42);"#).unwrap();
    let reqs = crate::state::take_ui_requests();
    assert!(
        reqs.iter().any(|r| matches!(
            r,
            crate::types::UiRequest::ClosePopup(crate::types::ClosePopupReq { id: 42 })
        )),
        "应入队 ClosePopup{{id:42}};reqs={reqs:?}"
    );
    // 参数非法 ⇒ 报错(不静默)
    assert!(load_script(r#"helix.close_popup("x");"#).is_err());
}

#[test]
fn popup_lifecycle() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    let rendered = null;
    helix.open_popup({
        render: () => ["a", "b", "c"],
        onKey: (key) => key.name === "Down" ? "handled" : "close",
        onClose: () => helix.echo("closed:" + rendered),
    });
    "#,
    )
    .unwrap();
    let reqs = take_ui_requests();
    assert_eq!(reqs.len(), 1);
    let UiRequest::OpenPopup(OpenPopupReq { id, .. }) = reqs[0] else {
        unreachable!("expected OpenPopup")
    };
    assert_eq!(id, 1); // 自增从 1 开始

    let lines = crate::host::with_state_or_init(|st| render_popup(st, id, 40, 10, None)).unwrap();
    assert_eq!(
        lines,
        Content::Lines(vec![
            StyledLine::plain("a"),
            StyledLine::plain("b"),
            StyledLine::plain("c"),
        ])
    );

    let key = PluginKey {
        name: "Down".into(),
        shift: false,
        ctrl: false,
        alt: false,
    };
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    crate::host::with_state_or_init(|st| {
        assert_eq!(
            popup_key(st, id, &key, &ctx).unwrap(),
            PopupKeyResult::Handled
        )
    });
    let key = PluginKey {
        name: "Esc".into(),
        shift: false,
        ctrl: false,
        alt: false,
    };
    crate::host::with_state_or_init(|st| {
        assert_eq!(
            popup_key(st, id, &key, &ctx).unwrap(),
            PopupKeyResult::Close
        )
    });

    close_popup(id).unwrap();
    assert_eq!(take_messages(), vec!["closed:null"]);
    assert!(crate::host::with_state_or_init(|st| render_popup(st, id, 40, 10, None)).is_err());
    // 已关闭，注册表移除
}

#[test]
fn popup_default_keys_and_validation() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 未提供 onKey：Esc 默认关闭，其他穿透
    load_script(r#"helix.open_popup({ render: () => ["x"] });"#).unwrap();
    let id = match take_ui_requests()[0] {
        UiRequest::OpenPopup(OpenPopupReq { id, .. }) => id,
        _ => unreachable!("expected OpenPopup"),
    };
    let key = PluginKey {
        name: "Enter".into(),
        shift: false,
        ctrl: false,
        alt: false,
    };
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    crate::host::with_state_or_init(|st| {
        assert_eq!(
            popup_key(st, id, &key, &ctx).unwrap(),
            PopupKeyResult::Ignored
        )
    });
    let key = PluginKey {
        name: "Esc".into(),
        shift: false,
        ctrl: false,
        alt: false,
    };
    crate::host::with_state_or_init(|st| {
        assert_eq!(
            popup_key(st, id, &key, &ctx).unwrap(),
            PopupKeyResult::Close
        )
    });
    close_popup(id).unwrap();

    // render 非数组 → Err
    load_script(r#"helix.open_popup({ render: () => "not an array" });"#).unwrap();
    let id = match take_ui_requests()[0] {
        UiRequest::OpenPopup(OpenPopupReq { id, .. }) => id,
        _ => unreachable!("expected OpenPopup"),
    };
    assert!(crate::host::with_state_or_init(|st| render_popup(st, id, 40, 10, None)).is_err());
    close_popup(id).unwrap();

    // 参数缺失/类型错误 → JS 报错
    assert!(load_script(r#"helix.open_popup({});"#).is_err());
    assert!(load_script(r#"helix.open_popup({ render: 42 });"#).is_err());
}

#[test]
fn popup_size_position() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.open_popup({ render: () => ["x"], width: 40, height: 10, position: { row: 3, col: 4 } });
    helix.open_popup({ render: () => ["y"] });
    "#,
    )
    .unwrap();
    let reqs = take_ui_requests();
    assert_eq!(reqs.len(), 2);
    match &reqs[0] {
        UiRequest::OpenPopup(OpenPopupReq {
            width,
            height,
            position,
            ..
        }) => {
            assert_eq!(*width, Some(40));
            assert_eq!(*height, Some(10));
            assert_eq!(*position, Some((3, 4)));
        }
        other => panic!("expected OpenPopup, got {other:?}"),
    }
    match &reqs[1] {
        UiRequest::OpenPopup(OpenPopupReq {
            width,
            height,
            position,
            ..
        }) => {
            assert_eq!(*width, None);
            assert_eq!(*height, None);
            assert_eq!(*position, None);
        }
        other => panic!("expected OpenPopup, got {other:?}"),
    }
    // 非法类型
    assert!(load_script(r#"helix.open_popup({ render: () => [], width: "big" });"#).is_err());
}

#[test]
fn popup_styled_lines() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.open_popup({
        render: () => [
            { text: "err: ", style: "error" },
            "plain",
            { text: "warn" },
        ],
    });
    "#,
    )
    .unwrap();
    let id = match take_ui_requests()[0] {
        UiRequest::OpenPopup(OpenPopupReq { id, .. }) => id,
        _ => unreachable!("expected OpenPopup"),
    };
    let lines = crate::host::with_state_or_init(|st| render_popup(st, id, 40, 10, None)).unwrap();
    assert_eq!(
        lines,
        Content::Lines(vec![
            StyledLine::styled("err: ", "error"),
            StyledLine::plain("plain"),
            StyledLine::plain("warn"),
        ])
    );
    close_popup(id).unwrap();
    // 非法元素（缺 text / 非字符串非对象）→ Err
    load_script(r#"helix.open_popup({ render: () => [{ style: "error" }] });"#).unwrap();
    let id = match take_ui_requests()[0] {
        UiRequest::OpenPopup(OpenPopupReq { id, .. }) => id,
        _ => unreachable!("expected OpenPopup"),
    };
    assert!(crate::host::with_state_or_init(|st| render_popup(st, id, 40, 10, None)).is_err());
    close_popup(id).unwrap();
}

#[test]
fn popup_onkey_edits_doc() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.open_popup({
        render: () => ["a", "b"],
        onKey: (key, doc) => {
            if (key.name === "Enter") {
                doc.insert(doc.cursor.row, doc.cursor.col, "XYZ");
                return "close";
            }
            return "handled";
        },
    });
    "#,
    )
    .unwrap();
    let id = match take_ui_requests()[0] {
        UiRequest::OpenPopup(OpenPopupReq { id, .. }) => id,
        _ => unreachable!("expected OpenPopup"),
    };
    let ctx = CommandContext {
        path: Some("/tmp/p.rs".into()),
        text: "line1\nline2".into(),
        cursor: (1, 2),
        selection: ((0, 0), (0, 0)),
        docs: vec![],
    };
    let key = PluginKey {
        name: "Enter".into(),
        shift: false,
        ctrl: false,
        alt: false,
    };
    crate::host::with_state_or_init(|st| {
        assert_eq!(
            popup_key(st, id, &key, &ctx).unwrap(),
            PopupKeyResult::Close
        )
    });
    assert_eq!(
        take_edits(),
        vec![Edit {
            doc: None,
            start: (1, 2),
            end: (1, 2),
            insert: "XYZ".into()
        }]
    );
    close_popup(id).unwrap();
}

#[test]
fn buffer_icon_hook() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::host::with_state_or_init(|st| assert_eq!(bufferline_icon(st, Some("a.rs")), None)); // 未注册 → None

    load_script(
        r#"
    helix.set_buffer_icon((path) => path && path.endsWith(".rs") ? "🦀" : null);
    "#,
    )
    .unwrap();
    crate::host::with_state_or_init(|st| {
        assert_eq!(bufferline_icon(st, Some("main.rs")), Some("🦀".to_string()))
    });
    crate::host::with_state_or_init(|st| assert_eq!(bufferline_icon(st, Some("main.py")), None));
    crate::host::with_state_or_init(|st| assert_eq!(bufferline_icon(st, None), None));
}

#[test]
fn keymap_registration() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();

    // 字符串命令 → MapKey 入队
    load_script(r#"helix.map("normal", "gd", "goto-def");"#).unwrap();
    let reqs = take_ui_requests();
    assert_eq!(reqs.len(), 1);
    match &reqs[0] {
        UiRequest::MapKey(MapKeyReq {
            mode, key, command, ..
        }) => {
            assert_eq!(mode, "normal");
            assert_eq!(key, "gd");
            assert_eq!(command, "goto-def");
        }
        other => panic!("expected MapKey, got {other:?}"),
    }

    // 回调 → 注册 __mapped_N + MapKey 入队
    load_script(
        r#"
    helix.map("insert", "C-n", () => { helix.echo("cb"); });
    "#,
    )
    .unwrap();
    let reqs = take_ui_requests();
    assert_eq!(reqs.len(), 1);
    let command = match &reqs[0] {
        UiRequest::MapKey(MapKeyReq { command, .. }) => command.clone(),
        other => panic!("expected MapKey, got {other:?}"),
    };
    assert!(command.starts_with("__mapped_"), "command: {command}");
    // 注册的命令可以运行（与普通插件命令同机制）
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, &command, &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["cb"]);

    // 校验失败
    assert!(load_script(r#"helix.map("bogus", "x", "y");"#).is_err());
    assert!(load_script(r#"helix.map("normal", 42, "y");"#).is_err());
    assert!(load_script(r#"helix.map("normal", "x", 42);"#).is_err());
    assert!(load_script(r#"helix.map("normal", "", "y");"#).is_err());
}

#[test]
fn statusline_hook() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let ctx = statusline_ctx();
    crate::host::with_state_or_init(|st| assert_eq!(statusline_parts(st, &ctx), None));

    // 字符串 → 单段（style None）
    load_script(r#"helix.set_statusline((ctx) => ctx.mode + ":" + ctx.cursor.row);"#).unwrap();
    assert_eq!(
        crate::host::with_state_or_init(|st| statusline_parts(st, &ctx)),
        Some(vec![StatuslinePart {
            text: "insert:3".into(),
            style: None,
            zone: None
        }])
    );

    // 数组 → 多段（字符串项 / {text, style} 项混用；style 透传）
    load_script(
        r#"
    helix.set_statusline((ctx) => [
        { text: " N ", style: "ui.statusline.normal" },
        ctx.mode + ":" + ctx.cursor.row,
        { text: String(ctx.diagnostics_error), style: "error", zone: "right" },
    ]);
    "#,
    )
    .unwrap();
    assert_eq!(
        crate::host::with_state_or_init(|st| statusline_parts(st, &ctx)),
        Some(vec![
            StatuslinePart {
                text: " N ".into(),
                style: Some("ui.statusline.normal".into()),
                zone: None
            },
            StatuslinePart {
                text: "insert:3".into(),
                style: None,
                zone: None
            },
            StatuslinePart {
                text: "2".into(),
                style: Some("error".into()),
                zone: Some("right".into())
            },
        ])
    );

    // 返回 null → None
    load_script(r#"helix.set_statusline(() => null);"#).unwrap();
    crate::host::with_state_or_init(|st| assert_eq!(statusline_parts(st, &ctx), None));
    // 抛错 → None
    load_script(r#"helix.set_statusline(() => { throw new Error("boom"); });"#).unwrap();
    crate::host::with_state_or_init(|st| assert_eq!(statusline_parts(st, &ctx), None));
    // set_statusline(null) 清除
    load_script(r#"helix.set_statusline(null);"#).unwrap();
    crate::host::with_state_or_init(|st| assert_eq!(statusline_parts(st, &ctx), None));
    // 非法参数 → JS 报错
    assert!(load_script(r#"helix.set_statusline(42);"#).is_err());
}

/// 外部状态栏段（`register_statusline_segment` / `statusline_segment`）：
/// 注册方能给别人的状态栏加段，错语义按"拼错可见、段崩不拖整条"分工。
#[test]
fn statusline_external_segments() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let read = |name: &str| -> boa_engine::JsValue {
        crate::state::with_engine(|engine| {
            engine
                .global_object()
                .get(JsString::from(name), engine)
                .unwrap()
        })
    };

    // 注册 → 渲染方按名取；ctx 原样转交（段函数看到的字段与状态栏钩子一致）
    load_script(
        r#"
        helix.register_statusline_segment("x", (ctx) => ({ text: "X" + ctx.mode, style: "error" }));
        globalThis.__seg = JSON.stringify(helix.statusline_segment("x", { mode: "insert" }));
        "#,
    )
    .unwrap();
    assert_eq!(
        read("__seg").as_string().unwrap().to_std_string_escaped(),
        r#"{"text":"Xinsert","style":"error"}"#,
        "外部段的 text/style 原样回来"
    );

    // 名字未注册 ⇒ 抛错（渲染方据此显示 `!段名`，让拼错可见）
    load_script(
        r#"
        // 注意:核心的 API 错误是**抛出的字符串**(不是 Error 对象) ⇒ 取 e.message 会拿到
        // undefined。JS 侧同理要写 `String(e && e.message ? e.message : e)`。
        try { helix.statusline_segment("nope", {}); globalThis.__err = ""; }
        catch (e) { globalThis.__err = String(e && e.message ? e.message : e); }
        "#,
    )
    .unwrap();
    let err = read("__err").as_string().unwrap().to_std_string_escaped();
    assert!(
        err.contains("no segment registered") && err.contains("nope"),
        "未注册要抛错并点出段名: {err}"
    );

    // 段函数自己抛错 ⇒ 返回 null（每帧渲染，一个插件的 bug 不能拖掉整条状态栏）
    load_script(
        r#"
        helix.register_statusline_segment("boom", () => { throw new Error("kaboom"); });
        globalThis.__boom = helix.statusline_segment("boom", {});
        "#,
    )
    .unwrap();
    assert!(read("__boom").is_null(), "段函数抛错 ⇒ null");

    // 名字/参数校验：非法段名、非函数
    assert!(
        load_script(r#"helix.register_statusline_segment("bad name", () => null);"#).is_err(),
        "带空格的段名应报错（配置里写不出来）"
    );
    assert!(
        load_script(r#"helix.register_statusline_segment("", () => null);"#).is_err(),
        "空段名应报错"
    );
    assert!(
        load_script(r#"helix.register_statusline_segment("ok", 42);"#).is_err(),
        "非函数应报错"
    );

    // 传 null = 注销；同名重复注册 = 后者覆盖
    load_script(
        r#"
        helix.register_statusline_segment("x", () => "second");
        globalThis.__over = helix.statusline_segment("x", {});
        helix.register_statusline_segment("x", null);
        try { helix.statusline_segment("x", {}); globalThis.__gone = "no-error"; }
        catch (e) { globalThis.__gone = "error"; }
        "#,
    )
    .unwrap();
    assert_eq!(
        read("__over").as_string().unwrap().to_std_string_escaped(),
        "second",
        "同名后注册覆盖"
    );
    assert_eq!(
        read("__gone").as_string().unwrap().to_std_string_escaped(),
        "error",
        "注销后取不到"
    );

    // 属主回收：有属主的段留着，属主消失后 `unload_orphans` 摘掉
    use crate::permissions::CbKey;
    load_script(r#"helix.register_statusline_segment("orph", () => "O");"#).unwrap();
    let key = CbKey::statusline_segment("orph");
    crate::permissions::bind_callback_to(key.clone(), "some-plugin");
    let removed = crate::host::with_state_or_init(crate::commands::unload_orphans);
    assert_eq!(removed, 0, "有属主的段不该被回收");
    load_script(r#"globalThis.__orph = helix.statusline_segment("orph", {});"#).unwrap();
    assert_eq!(
        read("__orph").as_string().unwrap().to_std_string_escaped(),
        "O"
    );
    crate::permissions::forget_callback(&key);
    let removed = crate::host::with_state_or_init(crate::commands::unload_orphans);
    assert_eq!(removed, 1, "属主消失后应回收这一段");
    assert!(
        load_script(r#"helix.statusline_segment("orph", {});"#).is_err(),
        "回收后取不到"
    );
}

/// 临时任务简报的验证测试。wait 条件用 Exit（chunk 是它的先导），
/// 返回的全部事件按序 resolve（先 Chunk 后 Exit），保证通道不残留。
/// 注意：编译报错调整——简报原文 `events[0]` 按值取会 move，改为 `&events[0]`；
/// `assert!(true, ...)` 触发 clippy::assertions_on_constants，删除。
#[test]
fn node_events_and_focusables() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
        helix.open_popup({
            render: (focus) => helix.el("col", [
                helix.el("button", "run", { id: "btn1", onPress: () => helix.echo("pressed"), style: focus === "btn1" ? "error" : null }),
                helix.el("input", { id: "in1", value: "abc", onKey: (k) => helix.echo("key:" + k) }),
            ]),
        });
        "#,
    )
    .unwrap();
    let id = match &take_ui_requests()[0] {
        UiRequest::OpenPopup(OpenPopupReq { id, .. }) => *id,
        other => panic!("expected OpenPopup, got {other:?}"),
    };
    // render 传 focus → JS render(focus, ctx) 收到（focus 样式分支由 JS 处理，这里验证渲染不崩 + focusables 收集）
    let content =
        crate::host::with_state_or_init(|st| render_popup(st, id, 60, 20, Some("btn1"))).unwrap();
    let mut focusables = Vec::new();
    if let Content::Tree(node) = &content {
        focusable_node_ids(node, &mut focusables);
    }
    assert_eq!(focusables, vec!["btn1".to_string(), "in1".to_string()]);
    // 事件分发：button onPress
    crate::host::with_state_or_init(
        |st| assert!(dispatch_node_event(st, id, "btn1", None).is_ok()),
    );
    // input onKey
    crate::host::with_state_or_init(|st| {
        assert!(dispatch_node_event(st, id, "in1", Some("a")).is_ok())
    });
    let msgs = take_messages();
    assert!(msgs.contains(&"pressed".to_string()), "{msgs:?}");
    assert!(msgs.contains(&"key:a".to_string()), "{msgs:?}");
    // 未知节点 → Ok（无处理器）
    crate::host::with_state_or_init(
        |st| assert!(dispatch_node_event(st, id, "nope", None).is_ok()),
    );
    close_popup(id).unwrap();
}

/// 简报验证测试：el 构造节点、render 返回树 → Content::Tree、非法节点类型 → Err
#[test]
fn component_nodes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.open_popup({
        render: () => helix.el("col", [
            helix.el("text", "title", { style: "error" }),
            helix.el("row", [
                helix.el("text", "left"),
                helix.el("text", "right", { width: 5 }),
            ], { gap: 1 }),
            helix.el("scroll", [helix.el("text", "s1"), helix.el("text", "s2")], { height: 1 }),
        ]),
    });
    helix.register_command("tree", (ctx) => {
        const n = helix.el("text", "hello", { width: 10 });
        helix.echo("type:" + n.type + " text:" + n.text + " w:" + n.width);
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
    assert!(crate::host::with_state_or_init(|st| run_command(st, "tree", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["type:text text:hello w:10"]);

    let reqs = take_ui_requests();
    let id = match &reqs[0] {
        UiRequest::OpenPopup(OpenPopupReq { id, .. }) => *id,
        _ => unreachable!("expected OpenPopup"),
    };
    match crate::host::with_state_or_init(|st| render_popup(st, id, 40, 10, None)).unwrap() {
        Content::Tree(root) => {
            assert!(matches!(&root, CompNode::Col { .. }));
            match &root {
                CompNode::Col { children, .. } => {
                    assert_eq!(children.len(), 3);
                    assert!(
                        matches!(&children[0], CompNode::Text { spans, .. } if spans.len() == 1 && spans[0].text == "title" && spans[0].style.as_deref() == Some("error"))
                    );
                    assert!(matches!(&children[1], CompNode::Row { .. }));
                    assert!(matches!(&children[2], CompNode::Scroll { .. }));
                }
                _ => panic!(),
            }
        }
        _ => panic!("expected tree"),
    }
    close_popup(id).unwrap();
    // 非法节点类型 → Err
    load_script(r#"helix.open_popup({ render: () => ({ type: "bogus" }) });"#).unwrap();
    let id = match take_ui_requests()[0] {
        UiRequest::OpenPopup(OpenPopupReq { id, .. }) => id,
        _ => unreachable!("expected OpenPopup"),
    };
    assert!(crate::host::with_state_or_init(|st| render_popup(st, id, 40, 10, None)).is_err());
    close_popup(id).unwrap();
}

/// 布局增强任务 1 验证：el/parse_node 透传 flex/wrap/offset 新 opts（含缺省与类型错误）
#[test]
fn el_new_opts_parse() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.open_popup({
        render: () => helix.el("col", [
            helix.el("scroll", [helix.el("text", "x")], { height: 4, offset: 2 }),
            helix.el("text", "y", { flex: 2, wrap: true }),
            helix.el("text", "z"),
            helix.el("row", [helix.el("text", "a", { flex: 1 })], { gap: 1, flex: 3 }),
            helix.el("button", "run", { id: "b1", flex: 1 }),
            helix.el("input", { id: "i1", value: "v", flex: 2 }),
        ]),
    });
    "#,
    )
    .unwrap();
    let reqs = take_ui_requests();
    let id = match &reqs[0] {
        UiRequest::OpenPopup(OpenPopupReq { id, .. }) => *id,
        _ => unreachable!("expected OpenPopup"),
    };
    match crate::host::with_state_or_init(|st| render_popup(st, id, 40, 10, None)).unwrap() {
        Content::Tree(CompNode::Col { children, .. }) => {
            assert_eq!(children.len(), 6);
            assert!(matches!(
                &children[0],
                CompNode::Scroll {
                    height: 4,
                    offset: Some(2),
                    ..
                }
            ));
            assert!(matches!(
                &children[1],
                CompNode::Text {
                    flex: Some(2),
                    wrap: true,
                    ..
                }
            ));
            assert!(matches!(
                &children[2],
                CompNode::Text {
                    flex: None,
                    wrap: false,
                    ..
                }
            ));
            assert!(
                matches!(&children[3], CompNode::Row { gap: 1, flex: Some(3), children, .. }
                if matches!(&children[0], CompNode::Text { flex: Some(1), .. }))
            );
            assert!(matches!(
                &children[4],
                CompNode::Button { flex: Some(1), .. }
            ));
            assert!(matches!(
                &children[5],
                CompNode::Input { flex: Some(2), .. }
            ));
        }
        _ => panic!("expected tree"),
    }
    close_popup(id).unwrap();
    // 类型错误：flex 传字符串 → Err（el 构造时即抛）
    load_script(r#"helix.open_popup({ render: () => helix.el("text", "x", { flex: "big" }) });"#)
        .unwrap();
    let id = match take_ui_requests()[0] {
        UiRequest::OpenPopup(OpenPopupReq { id, .. }) => id,
        _ => unreachable!("expected OpenPopup"),
    };
    assert!(crate::host::with_state_or_init(|st| render_popup(st, id, 40, 10, None)).is_err());
    close_popup(id).unwrap();
    // 类型错误：wrap 传数字 → Err
    load_script(r#"helix.open_popup({ render: () => helix.el("text", "x", { wrap: 1 }) });"#)
        .unwrap();
    let id = match take_ui_requests()[0] {
        UiRequest::OpenPopup(OpenPopupReq { id, .. }) => id,
        _ => unreachable!("expected OpenPopup"),
    };
    assert!(crate::host::with_state_or_init(|st| render_popup(st, id, 40, 10, None)).is_err());
    close_popup(id).unwrap();
}

/// ③ 每帧代价(手动基准,不进 CI):
/// `cargo test -p helix-js --lib bench_picker_frame -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_picker_frame() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 模拟 picker-ui 的**真实**渲染器:30 行 × 3 列 + 每行样式读取 + 选中标记
    load_script(
        r#"
helix.set_component_render(0, function (_focus, ctx) {
  var rows = [];
  for (var i = 0; i < 30; i++) rows.push("src/file_" + i + ".rs\t12:3\tfn item_" + i);
  var w = (ctx && ctx.width) ? ctx.width : 80;
  var cur = 12;
  var out = [{ type: "text", text: "JS-DRAWN  q=abc" }];
  for (var i = 0; i < rows.length; i++) {
var cells = rows[i].split("\t");
var mark = (i === cur) ? "\u25b8 " : "  ";
var line = { type: "text", text: mark + cells.join("   ") };
if (i === cur) { var st = null; try { st = helix.get_style("ui.text.focus"); } catch (e) {} if (st) line.style = st; }
out.push(line);
  }
  return out;
});
"#,
    )
    .unwrap();
    let (w, h) = (80u16, 30u16);
    for _ in 0..30 {
        let _ = crate::host::with_state_or_init(|st| render_component(st, 0, w, h, None));
    }
    const N: u32 = 300;
    let t0 = std::time::Instant::now();
    for _ in 0..N {
        let c = crate::host::with_state_or_init(|st| render_component(st, 0, w, h, None)).unwrap();
        std::hint::black_box(&c);
    }
    let el = t0.elapsed();
    let us = el.as_micros() as f64 / N as f64;
    println!(
        "\n③ 每帧 **JS 渲染** 代价: {us:.1} µs/frame ({N} 次, 总 {el:?})\n   载荷: 30 行 × 3 列 + 每行样式读取 + 选中标记\n   ⇒ 60fps 预算 16667 µs ⇒ 占比 {:.3}%\n   ⇒ 裁决: **高于\"免费\"**(约 3 ms/帧/个可见 picker)⇒ 保持`按实例注册`为默认策略,\n             `默认渲染器(id=0)`仅在**明确需要**时启用 ✓(数字见 docs/plugin-layout.md)\n",
        us / 16667.0 * 100.0
    );
    // 只做"灾难级回归"的哨兵(实测约 3.3 ms;> 20 ms 才算异常)⇒ 避免把基准变成红灯测试 ✓
    assert!(
        us < 20000.0,
        "每帧 JS 渲染异常慢(> 20ms),疑似灾难级回归: {us} µs"
    );
}

/// S1–S3:插件源的可选 `onKey` —— 校验 + 钩子契约
/// 验收:A1(无 onKey ⇒ 行为不变 ✓)/ A2(认领 ⇒ 消费 ✓)/ A3(不认领 ⇒ 不消费 ✓)
///       A4' 事实只读暴露 ✓ / A6(抛错 ⇒ 未处理,不上抛不崩 ✓)/ A8(无源 ⇒ 回退 ✓)
#[test]
fn picker_on_key_hook_contract() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();

    // A1 前置(S2 校验):onKey 存在但**不是函数** ⇒ define 报错
    assert!(
        load_script(
            r#"helix.picker.define("s-bad", { columns: ["a"], items: () => [], onKey: 42 });"#
        )
        .is_err(),
        "onKey 非函数必须报错"
    );

    // A1:未声明 onKey 的源 ⇒ has=false 且钩子一律 false(**零行为变化** ✓)
    load_script(r#"helix.picker.define("s-plain", { columns: ["a"], items: () => [] });"#).unwrap();
    assert!(!crate::picker::picker_has_on_key("s-plain"));
    assert!(!crate::picker::picker_key_hook(
        "s-plain", "y", true, false, false, "", -1
    ));

    // A8:源不存在 ⇒ 回退(false),不 panic ✓
    assert!(!crate::picker::picker_has_on_key("s-nonexistent"));
    assert!(!crate::picker::picker_key_hook(
        "s-nonexistent",
        "y",
        true,
        false,
        false,
        "",
        -1
    ));

    // A2/A3:声明了 onKey ⇒ 仅当返回 "handled" 才消费 ✓
    load_script(
        r#"helix.picker.define("s-key", { columns: ["a"], items: () => [],
             onKey: (e) => (e.ctrl && e.key === "y") ? "handled" : undefined });"#,
    )
    .unwrap();
    assert!(crate::picker::picker_has_on_key("s-key"));
    assert!(
        crate::picker::picker_key_hook("s-key", "y", true, false, false, "", -1),
        "C-y 应被消费"
    );
    assert!(
        !crate::picker::picker_key_hook("s-key", "z", true, false, false, "", -1),
        "未认领的键不得消费"
    );
    assert!(
        !crate::picker::picker_key_hook("s-key", "y", false, false, false, "", -1),
        "无 ctrl 时不得消费"
    );

    // A6:钩子抛错 ⇒ **未处理**(不上抛、不崩 ✓)
    load_script(
        r#"helix.picker.define("s-throw", { columns: ["a"], items: () => [],
             onKey: () => { throw new Error("boom"); } });"#,
    )
    .unwrap();
    assert!(
        !crate::picker::picker_key_hook("s-throw", "y", true, false, false, "", -1),
        "抛错必须等于未处理"
    );

    // A4':只暴露**事实** —— 用钩子自身的判定来证明实参原样送到 ✓(无需读回辅助 ✓)
    load_script(
        r#"helix.picker.define("s-facts", { columns: ["a"], items: () => [],
             onKey: (e) => (e.key === "x" && e.alt === true && e.ctrl === false
                            && e.shift === false && e.query === "abc"
                            && e.selected === 7) ? "handled" : undefined });"#,
    )
    .unwrap();
    assert!(
        crate::picker::picker_key_hook("s-facts", "x", false, true, false, "abc", 7),
        "事实应原样传给插件"
    );
    assert!(
        !crate::picker::picker_key_hook("s-facts", "x", true, true, false, "abc", 7),
        "任一事实不符即不应命中"
    );
}

/// 组件视图回调:set_component_render(id, fn) → render_component 读取
#[test]
fn component_render_registration() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 非法参数
    assert!(load_script(r#"helix.set_component_render("x", () => []);"#).is_err());
    assert!(load_script(r#"helix.set_component_render(3, 42);"#).is_err());
    // 注册后 render_component 可读
    load_script(r#"helix.set_component_render(3, () => [{ type: "text", text: "hi-view" }]);"#)
        .unwrap();
    let content =
        crate::host::with_state_or_init(|st| render_component(st, 3, 10, 1, None)).unwrap();
    // 数组返回 → Lines;组件树对象 → Tree;两种都支持(与 popup render 同语义)
    let joined = match &content {
        Content::Lines(lines) => lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.text.as_str())
            .collect::<String>(),
        Content::Tree(_) | Content::Ops(_) => String::new(),
    };
    assert_eq!(joined, "hi-view", "set_component_render 回调内容可读");
    // 未注册 → Err
    assert!(crate::host::with_state_or_init(|st| render_component(st, 999, 10, 1, None)).is_err());
}

/// `has_component_render` 是给"每帧问一句有没有渲染器"用的 ⇒ 它**不得调用 JS 回调**
/// (旧做法 `crate::host::with_state_or_init(|st| render_component(st, id,1,1,None)).is_ok()` 会真的渲染一次)。这里用副作用计数器钉死。
#[test]
fn has_component_render_probes_without_calling_js() {
    let _g = crate::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    crate::init();
    load_script(
        r#"globalThis.__probe_calls = 0;
           helix.set_component_render(77, () => { globalThis.__probe_calls++; return ["x"]; });"#,
    )
    .unwrap();

    assert!(crate::has_component_render(77), "已注册应报存在");
    assert!(!crate::has_component_render(78), "未注册应报不存在");

    let calls = crate::host::with_engine(|e| {
        e.eval(boa_engine::Source::from_bytes("globalThis.__probe_calls"))
            .unwrap()
            .to_number(e)
            .unwrap()
    });
    assert_eq!(
        calls, 0.0,
        "存在性查询不得触发渲染回调(调用了几次: {calls})"
    );

    crate::unregister_component_render(77);
    assert!(!crate::has_component_render(77), "注销后应报不存在");
}

/// 查询类入口**不得**创建宿主:`crate::init()` 在没有宿主的线程上会
/// `install_default(JsHost::new())` —— 一个布尔查询就能把整个 boa 引擎建起来,
/// 而它们全在每帧路径上(`has_handlers`/`has_component_render`/`popup_exists`…)。
///
/// 手法:另起一条**没有宿主**的线程调用它们,断言返回空答案**且**宿主仍不存在。
#[test]
fn queries_do_not_create_a_host() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::thread::spawn(|| {
        assert!(!crate::host::engine_exists(), "新线程本不该有宿主");

        assert!(!crate::has_handlers("startup"), "无宿主 ⇒ 无处理器");
        assert!(!crate::has_component_render(1), "无宿主 ⇒ 无渲染器");
        assert!(!crate::popup_exists(1), "无宿主 ⇒ 无弹窗");
        assert!(
            !crate::picker::picker_has_on_key("no-such-source"),
            "无宿主 ⇒ 无 onKey"
        );
        assert!(crate::command_names().is_empty(), "无宿主 ⇒ 无 JS 命令");
        assert!(
            crate::keymap_hint("g", &[]).is_none(),
            "无宿主 ⇒ 无 JS 提示"
        );
        assert!(crate::take_messages().is_empty(), "无宿主 ⇒ 无消息");

        assert!(
            !crate::host::engine_exists(),
            "查询把宿主/引擎建起来了 ⇒ 必须避免 init()"
        );
    })
    .join()
    .expect("查询不得 panic");
}

/// `{pure:true}` 的渲染器按**输入签名**命中缓存 ⇒ 命中时**连 JS 都不调**;
/// 未声明 `pure` 的渲染器逐帧照常调用(与今天一致,不会因缓存而陈旧)。
#[test]
fn pure_component_render_skips_js_when_inputs_unchanged() {
    let _g = crate::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    crate::init();
    load_script(
        r#"globalThis.__pure_calls = 0;
           globalThis.__impure_calls = 0;
           helix.set_component_render(93, () => { globalThis.__pure_calls++; return ["p"]; }, { pure: true });
           helix.set_component_render(94, () => { globalThis.__impure_calls++; return ["i"]; });"#,
    )
    .unwrap();
    let count = |expr: &str| {
        crate::host::with_engine(|e| {
            e.eval(boa_engine::Source::from_bytes(expr))
                .unwrap()
                .to_number(e)
                .unwrap()
        })
    };

    let _ =
        crate::host::with_state_or_init(|st| render_component_cached(st, 93, 10, 3, None, "sig-A"))
            .unwrap();
    let _ =
        crate::host::with_state_or_init(|st| render_component_cached(st, 93, 10, 3, None, "sig-A"))
            .unwrap();
    assert_eq!(
        count("globalThis.__pure_calls"),
        1.0,
        "同一签名第二次应命中缓存(不调 JS)"
    );

    let _ =
        crate::host::with_state_or_init(|st| render_component_cached(st, 93, 10, 3, None, "sig-B"))
            .unwrap();
    assert_eq!(count("globalThis.__pure_calls"), 2.0, "签名变化应重新渲染");

    let _ =
        crate::host::with_state_or_init(|st| render_component_cached(st, 93, 11, 3, None, "sig-B"))
            .unwrap();
    assert_eq!(
        count("globalThis.__pure_calls"),
        3.0,
        "尺寸变化也是输入变化"
    );

    let _ =
        crate::host::with_state_or_init(|st| render_component_cached(st, 94, 10, 3, None, "sig-A"))
            .unwrap();
    let _ =
        crate::host::with_state_or_init(|st| render_component_cached(st, 94, 10, 3, None, "sig-A"))
            .unwrap();
    assert_eq!(
        count("globalThis.__impure_calls"),
        2.0,
        "未声明 pure 时不得缓存(逐帧真渲染)"
    );
}

/// 脏渲染协议:回调返回 `null`/`undefined` = "无变化" ⇒ 复用上次内容(**不再解析**);
/// 但**首次**就返回 null ⇒ 明确报错(而不是静默空内容)。是否脏由插件决定 ⇒ 无陈旧风险。
#[test]
fn component_render_supports_dirty_null_reuse() {
    let _g = crate::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    crate::init();
    load_script(
        r#"globalThis.__dirty_calls = 0;
           helix.set_component_render(88, () => {
             globalThis.__dirty_calls++;
             return globalThis.__dirty_calls === 1 ? ["first"] : null;
           });"#,
    )
    .unwrap();

    let first = crate::host::with_state_or_init(|st| render_component(st, 88, 10, 3, None))
        .expect("首帧应有内容");
    let second = crate::host::with_state_or_init(|st| render_component(st, 88, 10, 3, None))
        .expect("null 应复用上次内容");
    assert_eq!(first, second, "null 之后应拿到**同一份**内容");

    let calls = crate::host::with_engine(|e| {
        e.eval(boa_engine::Source::from_bytes("globalThis.__dirty_calls"))
            .unwrap()
            .to_number(e)
            .unwrap()
    });
    assert_eq!(calls, 2.0, "两次渲染都应真的调用回调(脏不脏由插件决定)");

    // `undefined`(忘了 return)仍是**错误** —— 不能退化成静默陈旧
    load_script(r#"helix.set_component_render(90, () => {});"#).unwrap();
    assert!(
        crate::host::with_state_or_init(|st| render_component(st, 90, 10, 3, None)).is_err(),
        "undefined 应仍按原样报错(不是'无变化')"
    );

    load_script(r#"helix.set_component_render(89, () => null);"#).unwrap();
    let err = crate::host::with_state_or_init(|st| render_component(st, 89, 10, 3, None))
        .expect_err("首次返回 null 应报错");
    assert!(
        format!("{err}").contains("无变化"),
        "错误信息应点明脏协议语义: {err}"
    );
}

/// keymap 前缀提示:set_keymap_hint 回调 → keymap_hint 调用;null/无注册 → None
#[test]
fn keymap_hint_hook() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 未注册 → None
    assert_eq!(keymap_hint("g", &[]), None);
    // 注册:字符串 → (文本, 默认位置);对象 → 指定位置
    load_script(
        r#"
        helix.set_keymap_hint((ctx) => {
            if (ctx.title === "g") return "h 左移\ng 分组";
            if (ctx.title === "x") return { text: "x 提示", position: "bottom-left" };
            return null;
        });
        "#,
    )
    .unwrap();
    let entries = vec![("h".to_string(), "move_char_left".to_string())];
    let (text, pos) = keymap_hint("g", &entries).unwrap();
    assert!(text.contains("h 左移"), "回调返回文本: {text:?}");
    assert_eq!(pos, "bottom-right", "字符串返回默认位置");
    // 对象返回 → 指定位置
    let (text, pos) = keymap_hint("x", &entries).unwrap();
    assert_eq!(text, "x 提示");
    assert_eq!(pos, "bottom-left", "对象指定位置");
    // 其他前缀 → null → None
    assert_eq!(keymap_hint("z", &entries), None);
    // 清除
    load_script(r#"helix.set_keymap_hint(null);"#).unwrap();
    assert_eq!(keymap_hint("g", &entries), None);
}

/// 组件状态通道:register_component_state(JSON) → get_component_state(对象);未注册 → null
#[test]
fn component_state_roundtrip() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 未注册 → null
    load_script(r#"globalThis.__cs = helix.get_component_state(7);"#).unwrap();
    crate::state::with_engine(|engine| {
        let v = engine
            .global_object()
            .get(JsString::from("__cs"), engine)
            .unwrap();
        assert!(v.is_null(), "未注册 → null");
    });
    // 注册 JSON 提供者 → 对象
    crate::host::with_state_or_init(|st| {
        crate::state::register_component_state(st, 7, |_| {
            r#"{"mode":"insert","title":"bash"}"#.to_string()
        })
    });
    load_script(r#"globalThis.__cs2 = helix.get_component_state(7);"#).unwrap();
    crate::state::with_engine(|engine| {
        let v = engine
            .global_object()
            .get(JsString::from("__cs2"), engine)
            .unwrap();
        let obj = v.as_object().unwrap();
        let mode = obj.get(JsString::from("mode"), engine).unwrap();
        assert_eq!(mode.as_string().unwrap().to_std_string_escaped(), "insert");
        let title = obj.get(JsString::from("title"), engine).unwrap();
        assert_eq!(title.as_string().unwrap().to_std_string_escaped(), "bash");
    });
    crate::host::with_state_or_init(|st| crate::state::unregister_component_state(st, 7));
}

/// 装饰请求队列:set_virtual_text / set_highlight 入队,take_decorations 一次取走;
/// set_virtual_text 只传 path(text 省略)→ Clear。路径已 canonicalize(绝对路径断言原样)。
#[test]
fn decorations_queue_and_take() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("dec1", () => {
        helix.set_virtual_text("/tmp/a.rs", 0, 0, "hi", "ui.help");
        helix.set_highlight("/tmp/a.rs", 0, 0, 1, 2, "ui.selection");
        helix.set_virtual_text("/tmp/nope.rs");
    });
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
    crate::host::with_state_or_init(|st| run_command(st, "dec1", &ctx)).unwrap();
    let reqs = take_decorations();
    assert_eq!(reqs.len(), 3);
    // 路径已 canonicalize(相对→绝对:用绝对路径输入,断言原样)
    assert_eq!(reqs[0].doc.as_deref(), Some("/tmp/a.rs"));
    match &reqs[0].kind {
        crate::types::DecorationKind::VirtualText {
            row,
            col,
            text,
            style,
        } => {
            assert_eq!((*row, *col), (0, 0));
            assert_eq!(text, "hi");
            assert_eq!(style.as_deref(), Some("ui.help"));
        }
        other => panic!("expected VirtualText, got {other:?}"),
    }
    match &reqs[1].kind {
        crate::types::DecorationKind::Highlight {
            sr,
            sc,
            er,
            ec,
            style,
        } => {
            assert_eq!((*sr, *sc, *er, *ec), (0, 0, 1, 2));
            assert_eq!(style.as_deref(), Some("ui.selection"));
        }
        other => panic!("expected Highlight, got {other:?}"),
    }
    assert!(matches!(reqs[2].kind, crate::types::DecorationKind::Clear));
}

/// 事务门控与复位:begin/end 包住装饰 → end 后取到;命令入口清残留(不跨命令);
/// path 非字符串 → 命令失败(类型校验)。
#[test]
fn decorations_txn_holds_and_reset() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("dec-txn", () => {
        helix.begin_edit();
        helix.set_virtual_text("/tmp/t.rs", 0, 0, "x");
        helix.end_edit();
    });
    helix.register_command("dec-bad", () => { helix.set_virtual_text(42); });
    helix.register_command("dec-bad2", () => { helix.set_virtual_text("/tmp/t.rs", 0, 0); });
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
    // 命令内 begin/end 包住:命令返回后取到(证明 end 后放行)
    crate::host::with_state_or_init(|st| run_command(st, "dec-txn", &ctx)).unwrap();
    assert_eq!(take_decorations().len(), 1);
    // 类型校验:path 非字符串 → 命令失败
    assert!(crate::host::with_state_or_init(|st| run_command(st, "dec-bad", &ctx)).is_err());
    // 给了坐标却缺 text → 报错而非静默 Clear(避免误清空;清除 = 只传 path)
    assert!(crate::host::with_state_or_init(|st| run_command(st, "dec-bad2", &ctx)).is_err());
    // 命令开始复位:上一命令残留不跨命令(dec-bad 报错后队列也应被清)
    assert!(take_decorations().is_empty());
}

/// 脚本(重)加载不继承旧装饰:load_script_named 求值前 push 全局 Clear(doc: None)。
#[test]
fn decorations_cleared_on_script_load() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    assert!(take_decorations().is_empty());
    load_script("helix.register_command('x', () => {});").unwrap();
    let reqs = take_decorations();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].doc, None);
    assert!(matches!(reqs[0].kind, crate::types::DecorationKind::Clear));
}
