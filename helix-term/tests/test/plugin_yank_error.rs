use super::*;

// :plugin-load 失败 → set_error → last_error 记录 → :yank-error 复制最近错误到 + 寄存器
#[tokio::test(flavor = "multi_thread")]
async fn plugin_load_failure_yankable() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let bad_plugin = dir.path().join("bad.js");
    std::fs::write(&bad_plugin, "this is not valid js {{{")?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            // load 失败 → set_error → last_error 记录
            (
                Some(&format!(":plugin-load {}<ret>", bad_plugin.display())),
                None,
            ),
            // :yank-error 复制最近错误到 + 寄存器（错误文本含命令名与脚本路径）
            (
                Some(":yank-error<ret>"),
                Some(&|app| {
                    let mut reg = app.editor.registers.read('+', &app.editor).unwrap();
                    assert!(reg.any(|r| r.contains("plugin-load") && r.contains("bad.js")));
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
