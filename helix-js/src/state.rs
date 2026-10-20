//! 全局状态：thread_local 引擎/注册表/队列 + 跨线程事件通道 + 访问器。
//! 所有状态集中于此，其他模块经 `with_*` 访问器读写（thread_local 无法跨模块直接访问）。

use boa_engine::builtins::promise::ResolvingFunctions;
use boa_engine::JsValue;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// 加载缓存 / 已加载脚本清单已按宿主分桶(T5-b1),见 host.rs
pub(crate) use crate::host::{with_loaded_scripts, with_script_exports};

use crate::types::{
    AsyncEvent, CursorRequest, DecorationRequest, DocSnapshot, Edit, NodeHandlers, PopupCallbacks,
    TermCallbacks, TermCtrl, TermEvent, UiRequest,
};

/// T7 诊断开关：`HX_DBG_JS_THREAD=1` 时把"通道建在哪个线程 / 在哪个线程 spawn / 在哪个线程 drain"
/// 打到 stderr。**只打印，不改任何行为**（默认关闭 ⇒ 零开销、零语义变化）。
///
/// 用途：定位 1/5 概率的"事件丢了"。当前假说（`plugin_async.rs` 文件头）是"主任务迁移 OS 线程"，
/// 但 `Application` 持有 boa `Context`（`!Send`）⇒ 该假说站不住，必须靠日志找出真正的第二线程。
pub(crate) fn js_dbg(args: std::fmt::Arguments<'_>) {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("HX_DBG_JS_THREAD").is_some()) {
        eprintln!("[jsdbg tid={:?}] {}", std::thread::current().id(), args);
    }
}

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

thread_local! {}

// 引擎的存储/借出/归还已搬进 `host.rs`:
//   所有权 → `JsHost`;排他/重入检测(引擎的 `RefCell` + DEFAULT_HOST)→ `host::with_engine`
pub(crate) use crate::host::with_engine;

/// 访问 REGISTRY：首次触达惰性 Box::leak 创建；线程退出时内容泄漏（见上）
pub(crate) fn with_registry<T>(f: impl FnOnce(&mut HashMap<String, JsValue>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.registry.borrow_mut()))
}

/// 同 [`with_registry`],但**没有宿主时给 `None`**(查询类入口不得为了回答而建宿主)。
pub(crate) fn try_with_registry<T>(
    f: impl FnOnce(&mut HashMap<String, JsValue>) -> T,
) -> Option<T> {
    crate::host::try_with_host_ref(|h| f(&mut h.registry.borrow_mut()))
}

/// 访问 LAST_EXPORT：同上（内容泄漏）
pub(crate) fn with_last_export<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.last_export.borrow_mut()))
}

/// 当前 eval 脚本声明的依赖（helix.plugin deps 累积处；load eval 后 take 递归加载）
pub(crate) fn with_last_plugin_deps<T>(f: impl FnOnce(&mut Vec<String>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| {
        let c = &h.last_plugin_deps;
        f(&mut c.borrow_mut())
    })
}

/// 插件配置 schema 注册表(进程级,与 `CONFIGS` 同域)。
/// 为什么不是 host/thread_local:合并缓存 `CONFIGS` 是进程全局的,注册表若按线程存,
/// 另一个线程重建时看不到 schema ⇒ 全局缓存被写成 `{}`(集成测试多 app 并行时实测踩到;
/// 多线程重建 + 全局缓存的组合本身就是错的)。
static CONFIG_SCHEMAS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

