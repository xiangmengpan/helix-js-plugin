//! 文件系统 watcher 宿主侧：处理 UiRequest::Watch/Unwatch —— 用 notify 建
//! watcher（递归目录 / 单文件），变更经防抖线程批量投递到 helix-js 事件通道
//! （helix_js::push_watch_event），主线程每帧 drain + resolve 调 JS 回调。
//! 设计：docs/superpowers/specs/2026-08-15-fs-watcher-design.md

use anyhow::{anyhow, Result};
use notify::Watcher;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// 防抖窗口：批量文件操作（git checkout / 脚本生成）合并为一次回调
const DEBOUNCE_MS: u64 = 500;

/// watcher id → (监听路径, notify watcher 实例)。Drop watcher 即停止其监听线程。
/// notify 的 RecommendedWatcher 是 Send，可安全放全局。
// ponytail: 全局单 Mutex——并发注册量级小（插件显式 watch），per-id 锁等真需要再说
static WATCHERS: OnceLock<Mutex<HashMap<u64, (PathBuf, notify::RecommendedWatcher)>>> =
    OnceLock::new();

/// 处理 UiRequest::Watch/Unwatch（由 typed.rs apply_ui_requests 的 match 臂调用；
/// 无需 editor/compositor）。非 Watch/Unwatch 请求直接 Ok（no-op）。
pub fn apply_watch(request: &helix_js::UiRequest) -> Result<()> {
    match request {
        helix_js::UiRequest::Watch { id, path } => {
            // notify 7 无内置防抖（DebouncedEvent 已移除）——用立即 watcher +
            // 防抖转发线程；事件经 mpsc 送达，转发线程按 500ms 静默窗口批量投递
            let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
            let mut watcher = notify::recommended_watcher(tx)
                .map_err(|e| anyhow!("watch '{path}': {e}"))?;
            let mode = if std::path::Path::new(path).is_dir() {
                notify::RecursiveMode::Recursive
            } else {
                notify::RecursiveMode::NonRecursive
            };
            watcher
                .watch(std::path::Path::new(path), mode)
                .map_err(|e| anyhow!("watch '{path}': {e}"))?;
            let wid = *id;
            std::thread::spawn(move || debounce_forward(rx, wid));
            WATCHERS
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .insert(wid, (PathBuf::from(path), watcher));
            Ok(())
        }
        helix_js::UiRequest::Unwatch { id } => {
            // drop watcher → notify 线程停 → 转发线程 rx 断开自动退出
            WATCHERS.get_or_init(Default::default).lock().unwrap().remove(id);
            Ok(())
        }
        _ => Ok(()),
    }
}

/// 防抖转发线程：事件入 pending，500ms 静默后批量推给 helix-js；
/// watcher 断开（Unwatch/drop）→ 冲刷残留后退出。
// ponytail: 每 watcher 一个转发线程；批量操作时会逐条回调 JS（每次单元素数组）。
// 若事件量级需要合并为单次回调，改 push_watch_event 为批量变体即可。
fn debounce_forward(rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>, id: u64) {
    let mut pending: Vec<(String, String)> = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(DEBOUNCE_MS)) {
            Ok(Ok(ev)) => {
                if let Some((kind, path)) = event_to_change(&ev) {
                    pending.push((kind.to_string(), path));
                }
            }
            Ok(Err(_)) => {} // notify 自身错误（如被监听文件被删）：丢弃，watcher 继续
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => flush(&pending, id),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                flush(&pending, id);
                return;
            }
        }
    }
}

fn flush(pending: &[(String, String)], id: u64) {
    for (kind, path) in pending {
        // lib.rs 未 `pub use watch::*`——经模块路径访问（主代理接线时可加 re-export）
        helix_js::watch::push_watch_event(id, kind, path.clone());
    }
}

/// notify Event → (kind, path)。Access/Other/Any 无业务意义，丢弃。
/// rename 在 notify 7 是 ModifyKind::Name：From/To/Both 事件 paths 含来源与目标，
/// 取最后一项（目标路径，Both=[from,to] / To=[to]）——filetree 刷新关注新路径。
fn event_to_change(ev: &notify::Event) -> Option<(&'static str, String)> {
    use notify::event::{EventKind, ModifyKind};
    let is_rename = matches!(ev.kind, EventKind::Modify(ModifyKind::Name(_)));
    let kind = match ev.kind {
        EventKind::Create(_) => "create",
        EventKind::Remove(_) => "delete",
        EventKind::Modify(ModifyKind::Name(_)) => "rename",
        EventKind::Modify(_) => "modify",
        EventKind::Access(_) | EventKind::Other | EventKind::Any => return None,
    };
    let path = if is_rename { ev.paths.last() } else { ev.paths.first() }?;
    Some((kind, path.to_string_lossy().into_owned()))
}
