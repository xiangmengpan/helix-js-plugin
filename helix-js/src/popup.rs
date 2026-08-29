use anyhow::{anyhow, Result};
use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::{JsObject, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsString, JsValue, Source};

use crate::pty;
use crate::shell::spawn_pty_worker;

use crate::state::{
    with_buffer_icon_hook, with_keymap_hint_hook, with_popups, with_statusline_hook, with_terms,
    UI_REQUESTS,
};

use crate::commands::doc_to_js;
use crate::input::InputState;
use crate::types::*;

pub(crate) fn opt_u16(
    v: &JsValue,
    ctx: &mut Context,
    name: &str,
) -> boa_engine::JsResult<Option<u16>> {
    if v.is_null_or_undefined() {
        return Ok(None);
    }
    let n: f64 = v.try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "'{name}' must be a number"
        ))))
    })?;
    if !n.is_finite() || n < 0.0 || n > u16::MAX as f64 || n.fract() != 0.0 {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("'{name}' must be an integer in [0, {}]", u16::MAX),
        ))));
    }
    Ok(Some(n as u16))
}

pub(crate) fn js_open_popup(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let opts = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .as_object()
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "open_popup: options object required",
            )))
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
    // 尺寸/位置在注册前解析：任一非法则整体失败，不产生半注册
    let width = opt_u16(&opts.get(JsString::from("width"), ctx)?, ctx, "width")?;
    let height = opt_u16(&opts.get(JsString::from("height"), ctx)?, ctx, "height")?;
    let position = {
        let v = opts.get(JsString::from("position"), ctx)?;
        if v.is_null_or_undefined() {
            None
        } else {
            let obj = v.as_object().ok_or_else(|| {
                JsError::from_opaque(JsValue::from(JsString::from(
                    "open_popup: 'position' must be an object with row/col",
                )))
            })?;
            let row = opt_u16(&obj.get(JsString::from("row"), ctx)?, ctx, "position.row")?
                .ok_or_else(|| {
                    JsError::from_opaque(JsValue::from(JsString::from(
                        "open_popup: 'position.row' is required",
                    )))
                })?;
            let col = opt_u16(&obj.get(JsString::from("col"), ctx)?, ctx, "position.col")?
                .ok_or_else(|| {
                    JsError::from_opaque(JsValue::from(JsString::from(
                        "open_popup: 'position.col' is required",
                    )))
                })?;
            Some((row, col))
        }
    };

    let id = crate::state::next_popup_id();
    with_popups(|p| {
        p.insert(
            id,
            PopupCallbacks {
                render,
                on_key,
                on_close,
            },
        )
    });
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::OpenPopup {
            id,
            width,
            height,
            position,
        });
    Ok(JsValue::from(id))
}

/// 注销组件视图回调(组件 Drop 时;纯移除,不触发 onClose)。
pub fn unregister_component_render(id: u64) {
    crate::init();
    crate::state::with_popups(|p| {
        p.remove(&id);
    });
}

/// 组件视图回调注册:helix.set_component_render(id, fn)。
/// 不创建弹窗/面板——只把 render 回调挂到共享注册表;
/// Rust 组件(如终端)渲染时经 render_component 调用,JS 视图层画其外观。
pub(crate) fn js_set_component_render(
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
                "set_component_render: id must be a number",
            )))
        })?;
    let render = match args.get(1) {
        Some(v) => v,
        None => &JsValue::undefined(),
    };
    if render.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "set_component_render: render must be a function",
        ))));
    }
    with_popups(|p| {
        p.insert(
            id,
            PopupCallbacks {
                render: render.clone(),
                on_key: None,
                on_close: None,
            },
        )
    });
    Ok(JsValue::undefined())
}

/// open_panel 允许的 side 白名单
const PANEL_SIDES: [&str; 3] = ["right", "left", "bottom"];

/// 侧边面板：校验 side 白名单 / size / render 后注册回调（onKey 可选，同 open_popup），
/// 入队 OpenPanel。id 与弹窗共用 NEXT_POPUP_ID 空间，面板渲染复用 render_popup 同一注册表。
pub(crate) fn js_open_panel(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let opts = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .as_object()
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "open_panel: options object required",
            )))
        })?;
    let side: String = opts
        .get(JsString::from("side"), ctx)?
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "open_panel: 'side' must be a string",
            )))
        })?;
    if !PANEL_SIDES.contains(&side.as_str()) {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("open_panel: unknown side '{side}' (expected right|left|bottom)"),
        ))));
    }
    let render = opts.get(JsString::from("render"), ctx)?;
    if render.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "open_panel: render must be a function",
        ))));
    }
    let on_close = opts.get(JsString::from("onClose"), ctx)?;
    let on_close = on_close.as_callable().map(|_| on_close);
    let on_key = opts.get(JsString::from("onKey"), ctx)?;
    let on_key = on_key.as_callable().map(|_| on_key);
    let size = {
        let v = opts.get(JsString::from("size"), ctx)?;
        let n: f64 = v.try_js_into(ctx).map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "open_panel: 'size' must be a number",
            )))
        })?;
        if !n.is_finite() || n < 1.0 || n > u16::MAX as f64 || n.fract() != 0.0 {
            return Err(JsError::from_opaque(JsValue::from(JsString::from(
                format!("open_panel: 'size' must be an integer in [1, {}]", u16::MAX),
            ))));
        }
        n as u16
    };

    let id = crate::state::next_popup_id();
    crate::state::set_last_panel_id(Some(id));
    crate::state::with_open_panels(|p| p.push(id));
    with_popups(|p| {
        p.insert(
            id,
            PopupCallbacks {
                render,
                on_key,
                on_close,
            },
        )
    });
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::OpenPanel { id, side, size });
    Ok(JsValue::from(id))
}

/// 入队 ClosePanel（id 校验）；JS 侧与 :panel-close 共用
pub(crate) fn js_close_panel(
    _this: &JsValue,
    args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(_ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "close_panel: id must be a number",
            )))
        })?;
    if crate::state::last_panel_id() == Some(id) {
        crate::state::set_last_panel_id(None);
    };
    crate::state::with_open_panels(|p| p.retain(|x| *x != id));
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::ClosePanel { id });
    Ok(JsValue::undefined())
}

