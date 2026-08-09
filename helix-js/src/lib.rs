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
const EVENT_WHITELIST: [&str; 4] = ["save", "mode-change", "buffer-open", "buffer-close"];

/// 插件命令收到的只读上下文快照（由 helix-term 序列化编辑器状态得到）
pub struct CommandContext {
    pub path: Option<String>,
    pub text: String,
    pub cursor: (usize, usize),
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

// boa 的 Context/JsValue 是 !Send（Rc GC 堆），不能用 static 全局共享，
// 所以引擎按线程存放（编辑器主线程是唯一调用者）；MESSAGES 跨线程共享。
// drop 顺序：所有公共函数先调用 init()（先触达 CONTEXT），故线程销毁时
// REGISTRY 先于 CONTEXT drop，JsValue 的 GC 引用在 CONTEXT 销毁前释放。
// ponytail: 实现时观察到进程退出阶段偶发 tcache 崩溃（疑似 Context drop 的
// double-finalize，但独立复现未能确认），故 Box::leak 泄漏到 'static 规避——
// 进程退出时 OS 回收，对 PoC 无实际代价。升级 boa 后应改回正常持有。
thread_local! {
    static CONTEXT: RefCell<Option<&'static mut Context>> = const { RefCell::new(None) };
    // HashMap::new 非 const fn（1.90），REGISTRY 不能用 const 块初始化
    static REGISTRY: RefCell<HashMap<String, JsValue>> = RefCell::new(HashMap::new());
    static POPUPS: RefCell<HashMap<u64, PopupCallbacks>> = RefCell::new(HashMap::new());
    static NEXT_POPUP_ID: Cell<u64> = const { Cell::new(1) };
    static NEXT_MAP_ID: Cell<u64> = const { Cell::new(1) };
    static BUFFER_ICON_HOOK: RefCell<Option<JsValue>> = const { RefCell::new(None) };
    static STATUSLINE_HOOK: RefCell<Option<JsValue>> = const { RefCell::new(None) };
    static CURRENT_EDITS: RefCell<Vec<Edit>> = const { RefCell::new(Vec::new()) };
    // HashMap::new 非 const fn，EVENT_HANDLERS 不能用 const 块初始化
    static EVENT_HANDLERS: RefCell<HashMap<String, Vec<JsValue>>> = RefCell::new(HashMap::new());
}
static MESSAGES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static UI_REQUESTS: OnceLock<Mutex<Vec<UiRequest>>> = OnceLock::new();

/// 创建 boa 上下文并注册全局 `helix` 对象（幂等）
pub fn init() {
    MESSAGES.get_or_init(Default::default);
    UI_REQUESTS.get_or_init(Default::default);
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
                .function(NativeFunction::from_fn_ptr(js_set_statusline), JsString::from("set_statusline"), 1)
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
    REGISTRY.with(|r| r.borrow_mut().insert(name, func));
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
        REGISTRY.with(|r| r.borrow_mut().insert(name.clone(), command_arg));
        name
    } else {
        command_arg.try_js_into(context)?
    };

    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::MapKey { mode, key, command });
    Ok(JsValue::undefined())
}

/// 把 CommandContext 转成 doc 对象 { path, text } + 编辑方法
fn doc_to_js(ctx: &CommandContext, engine: &mut Context) -> boa_engine::JsResult<JsValue> {
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
        .function(NativeFunction::from_fn_ptr(js_doc_insert), JsString::from("insert"), 3)
        .function(NativeFunction::from_fn_ptr(js_doc_replace), JsString::from("replace"), 5)
        .function(NativeFunction::from_fn_ptr(js_doc_delete), JsString::from("delete"), 4)
        .build(),
    ))
}

/// 把 CommandContext 转成 JS 对象 { doc: { path, text }, cursor: { row, col } }
fn ctx_to_js(ctx: &CommandContext, engine: &mut Context) -> boa_engine::JsResult<JsValue> {
    let doc = doc_to_js(ctx, engine)?;
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
            .build(),
    ))
}

