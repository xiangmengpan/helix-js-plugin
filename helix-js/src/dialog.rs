//! `helix.dialog.*` —— **核心提供的文本输入 / 确认弹窗**。
//!
//! 为什么需要（由来）：面板插件各自手写「圆角框 + 自己拼字符」的输入弹窗，
//! 于是行为必然漂移 —— filetree 曾经只能追加/退格、没有粘贴与 IME（它自己在文件头
//! 注明了这条天花板），dapui 另画一套（`draw.rect` + 自绘光标）。核心的 `el("input")`
//! 明明已经把这些做好了（引擎权威值 + 光标 + 方向键 + 粘贴 + 多行），但没人用它
//! —— 因为它藏在 `open_popup` 的树里，每个插件都要重写一遍外壳。
//!
//! 这里把外壳收进核心：JS 只给**标题与回调**，渲染与按键由核心负责。
//!
//! ```js
//! helix.dialog.prompt({ title: "重命名 a.rs", value: "a.rs",
//!                       on_submit: (v) => rename(v), on_cancel: () => {} });
//! helix.dialog.confirm({ title: "删除 /tmp/x ?", on_ok: () => remove() });
//! ```
//!
//! 实现要点（两个真实的坑，改这里前请先读）：
//! * **不走 JS render**：`NativeDialog` 直接给出 `CompNode` 树 —— 少一跳、少一份外壳；
//! * **Enter 归 input 节点**：焦点在单行 input 时核心把 Enter 路由给该节点的 `onKey`
//!   （`js_surface::node_key`），根本不经过弹窗的 onKey。所以这里的 Esc/Enter 由
//!   `PluginPopup::handle_event` 在节点路由**之前**问一次（`native_dialog_key`）。

use boa_engine::{Context, JsResult, JsString, JsValue};

/// 弹窗种类。`Prompt` 有输入行，`Confirm` 只有一行提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    Prompt,
    Confirm,
    /// 单选列表（j/k 移动，Enter 选中）
    SelectOne,
    /// 多选列表（j/k 移动，Space 勾选，Enter 确认）
    SelectMany,
}

impl DialogKind {
    /// 列表类（`select`）：渲染是「可滚动行 + 提示」，没有输入框。
    fn is_select(self) -> bool {
        matches!(self, DialogKind::SelectOne | DialogKind::SelectMany)
    }
    fn multi(self) -> bool {
        matches!(self, DialogKind::SelectMany)
    }
}

/// 核心弹窗的状态：渲染树 + 回调。挂在 `PopupCallbacks` 上（按弹层 id）。
#[derive(Clone)]
pub struct NativeDialog {
    pub title: String,
    pub kind: DialogKind,
    /// 输入框初值（只在建 `InputState` 时用一次；之后值归引擎）
    pub initial: String,
    /// 输入节点的 id（`Prompt` 用；输入状态在 `input_states` 里按 `(弹层 id, 这个 id)` 存）
    pub input_id: String,
    pub width: u16,
    pub height: u16,
    pub on_submit: Option<JsValue>,
    pub on_cancel: Option<JsValue>,
    /// 列表项 `(显示文本, 回调值)`；`Prompt`/`Confirm` 为空
    pub items: Vec<(String, String)>,
    /// 列表初始选中项（下标）
    pub initial_picked: Vec<usize>,
}

impl NativeDialog {
    /// 提示行：两种弹窗都给"怎么用"，与插件里原来那套文案统一。
    fn hint(&self) -> &'static str {
        match self.kind {
            DialogKind::Prompt => "Enter 确认 · Esc 取消 · ←→ 移动 · 支持粘贴",
            DialogKind::Confirm => "Enter 确认 · Esc 取消",
            DialogKind::SelectOne => "j/k 移动 · Enter 选中 · Esc 取消",
            DialogKind::SelectMany => "j/k 移动 · Space 勾选 · Enter 确认 · Esc 取消",
        }
    }
}

