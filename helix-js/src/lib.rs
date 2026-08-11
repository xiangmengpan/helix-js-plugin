//! JavaScript plugin runtime for the Helix editor (PoC).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Result};
use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsString, JsValue, NativeFunction, Source};

/// 最小 PTY 封装：posix_openpt/grantpt/unlockpt/ptsname/open slave + TIOCSWINSZ。
/// slave 返回 File 直接喂给 Command 的 Stdio（spawn 后父侧自动关闭）；
/// master 由 worker 线程持有（Drop 关闭），term_resize 直连 master fd ioctl。
// ponytail: 仅 Unix（Linux/macOS）。无 TIOCSCTTY/作业控制/信号转发——
// 交互终端后续要完整的话，需把信号(SIGWINCH)与前后台管理接进 helix-term。
#[cfg(unix)]
mod pty {
    use std::fs::File;
    use std::os::fd::{FromRawFd, RawFd};

    use anyhow::{anyhow, Result};

    /// master fd 包装：Drop 时 close（worker 线程持有时保证 fd 生命周期）
    pub struct Master(RawFd);

    impl Master {
        pub fn fd(&self) -> RawFd {
            self.0
        }

        /// 把裸 fd 交给 File，不再 Drop close（防双关）
        pub fn into_file(self) -> File {
            let fd = self.0;
            std::mem::forget(self);
            unsafe { File::from_raw_fd(fd) }
        }
    }

    impl Drop for Master {
        fn drop(&mut self) {
            unsafe {
                libc::close(self.0);
            }
        }
    }

    /// 打开新 PTY（默认 24×80），返回 (master, slave File)
    pub fn open_pty() -> Result<(Master, File)> {
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
            if master < 0 {
                return Err(anyhow!("posix_openpt: {}", std::io::Error::last_os_error()));
            }
            let master = Master(master); // 尽早包进 RAII 包装，后续 `?` 错误路径不再泄漏 fd
            if libc::grantpt(master.fd()) != 0 {
                return Err(anyhow!("grantpt: {}", std::io::Error::last_os_error()));
            }
            if libc::unlockpt(master.fd()) != 0 {
                return Err(anyhow!("unlockpt: {}", std::io::Error::last_os_error()));
            }
            let name = libc::ptsname(master.fd());
            if name.is_null() {
                return Err(anyhow!("ptsname: {}", std::io::Error::last_os_error()));
            }
            let name = std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned();
            let slave = libc::open(
                std::ffi::CString::new(name)?.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            );
            if slave < 0 {
                return Err(anyhow!("open slave: {}", std::io::Error::last_os_error()));
            }
            let slave = File::from_raw_fd(slave);
            // 默认尺寸：内核新建 pty 的 winsize 是 0×0，stty size 会读成 "0 0"；
            // 简报期望缺省 "24 80"。在子进程启动前设好（master ioctl 作用于同一 tty）。
            set_winsize(master.fd(), 24, 80)?; // 失败时 master/slave 由 Drop 兜底关闭
            Ok((master, slave))
        }
    }

    /// TIOCSWINSZ 设置 pty 窗口尺寸（master/slave fd 均可，作用于同一 tty 设备）。
    /// 在 spawn 后立即调用也能在子进程启动前生效——winsize 是 tty 设备属性，
    /// 不依赖子进程是否存在，因此无消息时序竞态。
    pub fn set_winsize(fd: RawFd, rows: u16, cols: u16) -> Result<()> {
        unsafe {
            let ws = libc::winsize {
                ws_row: rows as libc::c_ushort,
                ws_col: cols as libc::c_ushort,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            if libc::ioctl(fd, libc::TIOCSWINSZ, &ws) != 0 {
                return Err(anyhow!("TIOCSWINSZ: {}", std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }
}

/// 插件向编辑器发起的 UI 请求（编辑器主线程取走后执行）
#[derive(Debug)]
pub enum UiRequest {
    OpenPopup {
        id: u64,
        width: Option<u16>,
        height: Option<u16>,
        position: Option<(u16, u16)>,
    },
    OpenPanel {
        id: u64,
        side: String,
        size: u16,
    },
    ClosePanel {
        id: u64,
    },
    OpenFile {
        path: String,
    },
    MovePanel {
        id: u64,
        side: String,
    },
    MapKey { mode: String, key: String, command: String },
}

/// 异步进程事件（worker 线程 → 主线程；主线程 drain 后 resolve 到 JS 回调）
#[derive(Debug)]
pub enum TermEvent {
    Chunk(u64, String),
    /// run_async 的 Exit 携带聚合后的完整 stdout（Option）；spawn 的 Exit 为 None
    Exit(u64, i32, Option<String>),
}

/// 异步 fs 操作结果（stat 快照）
#[derive(Debug, Clone)]
pub struct FsStat {
    pub size: u64,
    pub is_dir: bool,
    /// 修改时间（Unix 秒）
    pub mtime: u64,
}

/// 异步 fs 事件（一次性 worker → 主线程；drain 后 resolve 到 JS 回调）。
/// Result 的 Err 分支携带错误字符串（回调的 err 参数）。
#[derive(Debug)]
pub enum AsyncEvent {
    FsRead(u64, std::result::Result<String, String>),
    FsWrite(u64, std::result::Result<(), String>),
    FsStat(u64, std::result::Result<FsStat, String>),
    FsGlob(u64, std::result::Result<Vec<String>, String>),
}

/// 主线程 → worker 线程的控制指令（stdin 写入 / 杀进程）
enum TermCtrl {
    Write(String),
    Kill,
}

/// 一个进程 id 的 JS 回调集（resolve 时需要克隆出容器外调用）
struct TermCallbacks {
    on_chunk: JsValue,
    on_exit: Option<JsValue>,
    /// true = run_async（Exit 回调签名为 (err, out)）；false = spawn（签名为 (code)）
    is_run_async: bool,
}

/// 按键事件的只读快照，传给 JS onKey 回调
pub struct PluginKey {
    pub name: String,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// popup_key 的返回：关闭弹窗 / 已消费按键 / 穿透给编辑器
#[derive(Debug, PartialEq, Eq)]
pub enum PopupKeyResult {
    Close,
    Handled,
    Ignored,
}

/// 弹窗回调注册表项（JsValue 是 Clone 的，popup_key 需要克隆出来调）
#[derive(Clone)]
struct PopupCallbacks {
    render: JsValue,
    on_key: Option<JsValue>,
    on_close: Option<JsValue>,
}

/// render 回调返回的一行：文本 + 可选样式名（主题 scope，如 "error"）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledLine {
    pub text: String,
    pub style: Option<String>,
}

/// 事件名白名单：helix.on 只接受这些事件
const EVENT_WHITELIST: [&str; 5] = ["save", "mode-change", "buffer-open", "buffer-close", "doc-change"];

/// 插件命令收到的只读上下文快照（由 helix-term 序列化编辑器状态得到）
pub struct CommandContext {
    pub path: Option<String>,
    pub text: String,
    pub cursor: (usize, usize),
    /// 主选区（anchor, head）行列对
    pub selection: ((usize, usize), (usize, usize)),
}

/// 状态栏钩子收到的轻量上下文（不含 doc.text，避免每帧克隆全文）
pub struct StatuslineCtx {
    pub path: Option<String>,
    pub mode: String,
    pub cursor: (usize, usize),
}

/// 一次文档编辑请求（坐标基于命令开始时的原始快照，0-based 行列）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub insert: String,
}

/// 光标/选区请求（命令返回后由 helix-term 应用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorRequest {
    SetCursor { row: usize, col: usize },
    SetSelection { anchor: (usize, usize), head: (usize, usize) },
}

// boa 的 Context/JsValue 是 !Send（Rc GC 堆），不能用 static 全局共享，
// 所以引擎按线程存放（编辑器主线程是唯一调用者）；MESSAGES 跨线程共享。
// 进程退出阶段曾约 50% 概率 SIGABRT（glibc tcache corruption）：线程退出时
// 容器 drop → JsValue drop → boa Gc 堆对象被释放，与 teardown 时序冲突。
// 故 CONTEXT 及所有持有 JsValue 的容器（REGISTRY/POPUPS/EVENT_HANDLERS/
// BUFFER_ICON_HOOK/STATUSLINE_HOOK）一律 Box::leak 到 'static——线程退出时
// 只 drop 引用（指针），内容永不 drop，由 OS 在进程退出时回收。对 PoC 无
// 实际代价；升级 boa 后可改回正常持有。
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
    static THEME_OVERRIDES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    // 覆盖集是否变化（set/reset 置位；helix-term drain 时读取并清位）
    static THEME_DIRTY: Cell<bool> = const { Cell::new(false) };
}

/// 访问 REGISTRY：首次触达惰性 Box::leak 创建；线程退出时内容泄漏（见上）
fn with_registry<T>(f: impl FnOnce(&mut HashMap<String, JsValue>) -> T) -> T {
    REGISTRY.with(|r| {
        let mut slot = r.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 SCRIPT_EXPORTS：同上（内容泄漏）
fn with_script_exports<T>(f: impl FnOnce(&mut HashMap<String, JsValue>) -> T) -> T {
    SCRIPT_EXPORTS.with(|m| {
        let mut slot = m.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 LAST_EXPORT：同上（内容泄漏）
fn with_last_export<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    LAST_EXPORT.with(|l| {
        let mut slot = l.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 EVENT_HANDLERS：同上（内容泄漏）
fn with_event_handlers<T>(f: impl FnOnce(&mut HashMap<String, Vec<JsValue>>) -> T) -> T {
    EVENT_HANDLERS.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 POPUPS：同上（内容泄漏）
fn with_popups<T>(f: impl FnOnce(&mut HashMap<u64, PopupCallbacks>) -> T) -> T {
    POPUPS.with(|p| {
        let mut slot = p.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 BUFFER_ICON_HOOK：同上（内容泄漏）
fn with_buffer_icon_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    BUFFER_ICON_HOOK.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 STATUSLINE_HOOK：同上（内容泄漏）
fn with_statusline_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    STATUSLINE_HOOK.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}
static MESSAGES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static UI_REQUESTS: OnceLock<Mutex<Vec<UiRequest>>> = OnceLock::new();
/// 插件目录（helix-term 启动时设置；js_load 相对名解析用）
static PLUGINS_DIR: OnceLock<PathBuf> = OnceLock::new();
// 事件通道按线程存放：回调注册表（TERM_CALLBACKS）是线程本地的，通道也必须同线程配对——
// 全局单通道会被并发测试的 render 泵互偷（别的线程 drain 后 resolve 时找不到本线程的回调，静默丢弃）。
// worker 在 std 线程上持发起线程的 Sender 克隆；drain 只读本线程的 Receiver。
thread_local! {
    static TERM_EVENTS: RefCell<Option<std::sync::mpsc::Sender<TermEvent>>> = const { RefCell::new(None) };
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
    static ASYNC_EVENTS: RefCell<Option<std::sync::mpsc::Sender<AsyncEvent>>> = const { RefCell::new(None) };
    static ASYNC_EVENTS_RX: RefCell<Option<std::sync::mpsc::Receiver<AsyncEvent>>> = const { RefCell::new(None) };
    // 异步 fs id → JS 回调（持有 JsValue：线程退出时内容泄漏，同上）
    static ASYNC_CALLBACKS: RefCell<Option<&'static mut HashMap<u64, JsValue>>> = const { RefCell::new(None) };
    static NEXT_ASYNC_ID: Cell<u64> = const { Cell::new(1) };
}

/// 访问 TERM_CALLBACKS：同上（内容泄漏）
fn with_terms<T>(f: impl FnOnce(&mut HashMap<u64, TermCallbacks>) -> T) -> T {
    TERM_CALLBACKS.with(|t| {
        let mut slot = t.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 访问 ASYNC_CALLBACKS：同上（内容泄漏）
fn with_async_callbacks<T>(f: impl FnOnce(&mut HashMap<u64, JsValue>) -> T) -> T {
    ASYNC_CALLBACKS.with(|t| {
        let mut slot = t.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// 创建 boa 上下文并注册全局 `helix` 对象（幂等）
pub fn init() {
    MESSAGES.get_or_init(Default::default);
    UI_REQUESTS.get_or_init(Default::default);
    TERM_EVENTS.with(|t| {
        if t.borrow().is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            *t.borrow_mut() = Some(tx);
            TERM_EVENTS_RX.with(|r| *r.borrow_mut() = Some(rx));
        }
    });
    ASYNC_EVENTS.with(|t| {
        if t.borrow().is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            *t.borrow_mut() = Some(tx);
            ASYNC_EVENTS_RX.with(|r| *r.borrow_mut() = Some(rx));
        }
    });
    CONTEXT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let engine = Box::leak(Box::new(Context::default()));
            // ObjectInitializer 方法取 &mut self，链式必须在一个表达式内；
            // term_resize 是 cfg(unix) 的，拆成两步注册（builder 可变绑定）
            let mut builder = ObjectInitializer::new(engine);
            builder
                .function(NativeFunction::from_fn_ptr(js_echo), JsString::from("echo"), 1)
                .function(
                    NativeFunction::from_fn_ptr(js_register_command),
                    JsString::from("register_command"),
                    2,
                )
                .function(NativeFunction::from_fn_ptr(js_open_popup), JsString::from("open_popup"), 1)
                .function(NativeFunction::from_fn_ptr(js_open_panel), JsString::from("open_panel"), 1)
                .function(NativeFunction::from_fn_ptr(js_close_panel), JsString::from("close_panel"), 1)
                .function(NativeFunction::from_fn_ptr(js_read_dir), JsString::from("read_dir"), 1)
                .function(NativeFunction::from_fn_ptr(js_open_file), JsString::from("open_file"), 1)
                .function(NativeFunction::from_fn_ptr(js_move_panel), JsString::from("move_panel"), 2)
                .function(NativeFunction::from_fn_ptr(js_set_buffer_icon), JsString::from("set_buffer_icon"), 1)
                .function(NativeFunction::from_fn_ptr(js_on), JsString::from("on"), 2)
                .function(NativeFunction::from_fn_ptr(js_map), JsString::from("map"), 3)
                .function(NativeFunction::from_fn_ptr(js_set_cursor), JsString::from("set_cursor"), 2)
                .function(NativeFunction::from_fn_ptr(js_set_selection), JsString::from("set_selection"), 4)
                .function(NativeFunction::from_fn_ptr(js_set_statusline), JsString::from("set_statusline"), 1)
                .function(NativeFunction::from_fn_ptr(js_load), JsString::from("load"), 1)
                .function(NativeFunction::from_fn_ptr(js_export), JsString::from("export"), 1)
                .function(NativeFunction::from_fn_ptr(js_lazy), JsString::from("lazy"), 2)
                .function(NativeFunction::from_fn_ptr(js_run_command), JsString::from("run_command"), 1)
                .function(NativeFunction::from_fn_ptr(js_run), JsString::from("run"), 1)
                .function(NativeFunction::from_fn_ptr(js_run_async), JsString::from("run_async"), 2)
                .function(NativeFunction::from_fn_ptr(js_spawn), JsString::from("spawn"), 1)
                .function(NativeFunction::from_fn_ptr(js_term_write), JsString::from("term_write"), 2)
                .function(NativeFunction::from_fn_ptr(js_term_kill), JsString::from("term_kill"), 1)
                .function(NativeFunction::from_fn_ptr(js_read_file_async), JsString::from("read_file_async"), 2)
                .function(NativeFunction::from_fn_ptr(js_write_file_async), JsString::from("write_file_async"), 3)
                .function(NativeFunction::from_fn_ptr(js_stat_async), JsString::from("stat_async"), 2)
                .function(NativeFunction::from_fn_ptr(js_glob_async), JsString::from("glob_async"), 2)
                .function(NativeFunction::from_fn_ptr(js_set_theme), JsString::from("set_theme"), 1)
                .function(NativeFunction::from_fn_ptr(js_reset_theme), JsString::from("reset_theme"), 0);
            #[cfg(unix)]
            builder.function(NativeFunction::from_fn_ptr(js_term_resize), JsString::from("term_resize"), 3);
            let helix = builder.build();
            engine
                .register_global_property(JsString::from("helix"), helix, Attribute::READONLY | Attribute::NON_ENUMERABLE)
                .expect("register helix object");
            *slot = Some(engine);
        }
    });
}

/// 设置主题覆盖：scope → 颜色字符串，整体替换旧的覆盖集并置脏。
/// 非字符串值（对象/数字等）忽略；空对象等价清空。
fn js_set_theme(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let obj = args.first().unwrap_or(&JsValue::undefined()).as_object().ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from(
            "helix.set_theme: expected an object of { scope: color }",
        )))
    })?;
    let mut overrides = HashMap::new();
    for key in obj.own_property_keys(ctx)? {
        let boa_engine::property::PropertyKey::String(scope) = &key else { continue };
        let scope = scope.to_std_string_escaped();
        let value = obj.get(key, ctx)?;
        // 只接受字符串颜色值；其余类型忽略（不整体报错——部分非法条目不阻断其余覆盖）
        let Some(color) = (value.is_string())
            .then(|| value.try_js_into::<String>(ctx).ok())
            .flatten()
        else { continue };
        overrides.insert(scope, color);
    }
    THEME_OVERRIDES.with(|o| *o.borrow_mut() = overrides);
    THEME_DIRTY.with(|d| d.set(true));
    Ok(JsValue::undefined())
}

/// 清除主题覆盖并置脏（helix-term 下次 drain 还原基准主题）
fn js_reset_theme(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    THEME_OVERRIDES.with(|o| o.borrow_mut().clear());
    THEME_DIRTY.with(|d| d.set(true));
    Ok(JsValue::undefined())
}

/// 覆盖集是否自上次 drain 后变化（helix-term 读取并清位）
pub fn take_theme_dirty() -> bool {
    init();
    THEME_DIRTY.with(|d| d.replace(false))
}

/// 当前主题覆盖集快照（scope → 颜色字符串；helix-term 合并进基准主题）
pub fn theme_overrides() -> HashMap<String, String> {
    init();
    THEME_OVERRIDES.with(|o| o.borrow().clone())
}

fn js_register_command(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let func = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
            "invalid command name: {name:?}"
        )))));
    }
    // 先校验/存入 doc 再注册：第三参类型非法（如 42）时整体失败，不产生半注册
    if let Some(doc_arg) = args.get(2) {
        if !doc_arg.is_null_or_undefined() {
            let doc: String = doc_arg.try_js_into(context)?;
            COMMAND_DOCS.with(|d| d.borrow_mut().insert(name.clone(), doc));
        }
    }
    with_registry(|r| r.insert(name, func));
    Ok(JsValue::undefined())
}

/// 设置插件目录（js_load 相对名解析用）。OnceLock 只生效一次：helix-term 启动时调用。
pub fn set_plugins_dir(dir: PathBuf) {
    let _ = PLUGINS_DIR.set(dir);
}

/// helix.load(name)：从插件目录加载脚本（相对名或绝对路径），返回其 helix.export 的值；
/// 重复加载返回缓存对象。`.js` 后缀强制（无则补）。文件缺失/语法错 → 抛错。
/// 嵌套加载：内层 load 消费 LAST_EXPORT（take 语义），外层脚本自己的 export 随后设置。
/// 直接用传入的 boa Context 调（不再借 CONTEXT 线程局部）——load 可能发生在命令运行中
/// （lazy 桩），外层 run_command 正持有 CONTEXT 的 RefCell 借用，再借会 panic。
fn js_load(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from("helix.load: name must be a string")))
        })?;
    if name.is_empty() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.load: name must not be empty",
        ))));
    }
    let key = if name.ends_with(".js") { name } else { format!("{name}.js") };
    if let Some(cached) = with_script_exports(|m| m.get(&key).cloned()) {
        return Ok(cached);
    }
    let path = if Path::new(&key).is_absolute() {
        PathBuf::from(&key)
    } else {
        PLUGINS_DIR.get().map(|d| d.join(&key)).ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from("helix.load: plugins dir not set")))
        })?
    };
    let src = std::fs::read_to_string(&path).map_err(|e| {
        JsError::from_opaque(JsValue::from(JsString::from(format!("helix.load('{key}'): {e}"))))
    })?;
    eval_wrapped(ctx, &src).map_err(|e| {
        JsError::from_opaque(JsValue::from(JsString::from(format!("helix.load('{key}') failed: {e}"))))
    })?;
    let export = with_last_export(|l| l.take()).unwrap_or(JsValue::undefined());
    LOADED_SCRIPTS.with(|s| {
        let mut s = s.borrow_mut();
        if !s.iter().any(|(n, _)| n == &key) {
            s.push((key.clone(), src));
        }
    });
    with_script_exports(|m| m.insert(key, export.clone()));
    Ok(export)
}