/// 运行插件命令。返回 Ok(true) 表示已运行，Ok(false) 表示未注册
pub fn run_command(name: &str, ctx: &CommandContext) -> Result<bool> {
    init();
    // 命令开始时清空编辑队列，避免跨命令残留
    CURRENT_EDITS.with(|c| c.borrow_mut().clear());
    let func = REGISTRY.with(|r| r.borrow().get(name).cloned());
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
    REGISTRY.with(|r| r.borrow().keys().cloned().collect())
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
    EVENT_HANDLERS.with(|h| h.borrow_mut().entry(name).or_default().push(handler));
    Ok(JsValue::undefined())
}

/// 是否有注册的事件处理器（挂点快速跳过）
pub fn has_handlers(name: &str) -> bool {
    init();
    EVENT_HANDLERS.with(|h| h.borrow().get(name).is_some_and(|v| !v.is_empty()))
}

/// 触发事件：按注册顺序调用处理器；开始时清空编辑队列（防残留）。
/// extra 用于 mode-change 的 mode 字符串参数。
pub fn emit_event(name: &str, ctx: &CommandContext, extra: Option<&str>) -> Result<()> {
    init();
    CURRENT_EDITS.with(|c| c.borrow_mut().clear());
    let handlers = EVENT_HANDLERS.with(|h| h.borrow().get(name).cloned());
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
                anyhow!("event '{name}' handler failed: {e}")
            })?;
        }
        Ok(())
    })
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

/// 求值一段插件脚本；脚本里可调用 `helix.register_command` / `helix.echo`
pub fn load_script(src: &str) -> Result<()> {
    init();
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized by init()");
        engine
            .eval(Source::from_bytes(src))
            .map(|_| ())
            .map_err(|e| anyhow!("plugin script error: {e}"))
    })
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
    POPUPS.with(|p| p.borrow_mut().insert(id, PopupCallbacks { render, on_key, on_close }));
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
    BUFFER_ICON_HOOK.with(|h| *h.borrow_mut() = Some(hook.clone()));
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
        let render = POPUPS.with(|p| p.borrow().get(&id).map(|cb| cb.render.clone()))
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

/// 调 JS onKey 回调。未注册 onKey 或缺省时：Esc→Close，其他→Ignore。
pub fn popup_key(id: u64, key: &PluginKey) -> Result<PopupKeyResult> {
    init();
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized");
        let callbacks = POPUPS.with(|p| p.borrow().get(&id).cloned())
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
        let undefined = JsValue::undefined();
        let value: JsValue = func.call(&undefined, &[JsValue::from(key_obj)], engine)
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
        let callbacks = POPUPS.with(|p| p.borrow_mut().remove(&id));
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
        STATUSLINE_HOOK.with(|h| *h.borrow_mut() = None);
    } else if arg.as_callable().is_some() {
        STATUSLINE_HOOK.with(|h| *h.borrow_mut() = Some(arg));
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
    let hook = STATUSLINE_HOOK.with(|h| h.borrow().clone())?;
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
        let hook = BUFFER_ICON_HOOK.with(|h| h.borrow().clone());
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
        assert_eq!(popup_key(id, &key).unwrap(), PopupKeyResult::Handled);
        let key = PluginKey { name: "Esc".into(), shift: false, ctrl: false, alt: false };
        assert_eq!(popup_key(id, &key).unwrap(), PopupKeyResult::Close);

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
        assert_eq!(popup_key(id, &key).unwrap(), PopupKeyResult::Ignored);
        let key = PluginKey { name: "Esc".into(), shift: false, ctrl: false, alt: false };
        assert_eq!(popup_key(id, &key).unwrap(), PopupKeyResult::Close);
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
        let ctx = CommandContext { path: None, text: String::new(), cursor: (1, 2) };
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
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0) };
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
        let ctx = CommandContext { path: Some("/tmp/e.rs".into()), text: "x".into(), cursor: (0, 0) };
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
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0) };
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
}
