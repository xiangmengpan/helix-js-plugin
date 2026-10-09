//! 全局状态：thread_local 引擎/注册表/队列 + 跨线程事件通道 + 访问器。
//! 所有状态集中于此，其他模块经 `with_*` 访问器读写（thread_local 无法跨模块直接访问）。

use boa_engine::builtins::promise::ResolvingFunctions;
use boa_engine::{Context, JsValue};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::types::{
    AsyncEvent, CursorRequest, DecorationRequest, DocSnapshot, Edit, NodeHandlers, PopupCallbacks,
    TermCallbacks, TermCtrl, TermEvent, UiRequest,
};

pub struct WakeSender<T> {
    inner: std::sync::mpsc::Sender<T>,
}

impl<T> Clone for WakeSender<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T> WakeSender<T> {
    pub fn send(&self, t: T) -> Result<(), std::sync::mpsc::SendError<T>> {
        let r = self.inner.send(t);
        // 发送时触发当前注册的唤醒回调（set_term_wake 随时生效）
        fire_term_wake();
        r
    }
}

/// 宿主注册的跨线程唤醒回调（helix-term 启动时设置：request_redraw）
/// Mutex 而非 OnceLock：可重复注册（helix-js/term 单测各自设自己的探针 flag，
/// 后设者生效）；生产只注册一次，语义不变。
static TERM_WAKE: Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);

/// 注册跨线程唤醒回调（worker 发事件时调用；未注册时 no-op）
pub fn set_term_wake(f: Box<dyn Fn() + Send + Sync>) {
    *TERM_WAKE.lock().unwrap() = Some(std::sync::Arc::from(f));
}