/// helix.export(obj)：声明当前脚本的导出（被 helix.load 的返回值拿到）。undefined/null 清空。
fn js_export(_this: &JsValue, args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let arg = args.first().cloned().unwrap_or(JsValue::undefined());
    with_last_export(|l| *l = if arg.is_null_or_undefined() { None } else { Some(arg) });
    Ok(JsValue::undefined())
}

/// helix.lazy(name, ...cmds)：为每个 cmd 注册桩闭包——首次调用时加载 name 再转执行。
/// 桩经 eval 工厂构造闭包（不经 REGISTRY 捕获 JsValue，避免闭包环境问题）。
fn js_lazy(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    if args.len() < 2 {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.lazy: name and at least one command required",
        ))));
    }
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from("helix.lazy: name must be a string")))
        })?;
    if name.is_empty() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.lazy: name must not be empty",
        ))));
    }
    let factory = ctx
        .eval(Source::from_bytes(
            "(function(n, c) { return function(ctx) { helix.load(n); helix.run_command(c, ctx); }; })",
        ))
        .map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(format!("helix.lazy: {e}")))))?;
    let factory = factory
        .as_callable()
        .and_then(JsFunction::from_object)
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from("helix.lazy: internal factory error")))
        })?;
    let undefined = JsValue::undefined();
    for cmd in &args[1..] {
        let cmd: String = cmd.try_js_into(ctx).map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.lazy: command names must be strings",
            )))
        })?;
        if cmd.is_empty() || cmd.chars().any(char::is_whitespace) {
            return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
                "invalid command name: {cmd:?}"
            )))));
        }
        let closure = factory
            .call(
                &undefined,
                &[
                    JsValue::from(JsString::from(name.clone())),
                    JsValue::from(JsString::from(cmd.clone())),
                ],
                ctx,
            )
            .map_err(|e| {
                JsError::from_opaque(JsValue::from(JsString::from(format!("helix.lazy: {e}"))))
            })?;
        with_registry(|r| r.insert(cmd, closure));
    }
    Ok(JsValue::undefined())
}

/// 解析 JS { row, col } → 坐标（字段缺失/非对象用缺省值）
fn parse_pos(v: &JsValue, ctx: &mut Context, dflt: (usize, usize)) -> boa_engine::JsResult<(usize, usize)> {
    let Some(obj) = v.as_object() else { return Ok(dflt) };
    let row = obj.get(JsString::from("row"), ctx)?;
    let col = obj.get(JsString::from("col"), ctx)?;
    Ok((
        if row.is_null_or_undefined() { dflt.0 } else { row.try_js_into::<usize>(ctx)? },
        if col.is_null_or_undefined() { dflt.1 } else { col.try_js_into::<usize>(ctx)? },
    ))
}

/// 解析 JS ctx 对象 → CommandContext（缺省：path=None/text=""/cursor=(0,0)/selection 全 0）
/// 从 JS 对象读可选字符串字段
fn js_get_str(obj: &boa_engine::JsObject, key: &str, ctx: &mut Context) -> boa_engine::JsResult<Option<String>> {
    let v = obj.get(JsString::from(key), ctx)?;
    if v.is_null_or_undefined() { Ok(None) } else { Ok(Some(v.try_js_into::<String>(ctx)?)) }
}

fn parse_command_ctx(v: &JsValue, ctx: &mut Context) -> boa_engine::JsResult<CommandContext> {
    let dflt = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
    let Some(obj) = v.as_object() else { return Ok(dflt) };
    // 命令实际收到的 ctx 形状是 { doc: { path, text, ... }, cursor, selection }——
    // 兼容两层（doc.path ?? path），保证 lazy 转发不丢 path/text
    let doc_val = obj.get(JsString::from("doc"), ctx)?;
    let doc_obj = doc_val.as_object();
    let path = match &doc_obj {
        Some(doc) => js_get_str(doc, "path", ctx)?.or(js_get_str(&obj, "path", ctx)?),
        None => js_get_str(&obj, "path", ctx)?,
    };
    let text = match &doc_obj {
        Some(doc) => {
            js_get_str(doc, "text", ctx)?.unwrap_or_else(|| js_get_str(&obj, "text", ctx).ok().flatten().unwrap_or_default())
        }
        None => js_get_str(&obj, "text", ctx)?.unwrap_or_default(),
    };
    let cursor = parse_pos(&obj.get(JsString::from("cursor"), ctx)?, ctx, (0, 0))?;
    let selection = {
        let sel = obj.get(JsString::from("selection"), ctx)?;
        if sel.is_null_or_undefined() {
            ((0, 0), (0, 0))
        } else {
            let sel_obj = sel.as_object().ok_or_else(|| {
                JsError::from_opaque(JsValue::from(JsString::from(
                    "helix.run_command: 'selection' must be an object with anchor/head",
                )))
            })?;
            let anchor = parse_pos(&sel_obj.get(JsString::from("anchor"), ctx)?, ctx, (0, 0))?;
            let head = parse_pos(&sel_obj.get(JsString::from("head"), ctx)?, ctx, (0, 0))?;
            (anchor, head)
        }
    };
    Ok(CommandContext { path, text, cursor, selection })
}

/// helix.run_command(name, ctx?)：程序化调用插件命令。ctx 缺省空快照。
/// 嵌套调用合法：直接用传入的 boa Context 调（不再借 CONTEXT，避开 RefCell 重入 panic）；
/// 编辑/光标请求排队，由外层命令的 drain 应用。
fn js_run_command(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from("helix.run_command: name must be a string")))
        })?;
    let command_ctx = parse_command_ctx(args.get(1).unwrap_or(&JsValue::undefined()), ctx)?;
    let func = with_registry(|r| r.get(&name).cloned()).ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "helix.run_command: '{name}' is not registered"
        ))))
    })?;
    let func = func.as_callable().and_then(JsFunction::from_object).ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "registered value for '{name}' is not a function"
        ))))
    })?;
    let arg = ctx_to_js(&command_ctx, ctx)
        .map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(format!("helix.run_command: {e}")))))?;
    let undefined = JsValue::undefined();
    func.call(&undefined, &[arg], ctx)
        .map(|_| JsValue::undefined())
        .map_err(|e| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "helix.run_command('{name}') failed: {e}"
            ))))
        })
}

/// helix.map 允许的 mode 白名单
const KEYMAP_MODES: [&str; 3] = ["normal", "insert", "select"];

/// 注册键位绑定：字符串命令直接入队，函数注册为隐藏插件命令 __mapped_N
fn js_map(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let mode: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    if !KEYMAP_MODES.contains(&mode.as_str()) {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
            "helix.map: unknown mode '{mode}'"
        )))));
    }
    let key: String = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    if key.is_empty() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.map: key must not be empty",
        ))));
    }
    let command_arg = args.get(2).cloned().unwrap_or(JsValue::undefined());

    // 函数 → 注册为隐藏插件命令 __mapped_N
    let command: String = if command_arg.as_callable().is_some() {
        let id = NEXT_MAP_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
        let name = format!("__mapped_{id}");
        with_registry(|r| r.insert(name.clone(), command_arg));
        name
    } else {
        command_arg.try_js_into(context)?
    };

    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::MapKey { mode, key, command });
    Ok(JsValue::undefined())
}

/// 把 CommandContext 转成 doc 对象 { path, text, cursor } + 编辑方法
/// cursor 挂在 doc 上：命令 ctx 与事件 doc 共用，事件回调可直接读 doc.cursor
fn doc_to_js(ctx: &CommandContext, engine: &mut Context) -> boa_engine::JsResult<JsValue> {
    let cursor = ObjectInitializer::new(engine)
        .property(
            JsString::from("row"),
            JsValue::from(ctx.cursor.0 as f64),
            Attribute::all(),
        )
        .property(
            JsString::from("col"),
            JsValue::from(ctx.cursor.1 as f64),
            Attribute::all(),
        )
        .build();
    Ok(JsValue::from(
        ObjectInitializer::new(engine)
        .property(
            JsString::from("path"),
            match &ctx.path {
                Some(p) => JsValue::from(JsString::from(p.clone())),
                None => JsValue::null(),
            },
            Attribute::all(),
        )
        .property(
            JsString::from("text"),
            JsValue::from(JsString::from(ctx.text.clone())),
            Attribute::all(),
        )
        .property(JsString::from("cursor"), cursor, Attribute::all())
        .function(NativeFunction::from_fn_ptr(js_doc_insert), JsString::from("insert"), 3)
        .function(NativeFunction::from_fn_ptr(js_doc_replace), JsString::from("replace"), 5)
        .function(NativeFunction::from_fn_ptr(js_doc_delete), JsString::from("delete"), 4)
        .build(),
    ))
}

/// 把 CommandContext 转成 JS 对象 { doc: { path, text }, cursor: { row, col }, selection: { anchor: {row,col}, head: {row,col} } }
fn ctx_to_js(ctx: &CommandContext, engine: &mut Context) -> boa_engine::JsResult<JsValue> {
    let doc = doc_to_js(ctx, engine)?;
    let anchor = ObjectInitializer::new(engine)
        .property(JsString::from("row"), JsValue::from(ctx.selection.0 .0 as f64), Attribute::all())
        .property(JsString::from("col"), JsValue::from(ctx.selection.0 .1 as f64), Attribute::all())
        .build();
    let head = ObjectInitializer::new(engine)
        .property(JsString::from("row"), JsValue::from(ctx.selection.1 .0 as f64), Attribute::all())
        .property(JsString::from("col"), JsValue::from(ctx.selection.1 .1 as f64), Attribute::all())
        .build();
    let selection = ObjectInitializer::new(engine)
        .property(JsString::from("anchor"), anchor, Attribute::all())
        .property(JsString::from("head"), head, Attribute::all())
        .build();
    let cursor = ObjectInitializer::new(engine)
        .property(
            JsString::from("row"),
            JsValue::from(ctx.cursor.0 as f64),
            Attribute::all(),
        )
        .property(
            JsString::from("col"),
            JsValue::from(ctx.cursor.1 as f64),
            Attribute::all(),
        )
        .build();
    Ok(JsValue::from(
        ObjectInitializer::new(engine)
            .property(JsString::from("doc"), doc, Attribute::all())
            .property(JsString::from("cursor"), cursor, Attribute::all())
            .property(JsString::from("selection"), selection, Attribute::all())
            .build(),
    ))
}

