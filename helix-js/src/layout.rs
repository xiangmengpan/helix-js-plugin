
use boa_engine::{Context, JsError, JsString, JsValue, Source};

use crate::popup::obj_opt_u16;
use crate::state::LAST_LAYOUT;

use crate::state::{
    with_popups, UI_REQUESTS,
};


use crate::types::*;

pub(crate) fn js_split(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let dir: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx)?;
    if !["right", "left", "top", "bottom"].contains(&dir.as_str()) {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
            "helix.split: unknown dir '{dir}'"
        )))));
    }
    let opts = args.get(1).unwrap_or(&JsValue::undefined()).as_object().ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.split: opts object required")))
    })?;
    // 新叶子预分配 id（面板回调注册在 POPUPS 下）
    let id = crate::state::next_terminal_view_id();
    let (kind, cmd, size) = if let Some(term) = opts.get(JsString::from("terminal"), ctx)?.as_object() {
        let cmd: String = term.get(JsString::from("cmd"), ctx)?.try_js_into(ctx)?;
        let size: u16 = obj_opt_u16(&term, "size", ctx, "helix.split")?.unwrap_or(30);
        ("terminal".to_string(), Some(cmd), size)
    } else if let Some(panel) = opts.get(JsString::from("panel"), ctx)?.as_object() {
        let render = panel.get(JsString::from("render"), ctx)?;
        if render.as_callable().is_none() {
            return Err(JsError::from_opaque(JsValue::from(JsString::from(
                "helix.split: panel render must be a function",
            ))));
        }
        let on_key = panel.get(JsString::from("onKey"), ctx)?;
        let size: u16 = obj_opt_u16(&panel, "size", ctx, "helix.split")?.unwrap_or(30);
        with_popups(|p| {
            p.insert(id, PopupCallbacks {
                render,
                on_key: on_key.as_callable().map(|_| on_key),
                on_close: None,
            })
        });
        ("panel".to_string(), None, size)
    } else {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.split: opts must have 'terminal' or 'panel'",
        ))));
    };
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::SplitLeaf { id, dir, kind, cmd, size });
    Ok(JsValue::from(id))
}

pub(crate) fn js_close_leaf(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx)?;
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::CloseLeaf { id });
    Ok(JsValue::undefined())
}

pub(crate) fn js_zoom_leaf(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx)?;
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::ZoomLeaf { id });
    Ok(JsValue::undefined())
}

pub(crate) fn js_unzoom(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::Unzoom);
    Ok(JsValue::undefined())
}

pub(crate) fn js_resize_leaf(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx)?;
    let ratio: f64 = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(ctx)?;
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::ResizeLeaf { id, ratio: ratio as f32 });
    Ok(JsValue::undefined())
}

pub(crate) fn js_focus_leaf(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx)?;
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::FocusLeaf { id });
    Ok(JsValue::undefined())
}

/// 读取最近一次布局树序列化（helix-term 树变更时缓存；可能滞后一个操作）
pub(crate) fn js_get_layout(_this: &JsValue, _args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let json = LAST_LAYOUT.get().map(|m| m.lock().unwrap().clone()).unwrap_or_default();
    if json.is_empty() {
        return Ok(JsValue::null());
    }
    // 用 JSON.parse 解析缓存字符串
    ctx.eval(Source::from_bytes(format!("JSON.parse({json:?})").as_str()))
        .map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(format!("get_layout: {e}")))))
}

/// 恢复布局树：序列化传入的布局对象为 JSON，宿主据此重建
pub(crate) fn js_restore_layout(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let dump = args.first().cloned().unwrap_or(JsValue::undefined());
    if dump.is_null_or_undefined() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.restore_layout: layout object required",
        ))));
    }
    // 用 JSON.stringify 序列化传入对象
    let json: String = ctx
        .eval(Source::from_bytes("JSON.stringify(arguments[0])"))
        .and_then(|v| v.try_js_into::<String>(ctx))
        .map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(format!("restore_layout: {e}")))))?;
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::CacheLayout(json));
    Ok(JsValue::undefined())
}

