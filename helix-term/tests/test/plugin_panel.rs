use super::*;

use helix_view::current_ref;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_panel_open_edit_close() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("panel.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("panel.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("panel-demo", () => {
            helix.open_panel({ side: "right", size: 20, render: () => ["== panel ==", "content"] });
        });
        "#,
    )?;

    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":panel-demo<ret>"),
                Some(&|app| {
                    assert!(app.compositor.has_component(panel_type), "panel open");
                }),
            ),
            // 面板不拦截按键：事件穿透 → i 进 insert、x 插入、esc 回 normal，文档应变化
            (
                Some("ix<esc>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "xhello\n", "typed while panel open");
                }),
            ),
            (
                Some(":panel-close<ret>"),
                Some(&|app| {
                    assert!(!app.compositor.has_component(panel_type), "panel closed");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
