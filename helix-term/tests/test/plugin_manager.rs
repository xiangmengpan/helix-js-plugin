use super::*;

// :plugin 命令族——list/status 集成验证（不碰真实 plugins 目录：install/remove 有单测覆盖路径校验）。
// 本文件保留；integration.rs 的 mod 声明由控制器合并时统一添加。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_manager_list_status() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("r.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("list-demo.js");
    std::fs::write(
        &plugin_path,
        r#"helix.register_command("list-demo", () => {});"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":plugin list<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains(plugin_path.to_str().unwrap()),
                        ":plugin list should show loaded plugin path, got: {status}"
                    );
                }),
            ),
            (
                Some(":plugin status<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("loaded"),
                        ":plugin status should report loaded count, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