/// 触发注册的唤醒回调（无注册 no-op）。worker 任意线程可调。
pub(crate) fn fire_term_wake() {
    let wake = TERM_WAKE.lock().unwrap().clone();
    if let Some(w) = wake {
        (w)();
    }
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
    // 批量编辑事务深度(begin_edit/end_edit);>0 时 take_edits 积压
    static EDIT_TXN_DEPTH: Cell<usize> = const { Cell::new(0) };
    // 最近一次 open_panel 的面板 id（:panel-close 用，无面板时为 None）
    static LAST_PANEL_ID: Cell<Option<u64>> = const { Cell::new(None) };
    // 原生终端视图 id 计数器（open_terminal 返回值；与 pty 进程 id 分开分配）
    static NEXT_TERMINAL_VIEW_ID: Cell<u64> = const { Cell::new(1) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static BUFFER_ICON_HOOK: RefCell<Option<&'static mut Option<JsValue>>> = const { RefCell::new(None) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static COMPLETION_ICON_HOOK: RefCell<Option<&'static mut Option<JsValue>>> = const { RefCell::new(None) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static COMPLETION_RENDER_HOOK: RefCell<Option<&'static mut Option<JsValue>>> = const { RefCell::new(None) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static PICKER_SOURCES: RefCell<Option<&'static mut HashMap<String, JsValue>>> = const { RefCell::new(None) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static STATUSLINE_HOOK: RefCell<Option<&'static mut Option<JsValue>>> = const { RefCell::new(None) };
    // keymap 前缀提示回调(set_keymap_hint):无注册 → 内置 Info
    static KEYMAP_HINT_HOOK: RefCell<Option<&'static mut Option<JsValue>>> = const { RefCell::new(None) };
    static CURRENT_EDITS: RefCell<Vec<Edit>> = const { RefCell::new(Vec::new()) };
    // 插件装饰请求队列(与编辑队列同 take 点/同 txn 门控/同复位)
    static DECORATION_REQUESTS: RefCell<Vec<DecorationRequest>> = const { RefCell::new(Vec::new()) };
    // 其它已打开 buffer 快照(by_path 读取;命令/事件入口写入,渲染入口不写)
    static DOC_SNAPSHOTS: RefCell<Vec<DocSnapshot>> = const { RefCell::new(Vec::new()) };
    static CURSOR_REQUESTS: RefCell<Vec<CursorRequest>> = const { RefCell::new(Vec::new()) };
    // 当前 eval 脚本声明的依赖（helix.plugin deps；load eval 后 take 递归加载）
    static LAST_PLUGIN_DEPS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static EVENT_HANDLERS: RefCell<Option<&'static mut HashMap<String, Vec<JsValue>>>> = const { RefCell::new(None) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static TERM_CALLBACKS: RefCell<Option<&'static mut HashMap<u64, TermCallbacks>>> = const { RefCell::new(None) };
    // run_async 的 promise 解析函数（id → resolve/reject）；内容泄漏（同上）
    static TERM_PROMISES: RefCell<Option<&'static mut HashMap<u64, ResolvingFunctions>>> =
        const { RefCell::new(None) };
    static NEXT_TERM_ID: Cell<u64> = const { Cell::new(1) };
    // HashMap::new 非 const fn，COMMAND_DOCS 不能用 const 块初始化
    static COMMAND_DOCS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    static LOADED_SCRIPTS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    // 加载中的脚本 key 栈（跨脚本共享）：嵌套 helix.load 期间保持祖先在栈上，
    // 循环依赖（A load B, B load A）在此检出——否则嵌套 eval 无限递归栈溢出崩溃。
    static LOAD_STACK: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// 与 LOAD_STACK **一一对应**的"提供根"栈(§4-2)。两栈只在 commands.rs 同一处
    /// push/pop,便于保持同步;空 Path 表示"绝对路径加载,无提供根"。
    static LOAD_ROOTS: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static SCRIPT_EXPORTS: RefCell<Option<&'static mut HashMap<String, JsValue>>> = const { RefCell::new(None) };
    // 持有 JsValue：线程退出时内容泄漏（同上）
    static LAST_EXPORT: RefCell<Option<&'static mut Option<JsValue>>> = const { RefCell::new(None) };
    // 主题覆盖：scope → 颜色字符串（set_theme 整体替换；reset_theme 清空）。
    // 只存字符串，无 JsValue，普通 RefCell 即可（随线程 drop）。
    static THEME_OVERRIDES: RefCell<HashMap<String, crate::theme::StyleOverride>> = RefCell::new(HashMap::new());
    // 组件状态提供者：组件 id → JSON 字符串生成器（helix-term 不依赖 boa,状态经 JSON 传递）。
    // 组件创建时注册、Drop 时注销;get_component_state 读取。
    static COMPONENT_STATES: RefCell<ComponentStateMap> = RefCell::new(HashMap::new());
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

/// 当前 eval 脚本声明的依赖（helix.plugin deps 累积处；load eval 后 take 递归加载）
pub(crate) fn with_last_plugin_deps<T>(f: impl FnOnce(&mut Vec<String>) -> T) -> T {
    LAST_PLUGIN_DEPS.with(|c| f(&mut c.borrow_mut()))
}

/// 访问 CONFIG_SCHEMAS：同上
pub(crate) fn with_config_schemas<T>(f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    CONFIG_SCHEMAS.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 写入 schema(helix-term 测试/插件预置用;插件正常走 define_config)
pub fn seed_config_schema(name: &str, json: &str) {
    with_config_schemas(|m| {
        m.insert(name.to_string(), json.to_string());
    });
}

/// 读取全部插件配置 schema(helix-term build_plugin_configs 用)name → schema JSON
pub fn config_schemas() -> Vec<(String, String)> {
    with_config_schemas(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// 写入插件配置合并缓存（helix-term config load/reload 后调用）
pub fn cache_configs(json: &str) {
    let _ = CONFIGS.get_or_init(Default::default);
    if let Some(m) = CONFIGS.get() {
        *m.lock().unwrap() = json.to_string();
    }
}

/// 访问 EVENT_HANDLERS：同上（内容泄漏）
pub(crate) fn with_event_handlers<T>(f: impl FnOnce(&mut HashMap<String, Vec<JsValue>>) -> T) -> T {
    EVENT_HANDLERS.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 注册组件状态提供者：组件 id → 序列化为 JSON 的闭包（helix-term 侧调用）。
pub fn register_component_state(id: u64, f: impl Fn(u64) -> String + 'static) {
    COMPONENT_STATES.with(|s| {
        s.borrow_mut().insert(id, Box::new(f));
    })
}

/// 注销组件状态提供者（组件 Drop 时调用）。
pub fn unregister_component_state(id: u64) {
    COMPONENT_STATES.with(|s| {
        s.borrow_mut().remove(&id);
    })
}

/// 读取组件状态（JSON 字符串；未注册 → None）。
pub fn get_component_state_json(id: u64) -> Option<String> {
    COMPONENT_STATES.with(|s| s.borrow().get(&id).map(|f| f(id)))
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

/// 访问 COMPLETION_ICON_HOOK：同上（内容泄漏）
pub(crate) fn with_completion_icon_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    COMPLETION_ICON_HOOK.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 COMPLETION_RENDER_HOOK：同上（内容泄漏）
pub(crate) fn with_completion_render_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    COMPLETION_RENDER_HOOK.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 PICKER_SOURCES：同上（内容泄漏）
pub(crate) fn with_picker_sources<T>(f: impl FnOnce(&mut HashMap<String, JsValue>) -> T) -> T {
    PICKER_SOURCES.with(|s| {
        let mut slot = s.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 KEYMAP_HINT_HOOK：同上（内容泄漏）
pub(crate) fn with_keymap_hint_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    KEYMAP_HINT_HOOK.with(|h| {
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

thread_local! {
    // 状态栏替换模式（set_statusline(fn, { replace: true })；false = 追加右侧）。随线程
    static STATUSLINE_REPLACE: Cell<bool> = const { Cell::new(false) };
    // replace 模式的左:中:右区域比例（默认 1:1:1）
    static STATUSLINE_ZONES: RefCell<[u16; 3]> = const { RefCell::new([1, 1, 1]) };
}

pub(crate) fn set_statusline_replace(v: bool) {
    STATUSLINE_REPLACE.with(|r| r.set(v));
}

pub fn statusline_replaces() -> bool {
    STATUSLINE_REPLACE.with(|r| r.get())
}

/// 设置 replace 模式区域比例 [左, 中, 右]（0 允许：该区不分配宽度）
pub(crate) fn set_statusline_zones(zones: [u16; 3]) {
    STATUSLINE_ZONES.with(|z| *z.borrow_mut() = zones);
}

/// replace 模式区域比例 [左, 中, 右]
pub fn statusline_zones() -> [u16; 3] {
    STATUSLINE_ZONES.with(|z| *z.borrow())
}

/// 访问 NODE_HANDLERS：同上（内容泄漏）
pub(crate) fn with_node_handlers<T>(
    f: impl FnOnce(&mut HashMap<(u64, String), NodeHandlers>) -> T,
) -> T {
    NODE_HANDLERS.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问批量编辑事务深度(普通 usize,随线程 drop)
pub(crate) fn with_txn_depth<T>(f: impl FnOnce(&mut usize) -> T) -> T {
    EDIT_TXN_DEPTH.with(|d| {
        let mut v = d.get();
        let out = f(&mut v);
        d.set(v);
        out
    })
}

/// 访问 CURRENT_EDITS（普通 Vec，随线程 drop）
pub(crate) fn with_edits<T>(f: impl FnOnce(&mut Vec<Edit>) -> T) -> T {
    CURRENT_EDITS.with(|e| f(&mut e.borrow_mut()))
}

/// 访问 DECORATION_REQUESTS（普通 Vec，随线程 drop）
pub(crate) fn with_decoration_requests<T>(f: impl FnOnce(&mut Vec<DecorationRequest>) -> T) -> T {
    DECORATION_REQUESTS.with(|c| f(&mut c.borrow_mut()))
}

/// 写入其它已打开 buffer 快照（命令/事件入口调用，覆盖上次）
pub(crate) fn set_doc_snapshots(docs: Vec<DocSnapshot>) {
    DOC_SNAPSHOTS.with(|s| s.replace(docs));
}

/// 读取其它已打开 buffer 快照（by_path 查找）
pub(crate) fn with_doc_snapshots<T>(f: impl FnOnce(&Vec<DocSnapshot>) -> T) -> T {
    DOC_SNAPSHOTS.with(|s| f(&s.borrow()))
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

/// 访问 LOAD_STACK（加载中的脚本 key 栈；普通 Vec，随线程 drop）
pub(crate) fn with_load_stack<T>(f: impl FnOnce(&mut Vec<String>) -> T) -> T {
    LOAD_STACK.with(|s| f(&mut s.borrow_mut()))
}

pub(crate) fn push_load_root(root: PathBuf) {
    LOAD_ROOTS.with(|s| s.borrow_mut().push(root));
}

pub(crate) fn pop_load_root() {
    LOAD_ROOTS.with(|s| {
        s.borrow_mut().pop();
    });
}

pub(crate) fn clear_load_roots() {
    LOAD_ROOTS.with(|s| s.borrow_mut().clear());
}

/// 当前正在展开的插件由哪个根提供(无 → None)。**取栈顶**:嵌套加载时以最内层为准。
pub(crate) fn current_load_root() -> Option<PathBuf> {
    LOAD_ROOTS.with(|s| {
        s.borrow()
            .last()
            .filter(|p| !p.as_os_str().is_empty())
            .cloned()
    })
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

/// 当前打开的面板 id 列表（reload 时全部关闭——只关 last 会残留僵尸面板）。
/// 全局而非 thread_local：打开与 reload 命令可能在 tokio 不同线程执行，
/// thread_local 会因线程不同读不到列表 → 面板漏关。
pub(crate) fn with_open_panels<T>(f: impl FnOnce(&mut Vec<u64>) -> T) -> T {
    static OPEN_PANELS: OnceLock<Mutex<Vec<u64>>> = OnceLock::new();
    let mut v = OPEN_PANELS
        .get_or_init(Default::default)
        .lock()
        .expect("open panels lock");
    f(&mut v)
}

pub(crate) static MESSAGES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
/// 最近一次的**视口尺寸** (width, height) —— 由 `Compositor::resize` 推进来 ✓
/// 为什么需要:插件要"真正居中/铺满"就必须知道列数,而此前**没有任何接口**能读到 ✗
/// (曾据此猜过 `helix.width`/`helix.rows` ⇒ 都是 undefined ✗ —— 探针实测过)
pub(crate) static VIEWPORT: OnceLock<Mutex<(u16, u16)>> = OnceLock::new();

/// 记录视口尺寸(由核心在 `Compositor::resize` 里调用 ✓)
pub fn set_viewport(width: u16, height: u16) {
    let cell = VIEWPORT.get_or_init(|| Mutex::new((width, height)));
    if let Ok(mut v) = cell.lock() {
        *v = (width, height);
    }
}

/// 读取视口尺寸(JS 侧 `helix.viewport()` 用 ✓)
pub(crate) fn viewport() -> (u16, u16) {
    VIEWPORT
        .get()
        .and_then(|c| c.lock().ok().map(|v| *v))
        .unwrap_or((0, 0))
}

pub(crate) static UI_REQUESTS: OnceLock<Mutex<Vec<UiRequest>>> = OnceLock::new();
/// 插件目录（helix-term 启动时设置；js_load 相对名解析用）
/// 插件根目录(**有序**)。语义对齐 Neovim 的 `runtimepath`:自带 runtime 在前、
/// 用户目录在后,于是**后加的根覆盖先加的**(同名相对名取"最后一个存在的")。
/// 这是"把功能做成随软件分发的内置插件"的地基:内置放前、用户放后即可覆盖。
pub(crate) static PLUGIN_ROOTS: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// 设置插件根(只生效一次;已有则忽略 —— 与原先 `PLUGINS_DIR` 的语义一致)
pub fn set_plugin_roots(roots: Vec<PathBuf>) {
    let _ = PLUGIN_ROOTS.set(roots);
}

pub(crate) fn plugin_roots() -> &'static [PathBuf] {
    PLUGIN_ROOTS.get().map(|v| v.as_slice()).unwrap_or(&[])
}

/// 插件目录(**用户层 = 最后一个根**;`resolve_in` 用 `rev()` 迭代 ⇒ 末位优先级最高)。
/// `manifest.json` 与插件文件都在这里 —— `:plugin` 管理器必需
/// (此前 JS 侧**没有任何** config/插件目录访问器,实测 `config_dir`/`plugin_dir` 注册数为 0)。
pub fn plugins_dir() -> Option<PathBuf> {
    plugin_roots().last().cloned()
}

// ── 键入命令的参数通道(§12.0)──────────────────────────────────────
// 为什么是**独立通道**而不是给 `CommandContext` 加字段:它有 ~57 个构造点
// (3 处 typed.rs · 2 处 helix-js 非测试 · ≈52 处单测),加字段就是 57 处都要改 ——
// 正是本项目反复吃亏的"漏一处**不报错**"的形态。(同 `OpenScratchFile` 那次的取舍。)
thread_local! {
    static PENDING_COMMAND_ARGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// 分派器在调用 JS 命令**之前**设入(`run_plugin_command`)。
pub fn set_command_args(args: Vec<String>) {
    PENDING_COMMAND_ARGS.with(|a| *a.borrow_mut() = args);
}

/// **调用之后必须清空** —— 否则参数会"粘"到下一个命令,而那类 bug 极难查。
pub fn clear_command_args() {
    PENDING_COMMAND_ARGS.with(|a| a.borrow_mut().clear());
}

/// 当前待处理参数(JS 侧 `helix.command_args()` 读它)
pub fn command_args() -> Vec<String> {
    PENDING_COMMAND_ARGS.with(|a| a.borrow().clone())
}

/// 自带 runtime 目录(**有序**;由 helix-term 启动时推入)。
///
/// 为什么必须"推入"而不是自己取:`helix-js` **不依赖 `helix-loader`**(实测其 Cargo.toml),
/// 所以拿不到 runtime 目录 —— 与 `PLUGIN_ROOTS` / `LAYOUTS_DIR` 同一模式。
pub(crate) static RUNTIME_DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();

pub fn set_runtime_dirs(dirs: Vec<PathBuf>) {
    let _ = RUNTIME_DIRS.set(dirs);
}

/// `name` → runtime 里的绝对路径(**按序取第一个存在的**)。
/// 纯函数(吃 dirs)便于单测 —— 同 `resolve_in` / `plugin_roots_for`。
pub(crate) fn runtime_file_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    if Path::new(name).is_absolute() {
        return Path::new(name).exists().then(|| PathBuf::from(name));
    }
    dirs.iter().map(|d| d.join(name)).find(|p| p.exists())
}

/// 给 JS 侧用的包装:`helix.runtime_path(name)`
pub fn runtime_path(name: &str) -> Option<PathBuf> {
    runtime_file_in(
        RUNTIME_DIRS.get().map(|v| v.as_slice()).unwrap_or(&[]),
        name,
    )
}

/// 加载参数 → **候选 key(按序尝试)**。
///
/// 约定见 `docs/plugin-layout.md` §3.3:
/// - 带 `.js` → 按**文件路径**(后门:一次性脚本/调试/共享库)
/// - 含 `/` 但没 `.js` → 补 `.js`(旧行为,共享库走这类)
/// - **裸名** → 先按约定找**插件入口** `<name>/plugin.js`,找不到再回退旧的 `<name>.js`
///   (回退这一条是为了**不破坏**任何现有加载:约定是新增的第二条路,不是替换)
pub(crate) fn entry_keys(name: &str) -> Vec<String> {
    if name.ends_with(".js") {
        return vec![name.to_string()];
    }
    if name.contains('/') {
        return vec![format!("{name}.js")];
    }
    vec![format!("{name}/plugin.js"), format!("{name}.js")]
}

/// 相对名 → 绝对路径的**纯函数**(便于单测,不碰全局)。
///
/// 从**后往前**找第一个存在的:后加的根覆盖先加的(用户覆盖内置)。
/// 一个都不存在时回落到**最后一个根** —— 报错信息指向用户目录,更贴近实际。
pub(crate) fn resolve_in(roots: &[PathBuf], key: &str) -> Option<PathBuf> {
    if Path::new(key).is_absolute() {
        return Some(PathBuf::from(key));
    }
    for r in roots.iter().rev() {
        let p = r.join(key);
        if p.exists() {
            return Some(p);
        }
    }
    roots.last().map(|r| r.join(key))
}

// ── 插件文件删除(§12.2 的唯一新接口)──────────────────────────────
// **必须限域**:`remove_files` 那类操作的输入来自 manifest(可被插件/用户改动),
// 若允许任意路径,一次笔误就能删掉插件目录之外的东西。所以:
//   ① 只接受**相对路径**;绝对路径直接拒绝
//   ② 拒绝任何含 `..` 的段(防穿越)
//   ③ 解析**只走插件根**(与加载同一套多根),命中后删除
// 返回:Ok(true) 删掉了 · Ok(false) 本来就不存在 · Err(原因)

/// 纯函数版(吃 roots)便于单测:校验 + 限域删除。
pub(crate) fn remove_plugin_file_in(roots: &[PathBuf], rel: &str) -> Result<bool, String> {
    let bad = |why: &str| Err(format!("helix.remove_plugin_file({rel:?}): {why}"));
    if rel.is_empty() {
        return bad("路径不能为空");
    }
    if Path::new(rel).is_absolute() {
        return bad("只接受相对路径");
    }
    if Path::new(rel)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return bad("路径不能含 `..`");
    }
    // 从后往前找(与 resolve_in 同序:用户层覆盖内置层)
    for r in roots.iter().rev() {
        let p = r.join(rel);
        if !p.exists() {
            continue;
        }
        // 双保险:解析后仍须落在该根之下(符号链接/奇怪路径的最后一道闸)
        let real_root = r.canonicalize().map_err(|e| format!("{e}"))?;
        let real_p = p.canonicalize().map_err(|e| format!("{e}"))?;
        if !real_p.starts_with(&real_root) {
            return bad("解析后落在插件根之外");
        }
        return if real_p.is_dir() {
            std::fs::remove_dir_all(&real_p)
                .map(|_| true)
                .map_err(|e| format!("{e}"))
        } else {
            std::fs::remove_file(&real_p)
                .map(|_| true)
                .map_err(|e| format!("{e}"))
        };
    }
    Ok(false) // 本来就不存在 → 幂等
}

/// 用当前插件根
pub fn remove_plugin_file(rel: &str) -> Result<bool, String> {
    remove_plugin_file_in(plugin_roots(), rel)
}

// ── 插件文件写入(§12.2:`install` 需要)────────────────────────────
// 与删除同一套限域:**相对路径** · 拒 `..` · 只落插件根之内。
// 不同点:写入有明确目标 —— **用户层(最后一个根)**,因为覆盖优先级在末位
// (见 `resolve_in` 的 `rev()`),装到内置层既无意义也可能没权限。
// 父目录**按需创建**:实测 `helix-js` 原本没有任何建目录 API,连单文件安装都做不了。

/// 纯函数版(吃 roots)便于单测:校验 + 写入 + 按需建父目录。返回写入的绝对路径。
pub(crate) fn write_plugin_file_in(
    roots: &[PathBuf],
    rel: &str,
    text: &str,
) -> Result<PathBuf, String> {
    let bad = |why: &str| Err(format!("helix.write_plugin_file({rel:?}): {why}"));
    if rel.is_empty() {
        return bad("路径不能为空");
    }
    if Path::new(rel).is_absolute() {
        return bad("只接受相对路径");
    }
    if Path::new(rel)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return bad("路径不能含 `..`");
    }
    let root = roots
        .last()
        .ok_or_else(|| "helix.write_plugin_file: 没有可写的插件根(未推入?)".to_string())?;
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, text).map_err(|e| e.to_string())?;
    Ok(p)
}

/// 用当前插件根
pub fn write_plugin_file(rel: &str, text: &str) -> Result<PathBuf, String> {
    write_plugin_file_in(plugin_roots(), rel, text)
}

/// 相对名 → **(绝对路径, 提供它的根)**。从后往前找第一个存在的 = 后加的根覆盖先加的。
/// 绝对路径没有"根"的概念(返回空 Path)。
pub(crate) fn resolve_in_with_root(roots: &[PathBuf], key: &str) -> Option<(PathBuf, PathBuf)> {
    if Path::new(key).is_absolute() {
        return Some((PathBuf::from(key), PathBuf::new()));
    }
    for r in roots.iter().rev() {
        let p = r.join(key);
        if p.exists() {
            return Some((p, r.clone()));
        }
    }
    roots.last().map(|r| (r.join(key), r.clone()))
}

/// **目录级覆盖(规格 §4-2)**:优先在 `pref`(提供当前插件的那个根)内解析,
/// 找不到再按正常多根顺序。
///
/// 语义取舍:这里选的是 **prefer(自身根优先)** 而不是 strict(只在自身根内)。
/// 因为共享库是**独立插件**(§4-3 的 deps 用插件名),strict 会让用户插件
/// 用不到内置的共享库。prefer 已经解决"用户的 plugin.js + 内置的同名 sibling"这种
/// 版本不匹配的混合体 —— 那是 §4-2 真正要治的病。
pub(crate) fn resolve_in_pref(
    roots: &[PathBuf],
    key: &str,
    pref: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(pref) = pref {
        if !pref.as_os_str().is_empty() {
            let p = pref.join(key);
            if p.exists() {
                return Some(p);
            }
        }
    }
    resolve_in(roots, key)
}

/// 相对名 → 绝对路径(用当前插件根)
pub fn resolve_plugin_path(key: &str) -> Option<PathBuf> {
    // 目录级覆盖(§4-2):优先在"提供当前插件的那个根"内解析
    let pref = current_load_root();
    resolve_in_pref(plugin_roots(), key, pref.as_deref())
}
/// 布局树序列化缓存（helix-term 树变更时写入；get_layout 读取）
pub(crate) static LAST_LAYOUT: OnceLock<Mutex<String>> = OnceLock::new();
/// 打开文档序列化缓存（helix-term 每帧写入；buffers/current_buffer 读取）
pub(crate) static BUFFERS: OnceLock<Mutex<String>> = OnceLock::new();

/// 平级模式快照(JSON: `{mode, keys:[{key,desc,enabled,reason?}]}`)。
/// helix-term 在模式切换时写入;`helix.pane_mode.current()/keymap()` 读。
pub(crate) static PANE_MODE: OnceLock<Mutex<String>> = OnceLock::new();

/// pane 清单快照(JSON: `{panes:[{id,place,focused,fixed,pinned,z?,rect?}]}`)。
/// 布局变更时由 helix-term 写入;`helix.pane.list()` 读。
pub(crate) static PANES: OnceLock<Mutex<String>> = OnceLock::new();
/// 插件配置合并缓存（helix-term config load 后写入；get_config 读取）JSON: {"<name>": {...}}
pub(crate) static CONFIGS: OnceLock<Mutex<String>> = OnceLock::new();
thread_local! {
    // 插件配置 schema 注册表（define_config 写入；build_plugin_configs 读取）name → schema JSON
    static CONFIG_SCHEMAS: RefCell<Option<&'static mut HashMap<String, String>>> = const { RefCell::new(None) };
}
/// 当前文档诊断序列化缓存（helix-term 每帧写入；diagnostics 读取）
pub(crate) static DIAGNOSTICS: OnceLock<Mutex<String>> = OnceLock::new();

/// 组件状态提供者表(组件 id → JSON 生成器)
type ComponentStateMap = HashMap<u64, Box<dyn Fn(u64) -> String>>;

/// 终端实例注册表（pty_id → (view_id, cmd)；term_kill(pty_id) 清理、term_list 查询）
static OPEN_TERMS: OnceLock<Mutex<HashMap<u64, (u64, String)>>> = OnceLock::new();

pub(crate) fn register_term(pty_id: u64, view_id: u64, cmd: String) {
    OPEN_TERMS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("open terms lock")
        .insert(pty_id, (view_id, cmd));
}

pub(crate) fn unregister_term(pty_id: u64) {
    OPEN_TERMS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("open terms lock")
        .remove(&pty_id);
}

/// 当前打开的终端列表（view_id, cmd）。键是 pty_id：term_kill(pty_id) 能删对。
pub(crate) fn list_terms() -> Vec<(u64, String)> {
    OPEN_TERMS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("open terms lock")
        .iter()
        .map(|(_, (view_id, cmd))| (*view_id, cmd.clone()))
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
    // 异步 fs 的 promise 解析函数（id → resolve/reject）；内容泄漏（同上）
    static ASYNC_PROMISES: RefCell<Option<&'static mut HashMap<u64, ResolvingFunctions>>> =
        const { RefCell::new(None) };
    static NEXT_ASYNC_ID: Cell<u64> = const { Cell::new(1) };
    // helix.lsp.* 请求的 promise 解析函数（id → resolve/reject）；内容泄漏（同上）
    static LSP_PROMISES: RefCell<Option<&'static mut HashMap<u64, ResolvingFunctions>>> =
        const { RefCell::new(None) };
    // LSP 响应通道：与 ASYNC_EVENTS 同模式——发起线程（helix-term 泵）克隆 Sender 给
    // tokio 任务发结果，主线程 drain 消费。
    static LSP_RESULTS: RefCell<Option<WakeSender<crate::lsp::LspResult>>> = const { RefCell::new(None) };
    static LSP_RESULTS_RX: RefCell<Option<std::sync::mpsc::Receiver<crate::lsp::LspResult>>> =
        const { RefCell::new(None) };
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
pub(crate) fn with_term_masters<T>(
    f: impl FnOnce(&mut HashMap<u64, std::os::fd::RawFd>) -> T,
) -> T {
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

/// 访问 LSP_RESULTS 发送端（init 时创建；helix-term tokio 任务克隆持有）
pub(crate) fn with_lsp_results<T>(
    f: impl FnOnce(&mut Option<WakeSender<crate::lsp::LspResult>>) -> T,
) -> T {
    LSP_RESULTS.with(|t| f(&mut t.borrow_mut()))
}

pub(crate) fn with_lsp_results_rx<T>(
    f: impl FnOnce(&mut Option<std::sync::mpsc::Receiver<crate::lsp::LspResult>>) -> T,
) -> T {
    LSP_RESULTS_RX.with(|t| f(&mut t.borrow_mut()))
}

/// 访问 TERM_CALLBACKS：同上（内容泄漏）
pub(crate) fn with_terms<T>(f: impl FnOnce(&mut HashMap<u64, TermCallbacks>) -> T) -> T {
    TERM_CALLBACKS.with(|t| {
        let mut slot = t.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 TERM_PROMISES（run_async 的 promise 解析函数注册表）：同上（内容泄漏）
pub(crate) fn with_term_promises<T>(
    f: impl FnOnce(&mut HashMap<u64, ResolvingFunctions>) -> T,
) -> T {
    TERM_PROMISES.with(|t| {
        let mut slot = t.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 ASYNC_PROMISES（异步 fs 的 promise 解析函数注册表）：同上（内容泄漏）
pub(crate) fn with_async_promises<T>(
    f: impl FnOnce(&mut HashMap<u64, ResolvingFunctions>) -> T,
) -> T {
    ASYNC_PROMISES.with(|t| {
        let mut slot = t.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 LSP_PROMISES（helix.lsp.* 的 promise 解析函数注册表）：同上（内容泄漏）
pub(crate) fn with_lsp_promises<T>(
    f: impl FnOnce(&mut HashMap<u64, ResolvingFunctions>) -> T,
) -> T {
    LSP_PROMISES.with(|t| {
        let mut slot = t.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 取走并清空 echo 消息队列
pub fn take_messages() -> Vec<String> {
    crate::init();
    std::mem::take(
        &mut *MESSAGES
            .get()
            .expect("MESSAGES not initialized")
            .lock()
            .expect("messages lock poisoned"),
    )
}

/// 布局文件目录(`<config>/layouts`)。由 helix-term 启动时设置(同 PLUGIN_ROOTS 的做法)。
pub(crate) static LAYOUTS_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_layouts_dir(dir: PathBuf) {
    let _ = LAYOUTS_DIR.set(dir);
}

pub(crate) fn layouts_dir() -> Option<&'static PathBuf> {
    LAYOUTS_DIR.get()
}

/// 读当前布局 dump(还没缓存过 → 空串)
pub fn layout_json() -> String {
    LAST_LAYOUT
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default()
}

pub fn cache_layout(json: &str) {
    let _ = LAST_LAYOUT.get_or_init(Default::default);
    if let Some(m) = LAST_LAYOUT.get() {
        *m.lock().unwrap() = json.to_string();
    }
}

/// 写入打开文档序列化缓存(helix-term 每帧更新;buffers/current_buffer 读取)
pub fn cache_buffers(json: &str) {
    let _ = BUFFERS.get_or_init(Default::default);
    let _ = PANE_MODE.get_or_init(Default::default);
    let _ = PANES.get_or_init(Default::default);
    if let Some(m) = BUFFERS.get() {
        *m.lock().unwrap() = json.to_string();
    }
}

/// 写入平级模式快照(模式切换时由 helix-term 调用)
/// 写入 pane 清单快照(布局变更时由 helix-term 调用)
pub fn cache_panes(json: &str) {
    let _ = PANES.get_or_init(Default::default);
    if let Some(m) = PANES.get() {
        *m.lock().unwrap() = json.to_string();
    }
}

pub fn cache_pane_mode(json: &str) {
    let _ = PANE_MODE.get_or_init(Default::default);
    if let Some(m) = PANE_MODE.get() {
        *m.lock().unwrap() = json.to_string();
    }
}

/// 写入当前文档诊断序列化缓存(helix-term 每帧更新;diagnostics 读取)
pub fn cache_diagnostics(json: &str) {
    let _ = DIAGNOSTICS.get_or_init(Default::default);
    if let Some(m) = DIAGNOSTICS.get() {
        *m.lock().unwrap() = json.to_string();
    }
}

/// 取走并清空 UI 请求队列
pub fn take_ui_requests() -> Vec<UiRequest> {
    crate::init();
    std::mem::take(
        &mut *UI_REQUESTS
            .get()
            .expect("UI_REQUESTS initialized")
            .lock()
            .expect("ui requests lock"),
    )
}

#[cfg(test)]
mod entry_key_tests {
    use super::*;

    /// 加载参数 → 候选 key:三种形态各自的分支(约定 §3.3)
    #[test]
    fn entry_keys_follow_layout_convention() {
        // 裸名 → 先插件入口,再回退旧式 .js
        assert_eq!(
            entry_keys("filetree"),
            vec!["filetree/plugin.js".to_string(), "filetree.js".to_string()],
            "裸名:约定优先,旧式回退"
        );
        // 带 .js → 原样(文件路径后门)
        assert_eq!(entry_keys("lib/icons.js"), vec!["lib/icons.js".to_string()]);
        // 含 / 无 .js → 补 .js(旧行为)
        assert_eq!(
            entry_keys("features/terminal"),
            vec!["features/terminal.js".to_string()]
        );
    }

    /// 裸名解析真的落到 `<name>/plugin.js`(且**用户根覆盖内置根**)
    #[test]
    fn bare_name_resolves_to_plugin_entry_with_user_override() {
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let mk = |dir: &std::path::Path, rel: &str, tag: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, tag).unwrap();
        };
        mk(bundled.path(), "filetree/plugin.js", "bundled");
        mk(user.path(), "filetree/plugin.js", "user");
        mk(bundled.path(), "only-bundled/plugin.js", "b");
        let roots = vec![bundled.path().to_path_buf(), user.path().to_path_buf()];

        let key = entry_keys("filetree")[0].clone();
        let got = resolve_in(&roots, &key).unwrap();
        assert_eq!(
            std::fs::read_to_string(&got).unwrap(),
            "user",
            "用户根必须覆盖内置根"
        );

        // 旧式 `<name>.js` 仍可用(只有它存在时回退到它)
        mk(bundled.path(), "legacy.js", "old");
        assert!(resolve_in(&roots, &entry_keys("legacy")[1])
            .unwrap()
            .exists());
        assert!(
            !resolve_in(&roots, &entry_keys("legacy")[0])
                .unwrap()
                .exists(),
            "legacy 没有 <name>/plugin.js,所以候选 0 不存在 → 加载器会回退候选 1"
        );
    }
}

#[cfg(test)]
mod dir_override_tests {
    use super::*;

    /// §4-2 要治的病:**用户的 plugin.js 不能配内置的同名 sibling**
    #[test]
    fn prefer_root_keeps_plugin_files_together() {
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let mk = |d: &std::path::Path, rel: &str, tag: &str| {
            let p = d.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, tag).unwrap();
        };
        // 同一插件在两层的文件
        mk(bundled.path(), "plug/plugin.js", "bundled-main");
        mk(bundled.path(), "plug/helper.js", "bundled-helper");
        mk(user.path(), "plug/plugin.js", "user-main");
        let roots = vec![bundled.path().to_path_buf(), user.path().to_path_buf()];

        // 入口:用户层胜(后加的根覆盖)
        let (entry, root) = resolve_in_with_root(&roots, "plug/plugin.js").unwrap();
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), "user-main");
        assert_eq!(root, user.path(), "提供根 = 用户层");

        // 关键:该插件的**其它文件**也必须来自同一个根 —— 而不是回落到内置的 bundled-helper
        // (没有 §4-2 时,helper.js 只在内置层存在 → 会被解析成 bundled-helper,
        //  于是造出"用户的 main + 内置的 helper"这种版本不匹配的混合体)
        let helper = resolve_in_pref(&roots, "plug/helper.js", Some(user.path())).unwrap();
        assert_eq!(
            std::fs::read_to_string(&helper).unwrap(),
            "bundled-helper",
            "用户层没有 helper → prefer 语义下回落到内置(共享/缺省可回落)"
        );
        // 而**用户层有**该文件时,prefer 必须保住同一层
        mk(user.path(), "plug/helper.js", "user-helper");
        let helper = resolve_in_pref(&roots, "plug/helper.js", Some(user.path())).unwrap();
        assert_eq!(
            std::fs::read_to_string(&helper).unwrap(),
            "user-helper",
            "两层都有时,prefer 必须选与入口同层的那份"
        );
    }

    /// 无提供根 / 绝对路径时退回正常顺序
    #[test]
    fn prefer_falls_back_without_root() {
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(bundled.path().join("plug")).unwrap();
        std::fs::write(bundled.path().join("plug/x.js"), "b").unwrap();
        let roots = vec![bundled.path().to_path_buf(), user.path().to_path_buf()];
        let got = resolve_in_pref(&roots, "plug/x.js", None).unwrap();
        assert_eq!(std::fs::read_to_string(&got).unwrap(), "b");
        // 绝对路径:提供根为空 → 不参与 prefer
        let (p, r) = resolve_in_with_root(&roots, "/tmp/abs.js").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/abs.js"));
        assert!(r.as_os_str().is_empty(), "绝对路径没有提供根");
    }
}

#[cfg(test)]
mod runtime_path_tests {
    use super::*;

    /// 按序取第一个存在的;都不存在 → None;绝对路径按存在性判定
    #[test]
    fn runtime_file_resolution() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(b.path().join("tutor"), "T").unwrap();
        std::fs::write(a.path().join("only-a"), "A").unwrap();
        let dirs = vec![a.path().to_path_buf(), b.path().to_path_buf()];

        // 只有后者有 → 仍能解析到(按序但不要求必须在第一个)
        let got = runtime_file_in(&dirs, "tutor").unwrap();
        assert_eq!(std::fs::read_to_string(&got).unwrap(), "T");
        assert_eq!(
            runtime_file_in(&dirs, "only-a").unwrap(),
            a.path().join("only-a")
        );
        assert!(runtime_file_in(&dirs, "nope").is_none(), "都不存在 → None");
        // 绝对路径:存在 → 原样;不存在 → None
        let abs = b.path().join("tutor");
        assert_eq!(runtime_file_in(&dirs, abs.to_str().unwrap()).unwrap(), abs);
        assert!(runtime_file_in(&dirs, "/definitely/not/here").is_none());
        // 空目录列表 → None(不会 panic)
        assert!(runtime_file_in(&[], "tutor").is_none());
    }
}

#[cfg(test)]
mod command_args_tests {
    use super::*;

    /// 设 → 读 → **清空后必须为空**(防"粘到下一个命令")
    #[test]
    fn command_args_roundtrip_and_clear() {
        clear_command_args();
        assert!(command_args().is_empty(), "初始为空");
        set_command_args(vec!["save".into(), "dev".into()]);
        assert_eq!(command_args(), vec!["save".to_string(), "dev".to_string()]);
        // 再设会整体替换(不是累加)
        set_command_args(vec!["list".into()]);
        assert_eq!(command_args(), vec!["list".to_string()]);
        clear_command_args();
        assert!(
            command_args().is_empty(),
            "清空后为空 —— 否则会粘到下一个命令"
        );
    }
}

#[cfg(test)]
mod remove_plugin_file_tests {
    use super::*;

    /// 限域删除的三条硬要求:**只相对路径** · **拒 `..`** · **只动插件根内**
    #[test]
    fn scoped_removal_rejects_escapes() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("precious.txt");
        std::fs::write(&victim, "x").unwrap();
        std::fs::create_dir_all(root.path().join("plug")).unwrap();
        std::fs::write(root.path().join("plug/plugin.js"), "// x").unwrap();
        let roots = vec![root.path().to_path_buf()];

        // 绝对路径 → 拒
        assert!(remove_plugin_file_in(&roots, victim.to_str().unwrap()).is_err());
        assert!(victim.is_file(), "绝对路径必须被拒,且不动文件");
        // 含 `..` → 拒(即便它算出来真的指向外面)
        for rel in ["../precious.txt", "plug/../../precious.txt", "a/../b"] {
            assert!(remove_plugin_file_in(&roots, rel).is_err(), "应拒: {rel}");
        }
        assert!(victim.is_file(), "穿越尝试不得动到根外的文件");
        // 空路径 → 拒
        assert!(remove_plugin_file_in(&roots, "").is_err());

        // 合法:删根内的文件 → true;再删 → false(幂等)
        assert_eq!(
            remove_plugin_file_in(&roots, "plug/plugin.js").unwrap(),
            true
        );
        assert!(!root.path().join("plug/plugin.js").exists());
        assert_eq!(
            remove_plugin_file_in(&roots, "plug/plugin.js").unwrap(),
            false
        );
        // 目录也能删(remove_orphan 要删整棵插件目录)
        std::fs::create_dir_all(root.path().join("plug2/sub")).unwrap();
        std::fs::write(root.path().join("plug2/sub/a.js"), "x").unwrap();
        assert_eq!(remove_plugin_file_in(&roots, "plug2").unwrap(), true);
        assert!(!root.path().join("plug2").exists());
    }
}

#[cfg(test)]
mod write_plugin_file_tests {
    use super::*;

    /// 限域写入的三条硬要求:**只相对路径** · **拒 `..`** · **只落用户层(最后一个根)**
    /// 以及**父目录按需创建**(这是它存在的原因:`install` 要往还不存在的目录里写)
    #[test]
    fn scoped_write_rejects_escapes_and_creates_parents() {
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let roots = vec![bundled.path().to_path_buf(), user.path().to_path_buf()];

        // 拒绝 + **拒绝时两个根都不得被写入**
        for rel in ["/abs/x.js", "../x.js", "a/../../x.js", ""] {
            assert!(
                write_plugin_file_in(&roots, rel, "x").is_err(),
                "应拒: {rel:?}"
            );
        }
        assert!(!bundled.path().join("x.js").exists() && !user.path().join("x.js").exists());

        // 合法:父目录按需创建;落**用户层**
        let p = write_plugin_file_in(&roots, "my/plug/plugin.js", "// hi").unwrap();
        assert!(
            p.starts_with(user.path()),
            "应写入用户层(最后一个根),实得 {p:?}"
        );
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "// hi");
        assert!(!bundled.path().join("my").exists(), "不得写进内置层");

        // 空根列表 → 报错而非 panic
        assert!(write_plugin_file_in(&[], "x.js", "x").is_err());
    }
}
