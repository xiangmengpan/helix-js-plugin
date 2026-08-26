//! JS 插件主动 LSP 请求 API：`helix.lsp.hover/completion/goto_definition/document_symbols`。
//! 请求入队（LspRequest）+ Promise 注册（id → resolving）；helix-term 泵循环消费请求，
//! 响应经 resolve_lsp 兑现 Promise（响应分发在后续任务实现）。

use boa_engine::object::builtins::JsPromise;
use boa_engine::{Context, JsString, JsValue};
use std::cell::RefCell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspMethod {
    Hover,
    Completion,
    GotoDefinition,
    DocumentSymbols,
}

#[derive(Debug)]
pub struct LspRequest {
    pub id: u64,
    pub method: LspMethod,
    /// 字符坐标覆盖（row, col）；None = 当前光标（helix-term 泵处理时取）
    pub pos: Option<(u16, u16)>,
}

thread_local! {
    static LSP_REQUESTS: RefCell<Vec<LspRequest>> = const { RefCell::new(Vec::new()) };
}

/// 取走并清空待处理请求队列（helix-term 泵循环每帧调用）
pub fn take_lsp_requests() -> Vec<LspRequest> {
    LSP_REQUESTS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// 通用入口：解析可选位置对象 → 入队 → 注册 Promise
fn enqueue_lsp_request(
    method: LspMethod,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let pos = match args.first() {
        Some(obj) if obj.is_object() => {
            let obj = obj.as_object().unwrap();
            let row = crate::popup::opt_u16(&obj.get(JsString::from("row"), context)?, context, "row")?;
            let col = crate::popup::opt_u16(&obj.get(JsString::from("col"), context)?, context, "col")?;
            row.zip(col) // 缺 row/col 之一 → None（走光标）
        }
        _ => None,
    };
    let (promise, resolving) = JsPromise::new_pending(context);
    let id = crate::state::next_async_id();
    crate::state::with_lsp_promises(|m| m.insert(id, resolving));
    LSP_REQUESTS.with(|q| q.borrow_mut().push(LspRequest { id, method, pos }));
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

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::object::ObjectInitializer;
    use boa_engine::property::Attribute;
    use boa_engine::{Context, JsString, JsValue};
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
}
