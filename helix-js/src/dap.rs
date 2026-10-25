//! `helix.dap.*` —— 调试（DAP）的插件 API。
//!
//! 设计见 `docs/superpowers/specs/2026-10-25-dap-api-p1.md`。三条入口：
//!
//! | 类别 | 成员 | 机制 |
//! |---|---|---|
//! | 同步快照 | `sessions()` `configs()` `list_breakpoints()` | 读宿主缓存（`cache_dap_*` 写入），零 IO |
//! | 动作/查询 | 其余（Promise） | `JsPromise::new_pending` → `DapRequest` 入队 → helix-term 派发 →
//! |            |                  | 回来时 `resolve()` 兑现 |
//! | 事件 | 9 个 `dap-*` | `emit_event()` 调 JS handler（payload 为 JSON 对象） |
//!
//! **为什么请求要入队而不是直接调**：`helix-js` 不认识 `Editor`/`Registry`；能 await 且有
//! `&mut Editor` 的地方只有 helix-term 的事件循环。所以这里只做"校验 + 挂起 Promise + 入队"，
//! 派发与兑现都在 `helix-term/src/commands/dap.rs`。
//!
//! **行号口径**：`row` 一律 **0 起始**（与 `helix.lsp.*` / `set_virtual_text` 一致）；
//! DAP 的 1 起始换算只在内核（`BreakpointStore` / `Client`）里发生，不外泄。

use crate::permissions::CbKey;
use boa_engine::object::builtins::JsFunction;
use boa_engine::object::builtins::{JsArray, JsPromise};
use boa_engine::{Context, JsString, JsValue};
use serde_json::Value;

/// 一条待派发的 DAP 请求（helix-term 侧消费）。
#[derive(Debug, Clone)]
pub struct DapRequest {
    /// Promise id（与 `dap_promises` 对应）
    pub id: u64,
    /// 成员名（`"launch"` / `"stack_frames"` / …），派发侧按它分派
    pub kind: String,
    /// 已校验过的参数（JSON 对象；具体字段见 spec）
    pub params: Value,
}

/// Promise 的兑现结果。
#[derive(Debug)]
pub enum DapOutcome {
    /// `Some(json)` → `JSON.parse` 后 resolve；`None` → resolve `null`
    Json(Option<String>),
    /// reject（消息会进 `Error`）
    Err(String),
}

impl DapOutcome {
    pub fn json(v: impl serde::Serialize) -> Self {
        match serde_json::to_string(&v) {
            Ok(json) => Self::Json(Some(json)),
            Err(err) => Self::Err(format!("dap: failed to serialize result: {err}")),
        }
    }
    pub fn unit() -> Self {
        Self::Json(None)
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Self::Err(msg.into())
    }
}

// ============================ 入队（JS → 宿主） ============================

/// 挂起 Promise + 入队一条请求。返回 Promise 的 JS 值。
fn enqueue(ctx: &mut Context, kind: &str, params: Value) -> JsValue {
    let (promise, resolving) = JsPromise::new_pending(ctx);
    let id = crate::state::next_async_id();
    let st = crate::host::host_state(ctx);
    crate::state::with_dap_promises(st, |m| {
        m.insert(id, resolving);
    });
    // 注册期绑定属主：兑现时跑的是**发起者**注册的 .then/.catch（插件卸载要能摘掉）
    crate::permissions::bind_callback(CbKey::dap_promise(id));
    st.dap_requests.borrow_mut().push(DapRequest {
        id,
        kind: kind.to_string(),
        params,
    });
    promise.into()
}

/// 取参数对象里的可选 `id`（会话 id）。缺省 = 活动会话（派发侧决定）。
fn opt_id(
    obj: &boa_engine::JsObject,
    ctx: &mut Context,
    api: &str,
) -> boa_engine::JsResult<Option<u64>> {
    let v = obj.get(JsString::from("id"), ctx)?;
    if v.is_null_or_undefined() {
        return Ok(None);
    }
    let n: f64 = v
        .try_js_into(ctx)
        .map_err(|_| crate::js_err(format!("{api}: 'id' must be a number")))?;
    Ok(Some(n as u64))
}

