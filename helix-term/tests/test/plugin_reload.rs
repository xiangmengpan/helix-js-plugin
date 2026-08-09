use super::*;

// 验证 :plugin-reload 后插件命令仍可用（重跑注册）。
// 本文件保留；integration.rs 的 mod 声明由控制器合并时统一添加。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_reload_command() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("r.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("reload.js");
    std::fs::write(
        &plugin_path,
        r#"helix.register_command("reload-demo", () => { helix.echo("reloaded-ok"); });"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":reload-demo<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "reloaded-ok");
            })),
            (Some(":plugin-reload<ret>"), Some(&|app| {
                // reload 后命令仍可用（重跑注册）
            })),
            (Some(":reload-demo<ret>"), Some(&|app| {
                let (status, severity) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "reloaded-ok");
                assert!(matches!(severity, helix_core::diagnostic::Severity::Info), "no error on reload");
            })),
        ],
        false,
    )
    .await?;

    Ok(())
}
