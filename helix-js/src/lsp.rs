//! JS 插件主动 LSP 请求 API：`helix.lsp.hover/completion/goto_definition/document_symbols`。
//! 请求入队（LspRequest）+ Promise 注册（id → resolving）；helix-term 泵循环消费请求，
//! 响应经 LSP_RESULTS 通道回泵 → resolve_lsp 按 id 兑现 Promise。

use anyhow::{anyhow, Result};
use boa_engine::object::builtins::JsPromise;
use boa_engine::{Context, JsError, JsNativeError, JsString, JsValue};
use std::cell::RefCell;

use crate::state::WakeSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspMethod {
    Hover,
    Completion,
    GotoDefinition,
    DocumentSymbols,
    /// 全文档格式化(自动应用 TextEdit)
    Format,
    /// 重命名当前符号(自动应用 WorkspaceEdit,newName 走 params)
    Rename,
    /// 列出光标处 code actions(不应用)
    CodeActions,
    /// 执行选中的 code action(action JSON 走 params)
    ExecuteCodeAction,
}

#[derive(Debug)]
pub struct LspRequest {
    pub id: u64,
    pub method: LspMethod,
    /// 字符坐标覆盖（row, col）；None = 当前光标（helix-term 泵处理时取）
    pub pos: Option<(u16, u16)>,
    /// 方法参数(rename 的 new_name 字符串、execute_code_action 的 action JSON;其余 None)
    pub params: Option<serde_json::Value>,
}

thread_local! {
    static LSP_REQUESTS: RefCell<Vec<LspRequest>> = const { RefCell::new(Vec::new()) };
}