fn arg_object(
    args: &[JsValue],
    idx: usize,
    _ctx: &mut Context,
    api: &str,
) -> boa_engine::JsResult<Option<boa_engine::JsObject>> {
    match args.get(idx) {
        None => Ok(None),
        Some(v) if v.is_null_or_undefined() => Ok(None),
        Some(v) => match v.as_object() {
            Some(obj) => Ok(Some(obj.clone())),
            None => Err(crate::js_err(format!("{api}: argument must be an object"))),
        },
    }
}

fn to_json(v: &JsValue, ctx: &mut Context, api: &str) -> boa_engine::JsResult<Value> {
    v.to_json(ctx)
        .map_err(|e| crate::js_err(format!("{api}: {e}")))
        .map(|v| v.unwrap_or(Value::Null))
}

fn opt_string(
    args: &[JsValue],
    idx: usize,
    ctx: &mut Context,
    api: &str,
    field: &str,
) -> boa_engine::JsResult<Option<String>> {
    match args.get(idx) {
        None => Ok(None),
        Some(v) if v.is_null_or_undefined() => Ok(None),
        Some(v) => v
            .try_js_into::<String>(ctx)
            .map(Some)
            .map_err(|_| crate::js_err(format!("{api}: '{field}' must be a string"))),
    }
}

fn req_number(
    args: &[JsValue],
    idx: usize,
    ctx: &mut Context,
    api: &str,
    field: &str,
) -> boa_engine::JsResult<u64> {
    let Some(v) = args.get(idx) else {
        return Err(crate::js_err(format!("{api}: '{field}' is required")));
    };
    if v.is_null_or_undefined() {
        return Err(crate::js_err(format!("{api}: '{field}' is required")));
    }
    let n: f64 = v
        .try_js_into(ctx)
        .map_err(|_| crate::js_err(format!("{api}: '{field}' must be a number")))?;
    if !n.is_finite() || n < 0.0 {
        return Err(crate::js_err(format!(
            "{api}: '{field}' must be a non-negative number"
        )));
    }
    Ok(n as u64)
}

// ============================ 同步快照 ============================

fn snapshot(ctx: &mut Context, json: &str, api: &str) -> JsValue {
    let json = if json.is_empty() { "[]" } else { json };
    match crate::layout_api::js_json_parse(json.to_string(), ctx, api) {
        Ok(v) => v,
        // 缓存坏了不该抛给插件（它每帧都可能调）⇒ 退化成空数组
        Err(_) => JsArray::new(ctx)
            .map(JsValue::from)
            .unwrap_or_else(|_| JsValue::null()),
    }
}

/// `helix.dap.sessions()` → 会话快照（同步）
pub(crate) fn js_dap_sessions(
    _: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let json = crate::host::host_state(ctx).dap_sessions.borrow().clone();
    Ok(snapshot(ctx, &json, "helix.dap.sessions"))
}

/// `helix.dap.configs()` → 可用调试配置（同步；P1 只有 `source: "lang"`）
pub(crate) fn js_dap_configs(
    _: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let json = crate::host::host_state(ctx).dap_configs.borrow().clone();
    Ok(snapshot(ctx, &json, "helix.dap.configs"))
}

/// `helix.dap.list_breakpoints(path?)` → 断点快照（同步，可按文件过滤）
pub(crate) fn js_dap_list_breakpoints(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    const API: &str = "helix.dap.list_breakpoints";
    let filter = opt_string(args, 0, ctx, API, "path")?;
    let json = crate::host::host_state(ctx)
        .dap_breakpoints
        .borrow()
        .clone();
    let json = if json.is_empty() {
        "[]".to_string()
    } else {
        json
    };
    let Some(filter) = filter else {
        return Ok(snapshot(ctx, &json, API));
    };
    // 过滤在 Rust 侧做（缓存是全表）：调用频率是"打开断点面板时"，不是每帧
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&json) else {
        return Ok(JsArray::new(ctx)
            .map(JsValue::from)
            .unwrap_or_else(|_| JsValue::null()));
    };
    let kept: Vec<Value> = items
        .into_iter()
        .filter(|item| item.get("path").and_then(|p| p.as_str()) == Some(filter.as_str()))
        .collect();
    Ok(snapshot(
        ctx,
        &serde_json::to_string(&kept).unwrap_or_else(|_| "[]".into()),
        API,
    ))
}

