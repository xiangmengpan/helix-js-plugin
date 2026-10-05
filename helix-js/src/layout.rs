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

/// `helix.pane.info(id)` —— 单个 pane 的信息(找不到 → `null`)。
/// **复用 `pane.list()` 的同一份快照**,不新增任何 Rust 侧状态、不等下一帧。
pub(crate) fn js_pane_info(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let want: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.pane.info: id must be a number",
            )))
        })?;
    let arr = js_pane_list(&JsValue::undefined(), &[], ctx)?;
    let Some(arr) = arr.as_object() else {
        return Ok(JsValue::null());
    };
    let len: u64 = arr.get(JsString::from("length"), ctx)?.try_js_into(ctx)?;
    for i in 0..len {
        let item = arr.get(i, ctx)?;
        if let Some(o) = item.as_object() {
            if let Ok(idv) = o.get(JsString::from("id"), ctx) {
                if idv
                    .try_js_into::<u64>(ctx)
                    .map(|id| id == want)
                    .unwrap_or(false)
                {
                    return Ok(item);
                }
            }
        }
    }
    Ok(JsValue::null())
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

// ══════════════════════════════════════════════════════════════
// 布局文件持久化(`helix.layout.save/load/list/delete`,规格 ③)
//
// 纯函数内核 + 薄 JS 包装:内核只吃 `dir`,所以单测能用 tempdir 直接构造,
// 不必碰全局 OnceLock(同 `resolve_in` / `plugin_roots_for` 的做法)。
// ══════════════════════════════════════════════════════════════

/// 名字 → 文件路径。**只允许 `[A-Za-z0-9_-]`** —— 否则 `../x` 能写出目录外。
fn layout_path_in(dir: &std::path::Path, name: &str) -> Result<std::path::PathBuf, String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "非法的布局名 {name:?}:只允许字母/数字/下划线/连字符"
        ));
    }
    Ok(dir.join(format!("{name}.json")))
}

fn layout_save_in(dir: &std::path::Path, name: &str, json: &str) -> Result<(), String> {
    let path = layout_path_in(dir, name)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("建目录 {}: {e}", dir.display()))?;
    std::fs::write(&path, json).map_err(|e| format!("写 {}: {e}", path.display()))
}

fn layout_load_in(dir: &std::path::Path, name: &str) -> Result<String, String> {
    let path = layout_path_in(dir, name)?;
    std::fs::read_to_string(&path).map_err(|e| format!("读 {}: {e}", path.display()))
}

/// 列出已保存的布局名(按字典序;目录不存在 → 空)
fn layout_list_in(dir: &std::path::Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                return None;
            }
            p.file_stem().and_then(|s| s.to_str()).map(str::to_string)
        })
        .collect();
    out.sort();
    out
}

fn layout_delete_in(dir: &std::path::Path, name: &str) -> Result<bool, String> {
    let path = layout_path_in(dir, name)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("删 {}: {e}", path.display())),
    }
}

fn js_err(msg: String) -> JsError {
    JsError::from_opaque(JsValue::from(JsString::from(msg)))
}

fn want_dir() -> Result<&'static std::path::Path, JsError> {
    crate::state::layouts_dir()
        .map(|p| p.as_path())
        .ok_or_else(|| js_err("布局目录未设置(helix-term 启动时应设置)".into()))
}

fn arg_name(args: &[JsValue], ctx: &mut Context, who: &str) -> Result<String, JsError> {
    args.first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| js_err(format!("{who}: name must be a string")))
}

/// `helix.layout.save(name)` —— 把当前布局快照写成 `<config>/layouts/<name>.json`
pub(crate) fn js_layout_save(
    _t: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name = arg_name(args, ctx, "helix.layout.save")?;
    let json = crate::state::layout_json();
    if json.is_empty() {
        return Err(js_err(
            "helix.layout.save: 还没有布局快照(需编辑器先渲染过至少一帧)".into(),
        ));
    }
    layout_save_in(want_dir()?, &name, &json).map_err(js_err)?;
    Ok(JsValue::from(true))
}