/// 列表弹窗的运行期状态（游标 + 勾选集合）。
///
/// 为什么不塞进 `NativeDialog`：那是**声明**（Clone 后每帧重渲染），而游标是**状态**；
/// 混在一起会让每次渲染都复制一份可变数据 —— 与 `input_states` 分开存同一个理由。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SelectState {
    pub cursor: usize,
    pub picked: std::collections::BTreeSet<usize>,
}

/// 按键结果（由 `PluginPopup` 执行：它才知道怎么关层、怎么刷状态栏）
#[derive(Debug, Clone, PartialEq)]
pub enum DialogKey {
    /// 提交（带当前输入值，`Confirm` 为空串）
    Submit(String),
    /// 列表提交（带选中项的回调值；单选取当前游标那一项）
    SubmitMany(Vec<String>),
    /// 取消
    Cancel,
    /// 已消费但不关闭（列表里移动游标 / 勾选）—— 调用方只需重绘
    Nav,
}

/// 弹窗几何：宽度跟标题/提示走，再夹到视口内（与插件里那套 `dialog_box` 同一规则）。
fn geometry(title: &str, kind: DialogKind, want_w: Option<u16>) -> (u16, u16) {
    let (vw, vh) = crate::state::viewport();
    let (vw, vh) = if vw == 0 || vh == 0 {
        (80, 24)
    } else {
        (vw, vh)
    };
    let content_lines = match kind {
        DialogKind::Prompt => 2,  // 输入 + 提示
        DialogKind::Confirm => 1, // 提示
        _ => 8,                   // 列表：7 行滚动区 + 提示
    };
    let need_w = (title.chars().count() as u16 + 4).max(want_w.unwrap_or(48));
    let width = need_w.min(vw.saturating_sub(4)).max(24);
    let height = (content_lines + 2).min(vh.saturating_sub(4)).max(3);
    (width, height)
}

/// 把 `NativeDialog` 渲染成组件树（`el("box")` 等价物，直接构造 Rust 类型）。
///
/// 输入节点的**当前值取自 input_states**（引擎权威）；第一次渲染时用初值初始化。
fn content(
    st: &crate::host::HostState,
    popup_id: u64,
    dlg: &NativeDialog,
) -> crate::types::Content {
    use crate::types::CompNode;

    // 列表弹窗：走"可滚动行 + 提示"那条分支
    if dlg.kind.is_select() {
        return select_content(st, popup_id, dlg);
    }

    let mut children: Vec<CompNode> = Vec::new();
    if dlg.kind == DialogKind::Prompt {
        let (value, cursor) = crate::input::ensure_state(st, popup_id, &dlg.input_id, &dlg.initial);
        children.push(CompNode::Input {
            value,
            cursor,
            width: Some(dlg.width.saturating_sub(4)),
            id: dlg.input_id.clone(),
            flex: None,
            multiline: false,
        });
    }
    children.push(CompNode::Text {
        spans: vec![crate::types::TextSpan {
            text: dlg.hint().to_string(),
            style: Some("ui.help".to_string()),
        }],
        width: None,
        id: None,
        flex: None,
        wrap: false,
    });
    crate::types::Content::Tree(CompNode::Box {
        children,
        border: crate::draw::BorderKind::Round,
        title: vec![crate::types::TextSpan {
            text: format!(" {} ", dlg.title),
            style: None,
        }],
        style: None,
        width: Some(dlg.width),
        height: Some(dlg.height),
        flex: None,
    })
}

/// `helix.dialog.prompt({ title, value?, width?, on_submit, on_cancel? })`
pub(crate) fn js_dialog_prompt(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    open(args, ctx, DialogKind::Prompt)
}

/// `helix.dialog.confirm({ title, width?, on_ok, on_cancel? })`
pub(crate) fn js_dialog_confirm(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    open(args, ctx, DialogKind::Confirm)
}