/// 访问 CONFIG_SCHEMAS
pub(crate) fn with_config_schemas<T>(f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    let m = CONFIG_SCHEMAS.get_or_init(|| Mutex::new(HashMap::new()));
    f(&mut m.lock().unwrap())
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

/// 用户 config.toml `[plugins]` 原始覆盖（JSON: `{"<name>": {...}}`）。
/// config load / :config-reload 时由 helix-term 写入 ⇒ 插件 load/reload 后重建配置不必再碰
/// helix-term 的 Config（`Editor::config()` 只含 editor 段，拿不到 plugins 表）。
pub(crate) static PLUGIN_OVERRIDES: OnceLock<Mutex<String>> = OnceLock::new();

/// 写入用户覆盖（helix-term `build_plugin_configs` 调）
pub fn cache_plugin_overrides(json: &str) {
    let m = PLUGIN_OVERRIDES.get_or_init(|| Mutex::new("{}".to_string()));
    *m.lock().unwrap() = json.to_string();
}

/// 读取用户覆盖（未写入过 ⇒ `{}`）
pub(crate) fn plugin_overrides() -> String {
    PLUGIN_OVERRIDES
        .get_or_init(|| Mutex::new("{}".to_string()))
        .lock()
        .unwrap()
        .clone()
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
    crate::host::with_host_ref_or_init(|h| f(&mut h.event_handlers.borrow_mut()))
}

/// 同 [`with_event_handlers`],但**没有宿主时给 `None`**(查询类入口不得为了回答而建宿主)。
pub(crate) fn try_with_event_handlers<T>(
    f: impl FnOnce(&mut HashMap<String, Vec<JsValue>>) -> T,
) -> Option<T> {
    crate::host::try_with_host_ref(|h| f(&mut h.event_handlers.borrow_mut()))
}

/// 注册组件状态提供者：组件 id → 序列化为 JSON 的闭包（helix-term 侧调用）。
pub fn register_component_state(id: u64, f: impl Fn(u64) -> String + 'static) {
    crate::host::with_host_ref_or_init(|h| {
        let s = &h.component_states;
        {
            s.borrow_mut().insert(id, Box::new(f));
        }
    })
}

/// 注销组件状态提供者（组件 Drop 时调用）。
pub fn unregister_component_state(id: u64) {
    crate::host::with_host_ref_or_init(|h| {
        let s = &h.component_states;
        {
            s.borrow_mut().remove(&id);
        }
    })
}

/// 读取组件状态（JSON 字符串；未注册 → None）。
pub(crate) fn get_component_state_json(id: u64) -> Option<String> {
    crate::host::with_host_ref_or_init(|h| {
        let s = &h.component_states;
        s.borrow().get(&id).map(|f| f(id))
    })
}

/// 访问 POPUPS：同上（内容泄漏）
pub(crate) fn with_popups<T>(f: impl FnOnce(&mut HashMap<u64, PopupCallbacks>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.popups.borrow_mut()))
}

/// 同 [`with_popups`],但**没有宿主时给 `None`**(查询类入口不得为了回答而建宿主)。
pub(crate) fn try_with_popups<T>(
    f: impl FnOnce(&mut HashMap<u64, PopupCallbacks>) -> T,
) -> Option<T> {
    crate::host::try_with_host_ref(|h| f(&mut h.popups.borrow_mut()))
}

/// 访问 BUFFER_ICON_HOOK：同上（内容泄漏）
pub(crate) fn with_buffer_icon_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.buffer_icon_hook.borrow_mut()))
}

/// 访问 COMPLETION_ICON_HOOK：同上（内容泄漏）
pub(crate) fn with_completion_icon_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.completion_icon_hook.borrow_mut()))
}

/// 访问 COMPLETION_RENDER_HOOK：同上（内容泄漏）
pub(crate) fn with_completion_render_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.completion_render_hook.borrow_mut()))
}

/// 访问 PICKER_SOURCES：同上（内容泄漏）
pub(crate) fn with_picker_sources<T>(f: impl FnOnce(&mut HashMap<String, JsValue>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.picker_sources.borrow_mut()))
}

/// 访问 KEYMAP_HINT_HOOK：同上（内容泄漏）
pub(crate) fn with_keymap_hint_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.keymap_hint_hook.borrow_mut()))
}

/// 同 [`with_keymap_hint_hook`],但**没有宿主时给 `None`**(查询类入口不得为了回答而建宿主)。
pub(crate) fn try_with_keymap_hint_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> Option<T> {
    crate::host::try_with_host_ref(|h| f(&mut h.keymap_hint_hook.borrow_mut()))
}

/// 访问 STATUSLINE_HOOK：同上（内容泄漏）
pub(crate) fn with_statusline_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.statusline_hook.borrow_mut()))
}

