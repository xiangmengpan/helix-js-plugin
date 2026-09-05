use super::*;
use helix_view::current_ref;

// 无 LSP 环境（测试配置 lsp.enable=false，txt 无 language server）下：
// helix.lsp.* 请求 → 泵循环 take_lsp_requests → 无 server → resolve null →
// pump_jobs 执行 async 续体 → echo → 状态栏。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_no_server_resolves_null() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt"); // 无 LSP 配置的普通 txt
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("lsp.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("lsp-hover", async () => {
            const res = await helix.lsp.hover();
            helix.echo("hover:" + String(res));
        });
        helix.register_command("lsp-syms", async () => {
            const res = await helix.lsp.document_symbols();
            helix.echo("syms:" + String(res));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":lsp-hover<ret>"),
                Some(&|app| {
                    // 无 server → resolve null → async 续体 echo
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "hover:null");
                }),
            ),
            (
                Some(":lsp-syms<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "syms:null");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// LSP 增强 4 方法无 server 环境下的 null 路径：
// format/rename/code_actions 无 server → resolve null；execute_code_action 与列表同 server 选择，无则 null。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_enhance_no_server_resolves_null() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt"); // 无 LSP
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("lsp-enhance.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("lsp-fmt", async () => {
            const r = await helix.lsp.format();
            helix.echo("fmt:" + String(r));
        });
        helix.register_command("lsp-ren", async () => {
            const r = await helix.lsp.rename("x");
            helix.echo("ren:" + String(r));
        });
        helix.register_command("lsp-ca", async () => {
            const r = await helix.lsp.code_actions();
            helix.echo("ca:" + String(r));
        });
        helix.register_command("lsp-exec", async () => {
            const r = await helix.lsp.execute_code_action({ title: "t" });
            helix.echo("exec:" + String(r));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":lsp-fmt<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "fmt:null");
                }),
            ),
            (
                Some(":lsp-ren<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "ren:null");
                }),
            ),
            (
                Some(":lsp-ca<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "ca:null");
                }),
            ),
            (
                Some(":lsp-exec<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "exec:null");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// open_file 第二参数 {row, col}：打开后光标定位到指定行列（字符坐标 0-based）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_open_file_with_position() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let src = dir.path().join("src.txt");
    let lines: Vec<String> = (0..10).map(|i| format!("line{i}")).collect();
    std::fs::write(&src, lines.join("\n") + "\n")?;
    let plugin_path = dir.path().join("open.js");
    let src_str = src.display().to_string();
    std::fs::write(
        &plugin_path,
        format!(
            r#"
        helix.register_command("jump10", () => {{
            helix.open_file({src:?}, {{ row: 9, col: 4 }});
        }});
        "#,
            src = src_str,
        ),
    )?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(src, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":jump10<ret>"),
                Some(&|app| {
                    let (view, doc) = current_ref!(app.editor);
                    let pos = doc
                        .selection(view.id)
                        .primary()
                        .cursor(doc.text().slice(..));
                    let line = doc.text().char_to_line(pos);
                    assert_eq!(line, 9, "光标应跳到第 9 行");
                    let col = pos - doc.text().line_to_char(line);
                    assert_eq!(col, 4, "光标应到第 4 列");
                    assert_eq!(
                        doc.text().char(pos),
                        '9',
                        "光标应落在 'line9' 的第 4 字符 '9'"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