/// helix.open_terminal({ cmd, side, size, onExit? })：打开原生终端面板。
/// 校验 cmd/side/size/onExit 后分配 view_id，内部 spawn pty（复用 spawn 机制）：
/// onChunk 是 eval 工厂构造的桥接闭包 → helix.term_feed(view_id, chunk)（经 UiRequest 路由）；
/// onExit 透传用户回调。入队 OpenTerminal 后返回 view_id（= 面板 id，可 move_panel/term_feed）。
/// pty spawn 仅 Unix（与 js_spawn 的 pty 路径同约束）。
pub(crate) fn js_open_terminal(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let opts = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .as_object()
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "open_terminal: options object required",
            )))
        })?;
    let cmd: String = opts
        .get(JsString::from("cmd"), ctx)?
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "open_terminal: 'cmd' must be a string",
            )))
        })?;
    if cmd.is_empty() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "open_terminal: 'cmd' must not be empty",
        ))));
    }
    let side: String = opts
        .get(JsString::from("side"), ctx)?
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "open_terminal: 'side' must be a string",
            )))
        })?;
    if !PANEL_SIDES.contains(&side.as_str()) {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("open_terminal: unknown side '{side}' (expected right|left|bottom)"),
        ))));
    }
    let size = {
        let v = opts.get(JsString::from("size"), ctx)?;
        let n: f64 = v.try_js_into(ctx).map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "open_terminal: 'size' must be a number",
            )))
        })?;
        if !n.is_finite() || n < 1.0 || n > u16::MAX as f64 || n.fract() != 0.0 {
            return Err(JsError::from_opaque(JsValue::from(JsString::from(
                format!(
                    "open_terminal: 'size' must be an integer in [1, {}]",
                    u16::MAX
                ),
            ))));
        }
        n as u16
    };
    let on_exit = opts.get(JsString::from("onExit"), ctx)?;
    let on_exit = on_exit.as_callable().map(|_| on_exit);

    let view_id = crate::state::next_terminal_view_id();
    #[cfg(unix)]
    {
        // 桥接闭包经 eval 工厂构造（与 js_lazy 同款）：捕获 view_id，chunk → helix.term_feed
        let factory = ctx
            .eval(Source::from_bytes(
                "(function(vid) { return function(chunk) { helix.term_feed(vid, chunk); }; })",
            ))
            .map_err(|e| {
                JsError::from_opaque(JsValue::from(JsString::from(format!(
                    "open_terminal: bridge factory: {e}"
                ))))
            })?;
        let factory = factory
            .as_callable()
            .and_then(JsFunction::from_object)
            .ok_or_else(|| {
                JsError::from_opaque(JsValue::from(JsString::from(
                    "open_terminal: internal bridge error",
                )))
            })?;
        let undefined = JsValue::undefined();
        let bridge = factory
            .call(&undefined, &[JsValue::from(view_id)], ctx)
            .map_err(|e| {
                JsError::from_opaque(JsValue::from(JsString::from(format!(
                    "open_terminal: bridge: {e}"
                ))))
            })?;
        // spawn pty（与 js_spawn 的 pty 路径同款）：注册回调/worker/master → 起 worker
        let pty_id = crate::state::next_term_id();
        // 注册表键用 pty_id（term_kill 按 pty_id 清理）；view_id 是 UI 层/JS 侧句柄
        crate::state::register_term(pty_id, view_id, cmd.clone());
        with_terms(|m| {
            m.insert(
                pty_id,
                TermCallbacks {
                    on_chunk: bridge,
                    on_exit,
                },
            )
        });
        let (tx, rx) = std::sync::mpsc::channel();
        crate::state::with_term_workers(|m| m.insert(pty_id, tx));
        let term_tx =
            crate::state::with_term_events(|t| t.clone().expect("TERM_EVENTS initialized"));
        let (master, slave) = pty::open_pty().map_err(|e| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "open_terminal: pty: {e}"
            ))))
        })?;
        crate::state::with_term_masters(|m| m.insert(pty_id, master.fd()));
        spawn_pty_worker(pty_id, &cmd, term_tx, rx, master, slave);
        UI_REQUESTS
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .push(UiRequest::OpenTerminal {
                view_id,
                pty_id,
                cmd,
                side,
                size,
            });
        Ok(JsValue::from(view_id))
    }
    #[cfg(not(unix))]
    {
        let _ = (cmd, side, size, on_exit);
        Err(JsError::from_opaque(JsValue::from(JsString::from(
            "open_terminal: requires a unix platform",
        ))))
    }
}

/// helix.term_feed(view_id, chunk)：把 PTY 输出块入队 TermFeed，由 helix-term 按 view_id
/// 找终端层喂进 vte 网格。层不存在时 helix-term 侧丢弃（feed 早于层 push 的竞态）。
pub(crate) fn js_term_feed(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let view_id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "term_feed: view_id must be a number",
            )))
        })?;
    let chunk: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "term_feed: chunk must be a string",
            )))
        })?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::TermFeed { view_id, chunk });
    Ok(JsValue::undefined())
}
pub(crate) fn js_set_terminal_mode(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let view_id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let mode: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    if !["dock", "fullscreen", "floating", "minimized"].contains(&mode.as_str()) {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("helix.set_terminal_mode: unknown mode '{mode}'"),
        ))));
    }
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::TermMode { view_id, mode });
    Ok(JsValue::undefined())
}

pub(crate) fn js_term_clear(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let view_id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::TermClear { view_id });
    Ok(JsValue::undefined())
}

/// helix.term_save(view_id, path?)：把终端全部内容（scrollback + 屏幕）导出到文件。
/// path 省略/空 → 默认 ~/.cache/helix/term-<view_id>.log（Rust 侧补全）。
pub(crate) fn js_term_save(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let view_id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    let path: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .unwrap_or_default();
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::TermSave { view_id, path });
    Ok(JsValue::undefined())
}

pub(crate) fn js_resize_term(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let view_id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let size: u16 = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::TermResize { view_id, size });
    Ok(JsValue::undefined())
}
/// 布局树 API：split(dir, {terminal:{cmd}} | {panel:{render,onKey}}) -> leaf_id（预分配）
pub(crate) fn js_read_dir(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let path: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "read_dir: path must be a string",
            )))
        })?;
    let mut entries: Vec<(String, bool, String)> = std::fs::read_dir(&path)
        .map_err(|e| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "read_dir('{path}'): {e}"
            ))))
        })?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let full = entry.path().to_string_lossy().into_owned();
            Some((name, is_dir, full))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let arr = JsArray::new(ctx);
    for (name, is_dir, path) in entries {
        let obj = ObjectInitializer::new(ctx)
            .property(
                JsString::from("name"),
                JsValue::from(JsString::from(name)),
                Attribute::all(),
            )
            .property(
                JsString::from("is_dir"),
                JsValue::from(is_dir),
                Attribute::all(),
            )
            .property(
                JsString::from("path"),
                JsValue::from(JsString::from(path)),
                Attribute::all(),
            )
            .build();
        arr.push(JsValue::from(obj), ctx)?;
    }
    Ok(JsValue::from(arr))
}