/// `helix.dialog.select({ title, items, multi?, initial?, width?, on_pick, on_cancel? })`
///
/// `items`：字符串数组，或 `{label, value}` 数组（`value` 缺省 = `label`）。
/// `on_pick`：单选收到 `value`，多选收到 `value[]`。
pub(crate) fn js_dialog_select(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    use boa_engine::object::builtins::JsArray;

    let api = "dialog.select";
    let obj = args
        .first()
        .and_then(|v| v.as_object())
        .ok_or_else(|| crate::js_err(format!("{api}: 需要 {{ title, items, on_pick }}")))?;
    let get =
        |key: &str, ctx: &mut Context| -> JsResult<JsValue> { obj.get(JsString::from(key), ctx) };
    let title: String = match get("title", ctx)? {
        v if v.is_null_or_undefined() => String::new(),
        v => v
            .try_js_into(ctx)
            .map_err(|_| crate::js_err(format!("{api}: title 必须是字符串")))?,
    };
    let multi: bool = get("multi", ctx)?.as_boolean().unwrap_or(false);

    // items：字符串 或 {label, value}
    let items_val = get("items", ctx)?;
    let arr: JsArray = items_val
        .try_js_into(ctx)
        .map_err(|_| crate::js_err(format!("{api}: items 必须是数组")))?;
    let len: usize = arr
        .get(JsString::from("length"), ctx)?
        .try_js_into(ctx)
        .unwrap_or(0);
    let mut items: Vec<(String, String)> = Vec::with_capacity(len);
    for i in 0..len {
        let item = arr.get(i, ctx)?;
        if let Some(o) = item.as_object() {
            let label: String = o
                .get(JsString::from("label"), ctx)
                .ok()
                .and_then(|v| v.try_js_into::<String>(ctx).ok())
                .ok_or_else(|| crate::js_err(format!("{api}: items[{i}] 缺少 label")))?;
            let value: String = o
                .get(JsString::from("value"), ctx)
                .ok()
                .and_then(|v| v.try_js_into::<String>(ctx).ok())
                .unwrap_or_else(|| label.clone());
            items.push((label, value));
        } else {
            let label: String = item.try_js_into(ctx).map_err(|_| {
                crate::js_err(format!("{api}: items[{i}] 必须是字符串或 {{label}}"))
            })?;
            items.push((label.clone(), label));
        }
    }

    // initial：已选中的 value 列表（多选）；单选取第一个
    let mut initial_picked: Vec<usize> = Vec::new();
    if let Some(o) = get("initial", ctx)?.as_object() {
        let ilen: usize = o
            .get(JsString::from("length"), ctx)
            .ok()
            .and_then(|v| v.try_js_into::<usize>(ctx).ok())
            .unwrap_or(0);
        let mut want: Vec<String> = Vec::new();
        for i in 0..ilen {
            if let Ok(v) = o.get(i, ctx).and_then(|v| v.try_js_into::<String>(ctx)) {
                want.push(v);
            }
        }
        for (idx, (_, value)) in items.iter().enumerate() {
            if want.iter().any(|w| w == value) {
                initial_picked.push(idx);
            }
        }
    }

    let width_want: Option<u16> = get("width", ctx)?.try_js_into(ctx).ok();
    let widest = items
        .iter()
        .map(|(l, _)| l.chars().count() as u16 + 6)
        .max()
        .unwrap_or(0);
    let kind = if multi {
        DialogKind::SelectMany
    } else {
        DialogKind::SelectOne
    };
    let (width, height) = geometry(&title, kind, Some(widest).filter(|w| *w > 0).or(width_want));
    let height = (height as usize + items.len().min(8)) as u16; // 列表更高：条目多给点行
    let height = height.max(6);

    let on_pick = get("on_pick", ctx)?;
    let on_cancel = get("on_cancel", ctx)?;
    let id = crate::state::next_popup_id(crate::host::host_state(ctx));
    let dlg = NativeDialog {
        title: title.clone(),
        kind,
        initial: String::new(),
        input_id: String::new(),
        width,
        height,
        on_submit: on_pick.as_callable().map(|_| on_pick),
        on_cancel: on_cancel.as_callable().map(|_| on_cancel),
        items,
        initial_picked,
    };
    crate::popup::register_native_dialog(ctx, id, dlg);
    Ok(JsValue::from(id))
}

