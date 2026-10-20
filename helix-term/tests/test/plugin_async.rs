use super::*;

use std::time::Duration;

/// 异步等待预算：本地 worker 正常 <100ms；5s 只影响失败路径的诊断时长。
const D5: Duration = Duration::from_secs(5);

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

    // 不用 `test_key_sequences`：它的断言是"跑完一轮事件循环就查状态栏"，而
    // run_async 的结果经 worker 线程 → 主线程才 resolve。用 pump_until 等到**结果真的到了**。
    // （踩过的坑：空按键步 `(None, Some(fn))` 没有事件可等，会挂到超时，实测 887s。）
    let mut app = AppBuilder::new().with_file(file, None).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;

    pump_until(&mut app, ":async-demo<ret>", D5, "run_async 的 .then 回包", |a| {
        status_text(a).starts_with("async:")
    })
    .await?;
    // 原断言一字不改
    let (status, _) = app.editor.get_status().unwrap();
    assert_eq!(status.as_ref(), "async:async-ok");

    pump_until(&mut app, ":async-err<ret>", D5, "run_async 失败的 .catch 回包", |a| {
        status_text(a).contains("err:")
    })
    .await?;
    // 非零退出 → reject → .catch → e.message 形如 "exit 3: <输出>"
    let (status, _) = app.editor.get_status().unwrap();
    assert!(status.as_ref().contains("err:exit 3"), "status: {status:?}");

    Ok(())
}
