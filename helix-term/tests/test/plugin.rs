use super::*;

use helix_core::diagnostic::Severity;
use helix_term::ui;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_load_and_run_command() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("test_plugin.js");
    std::fs::write(
        &path,
        r#"
        helix.register_command("hello", (ctx) => {
            helix.echo("cursor: " + ctx.cursor.row + "," + ctx.cursor.col);
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", path.display())), None),
            (
                Some(":hello<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "cursor: 0,0");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_popup_open_and_close() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("popup_plugin.js");
    std::fs::write(
        &path,
        r#"
        helix.register_command("showbox", () => {
            helix.open_popup({
                render: () => ["╔═ box ═╗", "║  hi   ║", "╚═══════╝"],
                onKey: (key) => key.name === "Esc" ? "close" : "handled",
                onClose: () => helix.echo("box closed"),
            });
        });
        "#,
    )?;

    // 弹窗图层实际类型是 Popup<PluginPopup>，且测试断言回调只能拿到 &Application，
    // 故用 has_component(type_name) 断言存在/消失（find 需要 &mut 且无法降级到内容类型）。
    let popup_type = std::any::type_name::<ui::Popup<ui::PluginPopup>>();

    test_key_sequences(
        &mut AppBuilder::new().build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", path.display())), None),
            (
                Some(":showbox<ret>"),
                Some(&|app| {
                    assert!(
                        app.compositor.has_component(popup_type),
                        "popup layer should be open after :showbox"
                    );
                }),
            ),
            (
                Some("<esc>"),
                Some(&|app| {
                    assert!(
                        !app.compositor.has_component(popup_type),
                        "popup layer should close on Esc"
                    );
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "box closed");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
