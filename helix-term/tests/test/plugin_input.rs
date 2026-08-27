use super::*;

/// input 组件按键路由集成测试：
/// - Tab 聚焦 input 后单字符 → dispatch_input_key → onChange(v)（状态栏 chg:...）
/// - Up/Down → onKey 导航（状态栏 key:Up / key:Down）
/// - Enter → input 有状态时走 onKey("Enter")
#[tokio::test(flavor = "multi_thread")]
async fn plugin_input_edit_and_nav() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("in.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("inp", () => {
            helix.open_popup({
                render: (focus) => helix.el("col", [
                    { type: "input", id: "q", value: "", onChange: (v) => { helix.echo("chg:" + v); }, onKey: (k) => { helix.echo("key:" + k); } },
                ]),
            });
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":inp<ret>"), None),
            // Tab 聚焦 input，键入 'a' → onChange("a") → 状态栏 chg:a
            (Some("<tab>a"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert!(status.as_ref().contains("chg:a"), "status: {status:?}");
            })),
            // Up → onKey("Up")
            (Some("<up>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert!(status.as_ref().contains("key:Up"), "status: {status:?}");
            })),
            // Down → onKey("Down")
            (Some("<down>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert!(status.as_ref().contains("key:Down"), "status: {status:?}");
            })),
            // Enter → input 有状态 → onKey("Enter")
            (Some("<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert!(status.as_ref().contains("key:Enter"), "status: {status:?}");
            })),
        ],
        false,
    )
    .await?;

    Ok(())
}
