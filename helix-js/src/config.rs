//! 插件配置(方案 C:声明式 schema + 合并校验)。
//! JS:helix.define_config(name, schema) / helix.get_config(name) / helix.get_config_docs(name)
//! Rust 侧 build_plugin_configs 合并用户 config.toml 覆盖并校验,结果经 cache_configs 写入。
//! 设计:docs/superpowers/specs/2026-08-15-plugin-config-design.md

use crate::state::{config_schemas, plugin_overrides, with_config_schemas, CONFIGS};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsString, JsValue};

/// 按 schema 注册表 + 已缓存的用户覆盖重建合并结果:
/// `(合并 JSON, 校验错误)`。**纯函数**（不写缓存），helix-term 拿到后 `cache_configs`。
///
/// 为什么要它：schema 是**插件脚本执行时**才注册的，而缓存只在启动与 `:config-reload` 构建。
/// 插件 load/reload 之后不重建 ⇒ 新注册的 schema 不在 CONFIGS 里，`get_config` 拿到 null
/// （默认值全丢），reload 后新增的键也没有默认值。
pub fn rebuild_configs() -> (String, Vec<String>) {
    let user_all: serde_json::Value =
        serde_json::from_str(&plugin_overrides()).unwrap_or(serde_json::Value::Null);
    let user_all = user_all.as_object().cloned().unwrap_or_default();
    let mut out = serde_json::Map::new();
    let mut errors: Vec<String> = Vec::new();
    for (name, schema_json) in config_schemas() {
        let Ok(schema) = serde_json::from_str::<serde_json::Value>(&schema_json) else {
            continue;
        };
        let Some(schema_obj) = schema.as_object() else {
            continue;
        };
        // 用户覆盖 [plugins.<name>]（非表 ⇒ 忽略）
        let user = user_all
            .get(&name)
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let mut merged = serde_json::Map::new();
        for (key, field) in schema_obj {
            let Some(fo) = field.as_object() else {
                continue;
            };
            let ftype = fo.get("type").and_then(|t| t.as_str()).unwrap_or("string");
            let default = fo
                .get("default")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let val = match user.get(key) {
                Some(v) => match validate_field(ftype, fo, v) {
                    Ok(()) => v.clone(),
                    Err(e) => {
                        errors.push(format!("{name}.{key}: {e}（用默认值）"));
                        default
                    }
                },
                None => default,
            };
            merged.insert(key.clone(), val);
        }
        out.insert(name, serde_json::Value::Object(merged));
    }
    (serde_json::Value::Object(out).to_string(), errors)
}