/// 入队 OpenFile（path 字符串校验；第二参数可选 { row, col } 字符坐标定位）
pub(crate) fn js_open_file(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let path: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "open_file: path must be a string",
            )))
        })?;
    let (row, col) = match args.get(1) {
        Some(obj) if obj.is_object() => {
            let o = obj.as_object().unwrap();
            let row = opt_u16(&o.get(JsString::from("row"), ctx)?, ctx, "row")?;
            let col = opt_u16(&o.get(JsString::from("col"), ctx)?, ctx, "col")?;
            (row, col)
        }
        _ => (None, None),
    };
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::OpenFile { path, row, col });
    Ok(JsValue::undefined())
}

/// 入队 MovePanel（id 数字 + side 白名单校验）
pub(crate) fn js_move_panel(
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
                "move_panel: id must be a number",
            )))
        })?;
    let side: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "move_panel: side must be a string",
            )))
        })?;
    if !PANEL_SIDES.contains(&side.as_str()) {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("move_panel: unknown side '{side}' (expected right|left|bottom)"),
        ))));
    }
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::MovePanel { id, side });
    Ok(JsValue::undefined())
}

pub(crate) fn js_set_buffer_icon(
    _this: &JsValue,
    args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
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

pub(crate) fn js_set_completion_icon(
    _this: &JsValue,
    args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let undefined = JsValue::undefined();
    let hook = args.first().unwrap_or(&undefined);
    if !hook.is_callable() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "set_completion_icon: expected a function",
        ))));
    }
    crate::state::with_completion_icon_hook(|h| *h = Some(hook.clone()));
    Ok(JsValue::undefined())
}

/// 读对象可选字符串字段：null/undefined → None；非字符串 → Err
pub(crate) fn obj_opt_str(
    obj: &JsObject,
    key: &str,
    ctx: &mut Context,
    api: &str,
) -> boa_engine::JsResult<Option<String>> {
    let v = obj.get(JsString::from(key), ctx)?;
    if v.is_null_or_undefined() {
        return Ok(None);
    }
    let s: String = v.try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "{api}: '{key}' must be a string"
        ))))
    })?;
    Ok(Some(s))
}

/// 读对象可选 u16 字段：null/undefined → None；必须是 [0, u16::MAX] 整数
pub(crate) fn obj_opt_u16(
    obj: &JsObject,
    key: &str,
    ctx: &mut Context,
    api: &str,
) -> boa_engine::JsResult<Option<u16>> {
    let v = obj.get(JsString::from(key), ctx)?;
    if v.is_null_or_undefined() {
        return Ok(None);
    }
    let n: f64 = v.try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "{api}: '{key}' must be a number"
        ))))
    })?;
    if !n.is_finite() || n < 0.0 || n > u16::MAX as f64 || n.fract() != 0.0 {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("{api}: '{key}' must be an integer in [0, {}]", u16::MAX),
        ))));
    }
    Ok(Some(n as u16))
}

/// 读对象可选布尔字段：null/undefined → None；非布尔 → Err
pub(crate) fn obj_opt_bool(
    obj: &JsObject,
    key: &str,
    ctx: &mut Context,
    api: &str,
) -> boa_engine::JsResult<Option<bool>> {
    let v = obj.get(JsString::from(key), ctx)?;
    if v.is_null_or_undefined() {
        return Ok(None);
    }
    let b: bool = v.try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "{api}: '{key}' expects a boolean"
        ))))
    })?;
    Ok(Some(b))
}

/// helix.el(type, arg, opts)：构造组件节点数据对象 {type, ...}，实际解析在 render 时递归进行。
/// type 白名单：text（arg=文本字符串，opts={style,width}）/ row、col（arg=子节点数组，opts={gap}）
/// / scroll（arg=子节点数组，opts={height}）。只做浅层校验（子节点对象合法性由 parse_node 递归检查）。
pub(crate) fn js_el(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let api = "helix.el";
    let type_: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "{api}: 'type' must be a string"
            ))))
        })?;
    // 先收集字段再一次性建对象：builder 持有 &mut ctx，中途再借 ctx 会冲突
    let arg = args.get(1).cloned().unwrap_or(JsValue::undefined());
    let opts = args.get(2).filter(|o| !o.is_null_or_undefined());
    let mut props: Vec<(String, JsValue)> =
        vec![("type".into(), JsString::from(type_.clone()).into())];
    match type_.as_str() {
        "text" => {
            // text 可为字符串或富文本段数组（[{text, style}, ...]）
            if arg.try_js_into::<String>(ctx).is_ok() {
                // 字符串直通
            } else if arg
                .try_js_into::<boa_engine::object::builtins::JsArray>(ctx)
                .is_ok()
            {
                // 数组直通（parse_node 解析）
            } else {
                return Err(JsError::from_opaque(JsValue::from(JsString::from(
                    format!("{api}: 'text' expects a string or an array of segments"),
                ))));
            }
            props.push(("text".into(), arg));
            if let Some(opts) = opts {
                let obj = opts.as_object().ok_or_else(|| {
                    JsError::from_opaque(JsValue::from(JsString::from(format!(
                        "{api}: options must be an object"
                    ))))
                })?;
                if let Some(style) = obj_opt_str(&obj, "style", ctx, api)? {
                    props.push(("style".into(), JsString::from(style).into()));
                }
                if let Some(width) = obj_opt_u16(&obj, "width", ctx, api)? {
                    props.push(("width".into(), JsValue::from(width)));
                }
                if let Some(flex) = obj_opt_u16(&obj, "flex", ctx, api)? {
                    props.push(("flex".into(), JsValue::from(flex)));
                }
                if let Some(wrap) = obj_opt_bool(&obj, "wrap", ctx, api)? {
                    props.push(("wrap".into(), JsValue::from(wrap)));
                }
            }
        }
        "button" => {
            // button(label, { id, onPress, style })
            props.push(("text".into(), arg));
            if let Some(opts) = opts {
                let obj = opts.as_object().ok_or_else(|| {
                    JsError::from_opaque(JsValue::from(JsString::from(format!(
                        "{api}: options must be an object"
                    ))))
                })?;
                let id_val = obj.get(JsString::from("id"), ctx)?;
                if id_val.try_js_into::<String>(ctx).is_err() {
                    return Err(JsError::from_opaque(JsValue::from(JsString::from(
                        format!("{api}: 'button' requires an 'id' string"),
                    ))));
                }
                props.push(("id".into(), id_val));
                for key in ["onPress", "onKey", "style", "width", "flex"] {
                    let v = obj.get(JsString::from(key), ctx)?;
                    if !v.is_null_or_undefined() {
                        props.push((key.into(), v));
                    }
                }
            } else {
                return Err(JsError::from_opaque(JsValue::from(JsString::from(
                    format!("{api}: 'button' requires options with 'id'"),
                ))));
            }
        }
        "input" => {
            // input({ id, value, onKey, width })
            let obj = arg.as_object().ok_or_else(|| {
                JsError::from_opaque(JsValue::from(JsString::from(format!(
                    "{api}: 'input' expects an options object"
                ))))
            })?;
            let id_val = obj.get(JsString::from("id"), ctx)?;
            if id_val.try_js_into::<String>(ctx).is_err() {
                return Err(JsError::from_opaque(JsValue::from(JsString::from(
                    format!("{api}: 'input' requires an 'id' string"),
                ))));
            }
            props.push(("id".into(), id_val));
            props.push(("value".into(), obj.get(JsString::from("value"), ctx)?));
            for key in ["onKey", "onPress", "width", "flex"] {
                let v = obj.get(JsString::from(key), ctx)?;
                if !v.is_null_or_undefined() {
                    props.push((key.into(), v));
                }
            }
        }
        "row" | "col" | "scroll" => {
            let _: JsArray = arg.try_js_into(ctx).map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from(format!(
                    "{api}: '{type_}' expects an array of nodes"
                ))))
            })?;
            props.push(("children".into(), arg.clone()));
            if let Some(opts) = opts {
                let obj = opts.as_object().ok_or_else(|| {
                    JsError::from_opaque(JsValue::from(JsString::from(format!(
                        "{api}: options must be an object"
                    ))))
                })?;
                let is_scroll = type_ == "scroll";
                if let Some(v) =
                    obj_opt_u16(&obj, if is_scroll { "height" } else { "gap" }, ctx, api)?
                {
                    let key: &str = if is_scroll { "height" } else { "gap" };
                    props.push((key.into(), JsValue::from(v)));
                }
                if !is_scroll {
                    if let Some(flex) = obj_opt_u16(&obj, "flex", ctx, api)? {
                        props.push(("flex".into(), JsValue::from(flex)));
                    }
                } else if let Some(offset) = obj_opt_u16(&obj, "offset", ctx, api)? {
                    props.push(("offset".into(), JsValue::from(offset)));
                }
            }
        }
        other => {
            return Err(JsError::from_opaque(JsValue::from(JsString::from(
                format!("{api}: unknown type '{other}' (expected text|row|col|scroll)"),
            ))));
        }
    }
    let mut builder = ObjectInitializer::new(ctx);
    for (k, v) in props {
        builder.property(JsString::from(k), v, Attribute::all());
    }
    Ok(builder.build().into())
}