fn open(args: &[JsValue], ctx: &mut Context, kind: DialogKind) -> JsResult<JsValue> {
    let api = match kind {
        DialogKind::Prompt => "dialog.prompt",
        DialogKind::Confirm => "dialog.confirm",
        _ => "dialog.select",
    };
    let obj = args
        .first()
        .and_then(|v| v.as_object())
        .ok_or_else(|| crate::js_err(format!("{api}: 需要 {{ title, on_submit|on_ok, … }}")))?;

    let get =
        |key: &str, ctx: &mut Context| -> JsResult<JsValue> { obj.get(JsString::from(key), ctx) };
    let title: String = match get("title", ctx)? {
        v if v.is_null_or_undefined() => String::new(),
        v => v
            .try_js_into(ctx)
            .map_err(|_| crate::js_err(format!("{api}: title 必须是字符串")))?,
    };
    let initial: String = match get("value", ctx)? {
        v if v.is_null_or_undefined() => String::new(),
        v => v
            .try_js_into(ctx)
            .map_err(|_| crate::js_err(format!("{api}: value 必须是字符串")))?,
    };
    let want_w: Option<u16> = get("width", ctx)?.try_js_into(ctx).ok();
    // 回调名：prompt 用 on_submit，confirm 用 on_ok，列表用 on_pick（都接受对方的名字，少踩一次坑）
    let submit_val = match kind {
        DialogKind::Prompt => get("on_submit", ctx)?,
        DialogKind::Confirm => get("on_ok", ctx)?,
        _ => get("on_pick", ctx)?,
    };
    let submit_val = if submit_val.as_callable().is_none() {
        let alt = if kind == DialogKind::Prompt {
            "on_ok"
        } else {
            "on_submit"
        };
        get(alt, ctx)?
    } else {
        submit_val
    };
    let on_cancel = get("on_cancel", ctx)?;

    let (width, height) = geometry(&title, kind, want_w);
    let id = crate::state::next_popup_id(crate::host::host_state(ctx));
    let dlg = NativeDialog {
        title: title.clone(),
        kind,
        initial: initial.clone(),
        input_id: "helix-dialog-input".to_string(),
        width,
        height,
        on_submit: submit_val.as_callable().map(|_| submit_val),
        on_cancel: on_cancel.as_callable().map(|_| on_cancel),
        items: Vec::new(),
        initial_picked: Vec::new(),
    };
    crate::popup::register_native_dialog(ctx, id, dlg);
    Ok(JsValue::from(id))
}

/// 渲染入口（`PluginPopup::render` 先问这里；`None` ⇒ 走 JS render）。
pub fn native_dialog_content(
    st: &crate::host::HostState,
    popup_id: u64,
) -> Option<crate::types::Content> {
    let dlg = crate::state::with_popups(st, |p| {
        p.get(&popup_id)
            .and_then(|cb| cb.native.as_ref().map(NativeDialog::clone_shallow))
    })?;
    Some(content(st, popup_id, &dlg))
}

/// 当前登记的核心弹窗 id（调试用）
pub fn native_dialog_ids(st: &crate::host::HostState) -> Vec<u64> {
    crate::state::with_popups(st, |p| {
        p.iter()
            .filter(|(_, cb)| cb.native.is_some())
            .map(|(id, _)| *id)
            .collect()
    })
}

/// `native_dialog_content` 的**环境宿主**版本（布局期 `required_size` 拿不到 Context）。
pub fn native_dialog_content_env(popup_id: u64) -> Option<crate::types::Content> {
    let st = crate::host::state_handle_or_init();
    native_dialog_content(&st, popup_id)
}

