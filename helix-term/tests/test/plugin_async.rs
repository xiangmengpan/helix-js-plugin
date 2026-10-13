use super::*;

// ponytail: 本测试需 multi_thread（harness 的 app.close → block_try_flush_writes 用
// block_in_place，current_thread 会 panic）。multi_thread 下主任务 await 时可能迁移 OS 线程，
// 线程本地 TERM_EVENTS 通道滞留旧线程 → ~1/22 随机丢事件；与 docchange 同源，属已知测试侧
// 隐患，生产端编辑器单线程无此路径，待后续任务处理。
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
            helix.run_async("echo async-ok").then((out) => {
                helix.echo("async:" + (out ?? "").trim());
            });
        });
        helix.register_command("async-err", () => {
            helix.run_async("exit 3").catch((e) => {
                helix.echo("err:" + e.message);
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
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":async-demo<ret>"),
                Some(&|app| {
                    // promise resolve → pump_jobs 执行 .then → echo → 状态栏
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "async:async-ok");
                }),
            ),
            (
                Some(":async-err<ret>"),
                Some(&|app| {
                    // 非零退出 → reject → .catch → e.message 形如 "exit 3: <输出>"
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(status.as_ref().contains("err:exit 3"), "status: {status:?}");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