/// 字段类型校验：boolean/number/string/enum
pub(crate) fn validate_field(
    ftype: &str,
    field: &serde_json::Map<String, serde_json::Value>,
    v: &serde_json::Value,
) -> Result<(), String> {
    match ftype {
        "boolean" => {
            if v.is_boolean() {
                Ok(())
            } else {
                Err(format!("期望 boolean，得到 {}", v_type(v)))
            }
        }
        "number" => {
            if v.is_number() {
                Ok(())
            } else {
                Err(format!("期望 number，得到 {}", v_type(v)))
            }
        }
        "string" => {
            if v.is_string() {
                Ok(())
            } else {
                Err(format!("期望 string，得到 {}", v_type(v)))
            }
        }
        "enum" => {
            let options: Vec<&str> = field
                .get("options")
                .and_then(|o| o.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                .unwrap_or_default();
            let s = v.as_str().unwrap_or_default();
            if options.contains(&s) {
                Ok(())
            } else {
                Err(format!("期望 enum {options:?}，得到 {s:?}"))
            }
        }
        _ => Ok(()),
    }
}

fn v_type(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Null => "null",
        _ => "other",
    }
}

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
        .map_err(|_| crate::js_err("define_config: name must be a string"))?;
    if name.is_empty() {
        return Err(crate::js_err("define_config: name must not be empty"));
    }
    let schema = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if !schema.is_object() {
        return Err(crate::js_err("define_config: schema must be an object"));
    }
    // schema 对象序列化为 JSON 字符串(经 JSON.stringify)
    let json: String = schema
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
        .map_err(|_| crate::js_err("get_config: name must be a string"))?;
    let json = CONFIGS
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default();
    if json.is_empty() {
        return Ok(JsValue::null());
    }
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap_or(serde_json::Value::Null);
    let Some(obj) = parsed.as_object() else {
        return Ok(JsValue::null());
    };
    let Some(cfg) = obj.get(&name) else {
        return Ok(JsValue::null());
    };
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
        .map_err(|_| crate::js_err("get_config_docs: name must be a string"))?;
    let schema = with_config_schemas(|m| m.get(&name).cloned());
    let Some(schema) = schema else {
        return Ok(JsValue::null());
    };
    let parsed: serde_json::Value =
        serde_json::from_str(&schema).unwrap_or(serde_json::Value::Null);
    let Some(obj) = parsed.as_object() else {
        return Ok(JsValue::null());
    };
    let arr = boa_engine::object::builtins::JsArray::new(ctx)?;
    for (key, field) in obj {
        let Some(fo) = field.as_object() else {
            continue;
        };
        let to_js = |v: &serde_json::Value| match v {
            serde_json::Value::String(s) => JsValue::from(JsString::from(s.as_str())),
            serde_json::Value::Bool(b) => JsValue::from(*b),
            serde_json::Value::Number(n) => JsValue::from(n.as_f64().unwrap_or(0.0)),
            _ => JsValue::undefined(),
        };
        let get = |k: &str| fo.get(k).map(&to_js).unwrap_or(JsValue::undefined());
        let item = ObjectInitializer::new(ctx)
            .property(
                JsString::from("key"),
                JsValue::from(JsString::from(key.as_str())),
                Attribute::all(),
            )
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

    /// rebuild_configs:覆盖只作用于已声明 schema;未声明的段忽略;类型非法 → 报错 + 默认值
    #[test]
    fn rebuild_merges_overrides_over_defaults() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::init();
        load_script(
            r#"helix.define_config("rb", {n: {type: "number", default: 1}, s: {type: "string", default: "a"}});"#,
        );
        crate::cache_plugin_overrides(r#"{"rb":{"n":3},"ghost":{"x":1}}"#);
        let (json, errors) = rebuild_configs();
        assert!(errors.is_empty(), "无错误: {errors:?}");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["rb"]["n"], 3, "用户覆盖生效");
        assert_eq!(v["rb"]["s"], "a", "未覆盖用默认值");
        assert!(v.get("ghost").is_none(), "未声明 schema 的段忽略");

        // 非法类型 → 报错 + 回退默认(不崩)
        crate::cache_plugin_overrides(r#"{"rb":{"n":"oops"}}"#);
        let (json, errors) = rebuild_configs();
        assert_eq!(errors.len(), 1, "类型错误报一条: {errors:?}");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["rb"]["n"], 1, "非法值回退默认");

        // 覆盖源被替换后不再残留旧键
        crate::cache_plugin_overrides("{}");
        let (json, _) = rebuild_configs();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["rb"]["n"], 1, "覆盖清空后只剩默认值");
    }

    #[test]
    fn define_and_get_config() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::init();
        // 声明
        load_script(
            r#"helix.define_config("ft", {show_hidden: {type: "boolean", default: false, doc: "显示隐藏"}});"#,
        );
        // 未合并(缓存空)→ get_config null
        load_script(r#"globalThis.__g = helix.get_config("ft");"#);
        with_engine(|engine| {
            let v = engine
                .global_object()
                .get(JsString::from("__g"), engine)
                .unwrap();
            assert!(v.is_null() || v.is_undefined(), "未合并 → null/undefined");
        });
        // 合并缓存 → get_config 返回对象
        crate::state::cache_configs(r#"{"ft":{"show_hidden":true}}"#);
        load_script(r#"globalThis.__g2 = helix.get_config("ft");"#);
        with_engine(|engine| {
            let v = engine
                .global_object()
                .get(JsString::from("__g2"), engine)
                .unwrap();
            let o = v.as_object().unwrap();
            assert!(o
                .get(JsString::from("show_hidden"), engine)
                .unwrap()
                .as_boolean()
                .unwrap());
        });
        // docs
        load_script(r#"globalThis.__d = helix.get_config_docs("ft");"#);
        with_engine(|engine| {
            let v = engine
                .global_object()
                .get(JsString::from("__d"), engine)
                .unwrap();
            let arr = v.as_object().unwrap();
            let len = boa_engine::object::builtins::JsArray::from_object(arr.clone())
                .unwrap()
                .length(engine)
                .unwrap();
            assert_eq!(len, 1, "docs 一项");
        });
        let _ = take_ui_requests();
    }
}
