//! lsp-diagnostics 模块：当前文档诊断详情访问。
//! `helix.diagnostics()` 读 helix-term 每帧写入的 DIAGNOSTICS 缓存（JSON 数组）；
//! `emit_lsp_diagnostics` 在诊断更新时触发 "lsp-diagnostics" 事件（docId, diags）。

use std::cell::RefCell;

use boa_engine::object::builtins::JsFunction;
use boa_engine::{Context, JsValue};

use crate::state::{diagnostics_json, with_engine};

thread_local! {
    /// 诊断变了的文档排队等发（`DiagnosticsDidChange` 在 `helix-view` 侧拿不到 JS 宿主）。
    ///
    /// 为什么排队而不是当场发：`with_engine` 在"已在 JS 调用树内"时会 **panic**
    /// （见 host.rs 里那条断言）⇒ 只允许在确定的边界（`application.render`）里发。
    static PENDING: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// 记一个"该文档诊断变了"（去重；由 helix-term 的诊断 hook 调用）。无宿主时也只是排队。
pub fn queue_diagnostics(doc_id: u64) {
    PENDING.with(|q| {
        let mut q = q.borrow_mut();
        if !q.contains(&doc_id) {
            q.push(doc_id);
        }
    });
}

/// 取走排队的文档（render 每帧调用一次；没人排队时是 O(1)）。
pub fn take_pending_diagnostics() -> Vec<u64> {
    PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// 有插件在听 `lsp-diagnostics` 吗？（没有就不必序列化诊断 —— 那份 JSON 是贵的）
pub fn has_diagnostics_listener() -> bool {
    crate::state::with_event_handlers_current(|h| {
        h.get("lsp-diagnostics")
            .is_some_and(|handlers| !handlers.is_empty())
    })
}

/// 当前文档诊断:helix.diagnostics() → [{line, message, severity, code, source}]
/// （只读快照；helix-term 只在**有人读**时写入（兴趣标记）；无缓存 → null）
pub(crate) fn js_diagnostics(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let json = diagnostics_json();
    if json.is_empty() {
        return Ok(JsValue::null());
    }
    crate::layout_api::js_json_parse(json, ctx, "diagnostics")
}

/// 触发 lsp-diagnostics 事件（诊断更新时由 helix-term 侧调用）：
/// handler 收到 (docId, diags) — diags 为 JSON.parse 后的数组。
/// 无 handler 时零开销返回（不碰 engine）。
pub fn emit_lsp_diagnostics(doc_id: u64, diags_json: &str) {
    crate::init();
    let handlers = crate::state::with_event_handlers_current(|h| h.get("lsp-diagnostics").cloned());
    let Some(handlers) = handlers else { return };
    if handlers.is_empty() {
        return;
    }
    with_engine(|engine| {
        let Ok(diags) =
            crate::layout_api::js_json_parse(diags_json.to_string(), engine, "lsp-diagnostics")
        else {
            return;
        };
        let mut args = vec![JsValue::from(doc_id), diags];
        // B-1:具名 payload 追加为最后一个参数(位置参数不变);diagnostics 字段是原样 JSON
        match serde_json::from_str::<serde_json::Value>(diags_json) {
            Ok(v) => {
                if let Ok(p) = crate::commands::payload_js(
                    engine,
                    &crate::types::EventPayload::LspDiagnostics(
                        crate::types::LspDiagnosticsPayload {
                            hdr: crate::types::BusHeader::new("lsp-diagnostics"),
                            doc_id,
                            diagnostics: v,
                        },
                    ),
                ) {
                    args.push(p);
                }
            }
            // JSON 解析失败同上:不发半个 payload
            Err(_) => return,
        }
        let undefined = JsValue::undefined();
        for handler in &handlers {
            let Some(func) = handler.as_callable().and_then(JsFunction::from_object) else {
                continue;
            };
            let _ = func.call(&undefined, &args, engine);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::{JsString, Source};
    use std::sync::Mutex;

    // 与 lib.rs 测试同模式：全局状态（DIAGNOSTICS/EVENT_HANDLERS）用锁串行化
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// cache_diagnostics → js_diagnostics 解析；无缓存 → null
    #[test]
    fn diagnostics_cache_roundtrip() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut ctx = Context::default();
        // 无缓存 → null
        let v = js_diagnostics(&JsValue::undefined(), &[], &mut ctx).unwrap();
        assert!(v.is_null(), "无缓存 → null");
        // 缓存写入 → JSON.parse 数组
        crate::host::with_state_or_init(|st| {
            crate::state::cache_diagnostics(
                st,
                r#"[{"line":1,"message":"oops","severity":"error","code":1,"source":"rust-analyzer"}]"#,
            )
        });
        let v = js_diagnostics(&JsValue::undefined(), &[], &mut ctx).unwrap();
        let arr =
            boa_engine::object::builtins::JsArray::from_object(v.as_object().unwrap().clone())
                .unwrap();
        let len: usize = arr.length(&mut ctx).unwrap() as usize;
        assert_eq!(len, 1, "诊断数组长度");
        let first = arr.get(0, &mut ctx).unwrap();
        let obj = first.as_object().unwrap();
        let msg = obj.get(JsString::from("message"), &mut ctx).unwrap();
        assert_eq!(msg.as_string().unwrap().to_std_string_escaped(), "oops");
        let sev = obj.get(JsString::from("severity"), &mut ctx).unwrap();
        assert_eq!(sev.as_string().unwrap().to_std_string_escaped(), "error");
        crate::host::with_state_or_init(|st| crate::state::cache_diagnostics(st, ""));
    }

    /// emit_lsp_diagnostics 调 handler（docId + 解析后数组）；无 handler 零开销返回
    #[test]
    fn emit_lsp_diagnostics_calls_handler() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::init();
        // 注册 handler 记录参数
        crate::state::with_engine(|engine| {
            let handler = engine
                .eval(Source::from_bytes(
                    "(docId, diags) => { globalThis.__lsp = [docId, diags.length, diags[0].message, diags[0].severity]; }",
                ))
                .unwrap();
            let args = [JsValue::from(JsString::from("lsp-diagnostics")), handler];
            crate::commands::js_on(&JsValue::undefined(), &args, engine).unwrap();
        });
        emit_lsp_diagnostics(
            5,
            r#"[{"line":2,"message":"type mismatch","severity":"warning","code":null,"source":null}]"#,
        );
        crate::state::with_engine(|engine| {
            let got = engine
                .global_object()
                .get(JsString::from("__lsp"), engine)
                .unwrap();
            let arr = boa_engine::object::builtins::JsArray::from_object(
                got.as_object().unwrap().clone(),
            )
            .unwrap();
            let doc_id: u64 = arr.get(0, engine).unwrap().as_number().unwrap() as u64;
            assert_eq!(doc_id, 5, "docId 透传");
            let len: usize = arr.get(1, engine).unwrap().as_number().unwrap() as usize;
            assert_eq!(len, 1, "diags 数组长度");
            let msg = arr
                .get(2, engine)
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped();
            assert_eq!(msg, "type mismatch");
            let sev = arr
                .get(3, engine)
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped();
            assert_eq!(sev, "warning");
        });
        // 无 handler 时零开销返回（不 panic）
        emit_lsp_diagnostics(5, "[]");
    }
}
