//! JavaScript plugin runtime for the Helix editor (PoC).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Result};
use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsString, JsValue, NativeFunction, Source};

/// 插件向编辑器发起的 UI 请求（编辑器主线程取走后执行）
#[derive(Debug)]
pub enum UiRequest {
    OpenPopup { id: u64 },
    MapKey { mode: String, key: String, command: String },
}

/// 异步进程事件（worker 线程 → 主线程；主线程 drain 后 resolve 到 JS 回调）
#[derive(Debug)]
pub enum TermEvent {
    Chunk(u64, String),
    /// run_async 的 Exit 携带聚合后的完整 stdout（Option）；spawn 的 Exit 为 None
    Exit(u64, i32, Option<String>),
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
}

/// 访问 REGISTRY：首次触达惰性 Box::leak 创建；线程退出时内容泄漏（见上）
fn with_registry<T>(f: impl FnOnce(&mut HashMap<String, JsValue>) -> T) -> T {
    REGISTRY.with(|r| {
        let mut slot = r.borrow_mut();
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
}

/// 访问 TERM_CALLBACKS：同上（内容泄漏）
fn with_terms<T>(f: impl FnOnce(&mut HashMap<u64, TermCallbacks>) -> T) -> T {
    TERM_CALLBACKS.with(|t| {
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
    CONTEXT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let engine = Box::leak(Box::new(Context::default()));
            let helix = ObjectInitializer::new(engine)
                .function(NativeFunction::from_fn_ptr(js_echo), JsString::from("echo"), 1)
                .function(
                    NativeFunction::from_fn_ptr(js_register_command),
                    JsString::from("register_command"),
                    2,
                )
                .function(NativeFunction::from_fn_ptr(js_open_popup), JsString::from("open_popup"), 1)
                .function(NativeFunction::from_fn_ptr(js_set_buffer_icon), JsString::from("set_buffer_icon"), 1)
                .function(NativeFunction::from_fn_ptr(js_on), JsString::from("on"), 2)
                .function(NativeFunction::from_fn_ptr(js_map), JsString::from("map"), 3)
                .function(NativeFunction::from_fn_ptr(js_set_cursor), JsString::from("set_cursor"), 2)
                .function(NativeFunction::from_fn_ptr(js_set_selection), JsString::from("set_selection"), 4)
                .function(NativeFunction::from_fn_ptr(js_set_statusline), JsString::from("set_statusline"), 1)
                .function(NativeFunction::from_fn_ptr(js_run), JsString::from("run"), 1)
                .function(NativeFunction::from_fn_ptr(js_run_async), JsString::from("run_async"), 2)
                .function(NativeFunction::from_fn_ptr(js_spawn), JsString::from("spawn"), 1)
                .function(NativeFunction::from_fn_ptr(js_term_write), JsString::from("term_write"), 2)
                .function(NativeFunction::from_fn_ptr(js_term_kill), JsString::from("term_kill"), 1)
                .build();
            engine
                .register_global_property(JsString::from("helix"), helix, Attribute::READONLY | Attribute::NON_ENUMERABLE)
                .expect("register helix object");
            *slot = Some(engine);
        }
    });
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
    let id = NEXT_TERM_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
    with_terms(|m| {
        m.insert(id, TermCallbacks { on_chunk, on_exit, is_run_async: false })
    });
    let (tx, rx) = std::sync::mpsc::channel();
    TERM_WORKERS.with(|m| m.borrow_mut().insert(id, tx));
    spawn_worker(
        id,
        &cmd,
        false,
        TERM_EVENTS.with(|t| t.borrow().clone().expect("TERM_EVENTS initialized")),
        rx,
    );
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
            }
        }
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
    COMMAND_DOCS.with(|d| d.borrow_mut().clear());
}

/// 热重载：清空状态后按加载顺序重跑全部脚本。
/// 脚本以 IIFE 包裹求值，每次重跑都是全新词法作用域——顶层 const/let/var 都不冲突，
/// reload 必然成功（脚本不依赖其他脚本的状态，reload 会重新注册全部命令/处理器），
/// 因此 Err 分支仅作防御（如脚本依赖被清空的全局状态），单测只覆盖成功路径。
pub fn reload_all() -> Result<()> {
    init();
    let scripts = LOADED_SCRIPTS.with(|s| s.borrow().clone());
    reset_plugin_state();
    for (name, src) in &scripts {
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

    let id = NEXT_POPUP_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
    with_popups(|p| p.insert(id, PopupCallbacks { render, on_key, on_close }));
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::OpenPopup { id });
    Ok(JsValue::from(id))
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

/// 调 JS render 回调，返回行数组。ctx 对象 { width, height }。
pub fn render_popup(id: u64, width: u16, height: u16) -> Result<Vec<String>> {
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
            .map_err(|e| anyhow!("popup {id} render must return an array of strings: {e}"))?;
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
            let s: String = item.try_js_into(engine)
                .map_err(|e| anyhow!("popup {id} render line {i} must be a string: {e}"))?;
            lines.push(s);
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

    #[test]
    fn echo_captures_message() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(r#"helix.echo("hello from js");"#).unwrap();
        assert_eq!(take_messages(), vec!["hello from js"]);
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
        let UiRequest::OpenPopup { id } = reqs[0] else { unreachable!("expected OpenPopup") };
        assert_eq!(id, 1); // 自增从 1 开始

        let lines = render_popup(id, 40, 10).unwrap();
        assert_eq!(lines, vec!["a", "b", "c"]);

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
            UiRequest::OpenPopup { id } => id,
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
            UiRequest::OpenPopup { id } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        assert!(render_popup(id, 40, 10).is_err());
        close_popup(id).unwrap();

        // 参数缺失/类型错误 → JS 报错
        assert!(load_script(r#"helix.open_popup({});"#).is_err());
        assert!(load_script(r#"helix.open_popup({ render: 42 });"#).is_err());
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
            UiRequest::OpenPopup { id } => id,
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
}
