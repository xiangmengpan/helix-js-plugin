use super::*;
use helix_view::current_ref;

use std::time::Duration;

use helix_view::input::parse_macro;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

// format：mock 返回 TextEdit("one"→"ONE")→ 自动应用 + 摘要 resolve
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_format_applies() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("fmt.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-fmt", async () => {
            const r = await helix.lsp.format();
            helix.echo("fmt:" + JSON.stringify(r));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("format_basic", &[]))
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":mock-fmt<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), r#"fmt:{"applied":true}"#);
                }),
            ),
            (
                Some(":mock-fmt<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "ONE\n", "format 编辑已应用");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// rename 跨 buffer：当前 + 第二文件都变 + files 计数
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_rename_cross_file() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file_a = dir.path().join("a.mock");
    let file_b = dir.path().join("b.mock");
    std::fs::write(&file_a, "old\n")?;
    std::fs::write(&file_b, "old\n")?;
    let plugin_path = dir.path().join("ren.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-ren", async () => {
            const r = await helix.lsp.rename("new");
            helix.echo("ren:" + JSON.stringify(r));
        });
        "#,
    )?;
    let b_path = file_b.display().to_string();
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file_a, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("rename_cross_file", &[&b_path]))
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":mock-ren<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    // files = workspace_edit_file_count(Edits 按条目)= 2
                    assert_eq!(status.as_ref(), r#"ren:{"applied":true,"files":2}"#);
                }),
            ),
            (
                Some(&format!(":open {}<ret>", file_b.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "new\n", "第二文件被 rename 编辑");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// rename 版本过期：mock 恒返回 version 1（测试 buffer 初始 version=0）→
// apply_workspace_edit 校验失败 → resolve null
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_rename_stale_version() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "old\n")?;
    let plugin_path = dir.path().join("ren-stale.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-ren-stale", async () => {
            const r = await helix.lsp.rename("new");
            helix.echo("stale:" + String(r));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("rename_stale", &[]))
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":mock-ren-stale<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "stale:null", "过期版本 → null");
                }),
            ),
            (
                Some(":mock-ren-stale<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "old\n", "陈旧编辑不落地");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// initialize_only：能力最小 → 查询回 null（顺带验证 capabilities 未声明路径）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_no_capability_resolves_null() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("nocap.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-nocap", async () => {
            const r = await helix.lsp.hover();
            helix.echo("nocap:" + String(r));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("initialize_only", &[]))
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":mock-nocap<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "nocap:null");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// code_actions 两阶段：列表 JSON 可读 → execute 应用 edit（mock 缺省 tag "A" → title "mock-fix-A"/edit "ONE-A"）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_code_actions_execute() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("ca.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-ca", async () => {
            const actions = await helix.lsp.code_actions();
            if (actions === null || actions.length === 0) { helix.echo("ca:empty"); return; }
            const first = actions[0];
            helix.echo("ca:" + first.title + "|" + first.kind);
            const r = await helix.lsp.execute_code_action(first);
            helix.echo("exec:" + JSON.stringify(r));
        });
        "#,
    )?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(mock_lsp_loader("code_actions_basic", &[]))
        .build()?;
    for keys in [
        format!(":plugin-load {}<ret>", plugin_path.display()),
        ":mock-ca<ret>".to_string(),
    ] {
        for key_event in parse_macro(&keys)? {
            tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
        }
        app.event_loop_until_idle(&mut rx_stream).await;
    }
    // 轮询 status:先 ca echo,后 exec echo(execute 的 apply+resolve 在后续帧);
    // 键唤醒(线程局部 REDRAW_NOTIFY 跨线程 wake 丢失)。250ms idle 窗口在重负载下不够,故轮询。
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some((status, _)) = app.editor.get_status() {
            if status.as_ref().contains("exec:") {
                assert_eq!(status.as_ref(), r#"exec:{"applied":true}"#);
                break;
            }
            assert!(
                !status.as_ref().contains("ca:empty"),
                "无可用 action: {status}"
            );
        }
        assert!(
            std::time::Instant::now() < deadline,
            "execute 未在 10s 内完成"
        );
        for key_event in parse_macro("j")? {
            tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
        }
        app.event_loop_until_idle(&mut rx_stream).await;
    }
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "ONE-A\n",
        "code action 的 edit 已应用"
    );
    Ok(())
}

