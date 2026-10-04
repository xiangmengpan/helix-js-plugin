use boa_engine::{Context, JsError, JsString, JsValue, Source};

use crate::popup::{obj_opt_str, obj_opt_u16};
use crate::state::{BUFFERS, LAST_LAYOUT};

use crate::state::{with_popups, UI_REQUESTS};

use crate::types::*;

pub(crate) fn js_split(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let dir: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    if !["right", "left", "top", "bottom"].contains(&dir.as_str()) {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("helix.split: unknown dir '{dir}'"),
        ))));
    }
    let opts = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .as_object()
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.split: opts object required",
            )))
        })?;
    // 新叶子预分配 id（面板回调注册在 POPUPS 下）
    let id = crate::state::next_terminal_view_id();
    let (kind, cmd, size) =
        if let Some(term) = opts.get(JsString::from("terminal"), ctx)?.as_object() {
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
                p.insert(
                    id,
                    PopupCallbacks {
                        render,
                        on_key: on_key.as_callable().map(|_| on_key),
                        on_close: None,
                    },
                )
            });
            ("panel".to_string(), None, size)
        } else {
            return Err(JsError::from_opaque(JsValue::from(JsString::from(
                "helix.split: opts must have 'terminal' or 'panel'",
            ))));
        };
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::SplitLeaf {
            id,
            dir,
            kind,
            cmd,
            size,
        });
    Ok(JsValue::from(id))
}

pub(crate) fn js_buffer_open(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let path: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    if path.trim().is_empty() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.buffer_open: path must be a non-empty string",
        ))));
    }
    let split = if let Some(opts) = args.get(1).unwrap_or(&JsValue::undefined()).as_object() {
        let s = obj_opt_str(&opts, "split", ctx, "helix.buffer_open")?;
        match s.as_deref() {
            Some("h") | Some("v") => s,
            Some(other) => {
                return Err(JsError::from_opaque(JsValue::from(JsString::from(
                    format!("helix.buffer_open: split must be 'h' or 'v', got '{other}'"),
                ))));
            }
            None => None,
        }
    } else {
        None
    };
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::OpenBufferLeaf { path, split });
    Ok(JsValue::undefined())
}

pub(crate) fn js_close_leaf(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::CloseLeaf { id });
    Ok(JsValue::undefined())
}

pub(crate) fn js_zoom_leaf(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::ZoomLeaf { id });
    Ok(JsValue::undefined())
}

pub(crate) fn js_unzoom(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::Unzoom);
    Ok(JsValue::undefined())
}

pub(crate) fn js_resize_leaf(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    let ratio: f64 = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::ResizeLeaf {
            id,
            ratio: ratio as f32,
        });
    Ok(JsValue::undefined())
}

pub(crate) fn js_focus_leaf(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::FocusLeaf { id });
    Ok(JsValue::undefined())
}

/// helix.layout_resize(id, "h"|"v", delta)：方向感知调整叶子份额。
/// delta>0 增大该叶子（clamp 0.05~0.95）；方向与直接父 Split 不匹配则不生效。
pub(crate) fn js_resize_leaf_dir(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    let dir: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    if dir != "h" && dir != "v" {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "layout_resize: dir must be 'h' or 'v'",
        ))));
    }
    let delta: f64 = args
        .get(2)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::ResizeLeafDir {
            id,
            dir,
            delta: delta as f32,
        });
    Ok(JsValue::undefined())
}

/// helix.layout_swap(id1, id2)：交换两个叶子的内容（组件引用互换）
pub(crate) fn js_swap_leaves(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id1: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    let id2: u64 = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::SwapLeaves { id1, id2 });
    Ok(JsValue::undefined())
}

/// helix.pane.minimize(id, minimized)：最小化/恢复叶子（不占布局，渲染为底部标题横条）
pub(crate) fn js_minimize_leaf(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    let minimized: bool = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::MinimizeLeaf { id, minimized });
    Ok(JsValue::undefined())
}

