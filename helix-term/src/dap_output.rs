//! 构建/调试输出的**流式**通道：worker 线程逐行 push，主线程在泵里 drain 成
//! `dap-output` 事件。
//!
//! 为什么需要:早先 `dap_build_run` 用 `Command::output()` 一次性收集，构建期间
//! Output 面板**一片空白**（`cargo build` 20 秒就像卡死）。流式后每行立刻可见。
//!
//! 为什么不用 `Callback`:Job 的回调只在**任务完成时**执行一次，拿不到中间输出。
//! 所以这里是一个进程级的小队列 + `helix_event::request_redraw()` 唤醒：
//! 主线程的 `pump_term_events`（每帧都会跑）负责 drain。

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// 单条输出。`category` 直接并进 `dap-output` 事件的 `category` 字段
/// （`build` / `stderr` …，dapui 用它上色）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DapOutputLine {
    pub category: String,
    pub line: String,
}

/// 上限：构建输出可能很长（几千行），面板只保留最近的部分，队列也不该无限涨。
const CAP: usize = 4096;

fn queue() -> &'static Mutex<VecDeque<DapOutputLine>> {
    static Q: OnceLock<Mutex<VecDeque<DapOutputLine>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(VecDeque::with_capacity(CAP)))
}

/// 从 worker 线程推一行（**非阻塞**：满了丢最旧的）。
pub fn push(category: &str, line: String) {
    {
        let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
        if q.len() == CAP {
            q.pop_front();
        }
        q.push_back(DapOutputLine {
            category: category.to_string(),
            line,
        });
    }
    // 唤醒主线程：否则没有输入事件时这一行要等下一次按键才显示
    helix_event::request_redraw();
}

/// 主线程 drain（`pump_term_events` 每帧调用，空则 no-op）。
pub fn drain() -> Vec<DapOutputLine> {
    let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
    q.drain(..).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 队列是**进程级全局**的 ⇒ 两个测试并行会互相污染。串行化（cargo test 默认多线程）。
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn push_drain_roundtrip() {
        let _g = lock();
        let _ = drain(); // 清干净（测试共享全局队列）
        push("build", "line1".into());
        push("stderr", "line2".into());
        let got = drain();
        assert_eq!(
            got,
            vec![
                DapOutputLine {
                    category: "build".into(),
                    line: "line1".into()
                },
                DapOutputLine {
                    category: "stderr".into(),
                    line: "line2".into()
                },
            ]
        );
        assert!(drain().is_empty(), "drain 后应为空");
    }

    #[test]
    fn cap_drops_oldest() {
        let _g = lock();
        let _ = drain();
        for i in 0..CAP + 10 {
            push("build", format!("l{i}"));
        }
        let got = drain();
        assert_eq!(got.len(), CAP);
        assert_eq!(got.first().unwrap().line, "l10", "应丢最旧的");
    }
}
