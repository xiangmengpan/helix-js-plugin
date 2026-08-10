use super::*;

use helix_core::diagnostic::Severity;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_async_run() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("async.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("async-demo", () => {
            helix.run_async("echo async-ok", (err, out) => {
                helix.echo("async:" + (out ?? "").trim());
            });
        });
        helix.register_command("async-spawn", () => {
            const id = helix.spawn({ cmd: "echo streamed", onChunk: (c) => helix.echo("chunk:" + c.trim()), onExit: () => helix.echo("done") });
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some(":async-demo<ret>"),
                Some(&|app| {
                    // run_async 完成回调 → echo → 状态栏（render 泵 + idle 循环）
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "async:async-ok");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
