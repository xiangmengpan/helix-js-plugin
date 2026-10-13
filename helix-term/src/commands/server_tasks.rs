//! server 后台任务队列：JS 提交一组 items（`UiRequest::ServerTask`，task_id 由 JS
//! 分配）→ 单 worker 线程串行执行 server_manager::run_task → 事件经
//! `helix_js::server_tasks::push_event` 回投主线程队列（主线程 resolve 调 JS 回调）。
//! 事件含 seq（每批次内从 0 递增）与 name/kind：run_task 的 progress 参数是阶段
//! 事件（"下载中"/"写入配置"…，kind=phase）；install 的字节进度走
//! server_manager 的 DL_PROGRESS 线程局部钩子（kind=progress，download 层已节流）；
//! 结束按 TaskOutcome.ok 推 done/error。
//!
//! 防静默：run_task/内部 panic 不会杀死队列——per-item 与 batch 双层 catch_unwind，
//! panic 一律转 error 事件；spawn 失败(资源耗尽)退化为提交线程内联执行并重试。
//! 任何路径都不会让"队列永久静默吞掉"任务事件。

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use super::server_manager;
use helix_js::ServerTaskEvent;

/// worker 单任务 = 一批 items（task_id 由 JS 分配，回传事件沿用）
struct Job {
    task_id: u64,
    items: Vec<(String, String, Option<String>)>, // (op, name, version?)
}

/// 单 worker 槽：None = 未建起 / spawn 失败（不缓存无接收端的 tx——每次 submit
/// 重试建 worker，成功前批次走内联执行，事件不丢）。
static WORKER: Mutex<Option<Sender<Job>>> = Mutex::new(None);

/// 在途批次数：`submit` **入队前** +1，批次收尾（[InFlightExit] Drop）−1。
/// 语义 = "已提交但还没跑完"，`0` 即队列空且当前批已收尾。
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// 已提交但尚未跑完的后台批次数（0 = 队列空且无批在跑）。
///
/// 给**集成测试**做确定性收敛等待：先等它回 0，再断言行收敛（比轮询墙钟
/// 更准也更快——不浪费固定时长，也不靠"猜几帧够"）。生产代码不消费它。
pub fn pending_jobs() -> usize {
    IN_FLIGHT.load(Ordering::SeqCst)
}

/// 批次收尾的 RAII 计数：放在 `run_batch` 首行，含 `catch_unwind` 也拦不住的
/// 逃逸路径（事件 push / 清钩子）一起归属——正常路径与 unwind 都会减到。
/// 饱和减（不是 `fetch_sub`）：单测直接调 `run_batch`（无配对的自增）时
/// 不得下溢成 `usize::MAX`，那会让集成侧的"队列已空"门永远等不到。
struct InFlightExit;
impl Drop for InFlightExit {
    fn drop(&mut self) {
        let _ = IN_FLIGHT.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            Some(n.saturating_sub(1))
        });
    }
}

/// 自增在**入队前**（send/inline 都算在途）——顺序反过来的话，worker 抢在自增前
/// 跑完就会让等待方把"还没开始"看成"已经空"。
fn in_flight_enter() {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
}

/// 提交一批后台任务（typed.rs 分发臂调用；非阻塞——worker 线程串行执行，
/// 事件异步回投，编辑器主线程不等待）。spawn 失败的极端场景回退为当前线程
/// 内联跑完本批（同步，语义同 :server 同步路径），事件仍照常回投。
pub(crate) fn submit(task_id: u64, items: Vec<(String, String, Option<String>)>) {
    if items.is_empty() {
        return;
    }
    in_flight_enter();
    // 事件回投的落点:登记在**提交方线程**(主线程/调用线程)的宿主桶上 ——
    // worker 线程没有宿主,不登记的话它只能回落到自己的空桶(事件永远到不了这里)。
    // 经 JS 提交时 `js_server_task` 已登记过同一个值;不经 JS 的单元测试路径靠这一行。
    helix_js::server_tasks::register_task_host(task_id);
    // 锁内只做 spawn/发送决策,内联执行在锁外(不持锁跑同步下载)
    let inline = {
        let mut slot = WORKER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<Job>();
            match std::thread::Builder::new()
                .name("server-task-worker".to_string())
                .spawn(move || worker_loop(rx))
            {
                Ok(_) => *slot = Some(tx),
                Err(e) => log::warn!("server task worker spawn 失败({e});task {task_id} 内联执行"),
            }
        }
        match slot.as_mut() {
            // 接收端仅 worker 持有(双层 catch 保证其不死);send 失败仅当线程异常消亡
            Some(tx) => match tx.send(Job { task_id, items }) {
                Ok(()) => None,
                Err(e) => {
                    // 槽位置回 None:死 tx 不缓存,下次 submit 重试建 worker
                    // (否则每次 send 都 Err→内联,永不重试 spawn)
                    *slot = None;
                    log::warn!("server task worker 已失效;task {task_id} 内联执行");
                    let Job { task_id, items } = e.0;
                    Some((task_id, items))
                }
            },
            None => Some((task_id, items)), // spawn 失败:内联,下次 submit 重试
        }
    };
    if let Some((task_id, items)) = inline {
        run_batch(task_id, items);
    }
}