// ============================ 动作 / 查询（Promise） ============================

fn enqueue_simple(
    args: &[JsValue],
    ctx: &mut Context,
    kind: &str,
    api: &str,
) -> boa_engine::JsResult<JsValue> {
    let mut params = serde_json::Map::new();
    if let Some(obj) = arg_object(args, 0, ctx, api)? {
        if let Some(id) = opt_id(&obj, ctx, api)? {
            params.insert("id".into(), Value::from(id));
        }
    }
    Ok(enqueue(ctx, kind, Value::Object(params)))
}

pub(crate) fn js_dap_terminate(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    // 兼容 `terminate(id?)`：数字直接当 id
    let api = "helix.dap.terminate";
    let mut params = serde_json::Map::new();
    if let Some(v) = args.first() {
        if !v.is_null_or_undefined() {
            let n: f64 = v
                .try_js_into(ctx)
                .map_err(|_| crate::js_err(format!("{api}: 'id' must be a number")))?;
            params.insert("id".into(), Value::from(n as u64));
        }
    }
    Ok(enqueue(ctx, "terminate", Value::Object(params)))
}

pub(crate) fn js_dap_restart(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_simple(args, ctx, "restart", "helix.dap.restart")
}

pub(crate) fn js_dap_continue(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_simple(args, ctx, "continue", "helix.dap.continue")
}

pub(crate) fn js_dap_step_in(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_simple(args, ctx, "step_in", "helix.dap.step_in")
}

pub(crate) fn js_dap_step_out(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_simple(args, ctx, "step_out", "helix.dap.step_out")
}

pub(crate) fn js_dap_next(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_simple(args, ctx, "next", "helix.dap.next")
}

pub(crate) fn js_dap_threads(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_simple(args, ctx, "threads", "helix.dap.threads")
}

/// `pause(thread?, id?)`
pub(crate) fn js_dap_pause(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    const API: &str = "helix.dap.pause";
    let mut params = serde_json::Map::new();
    if let Some(v) = args.first() {
        if !v.is_null_or_undefined() {
            let n: f64 = v
                .try_js_into(ctx)
                .map_err(|_| crate::js_err(format!("{API}: 'thread' must be a number")))?;
            params.insert("thread".into(), Value::from(n as i64));
        }
    }
    if let Some(v) = args.get(1) {
        if !v.is_null_or_undefined() {
            let n: f64 = v
                .try_js_into(ctx)
                .map_err(|_| crate::js_err(format!("{API}: 'id' must be a number")))?;
            params.insert("id".into(), Value::from(n as u64));
        }
    }
    Ok(enqueue(ctx, "pause", Value::Object(params)))
}

/// `stack_frames(thread, id?)`
pub(crate) fn js_dap_stack_frames(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    const API: &str = "helix.dap.stack_frames";
    let thread = req_number(args, 0, ctx, API, "thread")?;
    let mut params = serde_json::Map::new();
    params.insert("thread".into(), Value::from(thread));
    if let Some(v) = args.get(1) {
        if !v.is_null_or_undefined() {
            let n: f64 = v
                .try_js_into(ctx)
                .map_err(|_| crate::js_err(format!("{API}: 'id' must be a number")))?;
            params.insert("id".into(), Value::from(n as u64));
        }
    }
    Ok(enqueue(ctx, "stack_frames", Value::Object(params)))
}

fn frame_or_ref(
    kind: &str,
    field: &str,
    api: &str,
) -> impl Fn(&JsValue, &[JsValue], &mut Context) -> boa_engine::JsResult<JsValue> {
    let kind = kind.to_string();
    let field = field.to_string();
    let api_owned = api.to_string();
    move |_: &JsValue, args: &[JsValue], ctx: &mut Context| {
        let n = req_number(args, 0, ctx, &api_owned, &field)?;
        let mut params = serde_json::Map::new();
        params.insert(field.clone(), Value::from(n));
        if let Some(v) = args.get(1) {
            if !v.is_null_or_undefined() {
                let id: f64 = v
                    .try_js_into(ctx)
                    .map_err(|_| crate::js_err(format!("{api_owned}: 'id' must be a number")))?;
                params.insert("id".into(), Value::from(id as u64));
            }
        }
        Ok(enqueue(ctx, &kind, Value::Object(params)))
    }
}

