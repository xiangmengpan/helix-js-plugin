//! 插件配置(方案 C:声明式 schema + 合并校验)。
//! JS:helix.define_config(name, schema) / helix.get_config(name) / helix.get_config_docs(name)
//! Rust 侧 build_plugin_configs 合并用户 config.toml 覆盖并校验,结果经 cache_configs 写入。
//! 设计:docs/superpowers/specs/2026-08-15-plugin-config-design.md

use crate::state::{with_config_schemas, with_engine, CONFIGS};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsString, JsValue};

/// 声明插件配置 schema:helix.define_config("filetree", {key: {type, default, doc}})
pub(crate) fn js_define_config(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "define_config: name must be a string",
            )))
        })?;
    if name.is_empty() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "define_config: name must not be empty",
        ))));
    }
    let schema = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if !schema.is_object() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "define_config: schema must be an object",
        ))));
    }
    // schema 对象序列化为 JSON 字符串(经 JSON.stringify)
    let json: String = JsValue::from(schema)
        .to_json(ctx)
        .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|_| "{}".to_string());
    with_config_schemas(|m| {
        m.insert(name, json);
    });
    Ok(JsValue::undefined())
}

/// 读取合并后的配置:helix.get_config(name) → 对象(含默认值);未声明 → null
pub(crate) fn js_get_config(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "get_config: name must be a string",
            )))
        })?;
    let json = CONFIGS
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default();
    if json.is_empty() {
        return Ok(JsValue::null());
    }
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap_or(serde_json::Value::Null);
    let Some(obj) = parsed.as_object() else { return Ok(JsValue::null()) };
    let Some(cfg) = obj.get(&name) else { return Ok(JsValue::null()) };
    // serde_json Value → JsValue(经 JSON.parse)
    let cfg_json = serde_json::to_string(cfg).unwrap_or_else(|_| "null".to_string());
    // JSON 对象字面量直接 eval 会被当语句块;用 JSON.parse 包裹(Rust {:?} 转义与 JS 字符串兼容)
    let code = format!("JSON.parse({cfg_json:?})");
    Ok(ctx
        .eval(boa_engine::Source::from_bytes(code.as_bytes()))
        .unwrap_or(JsValue::null()))
}

/// 配置说明:helix.get_config_docs(name) → [{key, type, default, doc}]
pub(crate) fn js_get_config_docs(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "get_config_docs: name must be a string",
            )))
        })?;
    let schema = with_config_schemas(|m| m.get(&name).cloned());
    let Some(schema) = schema else { return Ok(JsValue::null()) };
    let parsed: serde_json::Value = serde_json::from_str(&schema).unwrap_or(serde_json::Value::Null);
    let Some(obj) = parsed.as_object() else { return Ok(JsValue::null()) };
    let arr = boa_engine::object::builtins::JsArray::new(ctx);
    for (key, field) in obj {
        let Some(fo) = field.as_object() else { continue };
        let to_js = |v: &serde_json::Value| match v {
            serde_json::Value::String(s) => JsValue::from(JsString::from(s.as_str())),
            serde_json::Value::Bool(b) => JsValue::from(*b),
            serde_json::Value::Number(n) => JsValue::from(n.as_f64().unwrap_or(0.0)),
            _ => JsValue::undefined(),
        };
        let get = |k: &str| fo.get(k).map(&to_js).unwrap_or(JsValue::undefined());
        let item = ObjectInitializer::new(ctx)
            .property(JsString::from("key"), JsValue::from(JsString::from(key.as_str())), Attribute::all())
            .property(JsString::from("type"), get("type"), Attribute::all())
            .property(JsString::from("default"), get("default"), Attribute::all())
            .property(JsString::from("doc"), get("doc"), Attribute::all())
            .build();
        let _ = arr.push(item, ctx);
    }
    Ok(JsValue::from(arr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{take_ui_requests, with_engine};
    use boa_engine::Source;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn load_script(script: &str) {
        with_engine(|engine| {
            let _ = engine.eval(Source::from_bytes(script.as_bytes()));
            Ok::<(), anyhow::Error>(())
        })
        .unwrap();
    }

    #[test]
    fn define_and_get_config() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::init();
        // 声明
        load_script(r#"helix.define_config("ft", {show_hidden: {type: "boolean", default: false, doc: "显示隐藏"}});"#);
        // 未合并(缓存空)→ get_config null
        load_script(r#"globalThis.__g = helix.get_config("ft");"#);
        with_engine(|engine| {
            let v = engine.global_object().get(JsString::from("__g"), engine).unwrap();
            assert!(v.is_null() || v.is_undefined(), "未合并 → null/undefined");
        });
        // 合并缓存 → get_config 返回对象
        crate::state::cache_configs(r#"{"ft":{"show_hidden":true}}"#);
        load_script(r#"globalThis.__g2 = helix.get_config("ft");"#);
        with_engine(|engine| {
            let v = engine.global_object().get(JsString::from("__g2"), engine).unwrap();
            let o = v.as_object().unwrap();
            assert_eq!(
                o.get(JsString::from("show_hidden"), engine).unwrap().as_boolean().unwrap(),
                true
            );
        });
        // docs
        load_script(r#"globalThis.__d = helix.get_config_docs("ft");"#);
        with_engine(|engine| {
            let v = engine.global_object().get(JsString::from("__d"), engine).unwrap();
            let arr = v.as_object().unwrap();
            let len = boa_engine::object::builtins::JsArray::from_object(arr.clone()).unwrap().length(engine).unwrap();
            assert_eq!(len, 1, "docs 一项");
        });
        let _ = take_ui_requests();
    }
}
