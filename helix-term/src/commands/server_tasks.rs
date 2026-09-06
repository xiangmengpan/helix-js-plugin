//! server 后台任务队列：JS 提交一组 items（`UiRequest::ServerTask`，task_id 由 JS
//! 分配）→ 单 worker 线程串行执行 server_manager::run_task → 事件经
//! `helix_js::server_tasks::push_event` 回投主线程队列（主线程 resolve 调 JS 回调）。
//! 事件含 seq（每批次内从 0 递增）与 name/kind：run_task 的 progress 参数是阶段
//! 事件（"下载中"/"写入配置"…，kind=phase）；install 的字节进度走
//! server_manager 的 DL_PROGRESS 线程局部钩子（kind=progress，download 层已节流）；
//! 结束按 TaskOutcome.ok 推 done/error。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, OnceLock};

use super::server_manager;
use helix_js::ServerTaskEvent;

/// worker 单任务 = 一批 items（task_id 由 JS 分配，回传事件沿用）
struct Job {
    task_id: u64,
    items: Vec<(String, String, Option<String>)>, // (op, name, version?)
}

fn ensure_worker() -> &'static Sender<Job> {
    static TX: OnceLock<Sender<Job>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("server-task-worker".to_string())
            .spawn(move || worker_loop(rx))
            .expect("spawn server task worker");
        tx
    })
}

/// 提交一批后台任务（typed.rs 分发臂调用；非阻塞——worker 线程串行执行，
/// 事件异步回投，编辑器主线程不等待）。
pub(crate) fn submit(task_id: u64, items: Vec<(String, String, Option<String>)>) {
    if items.is_empty() {
        return;
    }
    let _ = ensure_worker().send(Job { task_id, items });
}

fn worker_loop(rx: Receiver<Job>) {
    while let Ok(job) = rx.recv() {
        // seq 每批次自 0：DL 钩子(Box<'static>)与阶段 progress 闭包共享
        let seq = Arc::new(AtomicU32::new(0));
        for (op, name, version) in &job.items {
            // install/update 的字节进度：install_adhoc 下载时 take 一次消费。
            // 下载前报错/非 install 类 op 未消费 → 静默留在本线程，下一 item 覆盖。
            let (seq_dl, task_id, dl_name) = (Arc::clone(&seq), job.task_id, name.clone());
            server_manager::with_download_progress(Box::new(move |bytes, total| {
                let s = seq_dl.fetch_add(1, Ordering::Relaxed);
                helix_js::server_tasks::push_event(ServerTaskEvent {
                    task_id,
                    seq: s,
                    name: dl_name.clone(),
                    kind: "progress".into(),
                    msg: None,
                    bytes: Some(bytes),
                    total,
                });
            }));
            let seq_ph = Arc::clone(&seq);
            let outcome =
                server_manager::run_task(op, name, version.as_deref(), move |phase, _b, _t| {
                    let s = seq_ph.fetch_add(1, Ordering::Relaxed);
                    helix_js::server_tasks::push_event(ServerTaskEvent {
                        task_id: job.task_id,
                        seq: s,
                        name: name.clone(),
                        kind: "phase".into(),
                        msg: Some(phase.to_string()),
                        bytes: None,
                        total: None,
                    });
                });
            let s = seq.fetch_add(1, Ordering::Relaxed);
            let (kind, msg) = if outcome.ok {
                ("done", Some(outcome.msg))
            } else {
                ("error", Some(outcome.msg))
            };
            helix_js::server_tasks::push_event(ServerTaskEvent {
                task_id: job.task_id,
                seq: s,
                name: name.clone(),
                kind: kind.into(),
                msg,
                bytes: None,
                total: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冒烟：submit → worker 串行执行 → error 事件回投 helix-js 通道
    /// （bogus op 不触网不碰 env，hermetic；worker 为全局单例，串行消费）。
    #[test]
    fn worker_submits_and_pushes_error_event() {
        // 清掉本进程内可能残留的旧事件（避免误判到别家 task_id 不匹配，留着无害）
        let _ = helix_js::server_tasks::drain_events();
        submit(7001, vec![("bogus".to_string(), "x".to_string(), None)]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            for ev in helix_js::server_tasks::drain_events() {
                if ev.task_id == 7001 {
                    assert_eq!(ev.kind, "error", "{ev:?}");
                    assert_eq!(ev.name, "x");
                    assert!(
                        ev.msg.as_deref().unwrap_or("").contains("unknown task op"),
                        "{ev:?}"
                    );
                    assert_eq!(ev.seq, 0, "首事件 seq=0");
                    return;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker 事件 5s 内未到达"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
