//! DAP 协议流水的**内存环形缓冲**（`:debug-log` 用）。
//!
//! 为什么单独存一份：`log::debug!` 只在 `-vv` 时落盘，而"断点没生效/启动失败"这类
//! 问题恰恰需要**当场**看协议收发。环形缓冲保留最近 `CAP` 条，`:debug-log` 直接开成
//! 只读的临时 buffer。

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// 保留多少条（请求+响应各算一条）。太长会吃内存，500 条足够看清一次会话。
const CAP: usize = 500;

fn buf() -> &'static Mutex<VecDeque<String>> {
    static BUF: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    BUF.get_or_init(|| Mutex::new(VecDeque::with_capacity(CAP)))
}

/// 记一条（`dir` = "->" / "<-"）。内部截断超长 body，避免一条把缓冲撑爆。
pub fn record(dir: &str, command: &str, id: u64, body: &str) {
    let mut s = format!("{dir} {command} (#{id})");
    if !body.is_empty() {
        let body = if body.len() > 4000 {
            format!("{}…({}B)", &body[..4000], body.len())
        } else {
            body.to_string()
        };
        s.push(' ');
        s.push_str(&body);
    }
    let mut b = buf().lock().unwrap_or_else(|e| e.into_inner());
    if b.len() == CAP {
        b.pop_front();
    }
    b.push_back(s);
}

/// 取最近 `n` 条（`n = 0` ⇒ 全部）。用于 `:debug-log`。
pub fn tail(n: usize) -> Vec<String> {
    let b = buf().lock().unwrap_or_else(|e| e.into_inner());
    let n = if n == 0 || n > b.len() { b.len() } else { n };
    b.iter().skip(b.len() - n).cloned().collect()
}

/// 清空（`:debug-log clear`）。
pub fn clear() {
    buf().lock().unwrap_or_else(|e| e.into_inner()).clear();
}
