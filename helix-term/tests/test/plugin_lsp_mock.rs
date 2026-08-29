use super::*;
use helix_view::current_ref;

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