/// 运行插件命令。返回 Ok(true) 表示已运行，Ok(false) 表示未注册
pub fn run_command(name: &str, ctx: &CommandContext) -> Result<bool> {
    init();
    // 命令开始时清空编辑队列，避免跨命令残留
    CURRENT_EDITS.with(|c| c.borrow_mut().clear());
    CURSOR_REQUESTS.with(|c| c.borrow_mut().clear());
    let func = with_registry(|r| r.get(name).cloned());
    let Some(func) = func else { return Ok(false) };

    let func = func
        .as_callable()
        .and_then(JsFunction::from_object)
        .ok_or_else(|| anyhow!("registered value for '{name}' is not a function"))?;

    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized by init()");
        let arg = ctx_to_js(ctx, engine)
            .map_err(|e| anyhow!("failed to build command context: {e}"))?;
        let undefined = JsValue::undefined();
        func.call(&undefined, &[arg], engine)
            .map(|_| true)
            .map_err(|e| anyhow!("plugin command '{name}' failed: {e}"))
    })
}

/// 已注册的插件命令名（供命令行补全）
pub fn command_names() -> Vec<String> {
    init();
    with_registry(|r| r.keys().cloned().collect())
}

/// 插件命令说明（未注册返回 None）
pub fn command_doc(name: &str) -> Option<String> {
    init();
    COMMAND_DOCS.with(|d| d.borrow().get(name).cloned())
}

fn js_on(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let name: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let handler = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if !EVENT_WHITELIST.contains(&name.as_str()) {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
            "helix.on: unknown event '{name}'"
        )))));
    }
    if handler.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.on: handler must be a function",
        ))));
    }
    with_event_handlers(|h| h.entry(name).or_default().push(handler));
    Ok(JsValue::undefined())
}

/// 是否有注册的事件处理器（挂点快速跳过）
pub fn has_handlers(name: &str) -> bool {
    init();
    with_event_handlers(|h| h.get(name).is_some_and(|v| !v.is_empty()))
}

/// 触发事件：按注册顺序调用处理器；开始时清空编辑队列（防残留）。
/// extra 用于 mode-change 的 mode 字符串参数。
pub fn emit_event(name: &str, ctx: &CommandContext, extra: Option<&str>) -> Result<()> {
    init();
    CURRENT_EDITS.with(|c| c.borrow_mut().clear());
    CURSOR_REQUESTS.with(|c| c.borrow_mut().clear());
    let handlers = with_event_handlers(|h| h.get(name).cloned());
    let Some(handlers) = handlers else { return Ok(()) };
    if handlers.is_empty() {
        return Ok(());
    }
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized");
        let doc = doc_to_js(ctx, engine).map_err(|e| anyhow!("failed to build event doc: {e}"))?;
        let undefined = JsValue::undefined();
        for handler in &handlers {
            let func = handler
                .as_callable()
                .and_then(JsFunction::from_object)
                .ok_or_else(|| anyhow!("event '{name}' handler not callable"))?;
            let args: Vec<JsValue> = match extra {
                Some(mode) => vec![JsValue::from(JsString::from(mode.to_string())), doc.clone()],
                None => vec![doc.clone()],
            };
            let _: JsValue = func.call(&undefined, &args, engine).map_err(|e| {
                CURRENT_EDITS.with(|c| c.borrow_mut().clear());
    CURSOR_REQUESTS.with(|c| c.borrow_mut().clear());
                anyhow!("event '{name}' handler failed: {e}")
            })?;
        }
        Ok(())
    })
}

fn js_set_cursor(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let row: usize = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let col: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    CURSOR_REQUESTS.with(|c| c.borrow_mut().push(CursorRequest::SetCursor { row, col }));
    Ok(JsValue::undefined())
}

fn js_set_selection(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let ar: usize = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let ac: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let hr: usize = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let hc: usize = args.get(3).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    CURSOR_REQUESTS.with(|c| c.borrow_mut().push(CursorRequest::SetSelection {
        anchor: (ar, ac),
        head: (hr, hc),
    }));
    Ok(JsValue::undefined())
}

/// shell 执行输出截断上限（防失控输出冻结状态栏）
const RUN_OUTPUT_LIMIT: usize = 65536;

// ponytail: 同步阻塞 + sh -c，仅 Unix；未来要 Windows 支持需改 cmd.exe /C。
fn js_run(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    use std::process::Command;
    let cmd: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let output = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .output()
        .map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(format!("helix.run: failed to spawn: {e}")))))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr: String = stderr.chars().take(RUN_OUTPUT_LIMIT).collect();
        let code = output.status.code().map_or_else(|| "signal".to_string(), |c| c.to_string());
        return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
            "helix.run: command failed ({code}): {stderr}"
        )))));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut stdout: String = stdout.chars().take(RUN_OUTPUT_LIMIT).collect();
    if output.stdout.len() > RUN_OUTPUT_LIMIT {
        stdout.push_str("(truncated)");
    }
    Ok(JsValue::from(JsString::from(stdout)))
}

fn js_echo(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let text: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    MESSAGES
        .get()
        .expect("MESSAGES initialized")
        .lock()
        .expect("messages lock")
        .push(text);
    Ok(JsValue::undefined())
}

/// worker 读块大小
const TERM_CHUNK_SIZE: usize = 4096;

/// 读线程主循环（stdout/stderr 共用）。
/// aggregate=true：原始字节累积进 output，EOF 后统一 lossy 解码（跨块多字节字符不被切开）；
/// aggregate=false：流式增量解码——拼上跨块的残留尾部（最多 3 字节，UTF-8 最长序列），
/// 用 from_utf8 判定有效前缀发出，不完整尾部留到下一块；EOF 时残留尾部 lossy 输出。
fn read_stream<R: std::io::Read>(
    mut stream: R,
    id: u64,
    aggregate: bool,
    output: &std::sync::Mutex<Vec<u8>>,
    tx: &std::sync::mpsc::Sender<TermEvent>,
) {
    let mut buf = [0u8; TERM_CHUNK_SIZE];
    let mut tail: Vec<u8> = Vec::with_capacity(3); // 跨块的不完整 UTF-8 尾部（最长序列 3 字节）
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if aggregate {
            output.lock().unwrap().extend_from_slice(&buf[..n]);
            continue;
        }
        tail.extend_from_slice(&buf[..n]);
        match std::str::from_utf8(&tail) {
            Ok(s) => {
                if tx.send(TermEvent::Chunk(id, s.to_string())).is_err() {
                    return;
                }
                tail.clear();
            }
            Err(e) => {
                let valid = e.valid_up_to();
                if e.error_len().is_some() {
                    // 硬性非法字节（非不完整尾部）：整段 lossy 输出，不留尾部
                    let chunk = String::from_utf8_lossy(&tail).into_owned();
                    if tx.send(TermEvent::Chunk(id, chunk)).is_err() {
                        return;
                    }
                    tail.clear();
                } else if valid > 0 {
                    // 尾部是不完整序列：发出有效前缀，残留字节留到下一块
                    let chunk = String::from_utf8_lossy(&tail[..valid]).into_owned();
                    if tx.send(TermEvent::Chunk(id, chunk)).is_err() {
                        return;
                    }
                    tail.drain(..valid);
                }
            }
        }
    }
    if !aggregate && !tail.is_empty() {
        // EOF：残留的不完整尾部 lossy 输出（无后续字节可拼）
        let chunk = String::from_utf8_lossy(&tail).into_owned();
        let _ = tx.send(TermEvent::Chunk(id, chunk));
    }
}

/// 启动 worker 线程：sh -c 跑子进程，读线程逐块发 Chunk（或聚合进 stdout）；
/// 主线程轮询控制通道（写 stdin / kill）与读线程完成信号，全部读完才收尾发 Exit。
/// 控制通道由 term_write/term_kill 发消息；无人发 Kill 且子进程不退出时 worker 一直存活（终端会话语义）。
fn spawn_worker(
    id: u64,
    cmd: &str,
    aggregate: bool,
    tx: std::sync::mpsc::Sender<TermEvent>,
    stdin_rx: std::sync::mpsc::Receiver<TermCtrl>,
) {
    let cmd = cmd.to_string();
    std::thread::spawn(move || {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                let _ = tx.send(TermEvent::Exit(id, 127, None));
                return;
            }
        };
        let stream_out = child.stdout.take();
        let stream_err = child.stderr.take();
        let mut stdin = child.stdin.take();

        // 读线程：stdout/stderr 各一个，逐块发 Chunk（非聚合）或拼进共享缓冲（聚合）；完成后发 done 信号
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        // 聚合缓冲存原始字节，EOF 后统一 lossy 解码：跨块多字节字符不损坏
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let mut readers = Vec::new();
        {
            let tx = tx.clone();
            let done_tx = done_tx.clone();
            let output = output.clone();
            readers.push(std::thread::spawn(move || {
                read_stream(stream_out.unwrap(), id, aggregate, &output, &tx);
                let _ = done_tx.send(());
            }));
        }
        {
            let tx = tx.clone();
            let done_tx = done_tx.clone();
            let output = output.clone();
            readers.push(std::thread::spawn(move || {
                read_stream(stream_err.unwrap(), id, aggregate, &output, &tx);
                let _ = done_tx.send(());
            }));
        }
        drop(done_tx);

        // 主循环：处理控制消息；两个读线程都 EOF 后收尾
        loop {
            while let Ok(msg) = stdin_rx.try_recv() {
                match msg {
                    TermCtrl::Write(text) => {
                        if let Some(s) = stdin.as_mut() {
                            let _ = s.write_all(text.as_bytes());
                            let _ = s.flush();
                        }
                    }
                    TermCtrl::Kill => {
                        let _ = child.kill();
                    }
                }
            }
            let mut finished = 0;
            while done_rx.try_recv().is_ok() {
                finished += 1;
            }
            if finished >= readers.len() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        drop(stdin); // 关 stdin → 仍等输入的子进程读到 EOF 后退出
        for h in readers {
            let _ = h.join();
        }
        let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        let bytes = std::mem::take(&mut *output.lock().unwrap());
        let stdout = if aggregate {
            // 全部原始字节统一 lossy 解码（跨块字符在整流上解码，不产生 U+FFFD）
            Some(String::from_utf8_lossy(&bytes).into_owned())
        } else {
            None
        };
        let _ = tx.send(TermEvent::Exit(id, code, stdout));
    });
}

/// PTY worker：子进程 stdin/stdout/stderr 接 slave；父线程读 master 发 Chunk（流式），
/// TermCtrl::Write 写 master 当 stdin，Kill 杀子进程。单读线程（pty 无 stderr 区分）。
/// worker 持 master File（Drop 关闭）；slave 经 Stdio::from 交给子进程，spawn 后父侧关闭。
#[cfg(unix)]
fn spawn_pty_worker(
    id: u64,
    cmd: &str,
    tx: std::sync::mpsc::Sender<TermEvent>,
    ctrl_rx: std::sync::mpsc::Receiver<TermCtrl>,
    master: pty::Master,
    slave: std::fs::File,
) {
    let cmd = cmd.to_string();
    std::thread::spawn(move || {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut master = master.into_file();
        // 子进程 stdin/stdout/stderr 都是 slave（dup 三份，spawn 后父侧副本关闭）
        let stdin_slave = match slave.try_clone() {
            Ok(f) => f,
            Err(_) => {
                let _ = tx.send(TermEvent::Exit(id, 127, None));
                return;
            }
        };
        let stdout_slave = match slave.try_clone() {
            Ok(f) => f,
            Err(_) => {
                let _ = tx.send(TermEvent::Exit(id, 127, None));
                return;
            }
        };
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(stdin_slave))
            .stdout(Stdio::from(stdout_slave))
            .stderr(Stdio::from(slave))
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                let _ = tx.send(TermEvent::Exit(id, 127, None));
                return;
            }
        };
        // master 读端单独 dup（读写两端并发：写线程主循环 + 读线程）
        let master_reader = match master.try_clone() {
            Ok(f) => f,
            Err(_) => {
                let _ = child.kill();
                let _ = tx.send(TermEvent::Exit(id, 127, None));
                return;
            }
        };

        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        // pty 无聚合模式：output 缓冲不会被 read_stream 使用，占位传引用
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        {
            let tx = tx.clone();
            let done_tx = done_tx.clone();
            let output = output.clone();
            std::thread::spawn(move || {
                read_stream(master_reader, id, false, &output, &tx);
                let _ = done_tx.send(());
            });
        }
        drop(done_tx);

        // 主循环：写 master / kill；读线程 EOF（子进程退出关闭 slave → master 读 EIO）后收尾
        loop {
            while let Ok(msg) = ctrl_rx.try_recv() {
                match msg {
                    TermCtrl::Write(text) => {
                        let _ = master.write_all(text.as_bytes());
                        let _ = master.flush();
                    }
                    TermCtrl::Kill => {
                        let _ = child.kill();
                    }
                }
            }
            if done_rx.try_recv().is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        let _ = tx.send(TermEvent::Exit(id, code, None));
        // master File drop → 关闭 fd（TERM_MASTERS 里的裸 fd 变陈旧，resize 报 EBADF，良性）
    });
}

fn js_run_async(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let cmd: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let cb = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if cb.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.run_async: callback must be a function",
        ))));
    }
    let id = NEXT_TERM_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
    with_terms(|m| {
        m.insert(id, TermCallbacks { on_chunk: cb.clone(), on_exit: Some(cb), is_run_async: true })
    });
    let (tx, rx) = std::sync::mpsc::channel();
    TERM_WORKERS.with(|m| m.borrow_mut().insert(id, tx.clone()));
    spawn_worker(
        id,
        &cmd,
        true,
        TERM_EVENTS.with(|t| t.borrow().clone().expect("TERM_EVENTS initialized")),
        rx,
    );
    Ok(JsValue::from(id))
}

fn js_spawn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let opts = args.first().unwrap_or(&JsValue::undefined()).as_object().ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.spawn: options object required")))
    })?;
    let cmd: String = opts.get(JsString::from("cmd"), ctx)?.try_js_into(ctx)?;
    let on_chunk = opts.get(JsString::from("onChunk"), ctx)?;
    if on_chunk.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.spawn: onChunk must be a function",
        ))));
    }
    let on_exit = opts.get(JsString::from("onExit"), ctx)?;
    let on_exit = on_exit.as_callable().map(|_| on_exit);
    // pty: bool，缺省 false（管道模式）。非布尔 → 报错
    let pty = {
        let v = opts.get(JsString::from("pty"), ctx)?;
        if v.is_null_or_undefined() {
            false
        } else {
            v.try_js_into::<bool>(ctx).map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from(
                    "helix.spawn: 'pty' must be a boolean",
                )))
            })?
        }
    };
    let id = NEXT_TERM_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
    with_terms(|m| {
        m.insert(id, TermCallbacks { on_chunk, on_exit, is_run_async: false })
    });
    let (tx, rx) = std::sync::mpsc::channel();
    TERM_WORKERS.with(|m| m.borrow_mut().insert(id, tx));
    let term_tx = TERM_EVENTS.with(|t| t.borrow().clone().expect("TERM_EVENTS initialized"));
    if pty {
        #[cfg(unix)]
        {
            let (master, slave) = pty::open_pty().map_err(|e| {
                JsError::from_opaque(JsValue::from(JsString::from(format!(
                    "helix.spawn: pty: {e}"
                ))))
            })?;
            // 先注册 master fd 再起 worker：spawn 返回后 JS 立即可 term_resize
            TERM_MASTERS.with(|m| m.borrow_mut().insert(id, master.fd()));
            spawn_pty_worker(id, &cmd, term_tx, rx, master, slave);
        }
        #[cfg(not(unix))]
        {
            return Err(JsError::from_opaque(JsValue::from(JsString::from(
                "helix.spawn: pty requires a unix platform",
            ))));
        }
    } else {
        spawn_worker(id, &cmd, false, term_tx, rx);
    }
    Ok(JsValue::from(id))
}