// 双 server code_actions：列表项带 _serverId（可区分来源），execute 按 _serverId 路由到对应 server。
// 判别器：execute 第二个 action——修复前 execute 固定第一个 server → resolve 返回 A 的 edit "ONE-A"（错）；
// 修复后按 _serverId 路由到 B → "ONE-B"（对）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_multiserver_execute_routes_to_owner() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock2");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("ca2.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-ca2", async () => {
            const actions = await helix.lsp.code_actions();
            if (actions === null || actions.length !== 2) { helix.echo("ca2:bad:" + (actions === null ? "null" : actions.length)); return; }
            if (actions[0]._serverId === actions[1]._serverId) { helix.echo("ca2:sameserver"); return; }
            helix.echo("ca2:" + actions.map(a => a.title).join("|") + "|" + actions[0]._serverId + "|" + actions[1]._serverId);
            const r = await helix.lsp.execute_code_action(actions[1]);
            helix.echo("exec2:" + JSON.stringify(r));
        });
        "#,
    )?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(dual_mock_lsp_loader())
        .build()?;
    for keys in [
        format!(":plugin-load {}<ret>", plugin_path.display()),
        ":mock-ca2<ret>".to_string(),
    ] {
        for key_event in parse_macro(&keys)? {
            tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
        }
        app.event_loop_until_idle(&mut rx_stream).await;
    }
    // 轮询 status:先 ca2 列表 echo,后 exec2 echo(execute 在后续帧);
    // 键唤醒(线程局部 REDRAW_NOTIFY 跨线程 wake 丢失)。250ms idle 窗口在重负载下不够,故轮询。
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some((status, _)) = app.editor.get_status() {
            if status.as_ref().contains("exec2:") {
                assert_eq!(status.as_ref(), r#"exec2:{"applied":true}"#);
                break;
            }
            if status.as_ref().starts_with("ca2:") {
                // 两个 server 的 action 都在：title 带 tag 可区分、_serverId 不同
                assert!(
                    status.as_ref().starts_with("ca2:mock-fix-A|mock-fix-B|"),
                    "list should carry both servers' actions with distinct _serverId, got {status:?}"
                );
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "双 server execute 未在 10s 内完成"
        );
        for key_event in parse_macro("j")? {
            tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
        }
        app.event_loop_until_idle(&mut rx_stream).await;
    }
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "ONE-B\n",
        "execute 第二个 action 应路由到 B server"
    );
    Ok(())
}

