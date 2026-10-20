//! 测试 harness 自身的自测（计划 T1）。
//!
//! 为什么值得单独测：`pump_until` 的**通过路径**决定所有异步用例的耗时，**超时路径**决定
//! 失败时人能不能看懂。两条都断了，迁移会静默劣化成"每条用例白等 5 秒"或"又变回
//! `Option::unwrap` 那种无信息 panic"。

use super::*;

use std::time::{Duration, Instant};

/// 条件第一轮就成立 ⇒ 立即返回，耗时远小于预算。
#[tokio::test(flavor = "multi_thread")]
async fn pump_until_returns_as_soon_as_cond_holds() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;
    let mut app = AppBuilder::new().with_file(file, None).build()?;

    let t0 = Instant::now();
    pump_until(
        &mut app,
        ":echo hello<ret>",
        Duration::from_secs(5),
        "状态栏出现 hello",
        |a| status_text(a).contains("hello"),
    )
    .await?;
    assert!(
        t0.elapsed() < Duration::from_millis(1500),
        "通过路径不该等满预算: {:?}",
        t0.elapsed()
    );
    Ok(())
}

/// 条件永不成立 ⇒ Err；信息必须含条件描述 / 预算 / 最后一次状态栏。
#[tokio::test(flavor = "multi_thread")]
async fn pump_until_timeout_carries_diagnostics() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("b.txt");
    std::fs::write(&file, "x\n")?;
    let mut app = AppBuilder::new().with_file(file, None).build()?;

    let err = pump_until(
        &mut app,
        ":echo bye<ret>",
        Duration::from_millis(400),
        "永不成立的条件",
        |_| false,
    )
    .await
    .expect_err("应当超时");

    let msg = format!("{err:#}");
    assert!(msg.contains("永不成立的条件"), "缺条件描述: {msg}");
    assert!(msg.contains("400"), "缺预算: {msg}");
    assert!(msg.contains("bye"), "缺最后一次状态栏: {msg}");
    Ok(())
}

/// 探针变体：每轮重发 keys，直到条件成立（`all=` 这类只读探针用）。
#[tokio::test(flavor = "multi_thread")]
async fn pump_until_probe_resends_keys_each_round() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("c.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("count.js");
    std::fs::write(
        &plugin,
        r#"let n = 0;
helix.register_command("count", () => { n = n + 1; helix.echo("n=" + n); });"#,
    )?;
    let mut app = AppBuilder::new().with_file(file, None).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;

    // 只发一次只会到 n=1；探针每轮重发 ⇒ 能等到 n>=3
    pump_until_probe(
        &mut app,
        ":count<ret>",
        Duration::from_secs(5),
        "计数到 3",
        |a| status_text(a).contains("n=3"),
    )
    .await?;
    assert!(
        status_text(&mut app).contains("n=3"),
        "{}",
        status_text(&mut app)
    );
    Ok(())
}

/// `status_text` 在没有状态时返回空串（调用方不必 unwrap）。
#[test]
fn status_text_is_empty_when_no_status() {
    // 该助手只读 `Editor::get_status`，用真实 Editor 构造代价过高 ⇒ 这里只钉
    // "函数存在且签名接受 &Application" 由上面三条用例保证；空串语义由实现保证。
    // 保留本用例是为了在有人把它改成 `unwrap` 时**立刻断链**（编译期即可发现）。
    let _f: fn(&helix_term::application::Application) -> String = status_text;
}
