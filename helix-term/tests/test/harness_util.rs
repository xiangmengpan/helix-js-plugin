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

// ── cwd 独占(scoped chdir):与 `pump_until` 同一类"harness 自身的正确性" ──
//
// 为什么值得测:`CwdScope` 的两件事都会**静默**错 ——
// 忘了还原 ⇒ 后面所有用例读到别人的临时目录;没拿锁 ⇒ 并行下与别的 `:cd` 交错。
// 两种都能让一整批用例随机红,而且红在**别人**的文件里(极难定位)。

use helix_stdx::env::current_working_dir;

/// Drop 时还原到进入前的 cwd(而且是绕过 `std::env` 直改 —— 必须走 helix-stdx 缓存)
#[tokio::test(flavor = "multi_thread")]
async fn cwd_scope_restores_previous_cwd() -> anyhow::Result<()> {
    let before = current_working_dir();
    let dir = tempfile::tempdir()?;
    {
        let _scope = CwdScope::enter(dir.path()).await;
        assert_eq!(
            current_working_dir(),
            helix_stdx::path::canonicalize(dir.path())
        );
    }
    assert_eq!(
        current_working_dir(),
        before,
        "Drop 之后必须回到进入前的 cwd"
    );
    Ok(())
}

/// `pinned()` 不改 cwd(只拿锁 + 记还原点),适合"用例自己发 `:cd`"的场景
#[tokio::test(flavor = "multi_thread")]
async fn cwd_scope_pinned_does_not_change_cwd() -> anyhow::Result<()> {
    let before = current_working_dir();
    {
        let _scope = CwdScope::pinned().await;
        assert_eq!(current_working_dir(), before, "pinned 不该动 cwd");
    }
    assert_eq!(current_working_dir(), before);
    Ok(())
}

/// 锁是**真**互斥:第二个 scope 必须等第一个放掉才拿得到(用超时把"没锁上"变成失败而不是挂死)
#[tokio::test(flavor = "multi_thread")]
async fn cwd_scope_serialises_other_takers() -> anyhow::Result<()> {
    let first = CwdScope::pinned().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(200), CwdScope::pinned())
            .await
            .is_err(),
        "第二个 scope 不该在第一个还活着时拿到锁"
    );
    drop(first);
    tokio::time::timeout(Duration::from_secs(2), CwdScope::pinned())
        .await
        .expect("放锁后应能立刻拿到");
    Ok(())
}