// 查询方法真实响应：hover（scenario hover_basic）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_query_real_hover() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("query.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-hover", async () => {
            const r = await helix.lsp.hover();
            helix.echo("hover:" + (r ? r.contents.value : "null"));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("hover_basic", &[]))
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":mock-hover<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "hover:mock hover");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 查询方法真实响应：completion（scenario completion_basic，读 label）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_query_real_completion() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("comp.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-comp", async () => {
            const r = await helix.lsp.completion();
            helix.echo("comp:" + (r && r.length ? r[0].label : "null"));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("completion_basic", &[]))
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":mock-comp<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "comp:mock-item");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 查询方法真实响应：goto_definition（scenario goto_definition_basic，mock 返回单 Location → 读 range.start.line）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_query_real_goto() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("def.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-def", async () => {
            const r = await helix.lsp.goto_definition();
            helix.echo("def:" + (r ? (Array.isArray(r) ? r[0].range.start.line : r.range.start.line) : "null"));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("goto_definition_basic", &[]))
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":mock-def<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "def:0");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 查询方法真实响应：document_symbols（scenario symbols_basic，读 name）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_query_real_symbols() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("syms.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-syms", async () => {
            const r = await helix.lsp.document_symbols();
            helix.echo("syms:" + (r && r.length ? r[0].name : "null"));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("symbols_basic", &[]))
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":mock-syms<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "syms:MockSymbol");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// LSP 超时路径：mock 永不响应（no_response）→ 客户端 per-server timeout(1s)→ promise reject → catch。
// 手动 pump + 轮询 status：harness 每 idle 周期(250ms)早于 1s 超时返回,断言须等超时触发后
// 的下一个 idle 周期（idle 泵会 drain LSP 结果并兑现 promise）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_timeout_rejects() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("timeout.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-timeout", async () => {
            try {
                const r = await helix.lsp.hover();
                helix.echo("resolved:" + String(r));
            } catch (e) {
                helix.echo("err:" + e.message);
            }
        });
        "#,
    )?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(mock_lsp_loader_with_timeout("no_response", &[], 1))
        .build()?;
    for keys in [format!(":plugin-load {}<ret>", plugin_path.display())] {
        for key_event in parse_macro(&keys)? {
            tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
        }
        app.event_loop_until_idle(&mut rx_stream).await;
    }
    // 等 server 初始化完成(否则 hover 被 is_initialized 短路 resolve null,测不到超时)
    let init_deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if app
            .editor
            .language_servers
            .iter_clients()
            .any(|c| c.is_initialized())
        {
            break;
        }
        assert!(
            std::time::Instant::now() < init_deadline,
            "mock server 未在 5s 内完成初始化"
        );
        app.event_loop_until_idle(&mut rx_stream).await;
    }
    for keys in [":mock-timeout<ret>".to_string()] {
        for key_event in parse_macro(&keys)? {
            tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
        }
        app.event_loop_until_idle(&mut rx_stream).await;
    }
    // 轮询 status：1s 超时 → reject → catch → echo；idle 周期泵结果,最多等 10s
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some((status, _)) = app.editor.get_status() {
            // 只对终态断言:echo 的 "err:...timed out" → 通过;"resolved" → 超时未触发(行为错误);
            // 其它瞬态状态(非插件 echo)继续轮询,避免急切断言误报
            if status.as_ref().contains("timed out") {
                break;
            }
            assert!(
                !status.as_ref().contains("resolved"),
                "LSP 超时未触发,请求 resolve 了: {status}"
            );
        }
        assert!(
            std::time::Instant::now() < deadline,
            "LSP 超时未在 10s 内触发"
        );
        // 线程局部 REDRAW_NOTIFY:跨线程 wake 会丢失,需发无害键唤醒循环(见批次 2/3 同源隐患)
        for key_event in parse_macro("j")? {
            tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
        }
        app.event_loop_until_idle(&mut rx_stream).await;
    }
    Ok(())
}

// 手动构造 action(无 _serverId)→ 回退第一个 CodeAction server → resolve 用 A → "ONE-A"
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_execute_manual_action_falls_back_first_server() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock2");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("ca-manual.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-ca-manual", async (ctx) => {
            const uri = "file://" + ctx.doc.path;
            // 手动构造 action(无 _serverId):带 edit.changes(含 uri)但无 command——
            // 触发 resolve(回退第一个 server),resolve 覆写 edit 为 ONE-A(而非自带的 IGNORED,证明走 resolve)
            const r = await helix.lsp.execute_code_action({
                title: "manual",
                edit: { changes: { [uri]: [{ range: { start: {line:0,character:0}, end: {line:0,character:3} }, newText: "IGNORED" }] } }
            });
            helix.echo("execm:" + JSON.stringify(r));
        });
        "#,
    )?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(dual_mock_lsp_loader())
        .build()?;
    for keys in [
        format!(":plugin-load {}<ret>", plugin_path.display()),
        ":mock-ca-manual<ret>".to_string(),
    ] {
        for key_event in parse_macro(&keys)? {
            tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
        }
        app.event_loop_until_idle(&mut rx_stream).await;
    }
    // 轮询 status：execute 完成(execm: echo)→ 断言；键唤醒(线程局部 REDRAW_NOTIFY 跨线程 wake 丢失)
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some((status, _)) = app.editor.get_status() {
            assert_eq!(status.as_ref(), r#"execm:{"applied":true}"#);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "手动构造 action 执行未在 10s 内完成"
        );
        for key_event in parse_macro("j")? {
            tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
        }
        app.event_loop_until_idle(&mut rx_stream).await;
    }
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "ONE-A\n",
        "手动构造 action(无 _serverId)应回退第一个 server(A)的 resolve 结果"
    );
    Ok(())
}
