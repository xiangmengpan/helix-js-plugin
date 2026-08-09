use super::*;

use helix_core::diagnostic::Severity;

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