pub(crate) fn set_statusline_replace(v: bool) {
    crate::host::with_host_ref_or_init(|h| {
        let r = &h.statusline_replace;
        r.set(v)
    });
}

pub fn statusline_replaces() -> bool {
    crate::host::with_host_ref_or_init(|h| {
        let r = &h.statusline_replace;
        r.get()
    })
}

/// 设置 replace 模式区域比例 [左, 中, 右]（0 允许：该区不分配宽度）
pub(crate) fn set_statusline_zones(zones: [u16; 3]) {
    crate::host::with_host_ref_or_init(|h| {
        let z = &h.statusline_zones;
        *z.borrow_mut() = zones
    });
}

/// replace 模式区域比例 [左, 中, 右]
pub fn statusline_zones() -> [u16; 3] {
    crate::host::with_host_ref_or_init(|h| {
        let z = &h.statusline_zones;
        *z.borrow()
    })
}

/// 访问 NODE_HANDLERS：同上（内容泄漏）
pub(crate) fn with_node_handlers<T>(
    f: impl FnOnce(&mut HashMap<(u64, String), NodeHandlers>) -> T,
) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.node_handlers.borrow_mut()))
}

/// 访问批量编辑事务深度(普通 usize,随线程 drop)
pub(crate) fn with_txn_depth<T>(f: impl FnOnce(&mut usize) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| {
        let d = &h.edit_txn_depth;
        {
            let mut v = d.get();
            let out = f(&mut v);
            d.set(v);
            out
        }
    })
}

/// 访问 CURRENT_EDITS（宿主字段;1b-3c 前为 thread_local）
pub(crate) fn with_edits<T>(f: impl FnOnce(&mut Vec<Edit>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| {
        let e = &h.current_edits;
        f(&mut e.borrow_mut())
    })
}

/// 访问 DECORATION_REQUESTS（宿主字段;1b-3c 前为 thread_local）
pub(crate) fn with_decoration_requests<T>(f: impl FnOnce(&mut Vec<DecorationRequest>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| {
        let c = &h.decoration_requests;
        f(&mut c.borrow_mut())
    })
}

/// 写入其它已打开 buffer 快照（命令/事件入口调用，覆盖上次）
pub(crate) fn set_doc_snapshots(docs: Vec<DocSnapshot>) {
    crate::host::with_host_ref_or_init(|h| {
        let s = &h.doc_snapshots;
        s.replace(docs)
    });
}

/// 读取其它已打开 buffer 快照（by_path 查找）
pub(crate) fn with_doc_snapshots<T>(f: impl FnOnce(&Vec<DocSnapshot>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| {
        let s = &h.doc_snapshots;
        f(&s.borrow())
    })
}

/// 访问 CURSOR_REQUESTS（宿主字段;1b-3c 前为 thread_local）
pub(crate) fn with_cursor_requests<T>(f: impl FnOnce(&mut Vec<CursorRequest>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| {
        let c = &h.cursor_requests;
        f(&mut c.borrow_mut())
    })
}

/// 访问 COMMAND_DOCS（宿主字段;1b-3c 前为 thread_local）
pub(crate) fn with_command_docs<T>(f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| {
        let d = &h.command_docs;
        f(&mut d.borrow_mut())
    })
}

/// 访问 LOAD_STACK（加载中的脚本 key 栈；普通 Vec，随线程 drop）
pub(crate) fn with_load_stack<T>(f: impl FnOnce(&mut Vec<String>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| {
        let s = &h.load_stack;
        f(&mut s.borrow_mut())
    })
}

pub(crate) fn push_load_root(root: PathBuf) {
    crate::host::with_host_ref_or_init(|h| {
        let s = &h.load_roots;
        s.borrow_mut().push(root)
    });
}

pub(crate) fn pop_load_root() {
    crate::host::with_host_ref_or_init(|h| {
        let s = &h.load_roots;
        {
            s.borrow_mut().pop();
        }
    });
}

pub(crate) fn clear_load_roots() {
    crate::host::with_host_ref_or_init(|h| {
        let s = &h.load_roots;
        s.borrow_mut().clear()
    });
}