fn js_term_write(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let text: String = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let sender = TERM_WORKERS.with(|m| m.borrow().get(&id).cloned()).ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.term_write: unknown id")))
    })?;
    sender
        .send(TermCtrl::Write(text))
        .map_err(|_| JsError::from_opaque(JsValue::from(JsString::from("helix.term_write: worker gone"))))?;
    Ok(JsValue::undefined())
}

fn js_term_kill(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let sender = TERM_WORKERS.with(|m| m.borrow_mut().remove(&id)).ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.term_kill: unknown id")))
    })?;
    sender
        .send(TermCtrl::Kill)
        .map_err(|_| JsError::from_opaque(JsValue::from(JsString::from("helix.term_kill: worker gone"))))?;
    Ok(JsValue::undefined())
}

/// 调整 PTY 窗口尺寸（rows/cols）。直连 master fd 做 TIOCSWINSZ：
/// winsize 是 tty 设备属性，spawn 后立即调用也在子进程启动前生效，无消息时序竞态。
/// 非 pty worker / 未知 id → Err。
#[cfg(unix)]
pub fn term_resize(id: u64, rows: u16, cols: u16) -> Result<()> {
    init();
    let fd = TERM_MASTERS
        .with(|m| m.borrow().get(&id).copied())
        .ok_or_else(|| anyhow!("term_resize: unknown id (not a pty worker)"))?;
    pty::set_winsize(fd, rows, cols)
}

#[cfg(unix)]
fn js_term_resize(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.term_resize: id must be a number")))
    })?;
    let rows: u16 = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.term_resize: rows must be a number")))
    })?;
    let cols: u16 = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.term_resize: cols must be a number")))
    })?;
    let fd = TERM_MASTERS.with(|m| m.borrow().get(&id).copied()).ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.term_resize: unknown id")))
    })?;
    pty::set_winsize(fd, rows, cols)
        .map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(format!("helix.term_resize: {e}")))))?;
    Ok(JsValue::undefined())
}

/// 基础 glob：基目录 = 模式中首个元字符（`*`/`?`/`[`）之前的字面前缀递归 walk
/// （如 `dir/**/*.js` → `dir`，裸模式 `*.js` → `.`；无元字符 → 整模式为字面路径取父目录），
/// globset 匹配完整路径字符串。literal_separator(true)：`*`/`?` 不跨目录分隔符
/// （`**` 仍跨目录，含零层，globset 语义）。
/// 前导 `./` 归一化：globset 裸模式不匹配 `./x` 候选（`*` 不跨 `/`），模式与候选统一去掉。
fn glob_matches(pattern: &str) -> std::result::Result<Vec<String>, String> {
    use globset::GlobBuilder;
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    let matcher = GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| format!("invalid glob '{pattern}': {e}"))?
        .compile_matcher();
    let base = match pattern.find(['*', '?', '[']) {
        Some(0) => std::path::PathBuf::from("."),
        Some(i) => std::path::PathBuf::from(&pattern[..i]),
        None => std::path::Path::new(pattern)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from(".")),
    };
    let mut out = Vec::new();
    walk_glob(&base, &matcher, &mut out).map_err(|e| format!("glob_async('{pattern}'): {e}"))?;
    out.sort();
    Ok(out)
}

/// 递归 walk 目录树，匹配完整路径字符串（含目录本身——glob 常规语义）
fn walk_glob(dir: &std::path::Path, matcher: &globset::GlobMatcher, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // 基目录为 `.` 时路径带前导 `./`，globset 裸模式（`*.js`）不匹配 → 统一去掉
        let pstr = path.to_string_lossy();
        let pstr = pstr.strip_prefix("./").unwrap_or(&pstr);
        if matcher.is_match(pstr) {
            out.push(pstr.to_owned());
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            walk_glob(&path, matcher, out)?;
        }
    }
    Ok(())
}

/// 注册异步 fs 回调并 spawn 一次性 worker：操作在线程里执行，
/// 结果（Ok 或 Err 字符串）经 ASYNC_EVENTS 通道送回发起线程。
fn spawn_async_op<T: Send + 'static>(
    id: u64,
    op: impl FnOnce() -> std::result::Result<T, String> + Send + 'static,
    mk: impl FnOnce(u64, std::result::Result<T, String>) -> AsyncEvent + Send + 'static,
) {
    let tx = ASYNC_EVENTS.with(|t| t.borrow().clone().expect("ASYNC_EVENTS initialized"));
    std::thread::spawn(move || {
        let _ = tx.send(mk(id, op()));
    });
}

/// 校验回调参数（第二个/第三个参数必须是可调用函数）
fn async_cb(args: &[JsValue], pos: usize, api: &str) -> boa_engine::JsResult<JsValue> {
    let cb = args.get(pos).cloned().unwrap_or(JsValue::undefined());
    if cb.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
            "helix.{api}: callback must be a function"
        )))));
    }
    Ok(cb)
}

fn js_read_file_async(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.read_file_async: path must be a string")))
    })?;
    let cb = async_cb(args, 1, "read_file_async")?;
    let id = NEXT_ASYNC_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
    with_async_callbacks(|m| { m.insert(id, cb); });
    spawn_async_op(id, move || {
        std::fs::read_to_string(&path).map_err(|e| format!("read_file_async('{path}'): {e}"))
    }, AsyncEvent::FsRead);
    Ok(JsValue::from(id))
}

fn js_write_file_async(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.write_file_async: path must be a string")))
    })?;
    let content: String = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.write_file_async: content must be a string")))
    })?;
    let cb = async_cb(args, 2, "write_file_async")?;
    let id = NEXT_ASYNC_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
    with_async_callbacks(|m| { m.insert(id, cb); });
    spawn_async_op(id, move || {
        std::fs::write(&path, &content).map_err(|e| format!("write_file_async('{path}'): {e}"))
    }, AsyncEvent::FsWrite);
    Ok(JsValue::from(id))
}

fn js_stat_async(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.stat_async: path must be a string")))
    })?;
    let cb = async_cb(args, 1, "stat_async")?;
    let id = NEXT_ASYNC_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
    with_async_callbacks(|m| { m.insert(id, cb); });
    spawn_async_op(id, move || {
        std::fs::metadata(&path)
            .map(|m| FsStat {
                size: m.len(),
                is_dir: m.is_dir(),
                mtime: m.modified()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).map_err(std::io::Error::other))
                    .unwrap_or(0),
            })
            .map_err(|e| format!("stat_async('{path}'): {e}"))
    }, AsyncEvent::FsStat);
    Ok(JsValue::from(id))
}

fn js_glob_async(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let pattern: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.glob_async: pattern must be a string")))
    })?;
    let cb = async_cb(args, 1, "glob_async")?;
    let id = NEXT_ASYNC_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
    with_async_callbacks(|m| { m.insert(id, cb); });
    spawn_async_op(id, move || glob_matches(&pattern), AsyncEvent::FsGlob);
    Ok(JsValue::from(id))
}

/// 取走全部待处理进程事件（主线程轮询用）
pub fn drain_term_events() -> Vec<TermEvent> {
    init();
    let mut events = Vec::new();
    TERM_EVENTS_RX.with(|r| {
        if let Some(rx) = r.borrow_mut().as_mut() {
            while let Ok(e) = rx.try_recv() {
                events.push(e);
            }
        }
    });
    events
}

/// 把一条进程事件投递到对应 id 的 JS 回调。
/// Chunk → onChunk(chunk)；Exit → run_async 调 onExit(null, stdout)、spawn 调 onExit(code)；
/// 进程结束（Exit）后清理回调与 worker 注册，防止重复回调。
pub fn resolve_term_event(id: u64, event: TermEvent) -> Result<()> {
    init();
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized");
        let callbacks = with_terms(|m| {
            m.get(&id).map(|c| TermCallbacks {
                on_chunk: c.on_chunk.clone(),
                on_exit: c.on_exit.clone(),
                is_run_async: c.is_run_async,
            })
        });
        let Some(callbacks) = callbacks else { return Ok(()) };
        let undefined = JsValue::undefined();
        match event {
            TermEvent::Chunk(_, chunk) => {
                let func = callbacks.on_chunk.as_callable().and_then(JsFunction::from_object)
                    .ok_or_else(|| anyhow!("term {id} onChunk not callable"))?;
                let _: JsValue = func.call(&undefined, &[JsValue::from(JsString::from(chunk))], engine)
                    .map_err(|e| anyhow!("term {id} onChunk failed: {e}"))?;
            }
            TermEvent::Exit(_, code, stdout) => {
                if let Some(on_exit) = callbacks.on_exit {
                    let func = on_exit.as_callable().and_then(JsFunction::from_object)
                        .ok_or_else(|| anyhow!("term {id} onExit not callable"))?;
                    let args = if callbacks.is_run_async {
                        vec![JsValue::null(), JsValue::from(JsString::from(stdout.unwrap_or_default()))]
                    } else {
                        vec![JsValue::from(code)]
                    };
                    let _: JsValue = func.call(&undefined, &args, engine)
                        .map_err(|e| anyhow!("term {id} onExit failed: {e}"))?;
                }
                with_terms(|m| m.remove(&id));
                TERM_WORKERS.with(|m| m.borrow_mut().remove(&id));
                #[cfg(unix)]
                TERM_MASTERS.with(|m| m.borrow_mut().remove(&id));
            }
        }
        Ok(())
    })
}

/// 取走全部待处理异步 fs 事件（主线程轮询用）
pub fn drain_async_events() -> Vec<AsyncEvent> {
    init();
    let mut events = Vec::new();
    ASYNC_EVENTS_RX.with(|r| {
        if let Some(rx) = r.borrow_mut().as_mut() {
            while let Ok(e) = rx.try_recv() {
                events.push(e);
            }
        }
    });
    events
}

/// stat 快照 → JS 对象 { size, is_dir, mtime }
fn stat_to_js(st: &FsStat, ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    Ok(JsValue::from(
        ObjectInitializer::new(ctx)
            .property(JsString::from("size"), JsValue::from(st.size as f64), Attribute::all())
            .property(JsString::from("is_dir"), JsValue::from(st.is_dir), Attribute::all())
            .property(JsString::from("mtime"), JsValue::from(st.mtime as f64), Attribute::all())
            .build(),
    ))
}

/// 把一条异步 fs 事件投递到对应 id 的 JS 回调。
/// 回调签名：read → (err, content)；write → (err)；stat → (err, {size,is_dir,mtime})；glob → (err, paths[])。
/// err 成功为 null、失败为错误字符串。一次性语义：resolve 后从注册表移除（回调失败也移除）。
pub fn resolve_async_event(id: u64, event: AsyncEvent) -> Result<()> {
    init();
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized");
        let cb = with_async_callbacks(|m| m.remove(&id));
        let Some(cb) = cb else { return Ok(()) }; // 已 resolve / 未知 id → no-op（幂等）
        let func = cb.as_callable().and_then(JsFunction::from_object)
            .ok_or_else(|| anyhow!("async fs {id} callback not callable"))?;
        let (err, arg): (JsValue, Option<JsValue>) = match event {
            AsyncEvent::FsRead(_, Ok(content)) => (JsValue::null(), Some(JsValue::from(JsString::from(content)))),
            AsyncEvent::FsRead(_, Err(e)) => (JsValue::from(JsString::from(e)), None),
            AsyncEvent::FsWrite(_, Ok(())) => (JsValue::null(), None),
            AsyncEvent::FsWrite(_, Err(e)) => (JsValue::from(JsString::from(e)), None),
            AsyncEvent::FsStat(_, Ok(st)) => (
                JsValue::null(),
                Some(stat_to_js(&st, engine).map_err(|e| anyhow!("async fs {id} stat result failed: {e}"))?),
            ),
            AsyncEvent::FsStat(_, Err(e)) => (JsValue::from(JsString::from(e)), None),
            AsyncEvent::FsGlob(_, Ok(paths)) => {
                let arr = JsArray::new(engine);
                for p in &paths {
                    arr.push(JsValue::from(JsString::from(p.clone())), engine)
                        .map_err(|e| anyhow!("async fs {id} glob result failed: {e}"))?;
                }
                (JsValue::null(), Some(JsValue::from(arr)))
            }
            AsyncEvent::FsGlob(_, Err(e)) => (JsValue::from(JsString::from(e)), None),
        };
        let undefined = JsValue::undefined();
        let mut call_args = vec![err];
        if let Some(arg) = arg {
            call_args.push(arg);
        }
        let _: JsValue = func.call(&undefined, &call_args, engine)
            .map_err(|e| anyhow!("async fs {id} callback failed: {e}"))?;
        Ok(())
    })
}

fn js_doc_insert(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let row: usize = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let col: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let insert: String = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    CURRENT_EDITS.with(|c| c.borrow_mut().push(Edit { start: (row, col), end: (row, col), insert }));
    Ok(JsValue::undefined())
}

fn js_doc_replace(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let sr: usize = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let sc: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let er: usize = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let ec: usize = args.get(3).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let insert: String = args.get(4).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    CURRENT_EDITS.with(|c| c.borrow_mut().push(Edit { start: (sr, sc), end: (er, ec), insert }));
    Ok(JsValue::undefined())
}

fn js_doc_delete(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let sr: usize = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let sc: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let er: usize = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let ec: usize = args.get(3).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    CURRENT_EDITS.with(|c| c.borrow_mut().push(Edit { start: (sr, sc), end: (er, ec), insert: String::new() }));
    Ok(JsValue::undefined())
}

/// 取走并清空编辑队列（helix-term 在命令返回后消费）
pub fn take_edits() -> Vec<Edit> {
    init();
    CURRENT_EDITS.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// 取走并清空光标/选区请求队列（helix-term 消费）
pub fn take_cursor_requests() -> Vec<CursorRequest> {
    init();
    CURSOR_REQUESTS.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// 求值一段插件脚本（无名字，用于测试/内联）；脚本里可调用 `helix.register_command` / `helix.echo`
pub fn load_script(src: &str) -> Result<()> {
    load_script_named("<anon>", src)
}

/// IIFE 包裹求值：每次求值（含 reload 重跑）都得到全新词法作用域，
/// 顶层 const/let 不再与上一次求值冲突；顺带隔离脚本间全局污染。
/// `helix` 是全局属性，IIFE 内可正常访问；错误消息定位脚本名。
fn eval_wrapped(engine: &mut boa_engine::Context, src: &str) -> boa_engine::JsResult<JsValue> {
    let wrapped = format!("(function() {{\n{src}\n}})()");
    engine.eval(Source::from_bytes(&wrapped))
}

/// 求值并记录脚本（名字用于报错定位与热重载）
pub fn load_script_named(name: &str, src: &str) -> Result<()> {
    init();
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized");
        eval_wrapped(engine, src)
            .map(|_| ())
            .map_err(|e| anyhow!("plugin script '{name}' error: {e}"))?;
        LOADED_SCRIPTS.with(|s| s.borrow_mut().push((name.to_string(), src.to_string())));
        Ok(())
    })
}

/// 已加载脚本名（按加载顺序；供 :plugin list / status）
pub fn loaded_scripts() -> Vec<String> {
    init();
    LOADED_SCRIPTS.with(|s| s.borrow().iter().map(|(name, _)| name.clone()).collect())
}