/// 取走并清空 UI 请求队列
/// 解析 render 返回数组的一个元素：对象（含 text 属性）→ StyledLine{text, style}；字符串 → (text, None)；否则 Err
fn parse_line_item(
    item: &JsValue,
    ctx: &mut Context,
    id: u64,
    i: usize,
) -> boa_engine::JsResult<StyledLine> {
    if let Some(obj) = item.as_object() {
        let text: String = obj
            .get(JsString::from("text"), ctx)?
            .try_js_into(ctx)
            .map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from(format!(
                    "popup {id} render line {i}: object must have a string 'text' property"
                ))))
            })?;
        let style = obj.get(JsString::from("style"), ctx)?;
        let style = if style.is_null_or_undefined() {
            None
        } else {
            Some(style.try_js_into::<String>(ctx).map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from(format!(
                    "popup {id} render line {i}: 'style' must be a string"
                ))))
            })?)
        };
        Ok(match style {
            Some(style) => StyledLine::styled(text, style),
            None => StyledLine::plain(text),
        })
    } else if let Ok(text) = item.try_js_into::<String>(ctx) {
        Ok(StyledLine::plain(text))
    } else {
        Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("popup {id} render line {i} must be a string or an object with 'text'"),
        ))))
    }
}

/// 调 JS render 回调，返回内容：数组 → Content::Lines（旧行 API）；单节点对象（含 type）→ Content::Tree。
/// ctx 对象 { width, height }。
/// 通用组件渲染入口:与 render_popup 同一实现(panel/popup/任意组件共用注册表),
/// 命名语义化——JS 视图层渲染任意已注册组件。
pub fn render_component(id: u64, width: u16, height: u16, focus: Option<&str>) -> Result<Content> {
    render_popup(id, width, height, focus)
}

pub fn render_popup(id: u64, width: u16, height: u16, focus: Option<&str>) -> Result<Content> {
    crate::init();
    crate::state::with_engine(|engine| {
        let render = with_popups(|p| p.get(&id).map(|cb| cb.render.clone()))
            .ok_or_else(|| anyhow!("popup {id} not open"))?;
        let ctx_obj = ObjectInitializer::new(engine)
            .property(JsString::from("width"), width, Attribute::all())
            .property(JsString::from("height"), height, Attribute::all())
            .build();
        let func = render
            .as_callable()
            .and_then(JsFunction::from_object)
            .ok_or_else(|| anyhow!("popup {id} render is not a function"))?;
        let undefined = JsValue::undefined();
        // render(focus, ctx)：focus 为当前焦点节点 id（JS 侧据此渲染焦点样式）
        let focus_arg = match focus {
            Some(f) => JsValue::from(JsString::from(f.to_string())),
            None => JsValue::null(),
        };
        let value: JsValue = func
            .call(&undefined, &[focus_arg, JsValue::from(ctx_obj)], engine)
            .map_err(|e| anyhow!("popup {id} render failed: {e}"))?;
        // 数组 → 旧行 API；单对象含 type → 组件树（数组也是对象，数组判断在前）
        if let Ok(arr) = value.try_js_into::<JsArray>(engine) {
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
                lines.push(
                    parse_line_item(&item, engine, id, i)
                        .map_err(|e| anyhow!("popup {id} render failed: {e}"))?,
                );
            }
            Ok(Content::Lines(lines))
        } else if let Some(obj) = value.as_object() {
            let has_type = obj
                .has_own_property(JsString::from("type"), engine)
                .map_err(|e| anyhow!("popup {id} type read failed: {e}"))?;
            if has_type {
                let node = parse_node(&value, engine, id)
                    .map_err(|e| anyhow!("popup {id} render failed: {e}"))?;
                Ok(Content::Tree(node))
            } else {
                Err(anyhow!(
                    "popup {id} render must return an array of strings/styled objects, or a node object with 'type'"
                ))
            }
        } else {
            Err(anyhow!(
                "popup {id} render must return an array of strings/styled objects, or a node object with 'type'"
            ))
        }
    })
}

