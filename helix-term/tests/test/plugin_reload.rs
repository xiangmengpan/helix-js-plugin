use super::*;

use helix_term::ui;

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
        r#"helix.register_command("reload-demo", () => { helix.echo("reloaded-ok"); });
helix.register_command("show-panel", () => { helix.open_panel({ side: "right", size: 30, render: () => ["p1"] }); });"#,
    )?;

    let panel_type = std::any::type_name::<ui::PluginPanel>();

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some(":show-panel<ret>"),
                Some(&|app| {
                    assert!(app.compositor.has_component(panel_type), "panel layer open");
                }),
            ),
            (
                Some(":plugin-reload<ret>"),
                Some(&|app| {
                    assert!(
                        !app.compositor.has_component(panel_type),
                        "panel layer removed after reload (no zombie layer)"
                    );
                }),
            ),
            (Some(":panel-close<ret>"), Some(&|app| {
                // LAST_PANEL_ID 已清空 → 不误报有面板，也不产生残留层
                let (status, severity) = app.editor.get_status().unwrap();
                assert!(matches!(severity, helix_core::diagnostic::Severity::Error), "panel-close reports no panel");
                assert!(status.as_ref().contains("no panel open"));
                assert!(!app.compositor.has_component(panel_type));
            })),
            (Some(":reload-demo<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "reloaded-ok");
            })),
        ],
        false,
    )
    .await?;

    Ok(())
}
