//! 诊断标记图标 API：JS 侧 `helix.set_diagnostic_icons` → 覆盖集，helix-term 同步到 gutter 渲染。
//! 统一图标映射见 plugins/icons.js（本 API 是方案 4 的 Rust 接线）。

use std::collections::HashMap;
use std::sync::OnceLock;

use boa_engine::{Context, JsError, JsString, JsValue};

/// 诊断图标覆盖集（severity 名 → 图标字符；空 = 渲染侧默认 ●）
static DIAGNOSTIC_ICONS: OnceLock<std::sync::Mutex<HashMap<String, String>>> = OnceLock::new();

fn icons() -> &'static std::sync::Mutex<HashMap<String, String>> {
    DIAGNOSTIC_ICONS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// 设置诊断标记图标：{ error?, warning?, info?, hint? }（非字符串/空串忽略；整体替换）。
pub(crate) fn js_set_diagnostic_icons(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let api = "helix.set_diagnostic_icons";
    let obj = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .as_object()
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "{api}: expected an object of {{ error, warning, info, hint }}"
            ))))
        })?;
    let mut map = HashMap::new();
    for key in ["error", "warning", "info", "hint"] {
        if let Ok(v) = obj.get(JsString::from(key), ctx) {
            if v.is_string() {
                if let Ok(s) = v.try_js_into::<String>(ctx) {
                    if !s.is_empty() {
                        map.insert(key.to_string(), s);
                    }
                }
            }
        }
    }
    *icons().lock().expect("diag icons lock") = map;
    Ok(JsValue::undefined())
}

/// helix-term 取走当前诊断图标集（render 泵时同步到 gutter；未 set 过返回空表）
pub fn take_diagnostic_icons() -> HashMap<String, String> {
    std::mem::take(&mut *icons().lock().expect("diag icons lock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_icons_roundtrip() {
        crate::init();
        // 空表默认
        assert!(take_diagnostic_icons().is_empty());

        // set 后 take 拿到完整映射；非字符串/空串忽略
        crate::load_script(
            r##"
            helix.set_diagnostic_icons({
                error: "\uf00d",
                warning: "\uf12a",
                info: "\uf129",
                hint: 42,          // 非字符串忽略
                unknown: "x",      // 未知键忽略
            });
            "##,
        )
        .unwrap();
        let icons = take_diagnostic_icons();
        assert_eq!(icons.get("error").map(String::as_str), Some("\u{f00d}"));
        assert_eq!(icons.get("warning").map(String::as_str), Some("\u{f12a}"));
        assert_eq!(icons.get("info").map(String::as_str), Some("\u{f129}"));
        assert_eq!(icons.len(), 3, "非字符串与未知键应忽略: {icons:?}");

        // take 后清空（幂等）
        assert!(take_diagnostic_icons().is_empty(), "take 取走后应为空");

        // 缺参/非对象 → JS 报错
        assert!(crate::load_script(r#"helix.set_diagnostic_icons("x");"#).is_err());
    }
}