/// 递归解析节点对象 → CompNode。type 白名单 + 字段校验（text 必需；style/width/gap/height 可选）。
fn parse_node(value: &JsValue, ctx: &mut Context, id: u64) -> boa_engine::JsResult<CompNode> {
    let api = format!("popup {id} render");
    let obj = value.as_object().ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "{api}: node must be an object with 'type'"
        ))))
    })?;
    let type_: String = obj
        .get(JsString::from("type"), ctx)?
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "{api}: node 'type' must be a string"
            ))))
        })?;
    match type_.as_str() {
        "text" => {
            let text_val = obj.get(JsString::from("text"), ctx)?;
            // 节点级 style 属性（字符串文本时整段应用）
            let node_style = obj_opt_str(&obj, "style", ctx, &api)?;
            // text 支持字符串或富文本段数组 [{text, style}, ...]
            let spans = if let Ok(s) = text_val.try_js_into::<String>(ctx) {
                vec![TextSpan {
                    text: s,
                    style: node_style,
                }]
            } else if let Ok(arr) =
                text_val.try_js_into::<boa_engine::object::builtins::JsArray>(ctx)
            {
                let mut spans = Vec::new();
                let len: usize = arr.get(JsString::from("length"), ctx)?.try_js_into(ctx)?;
                for i in 0..len {
                    let item = arr.get(i, ctx)?;
                    let obj = item.as_object().ok_or_else(|| {
                        JsError::from_opaque(JsValue::from(JsString::from(format!(
                            "{api}: rich text segments must be objects with 'text'"
                        ))))
                    })?;
                    let t: String = obj
                        .get(JsString::from("text"), ctx)?
                        .try_js_into(ctx)
                        .map_err(|_| {
                            JsError::from_opaque(JsValue::from(JsString::from(format!(
                                "{api}: rich text segment must have a string 'text'"
                            ))))
                        })?;
                    let st = obj_opt_str(&obj, "style", ctx, &api)?;
                    spans.push(TextSpan { text: t, style: st });
                }
                spans
            } else {
                return Err(JsError::from_opaque(JsValue::from(JsString::from(
                    format!("{api}: text node 'text' must be a string or an array of segments"),
                ))));
            };
            let width = obj_opt_u16(&obj, "width", ctx, &api)?;
            let node_id = obj_opt_str(&obj, "id", ctx, &api)?;
            register_node_handlers(&obj, ctx, id, node_id.as_deref())?;
            let flex = obj_opt_u16(&obj, "flex", ctx, &api)?;
            let wrap = obj_opt_bool(&obj, "wrap", ctx, &api)?.unwrap_or(false);
            Ok(CompNode::Text {
                spans,
                width,
                id: node_id,
                flex,
                wrap,
            })
        }
        "row" | "col" => {
            let children = parse_children(&obj, ctx, id)?;
            let gap = obj_opt_u16(&obj, "gap", ctx, &api)?.unwrap_or(0);
            let flex = obj_opt_u16(&obj, "flex", ctx, &api)?;
            Ok(if type_ == "row" {
                CompNode::Row {
                    children,
                    gap,
                    flex,
                }
            } else {
                CompNode::Col {
                    children,
                    gap,
                    flex,
                }
            })
        }
        "scroll" => {
            let children = parse_children(&obj, ctx, id)?;
            let height = obj_opt_u16(&obj, "height", ctx, &api)?.unwrap_or(0);
            let offset = obj_opt_u16(&obj, "offset", ctx, &api)?;
            Ok(CompNode::Scroll {
                children,
                height,
                offset,
            })
        }
        "button" => {
            let node_id: String = obj
                .get(JsString::from("id"), ctx)?
                .try_js_into(ctx)
                .map_err(|_| {
                    JsError::from_opaque(JsValue::from(JsString::from(format!(
                        "{api}: button node must have a string 'id'"
                    ))))
                })?;
            let label = parse_text_spans(&obj, ctx, &api)?;
            let width = obj_opt_u16(&obj, "width", ctx, &api)?;
            register_node_handlers(&obj, ctx, id, Some(&node_id))?;
            let flex = obj_opt_u16(&obj, "flex", ctx, &api)?;
            Ok(CompNode::Button {
                label,
                width,
                id: node_id,
                flex,
            })
        }
        "input" => {
            let node_id: String = obj
                .get(JsString::from("id"), ctx)?
                .try_js_into(ctx)
                .map_err(|_| {
                    JsError::from_opaque(JsValue::from(JsString::from(format!(
                        "{api}: input node must have a string 'id'"
                    ))))
                })?;
            let js_value: String = obj
                .get(JsString::from("value"), ctx)?
                .try_js_into(ctx)
                .unwrap_or_default();
            let width = obj_opt_u16(&obj, "width", ctx, &api)?;
            register_node_handlers(&obj, ctx, id, Some(&node_id))?;
            let flex = obj_opt_u16(&obj, "flex", ctx, &api)?;
            // 引擎权威：首次渲染用 JS 传值初始化；之后用 InputStates 状态覆盖 JS 传值
            let (value, cursor) =
                crate::input::with_input_states(|m| match m.entry((id, node_id.clone())) {
                    std::collections::hash_map::Entry::Occupied(e) => {
                        let s = e.get();
                        (s.value.clone(), s.cursor)
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        let c = js_value.chars().count();
                        e.insert(InputState {
                            value: js_value.clone(),
                            cursor: c,
                        });
                        (js_value.clone(), c)
                    }
                });
            Ok(CompNode::Input {
                value,
                cursor,
                width,
                id: node_id,
                flex,
            })
        }
        other => Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!(
                "{api}: unknown node type '{other}' (expected text|row|col|scroll|button|input)"
            ),
        )))),
    }
}

