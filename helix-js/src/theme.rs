//! 主题 API：JS 侧 set_theme/reset_theme → 覆盖集 + 脏位，helix-term drain 合并；
//! get_style/current_theme/list_themes → 查询当前主题（helix-term 注册回调）；
//! set_theme_name → 切换基准主题（UI 请求）。

use std::collections::HashMap;
use std::sync::OnceLock;

use boa_engine::{Context, JsError, JsString, JsValue};

use crate::state::{mark_theme_dirty, take_theme_dirty_flag, with_theme_overrides};

/// 一个 scope 的覆盖样式（fg/bg/modifiers 均可省略；省略项继承基准主题）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleOverride {
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub modifiers: Vec<String>,
}

/// 查询到的当前样式（helix-term 从 Theme::get 转换）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleInfo {
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub modifiers: Vec<String>,
}

type StyleQuery = dyn Fn(&str) -> Option<StyleInfo> + Send + Sync;
type ThemeQuery = dyn Fn() -> (String, Vec<String>) + Send + Sync;

/// scope → 当前样式查询回调（helix-term 注册；返回 None 表示 scope 未知）
static STYLE_SOURCE: OnceLock<std::sync::Arc<StyleQuery>> = OnceLock::new();

/// 当前主题名 + 可用主题列表回调（helix-term 注册）
static THEME_SOURCE: OnceLock<std::sync::Arc<ThemeQuery>> = OnceLock::new();

pub fn set_theme_style_cb(f: Box<StyleQuery>) {
    let _ = STYLE_SOURCE.set(std::sync::Arc::from(f));
}

pub fn set_theme_info_cb(f: Box<ThemeQuery>) {
    let _ = THEME_SOURCE.set(std::sync::Arc::from(f));
}

/// 解析覆盖值：字符串 = fg；对象 = { fg?, bg?, modifiers?: [...] }（未知字段忽略）
fn parse_style_value(
    v: &JsValue,
    ctx: &mut Context,
    api: &str,
) -> boa_engine::JsResult<StyleOverride> {
    if v.is_string() {
        return Ok(StyleOverride {
            fg: Some(v.try_js_into::<String>(ctx).map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from(format!(
                    "{api}: invalid color value"
                ))))
            })?),
            ..Default::default()
        });
    }
    let Some(obj) = v.as_object() else {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("{api}: expected a color string or {{ fg, bg, modifiers }} object"),
        ))));
    };
    let mut out = StyleOverride::default();
    for key in ["fg", "bg"] {
        if let Ok(val) = obj.get(JsString::from(key), ctx) {
            if val.is_string() {
                let c: String = val.try_js_into(ctx).map_err(|_| {
                    JsError::from_opaque(JsValue::from(JsString::from(format!(
                        "{api}: '{key}' must be a string"
                    ))))
                })?;
                if key == "fg" {
                    out.fg = Some(c);
                } else {
                    out.bg = Some(c);
                }
            }
        }
    }
    if let Ok(val) = obj.get(JsString::from("modifiers"), ctx) {
        if let Some(arr) = val
            .as_object()
            .and_then(|o| boa_engine::object::builtins::JsArray::from_object(o.clone()).ok())
        {
            let len: u64 = arr.length(ctx).map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from(format!(
                    "{api}: modifiers length"
                ))))
            })?;
            for i in 0..len {
                if let Ok(item) = arr.get(i, ctx) {
                    if let Ok(s) = item.try_js_into::<String>(ctx) {
                        out.modifiers.push(s);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// 设置主题覆盖：scope → 颜色字符串或 { fg, bg, modifiers } 对象，整体替换旧覆盖集并置脏。
/// 空对象等价清空。非字符串/非对象值忽略（不整体报错——部分非法条目不阻断其余覆盖）。
pub(crate) fn js_set_theme(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let api = "helix.set_theme";
    let obj = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .as_object()
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "{api}: expected an object of {{ scope: color|{{fg,bg,modifiers}} }}"
            ))))
        })?;
    let mut overrides = HashMap::new();
    for key in obj.own_property_keys(ctx)? {
        let boa_engine::property::PropertyKey::String(scope) = &key else {
            continue;
        };
        let scope = scope.to_std_string_escaped();
        let value = obj.get(key, ctx)?;
        if let Ok(style) = parse_style_value(&value, ctx, api) {
            overrides.insert(scope, style);
        }
    }
    with_theme_overrides(|o| *o = overrides);
    mark_theme_dirty();
    Ok(JsValue::undefined())
}