/// 清空所有插件状态（热重载用；id 计数器保留保证唯一性）。
/// 注意：UI_REQUESTS/MESSAGES 不清——它们是命令边界 drain 的队列。
fn reset_plugin_state() {
    with_registry(|r| r.clear());
    with_event_handlers(|h| h.clear());
    with_popups(|p| p.clear());
    CURRENT_EDITS.with(|c| c.borrow_mut().clear());
    CURSOR_REQUESTS.with(|c| c.borrow_mut().clear());
    with_buffer_icon_hook(|h| *h = None);
    with_statusline_hook(|h| *h = None);
    LAST_PANEL_ID.with(|c| c.set(None));
    COMMAND_DOCS.with(|d| d.borrow_mut().clear());
    // 导出缓存与 pending 导出随插件状态重置（reload 后按名重读磁盘重跑）
    with_script_exports(|m| m.clear());
    with_last_export(|l| *l = None);
    // 主题覆盖随插件状态重置：清空并置脏（下次 drain 还原基准主题）
    THEME_OVERRIDES.with(|o| o.borrow_mut().clear());
    THEME_DIRTY.with(|d| d.set(true));
}

/// 热重载：清空状态后按加载顺序重跑全部脚本。
/// 脚本以 IIFE 包裹求值，每次重跑都是全新词法作用域——顶层 const/let/var 都不冲突，
/// reload 必然成功（脚本不依赖其他脚本的状态，reload 会重新注册全部命令/处理器），
/// 因此 Err 分支仅作防御（如脚本依赖被清空的全局状态），单测只覆盖成功路径。
pub fn reload_all() -> Result<()> {
    init();
    // 面板层还挂在 compositor 上——入队 ClosePanel 让 plugin_reload 的 apply_ui_requests 移除它，
    // 否则 reset_plugin_state 清掉 LAST_PANEL_ID 后该层变成无法关闭的僵尸层
    if let Some(panel_id) = LAST_PANEL_ID.with(|c| c.get()) {
        UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::ClosePanel { id: panel_id });
    }
    let scripts = LOADED_SCRIPTS.with(|s| s.borrow().clone());
    reset_plugin_state();
    for (name, src) in &scripts {
        // 按名重读磁盘：js_load 记录的模块文件更新生效（相对名解析 PLUGINS_DIR，
        // 绝对路径直接用）；读不到（load_script_named 的字符串脚本/目录已删）用记录 src 兜底
        let disk_src = if Path::new(name).is_absolute() {
            std::fs::read_to_string(name).ok()
        } else {
            PLUGINS_DIR
                .get()
                .map(|d| d.join(name))
                .filter(|p| p.is_file())
                .and_then(|p| std::fs::read_to_string(p).ok())
        };
        let src = disk_src.as_deref().unwrap_or(src);
        CONTEXT.with(|cell| -> Result<()> {
            let mut binding = cell.borrow_mut();
            let engine = binding.as_mut().expect("CONTEXT initialized");
            eval_wrapped(engine, src)
                .map(|_| ())
                .map_err(|e| anyhow!("plugin reload '{name}' failed: {e}"))?;
            Ok(())
        })?;
    }
    Ok(())
}

/// 取走并清空 echo 消息队列
pub fn take_messages() -> Vec<String> {
    init();
    std::mem::take(&mut *MESSAGES.get().expect("MESSAGES not initialized").lock().expect("messages lock poisoned"))
}

/// 解析 open_popup 选项里的可选 u16（undefined/null → None）；非数、负数、非整数、超上限报错
fn opt_u16(v: &JsValue, ctx: &mut Context, name: &str) -> boa_engine::JsResult<Option<u16>> {
    if v.is_null_or_undefined() {
        return Ok(None);
    }
    let n: f64 = v.try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "open_popup: '{name}' must be a number"
        ))))
    })?;
    if !n.is_finite() || n < 0.0 || n > u16::MAX as f64 || n.fract() != 0.0 {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
            "open_popup: '{name}' must be an integer in [0, {}]",
            u16::MAX
        )))));
    }
    Ok(Some(n as u16))
}

fn js_open_popup(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let opts = args.first().unwrap_or(&JsValue::undefined()).as_object().ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from("open_popup: options object required")))
    })?;
    let render = opts.get(JsString::from("render"), ctx)?;
    if render.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "open_popup: render must be a function",
        ))));
    }
    let on_key = opts.get(JsString::from("onKey"), ctx)?;
    let on_key = on_key.as_callable().map(|_| on_key);
    let on_close = opts.get(JsString::from("onClose"), ctx)?;
    let on_close = on_close.as_callable().map(|_| on_close);
    // 尺寸/位置在注册前解析：任一非法则整体失败，不产生半注册
    let width = opt_u16(&opts.get(JsString::from("width"), ctx)?, ctx, "width")?;
    let height = opt_u16(&opts.get(JsString::from("height"), ctx)?, ctx, "height")?;
    let position = {
        let v = opts.get(JsString::from("position"), ctx)?;
        if v.is_null_or_undefined() {
            None
        } else {
            let obj = v.as_object().ok_or_else(|| {
                JsError::from_opaque(JsValue::from(JsString::from(
                    "open_popup: 'position' must be an object with row/col",
                )))
            })?;
            let row = opt_u16(&obj.get(JsString::from("row"), ctx)?, ctx, "position.row")?
                .ok_or_else(|| {
                    JsError::from_opaque(JsValue::from(JsString::from(
                        "open_popup: 'position.row' is required",
                    )))
                })?;
            let col = opt_u16(&obj.get(JsString::from("col"), ctx)?, ctx, "position.col")?
                .ok_or_else(|| {
                    JsError::from_opaque(JsValue::from(JsString::from(
                        "open_popup: 'position.col' is required",
                    )))
                })?;
            Some((row, col))
        }
    };

    let id = NEXT_POPUP_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
    with_popups(|p| p.insert(id, PopupCallbacks { render, on_key, on_close }));
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::OpenPopup { id, width, height, position });
    Ok(JsValue::from(id))
}

/// open_panel 允许的 side 白名单
const PANEL_SIDES: [&str; 3] = ["right", "left", "bottom"];

/// 侧边面板：校验 side 白名单 / size / render 后注册回调（onKey 可选，同 open_popup），
/// 入队 OpenPanel。id 与弹窗共用 NEXT_POPUP_ID 空间，面板渲染复用 render_popup 同一注册表。
fn js_open_panel(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let opts = args.first().unwrap_or(&JsValue::undefined()).as_object().ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from("open_panel: options object required")))
    })?;
    let side: String = opts.get(JsString::from("side"), ctx)?.try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("open_panel: 'side' must be a string")))
    })?;
    if !PANEL_SIDES.contains(&side.as_str()) {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
            "open_panel: unknown side '{side}' (expected right|left|bottom)"
        )))));
    }
    let render = opts.get(JsString::from("render"), ctx)?;
    if render.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "open_panel: render must be a function",
        ))));
    }
    let on_close = opts.get(JsString::from("onClose"), ctx)?;
    let on_close = on_close.as_callable().map(|_| on_close);
    let on_key = opts.get(JsString::from("onKey"), ctx)?;
    let on_key = on_key.as_callable().map(|_| on_key);
    let size = {
        let v = opts.get(JsString::from("size"), ctx)?;
        let n: f64 = v.try_js_into(ctx).map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from("open_panel: 'size' must be a number")))
        })?;
        if !n.is_finite() || n < 1.0 || n > u16::MAX as f64 || n.fract() != 0.0 {
            return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
                "open_panel: 'size' must be an integer in [1, {}]",
                u16::MAX
            )))));
        }
        n as u16
    };

    let id = NEXT_POPUP_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
    LAST_PANEL_ID.with(|c| c.set(Some(id)));
    with_popups(|p| p.insert(id, PopupCallbacks { render, on_key, on_close }));
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::OpenPanel { id, side, size });
    Ok(JsValue::from(id))
}

/// 入队 ClosePanel（id 校验）；JS 侧与 :panel-close 共用
fn js_close_panel(_this: &JsValue, args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(_ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("close_panel: id must be a number")))
    })?;
    LAST_PANEL_ID.with(|c| { if c.get() == Some(id) { c.set(None); } });
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::ClosePanel { id });
    Ok(JsValue::undefined())
}

/// 同步列目录（不递归）：read_dir → 错误条目跳过 → 按名字排序 → [{ name, is_dir, path }]
fn js_read_dir(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("read_dir: path must be a string")))
    })?;
    let mut entries: Vec<(String, bool, String)> = std::fs::read_dir(&path)
        .map_err(|e| {
            JsError::from_opaque(JsValue::from(JsString::from(format!("read_dir('{path}'): {e}"))))
        })?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let full = entry.path().to_string_lossy().into_owned();
            Some((name, is_dir, full))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let arr = JsArray::new(ctx);
    for (name, is_dir, path) in entries {
        let obj = ObjectInitializer::new(ctx)
            .property(JsString::from("name"), JsValue::from(JsString::from(name)), Attribute::all())
            .property(JsString::from("is_dir"), JsValue::from(is_dir), Attribute::all())
            .property(JsString::from("path"), JsValue::from(JsString::from(path)), Attribute::all())
            .build();
        arr.push(JsValue::from(obj), ctx)?;
    }
    Ok(JsValue::from(arr))
}

/// 入队 OpenFile（path 字符串校验）
fn js_open_file(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("open_file: path must be a string")))
    })?;
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::OpenFile { path });
    Ok(JsValue::undefined())
}

/// 入队 MovePanel（id 数字 + side 白名单校验）
fn js_move_panel(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("move_panel: id must be a number")))
    })?;
    let side: String = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("move_panel: side must be a string")))
    })?;
    if !PANEL_SIDES.contains(&side.as_str()) {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
            "move_panel: unknown side '{side}' (expected right|left|bottom)"
        )))));
    }
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::MovePanel { id, side });
    Ok(JsValue::undefined())
}

fn js_set_buffer_icon(_this: &JsValue, args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let undefined = JsValue::undefined();
    let hook = args.first().unwrap_or(&undefined);
    if !hook.is_callable() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "set_buffer_icon: expected a function",
        ))));
    }
    with_buffer_icon_hook(|h| *h = Some(hook.clone()));
    Ok(JsValue::undefined())
}

/// 取走并清空 UI 请求队列
pub fn take_ui_requests() -> Vec<UiRequest> {
    init();
    std::mem::take(&mut *UI_REQUESTS.get().expect("UI_REQUESTS initialized").lock().expect("ui requests lock"))
}

/// 解析 render 返回数组的一个元素：对象（含 text 属性）→ StyledLine{text, style}；字符串 → (text, None)；否则 Err
fn parse_line_item(item: &JsValue, ctx: &mut Context, id: u64, i: usize) -> boa_engine::JsResult<StyledLine> {
    if let Some(obj) = item.as_object() {
        let text: String = obj.get(JsString::from("text"), ctx)?.try_js_into(ctx).map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "popup {id} render line {i}: object must have a string 'text' property"
            ))))
        })?;
        let style = obj.get(JsString::from("style"), ctx)?;
        let style = if style.is_null_or_undefined() {
            None
        } else {
            Some(style.try_js_into::<String>(ctx).map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from(format!(
                    "popup {id} render line {i}: 'style' must be a string"
                ))))
            })?)
        };
        Ok(StyledLine { text, style })
    } else if let Ok(text) = item.try_js_into::<String>(ctx) {
        Ok(StyledLine { text, style: None })
    } else {
        Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
            "popup {id} render line {i} must be a string or an object with 'text'"
        )))))
    }
}

/// 调 JS render 回调，返回样式化行数组。ctx 对象 { width, height }。
pub fn render_popup(id: u64, width: u16, height: u16) -> Result<Vec<StyledLine>> {
    init();
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized");
        let render = with_popups(|p| p.get(&id).map(|cb| cb.render.clone()))
            .ok_or_else(|| anyhow!("popup {id} not open"))?;
        let ctx_obj = ObjectInitializer::new(engine)
            .property(JsString::from("width"), width, Attribute::all())
            .property(JsString::from("height"), height, Attribute::all())
            .build();
        let func = render.as_callable().and_then(JsFunction::from_object)
            .ok_or_else(|| anyhow!("popup {id} render is not a function"))?;
        let undefined = JsValue::undefined();
        let value: JsValue = func.call(&undefined, &[JsValue::from(ctx_obj)], engine)
            .map_err(|e| anyhow!("popup {id} render failed: {e}"))?;
        // 结果必须是 string[]
        let arr: JsArray = value.try_js_into(engine)
            .map_err(|e| anyhow!("popup {id} render must return an array of strings or styled objects: {e}"))?;
        let len: usize = arr
            .get(JsString::from("length"), engine)
            .map_err(|e| anyhow!("popup {id} length read failed: {e}"))?
            .try_js_into(engine)
            .map_err(|e| anyhow!("popup {id} render length invalid: {e}"))?;
        let mut lines = Vec::with_capacity(len);
        for i in 0..len {
            let item = arr
                .get(i, engine)
                .map_err(|e| anyhow!("popup {id} render line {i} read failed: {e}"))?;
            lines.push(parse_line_item(&item, engine, id, i)
                .map_err(|e| anyhow!("popup {id} render failed: {e}"))?);
        }
        Ok(lines)
    })
}

/// 调 JS onKey 回调（第二个参数是可编辑的 doc 快照）。
/// 未注册 onKey 或缺省时：Esc→Close，其他→Ignore。
pub fn popup_key(id: u64, key: &PluginKey, ctx: &CommandContext) -> Result<PopupKeyResult> {
    init();
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized");
        let callbacks = with_popups(|p| p.get(&id).cloned())
            .ok_or_else(|| anyhow!("popup {id} not open"))?;
        let Some(on_key) = callbacks.on_key else {
            return Ok(if key.name == "Esc" { PopupKeyResult::Close } else { PopupKeyResult::Ignored });
        };
        let func = on_key.as_callable().and_then(JsFunction::from_object)
            .ok_or_else(|| anyhow!("popup {id} onKey is not a function"))?;
        let key_obj = ObjectInitializer::new(engine)
            .property(JsString::from("name"), JsString::from(key.name.clone()), Attribute::all())
            .property(JsString::from("shift"), key.shift, Attribute::all())
            .property(JsString::from("ctrl"), key.ctrl, Attribute::all())
            .property(JsString::from("alt"), key.alt, Attribute::all())
            .build();
        let doc = doc_to_js(ctx, engine).map_err(|e| anyhow!("failed to build popup doc: {e}"))?;
        let undefined = JsValue::undefined();
        let value: JsValue = func.call(&undefined, &[JsValue::from(key_obj), doc], engine)
            .map_err(|e| anyhow!("popup {id} onKey failed: {e}"))?;
        let s: Option<String> = value.try_js_into(engine).ok();
        Ok(match s.as_deref() {
            Some("close") => PopupKeyResult::Close,
            Some("handled") => PopupKeyResult::Handled,
            Some("ignore") => PopupKeyResult::Ignored,
            _ => PopupKeyResult::Handled, // 未识别返回值 → 消费（安全默认）
        })
    })
}

/// 面板是否注册了 onKey 回调。helix-term 侧据此决定是否把按键交给 popup_key：
/// 无 onKey 的面板缺省全 Ignore（事件穿透），不调 popup_key（其缺省 Esc→Close 语义不适用于面板）。
pub fn panel_has_onkey(id: u64) -> bool {
    init();
    with_popups(|p| p.get(&id).map(|cb| cb.on_key.is_some()).unwrap_or(false))
}