/// 解析富文本段数组 [{text, style}, ...] 或字符串 → Vec<TextSpan>
fn parse_text_spans(
    obj: &boa_engine::JsObject,
    ctx: &mut Context,
    api: &str,
) -> boa_engine::JsResult<Vec<TextSpan>> {
    let text_val = obj.get(JsString::from("text"), ctx)?;
    let node_style = obj_opt_str(obj, "style", ctx, api)?;
    if let Ok(s) = text_val.try_js_into::<String>(ctx) {
        Ok(vec![TextSpan {
            text: s,
            style: node_style,
        }])
    } else if let Ok(arr) = text_val.try_js_into::<boa_engine::object::builtins::JsArray>(ctx) {
        let mut spans = Vec::new();
        let len: usize = arr.get(JsString::from("length"), ctx)?.try_js_into(ctx)?;
        for i in 0..len {
            let item = arr.get(i, ctx)?;
            let seg_obj = item.as_object().ok_or_else(|| {
                JsError::from_opaque(JsValue::from(JsString::from(format!(
                    "{api}: rich text segments must be objects with 'text'"
                ))))
            })?;
            let t: String = seg_obj
                .get(JsString::from("text"), ctx)?
                .try_js_into(ctx)
                .map_err(|_| {
                    JsError::from_opaque(JsValue::from(JsString::from(format!(
                        "{api}: rich text segment must have a string 'text'"
                    ))))
                })?;
            let st = obj_opt_str(&seg_obj, "style", ctx, api)?;
            spans.push(TextSpan { text: t, style: st });
        }
        Ok(spans)
    } else {
        Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("{api}: 'text' must be a string or an array of segments"),
        ))))
    }
}

/// 注册节点事件处理器（id + onPress/onKey 存在时）：存 NODE_HANDLERS[(view_id, node_id)]
fn register_node_handlers(
    obj: &boa_engine::JsObject,
    ctx: &mut Context,
    view_id: u64,
    node_id: Option<&str>,
) -> boa_engine::JsResult<()> {
    let Some(node_id) = node_id else {
        return Ok(());
    };
    let on_press = obj.get(JsString::from("onPress"), ctx)?;
    let on_key = obj.get(JsString::from("onKey"), ctx)?;
    let on_change = obj.get(JsString::from("onChange"), ctx)?;
    if on_press.as_callable().is_none()
        && on_key.as_callable().is_none()
        && on_change.as_callable().is_none()
    {
        return Ok(());
    }
    crate::state::with_node_handlers(|map| {
        map.insert(
            (view_id, node_id.to_string()),
            NodeHandlers {
                on_press: on_press.as_callable().map(|_| on_press),
                on_key: on_key.as_callable().map(|_| on_key),
                on_change: on_change.as_callable().map(|_| on_change),
            },
        );
    });
    Ok(())
}

/// 解析容器节点的 children 数组：每项必须是节点对象（递归 parse_node）
fn parse_children(
    obj: &JsObject,
    ctx: &mut Context,
    id: u64,
) -> boa_engine::JsResult<Vec<CompNode>> {
    let api = format!("popup {id} render");
    let v = obj.get(JsString::from("children"), ctx)?;
    let arr: JsArray = v.try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "{api}: container node must have an array 'children'"
        ))))
    })?;
    let len: usize = arr
        .get(JsString::from("length"), ctx)?
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "{api}: children length invalid"
            ))))
        })?;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let item = arr.get(i, ctx).map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "{api}: children[{i}] read failed"
            ))))
        })?;
        out.push(parse_node(&item, ctx, id)?);
    }
    Ok(out)
}

/// 调 JS onKey 回调（第二个参数是可编辑的 doc 快照）。
/// 未注册 onKey 或缺省时：Esc→Close，其他→Ignore。
pub fn popup_key(id: u64, key: &PluginKey, ctx: &CommandContext) -> Result<PopupKeyResult> {
    crate::init();
    crate::state::with_engine(|engine| {
        let callbacks =
            with_popups(|p| p.get(&id).cloned()).ok_or_else(|| anyhow!("popup {id} not open"))?;
        let Some(on_key) = callbacks.on_key else {
            return Ok(if key.name == "Esc" {
                PopupKeyResult::Close
            } else {
                PopupKeyResult::Ignored
            });
        };
        let func = on_key
            .as_callable()
            .and_then(JsFunction::from_object)
            .ok_or_else(|| anyhow!("popup {id} onKey is not a function"))?;
        let key_obj = ObjectInitializer::new(engine)
            .property(
                JsString::from("name"),
                JsString::from(key.name.clone()),
                Attribute::all(),
            )
            .property(JsString::from("shift"), key.shift, Attribute::all())
            .property(JsString::from("ctrl"), key.ctrl, Attribute::all())
            .property(JsString::from("alt"), key.alt, Attribute::all())
            .build();
        let doc = doc_to_js(ctx, engine).map_err(|e| anyhow!("failed to build popup doc: {e}"))?;
        let undefined = JsValue::undefined();
        let value: JsValue = func
            .call(&undefined, &[JsValue::from(key_obj), doc], engine)
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

/// 面板是否注册了 onKey 回调。helix-term 侧据此决定是否把按键交给 popup_key：
/// 无 onKey 的面板缺省全 Ignore（事件穿透），不调 popup_key（其缺省 Esc→Close 语义不适用于面板）。
pub fn panel_has_onkey(id: u64) -> bool {
    crate::init();
    with_popups(|p| p.get(&id).map(|cb| cb.on_key.is_some()).unwrap_or(false))
}

/// 面板/弹窗是否在 JS 注册表（存在但无 onKey 与完全丢失区分：僵尸面板检测用）
pub fn popup_exists(id: u64) -> bool {
    crate::init();
    with_popups(|p| p.contains_key(&id))
}

/// 关闭弹窗：触发 onClose 并移除注册表项。幂等（已关闭返回 Ok）。
pub fn close_popup(id: u64) -> Result<()> {
    crate::init();
    crate::input::clear_popup_inputs(id);
    crate::state::with_engine(|engine| {
        let callbacks = with_popups(|p| p.remove(&id));
        let Some(callbacks) = callbacks else {
            return Ok(());
        };
        if let Some(on_close) = callbacks.on_close {
            let func = on_close
                .as_callable()
                .and_then(JsFunction::from_object)
                .ok_or_else(|| anyhow!("popup {id} onClose is not a function"))?;
            let undefined = JsValue::undefined();
            let _: JsValue = func
                .call(&undefined, &[], engine)
                .map_err(|e| anyhow!("popup {id} onClose failed: {e}"))?;
        }
        Ok(())
    })
}

/// 面板状态清理（僵尸自愈等 Rust 侧路径）：清 JS 注册表 + OPEN_PANELS 列表
pub fn close_panel_state(id: u64) {
    let _ = close_popup(id);
    crate::state::with_open_panels(|p| p.retain(|x| *x != id));
    if crate::state::last_panel_id() == Some(id) {
        crate::state::set_last_panel_id(None);
    }
}

/// 入队关闭最近一次 open_panel 的面板（:panel-close 用）；无面板时 Err
pub fn close_last_panel() -> Result<()> {
    crate::init();
    let id = crate::state::last_panel_id().ok_or_else(|| anyhow!("no panel open"))?;
    crate::state::set_last_panel_id(None);
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::ClosePanel { id });
    Ok(())
}

pub(crate) fn js_set_statusline(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let arg = args.first().cloned().unwrap_or(JsValue::null());
    // 第二参 { replace: true }：整个状态栏由 JS 控制（left/right 分栏）
    let mut replace = false;
    if let Some(opts) = args.get(1).and_then(|o| o.as_object()) {
        if let Ok(v) = opts.get(JsString::from("replace"), context) {
            replace = v.try_js_into::<bool>(context).unwrap_or(false);
        }
        // zones: [左, 中, 右] 区域比例（replace 模式；缺省 1:1:1）
        if let Ok(z) = opts.get(JsString::from("zones"), context) {
            if let Some(arr) = z
                .as_object()
                .and_then(|o| boa_engine::object::builtins::JsArray::from_object(o.clone()).ok())
            {
                let mut zones = [1u16, 1, 1];
                let mut ok = true;
                for (i, slot) in zones.iter_mut().enumerate() {
                    if let Ok(item) = arr.get(i as u64, context) {
                        match item.try_js_into::<f64>(context) {
                            Ok(n) if n >= 0.0 && n <= u16::MAX as f64 && n.fract() == 0.0 => {
                                *slot = n as u16;
                            }
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                }
                if ok {
                    crate::state::set_statusline_zones(zones);
                }
            }
        }
    }
    if arg.is_null_or_undefined() {
        with_statusline_hook(|h| *h = None);
        crate::state::set_statusline_replace(false);
    } else if arg.as_callable().is_some() {
        with_statusline_hook(|h| *h = Some(arg));
        crate::state::set_statusline_replace(replace);
    } else {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.set_statusline: expected a function or null",
        ))));
    }
    Ok(JsValue::undefined())
}