/// 取走并清空待处理请求队列（helix-term 泵循环每帧调用）
pub fn take_lsp_requests() -> Vec<LspRequest> {
    LSP_REQUESTS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// 通用入口：解析可选位置对象与各方法专属参数 → 入队 → 注册 Promise
fn enqueue_lsp_request(
    method: LspMethod,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let pos = match args.first() {
        Some(obj) if obj.is_object() => {
            let obj = obj.as_object().unwrap();
            let row =
                crate::popup::opt_u16(&obj.get(JsString::from("row"), context)?, context, "row")?;
            let col =
                crate::popup::opt_u16(&obj.get(JsString::from("col"), context)?, context, "col")?;
            row.zip(col) // 缺 row/col 之一 → None（走光标）
        }
        _ => None,
    };
    // 各方法专属参数(校验失败 → TypeError,不入队)
    let params: Option<serde_json::Value> = match method {
        LspMethod::Rename => {
            let name: String = args
                .first()
                .unwrap_or(&JsValue::undefined())
                .try_js_into(context)
                .map_err(|_| {
                    JsError::from_opaque(JsValue::from(JsString::from(
                        "helix.lsp.rename: newName must be a string",
                    )))
                })?;
            Some(serde_json::Value::String(name))
        }
        LspMethod::ExecuteCodeAction => {
            let obj = args
                .first()
                .unwrap_or(&JsValue::undefined())
                .as_object()
                .ok_or_else(|| {
                    JsError::from_opaque(JsValue::from(JsString::from(
                        "helix.lsp.execute_code_action: action must be an object",
                    )))
                })?;
            Some(
                JsValue::from(obj.clone())
                    .to_json(context)
                    .map_err(|e| {
                        JsError::from_opaque(JsValue::from(JsString::from(format!(
                            "helix.lsp.execute_code_action: {e}"
                        ))))
                    })?
                    .unwrap_or(serde_json::Value::Null),
            )
        }
        _ => None,
    };
    let (promise, resolving) = JsPromise::new_pending(context);
    let id = crate::state::next_async_id();
    crate::state::with_lsp_promises(|m| m.insert(id, resolving));
    LSP_REQUESTS.with(|q| {
        q.borrow_mut().push(LspRequest {
            id,
            method,
            pos,
            params,
        })
    });
    Ok(promise.into())
}

pub(crate) fn js_lsp_hover(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_lsp_request(LspMethod::Hover, args, ctx)
}

pub(crate) fn js_lsp_completion(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_lsp_request(LspMethod::Completion, args, ctx)
}

pub(crate) fn js_lsp_goto_definition(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_lsp_request(LspMethod::GotoDefinition, args, ctx)
}

pub(crate) fn js_lsp_document_symbols(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_lsp_request(LspMethod::DocumentSymbols, args, ctx)
}

pub(crate) fn js_lsp_format(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_lsp_request(LspMethod::Format, args, ctx)
}

pub(crate) fn js_lsp_rename(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_lsp_request(LspMethod::Rename, args, ctx)
}

pub(crate) fn js_lsp_code_actions(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_lsp_request(LspMethod::CodeActions, args, ctx)
}

pub(crate) fn js_lsp_execute_code_action(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    enqueue_lsp_request(LspMethod::ExecuteCodeAction, args, ctx)
}

/// LSP 响应（helix-term 泵回结果用；与 ASYNC_EVENTS 同款 WakeSender 通道）
#[derive(Debug)]
pub struct LspResult {
    pub id: u64,
    pub result: Result<Option<String>, String>,
    /// 需要主线程应用的 LSP 编辑载荷(纯 JSON,跨线程;lsp 类型在 term 侧反序列化)。
    /// 泵循环段 A 先应用再 resolve——promise resolve 时编辑已生效。
    pub apply: Option<LspApply>,
}

/// 主线程应用的编辑载荷:tokio 任务把响应序列化成 JSON,主线程反序列化并应用。
#[derive(Debug)]
pub enum LspApply {
    /// format:TextEdit 列表应用到指定 doc(doc_id 为 DocumentId 的 u64 透传)
    Format {
        doc_id: u64,
        /// 请求侧捕获的 OffsetEncoding 判别值,应用侧不回查(防 server 集变化错位换算)
        offset_encoding: u8,
        edits: serde_json::Value,
    },
    /// rename:WorkspaceEdit(URI 自含,apply_workspace_edit 处理跨 doc/打开)
    WorkspaceEdit(serde_json::Value),
    /// execute code action:完整 CodeActionOrCommand JSON
    ExecuteAction(serde_json::Value),
}

/// 克隆 LSP 响应通道发送端（helix-term tokio 任务发结果用）
pub fn lsp_result_tx() -> WakeSender<LspResult> {
    crate::init();
    crate::state::with_lsp_results(|t| t.clone().expect("LSP_RESULTS initialized"))
}

/// 取走全部待处理 LSP 响应（主线程泵循环每帧调用）
pub fn drain_lsp_results() -> Vec<LspResult> {
    crate::init();
    let mut out = Vec::new();
    crate::state::with_lsp_results_rx(|r| {
        if let Some(rx) = r.as_mut() {
            while let Ok(e) = rx.try_recv() {
                out.push(e);
            }
        }
    });
    out
}

/// 按 id 兑现 LSP promise：Ok(Some(json)) → JSON.parse 后 resolve；Ok(None) → resolve null；
/// Err(e) → reject Error（e.message 可读）。幂等：未知/已兑现 id → no-op。
pub fn resolve_lsp(id: u64, result: Result<Option<String>, String>) -> Result<()> {
    crate::init();
    crate::state::with_engine(|engine| {
        let resolving = crate::state::with_lsp_promises(|m| m.remove(&id));
        let Some(resolving) = resolving else {
            return Ok(());
        };
        let undefined = JsValue::undefined();
        let outcome = match result {
            Ok(Some(json)) => match crate::layout::js_json_parse(json, engine, "lsp result") {
                Ok(parsed) => resolving
                    .resolve
                    .call(&undefined, &[parsed], engine)
                    .map(|_| ()),
                Err(e) => {
                    // 服务端响应畸形 → 走 reject，promise 不能悬空
                    let err = JsNativeError::error()
                        .with_message(e.to_string())
                        .into_opaque(engine);
                    resolving
                        .reject
                        .call(&undefined, &[err.into()], engine)
                        .map(|_| ())
                }
            },
            Ok(None) => resolving
                .resolve
                .call(&undefined, &[JsValue::null()], engine)
                .map(|_| ()),
            Err(e) => {
                let err = JsNativeError::error().with_message(e).into_opaque(engine);
                resolving
                    .reject
                    .call(&undefined, &[err.into()], engine)
                    .map(|_| ())
            }
        };
        outcome.map_err(|e| anyhow!("lsp {id} settle failed: {e}"))
    })
}

/// goto_definition 响应注入 path 便利字段（file:// URI → 文件路径字符串）。
/// 纯函数可单测；Location（uri）与 LocationLink（targetUri，序列化 camelCase）两形态。
pub fn inject_location_paths(v: &mut serde_json::Value) {
    fn inject_one(obj: &mut serde_json::Map<String, serde_json::Value>) {
        let uri = obj.get("uri").or_else(|| obj.get("targetUri"));
        if let Some(serde_json::Value::String(u)) = uri {
            if let Some(path) = file_uri_to_path(u) {
                obj.insert("path".into(), serde_json::Value::String(path));
            }
        }
    }
    match v {
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                if let serde_json::Value::Object(map) = item {
                    inject_one(map);
                }
            }
        }
        serde_json::Value::Object(map) => inject_one(map),
        _ => {}
    }
}

