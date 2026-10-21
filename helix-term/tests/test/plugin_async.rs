use super::*;

use std::time::Duration;

/// 异步等待预算：本地 worker 正常 <100ms；5s 只影响失败路径的诊断时长。
const D5: Duration = Duration::from_secs(5);

// 本测试需 multi_thread（harness 的 app.close → block_try_flush_writes 用 block_in_place，
// current_thread 会 panic）。
//
// 2026-10-20 更新（原注释说"主任务迁移 OS 线程导致线程本地通道丢事件"——**已证伪**）：
// `Application` 持 boa `Context`（`!Send`）⇒ 它不能被 tokio 在 worker 间搬。实测
// （`HX_DBG_JS_THREAD=1`）失败时的形态是：进程 worker 线程**开始跑但迟迟不 send(Exit)**
// （日志里 5 个 worker 只有 3–5 个打出 `worker send Exit`，且无 `spawn FAILED`），
// 即**线程饥饿**（测试开 64 线程、或两套全量并发跑时尤其明显），不是通道串线程。
// 证据与结论见 `docs/superpowers/specs/2026-10-19-t7-event-loss.md` §3。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_async_run() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("async.js");
    std::fs::write(
        &plugin_path,
        r#"
        // 测试夹具:fail-closed 之后,脚本必须声明它用的能力(否则被拒)
        helix.plugin("test-fixture", { deps: [], permissions: { process: true, fs_read: true, fs_write: true } });


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
    pump(
        &mut app,
        &format!(":plugin-load {}<ret>", plugin_path.display()),
    )
    .await?;

    pump_until(
        &mut app,
        ":async-demo<ret>",
        D5,
        "run_async 的 .then 回包",
        |a| status_text(a).starts_with("async:"),
    )
    .await?;
    // 原断言一字不改
    let (status, _) = app.editor.get_status().unwrap();
    assert_eq!(status.as_ref(), "async:async-ok");

    pump_until(
        &mut app,
        ":async-err<ret>",
        D5,
        "run_async 失败的 .catch 回包",
        |a| status_text(a).contains("err:"),
    )
    .await?;
    // 非零退出 → reject → .catch → e.message 形如 "exit 3: <输出>"
    let (status, _) = app.editor.get_status().unwrap();
    assert!(status.as_ref().contains("err:exit 3"), "status: {status:?}");

    Ok(())
}