/// 当前正在展开的插件由哪个根提供(无 → None)。**取栈顶**:嵌套加载时以最内层为准。
pub(crate) fn current_load_root() -> Option<PathBuf> {
    crate::host::with_host_ref_or_init(|h| {
        let s = &h.load_roots;
        {
            s.borrow()
                .last()
                .filter(|p| !p.as_os_str().is_empty())
                .cloned()
        }
    })
}

/// 访问 THEME_OVERRIDES（宿主字段;1b-3c 前为 thread_local）
pub(crate) fn with_theme_overrides<T>(
    f: impl FnOnce(&mut HashMap<String, crate::theme::StyleOverride>) -> T,
) -> T {
    crate::host::with_host_ref_or_init(|h| {
        let o = &h.theme_overrides;
        f(&mut o.borrow_mut())
    })
}

/// 置主题脏位（set/reset_theme 调用；helix-term drain 读取）
pub(crate) fn mark_theme_dirty() {
    crate::host::with_host_ref_or_init(|h| {
        let d = &h.theme_dirty;
        d.set(true)
    });
}

/// 读取并清主题脏位
pub(crate) fn take_theme_dirty_flag() -> bool {
    crate::host::with_host_ref_or_init(|h| {
        let d = &h.theme_dirty;
        d.replace(false)
    })
}

pub(crate) fn next_popup_id() -> u64 {
    crate::host::with_host_ref_or_init(|h| {
        let c = &h.next_popup_id;
        {
            let v = c.get();
            c.set(v + 1);
            v
        }
    })
}

pub(crate) fn next_map_id() -> u64 {
    crate::host::with_host_ref_or_init(|h| {
        let c = &h.next_map_id;
        {
            let v = c.get();
            c.set(v + 1);
            v
        }
    })
}

pub(crate) fn next_terminal_view_id() -> u64 {
    crate::host::with_host_ref_or_init(|h| {
        let c = &h.next_terminal_view_id;
        {
            let v = c.get();
            c.set(v + 1);
            v
        }
    })
}