/// 调状态栏钩子；返回分段（每段可带 theme scope）。
/// 回调返回 null/undefined → None；字符串 → 单段；数组 [{text, style?} | "str", ...] → 多段。
/// keymap 前缀提示注册:helix.set_keymap_hint(fn) / 清除:set_keymap_hint(null)。
/// 回调 fn(ctx) → 多行文本(显示提示)| null(不显示);ctx = {title, entries:[{keys, doc}]}。
/// 未注册回调 → Rust 内置 Info 兜底。
pub(crate) fn js_set_keymap_hint(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let arg = args.first().cloned().unwrap_or(JsValue::null());
    with_keymap_hint_hook(|h| *h = if arg.is_null() { None } else { Some(arg) });
    Ok(JsValue::undefined())
}

/// keymap 前缀提示:调用注册回调,返回 (多行文本, 位置字符串)。
/// 回调返回字符串 → 默认位置;对象 {text, position} → 指定位置;null/无回调/抛错 → None(用内置 Info)。
pub fn keymap_hint(title: &str, entries: &[(String, String)]) -> Option<(String, String)> {
    crate::init();
    let hook = with_keymap_hint_hook(|h| h.clone())?;
    let func = hook.as_callable().and_then(JsFunction::from_object)?;
    crate::state::with_engine(|engine| {
        let undefined = JsValue::undefined();
        let entries_arr = JsArray::new(engine);
        for (keys, doc) in entries {
            let item = ObjectInitializer::new(engine)
                .property(
                    JsString::from("keys"),
                    JsValue::from(JsString::from(keys.as_str())),
                    Attribute::all(),
                )
                .property(
                    JsString::from("doc"),
                    JsValue::from(JsString::from(doc.as_str())),
                    Attribute::all(),
                )
                .build();
            let _ = entries_arr.push(item, engine);
        }
        let ctx_obj = ObjectInitializer::new(engine)
            .property(
                JsString::from("title"),
                JsValue::from(JsString::from(title)),
                Attribute::all(),
            )
            .property(
                JsString::from("entries"),
                JsValue::from(entries_arr),
                Attribute::all(),
            )
            .build();
        let Ok(ret) = func.call(&undefined, &[JsValue::from(ctx_obj)], engine) else {
            return None;
        };
        if ret.is_null_or_undefined() {
            return None;
        }
        // 字符串 → 默认位置
        if let Ok(s) = ret.clone().try_js_into::<String>(engine) {
            return Some((s, "bottom-right".to_string()));
        }
        // 对象 {text, position}
        let obj = ret.as_object()?;
        let text: String = obj
            .get(JsString::from("text"), engine)
            .ok()?
            .try_js_into(engine)
            .ok()?;
        let position: String = obj
            .get(JsString::from("position"), engine)
            .ok()
            .and_then(|v| v.try_js_into::<String>(engine).ok())
            .unwrap_or_else(|| "bottom-right".to_string());
        Some((text, position))
    })
}