/// 方向统一解析：left/right/up/down → (dir, first_side)。
fn parse_dir(dir: &str) -> Option<(String, bool)> {
    match dir {
        "left" => Some(("h".into(), true)),
        "right" => Some(("h".into(), false)),
        "up" => Some(("v".into(), true)),
        "down" => Some(("v".into(), false)),
        _ => None,
    }
}

/// helix.pane.focus_dir(id, "left"|"right"|"up"|"down")：聚焦方向邻居
pub(crate) fn js_focus_leaf_dir(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    let dir: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    if parse_dir(&dir).is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "layout_focus: dir must be left/right/up/down",
        ))));
    }
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::FocusLeafDir { id, dir });
    Ok(JsValue::undefined())
}

/// helix.pane.move(id, "left"|...)：与方向邻居交换内容
pub(crate) fn js_swap_leaf_dir(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    let dir: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    let Some(_) = parse_dir(&dir) else {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "layout_swap_dir: dir must be left/right/up/down",
        ))));
    };
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::SwapLeafDir { id, dir });
    Ok(JsValue::undefined())
}

/// helix.pane.equalize(id)：叶子所在 Split 恢复 50/50
pub(crate) fn js_equalize_leaf(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::EqualizeLeaf { id });
    Ok(JsValue::undefined())
}

/// helix.pane.fix(id, fixed)：设置/取消叶子 fixed 标记
/// （fixed 叶子不被 swap/resize/close/minimize/equalize，可被焦点穿过）
pub(crate) fn js_layout_fix(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: f64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "layout_fix: id must be a number",
            )))
        })?;
    if id < 0.0 || id.fract() != 0.0 {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "layout_fix: id must be a non-negative integer",
        ))));
    }
    let fixed: bool = args
        .get(1)
        .cloned()
        .unwrap_or(JsValue::from(false))
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "layout_fix: fixed must be a boolean",
            )))
        })?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::LayoutFix {
            id: id as u64,
            fixed,
        });
    Ok(JsValue::undefined())
}

/// JSON.parse 公共路径（不经 eval 字符串转义——Debug 格式的 \" 双重转义会让 JSON.parse 报错）
pub(crate) fn js_json_parse(
    json: String,
    ctx: &mut Context,
    api: &str,
) -> boa_engine::JsResult<JsValue> {
    let json_global = ctx.global_object().get(JsString::from("JSON"), ctx)?;
    let parse = json_global
        .as_object()
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "{api}: JSON missing"
            ))))
        })?
        .get(JsString::from("parse"), ctx)?;
    let parse = parse.as_callable().ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "{api}: JSON.parse missing"
        ))))
    })?;
    parse
        .call(&json_global, &[JsValue::from(JsString::from(json))], ctx)
        .map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(format!("{api}: {e}")))))
}

/// 读取最近一次布局树序列化（helix-term 树变更时缓存；可能滞后一个操作）
pub(crate) fn js_get_layout(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let json = LAST_LAYOUT
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default();
    if json.is_empty() {
        return Ok(JsValue::null());
    }
    js_json_parse(json, ctx, "get_layout")
}

/// `helix.pane.float(id)` —— 把叶子浮动起来(平铺 → 浮窗)。
/// 命名取自 zellij 插件 API 的 `float_multiple_panes` / `embed_multiple_panes`:
/// 它的词汇是 float / embed,比自造的 "set_place" 更贴用户预期。
pub(crate) fn js_pane_float(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    pane_place_req(args, ctx, "pane.float", true)
}

/// `helix.pane.embed(id)` —— 把浮动的叶子收回平铺(浮窗 → 平铺)。
pub(crate) fn js_pane_embed(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    pane_place_req(args, ctx, "pane.embed", false)
}

/// float/embed 共用的参数解析 + 请求入队
fn pane_place_req(
    args: &[JsValue],
    ctx: &mut Context,
    who: &str,
    float: bool,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    let req = if float {
        UiRequest::PaneFloat { id }
    } else {
        UiRequest::PaneEmbed { id }
    };
    let _ = who;
    UI_REQUESTS.get().unwrap().lock().unwrap().push(req);
    Ok(JsValue::undefined())
}

