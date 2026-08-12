//! 全局状态：thread_local 引擎/注册表/队列 + 跨线程事件通道 + 访问器。
//! 所有状态集中于此，其他模块经 `with_*` 访问器读写（thread_local 无法跨模块直接访问）。

use boa_engine::{Context, JsValue};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::types::{
    AsyncEvent, CursorRequest, Edit, NodeHandlers, PopupCallbacks, TermCallbacks, TermCtrl,
    TermEvent, UiRequest,
};

pub(crate) struct WakeSender<T> {
    inner: std::sync::mpsc::Sender<T>,
}

impl<T> Clone for WakeSender<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<T> WakeSender<T> {
    pub(crate) fn send(&self, t: T) -> Result<(), std::sync::mpsc::SendError<T>> {
        let r = self.inner.send(t);
        // 发送时查当前注册的唤醒回调（set_term_wake 随时生效）
        if let Some(wake) = TERM_WAKE.get() {
            (wake)();
        }
        r
    }
}

/// 宿主注册的跨线程唤醒回调（helix-term 启动时设置：request_redraw）
static TERM_WAKE: OnceLock<std::sync::Arc<dyn Fn() + Send + Sync>> = OnceLock::new();

/// 注册跨线程唤醒回调（worker 发事件时调用；未注册时 no-op）
pub fn set_term_wake(f: Box<dyn Fn() + Send + Sync>) {
    let _ = TERM_WAKE.set(std::sync::Arc::from(f));
}

pub(crate) fn wake_sender<T>(inner: std::sync::mpsc::Sender<T>) -> WakeSender<T> {
    WakeSender { inner }
}

thread_local! {
    static NODE_HANDLERS: RefCell<Option<&'static mut HashMap<(u64, String), NodeHandlers>>> =
        const { RefCell::new(None) };
}
thread_local! {
    static CONTEXT: RefCell<Option<&'static mut Context>> = const { RefCell::new(None) };
    // 持有 JsValue：线程退出时内容泄漏，不随线程 drop（与 CONTEXT 同哲学，见上）
    static REGISTRY: RefCell<Option<&'static mut HashMap<String, JsValue>>> = const { RefCell::new(None) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static POPUPS: RefCell<Option<&'static mut HashMap<u64, PopupCallbacks>>> = const { RefCell::new(None) };
    static NEXT_POPUP_ID: Cell<u64> = const { Cell::new(1) };
    static NEXT_MAP_ID: Cell<u64> = const { Cell::new(1) };
    // 最近一次 open_panel 的面板 id（:panel-close 用，无面板时为 None）
    static LAST_PANEL_ID: Cell<Option<u64>> = const { Cell::new(None) };
    // 原生终端视图 id 计数器（open_terminal 返回值；与 pty 进程 id 分开分配）
    static NEXT_TERMINAL_VIEW_ID: Cell<u64> = const { Cell::new(1) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static BUFFER_ICON_HOOK: RefCell<Option<&'static mut Option<JsValue>>> = const { RefCell::new(None) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static STATUSLINE_HOOK: RefCell<Option<&'static mut Option<JsValue>>> = const { RefCell::new(None) };
    static CURRENT_EDITS: RefCell<Vec<Edit>> = const { RefCell::new(Vec::new()) };
    static CURSOR_REQUESTS: RefCell<Vec<CursorRequest>> = const { RefCell::new(Vec::new()) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static EVENT_HANDLERS: RefCell<Option<&'static mut HashMap<String, Vec<JsValue>>>> = const { RefCell::new(None) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static TERM_CALLBACKS: RefCell<Option<&'static mut HashMap<u64, TermCallbacks>>> = const { RefCell::new(None) };
    static NEXT_TERM_ID: Cell<u64> = const { Cell::new(1) };
    // HashMap::new 非 const fn，COMMAND_DOCS 不能用 const 块初始化
    static COMMAND_DOCS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    static LOADED_SCRIPTS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static SCRIPT_EXPORTS: RefCell<Option<&'static mut HashMap<String, JsValue>>> = const { RefCell::new(None) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static LAST_EXPORT: RefCell<Option<&'static mut Option<JsValue>>> = const { RefCell::new(None) };
    // 主题覆盖：scope → 颜色字符串（set_theme 整体替换；reset_theme 清空）。
    // 只存字符串，无 JsValue，普通 RefCell 即可（随线程 drop）。
    static THEME_OVERRIDES: RefCell<HashMap<String, crate::theme::StyleOverride>> = RefCell::new(HashMap::new());
    // 覆盖集是否变化（set/reset 置位；helix-term drain 时读取并清位）
    static THEME_DIRTY: Cell<bool> = const { Cell::new(false) };
}

/// 访问 CONTEXT 引擎（init() 后 Some；未初始化 panic——所有入口先调 init）
pub(crate) fn with_engine<T>(f: impl FnOnce(&mut Context) -> T) -> T {
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        f(binding.as_mut().expect("CONTEXT initialized by init()"))
    })
}