fn worker_loop(rx: Receiver<Job>) {
    while let Ok(job) = rx.recv() {
        run_batch(job.task_id, job.items);
    }
}

/// 跑完一批（worker 线程或内联回退共用）：batch 级 catch 兜底 per-item 之外的
/// panic（事件 push 自身等）——若逃逸会杀死 worker 线程导致队列永久静默。
/// catch 后推批次 error 事件；per-item 的 panic 已在 run_job 内转成 error 事件。
/// 批次结束(含 catch 分支)无条件清 DL_PROGRESS 钩子：未达下载点的批次(remove/
/// unmanage/下载前报错)不会把残留钩子留在本线程——内联回退发生在主线程时,
/// 残留钩子会让后续同步 :server install 消费到死 task_id/seq 的幽灵 progress 事件。
fn run_batch(task_id: u64, items: Vec<(String, String, Option<String>)>) {
    // 在途计数收尾(声明在最前 ⇒ 最后 drop:op_lock 先释放,计数后归零,
    // 等"队列已空"的一方不会在写锁还没放开时就开跑)
    let _in_flight = InFlightExit;
    // 写互斥：整批(含每 item 的 rewrite)与主线程 :server 同步写/picker Enter 不交错。
    // worker 串行消费时锁多半空闲；与主线程写流同时发生时才是关键串行点。
    let _op = server_manager::op_lock();
    // seq 每批次自 0：DL 钩子(Box<'static>)与阶段 progress 闭包共享
    let seq = Arc::new(AtomicU32::new(0));
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_job(&Job { task_id, items }, &seq);
    }));
    if let Err(payload) = res {
        push_event(
            task_id,
            &seq,
            "",
            "error",
            Some(format!(
                "server task batch worker panic: {}",
                panic_msg(&*payload)
            )),
            None,
            None,
        );
    }
    // 无论 Ok/Err(含 run_job 中途 unwind)收尾都清:本线程不留钩子
    server_manager::clear_download_progress();
}

fn push_event(
    task_id: u64,
    seq: &AtomicU32,
    name: &str,
    kind: &str,
    msg: Option<String>,
    bytes: Option<u64>,
    total: Option<u64>,
) {
    let s = seq.fetch_add(1, Ordering::Relaxed);
    helix_js::server_tasks::push_event(ServerTaskEvent {
        task_id,
        seq: s,
        name: name.to_string(),
        kind: kind.to_string(),
        msg,
        bytes,
        total,
    });
}

/// panic payload → 人读文案(&str / String / 兜底)
fn panic_msg(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = p.downcast_ref::<String>() {
        return s.clone();
    }
    "unknown panic payload".to_string()
}