/// `scopes(frame, id?)`
pub(crate) fn js_dap_scopes(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    frame_or_ref("scopes", "frame", "helix.dap.scopes")(&JsValue::undefined(), args, ctx)
}

/// `variables(reference, id?)`
pub(crate) fn js_dap_variables(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    frame_or_ref("variables", "reference", "helix.dap.variables")(&JsValue::undefined(), args, ctx)
}

/// `eval(expr, frame?, id?)`
pub(crate) fn js_dap_eval(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    const API: &str = "helix.dap.eval";
    let expr = opt_string(args, 0, ctx, API, "expr")?
        .ok_or_else(|| crate::js_err(format!("{API}: 'expr' is required")))?;
    if expr.is_empty() {
        return Err(crate::js_err(format!("{API}: 'expr' must not be empty")));
    }
    let mut params = serde_json::Map::new();
    params.insert("expr".into(), Value::from(expr));
    if let Some(v) = args.get(1) {
        if !v.is_null_or_undefined() {
            let n: f64 = v
                .try_js_into(ctx)
                .map_err(|_| crate::js_err(format!("{API}: 'frame' must be a number")))?;
            params.insert("frame".into(), Value::from(n as u64));
        }
    }
    if let Some(v) = args.get(2) {
        if !v.is_null_or_undefined() {
            let n: f64 = v
                .try_js_into(ctx)
                .map_err(|_| crate::js_err(format!("{API}: 'id' must be a number")))?;
            params.insert("id".into(), Value::from(n as u64));
        }
    }
    Ok(enqueue(ctx, "eval", Value::Object(params)))
}

/// `set_breakpoint(path, row, opts?)`
pub(crate) fn js_dap_set_breakpoint(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    const API: &str = "helix.dap.set_breakpoint";
    let path = opt_string(args, 0, ctx, API, "path")?
        .ok_or_else(|| crate::js_err(format!("{API}: 'path' is required")))?;
    if !path.starts_with('/') {
        return Err(crate::js_err(format!(
            "{API}: 'path' must be absolute (got {path:?})"
        )));
    }
    let row = req_number(args, 1, ctx, API, "row")?;
    let mut params = serde_json::Map::new();
    params.insert("path".into(), Value::from(path));
    params.insert("row".into(), Value::from(row));
    if let Some(obj) = arg_object(args, 2, ctx, API)? {
        let mut opts = serde_json::Map::new();
        for field in ["condition", "hit", "log"] {
            let v = obj.get(JsString::from(field), ctx)?;
            if !v.is_null_or_undefined() {
                let s: String = v
                    .try_js_into(ctx)
                    .map_err(|_| crate::js_err(format!("{API}: opts.{field} must be a string")))?;
                opts.insert(field.into(), Value::from(s));
            }
        }
        params.insert("opts".into(), Value::Object(opts));
    }
    Ok(enqueue(ctx, "set_breakpoint", Value::Object(params)))
}

/// `remove_breakpoint(path, row)`
pub(crate) fn js_dap_remove_breakpoint(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    const API: &str = "helix.dap.remove_breakpoint";
    let path = opt_string(args, 0, ctx, API, "path")?
        .ok_or_else(|| crate::js_err(format!("{API}: 'path' is required")))?;
    let row = req_number(args, 1, ctx, API, "row")?;
    let mut params = serde_json::Map::new();
    params.insert("path".into(), Value::from(path));
    params.insert("row".into(), Value::from(row));
    Ok(enqueue(ctx, "remove_breakpoint", Value::Object(params)))
}

/// `clear_breakpoints(path?)`
pub(crate) fn js_dap_clear_breakpoints(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    const API: &str = "helix.dap.clear_breakpoints";
    let mut params = serde_json::Map::new();
    if let Some(path) = opt_string(args, 0, ctx, API, "path")? {
        params.insert("path".into(), Value::from(path));
    }
    Ok(enqueue(ctx, "clear_breakpoints", Value::Object(params)))
}