/// 按键入口：`Esc` = 取消、`Enter` = 提交（**在节点路由之前**问，见模块注释）。
///
/// 返回 `Some(DialogKey)` 时调用方负责：调回调 → 关层。
pub fn native_dialog_key(
    st: &crate::host::HostState,
    popup_id: u64,
    key: &str,
) -> Option<(DialogKey, Option<JsValue>, Option<JsValue>)> {
    let (kind, input_id, submit, cancel, items) = crate::state::with_popups(st, |p| {
        p.get(&popup_id).and_then(|cb| {
            cb.native.as_ref().map(|n| {
                (
                    n.kind,
                    n.input_id.clone(),
                    n.on_submit.clone(),
                    n.on_cancel.clone(),
                    n.items.clone(),
                )
            })
        })
    })?;

    if kind.is_select() {
        let len = items.len();
        let mut sel = select_state(st, popup_id, len, &[]);
        match key {
            "Esc" => return Some((DialogKey::Cancel, submit, cancel)),
            "j" | "Down" => {
                if len > 0 {
                    sel.cursor = (sel.cursor + 1).min(len - 1);
                }
                set_select_state(st, popup_id, sel);
                return Some((DialogKey::Nav, submit, cancel));
            }
            "k" | "Up" => {
                sel.cursor = sel.cursor.saturating_sub(1);
                set_select_state(st, popup_id, sel);
                return Some((DialogKey::Nav, submit, cancel));
            }
            "g" => {
                sel.cursor = 0;
                set_select_state(st, popup_id, sel);
                return Some((DialogKey::Nav, submit, cancel));
            }
            "G" => {
                sel.cursor = len.saturating_sub(1);
                set_select_state(st, popup_id, sel);
                return Some((DialogKey::Nav, submit, cancel));
            }
            // 多选：Space 勾选当前项（不关闭）
            "Space" | " " if kind.multi() => {
                if sel.picked.contains(&sel.cursor) {
                    sel.picked.remove(&sel.cursor);
                } else {
                    sel.picked.insert(sel.cursor);
                }
                set_select_state(st, popup_id, sel);
                return Some((DialogKey::Nav, submit, cancel));
            }
            "Enter" => {
                let values: Vec<String> = if kind.multi() {
                    let picks = if sel.picked.is_empty() {
                        // 一个都没勾 = 空选择（与 dapui 原语义一致：全部关闭）
                        Vec::new()
                    } else {
                        sel.picked.iter().copied().collect()
                    };
                    picks
                        .into_iter()
                        .filter_map(|i| items.get(i).map(|(_, v)| v.clone()))
                        .collect()
                } else {
                    items
                        .get(sel.cursor)
                        .map(|(_, v)| vec![v.clone()])
                        .unwrap_or_default()
                };
                return Some((DialogKey::SubmitMany(values), submit, cancel));
            }
            _ => return Some((DialogKey::Nav, submit, cancel)),
        }
    }

    match key {
        "Esc" => Some((DialogKey::Cancel, submit, cancel)),
        "Enter" => {
            let value = match kind {
                DialogKind::Prompt => {
                    crate::input::state_value(st, popup_id, &input_id).unwrap_or_default()
                }
                _ => String::new(),
            };
            Some((DialogKey::Submit(value), submit, cancel))
        }
        _ => None,
    }
}

/// 取（或初始化）列表弹窗的状态：游标 0、勾选 = 声明时的初值
pub(crate) fn select_state(
    st: &crate::host::HostState,
    popup_id: u64,
    len: usize,
    initial: &[usize],
) -> SelectState {
    let mut m = st.dialog_select.borrow_mut();
    m.entry(popup_id).or_insert_with(|| SelectState {
        cursor: 0,
        picked: initial.iter().copied().filter(|i| *i < len).collect(),
    });
    m.get(&popup_id).cloned().unwrap_or_default()
}

fn set_select_state(st: &crate::host::HostState, popup_id: u64, state: SelectState) {
    st.dialog_select.borrow_mut().insert(popup_id, state);
}

