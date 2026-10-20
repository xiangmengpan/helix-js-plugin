use super::*;
use helix_term::application::Application;
use helix_view::current_ref;

use std::time::Duration;

use helix_view::input::parse_macro;

/// 异步等待预算：mock server 本地往返正常 <100ms；5s 只影响失败路径的诊断时长。
const D5: Duration = Duration::from_secs(5);

/// 状态栏含 `needle`（谓词用前缀，见计划 T3 表格的说明）
fn status_has(app: &mut Application, needle: &str) -> bool {
    status_text(app).contains(needle)
}

/// 跑一次 `:<cmd>` 并**等到**状态栏出现 `prefix`，返回状态栏全文。
///
/// LSP 响应不是同步的：`helix.lsp.*` 走 promise，回包要等 `capabilities` 初始化
/// + 一次往返。"跑完一轮事件循环就查状态栏"在高并发下会拿到 None（本文件 9 条用例
/// 此前都这么写，加压下必红）。
async fn run_and_wait(app: &mut Application, cmd: &str, prefix: &str) -> anyhow::Result<String> {
    pump_until(
        app,
        &format!("{cmd}<ret>"),
        D5,
        &format!("{cmd} 的 LSP 回包（前缀 {prefix}）"),
        |a| status_text(a).contains(prefix),
    )
    .await?;
    Ok(status_text(app))
}
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

// format：mock 返回 TextEdit("one"→"ONE")→ 自动应用 + 摘要 resolve
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_format_applies() -> anyhow::Result<()> {
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
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(mock_lsp_loader("format_basic", &[]))
        .build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;

    let status = run_and_wait(&mut app, ":mock-fmt", "fmt:").await?;
    assert_eq!(status, r#"fmt:{"applied":true}"#);

    run_and_wait(&mut app, ":mock-fmt", "fmt:").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.text().to_string(), "ONE\n", "format 编辑已应用");
    Ok(())
}

// rename 跨 buffer：当前 + 第二文件都变 + files 计数
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_rename_cross_file() -> anyhow::Result<()> {
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
    let mut app = AppBuilder::new()
        .with_file(file_a, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(mock_lsp_loader("rename_cross_file", &[&b_path]))
        .build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;

    let status = run_and_wait(&mut app, ":mock-ren", "ren:").await?;
    // files = workspace_edit_file_count(Edits 按条目)= 2
    assert_eq!(status, r#"ren:{"applied":true,"files":2}"#);

    pump(&mut app, &format!(":open {}<ret>", file_b.display())).await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.text().to_string(), "new\n", "第二文件被 rename 编辑");
    Ok(())
}

// rename 版本过期：mock 恒返回 version 1（测试 buffer 初始 version=0）→
// apply_workspace_edit 校验失败 → resolve null
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_rename_stale_version() -> anyhow::Result<()> {
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
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(mock_lsp_loader("rename_stale", &[]))
        .build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;

    let status = run_and_wait(&mut app, ":mock-ren-stale", "stale:").await?;
    assert_eq!(status, "stale:null", "过期版本 → null");

    run_and_wait(&mut app, ":mock-ren-stale", "stale:").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.text().to_string(), "old\n", "陈旧编辑不落地");
    Ok(())
}

// initialize_only：能力最小 → 查询回 null（顺带验证 capabilities 未声明路径）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_no_capability_resolves_null() -> anyhow::Result<()> {
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
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(mock_lsp_loader("initialize_only", &[]))
        .build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;

    let status = run_and_wait(&mut app, ":mock-nocap", "nocap:").await?;
    assert_eq!(status, "nocap:null");
    Ok(())
}

// code_actions 两阶段：列表 JSON 可读 → execute 应用 edit（mock 缺省 tag "A" → title "mock-fix-A"/edit "ONE-A"）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_code_actions_execute() -> anyhow::Result<()> {
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
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(mock_lsp_loader("hover_basic", &[]))
        .build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;

    let status = run_and_wait(&mut app, ":mock-hover", "hover:").await?;
    assert_eq!(status, "hover:mock hover");
    Ok(())
}

// 查询方法真实响应：completion（scenario completion_basic，读 label）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_query_real_completion() -> anyhow::Result<()> {
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
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(mock_lsp_loader("completion_basic", &[]))
        .build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;

    let status = run_and_wait(&mut app, ":mock-comp", "comp:").await?;
    assert_eq!(status, "comp:mock-item");
    Ok(())
}

// 查询方法真实响应：goto_definition（scenario goto_definition_basic，mock 返回单 Location → 读 range.start.line）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_query_real_goto() -> anyhow::Result<()> {
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
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(mock_lsp_loader("goto_definition_basic", &[]))
        .build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;

    let status = run_and_wait(&mut app, ":mock-def", "def:").await?;
    assert_eq!(status, "def:0");
    Ok(())
}