/// 清除主题覆盖并置脏（helix-term 下次 drain 还原基准主题）
pub(crate) fn js_reset_theme(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    with_theme_overrides(|o| o.clear());
    mark_theme_dirty();
    Ok(JsValue::undefined())
}

/// Rust 侧清除主题覆盖并置脏（helix-term 切基准主题后调用）
pub fn reset_theme() {
    crate::init();
    with_theme_overrides(|o| o.clear());
    mark_theme_dirty();
}

/// 覆盖集是否自上次 drain 后变化（helix-term 读取并清位）
pub fn take_theme_dirty() -> bool {
    crate::init();
    take_theme_dirty_flag()
}

/// 当前主题覆盖集快照（scope → 覆盖样式；helix-term 合并进基准主题）
pub fn theme_overrides() -> HashMap<String, StyleOverride> {
    crate::init();
    with_theme_overrides(|o| o.clone())
}

/// 当前解析后的样式（fg/bg/modifiers，hex）；scope 未知返回 null。
/// 依赖 helix-term 注册的 STYLE_SOURCE；未注册时返回 null。
pub(crate) fn js_get_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let Some(scope) = args.first().and_then(|v| v.as_string()) else {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.get_style: expected a scope string",
        ))));
    };
    let scope = scope.to_std_string_escaped();
    let Some(src) = STYLE_SOURCE.get() else {
        return Ok(JsValue::null());
    };
    let Some(info) = src(&scope) else {
        return Ok(JsValue::null());
    };
    let fg = match &info.fg {
        Some(c) => JsValue::from(JsString::from(c.clone())),
        None => JsValue::null(),
    };
    let bg = match &info.bg {
        Some(c) => JsValue::from(JsString::from(c.clone())),
        None => JsValue::null(),
    };
    // modifiers 数组
    let mods = boa_engine::object::builtins::JsArray::new(ctx)?;
    for m in &info.modifiers {
        mods.push(JsValue::from(JsString::from(m.clone())), ctx)
            .map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from(
                    "helix.get_style: modifiers push",
                )))
            })?;
    }
    let obj = boa_engine::object::ObjectInitializer::new(ctx)
        .property(
            JsString::from("fg"),
            fg,
            boa_engine::property::Attribute::all(),
        )
        .property(
            JsString::from("bg"),
            bg,
            boa_engine::property::Attribute::all(),
        )
        .property(
            JsString::from("modifiers"),
            mods,
            boa_engine::property::Attribute::all(),
        )
        .build();
    Ok(obj.into())
}

/// 当前主题名 + 可用主题列表（切换器用）；未注册回调时返回 { name: null, themes: [] }。
pub(crate) fn js_theme_info(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let (name, themes) = match THEME_SOURCE.get() {
        Some(src) => src(),
        None => (String::new(), Vec::new()),
    };
    let arr = boa_engine::object::builtins::JsArray::new(ctx)?;
    for t in &themes {
        arr.push(JsValue::from(JsString::from(t.clone())), ctx)
            .map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from("helix.theme_info: push")))
            })?;
    }
    let obj = boa_engine::object::ObjectInitializer::new(ctx)
        .property(
            JsString::from("name"),
            if name.is_empty() {
                JsValue::null()
            } else {
                JsValue::from(JsString::from(name))
            },
            boa_engine::property::Attribute::all(),
        )
        .property(
            JsString::from("themes"),
            arr,
            boa_engine::property::Attribute::all(),
        )
        .build();
    Ok(obj.into())
}

/// 切换基准主题（UI 请求：helix-term 加载 + set_theme + 清覆盖）。空串/非字符串报错。
pub(crate) fn js_set_theme_name(
    _this: &JsValue,
    args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let Some(name) = args.first().and_then(|v| v.as_string()) else {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.set_theme_name: expected a theme name string",
        ))));
    };
    let name = name.to_std_string_escaped();
    if name.is_empty() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.set_theme_name: empty theme name",
        ))));
    }
    crate::state::UI_REQUESTS
        .get()
        .expect("UI_REQUESTS initialized")
        .lock()
        .expect("ui requests lock")
        .push(crate::types::UiRequest::SetTheme { name });
    Ok(JsValue::undefined())
}