/// 关闭弹窗：触发 onClose 并移除注册表项。幂等（已关闭返回 Ok）。
pub fn close_popup(id: u64) -> Result<()> {
    init();
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized");
        let callbacks = with_popups(|p| p.remove(&id));
        let Some(callbacks) = callbacks else { return Ok(()) };
        if let Some(on_close) = callbacks.on_close {
            let func = on_close.as_callable().and_then(JsFunction::from_object)
                .ok_or_else(|| anyhow!("popup {id} onClose is not a function"))?;
            let undefined = JsValue::undefined();
            let _: JsValue = func.call(&undefined, &[], engine)
                .map_err(|e| anyhow!("popup {id} onClose failed: {e}"))?;
        }
        Ok(())
    })
}

/// 入队关闭最近一次 open_panel 的面板（:panel-close 用）；无面板时 Err
pub fn close_last_panel() -> Result<()> {
    init();
    let id = LAST_PANEL_ID.with(|c| c.get()).ok_or_else(|| anyhow!("no panel open"))?;
    LAST_PANEL_ID.with(|c| c.set(None));
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::ClosePanel { id });
    Ok(())
}

fn js_set_statusline(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let arg = args.first().cloned().unwrap_or(JsValue::null());
    if arg.is_null_or_undefined() {
        with_statusline_hook(|h| *h = None);
    } else if arg.as_callable().is_some() {
        with_statusline_hook(|h| *h = Some(arg));
    } else {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.set_statusline: expected a function or null",
        ))));
    }
    Ok(JsValue::undefined())
}

/// 调状态栏钩子；无钩子 / 返回 null / 非字符串 / 抛错 → None
pub fn statusline_text(ctx: &StatuslineCtx) -> Option<String> {
    init();
    let hook = with_statusline_hook(|h| h.clone())?;
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized");
        let func = hook.as_callable().and_then(JsFunction::from_object)?;
        let cursor = ObjectInitializer::new(engine)
            .property(JsString::from("row"), JsValue::from(ctx.cursor.0 as f64), Attribute::all())
            .property(JsString::from("col"), JsValue::from(ctx.cursor.1 as f64), Attribute::all())
            .build();
        let ctx_obj = ObjectInitializer::new(engine)
            .property(
                JsString::from("path"),
                match &ctx.path {
                    Some(p) => JsValue::from(JsString::from(p.clone())),
                    None => JsValue::null(),
                },
                Attribute::all(),
            )
            .property(JsString::from("mode"), JsValue::from(JsString::from(ctx.mode.clone())), Attribute::all())
            .property(JsString::from("cursor"), JsValue::from(cursor), Attribute::all())
            .build();
        let undefined = JsValue::undefined();
        let value: JsValue = func.call(&undefined, &[JsValue::from(ctx_obj)], engine).ok()?;
        value.try_js_into::<String>(engine).ok()
    })
}

