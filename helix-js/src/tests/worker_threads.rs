//! worker 线程预算（T7 单线程化后的回归锁）
//!
//! 背景：每个进程 worker 曾经是 **1 主 + 2 读线程 = 3 个线程**；并行测试里这会直接把机器
//! 压到线程饥饿 —— worker 起得来却迟迟不 `send(Exit)`，表现为"事件没回来"（见
//! `docs/superpowers/specs/2026-10-19-t7-event-loss.md` §9）。现在改成一个线程用
//! `poll` 同时盯 stdout/stderr。
//!
//! 这条用例用 `/proc/self/status` 的 `Threads:` 直接量增量 —— 只有**串行**跑才有意义，
//! 所以先拿 `TEST_LOCK`。判定阈值取 2.0：真实值 1.0（单线程化前是 3.0），
//! 留一点余量给测试框架自己的线程波动，但加回任一条读线程都会立刻超阈值。

use super::*;

#[cfg(target_os = "linux")]
fn threads_now() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Threads:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
#[test]
fn process_worker_uses_at_most_one_thread_per_spawn() {
    use std::time::Duration;
    const N: usize = 5;
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::init();
    let before = threads_now();
    for i in 0..N {
        let (ctl_tx, ctl_rx) = std::sync::mpsc::channel();
        crate::state::with_term_workers(|m| m.insert(9000 + i as u64, ctl_tx));
        let ev_tx = crate::state::with_term_events(|t| t.clone()).expect("TERM_EVENTS");
        crate::shell::spawn_worker_for_test(9000 + i as u64, "sleep 0.4", false, ev_tx, ctl_rx);
    }
    std::thread::sleep(Duration::from_millis(150));
    let during = threads_now();
    let per_spawn = (during.saturating_sub(before)) as f64 / N as f64;
    println!(
        "thread delta={} per_spawn={per_spawn:.1}",
        during.saturating_sub(before)
    );
    assert!(
        per_spawn <= 2.0,
        "每个进程 worker 的线程数应 ≈1（单线程 poll），实测 {per_spawn:.1}"
    );
    // 收尾：Kill 掉，别让 sleep 拖住后续用例
    for i in 0..N {
        let _ = crate::shell::term_kill(9000 + i as u64);
    }
    std::thread::sleep(Duration::from_millis(500));
}
