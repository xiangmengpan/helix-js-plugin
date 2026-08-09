use super::*;

use helix_core::diagnostic::Severity;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_command_doc_shown() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("doc.js");
    std::fs::write(
        &plugin_path,
        // 带 doc 第三参注册；执行时 echo，状态栏可断言（证明 load → 注册 → 可执行）
        r#"helix.register_command("docdemo", () => { helix.echo("docdemo-ran"); }, "演示命令说明");"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            // 一次断言同时证明：带 doc 的命令已注册且可执行。doc 文本本身靠单测
            // command_doc 覆盖（prompt doc 渲染在 Prompt 内部，无法外部断言）。
            (
                Some(":docdemo<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "docdemo-ran");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