/// 调 bufferline 图标钩子；未注册 / 返回 null / 报错 → None。
pub fn bufferline_icon(path: Option<&str>) -> Option<String> {
    init();
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized");
        let hook = with_buffer_icon_hook(|h| h.clone());
        let hook = hook?;
        let func = hook.as_callable().and_then(JsFunction::from_object)?;
        let arg = match path { Some(p) => JsValue::from(JsString::from(p)), None => JsValue::null() };
        let undefined = JsValue::undefined();
        let value: JsValue = func.call(&undefined, &[arg], engine).ok()?;
        value.try_js_into::<String>(engine).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // 多个测试共享全局运行时，用锁串行化避免消息队列竞争
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// v13 任务简报验证测试：read_dir 排序/is_dir、open_file、move_panel 入队 + 校验。
    /// 简报原文断言 count:2，但设置创建 3 个条目（a.txt、b.js、sub/）→ 按实际调整为 count:3；
    /// 排序断言 entries[0].name < entries[1].name 不受影响（a.txt < b.js）。
    #[test]
    fn sidecar_apis() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = std::env::temp_dir().join(format!("helix-js-sidecar-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("b.js"), "b").unwrap();

        // 路径经 {:?}（JSON 字符串转义）注入脚本
        let dir_str = dir.to_string_lossy();
        let file_path = dir.join("a.txt");
        let file_str = file_path.to_string_lossy();
        let script = format!(
            r#"
        helix.register_command("sc", () => {{
            const entries = helix.read_dir({dir:?});
            helix.echo("count:" + entries.length + " sorted:" + (entries[0].name < entries[1].name));
            const dirs = entries.filter(e => e.is_dir);
            helix.echo("dirs:" + dirs.length + ":" + dirs[0].name);
            helix.open_file({file:?});
            helix.move_panel(7, "left");
        }});
        "#,
            dir = dir_str,
            file = file_str,
        );
        load_script(&script).unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("sc", &ctx).unwrap());
        let msgs = take_messages();
        assert!(msgs[0].starts_with("count:3 sorted:true"), "{msgs:?}");
        assert_eq!(msgs[1], "dirs:1:sub");
        let reqs = take_ui_requests();
        assert!(matches!(&reqs[0], UiRequest::OpenFile { path } if path.ends_with("a.txt")));
        assert!(matches!(&reqs[1], UiRequest::MovePanel { id: 7, side } if side == "left"));

        // 校验：read_dir 不存在路径 → 抛错；move_panel 非法 side → 抛错；open_file 非字符串 → 抛错
        load_script(
            r#"
        helix.register_command("bad1", () => { helix.read_dir("/nonexistent-helix-js-xyz"); });
        helix.register_command("bad2", () => { helix.move_panel(7, "top"); });
        helix.register_command("bad3", () => { helix.open_file(42); });
        "#,
        )
        .unwrap();
        assert!(run_command("bad1", &ctx).is_err());
        assert!(run_command("bad2", &ctx).is_err());
        assert!(run_command("bad3", &ctx).is_err());
    }

    #[test]
    fn theme_overrides_api() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 清残留脏位（本测试是唯一 set_theme 的测试，防御顺序依赖）
        let _ = take_theme_dirty();
        assert!(theme_overrides().is_empty());

        // set_theme：整体替换 + 置脏；非字符串值忽略
        load_script(
            r##"
        helix.set_theme({
            "ui.popup": "#ff00aa",
            "error": "red",
            "ui.window": { fg: "#112233" },
        });
        "##,
        )
        .unwrap();
        assert!(take_theme_dirty());
        let ov = theme_overrides();
        assert_eq!(ov.get("ui.popup").map(String::as_str), Some("#ff00aa"));
        assert_eq!(ov.get("error").map(String::as_str), Some("red"));
        assert!(!ov.contains_key("ui.window"), "非字符串值应被忽略");

        // 再次 set_theme：替换而非累积
        load_script(r#"helix.set_theme({ "error": "blue" });"#).unwrap();
        assert!(take_theme_dirty());
        let ov = theme_overrides();
        assert_eq!(ov.len(), 1);
        assert_eq!(ov.get("error").map(String::as_str), Some("blue"));

        // 空对象 → 清空覆盖（等价的 reset）
        load_script(r#"helix.set_theme({});"#).unwrap();
        assert!(theme_overrides().is_empty());

        // reset_theme：清空 + 置脏
        load_script(r#"helix.set_theme({ "error": "red" });"#).unwrap();
        assert!(take_theme_dirty());
        load_script(r#"helix.reset_theme();"#).unwrap();
        assert!(take_theme_dirty());
        assert!(theme_overrides().is_empty());

        // 非法参数（非对象/缺参）→ JS 报错
        assert!(load_script(r#"helix.set_theme("red");"#).is_err());
        assert!(load_script(r#"helix.set_theme();"#).is_err());
    }

    #[test]
    fn echo_captures_message() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(r#"helix.echo("hello from js");"#).unwrap();
        assert_eq!(take_messages(), vec!["hello from js"]);
    }

    #[test]
    fn loaded_scripts_list() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script_named("a.js", r#"helix.register_command("a", () => {});"#).unwrap();
        load_script_named("b.js", r#"helix.register_command("b", () => {});"#).unwrap();
        let names = loaded_scripts();
        assert!(names.iter().any(|n| n == "a.js"));
        assert!(names.iter().any(|n| n == "b.js"));
    }

    #[test]
    fn register_and_run_command() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("where", (ctx) => {
            helix.echo("cursor: " + ctx.cursor.row + "," + ctx.cursor.col);
        });
        "#,
        )
        .unwrap();

        let ctx = CommandContext {
            path: Some("/tmp/demo.rs".to_string()),
            text: "hello\nworld".to_string(),
            cursor: (1, 2),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("where", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["cursor: 1,2"]);

        // 未注册的命令返回 false
        assert!(!run_command("nope", &ctx).unwrap());
        // 非法命令名（含空白）注册时报错
        assert!(load_script(r#"helix.register_command("bad name", () => {});"#).is_err());
    }

    #[test]
    fn panel_api() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        const pid = helix.open_panel({ side: "right", size: 30, render: () => ["p1", "p2"] });
        helix.echo("id:" + pid);
        helix.close_panel(pid);
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        // 编译器建议：matches! 守卫未用 id 绑定 → id: _（简报原文绑了 id，clippy 要求 0 告警）
        assert!(matches!(&reqs[0], UiRequest::OpenPanel { id: _, side, size } if side == "right" && *size == 30));
        assert!(matches!(&reqs[1], UiRequest::ClosePanel { id: _ }));
        assert!(take_messages()[0].starts_with("id:"));
        // 校验：side 白名单 / size / render
        assert!(load_script(r#"helix.open_panel({ side: "top", size: 10, render: () => [] });"#).is_err());
        assert!(load_script(r#"helix.open_panel({ side: "right", size: 10 });"#).is_err()); // 缺 render
        assert!(load_script(r#"helix.open_panel({ side: "right", size: "big", render: () => [] });"#).is_err());
    }

    #[test]
    fn panel_onkey() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        const pid = helix.open_panel({
            side: "right", size: 20,
            render: () => ["p"],
            onKey: (key) => { helix.echo("panel-key:" + key.name); return key.name === "Esc" ? "close" : "handled"; },
        });
        helix.echo("pid:" + pid);
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        // 编译器建议：matches! 未用 id 绑定 → id: _（简报原文绑了 id，clippy 要求 0 告警）
        assert!(matches!(&reqs[0], UiRequest::OpenPanel { id: _, .. }));
        // 编译器要求：单臂 match 非穷尽 → 改 let-else（与 popup_lifecycle 同款）
        let UiRequest::OpenPanel { id, .. } = reqs[0] else { unreachable!("expected OpenPanel") };
        assert!(take_messages()[0].starts_with("pid:"));
        // popup_key 走同一注册表：Esc → close，其他 → handled
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        let esc = PluginKey { name: "Esc".into(), shift: false, ctrl: false, alt: false };
        assert_eq!(popup_key(id, &esc, &ctx).unwrap(), PopupKeyResult::Close);
        assert_eq!(take_messages(), vec!["panel-key:Esc"]);
        // panel_has_onkey：有 onKey → true（简报测试的补充断言）
        assert!(panel_has_onkey(id));
        // 无 onKey 的面板：false（helix-term 侧据此全 Ignore 穿透，不调 popup_key）
        load_script(r#"helix.open_panel({ side: "left", size: 10, render: () => ["x"] });"#).unwrap();
        let UiRequest::OpenPanel { id: id2, .. } = take_ui_requests()[0] else { unreachable!("expected OpenPanel") };
        assert!(!panel_has_onkey(id2));
    }

    #[test]
    fn popup_lifecycle() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        let rendered = null;
        helix.open_popup({
            render: () => ["a", "b", "c"],
            onKey: (key) => key.name === "Down" ? "handled" : "close",
            onClose: () => helix.echo("closed:" + rendered),
        });
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 1);
        let UiRequest::OpenPopup { id, .. } = reqs[0] else { unreachable!("expected OpenPopup") };
        assert_eq!(id, 1); // 自增从 1 开始

        let lines = render_popup(id, 40, 10).unwrap();
        assert_eq!(
            lines,
            vec![
                StyledLine { text: "a".into(), style: None },
                StyledLine { text: "b".into(), style: None },
                StyledLine { text: "c".into(), style: None },
            ]
        );

        let key = PluginKey { name: "Down".into(), shift: false, ctrl: false, alt: false };
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Handled);
        let key = PluginKey { name: "Esc".into(), shift: false, ctrl: false, alt: false };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Close);

        close_popup(id).unwrap();
        assert_eq!(take_messages(), vec!["closed:null"]);
        assert!(render_popup(id, 40, 10).is_err()); // 已关闭，注册表移除
    }

    #[test]
    fn popup_default_keys_and_validation() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 未提供 onKey：Esc 默认关闭，其他穿透
        load_script(r#"helix.open_popup({ render: () => ["x"] });"#).unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        let key = PluginKey { name: "Enter".into(), shift: false, ctrl: false, alt: false };
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Ignored);
        let key = PluginKey { name: "Esc".into(), shift: false, ctrl: false, alt: false };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Close);
        close_popup(id).unwrap();

        // render 非数组 → Err
        load_script(r#"helix.open_popup({ render: () => "not an array" });"#).unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        assert!(render_popup(id, 40, 10).is_err());
        close_popup(id).unwrap();

        // 参数缺失/类型错误 → JS 报错
        assert!(load_script(r#"helix.open_popup({});"#).is_err());
        assert!(load_script(r#"helix.open_popup({ render: 42 });"#).is_err());
    }

    #[test]
    fn popup_size_position() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.open_popup({ render: () => ["x"], width: 40, height: 10, position: { row: 3, col: 4 } });
        helix.open_popup({ render: () => ["y"] });
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 2);
        match &reqs[0] {
            UiRequest::OpenPopup { width, height, position, .. } => {
                assert_eq!(*width, Some(40));
                assert_eq!(*height, Some(10));
                assert_eq!(*position, Some((3, 4)));
            }
            other => panic!("expected OpenPopup, got {other:?}"),
        }
        match &reqs[1] {
            UiRequest::OpenPopup { width, height, position, .. } => {
                assert_eq!(*width, None);
                assert_eq!(*height, None);
                assert_eq!(*position, None);
            }
            other => panic!("expected OpenPopup, got {other:?}"),
        }
        // 非法类型
        assert!(load_script(r#"helix.open_popup({ render: () => [], width: "big" });"#).is_err());
    }

    #[test]
    fn popup_styled_lines() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.open_popup({
            render: () => [
                { text: "err: ", style: "error" },
                "plain",
                { text: "warn" },
            ],
        });
        "#,
        )
        .unwrap();
        let id = match take_ui_requests()[0] { UiRequest::OpenPopup { id, .. } => id, _ => unreachable!("expected OpenPopup") };
        let lines = render_popup(id, 40, 10).unwrap();
        assert_eq!(
            lines,
            vec![
                StyledLine { text: "err: ".into(), style: Some("error".into()) },
                StyledLine { text: "plain".into(), style: None },
                StyledLine { text: "warn".into(), style: None },
            ]
        );
        close_popup(id).unwrap();
        // 非法元素（缺 text / 非字符串非对象）→ Err
        load_script(r#"helix.open_popup({ render: () => [{ style: "error" }] });"#).unwrap();
        let id = match take_ui_requests()[0] { UiRequest::OpenPopup { id, .. } => id, _ => unreachable!("expected OpenPopup") };
        assert!(render_popup(id, 40, 10).is_err());
        close_popup(id).unwrap();
    }

    #[test]
    fn popup_onkey_edits_doc() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.open_popup({
            render: () => ["a", "b"],
            onKey: (key, doc) => {
                if (key.name === "Enter") {
                    doc.insert(doc.cursor.row, doc.cursor.col, "XYZ");
                    return "close";
                }
                return "handled";
            },
        });
        "#,
        )
        .unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        let ctx = CommandContext {
            path: Some("/tmp/p.rs".into()),
            text: "line1\nline2".into(),
            cursor: (1, 2),
            selection: ((0, 0), (0, 0)),
        };
        let key = PluginKey { name: "Enter".into(), shift: false, ctrl: false, alt: false };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Close);
        assert_eq!(
            take_edits(),
            vec![Edit { start: (1, 2), end: (1, 2), insert: "XYZ".into() }]
        );
        close_popup(id).unwrap();
    }

    #[test]
    fn buffer_icon_hook() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        assert_eq!(bufferline_icon(Some("a.rs")), None); // 未注册 → None

        load_script(
            r#"
        helix.set_buffer_icon((path) => path && path.endsWith(".rs") ? "🦀" : null);
        "#,
        )
        .unwrap();
        assert_eq!(bufferline_icon(Some("main.rs")), Some("🦀".to_string()));
        assert_eq!(bufferline_icon(Some("main.py")), None);
        assert_eq!(bufferline_icon(None), None);
    }

    #[test]
    fn doc_edits_queue() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("edit", (ctx) => {
            ctx.doc.insert(1, 2, "ab");
            ctx.doc.replace(0, 0, 0, 5, "new");
            ctx.doc.delete(3, 0, 4, 0);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (1, 2), selection: ((0, 0), (0, 0)) };
        run_command("edit", &ctx).unwrap();
        let edits = take_edits();
        assert_eq!(
            edits,
            vec![
                Edit { start: (1, 2), end: (1, 2), insert: "ab".into() },
                Edit { start: (0, 0), end: (0, 5), insert: "new".into() },
                Edit { start: (3, 0), end: (4, 0), insert: String::new() },
            ]
        );
        // take_edits 清空
        assert!(take_edits().is_empty());
    }

    #[test]
    fn doc_edit_validation() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 类型错误 → 命令失败（run_command 返回 Err），且队列被清空
        load_script(
            r#"
        helix.register_command("bad1", (ctx) => { ctx.doc.insert("x", 0, "a"); });
        helix.register_command("bad2", (ctx) => { ctx.doc.replace(0, 0, 0, 0, 42); });
        helix.register_command("bad3", (ctx) => { ctx.doc.delete(0, 0, 0); });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("bad1", &ctx).is_err());
        assert!(take_edits().is_empty());
        assert!(run_command("bad2", &ctx).is_err());
        assert!(run_command("bad3", &ctx).is_err());
        assert!(take_edits().is_empty());
        // 正常命令运行后队列仍有值（供 helix-term 消费）
        load_script(r#"helix.register_command("ok", (ctx) => { ctx.doc.insert(0, 0, "z"); });"#).unwrap();
        run_command("ok", &ctx).unwrap();
        assert_eq!(take_edits().len(), 1);
    }

    #[test]
    fn event_handlers() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        assert!(!has_handlers("save"));

        load_script(
            r#"
        let order = [];
        helix.on("save", (doc) => { order.push("a"); });
        helix.on("save", (doc) => { order.push("b"); });
        helix.on("mode-change", (mode, doc) => { helix.echo("mode:" + mode); });
        "#,
        )
        .unwrap();

        assert!(has_handlers("save"));
        assert!(has_handlers("mode-change"));
        assert!(!has_handlers("buffer-open"));

        // emit 带编辑队列清空 + 多处理器按注册顺序
        let ctx = CommandContext { path: Some("/tmp/e.rs".into()), text: "x".into(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        emit_event("save", &ctx, None).unwrap();
        assert!(take_edits().is_empty());

        // 事件名白名单校验 + 回调类型校验
        assert!(load_script(r#"helix.on("bogus", () => {});"#).is_err());
        assert!(load_script(r#"helix.on("save", 42);"#).is_err());

        // mode-change 处理器带 mode 参数 + echo
        emit_event("mode-change", &ctx, Some("insert")).unwrap();
        assert_eq!(take_messages(), vec!["mode:insert"]);
    }

    #[test]
    fn doc_change_event() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.on("doc-change", (doc) => { helix.echo("changed:" + doc.cursor.row); });
        "#,
        )
        .unwrap();
        assert!(has_handlers("doc-change"));
        let ctx = CommandContext { path: None, text: "x".into(), cursor: (2, 0), selection: ((0, 0), (0, 0)) };
        emit_event("doc-change", &ctx, None).unwrap();
        assert_eq!(take_messages(), vec!["changed:2"]);
        // 未注册的事件名仍然报错
        assert!(load_script(r#"helix.on("bogus", () => {});"#).is_err());
    }

    #[test]
    fn keymap_registration() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();

        // 字符串命令 → MapKey 入队
        load_script(r#"helix.map("normal", "gd", "goto-def");"#).unwrap();
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 1);
        match &reqs[0] {
            UiRequest::MapKey { mode, key, command } => {
                assert_eq!(mode, "normal");
                assert_eq!(key, "gd");
                assert_eq!(command, "goto-def");
            }
            other => panic!("expected MapKey, got {other:?}"),
        }

        // 回调 → 注册 __mapped_N + MapKey 入队
        load_script(
            r#"
        helix.map("insert", "C-n", () => { helix.echo("cb"); });
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 1);
        let command = match &reqs[0] {
            UiRequest::MapKey { command, .. } => command.clone(),
            other => panic!("expected MapKey, got {other:?}"),
        };
        assert!(command.starts_with("__mapped_"), "command: {command}");
        // 注册的命令可以运行（与普通插件命令同机制）
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command(&command, &ctx).unwrap());
        assert_eq!(take_messages(), vec!["cb"]);

        // 校验失败
        assert!(load_script(r#"helix.map("bogus", "x", "y");"#).is_err());
        assert!(load_script(r#"helix.map("normal", 42, "y");"#).is_err());
        assert!(load_script(r#"helix.map("normal", "x", 42);"#).is_err());
        assert!(load_script(r#"helix.map("normal", "", "y");"#).is_err());
    }

    #[test]
    fn statusline_hook() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        assert_eq!(statusline_text(&StatuslineCtx { path: None, mode: "normal".into(), cursor: (0, 0) }), None);

        load_script(
            r#"
        helix.set_statusline((ctx) => ctx.mode + ":" + ctx.cursor.row);
        "#,
        )
        .unwrap();
        let ctx = StatuslineCtx { path: Some("/tmp/a.rs".into()), mode: "insert".into(), cursor: (3, 7) };
        assert_eq!(statusline_text(&ctx), Some("insert:3".to_string()));

        // 返回 null → None
        load_script(r#"helix.set_statusline(() => null);"#).unwrap();
        assert_eq!(statusline_text(&ctx), None);
        // 抛错 → None
        load_script(r#"helix.set_statusline(() => { throw new Error("boom"); });"#).unwrap();
        assert_eq!(statusline_text(&ctx), None);
        // set_statusline(null) 清除
        load_script(r#"helix.set_statusline(null);"#).unwrap();
        assert_eq!(statusline_text(&ctx), None);
        // 非法参数 → JS 报错
        assert!(load_script(r#"helix.set_statusline(42);"#).is_err());
    }

    #[test]
    fn command_docs() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("doc1", () => {}, "first doc");
        helix.register_command("nodoc", () => {});
        helix.register_command("nulldoc", () => {}, undefined);
        "#,
        )
        .unwrap();
        assert_eq!(command_doc("doc1"), Some("first doc".to_string()));
        assert_eq!(command_doc("nodoc"), None);
        assert_eq!(command_doc("nulldoc"), None);
        assert_eq!(command_doc("missing"), None);
        // 非法 doc 类型 → 报错
        assert!(load_script(r#"helix.register_command("bad", () => {}, 42);"#).is_err());
    }

    #[test]
    fn plugin_reload() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script_named("a.js", r#"helix.register_command("reload-cmd", () => { helix.echo("v1"); });"#).unwrap();
        load_script_named("b.js", r#"helix.on("save", () => {});"#).unwrap();

        assert!(has_handlers("save"));
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("reload-cmd", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["v1"]);

        // reload：清空状态后重跑全部已记录脚本
        reload_all().unwrap();
        assert!(has_handlers("save"), "handlers re-registered after reload");
        assert!(run_command("reload-cmd", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["v1"]);
    }

    #[test]
    fn reload_closes_open_panel_layer() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 面板由命令打开（不在加载时），保证 reload 重跑脚本不会自动重开面板
        load_script(
            r#"helix.register_command("open-panel", () => { helix.open_panel({ side: "right", size: 30, render: () => ["p1"] }); });"#,
        )
        .unwrap();
        let ctx = CommandContext {
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("open-panel", &ctx).unwrap());
        assert!(matches!(take_ui_requests()[0], UiRequest::OpenPanel { .. }));

        // reload：面板层还挂在 compositor 上 → 必须入队 ClosePanel 供 apply_ui_requests 移除
        reload_all().unwrap();
        let reqs = take_ui_requests();
        assert!(matches!(&reqs[0], UiRequest::ClosePanel { .. }), "layer removed after reload");
        // LAST_PANEL_ID 已清空 → :panel-close 不再误报有面板
        assert!(close_last_panel().is_err());
    }

    #[test]
    fn selection_and_cursor() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("selcmd", (ctx) => {
            helix.set_cursor(2, 3);
            helix.set_selection(0, 1, 0, 5);
            helix.echo("sel:" + ctx.selection.anchor.row + "," + ctx.selection.anchor.col + "-" + ctx.selection.head.row + "," + ctx.selection.head.col);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            path: None,
            text: "abc\ndef\nghi".into(),
            cursor: (0, 0),
            selection: ((0, 1), (0, 5)),
        };
        assert!(run_command("selcmd", &ctx).unwrap());
        let reqs = take_cursor_requests();
        assert_eq!(
            reqs,
            vec![
                CursorRequest::SetCursor { row: 2, col: 3 },
                CursorRequest::SetSelection { anchor: (0, 1), head: (0, 5) },
            ]
        );
        assert_eq!(take_messages(), vec!["sel:0,1-0,5"]);

        // 类型错误 → 命令失败（run_command 返回 Err），请求队列被清空
        load_script(r#"helix.register_command("badsel", (ctx) => { helix.set_cursor("x", 0); });"#).unwrap();
        assert!(run_command("badsel", &ctx).is_err());
        assert!(take_cursor_requests().is_empty());
    }

    #[test]
    fn shell_run() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 成功路径
        load_script(r#"helix.register_command("r1", () => { helix.echo(helix.run("echo hi")); });"#).unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("r1", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["hi\n"]);

        // 非零退出码 → 错误
        load_script(r#"helix.register_command("r2", () => { helix.run("echo boom >&2; exit 3"); });"#).unwrap();
        let err = run_command("r2", &ctx).unwrap_err().to_string();
        assert!(err.contains("command failed"), "err: {err}");
        assert!(err.contains("boom"), "stderr should be included: {err}");

        // 类型错误
        load_script(r#"helix.register_command("r3", () => { helix.run(42); });"#).unwrap();
        assert!(run_command("r3", &ctx).is_err());

        // 截断：输出超限 → 返回长度 ≤ 65536
        load_script(
            r#"
        helix.register_command("r4", () => {
            const out = helix.run("head -c 100000 /dev/zero | tr '\\0' 'x'");
            helix.echo("len:" + out.length + " tail:" + out.slice(-11));
        });
        "#,
        )
        .unwrap();
        assert!(run_command("r4", &ctx).unwrap());
        let msg = take_messages();
        assert!(msg[0].contains("tail:(truncated)"), "marker expected: {:?}", msg[0]);
        let len: usize = msg[0].strip_prefix("len:").unwrap().split(" tail:").next().unwrap().parse().unwrap();
        assert!(len <= 65536 + "(truncated)".len(), "truncated output, len={len}");
    }

    /// 轮询 drain_term_events 直到谓词命中或超时（async 测试需要）。
    /// 累积自调用以来的全部事件返回：进程事件是 Chunk(s)→Exit 的顺序流，
    /// 命中谓词的那次 drain 之前可能已有事件被取走，须一并保留按序 resolve。
    fn wait_for_term_event(pred: impl Fn(&TermEvent) -> bool) -> Vec<TermEvent> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut all = Vec::new();
        loop {
            all.extend(drain_term_events());
            if all.iter().any(&pred) {
                return all;
            }
            if std::time::Instant::now() > deadline {
                panic!("timed out waiting for term event");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// 轮询 drain_async_events 直到谓词命中或超时，事件累积进调用方传入的 vec
    /// （跨调用共享累积：异步 fs 四个操作并发发送，后几次 wait 必须能看到先前已 drain 的事件）。
    fn wait_for_async(all: &mut Vec<AsyncEvent>, pred: impl Fn(&AsyncEvent) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            all.extend(drain_async_events());
            if all.iter().any(&pred) {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!("timed out waiting for async event");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn async_run_and_spawn() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();

        // run_async：echo → Exit 事件携带 stdout → resolve 触发回调
        load_script(
            r#"
        helix.run_async("echo async-hello", (err, out) => {
            helix.echo("cb:" + (err ?? "ok") + ":" + (out ?? "").trim());
        });
        "#,
        )
        .unwrap();
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        let TermEvent::Exit(id, code, stdout) = &events[0] else { unreachable!() };
        assert_eq!(*code, 0);
        let stdout = stdout.clone().unwrap_or_default();
        resolve_term_event(*id, TermEvent::Exit(*id, *code, Some(stdout))).unwrap();
        assert_eq!(take_messages(), vec!["cb:ok:async-hello"]);

        // spawn 流式：cat 回显
        load_script(
            r#"
        helix.register_command("sp", () => {
            const id = helix.spawn({ cmd: "cat", onChunk: (c) => helix.echo("chunk:" + c), onExit: (code) => helix.echo("exit:" + code) });
            helix.term_write(id, "hello-term\n");
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("sp", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Chunk(_, c) if c.contains("hello-term")));
        let ev = events.iter().find(|e| matches!(e, TermEvent::Chunk(_, c) if c.contains("hello-term"))).expect("chunk event");
        let TermEvent::Chunk(id, chunk) = ev else { unreachable!() };
        resolve_term_event(*id, TermEvent::Chunk(*id, chunk.clone())).unwrap();
        assert!(take_messages().contains(&format!("chunk:{chunk}")));

        // term_kill：sleep 100 → kill → Exit 快到达
        load_script(
            r#"
        helix.register_command("kp", () => {
            const id = helix.spawn({ cmd: "sleep 100", onChunk: () => {}, onExit: (code) => helix.echo("killed:" + code) });
            helix.term_kill(id);
        });
        "#,
        )
        .unwrap();
        assert!(run_command("kp", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        let TermEvent::Exit(id, code, _) = &events[0] else { unreachable!() };
        assert!(*code != 0, "killed process should have non-zero exit");
        resolve_term_event(*id, TermEvent::Exit(*id, *code, None)).unwrap();
        assert!(take_messages()[0].starts_with("killed:"));

        // 类型校验：run_async/spawn 参数错误在 load 时即报错
        assert!(load_script(r#"helix.run_async(42, () => {});"#).is_err());
        assert!(load_script(r#"helix.run_async("x", 42);"#).is_err());
        assert!(load_script(r#"helix.spawn({ cmd: "x" });"#).is_err()); // 缺 onChunk
        // term_write 未知 id 在 load 时不会执行（命令体），须放进命令里跑
        load_script(r#"helix.register_command("badid", () => { helix.term_write(999, "x"); });"#).unwrap();
        assert!(run_command("badid", &ctx).is_err());
    }

    #[test]
    fn async_utf8_across_chunks() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();

        // run_async 聚合：19999 字节 CJK 输出（"中文\n"×2857，7 字节/行）跨多个 4096 块，
        // 每块边界都可能切开 3 字节字符——修复前逐块 from_utf8_lossy 会产出 U+FFFD。
        // head -c 19999 恰好截在行边界（19999 = 7×2857），整流是合法 UTF-8。
        load_script(
            r#"
        helix.run_async("yes 中文 | head -c 19999", (err, out) => {
            helix.echo("agg:" + (err === null) + ":" + out.length);
        });
        "#,
        )
        .unwrap();
        // 只等聚合模式的 Exit（携带 Some(stdout)），遗漏的遗留 Exit 一并 resolve 清理
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, Some(_))));
        let ev = events
            .iter()
            .find(|e| matches!(e, TermEvent::Exit(_, _, Some(_))))
            .expect("run_async exit");
        let TermEvent::Exit(_, code, stdout) = ev else { unreachable!() };
        assert_eq!(*code, 0);
        let out = stdout.clone().unwrap_or_default();
        assert_eq!(out.len(), 19999, "aggregated bytes intact across chunks");
        assert!(!out.contains('\u{FFFD}'), "no replacement chars in aggregated output");
        assert_eq!(out.matches("中文").count(), 2857, "CJK lines preserved (7 bytes/line)");
        for ev in &events {
            if let TermEvent::Exit(id, code, stdout) = ev {
                resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap();
            }
        }
        let msg = take_messages();
        let m = msg.iter().find(|m| m.starts_with("agg:")).expect("run_async echo");
        // JS 收到完整输出：19999 字节 = 2857 行 × 3 个 BMP 字符 = 8571 个 UTF-16 单元
        assert_eq!(m.as_str(), "agg:true:8571");

        // spawn 流式：同一输出经 onChunk 增量解码拼接，块边界不得产生 U+FFFD
        load_script(
            r#"
        helix.register_command("spcjk", () => {
            const parts = [];
            const id = helix.spawn({
                cmd: "yes 中文 | head -c 19999",
                onChunk: (c) => parts.push(c),
                onExit: (code) => helix.echo("spawn:" + code + ":" + parts.length + ":" + parts.join("")),
            });
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("spcjk", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        // 按序 resolve：先 Chunk 后 Exit，JS 的 parts 才能拼全
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, chunk) => {
                    resolve_term_event(*id, TermEvent::Chunk(*id, chunk.clone())).unwrap();
                }
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap();
                }
            }
        }
        let msg = take_messages();
        let m = msg.iter().find(|m| m.starts_with("spawn:0:")).expect("spawn echo");
        let mut it = m.splitn(4, ':');
        assert_eq!(it.next(), Some("spawn"));
        assert_eq!(it.next(), Some("0"));
        let chunks: usize = it.next().unwrap().parse().expect("chunk count");
        assert!(chunks >= 4, "streaming should split into multiple chunks, got {chunks}");
        let joined = it.next().unwrap();
        assert_eq!(joined.len(), 19999, "streamed bytes intact across chunks");
        assert!(!joined.contains('\u{FFFD}'), "no replacement chars in streamed output");
        assert_eq!(joined.matches("中文").count(), 2857, "CJK lines preserved in streamed output");
    }

    /// 任务简报验证测试：四个异步 fs API（read/write/stat/glob）回调 → echo；
    /// 错误路径 err 非空；参数类型校验。
    /// 注（相对简报的测试侧调整）：wait_for_async 把事件累积进共享 vec（四个 worker
    /// 并发发送，四次顺序 wait 需共享累积）；简报注释 "resolve 全部" 落实为逐事件 resolve。
    #[test]
    fn async_fs() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = std::env::temp_dir().join(format!("helix-js-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello fs").unwrap();
        std::fs::write(dir.join("b.js"), "x").unwrap();

        load_script(&format!(r#"
        helix.register_command("fsd", () => {{
            helix.read_file_async("{dir}/a.txt", (err, content) => {{
                helix.echo("read:" + (err ?? "") + ":" + (content ?? ""));
            }});
            helix.write_file_async("{dir}/out.txt", "written", (err) => {{
                helix.echo("write:" + (err ?? "ok"));
            }});
            helix.stat_async("{dir}/a.txt", (err, st) => {{
                helix.echo("stat:" + st.size + ":" + st.is_dir);
            }});
            helix.glob_async("{dir}/*.js", (err, paths) => {{
                helix.echo("glob:" + paths.length);
            }});
        }});
    "#, dir = dir.display())).unwrap();

        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("fsd", &ctx).unwrap());
        // 轮询 drain_async_events 直到四个回调都到（wait_for_async 辅助，仿 wait_for_term_event）
        let mut events = Vec::new();
        wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsRead(_, _)));
        wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsWrite(_, _)));
        wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsStat(_, _)));
        wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsGlob(_, _)));
        // resolve 全部（事件里带 id）→ 断言回调 echo
        for ev in events {
            let id = match &ev {
                AsyncEvent::FsRead(id, _) | AsyncEvent::FsWrite(id, _) | AsyncEvent::FsStat(id, _) | AsyncEvent::FsGlob(id, _) => *id,
            };
            resolve_async_event(id, ev).unwrap();
        }
        let msgs = take_messages();
        assert!(msgs.iter().any(|m| m == "read::hello fs"), "{msgs:?}");
        assert!(msgs.iter().any(|m| m == "write:ok"), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.starts_with("stat:8:false")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m == "glob:1"), "{msgs:?}");
        assert_eq!(std::fs::read_to_string(dir.join("out.txt")).unwrap(), "written");

        // review: ** 中缀（`dir/**/*.js`）——嵌套目录也要命中（独立回合：wait_for_async 为
        // any 语义，同一事件类型不能连续 wait 两次）
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/deep.js"), "d").unwrap();
        load_script(&format!(r#"
        helix.register_command("fsd2", () => {{
            helix.glob_async("{dir}/**/*.js", (err, paths) => {{
                helix.echo("glob2:" + (err ?? "") + ":" + paths.length);
            }});
        }});
    "#, dir = dir.display())).unwrap();
        assert!(run_command("fsd2", &ctx).unwrap());
        let mut g2 = Vec::new();
        wait_for_async(&mut g2, |e| matches!(e, AsyncEvent::FsGlob(_, _)));
        for ev in g2 {
            let id = match &ev {
                AsyncEvent::FsRead(id, _) | AsyncEvent::FsWrite(id, _) | AsyncEvent::FsStat(id, _) | AsyncEvent::FsGlob(id, _) => *id,
            };
            resolve_async_event(id, ev).unwrap();
        }
        let msgs2 = take_messages();
        assert!(msgs2.iter().any(|m| m == "glob2::2"), "** 应命中根目录+嵌套: {msgs2:?}");

        // 错误路径：读不存在 → err 非空
        load_script(&format!(r#"
        helix.register_command("fsbad", () => {{
            helix.read_file_async("{dir}/nope.txt", (err, content) => {{
                helix.echo("bad:" + (err !== null ? "err" : "noerr"));
            }});
        }});
    "#, dir = dir.display())).unwrap();
        assert!(run_command("fsbad", &ctx).unwrap());
        let mut bad = Vec::new();
        wait_for_async(&mut bad, |e| matches!(e, AsyncEvent::FsRead(_, _)));
        // resolve → 断言
        for ev in bad {
            let id = match &ev {
                AsyncEvent::FsRead(id, _) | AsyncEvent::FsWrite(id, _) | AsyncEvent::FsStat(id, _) | AsyncEvent::FsGlob(id, _) => *id,
            };
            resolve_async_event(id, ev).unwrap();
        }
        assert!(take_messages().iter().any(|m| m == "bad:err"));

        // 类型校验
        assert!(load_script(r#"helix.read_file_async(42, () => {});"#).is_err());
        assert!(load_script(r#"helix.read_file_async("x", 42);"#).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// review 回归：裸模式 `*.js`（基目录 = `.`，cwd 内匹配）——验证字面前缀基目录 +
    /// 前导 `./` 归一化。chdir 受 TEST_LOCK 保护（同进程单测串行，本 crate 全部测试取锁）。
    #[test]
    fn glob_bare_pattern() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = std::env::temp_dir().join(format!("helix-js-glob-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("x.js"), "x").unwrap();
        std::fs::write(dir.join("y.txt"), "y").unwrap();
        std::fs::write(dir.join("sub/z.js"), "z").unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let bare = glob_matches("*.js");
        let dbl = glob_matches("**/*.js");
        let explicit = glob_matches("./*.js"); // ./ 归一化与裸模式一致
        std::env::set_current_dir(cwd).unwrap();
        assert_eq!(bare.unwrap(), vec!["x.js"], "裸 * 不跨目录分隔符");
        assert_eq!(dbl.unwrap(), vec!["sub/z.js", "x.js"], "** 跨目录（含零层）");
        assert_eq!(explicit.unwrap(), vec!["x.js"], "前导 ./ 归一化");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 临时任务简报的验证测试。wait 条件用 Exit（chunk 是它的先导），
    /// 返回的全部事件按序 resolve（先 Chunk 后 Exit），保证通道不残留。
    /// 注意：编译报错调整——简报原文 `events[0]` 按值取会 move，改为 `&events[0]`；
    /// `assert!(true, ...)` 触发 clippy::assertions_on_constants，删除。
    #[test]
    #[cfg(unix)]
    fn pty_spawn() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("pty-tty", () => {
            const id = helix.spawn({ pty: true, cmd: "tty", onChunk: (c) => helix.echo("out:" + c.trim()), onExit: (code) => helix.echo("exit:" + code) });
        });
        helix.register_command("pty-size", () => {
            const id = helix.spawn({ pty: true, cmd: "stty size", onChunk: (c) => helix.echo("size:" + c.trim()), onExit: (code) => helix.echo("sizeexit:" + code) });
        });
        helix.register_command("pty-cat", () => {
            const id = helix.spawn({ pty: true, cmd: "cat", onChunk: (c) => helix.echo("pty:" + c.trim()), onExit: (code) => helix.echo("ptyexit:" + code) });
            helix.term_write(id, "hello-pty\n\u{0004}");
        });
        helix.register_command("pty-badresize", () => { helix.term_resize(999, 1, 1); });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };

        // tty：stdin 是 pty → 输出 /dev/pts/N（CRLF 行尾，用 contains 断言）
        assert!(run_command("pty-tty", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, c) => resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap(),
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap()
                }
            }
        }
        assert!(
            take_messages().iter().any(|m| m.contains("/dev/pts/")),
            "tty command sees a pty"
        );

        // stty size：默认 winsize 24×80（输出顺序 rows cols）
        assert!(run_command("pty-size", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, c) => resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap(),
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap()
                }
            }
        }
        assert!(
            take_messages().iter().any(|m| m.contains("24 80")),
            "default winsize 24x80"
        );

        // 写 master → 子进程 stdin：cat 回显 + tty 驱动 echo → Ctrl-D(\u{0004}) EOF 退出
        assert!(run_command("pty-cat", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, c) => resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap(),
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap()
                }
            }
        }
        // take_messages 是消费型：先取一次再断言两条
        let msgs = take_messages();
        assert!(
            msgs.iter().any(|m| m.contains("hello-pty")),
            "write to master reaches child: {msgs:?}"
        );
        assert!(
            msgs.iter().any(|m| m.contains("ptyexit:0")),
            "Ctrl-D EOF exits cat cleanly: {msgs:?}"
        );

        // 校验：pty 非布尔 / resize 未知 id → 报错
        assert!(load_script(r#"helix.spawn({ pty: "yes", cmd: "tty", onChunk: () => {} });"#).is_err());
        assert!(run_command("pty-badresize", &ctx).is_err());
    }

    /// spawn 后立即 term_resize → 子进程 stty size 读到新值。
    /// 实现用 master fd 直连 ioctl（spawn 返回时已注册），无消息时序问题。
    #[test]
    #[cfg(unix)]
    fn pty_resize() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("pty-resize", () => {
            const id = helix.spawn({ pty: true, cmd: "stty size", onChunk: (c) => helix.echo("size:" + c.trim()), onExit: (code) => helix.echo("resizeexit:" + code) });
            helix.term_resize(id, 40, 100);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("pty-resize", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, c) => resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap(),
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap()
                }
            }
        }
        let msgs = take_messages();
        // stty size 输出顺序是 rows cols（简报写 "100 40"，实为行列反了）：
        // term_resize(40, 100) → "40 100"，以实际输出为准
        assert!(
            msgs.iter().any(|m| m.contains("40 100")),
            "resize applied before stty runs: {msgs:?}"
        );
    }

    #[test]
    fn plugin_reload_top_level_const() {
        // 顶层 const/let 脚本 reload 必须成功：IIFE 包裹下重跑获得全新词法作用域
        // （无 IIFE 时全局词法环境重复声明会抛 SyntaxError: duplicate lexical declaration）
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script_named(
            "c.js",
            r#"const X = 1; helix.register_command("ccmd", () => helix.echo("ok"));"#,
        )
        .unwrap();

        reload_all().unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("ccmd", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["ok"]);
    }

    /// 统一入口：load/export 往返 + 缓存、lazy 桩、run_command 带 ctx、未知文件报错。
    /// set_plugins_dir 是进程全局——测试用临时目录隔离。
    // ponytail: tempdir 不 drop（std::mem::forget）——线程池复用线程，后续 reload 测试
    // 会在本线程重跑 LOADED_SCRIPTS（含 init.js→load("exp.js")），目录被删会误伤；
    // 泄漏几个 /tmp 小文件换确定性。
    #[test]
    fn entry_load_export_lazy() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = tempfile::tempdir().unwrap();
        set_plugins_dir(dir.path().to_path_buf());

        // 导出 + 加载往返
        std::fs::write(dir.path().join("exp.js"), r#"helix.export({ a: 1, b: "x" });"#).unwrap();
        load_script_named("init.js", r#"helix.load("exp.js");"#).unwrap();
        // init.js 的 load 本身无法断言返回值——直接测 js_load 路径：
        // 用 helix.run_command 间接：注册命令调用 load 并把结果 echo 出来
        load_script_named(
            "driver.js",
            r#"
        helix.register_command("load-exp", () => {
            const mod = helix.load("exp.js");
            helix.echo("a:" + mod.a + " b:" + mod.b);
        });
        helix.register_command("load-cached", () => {
            const m1 = helix.load("exp.js");
            const m2 = helix.load("exp.js");
            helix.echo("same:" + (m1 === m2));
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("load-exp", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["a:1 b:x"]);
        assert!(run_command("load-cached", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["same:true"]);

        // lazy：桩首次调用时加载 + 转执行
        std::fs::write(
            dir.path().join("lazy.js"),
            r#"helix.register_command("lazy-cmd", () => { helix.echo("lazy-ran"); });"#,
        )
        .unwrap();
        load_script_named("lazy-driver.js", r#"helix.lazy("lazy.js", "lazy-cmd");"#).unwrap();
        assert!(run_command("lazy-cmd", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["lazy-ran"]);

        // run_command 带 ctx：命令读 ctx.cursor
        load_script_named(
            "rc.js",
            r#"
        helix.register_command("where", (c) => { helix.echo("at:" + c.cursor.row + "," + c.cursor.col); });
        "#,
        )
        .unwrap();
        load_script_named(
            "rc-driver.js",
            r#"helix.register_command("call-where", () => { helix.run_command("where", { cursor: { row: 3, col: 7 } }); });"#,
        )
        .unwrap();
        assert!(run_command("call-where", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["at:3,7"]);

        // 校验：未知文件 → 抛错
        load_script_named("bad-driver.js", r#"helix.register_command("bad-load", () => { helix.load("nope.js"); });"#).unwrap();
        assert!(run_command("bad-load", &ctx).is_err());

        std::mem::forget(dir);
    }
}