/// pane 清单:helix.pane.list() → `[{id, place, focused, fixed, pinned, z?, rect?}]`。
/// place 取 `tiled` / `rail` / `float` —— 浮窗在这里也能看到了(之前 get_layout 看不到)。
pub(crate) fn js_pane_list(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let json = crate::state::PANES
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default();
    if json.is_empty() {
        return js_json_parse("{\"panes\":[]}".to_string(), ctx, "pane.list")
            .and_then(|o| o.as_object().unwrap().get(JsString::from("panes"), ctx))
            .map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from("pane.list: 解析失败")))
            });
    }
    js_json_parse(json, ctx, "pane.list")
        .and_then(|o| o.as_object().unwrap().get(JsString::from("panes"), ctx))
        .map_err(|_| JsError::from_opaque(JsValue::from(JsString::from("pane.list: 解析失败"))))
}

/// 当前平级模式:helix.pane_mode.current() → "Normal"|"Locked"|"Pane"|…
pub(crate) fn js_pane_mode_current(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let json = crate::state::PANE_MODE
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default();
    if json.is_empty() {
        return Ok(JsValue::from(JsString::from("Normal")));
    }
    let obj = js_json_parse(json, ctx, "pane_mode.current")?;
    Ok(obj
        .as_object()
        .and_then(|o| o.get(JsString::from("mode"), ctx).ok())
        .unwrap_or_else(|| JsValue::from(JsString::from("Normal"))))
}

/// 当前模式的键位表:helix.pane_mode.keymap() → `{mode, keys:[{key,desc,enabled,reason?}]}`。
/// 条目由 Rust 侧单一来源提供,插件(which-key)不必再硬编码。
pub(crate) fn js_pane_mode_keymap(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let json = crate::state::PANE_MODE
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default();
    if json.is_empty() {
        return js_json_parse(
            "{\"mode\":\"Normal\",\"keys\":[]}".to_string(),
            ctx,
            "pane_mode.keymap",
        );
    }
    js_json_parse(json, ctx, "pane_mode.keymap")
}

/// 组件状态只读:helix.get_component_state(id) → 对象(未注册 → null)。
/// 状态由 Rust 组件经 JSON 字符串提供(register_component_state),JS 视图层据此画外观。
/// 打开文档列表:helix.buffer.list() → [{id, path, name, dirty, language}](只读快照)。
/// BUFFERS JSON 形如 {"current": id, "buffers": [...]}
pub(crate) fn js_buffers(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let json = BUFFERS
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default();
    if json.is_empty() {
        return Ok(JsValue::null());
    }
    let parsed = js_json_parse(json, ctx, "buffers")?;
    let Some(obj) = parsed.as_object() else {
        return Ok(JsValue::null());
    };
    obj.get(JsString::from("buffers"), ctx)
}

/// 当前文档 id:helix.buffer.current()
pub(crate) fn js_current_buffer(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let json = BUFFERS
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default();
    if json.is_empty() {
        return Ok(JsValue::undefined());
    }
    let parsed = js_json_parse(json, ctx, "current_buffer")?;
    let Some(obj) = parsed.as_object() else {
        return Ok(JsValue::undefined());
    };
    obj.get(JsString::from("current"), ctx)
}

/// 聚焦指定文档:helix.buffer.focus(id) — 当前 view 切换到该文档
pub(crate) fn js_focus_buffer(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "focus_buffer: id must be a number",
            )))
        })?;
    crate::state::UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::FocusBuffer { id });
    Ok(JsValue::undefined())
}

pub(crate) fn js_get_component_state(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "get_component_state: id must be a number",
            )))
        })?;
    let Some(json) = crate::state::get_component_state_json(id) else {
        return Ok(JsValue::null());
    };
    js_json_parse(json, ctx, "get_component_state")
}

/// 恢复布局树：序列化传入的布局对象为 JSON，宿主据此重建
pub(crate) fn js_restore_layout(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
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
        .map_err(|e| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "restore_layout: {e}"
            ))))
        })?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::CacheLayout(json));
    Ok(JsValue::undefined())
}