pub fn statusline_parts(ctx: &StatuslineCtx) -> Option<Vec<StatuslinePart>> {
    crate::init();
    let hook = with_statusline_hook(|h| h.clone())?;
    crate::state::with_engine(|engine| {
        let func = hook.as_callable().and_then(JsFunction::from_object)?;
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
        let ctx_obj = ObjectInitializer::new(engine)
            .property(
                JsString::from("path"),
                match &ctx.path {
                    Some(p) => JsValue::from(JsString::from(p.clone())),
                    None => JsValue::null(),
                },
                Attribute::all(),
            )
            .property(
                JsString::from("mode"),
                JsValue::from(JsString::from(ctx.mode.clone())),
                Attribute::all(),
            )
            .property(
                JsString::from("cursor"),
                JsValue::from(cursor),
                Attribute::all(),
            )
            .property(
                JsString::from("total_lines"),
                JsValue::from(ctx.total_lines as f64),
                Attribute::all(),
            )
            .property(
                JsString::from("diagnostics_error"),
                JsValue::from(ctx.diagnostics_error as f64),
                Attribute::all(),
            )
            .property(
                JsString::from("diagnostics_warning"),
                JsValue::from(ctx.diagnostics_warning as f64),
                Attribute::all(),
            )
            .property(
                JsString::from("window_mode"),
                JsValue::from(ctx.window_mode),
                Attribute::all(),
            )
            .property(
                JsString::from("active_leaf_type"),
                JsValue::from(JsString::from(ctx.active_leaf_type.clone())),
                Attribute::all(),
            )
            .property(
                JsString::from("active_leaf_path"),
                match &ctx.active_leaf_path {
                    Some(p) => JsValue::from(JsString::from(p.clone())),
                    None => JsValue::null(),
                },
                Attribute::all(),
            )
            .build();
        let undefined = JsValue::undefined();
        let value: JsValue = func
            .call(&undefined, &[JsValue::from(ctx_obj)], engine)
            .ok()?;
        if value.is_null_or_undefined() {
            return None;
        }
        // 数组 → 多段（每项：字符串 或 { text, style?, right? }）
        if let Some(obj) = value.as_object() {
            if let Ok(arr) = boa_engine::object::builtins::JsArray::from_object(obj.clone()) {
                let mut parts = Vec::new();
                let len: u64 = arr.length(engine).unwrap_or(0);
                for i in 0..len {
                    if let Ok(item) = arr.get(i, engine) {
                        if let Ok(text) = item.try_js_into::<String>(engine) {
                            parts.push(StatuslinePart {
                                text,
                                style: None,
                                zone: None,
                            });
                        } else if let Some(seg) = item.as_object() {
                            if let Ok(text) = seg
                                .get(JsString::from("text"), engine)
                                .and_then(|v| v.try_js_into::<String>(engine))
                            {
                                let style = seg
                                    .get(JsString::from("style"), engine)
                                    .ok()
                                    .and_then(|v| v.try_js_into::<String>(engine).ok());
                                let zone = seg
                                    .get(JsString::from("zone"), engine)
                                    .ok()
                                    .and_then(|v| v.try_js_into::<String>(engine).ok());
                                // 兼容旧字段 right:true → zone "right"
                                let right = seg
                                    .get(JsString::from("right"), engine)
                                    .ok()
                                    .and_then(|v| v.try_js_into::<bool>(engine).ok())
                                    .unwrap_or(false);
                                let zone = match zone.as_deref() {
                                    Some("center") => Some("center".to_string()),
                                    Some("right") => Some("right".to_string()),
                                    _ if right => Some("right".to_string()),
                                    _ => None,
                                };
                                parts.push(StatuslinePart { text, style, zone });
                            }
                        }
                    }
                }
                return Some(parts);
            }
        }
        // 字符串 → 单段
        match value.try_js_into::<String>(engine) {
            Ok(text) => Some(vec![StatuslinePart {
                text,
                style: None,
                zone: None,
            }]),
            Err(_) => None,
        }
    })
}

/// 当前打开的终端列表：[{ view_id, cmd }]（注册表在 Rust 侧，reload 后仍准确）
pub(crate) fn js_term_list(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let arr = boa_engine::object::builtins::JsArray::new(ctx);
    for (view_id, cmd) in crate::state::list_terms() {
        let item = ObjectInitializer::new(ctx)
            .property(
                JsString::from("view_id"),
                JsValue::from(view_id),
                Attribute::all(),
            )
            .property(
                JsString::from("cmd"),
                JsValue::from(JsString::from(cmd)),
                Attribute::all(),
            )
            .build();
        arr.push(item, ctx).map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from("helix.term_list: push")))
        })?;
    }
    Ok(arr.into())
}

/// 按 view_id 关闭指定终端（UI 请求：helix-term 移除对应叶子并杀 pty）
pub(crate) fn js_term_close(
    _this: &JsValue,
    args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let Some(v) = args.first() else {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.term_close: expected view_id",
        ))));
    };
    let view_id: f64 = v.try_js_into(_ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from(
            "helix.term_close: view_id must be a number",
        )))
    })?;
    if view_id < 0.0 || view_id.fract() != 0.0 {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.term_close: view_id must be a non-negative integer",
        ))));
    }
    crate::state::UI_REQUESTS
        .get()
        .expect("UI_REQUESTS initialized")
        .lock()
        .expect("ui requests lock")
        .push(crate::types::UiRequest::TermClose {
            view_id: view_id as u64,
        });
    Ok(JsValue::undefined())
}

/// 调 bufferline 图标钩子；未注册 / 返回 null / 报错 → None。
pub fn bufferline_icon(path: Option<&str>) -> Option<String> {
    crate::init();
    crate::state::with_engine(|engine| {
        let hook = with_buffer_icon_hook(|h| h.clone());
        let hook = hook?;
        let func = hook.as_callable().and_then(JsFunction::from_object)?;
        let arg = match path {
            Some(p) => JsValue::from(JsString::from(p)),
            None => JsValue::null(),
        };
        let undefined = JsValue::undefined();
        let value: JsValue = func.call(&undefined, &[arg], engine).ok()?;
        value.try_js_into::<String>(engine).ok()
    })
}

/// 调补全 kind 图标钩子；kind 0(非 LSP/未知)短路返回 None；未注册 / 返回空 / 非字符串 / 抛错 → None。
pub fn completion_kind_icon(kind: u8) -> Option<String> {
    if kind == 0 {
        return None;
    }
    crate::init();
    crate::state::with_engine(|engine| {
        let hook = crate::state::with_completion_icon_hook(|h| h.clone());
        let hook = hook?;
        let func = hook.as_callable().and_then(JsFunction::from_object)?;
        let arg = JsValue::from(kind);
        let undefined = JsValue::undefined();
        let value: JsValue = func.call(&undefined, &[arg], engine).ok()?;
        let s: String = value.try_js_into(engine).ok()?;
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn completion_icon_hook() {
        let _guard = crate::tests::TEST_LOCK.lock().unwrap();
        crate::init();
        // 未注册 → None
        assert!(crate::popup::completion_kind_icon(7).is_none());
        // 注册后返回图标；kind 参数透传
        crate::load_script(
            r#"
            helix.set_completion_icon((kind) => "i" + kind);
            "#,
        )
        .unwrap();
        assert_eq!(crate::popup::completion_kind_icon(7).as_deref(), Some("i7"));
        // 返回空串 → None（回退）
        crate::load_script(
            r#"
            helix.set_completion_icon((kind) => kind === 7 ? "" : "x");
            "#,
        )
        .unwrap();
        assert!(crate::popup::completion_kind_icon(7).is_none());
        // 抛错 → None
        crate::load_script(
            r#"
            helix.set_completion_icon((kind) => { throw new Error("boom"); });
            "#,
        )
        .unwrap();
        assert!(crate::popup::completion_kind_icon(7).is_none());
        // 缺参/非函数 → JS 报错
        assert!(crate::load_script(r#"helix.set_completion_icon("x");"#).is_err());
        // kind 0 短路:不调钩子,即使钩子对 0 有返回也回退
        crate::load_script(r#"helix.set_completion_icon((kind) => "z");"#).unwrap();
        assert!(crate::popup::completion_kind_icon(0).is_none());
        // 收尾重注册无害钩子(避免 thread_local 残留抛错钩子影响后续断言)
        crate::load_script(r#"helix.set_completion_icon((kind) => "");"#).unwrap();
    }
}