// 查询方法真实响应：document_symbols（scenario symbols_basic，读 name）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_query_real_symbols() -> anyhow::Result<()> {
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
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(mock_lsp_loader("symbols_basic", &[]))
        .build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;

    let status = run_and_wait(&mut app, ":mock-syms", "syms:").await?;
    assert_eq!(status, "syms:MockSymbol");
    Ok(())
}

// LSP 超时路径：mock 永不响应（no_response）→ 客户端 per-server timeout(1s)→ promise reject → catch。
// 手动 pump + 轮询 status：harness 每 idle 周期(250ms)早于 1s 超时返回,断言须等超时触发后
// 的下一个 idle 周期（idle 泵会 drain LSP 结果并兑现 promise）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_timeout_rejects() -> anyhow::Result<()> {
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
    // 等 server 初始化完成(否则 hover 被 is_initialized 短路 resolve null,测不到超时)。
    // 空 keys 只推进事件循环；条件与原来的手写 deadline 循环等价(5s 预算)。
    pump_until(
        &mut app,
        "",
        D5,
        "mock server 完成 initialize",
        |a| a.editor.language_servers.iter_clients().any(|c| c.is_initialized()),
    )
    .await?;
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

// 补全菜单的 JS 图标钩子(`helix.set_completion_icon`)—— **此前零集成覆盖**
// (全仓测试搜不到 completion_kind_icon / render_completion_row 的任何断言)。
//
// 为什么值得钉:这两个钩子是**最后两个走隐式宿主解析**的 JS 入口(理由见
// `ui/completion.rs` 开头的注释:它们从 `menu::Item::format` 走,而该函数也被
// **无 `Context`** 的尺寸路径 `Component::required_size` 调用,且 JS 内容参与弹窗宽度)。
// 契约既然**有意**不动,就必须有用例证明"钩子确实被调到",否则这条路径坏了没人知道。
//
// 探针形状(试了三版,记下来):
//   ① 一开始断言"渲染出来的菜单里出现 ★" —— 不稳定:菜单**会被建出来**(见下面的
//      `last_completion` 探针)但在 `event_loop_until_idle` 处理完 idle 事件后就不在了,
//      渲染那一刻抓不到;
//   ② 第二版在钩子里对 `kind_num == 0` 返回空 —— mock 的 item **没有 `kind`** ⇒ kind_num
//      恒为 0 ⇒ 断言必然踩空(探针 bug,不是实现 bug);
//   ③ 现在:**只累加 JS 侧计数器**,菜单消失后用一条命令读出来。钩子在菜单**布局**时就会
//      被调(`Menu::required_size` → `format`),所以不依赖渲染时机 ✓。
#[tokio::test(flavor = "multi_thread")]
async fn completion_icon_hook_is_called() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin = dir.path().join("comp_icon.js");
    std::fs::write(
        &plugin,
        r#"
        let icon_calls = 0;
        helix.set_completion_icon((k) => { icon_calls++; return ""; });
        helix.register_command("icon-report", () => helix.echo("icon_calls:" + icon_calls));
        "#,
    )?;

    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config_with_lsp())
        .with_lang_loader(mock_lsp_loader("completion_basic", &[]))
        .build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    let mut pump = async |app: &mut Application, keys: &str| -> anyhow::Result<()> {
        for key_event in parse_macro(keys)? {
            tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
        }
        app.event_loop_until_idle(&mut rx_stream).await;
        Ok(())
    };

    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    // 先确认插件真的加载了(加载失败 ⇒ 钩子没注册 ⇒ 测的就不是这条路径)
    let after_load = app
        .editor
        .get_status()
        .map(|(s, _)| s.clone())
        .unwrap_or_default();
    assert!(
        !after_load.contains("error"),
        "插件应先加载成功,实得 {after_load}"
    );

    // insert + C-x 触发补全(mock LSP 的 completion_basic)。补全是异步的 ⇒ 有界等待:
    // 每轮重发 `i<C-x>`（幂等）并推进事件循环，直到菜单**被创建**(`last_completion`)。
    pump_until_probe(
        &mut app,
        "i<C-x>",
        D5,
        "补全菜单被创建（mock LSP 就绪）",
        |a| a.editor.last_completion.is_some(),
    )
    .await?;

    // 回到普通模式,再读计数器:钩子应在菜单布局/渲染时被调过至少一次
    pump(&mut app, "<esc>").await?;
    pump(&mut app, ":icon-report<ret>").await?;
    let (status, _) = app.editor.get_status().unwrap();
    let s = status.as_ref();
    let calls: u64 = s
        .strip_prefix("icon_calls:")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    assert!(
        calls >= 1,
        "补全图标钩子应被调用(菜单布局时就会调);状态栏实得 {s:?}"
    );
    Ok(())
}