/// `helix.layout.load(name)` —— 读文件并应用(与 `restore` 同一条通道)
pub(crate) fn js_layout_load(
    _t: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name = arg_name(args, ctx, "helix.layout.load")?;
    let json = layout_load_in(want_dir()?, &name).map_err(js_err)?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::CacheLayout(json));
    Ok(JsValue::from(true))
}

/// `helix.layout.list()` —— 已保存的布局名
pub(crate) fn js_layout_list(
    _t: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let names = match crate::state::layouts_dir() {
        Some(dir) => layout_list_in(dir),
        None => Vec::new(),
    };
    let arr = boa_engine::object::builtins::JsArray::new(ctx)?;
    for (i, n) in names.iter().enumerate() {
        let _ = arr.set(i, JsValue::from(JsString::from(n.as_str())), false, ctx);
    }
    Ok(arr.into())
}

/// `helix.layout.delete(name)` —— 删除;不存在返回 false
pub(crate) fn js_layout_delete(
    _t: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name = arg_name(args, ctx, "helix.layout.delete")?;
    Ok(JsValue::from(
        layout_delete_in(want_dir()?, &name).map_err(js_err)?,
    ))
}

// ── 对外公开的 Rust 层入口(`:layout` 类型化命令用;JS 侧走上面的 native fn)──

fn dir_or_err() -> Result<&'static std::path::Path, String> {
    crate::state::layouts_dir()
        .map(|p| p.as_path())
        .ok_or_else(|| "布局目录未设置(helix-term 启动时应设置)".to_string())
}

pub fn layout_save(name: &str) -> Result<(), String> {
    let json = crate::state::layout_json();
    if json.is_empty() {
        return Err("还没有布局快照(需编辑器先渲染过至少一帧)".into());
    }
    layout_save_in(dir_or_err()?, name, &json)
}

pub fn layout_load(name: &str) -> Result<(), String> {
    let json = layout_load_in(dir_or_err()?, name)?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::CacheLayout(json));
    Ok(())
}

pub fn layout_list() -> Vec<String> {
    crate::state::layouts_dir()
        .map(|d| layout_list_in(d))
        .unwrap_or_default()
}

pub fn layout_delete(name: &str) -> Result<bool, String> {
    layout_delete_in(dir_or_err()?, name)
}

#[cfg(test)]
mod layout_file_tests {
    use super::*;

    /// save → list → load → delete 全程(用 tempdir,不碰全局)
    #[test]
    fn save_list_load_delete_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert!(layout_list_in(d).is_empty(), "空目录 → 空列表");

        layout_save_in(d, "dev", r#"{"tree":{"type":"leaf","id":0}}"#).unwrap();
        layout_save_in(d, "aaa", r#"{"tree":{"type":"leaf","id":1}}"#).unwrap();
        assert_eq!(layout_list_in(d), vec!["aaa", "dev"], "按字典序");
        assert!(layout_load_in(d, "dev").unwrap().contains("\"id\":0"));

        assert!(layout_delete_in(d, "dev").unwrap(), "删除已存在 → true");
        assert!(!layout_delete_in(d, "dev").unwrap(), "再删 → false");
        assert_eq!(layout_list_in(d), vec!["aaa"]);
    }

    /// 名字校验:路径穿越必须被挡住(否则能写出目录外)
    #[test]
    fn rejects_path_traversal_and_bad_names() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["../escape", "a/b", "", "a b", "a.b", ".."] {
            assert!(
                layout_save_in(dir.path(), bad, "{}").is_err(),
                "应拒绝坏名字: {bad:?}"
            );
        }
        // 合法的都接受
        for ok in ["dev", "my-layout", "layout_1", "A9"] {
            assert!(layout_save_in(dir.path(), ok, "{}").is_ok(), "应接受: {ok}");
        }
    }
}