fn run_job(job: &Job, seq: &Arc<AtomicU32>) {
    for (op, name, version) in &job.items {
        // 每 item 包 catch_unwind：run_task(或其内部)panic → 推该 item 的 error
        // 事件(含 panic 内容)并继续处理队列下一 item；seq 推进不受影响。
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // install/update 的字节进度：install_adhoc 下载时 take 一次消费。
            // 下载前报错/非 install 类 op 未消费 → 静默留在本线程，下一 item 覆盖。
            let (seq_dl, task_id, dl_name) = (Arc::clone(seq), job.task_id, name.clone());
            server_manager::with_download_progress(Box::new(move |bytes, total| {
                push_event(
                    task_id,
                    &seq_dl,
                    &dl_name,
                    "progress",
                    None,
                    Some(bytes),
                    total,
                );
            }));
            let seq_ph = Arc::clone(seq);
            server_manager::run_task(op, name, version.as_deref(), move |phase, _b, _t| {
                // run_task 的 progress 参数是阶段事件(下载中/写入配置…);字节进度走 DL 钩子
                push_event(
                    job.task_id,
                    &seq_ph,
                    name,
                    "phase",
                    Some(phase.to_string()),
                    None,
                    None,
                );
            })
        }));
        match res {
            Ok(outcome) => {
                let (kind, msg) = if outcome.ok {
                    ("done", Some(outcome.msg))
                } else {
                    ("error", Some(outcome.msg))
                };
                push_event(job.task_id, seq, name, kind, msg, None, None);
            }
            Err(payload) => {
                push_event(
                    job.task_id,
                    seq,
                    name,
                    "error",
                    Some(format!("worker panic: {}", panic_msg(&*payload))),
                    None,
                    None,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 两测试共享 helix-js 全局事件通道,串行化防互偷(drain 竞争)
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// R2:未达下载点的批次(remove/bogus,均 error 分支不触网)跑完后当前线程
    /// DL 钩子必清——内联回退在主线程时,残留钩子会让后续同步 :server install
    /// 消费到死 task_id/seq 的幽灵 progress 事件。run_batch = submit/worker 共用入口。
    #[test]
    fn run_batch_leaves_no_dl_hook_after_nondownload_batch() {
        let _g = lock();
        let _ = helix_js::server_tasks::drain_events();
        // 预设幽灵钩子(模拟历史残留),批次跑完必须为 None
        server_manager::with_download_progress(Box::new(|_, _| {}));
        assert!(
            server_manager::download_progress_hook_set(),
            "前置:钩子已挂"
        );
        run_batch(
            8001,
            vec![
                ("remove".to_string(), "ghost-ls".to_string(), None), // 未装→error
                ("bogus".to_string(), "x".to_string(), None),         // unknown op→error
            ],
        );
        assert!(
            !server_manager::download_progress_hook_set(),
            "批次结束当前线程钩子已清(含未达下载点的 item)"
        );
        let evs: Vec<_> = helix_js::server_tasks::drain_events()
            .into_iter()
            .filter(|e| e.task_id == 8001)
            .collect();
        assert_eq!(evs.len(), 2, "两个 item 各推一个 error 事件");
        assert!(evs.iter().all(|e| e.kind == "error"), "{evs:?}");
        assert_eq!((evs[0].seq, evs[1].seq), (0, 1), "seq 连续");
    }

    /// 冒烟：submit → worker 串行执行 → error 事件回投 helix-js 通道
    /// （bogus op 不触网不碰 env，hermetic；worker 为全局单例，串行消费）。
    #[test]
    fn worker_submits_and_pushes_error_event() {
        let _g = lock();
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

    /// B2：`pending_jobs()` 是集成测试的确定性收敛门（等它回 0 再断言行收敛，
    /// 不靠墙钟猜）。它只有两个不变量，都会静默坏，所以逐条钉住：
    /// ① 自增发生在**入队前** —— 反了的话 worker 抢跑完 ⇒ 等待方把"还没开始"当
    ///    "已经空"（假绿）；
    /// ② 收尾减是**饱和**减 —— 直接调 `run_batch`（无配对自增）时不得下溢成
    ///    `usize::MAX`（那会让门永远等不到，把红变成 90s 超时）。
    #[test]
    fn pending_jobs_counts_unfinished_batches_without_underflow() {
        let _g = lock();
        let _ = helix_js::server_tasks::drain_events();
        assert_eq!(pending_jobs(), 0, "初态无在途批次");

        // ② 无配对自增的直接调用：饱和减，计数仍为 0
        run_batch(8201, vec![("bogus".to_string(), "z".to_string(), None)]);
        assert_eq!(pending_jobs(), 0, "直接 run_batch 不得把计数减成负数");

        // ① 入队前自增：+1 立即可见，配对的收尾后归零
        in_flight_enter();
        assert_eq!(pending_jobs(), 1, "入队前自增,提交即刻可见");
        run_batch(8202, vec![("bogus".to_string(), "z".to_string(), None)]);
        assert_eq!(pending_jobs(), 0, "批次收尾后归零");

        // 真实路径：submit 的批次（bogus 不触网）跑完也归零
        submit(8203, vec![("bogus".to_string(), "z".to_string(), None)]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while pending_jobs() != 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "submit 的批次 5s 内在途计数未归零"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let _ = helix_js::server_tasks::drain_events();
    }
}