/// 列表弹窗的渲染：一组行 + 提示。行样式沿用面板惯例（`ui.selection` = 游标）。
fn select_content(
    st: &crate::host::HostState,
    popup_id: u64,
    dlg: &NativeDialog,
) -> crate::types::Content {
    use crate::types::{CompNode, TextSpan};

    let len = dlg.items.len();
    let sel = select_state(st, popup_id, len, &dlg.initial_picked);
    // 内容区 = 外框高 - 边框(2) - 提示(1)
    let body = (dlg.height as usize).saturating_sub(3).max(1);
    let top = sel
        .cursor
        .saturating_sub(body / 2)
        .min(len.saturating_sub(body));
    let mut rows = Vec::new();
    for i in top..(top + body).min(len) {
        let (label, _) = &dlg.items[i];
        let marker = if i == sel.cursor { "▸ " } else { "  " };
        let check = if dlg.kind.multi() {
            if sel.picked.contains(&i) {
                "[x] "
            } else {
                "[ ] "
            }
        } else {
            ""
        };
        rows.push(CompNode::Text {
            spans: vec![TextSpan {
                text: format!("{marker}{check}{label}"),
                style: Some(if i == sel.cursor {
                    "ui.selection".to_string()
                } else {
                    "ui.text".to_string()
                }),
            }],
            width: None,
            id: None,
            flex: None,
            wrap: false,
        });
    }
    let hint = if dlg.kind.multi() {
        "Space 勾选 · Enter 确认 · Esc 取消"
    } else {
        "Enter 选中 · Esc 取消"
    };
    crate::types::Content::Tree(CompNode::Box {
        children: vec![
            CompNode::Scroll {
                children: rows,
                height: body as u16,
                offset: None,
            },
            CompNode::Text {
                spans: vec![TextSpan {
                    text: hint.to_string(),
                    style: Some("ui.help".to_string()),
                }],
                width: None,
                id: None,
                flex: None,
                wrap: false,
            },
        ],
        border: crate::draw::BorderKind::Round,
        title: vec![TextSpan {
            text: format!(" {} ", dlg.title),
            style: None,
        }],
        style: None,
        width: Some(dlg.width),
        height: Some(dlg.height),
        flex: None,
    })
}

/// 调回调：字符串参数（prompt/confirm）
pub fn call_dialog_callback(cb: &JsValue, arg: &str) -> Result<(), String> {
    call_with(cb, vec![JsValue::from(JsString::from(arg))])
}

/// 调回调：数组参数（列表弹窗，单选取值也按数组交回，插件侧保持一致）。
///
/// ⚠ 造数组与调用必须在**同一次 `with_engine`** 里完成 —— 分成两层会撞上
/// "engine used again while already borrowed"（实测踩过）。
pub fn call_dialog_callback_values(cb: &JsValue, values: &[String]) -> Result<(), String> {
    use boa_engine::object::builtins::{JsArray, JsFunction};
    let Some(func) = cb.as_callable().and_then(JsFunction::from_object) else {
        return Ok(());
    };
    let vals: Vec<String> = values.to_vec();
    crate::state::with_engine(|engine| {
        let arr = JsArray::new(engine).map_err(|e| format!("dialog: 造数组失败: {e}"))?;
        for v in vals {
            let _ = arr.push(JsValue::from(JsString::from(v)), engine);
        }
        let undefined = JsValue::undefined();
        func.call(&undefined, &[JsValue::from(arr)], engine)
            .map(|_| ())
            .map_err(|e| format!("dialog 回调失败: {e}"))
    })
}

fn call_with(cb: &JsValue, args: Vec<JsValue>) -> Result<(), String> {
    use boa_engine::object::builtins::JsFunction;
    let Some(func) = cb.as_callable().and_then(JsFunction::from_object) else {
        return Ok(());
    };
    crate::state::with_engine(|engine| {
        let undefined = JsValue::undefined();
        func.call(&undefined, &args, engine)
            .map(|_| ())
            .map_err(|e| format!("dialog 回调失败: {e}"))
    })
}

/// `NativeDialog` 的浅拷贝（回调是 `JsValue`，克隆即引用，GC 不负责这里）
impl NativeDialog {
    fn clone_shallow(&self) -> NativeDialog {
        NativeDialog {
            title: self.title.clone(),
            kind: self.kind,
            input_id: self.input_id.clone(),
            initial: self.initial.clone(),
            width: self.width,
            height: self.height,
            on_submit: self.on_submit.clone(),
            on_cancel: self.on_cancel.clone(),
            items: self.items.clone(),
            initial_picked: self.initial_picked.clone(),
        }
    }
}