/// `set_exception_breakpoints(filters)`
pub(crate) fn js_dap_set_exception_breakpoints(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    const API: &str = "helix.dap.set_exception_breakpoints";
    let Some(v) = args.first() else {
        return Err(crate::js_err(format!("{API}: 'filters' is required")));
    };
    let Some(arr) = v
        .as_object()
        .and_then(|o| JsArray::from_object(o.clone()).ok())
    else {
        return Err(crate::js_err(format!("{API}: 'filters' must be an array")));
    };
    let len = arr.length(ctx)?;
    let mut filters = Vec::new();
    for i in 0..len {
        let item = arr.get(i, ctx)?;
        let s: String = item
            .try_js_into(ctx)
            .map_err(|_| crate::js_err(format!("{API}: filters[{i}] must be a string")))?;
        filters.push(Value::from(s));
    }
    let mut params = serde_json::Map::new();
    params.insert("filters".into(), Value::Array(filters));
    Ok(enqueue(
        ctx,
        "set_exception_breakpoints",
        Value::Object(params),
    ))
}

/// `request(name, args?, id?)` —— 逃生口：任意 DAP 请求，响应原样返回
pub(crate) fn js_dap_request(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    const API: &str = "helix.dap.request";
    let name = opt_string(args, 0, ctx, API, "name")?
        .ok_or_else(|| crate::js_err(format!("{API}: 'name' is required")))?;
    if name.is_empty() {
        return Err(crate::js_err(format!("{API}: 'name' must not be empty")));
    }
    let mut params = serde_json::Map::new();
    params.insert("name".into(), Value::from(name));
    if let Some(v) = args.get(1) {
        if !v.is_null_or_undefined() {
            params.insert("args".into(), to_json(v, ctx, API)?);
        }
    }
    if let Some(v) = args.get(2) {
        if !v.is_null_or_undefined() {
            let n: f64 = v
                .try_js_into(ctx)
                .map_err(|_| crate::js_err(format!("{API}: 'id' must be a number")))?;
            params.insert("id".into(), Value::from(n as u64));
        }
    }
    Ok(enqueue(ctx, "request", Value::Object(params)))
}

/// `launch(name?, args?, id?)` / `attach(...)`：名字在第一参，`args` 在第二参
fn start_session(
    ctx: &mut Context,
    kind: &str,
    args: &[JsValue],
    api: &str,
) -> boa_engine::JsResult<JsValue> {
    let name = opt_string(args, 0, ctx, api, "name")?;
    let mut params = serde_json::Map::new();
    if let Some(name) = name {
        params.insert("name".into(), Value::from(name));
    }
    if let Some(v) = args.get(1) {
        if !v.is_null_or_undefined() {
            params.insert("args".into(), to_json(v, ctx, api)?);
        }
    }
    Ok(enqueue(ctx, kind, Value::Object(params)))
}

pub(crate) fn js_dap_launch(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    start_session(ctx, "launch", args, "helix.dap.launch")
}

pub(crate) fn js_dap_attach(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    start_session(ctx, "attach", args, "helix.dap.attach")
}

// ============================ 兑现 ============================

/// 兑现（或拒绝）一个 DAP Promise。未知/已兑现 id → no-op（幂等）。
pub fn resolve(st: &crate::host::HostState, id: u64, outcome: DapOutcome) -> Result<(), String> {
    crate::init();
    crate::permissions::with_callback_owner(&CbKey::dap_promise(id), || {
        crate::state::with_engine(|engine| {
            let Some(resolving) = crate::state::with_dap_promises(st, |m| m.remove(&id)) else {
                return Ok(());
            };
            let undefined = JsValue::undefined();
            match outcome {
                DapOutcome::Json(json) => {
                    let value = match json {
                        None => Ok(JsValue::null()),
                        Some(json) => crate::layout_api::js_json_parse(json, engine, "dap result"),
                    };
                    match value {
                        Ok(parsed) => resolving
                            .resolve
                            .call(&undefined, &[parsed], engine)
                            .map(|_| ()),
                        Err(err) => resolving
                            .reject
                            .call(
                                &undefined,
                                &[JsValue::from(JsString::from(format!(
                                    "dap: invalid result: {err}"
                                )))],
                                engine,
                            )
                            .map(|_| ()),
                    }
                }
                DapOutcome::Err(msg) => resolving
                    .reject
                    .call(&undefined, &[JsValue::from(JsString::from(msg))], engine)
                    .map(|_| ()),
            }
            .map_err(|e| format!("dap promise {id}: {e}"))
        })
    })
}

