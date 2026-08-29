use super::*;

/// 多面板并存（用标准 harness：按键可靠处理；布局细节由单测覆盖）
#[tokio::test(flavor = "multi_thread")]
async fn two_right_panels_coexist_and_close_by_id() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("mp.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("multipanel.js");
    std::fs::write(
        &plugin_path,
        r#"
        let a, b;
        helix.register_command("open-a", () => {
            a = helix.open_panel({ side: "right", size: 20, render: (ctx) => Array.from({ length: 2 }, () => "A".repeat(20)) });
        });
        helix.register_command("open-b", () => {
            b = helix.open_panel({ side: "right", size: 10, render: (ctx) => Array.from({ length: 2 }, () => "B".repeat(10)) });
        });
        helix.register_command("close-a", () => { helix.close_panel(a); });
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
                Some(":open-a<ret>"),
                Some(&|app| {
                    let n = app.compositor.count_type(std::any::type_name::<
                        helix_term::ui::plugin_panel::PluginPanel,
                    >());
                    assert!(n >= 1, "panel A should exist, got {n}");
                }),
            ),
            (
                Some(":open-b<ret>"),
                Some(&|app| {
                    let n = app.compositor.count_type(std::any::type_name::<
                        helix_term::ui::plugin_panel::PluginPanel,
                    >());
                    assert!(n >= 2, "two panels should coexist, got {n}");
                }),
            ),
            (
                Some(":close-a<ret>"),
                Some(&|app| {
                    let n = app.compositor.count_type(std::any::type_name::<
                        helix_term::ui::plugin_panel::PluginPanel,
                    >());
                    assert!(n <= 1, "panel A should be closed, got {n}");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