/// 访问 CONTEXT 槽位（init() 创建引擎用；None → 惰性创建）
pub(crate) fn with_engine_slot<T>(f: impl FnOnce(&mut Option<&'static mut Context>) -> T) -> T {
    CONTEXT.with(|cell| f(&mut cell.borrow_mut()))
}

/// 访问 REGISTRY：首次触达惰性 Box::leak 创建；线程退出时内容泄漏（见上）
pub(crate) fn with_registry<T>(f: impl FnOnce(&mut HashMap<String, JsValue>) -> T) -> T {
    REGISTRY.with(|r| {
        let mut slot = r.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 SCRIPT_EXPORTS：同上（内容泄漏）
pub(crate) fn with_script_exports<T>(f: impl FnOnce(&mut HashMap<String, JsValue>) -> T) -> T {
    SCRIPT_EXPORTS.with(|m| {
        let mut slot = m.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 LAST_EXPORT：同上（内容泄漏）
pub(crate) fn with_last_export<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    LAST_EXPORT.with(|l| {
        let mut slot = l.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 EVENT_HANDLERS：同上（内容泄漏）
pub(crate) fn with_event_handlers<T>(f: impl FnOnce(&mut HashMap<String, Vec<JsValue>>) -> T) -> T {
    EVENT_HANDLERS.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 POPUPS：同上（内容泄漏）
pub(crate) fn with_popups<T>(f: impl FnOnce(&mut HashMap<u64, PopupCallbacks>) -> T) -> T {
    POPUPS.with(|p| {
        let mut slot = p.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 BUFFER_ICON_HOOK：同上（内容泄漏）
pub(crate) fn with_buffer_icon_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    BUFFER_ICON_HOOK.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 STATUSLINE_HOOK：同上（内容泄漏）
pub(crate) fn with_statusline_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    STATUSLINE_HOOK.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 NODE_HANDLERS：同上（内容泄漏）
pub(crate) fn with_node_handlers<T>(f: impl FnOnce(&mut HashMap<(u64, String), NodeHandlers>) -> T) -> T {
    NODE_HANDLERS.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 CURRENT_EDITS（普通 Vec，随线程 drop）
pub(crate) fn with_edits<T>(f: impl FnOnce(&mut Vec<Edit>) -> T) -> T {
    CURRENT_EDITS.with(|e| f(&mut e.borrow_mut()))
}

/// 访问 CURSOR_REQUESTS（普通 Vec，随线程 drop）
pub(crate) fn with_cursor_requests<T>(f: impl FnOnce(&mut Vec<CursorRequest>) -> T) -> T {
    CURSOR_REQUESTS.with(|c| f(&mut c.borrow_mut()))
}

/// 访问 COMMAND_DOCS（普通 HashMap，随线程 drop）
pub(crate) fn with_command_docs<T>(f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    COMMAND_DOCS.with(|d| f(&mut d.borrow_mut()))
}

/// 访问 LOADED_SCRIPTS（普通 Vec，随线程 drop）
pub(crate) fn with_loaded_scripts<T>(f: impl FnOnce(&mut Vec<(String, String)>) -> T) -> T {
    LOADED_SCRIPTS.with(|s| f(&mut s.borrow_mut()))
}

/// 访问 THEME_OVERRIDES（普通 HashMap，随线程 drop）
pub(crate) fn with_theme_overrides<T>(
    f: impl FnOnce(&mut HashMap<String, crate::theme::StyleOverride>) -> T,
) -> T {
    THEME_OVERRIDES.with(|o| f(&mut o.borrow_mut()))
}

/// 置主题脏位（set/reset_theme 调用；helix-term drain 读取）
pub(crate) fn mark_theme_dirty() {
    THEME_DIRTY.with(|d| d.set(true));
}

/// 读取并清主题脏位
pub(crate) fn take_theme_dirty_flag() -> bool {
    THEME_DIRTY.with(|d| d.replace(false))
}

pub(crate) fn next_popup_id() -> u64 {
    NEXT_POPUP_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}

pub(crate) fn next_map_id() -> u64 {
    NEXT_MAP_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}

pub(crate) fn next_terminal_view_id() -> u64 {
    NEXT_TERMINAL_VIEW_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}

pub(crate) fn next_term_id() -> u64 {
    NEXT_TERM_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}

pub(crate) fn next_async_id() -> u64 {
    NEXT_ASYNC_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}

pub(crate) fn last_panel_id() -> Option<u64> {
    LAST_PANEL_ID.with(|c| c.get())
}

pub(crate) fn set_last_panel_id(v: Option<u64>) {
    LAST_PANEL_ID.with(|c| c.set(v));
}

pub(crate) static MESSAGES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
pub(crate) static UI_REQUESTS: OnceLock<Mutex<Vec<UiRequest>>> = OnceLock::new();
/// 插件目录（helix-term 启动时设置；js_load 相对名解析用）
pub(crate) static PLUGINS_DIR: OnceLock<PathBuf> = OnceLock::new();
/// 布局树序列化缓存（helix-term 树变更时写入；get_layout 读取）
pub(crate) static LAST_LAYOUT: OnceLock<Mutex<String>> = OnceLock::new();

/// 终端实例注册表（view_id → cmd；term_list 查询、reload 后仍准确）
static OPEN_TERMS: OnceLock<Mutex<HashMap<u64, String>>> = OnceLock::new();

pub(crate) fn register_term(view_id: u64, cmd: String) {
    OPEN_TERMS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("open terms lock")
        .insert(view_id, cmd);
}

pub(crate) fn unregister_term(view_id: u64) {
    OPEN_TERMS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("open terms lock")
        .remove(&view_id);
}

/// 当前打开的终端列表（view_id, cmd）
pub(crate) fn list_terms() -> Vec<(u64, String)> {
    OPEN_TERMS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("open terms lock")
        .iter()
        .map(|(id, cmd)| (*id, cmd.clone()))
        .collect()
}
// 事件通道按线程存放：回调注册表（TERM_CALLBACKS）是线程本地的，通道也必须同线程配对——
// 全局单通道会被并发测试的 render 泵互偷（别的线程 drain 后 resolve 时找不到本线程的回调，静默丢弃）。
// worker 在 std 线程上持发起线程的 Sender 克隆；drain 只读本线程的 Receiver。
thread_local! {
    static TERM_EVENTS: RefCell<Option<WakeSender<TermEvent>>> = const { RefCell::new(None) };
    static TERM_EVENTS_RX: RefCell<Option<std::sync::mpsc::Receiver<TermEvent>>> = const { RefCell::new(None) };
    // 进程 id → 控制通道（term_write / term_kill 用）。按线程存放：id 由本线程计数器
    // 分配，若 map 全局则并发线程的同 id 互相覆盖（与 TERM_EVENTS 同模式）；
    // worker 线程不访问此表，只在发起线程的 Sender 克隆上发事件。
    static TERM_WORKERS: RefCell<HashMap<u64, std::sync::mpsc::Sender<TermCtrl>>> = RefCell::new(HashMap::new());
    // 进程 id → pty master fd（term_resize 直连 ioctl 用）。
    // 生命周期：worker 线程持 master 所有权；表在 Exit 清理时移除条目。
    // worker 先退出、清理后发生 resize → fd 陈旧返回 EBADF → Err，良性。
    #[cfg(unix)]
    static TERM_MASTERS: RefCell<HashMap<u64, std::os::fd::RawFd>> = RefCell::new(HashMap::new());
    // 异步 fs 事件通道：与 TERM_EVENTS 同模式——发起线程持有 Sender 克隆，
    // 一次性 worker 线程发结果，发起线程 drain 消费。
    static ASYNC_EVENTS: RefCell<Option<WakeSender<AsyncEvent>>> = const { RefCell::new(None) };
    static ASYNC_EVENTS_RX: RefCell<Option<std::sync::mpsc::Receiver<AsyncEvent>>> = const { RefCell::new(None) };
    // 异步 fs id → JS 回调（持有 JsValue：线程退出时内容泄漏，同上）
    static ASYNC_CALLBACKS: RefCell<Option<&'static mut HashMap<u64, JsValue>>> = const { RefCell::new(None) };
    static NEXT_ASYNC_ID: Cell<u64> = const { Cell::new(1) };
}

/// 访问 TERM_EVENTS 发送端（init 时创建；worker 克隆持有）
pub(crate) fn with_term_events<T>(f: impl FnOnce(&mut Option<WakeSender<TermEvent>>) -> T) -> T {
    TERM_EVENTS.with(|t| f(&mut t.borrow_mut()))
}

pub(crate) fn with_term_events_rx<T>(
    f: impl FnOnce(&mut Option<std::sync::mpsc::Receiver<TermEvent>>) -> T,
) -> T {
    TERM_EVENTS_RX.with(|t| f(&mut t.borrow_mut()))
}

/// 访问 TERM_WORKERS（进程 id → 控制通道）
pub(crate) fn with_term_workers<T>(
    f: impl FnOnce(&mut HashMap<u64, std::sync::mpsc::Sender<TermCtrl>>) -> T,
) -> T {
    TERM_WORKERS.with(|t| f(&mut t.borrow_mut()))
}

#[cfg(unix)]
/// 访问 TERM_MASTERS（进程 id → pty master fd）
pub(crate) fn with_term_masters<T>(f: impl FnOnce(&mut HashMap<u64, std::os::fd::RawFd>) -> T) -> T {
    TERM_MASTERS.with(|t| f(&mut t.borrow_mut()))
}

pub(crate) fn with_async_events<T>(f: impl FnOnce(&mut Option<WakeSender<AsyncEvent>>) -> T) -> T {
    ASYNC_EVENTS.with(|t| f(&mut t.borrow_mut()))
}

pub(crate) fn with_async_events_rx<T>(
    f: impl FnOnce(&mut Option<std::sync::mpsc::Receiver<AsyncEvent>>) -> T,
) -> T {
    ASYNC_EVENTS_RX.with(|t| f(&mut t.borrow_mut()))
}

/// 访问 TERM_CALLBACKS：同上（内容泄漏）
pub(crate) fn with_terms<T>(f: impl FnOnce(&mut HashMap<u64, TermCallbacks>) -> T) -> T {
    TERM_CALLBACKS.with(|t| {
        let mut slot = t.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 ASYNC_CALLBACKS：同上（内容泄漏）
pub(crate) fn with_async_callbacks<T>(f: impl FnOnce(&mut HashMap<u64, JsValue>) -> T) -> T {
    ASYNC_CALLBACKS.with(|t| {
        let mut slot = t.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 取走并清空 echo 消息队列
pub fn take_messages() -> Vec<String> {
    crate::init();
    std::mem::take(&mut *MESSAGES.get().expect("MESSAGES not initialized").lock().expect("messages lock poisoned"))
}

pub fn cache_layout(json: &str) {
    let _ = LAST_LAYOUT.get_or_init(Default::default);
    if let Some(m) = LAST_LAYOUT.get() {
        *m.lock().unwrap() = json.to_string();
    }
}

/// 取走并清空 UI 请求队列
pub fn take_ui_requests() -> Vec<UiRequest> {
    crate::init();
    std::mem::take(&mut *UI_REQUESTS.get().expect("UI_REQUESTS initialized").lock().expect("ui requests lock"))
}
