//! `helix-js` 单元测试(从 `lib.rs` 的内联模块拆出,见 `docs/architecture-audit.md` G1)。
//!
//! 约定:共享夹具(`TEST_LOCK` / `wait_for_*`)只放本文件;按域的测试放子模块,
//! 子模块靠 `use super::*;` 拿到夹具与 crate 根(父模块的私有 `use` 对子模块可见,
//! 所以子模块不需要也不应该有 `use crate::*;`)。

use super::*;
use crate::state::with_terms;
use boa_engine::JsValue;
use std::sync::Mutex;

// 多个测试共享全局运行时，用锁串行化避免消息队列竞争
//
// ⚠️ 取锁一律用 `.unwrap_or_else(|e| e.into_inner())` **不用** `.unwrap()`:
// 一个测试 panic 会 poison 这个 Mutex,其后每个 `.lock().unwrap()` 都会以
// `PoisonError` 失败 ⇒ **一次抖动被放大成整模块红**。2026-10-14 实测:`cargo test -p
// helix-js`(默认并行)下一次进程事件超时 ⇒ 81/161 失败(其中 80 个是 PoisonError
// 连锁)。CI 的 macOS `Test Suite` 跑的是并行 `cargo test --workspace`,exit 101 的
// 放大机制就是这条 —— 根因往往只有一个超时。
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 轮询 drain_term_events 直到谓词命中或超时（async 测试需要）。
/// 累积自调用以来的全部事件返回：进程事件是 Chunk(s)→Exit 的顺序流，
/// 命中谓词的那次 drain 之前可能已有事件被取走，须一并保留按序 resolve。
/// 诊断用:`TermEvent` 没实现 `Debug`(它是公共 API,不为测试加 trait 派生)
fn describe_term_event(e: &TermEvent) -> String {
    match e {
        TermEvent::Chunk(id, s) => format!("Chunk(id={id}, {} bytes)", s.len()),
        TermEvent::Exit(id, code, out) => format!(
            "Exit(id={id}, code={code}, stdout={})",
            out.as_ref()
                .map(|s| format!("{} bytes", s.len()))
                .unwrap_or_else(|| "None".to_string())
        ),
    }
}

fn wait_for_term_event(pred: impl Fn(&TermEvent) -> bool) -> Vec<TermEvent> {
    // 超时上界是 60s 而不是 5s:CI 用**并行** `cargo test --workspace`,而本 crate 里
    // **三个**测试模块会各自 spawn 进程(picker/pty/lib)⇒ 慢机上"spawn 进程 + 收
    // Chunk/Exit"被拖到十几秒是见过的(2026-10-16 实测:并行跑 31.9s 的机器上
    // `async_utf8_across_chunks` 20s 超时)。这个上界只在谓词**永不**成立时才触发
    // (那是真失败)⇒ 放宽它只影响失败路径的时长,不影响正常路径。
    //
    // 但**超时要能诊断**:一条只有"timed out"的报错等于没有信息(实测吃过这个亏),
    // 所以下面把"等了多久、收到了什么"一并报出来。
    let started = std::time::Instant::now();
    let deadline = started + std::time::Duration::from_secs(60);
    let mut all = Vec::new();
    loop {
        all.extend(drain_term_events());
        if all.iter().any(&pred) {
            return all;
        }
        if std::time::Instant::now() > deadline {
            let kinds: Vec<String> = all.iter().map(describe_term_event).collect();
            panic!(
                "timed out waiting for term event after {:?};已收到 {} 条事件:{:?}",
                started.elapsed(),
                kinds.len(),
                kinds
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// 轮询 drain_async_events 直到谓词命中或超时，事件累积进调用方传入的 vec
/// （跨调用共享累积：异步 fs 四个操作并发发送，后几次 wait 必须能看到先前已 drain 的事件）。
fn wait_for_async(all: &mut Vec<AsyncEvent>, pred: impl Fn(&AsyncEvent) -> bool) {
    // 同 wait_for_term_event:60s 上界 + **超时要能诊断**(报出等了多久、已收到什么)
    let started = std::time::Instant::now();
    let deadline = started + std::time::Duration::from_secs(60);
    loop {
        all.extend(drain_async_events());
        if all.iter().any(&pred) {
            return;
        }
        if std::time::Instant::now() > deadline {
            let kinds: Vec<String> = all.iter().map(|e| format!("{e:?}")).collect();
            panic!(
                "timed out waiting for async event after {:?};已收到 {} 条:{:?}",
                started.elapsed(),
                kinds.len(),
                kinds
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// 状态栏渲染测试的固定快照:字段**全部非默认**,这样断言里"某个值出现了"必定来自这份 ctx
/// (而不是恰好等于默认值)。
///
/// 收在 `tests/mod.rs` 的原因:两份测试(plugin / popup_panel)原本各写一遍 26 行字面量,
/// 归一化后是 9 个 20 行窗口的重复 —— 新增一个字段要改两处。
pub(crate) fn statusline_ctx() -> crate::types::StatuslineCtx {
    crate::types::StatuslineCtx {
        path: Some("/tmp/a.rs".into()),
        mode: "insert".into(),
        cursor: (3, 7),
        total_lines: 100,
        diagnostics_error: 2,
        diagnostics_warning: 1,
        window_mode: false,
        active_leaf_type: "editor".into(),
        active_leaf_path: None,
        modified: false,
        selections: 1,
        selections_primary: 0,
        read_only: false,
        encoding: "utf-8".into(),
        line_ending: "LF".into(),
        register: None,
        primary_selection_length: 1,
        indent_style: "4 spaces".into(),
        file_type: Some("rust".into()),
        cwd: "proj".into(),
        code_action_hint: false,
        workspace_diagnostics_error: 0,
        workspace_diagnostics_warning: 0,
        workspace_diagnostics_info: 0,
        workspace_diagnostics_hint: 0,
        spinner: None,
    }
}

mod doc_edit;
mod events_cwd;
mod events_lsp;
mod fs_async;
mod pane_stack;
mod plugin;
mod popup_panel;
mod term_pty;
