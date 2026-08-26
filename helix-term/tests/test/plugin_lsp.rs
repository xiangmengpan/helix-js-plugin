use super::*;

// 无 LSP 环境（测试配置 lsp.enable=false，txt 无 language server）下：
// helix.lsp.* 请求 → 泵循环 take_lsp_requests → 无 server → resolve null →
// pump_jobs 执行 async 续体 → echo → 状态栏。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_no_server_resolves_null() -> anyhow::Result<()> {
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
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
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
