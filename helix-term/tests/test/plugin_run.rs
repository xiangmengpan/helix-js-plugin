use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_shell_run() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("r.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("run.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("runchain", () => {
            const out = helix.run("echo plugin-shell-ok");
            helix.echo(out.trim());
        });
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
                Some(":runchain<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "plugin-shell-ok");
                    assert!(
                        matches!(severity, helix_core::diagnostic::Severity::Info),
                        "no error"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
