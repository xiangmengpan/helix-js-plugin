use super::*;

// 任务简报步骤 6（临时 mod 验证）：:fs-demo 调 read_file_async 读临时文件 → echo 内容 → 状态栏断言。
// 与 plugin_async 同源隐患：multi_thread 下线程本地 ASYNC_EVENTS 通道可能滞留旧线程（已知测试侧问题）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_fsasync_read() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello fs")?;
    let plugin_path = dir.path().join("fsasync.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("fs-demo", () => {
            helix.read_file_async("PLACEHOLDER_FILE").then((content) => {
                helix.echo("fs::" + content);
            });
        });
        "#
        .replace("PLACEHOLDER_FILE", &format!("{}", file.display())),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":fs-demo<ret>"),
                Some(&|app| {
                    // read_file_async promise 兑现（render 泵）→ echo → 状态栏
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "fs::hello fs");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