pub(crate) fn next_term_id() -> u64 {
    crate::host::with_host_ref_or_init(|h| {
        let c = &h.next_term_id;
        {
            let v = c.get();
            c.set(v + 1);
            v
        }
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
    crate::host::with_host_ref_or_init(|h| {
        let c = &h.last_panel_id;
        c.get()
    })
}

pub(crate) fn set_last_panel_id(v: Option<u64>) {
    crate::host::with_host_ref_or_init(|h| {
        let c = &h.last_panel_id;
        c.set(v)
    });
}

/// 当前打开的面板 id 列表(reload 时全部关闭 —— 只关 last 会残留僵尸面板)。
/// **按宿主分桶**(见 `host::with_open_panels`):并发测试各看各的桶,不再互相"抢面板"。
pub(crate) fn with_open_panels<T>(f: impl FnOnce(&mut Vec<u64>) -> T) -> T {
    crate::host::with_open_panels(f)
}

/// **picker 状态摘要** —— 由核心在渲染 picker 时推进来 ✓
///
/// 为什么需要:渲染回调只拿到 `(focus, {width,height})` ✗ —— **没有数据** ✗
/// (弹窗探针实测:`POP-RENDER focus=null w=96 h=105` ✓)
/// ⇒ 要把"绘 UI"交给 JS,必须先把**状态**喂过去 ✓ —— 这正是本字段的作用 ✓
pub(crate) static PICKER_STATE: OnceLock<Mutex<String>> = OnceLock::new();

/// 记录 picker 的状态摘要(由核心渲染路径调用 ✓;每帧一次,代价≈一次加锁+分配 ✓)
pub fn set_picker_state(s: &str) {
    let cell = PICKER_STATE.get_or_init(|| Mutex::new(String::new()));
    if let Ok(mut v) = cell.lock() {
        v.clear();
        v.push_str(s);
    }
}

/// 读取 picker 状态摘要(JS 侧 `helix.picker_state()` 用 ✓)
pub(crate) fn picker_state() -> String {
    PICKER_STATE
        .get()
        .and_then(|c| c.lock().ok().map(|v| v.clone()))
        .unwrap_or_default()
}

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

/// UI 请求队列:**按宿主分桶**(见 `host::UiRequests`)—— 63 个 push 点因此无需改动。
pub(crate) use crate::host::UiRequests;
pub(crate) static UI_REQUESTS: OnceLock<UiRequests> = OnceLock::new();

/// 设置插件根(**有序**)。1b:值存进宿主字段;`Box::leak` 保留只为不动 `plugin_roots()` 的
/// `&'static` 签名。同宿主重复设置 = **覆盖**(与 T5-a/T5-b 的"首设生效"相反,那正是并行串台的根源)。
pub fn set_plugin_roots(roots: Vec<PathBuf>) {
    let leaked: &'static [PathBuf] = Box::leak(roots.into_boxed_slice());
    crate::host::with_host_ref_or_init(|h| *h.plugin_roots.borrow_mut() = leaked);
}

/// 当前宿主的插件根;未设置 ⇒ 空切片(集成测试依赖这个语义)
/// 当前宿主的插件根;未设置 ⇒ 空切片(集成测试依赖这个语义)
pub(crate) fn plugin_roots() -> &'static [PathBuf] {
    crate::host::with_host_ref_or_init(|h| *h.plugin_roots.borrow())
}

/// 某个提供根是不是"内置"(随软件分发的 runtime)。
///
/// 判据取**根而不是名字**:用户可以用 `config/plugins/<同名>/plugin.js` 覆盖内置插件,
/// 那份是**第三方代码**,不该继承内置的豁免。
/// 只剩一个根时(set_plugins_dir 的单根语义 / 单测)那个根就是用户层 ⇒ 都不是内置。
pub(crate) fn is_builtin_root(root: &std::path::Path) -> bool {
    // **空根 = 无提供根**(绝对路径加载 / `load_script_named`,如 `:plugin-load /abs/x.js`)
    // ⇒ 按**第三方**对待:这不是"内置",只是"没有根信息"。
    // 早先的写法(`last() != root`)会把这个情形判成内置 ⇒ 检查点对 `:plugin-load` 失效(假阴性)。
    if root.as_os_str().is_empty() {
        return false;
    }
    let roots = plugin_roots();
    // 内置 = 提供根是**用户插件目录(末位)以外**的已知根(即随软件分发的 runtime roots)
    roots.len() > 1 && roots[..roots.len() - 1].iter().any(|r| r == root)
}

/// 插件目录(**用户层 = 最后一个根**;`resolve_in` 用 `rev()` 迭代 ⇒ 末位优先级最高)。
/// `manifest.json` 与插件文件都在这里 —— `:plugin` 管理器必需
/// (此前 JS 侧**没有任何** config/插件目录访问器,实测 `config_dir`/`plugin_dir` 注册数为 0)。
pub(crate) fn plugins_dir() -> Option<PathBuf> {
    plugin_roots().last().cloned()
}

// ── 键入命令的参数通道(§12.0)──────────────────────────────────────
// 为什么是**独立通道**而不是给 `CommandContext` 加字段:它有 ~57 个构造点
// (3 处 typed.rs · 2 处 helix-js 非测试 · ≈52 处单测),加字段就是 57 处都要改 ——
// 正是本项目反复吃亏的"漏一处**不报错**"的形态。(同 `OpenScratchFile` 那次的取舍。)
/// 分派器在调用 JS 命令**之前**设入(`run_plugin_command`)。
pub fn set_command_args(args: Vec<String>) {
    crate::host::with_host_ref_or_init(|h| {
        let a = &h.pending_command_args;
        *a.borrow_mut() = args
    });
}

/// **调用之后必须清空** —— 否则参数会"粘"到下一个命令,而那类 bug 极难查。
pub fn clear_command_args() {
    crate::host::with_host_ref_or_init(|h| {
        let a = &h.pending_command_args;
        a.borrow_mut().clear()
    });
}

/// 当前待处理参数(JS 侧 `helix.command_args()` 读它)
pub(crate) fn command_args() -> Vec<String> {
    crate::host::with_host_ref_or_init(|h| {
        let a = &h.pending_command_args;
        a.borrow().clone()
    })
}

/// 设置 runtime 目录(同上:进宿主字段,`&'static` 签名不变)
pub fn set_runtime_dirs(dirs: Vec<PathBuf>) {
    let leaked: &'static [PathBuf] = Box::leak(dirs.into_boxed_slice());
    crate::host::with_host_ref_or_init(|h| *h.runtime_dirs.borrow_mut() = leaked);
}

/// 当前宿主的 runtime 目录(`runtime_path()` 的读取口);未设置 ⇒ 空切片
/// 当前宿主的 runtime 目录(`runtime_path()` 的读取口);未设置 ⇒ 空切片
pub(crate) fn runtime_dirs() -> &'static [PathBuf] {
    crate::host::with_host_ref_or_init(|h| *h.runtime_dirs.borrow())
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
pub(crate) fn runtime_path(name: &str) -> Option<PathBuf> {
    runtime_file_in(runtime_dirs(), name)
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
pub(crate) fn remove_plugin_file(rel: &str) -> Result<bool, String> {
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
pub(crate) fn write_plugin_file(rel: &str, text: &str) -> Result<PathBuf, String> {
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
pub(crate) fn resolve_plugin_path(key: &str) -> Option<PathBuf> {
    // 目录级覆盖(§4-2):优先在"提供当前插件的那个根"内解析
    let pref = current_load_root();
    resolve_in_pref(plugin_roots(), key, pref.as_deref())
}
/// 布局树序列化缓存（helix-term 树变更时写入；get_layout 读取）
pub(crate) static LAST_LAYOUT: OnceLock<Mutex<String>> = OnceLock::new();
/// 插件配置合并缓存（helix-term config load 后写入；get_config 读取）JSON: `{"<name>": {...}}`
pub(crate) static CONFIGS: OnceLock<Mutex<String>> = OnceLock::new();
/// 当前文档诊断序列化缓存（helix-term 每帧写入；diagnostics 读取）
pub(crate) static DIAGNOSTICS: OnceLock<Mutex<String>> = OnceLock::new();

/// 终端实例注册表(pty_id → (view_id, cmd);`term_kill(pty_id)` 清理、`term_list` 查询)。
/// 1b:存进宿主字段(这张表的键是 pty id,但状态本就属于某个宿主)。
///
/// 非 unix 下没有 pty ⇒ 无人调用(但函数本身是跨平台注册表的一部分,不整个 cfg 掉)
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn register_term(pty_id: u64, view_id: u64, cmd: String) {
    crate::host::with_host_ref_or_init(|h| {
        h.open_terms.borrow_mut().insert(pty_id, (view_id, cmd));
    });
}

pub(crate) fn unregister_term(pty_id: u64) {
    crate::host::with_host_ref_or_init(|h| {
        h.open_terms.borrow_mut().remove(&pty_id);
    });
}

/// 当前打开的终端列表（view_id, cmd）。键是 pty_id：term_kill(pty_id) 能删对。
pub(crate) fn list_terms() -> Vec<(u64, String)> {
    crate::host::with_host_ref_or_init(|h| {
        h.open_terms
            .borrow()
            .values()
            .map(|(view_id, cmd)| (*view_id, cmd.clone()))
            .collect()
    })
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
    static NEXT_ASYNC_ID: Cell<u64> = const { Cell::new(1) };
    // helix.lsp.* 请求的 promise 解析函数（id → resolve/reject）；内容泄漏（同上）
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
    crate::host::with_host_ref_or_init(|h| f(&mut h.terms.borrow_mut()))
}

/// 访问 TERM_PROMISES（run_async 的 promise 解析函数注册表）：同上（内容泄漏）
pub(crate) fn with_term_promises<T>(
    f: impl FnOnce(&mut HashMap<u64, ResolvingFunctions>) -> T,
) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.term_promises.borrow_mut()))
}

/// 访问 ASYNC_PROMISES（异步 fs 的 promise 解析函数注册表）：同上（内容泄漏）
pub(crate) fn with_async_promises<T>(
    f: impl FnOnce(&mut HashMap<u64, ResolvingFunctions>) -> T,
) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.async_promises.borrow_mut()))
}

