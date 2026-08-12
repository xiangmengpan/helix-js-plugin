use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use boa_engine::object::builtins::JsFunction;
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::NativeFunction;
use boa_engine::{Context, JsError, JsString, JsValue, Source};


/// 事件名白名单：helix.on 只接受这些事件
const EVENT_WHITELIST: [&str; 6] = ["save", "mode-change", "buffer-open", "buffer-close", "doc-change", "theme-change"];

use crate::state::{
    with_buffer_icon_hook, with_event_handlers, with_last_export,
    with_popups, with_registry, with_script_exports, with_statusline_hook, MESSAGES, PLUGINS_DIR, UI_REQUESTS,
};


use crate::types::*;

pub(crate) fn js_register_command(
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
            crate::state::with_command_docs(|d| d.insert(name.clone(), doc));
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
pub(crate) fn js_load(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
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
    crate::state::with_loaded_scripts(|s| {
        if !s.iter().any(|(n, _)| n == &key) {
            s.push((key.clone(), src));
        }
    });
    with_script_exports(|m| m.insert(key, export.clone()));
    Ok(export)
}

/// helix.export(obj)：声明当前脚本的导出（被 helix.load 的返回值拿到）。undefined/null 清空。
pub(crate) fn js_export(_this: &JsValue, args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let arg = args.first().cloned().unwrap_or(JsValue::undefined());
    with_last_export(|l| *l = if arg.is_null_or_undefined() { None } else { Some(arg) });
    Ok(JsValue::undefined())
}

/// helix.lazy(name, ...cmds)：为每个 cmd 注册桩闭包——首次调用时加载 name 再转执行。
/// 桩经 eval 工厂构造闭包（不经 REGISTRY 捕获 JsValue，避免闭包环境问题）。
pub(crate) fn js_lazy(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
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
pub(crate) fn js_get_str(obj: &boa_engine::JsObject, key: &str, ctx: &mut Context) -> boa_engine::JsResult<Option<String>> {
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
pub(crate) fn js_run_command(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
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
pub(crate) fn js_map(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
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
        let id = crate::state::next_map_id();
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
pub(crate) fn doc_to_js(ctx: &CommandContext, engine: &mut Context) -> boa_engine::JsResult<JsValue> {
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
    crate::init();
    // 命令开始时清空编辑队列，避免跨命令残留
    crate::state::with_edits(|c| c.clear());
    crate::state::with_cursor_requests(|c| c.clear());
    let func = with_registry(|r| r.get(name).cloned());
    let Some(func) = func else { return Ok(false) };

    let func = func
        .as_callable()
        .and_then(JsFunction::from_object)
        .ok_or_else(|| anyhow!("registered value for '{name}' is not a function"))?;

    crate::state::with_engine(|engine| {
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
    crate::init();
    with_registry(|r| r.keys().cloned().collect())
}

/// 插件命令说明（未注册返回 None）
pub fn command_doc(name: &str) -> Option<String> {
    crate::init();
    crate::state::with_command_docs(|d| d.get(name).cloned())
}

pub(crate) fn js_on(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
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
    crate::init();
    with_event_handlers(|h| h.get(name).is_some_and(|v| !v.is_empty()))
}

/// 触发事件：按注册顺序调用处理器；开始时清空编辑队列（防残留）。
/// extra 用于 mode-change 的 mode 字符串参数。
pub fn emit_event(name: &str, ctx: &CommandContext, extra: Option<&str>) -> Result<()> {
    crate::init();
    crate::state::with_edits(|c| c.clear());
    crate::state::with_cursor_requests(|c| c.clear());
    let handlers = with_event_handlers(|h| h.get(name).cloned());
    let Some(handlers) = handlers else { return Ok(()) };
    if handlers.is_empty() {
        return Ok(());
    }
    crate::state::with_engine(|engine| {
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
                crate::state::with_edits(|c| c.clear());
    crate::state::with_cursor_requests(|c| c.clear());
                anyhow!("event '{name}' handler failed: {e}")
            })?;
        }
        Ok(())
    })
}

pub(crate) fn js_set_cursor(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let row: usize = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let col: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    crate::state::with_cursor_requests(|c| c.push(CursorRequest::SetCursor { row, col }));
    Ok(JsValue::undefined())
}

pub(crate) fn js_set_selection(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let ar: usize = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let ac: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let hr: usize = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let hc: usize = args.get(3).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    crate::state::with_cursor_requests(|c| c.push(CursorRequest::SetSelection {
        anchor: (ar, ac),
        head: (hr, hc),
    }));
    Ok(JsValue::undefined())
}

/// shell 执行输出截断上限（防失控输出冻结状态栏）
const RUN_OUTPUT_LIMIT: usize = 65536;

// ponytail: 同步阻塞 + sh -c，仅 Unix；未来要 Windows 支持需改 cmd.exe /C。
pub(crate) fn js_run(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
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

pub(crate) fn js_echo(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
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

/// 读线程主循环（stdout/stderr 共用）。
/// aggregate=true：原始字节累积进 output，EOF 后统一 lossy 解码（跨块多字节字符不被切开）；
/// aggregate=false：流式增量解码——拼上跨块的残留尾部（最多 3 字节，UTF-8 最长序列），
pub(crate) fn js_doc_insert(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let row: usize = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let col: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let insert: String = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    crate::state::with_edits(|c| c.push(Edit { start: (row, col), end: (row, col), insert }));
    Ok(JsValue::undefined())
}

pub(crate) fn js_doc_replace(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let sr: usize = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let sc: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let er: usize = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let ec: usize = args.get(3).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let insert: String = args.get(4).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    crate::state::with_edits(|c| c.push(Edit { start: (sr, sc), end: (er, ec), insert }));
    Ok(JsValue::undefined())
}

pub(crate) fn js_doc_delete(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let sr: usize = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let sc: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let er: usize = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let ec: usize = args.get(3).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    crate::state::with_edits(|c| c.push(Edit { start: (sr, sc), end: (er, ec), insert: String::new() }));
    Ok(JsValue::undefined())
}

/// 取走并清空编辑队列（helix-term 在命令返回后消费）
pub fn take_edits() -> Vec<Edit> {
    crate::init();
    crate::state::with_edits(std::mem::take)
}

/// 触发节点事件（焦点路由）：onPress → 无参调用；onKey → 传键名
pub fn dispatch_node_event(view_id: u64, node_id: &str, key: Option<&str>) -> Result<()> {
    crate::init();
    crate::state::with_engine(|engine| {
        let handlers = crate::state::with_node_handlers(|h| {
            h.get(&(view_id, node_id.to_string()))
                .cloned()
                .map(|hd| NodeHandlers { on_press: hd.on_press.clone(), on_key: hd.on_key.clone() })
        });
        let Some(handlers) = handlers else { return Ok(()) };
        let undefined = JsValue::undefined();
        match key {
            None => {
                // button 按下（Enter/Space）
                if let Some(f) = handlers.on_press {
                    let func = f.as_callable().and_then(JsFunction::from_object)
                        .ok_or_else(|| anyhow!("node {node_id} onPress not callable"))?;
                    let _: JsValue = func.call(&undefined, &[], engine)
                        .map_err(|e| anyhow!("node {node_id} onPress failed: {e}"))?;
                }
            }
            Some(k) => {
                // input 按键
                if let Some(f) = handlers.on_key {
                    let func = f.as_callable().and_then(JsFunction::from_object)
                        .ok_or_else(|| anyhow!("node {node_id} onKey not callable"))?;
                    let _: JsValue = func.call(&undefined, &[JsValue::from(JsString::from(k.to_string()))], engine)
                        .map_err(|e| anyhow!("node {node_id} onKey failed: {e}"))?;
                }
            }
        }
        Ok(())
    })
}

/// 收集组件树中的可聚焦节点 id（button/input，树序）
pub fn focusable_node_ids(node: &CompNode, out: &mut Vec<String>) {
    match node {
        CompNode::Button { id, .. } => out.push(id.clone()),
        CompNode::Input { id, .. } => out.push(id.clone()),
        CompNode::Row { children, .. } | CompNode::Col { children, .. } | CompNode::Scroll { children, .. } => {
            for c in children {
                focusable_node_ids(c, out);
            }
        }
        CompNode::Text { .. } => {}
    }
}

/// 取走并清空光标/选区请求队列（helix-term 消费）
pub fn take_cursor_requests() -> Vec<CursorRequest> {
    crate::init();
    crate::state::with_cursor_requests(std::mem::take)
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
    crate::init();
    crate::state::with_engine(|engine| {
        eval_wrapped(engine, src)
            .map(|_| ())
            .map_err(|e| anyhow!("plugin script '{name}' error: {e}"))?;
        crate::state::with_loaded_scripts(|s| s.push((name.to_string(), src.to_string())));
        Ok(())
    })
}

/// 已加载脚本名（按加载顺序；供 :plugin list / status）
pub fn loaded_scripts() -> Vec<String> {
    crate::init();
    crate::state::with_loaded_scripts(|s| s.iter().map(|(name, _)| name.clone()).collect())
}

/// 清空所有插件状态（热重载用；id 计数器保留保证唯一性）。
/// 注意：UI_REQUESTS/MESSAGES 不清——它们是命令边界 drain 的队列。
fn reset_plugin_state() {
    with_registry(|r| r.clear());
    with_event_handlers(|h| h.clear());
    with_popups(|p| p.clear());
    crate::state::with_edits(|c| c.clear());
    crate::state::with_cursor_requests(|c| c.clear());
    with_buffer_icon_hook(|h| *h = None);
    with_statusline_hook(|h| *h = None);
    crate::state::set_last_panel_id(None);
    crate::state::with_command_docs(|d| d.clear());
    // 导出缓存与 pending 导出随插件状态重置（reload 后按名重读磁盘重跑）
    with_script_exports(|m| m.clear());
    with_last_export(|l| *l = None);
    // 主题覆盖随插件状态重置：清空并置脏（下次 drain 还原基准主题）
    crate::state::with_theme_overrides(|o| o.clear());
    crate::state::mark_theme_dirty();
}

/// 热重载：清空状态后按加载顺序重跑全部脚本。
/// 脚本以 IIFE 包裹求值，每次重跑都是全新词法作用域——顶层 const/let/var 都不冲突，
/// reload 必然成功（脚本不依赖其他脚本的状态，reload 会重新注册全部命令/处理器），
/// 因此 Err 分支仅作防御（如脚本依赖被清空的全局状态），单测只覆盖成功路径。
pub fn reload_all() -> Result<()> {
    crate::init();
    // 面板层还挂在 compositor 上——入队 ClosePanel 让 plugin_reload 的 apply_ui_requests 移除它，
    // 否则 reset_plugin_state 清掉 LAST_PANEL_ID 后该层变成无法关闭的僵尸层
    if let Some(panel_id) = crate::state::last_panel_id() {
        UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::ClosePanel { id: panel_id });
    }
    let scripts = crate::state::with_loaded_scripts(|s| s.clone());
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
        crate::state::with_engine(|engine| -> Result<()> {
            eval_wrapped(engine, src)
                .map(|_| ())
                .map_err(|e| anyhow!("plugin reload '{name}' failed: {e}"))?;
            Ok(())
        })?;
    }
    Ok(())
}