/// file:// URI → 文件路径字符串（剥前缀 + 百分号解码）；非 file:// 或畸形 → None
fn file_uri_to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            out.push(
                (bytes[i + 1] as char).to_digit(16)? as u8 * 16
                    + (bytes[i + 2] as char).to_digit(16)? as u8,
            );
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::object::ObjectInitializer;
    use boa_engine::property::Attribute;
    use boa_engine::{Context, JsString, JsValue, Source};
    use std::sync::Mutex;

    // 与 lib.rs 测试同模式：全局状态（LSP_REQUESTS/LSP_PROMISES）用锁串行化
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 请求入队形状：方法/位置快照/id 自增
    #[test]
    fn lsp_request_enqueue_shape() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        let mut ctx = Context::default();
        // 无参 → pos=None（helix-term 侧取当前光标）
        let p = js_lsp_hover(&JsValue::undefined(), &[], &mut ctx).unwrap();
        assert!(p.is_object(), "hover 应返回 Promise");
        let hover_reqs = take_lsp_requests();
        assert_eq!(hover_reqs.len(), 1);
        assert!(matches!(hover_reqs[0].method, LspMethod::Hover));
        assert!(hover_reqs[0].pos.is_none());
        let hover_id = hover_reqs[0].id;
        // 带位置覆盖
        let pos = ObjectInitializer::new(&mut ctx)
            .property(JsString::from("row"), JsValue::from(5), Attribute::all())
            .property(JsString::from("col"), JsValue::from(3), Attribute::all())
            .build();
        let _ = js_lsp_goto_definition(&JsValue::undefined(), &[pos.into()], &mut ctx).unwrap();
        let goto_reqs = take_lsp_requests();
        assert_eq!(goto_reqs.len(), 1);
        assert!(matches!(goto_reqs[0].method, LspMethod::GotoDefinition));
        assert_eq!(goto_reqs[0].pos, Some((5, 3)));
        // id 自增
        assert_ne!(goto_reqs[0].id, hover_id);
    }

    /// 注册名经全局 helix.lsp.* 路径可达（防注册字符串拼错：拼错则 eval 抛错）
    #[test]
    fn lsp_global_registration_names() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        // 真实 CONTEXT 引擎上 eval 全局路径，四个注册名逐一验证返回 Promise
        crate::state::with_engine(|engine| {
            for expr in [
                "helix.lsp.hover()",
                "helix.lsp.completion()",
                "helix.lsp.goto_definition()",
                "helix.lsp.document_symbols()",
            ] {
                let v = engine
                    .eval(Source::from_bytes(expr))
                    .expect("注册名应存在且可调用");
                assert!(v.is_object(), "{expr} 应返回 Promise");
            }
        });
        let got: Vec<LspMethod> = take_lsp_requests().into_iter().map(|r| r.method).collect();
        assert_eq!(
            got,
            vec![
                LspMethod::Hover,
                LspMethod::Completion,
                LspMethod::GotoDefinition,
                LspMethod::DocumentSymbols
            ]
        );
    }

    /// 响应分发：resolve null / resolve JSON / reject，.then/.catch 经 pump_jobs 兑现
    #[test]
    fn lsp_result_dispatch() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        // 在 CONTEXT 引擎上建 promise（resolve_lsp 用同一引擎兑现）
        let id = crate::state::with_engine(|engine| {
            let p = js_lsp_hover(&JsValue::undefined(), &[], engine).unwrap();
            engine
                .register_global_property(JsString::from("__lsp_p"), p, Attribute::all())
                .unwrap();
            take_lsp_requests()[0].id
        });
        // Ok(None) → null
        crate::load_script("__lsp_p.then((v) => { globalThis.__seen = v; });").unwrap();
        resolve_lsp(id, Ok(None)).unwrap();
        crate::pump_jobs().unwrap();
        crate::state::with_engine(|engine| {
            let seen = engine
                .global_object()
                .get(JsString::from("__seen"), engine)
                .unwrap();
            assert!(seen.is_null(), "Ok(None) 应 resolve null, got {seen:?}");
        });
        // Ok(Some(json)) → JSON.parse 后 resolve
        let id2 = crate::state::with_engine(|engine| {
            let p = js_lsp_hover(&JsValue::undefined(), &[], engine).unwrap();
            engine
                .register_global_property(JsString::from("__lsp_p2"), p, Attribute::all())
                .unwrap();
            take_lsp_requests()[0].id
        });
        crate::load_script("__lsp_p2.then((v) => { globalThis.__seen2 = v; });").unwrap();
        resolve_lsp(id2, Ok(Some(r#"{"a": 1}"#.to_string()))).unwrap();
        crate::pump_jobs().unwrap();
        crate::state::with_engine(|engine| {
            let seen = engine
                .global_object()
                .get(JsString::from("__seen2"), engine)
                .unwrap();
            let a = seen
                .as_object()
                .unwrap()
                .get(JsString::from("a"), engine)
                .unwrap();
            assert_eq!(
                a,
                JsValue::from(1),
                "Ok(Some) 应 resolve 解析后的对象, got {seen:?}"
            );
        });
        // Err(e) → reject Error（.catch 读到 e.message）
        let id3 = crate::state::with_engine(|engine| {
            let p = js_lsp_hover(&JsValue::undefined(), &[], engine).unwrap();
            engine
                .register_global_property(JsString::from("__lsp_p3"), p, Attribute::all())
                .unwrap();
            take_lsp_requests()[0].id
        });
        crate::load_script("__lsp_p3.catch((e) => { globalThis.__seen3 = e.message; });").unwrap();
        resolve_lsp(id3, Err("boom".to_string())).unwrap();
        crate::pump_jobs().unwrap();
        crate::state::with_engine(|engine| {
            let seen = engine
                .global_object()
                .get(JsString::from("__seen3"), engine)
                .unwrap();
            assert_eq!(
                seen,
                JsValue::from(JsString::from("boom")),
                "Err 应 reject Error"
            );
        });
        // 畸形 JSON → reject（不 resolve、不悬空），e.message 非空
        let id4 = crate::state::with_engine(|engine| {
            let p = js_lsp_hover(&JsValue::undefined(), &[], engine).unwrap();
            engine
                .register_global_property(JsString::from("__lsp_p4"), p, Attribute::all())
                .unwrap();
            take_lsp_requests()[0].id
        });
        crate::load_script("__lsp_p4.catch((e) => { globalThis.__seen4 = e.message; });").unwrap();
        resolve_lsp(id4, Ok(Some("not json".to_string()))).unwrap();
        crate::pump_jobs().unwrap();
        crate::state::with_engine(|engine| {
            let seen = engine
                .global_object()
                .get(JsString::from("__seen4"), engine)
                .unwrap();
            assert!(
                seen.is_string() && !seen.as_string().unwrap().is_empty(),
                "畸形 JSON 应 reject 且 e.message 非空, got {seen:?}"
            );
        });
    }

    /// goto_definition 响应注入 path 便利字段（Location / LocationLink 两形态）
    #[test]
    fn location_path_injection() {
        let mut v = serde_json::json!([{
            "uri": "file:///a/b.rs",
            "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 1 } }
        }]);
        inject_location_paths(&mut v);
        assert_eq!(v[0]["path"], "/a/b.rs");
        // LocationLink 形态（真实序列化键 targetUri，camelCase）
        let mut v2 = serde_json::json!([{"targetUri": "file:///c/d.rs", "targetRange": {}, "origin_selection_range": {}}]);
        inject_location_paths(&mut v2);
        assert_eq!(v2[0]["path"], "/c/d.rs");
        // snake_case 旧键不命中（防回归：真实 JSON 是 camelCase）
        let mut v2b = serde_json::json!([{"target_uri": "file:///c/d.rs"}]);
        inject_location_paths(&mut v2b);
        assert!(v2b[0].get("path").is_none());
        // 单对象形态
        let mut v3 = serde_json::json!({"uri": "file:///e/f.rs", "range": {}});
        inject_location_paths(&mut v3);
        assert_eq!(v3["path"], "/e/f.rs");
        // 非 file:// uri → 不加 path
        let mut v4 = serde_json::json!([{"uri": "untitled:foo", "range": {}}]);
        inject_location_paths(&mut v4);
        assert!(v4[0].get("path").is_none());
        // %20 → 空格解码
        let mut v5 = serde_json::json!([{"uri": "file:///a%20b.rs", "range": {}}]);
        inject_location_paths(&mut v5);
        assert_eq!(v5[0]["path"], "/a b.rs");
        // 畸形百分号序列（%zz）→ 整路径 None，不加 path
        let mut v6 = serde_json::json!([{"uri": "file:///a%zzb.rs", "range": {}}]);
        inject_location_paths(&mut v6);
        assert!(v6[0].get("path").is_none());
    }
}