/// 访问 LSP_PROMISES（helix.lsp.* 的 promise 解析函数注册表）：同上（内容泄漏）
pub(crate) fn with_lsp_promises<T>(
    f: impl FnOnce(&mut HashMap<u64, ResolvingFunctions>) -> T,
) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.lsp_promises.borrow_mut()))
}

/// 取走并清空 echo 消息队列
pub fn take_messages() -> Vec<String> {
    // 取走:队列按宿主隔离(T1 步骤 2);无宿主 ⇒ 空(**不得** init —— 那会建整个引擎)
    crate::host::try_with_messages(std::mem::take).unwrap_or_default()
}

/// 设置布局目录(同上:进宿主字段)。同宿主重复设置 = 覆盖(T5-a 的"首设生效"曾导致并行串台)。
pub fn set_layouts_dir(dir: PathBuf) {
    let leaked: &'static PathBuf = Box::leak(Box::new(dir));
    crate::host::with_host_ref_or_init(|h| *h.layouts_dir.borrow_mut() = Some(leaked));
}

/// 当前宿主的布局目录;未设置 ⇒ `None`
pub(crate) fn layouts_dir() -> Option<&'static PathBuf> {
    crate::host::with_host_ref_or_init(|h| *h.layouts_dir.borrow())
}

/// 读当前布局 dump(还没缓存过 → 空串)
pub(crate) fn layout_json() -> String {
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
/// 打开文档快照(**宿主字段**;1b:原是一张 `HashMap<宿主key, String>` 的进程级键表)。
/// 每帧写 ⇒ 存宿主里、读时 clone;**不做 `Box::leak`**(那会无限增长)。
/// 历史:键表时代并行跑会读到别人的快照(实测 `buffer_traversal_and_focus` 拿到 z.txt)。
pub fn cache_buffers(json: &str) {
    // 1b:快照存宿主字段(原来是 `HashMap<宿主key, String>` 键表)
    crate::host::with_host_ref_or_init(|h| *h.buffers.borrow_mut() = json.to_string());
}

/// 当前宿主的 pane 清单快照(还没缓存过 → 空串)。`helix.pane.*` 的读取口。
pub(crate) fn panes_json() -> String {
    crate::host::with_host_ref(|h| h.panes.borrow().clone())
}

/// 当前宿主的平级模式快照(还没缓存过 → 空串)。**which-key 的键位表唯一来源**。
pub(crate) fn pane_mode_json() -> String {
    crate::host::with_host_ref(|h| h.pane_mode.borrow().clone())
}

/// 当前宿主的打开文档快照(还没缓存过 → 空串)。
/// `helix.buffer.list()` / `helix.buffer.current()` 的**唯一读取口** ⇒ 分桶时只改这里。
pub(crate) fn buffers_json() -> String {
    crate::host::with_host_ref(|h| h.buffers.borrow().clone())
}

/// 写入平级模式快照(模式切换时由 helix-term 调用)
/// 写入 pane 清单快照(布局变更时由 helix-term 调用)
/// pane 清单快照(宿主字段;1b 同上)。布局变更时写,`helix.pane.list()` 读。
pub fn cache_panes(json: &str) {
    crate::host::with_host_ref_or_init(|h| *h.panes.borrow_mut() = json.to_string());
}

/// 平级模式快照(宿主字段;1b 同上)。模式切换时写,`helix.pane_mode.current()/keymap()` 读。
/// 历史:值取自它 ⇒ which-key 曾渲染出**别的用例的**键位(实测 2/2 复现)。
pub fn cache_pane_mode(json: &str) {
    crate::host::with_host_ref_or_init(|h| *h.pane_mode.borrow_mut() = json.to_string());
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
        assert!(remove_plugin_file_in(&roots, "plug/plugin.js").unwrap());
        assert!(!root.path().join("plug/plugin.js").exists());
        assert!(!remove_plugin_file_in(&roots, "plug/plugin.js").unwrap());
        // 目录也能删(remove_orphan 要删整棵插件目录)
        std::fs::create_dir_all(root.path().join("plug2/sub")).unwrap();
        std::fs::write(root.path().join("plug2/sub/a.js"), "x").unwrap();
        assert!(remove_plugin_file_in(&roots, "plug2").unwrap());
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

// 引擎所有权/借出/重入的测试随实现搬进 `helix-js/src/host.rs`
