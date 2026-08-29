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

// code_actions 两阶段：列表 JSON 可读 → execute 应用 edit
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
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("code_actions_basic", &[]))
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":mock-ca<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "ca:mock-fix|quickfix");
                }),
            ),
            (
                Some(":mock-ca<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), r#"exec:{"applied":true}"#);
                }),
            ),
            (
                Some(":mock-ca<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(
                        doc.text().to_string(),
                        "ONE\n",
                        "code action 的 edit 已应用"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
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