// ============================ 事件 ============================

/// 9 个 DAP 事件名（白名单见 `commands.rs`，文档门禁见 `docs/plugin-api.md`）。
pub const EVENTS: [&str; 9] = [
    "dap-session",
    "dap-stopped",
    "dap-continued",
    "dap-exited",
    "dap-terminated",
    "dap-output",
    "dap-thread",
    "dap-breakpoint",
    "dap-progress",
];

/// 发一个 DAP 事件：payload 是**一个 JSON 对象**（扁平字段，见 d.ts）。
/// 没注册 handler 时零开销返回（不碰 engine）。
pub fn emit_event(name: &str, payload_json: &str) {
    crate::init();
    let handlers = crate::state::with_event_handlers_current(|h| h.get(name).cloned());
    let Some(handlers) = handlers else { return };
    if handlers.is_empty() {
        return;
    }
    crate::state::with_engine(|engine| {
        let Ok(payload) = crate::layout_api::js_json_parse(payload_json.to_string(), engine, name)
        else {
            return;
        };
        let undefined = JsValue::undefined();
        for handler in &handlers {
            let Some(func) = handler.as_callable().and_then(JsFunction::from_object) else {
                continue;
            };
            // 处理器的属主：事件回调在注册时已绑定，这里只需按它执行（与 diagnostics 同款）
            let _ = func.call(&undefined, std::slice::from_ref(&payload), engine);
        }
    })
}

// ============================ 宿主缓存写入（helix-term 侧调用） ============================

/// 刷新快照缓存：会话列表 / 断点表 / 可用配置。参数都是**已序列化好的 JSON 数组**。
pub fn cache_snapshots(
    st: &crate::host::HostState,
    sessions: &str,
    breakpoints: &str,
    configs: &str,
) {
    *st.dap_sessions.borrow_mut() = sessions.to_string();
    *st.dap_breakpoints.borrow_mut() = breakpoints.to_string();
    *st.dap_configs.borrow_mut() = configs.to_string();
}

/// 取走待派发请求（helix-term 的事件循环调用）。
pub fn take_requests(st: &crate::host::HostState) -> Vec<DapRequest> {
    std::mem::take(&mut *st.dap_requests.borrow_mut())
}

/// 请求数（早退判定用：有请求就不能早退）。
pub fn has_pending_requests(st: &crate::host::HostState) -> bool {
    !st.dap_requests.borrow().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::builtins::promise::PromiseState;
    use boa_engine::Source;

    fn eval(script: &str) -> Result<(), String> {
        crate::state::with_engine(|engine| engine.eval(Source::from_bytes(script.as_bytes())))
            .map(|_| ())
            .map_err(|e| format!("{e}"))
    }

    #[test]
    fn requests_are_validated_and_queued() {
        let st = crate::host::state_handle_or_init();
        let _ = take_requests(&st);
        eval(
            r#"globalThis.__p = helix.dap.stack_frames(3);
               globalThis.__p2 = helix.dap.set_breakpoint("/tmp/a.rs", 4, {condition: "x>1"});"#,
        )
        .expect("合法调用不抛");
        let reqs = take_requests(&st);
        assert_eq!(reqs.len(), 2, "两条入队: {reqs:?}");
        assert_eq!(reqs[0].kind, "stack_frames");
        assert_eq!(reqs[0].params["thread"], 3);
        assert_eq!(reqs[1].kind, "set_breakpoint");
        assert_eq!(reqs[1].params["row"], 4);
        assert_eq!(reqs[1].params["opts"]["condition"], "x>1");
        // Promise 真的挂起了（pending 状态）
        let pending = crate::state::with_engine(|engine| {
            engine
                .global_object()
                .get(JsString::from("__p"), engine)
                .ok()
                .and_then(|v| {
                    boa_engine::object::builtins::JsPromise::from_object(v.as_object()?.clone())
                        .ok()
                })
                .map(|p| p.state())
        });
        assert_eq!(
            format!("{:?}", pending),
            format!("{:?}", Some(PromiseState::Pending)),
            "Promise 必须处于 pending（由 helix-term 兑现）"
        );
    }

    #[test]
    fn invalid_arguments_throw_and_queue_nothing() {
        let st = crate::host::state_handle_or_init();
        let _ = take_requests(&st);
        assert!(
            eval(r#"helix.dap.stack_frames();"#).is_err(),
            "缺 thread 要抛"
        );
        assert!(
            eval(r#"helix.dap.set_breakpoint("rel/path.rs", 1);"#).is_err(),
            "相对路径要抛"
        );
        assert!(eval(r#"helix.dap.eval("");"#).is_err(), "空表达式要抛");
        assert!(
            eval(r#"helix.dap.variables("nope");"#).is_err(),
            "非数字 reference 要抛"
        );
        assert!(take_requests(&st).is_empty(), "校验失败不应入队");
    }

    #[test]
    fn snapshots_parse_from_cache() {
        let st = crate::host::state_handle_or_init();
        cache_snapshots(
            &st,
            r#"[{"id":1,"name":"lldb-dap","state":"stopped","active":true}]"#,
            r#"[{"path":"/tmp/a.rs","line":3,"verified":true},{"path":"/tmp/b.rs","line":1,"verified":false}]"#,
            r#"[{"name":"binary","request":"launch","source":"lang","adapter":"lldb-dap"}]"#,
        );
        eval(
            r#"globalThis.__s = JSON.stringify(helix.dap.sessions());
               globalThis.__b = JSON.stringify(helix.dap.list_breakpoints("/tmp/a.rs"));
               globalThis.__c = JSON.stringify(helix.dap.configs());"#,
        )
        .expect("快照调用不抛");
        let read = |name: &str| {
            crate::state::with_engine(|engine| {
                engine
                    .global_object()
                    .get(JsString::from(name), engine)
                    .expect("global 存在")
                    .as_string()
                    .map(|s| s.to_std_string_escaped())
            })
        };
        let sessions = read("__s").unwrap_or_default();
        assert!(sessions.contains("lldb-dap"), "会话快照: {sessions}");
        let breakpoints = read("__b").unwrap_or_default();
        assert!(breakpoints.contains("a.rs"), "按路径过滤: {breakpoints}");
        assert!(
            !breakpoints.contains("b.rs"),
            "别的文件不该出现: {breakpoints}"
        );
        let configs = read("__c").unwrap_or_default();
        assert!(configs.contains("binary"), "配置快照: {configs}");
        // 清空缓存 → 空数组而不是抛错
        cache_snapshots(&st, "", "", "");
        assert!(eval(r#"helix.dap.sessions().length === 0;"#).is_ok());
    }

    #[test]
    fn resolve_settles_promise_and_is_idempotent() {
        let st = crate::host::state_handle_or_init();
        let _ = take_requests(&st);
        eval(
            r#"globalThis.__got = "pending";
               helix.dap.threads().then((v) => { globalThis.__got = JSON.stringify(v); },
                                        (e) => { globalThis.__got = "err:" + e; });"#,
        )
        .expect("调用不抛");
        let reqs = take_requests(&st);
        assert_eq!(reqs.len(), 1);
        resolve(
            &st,
            reqs[0].id,
            DapOutcome::json(serde_json::json!([{"id":1,"name":"main"}])),
        )
        .expect("兑现不报错");
        let _ = crate::pump_jobs();
        let got = crate::state::with_engine(|engine| {
            engine
                .global_object()
                .get(JsString::from("__got"), engine)
                .ok()
                .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
                .unwrap_or_default()
        });
        assert!(got.contains("main"), "then 应拿到解析后的结果: {got}");
        // 幂等：同一个 id 再兑现一次不报错也不改结果
        resolve(&st, reqs[0].id, DapOutcome::json(serde_json::json!([]))).expect("幂等");
    }
}
