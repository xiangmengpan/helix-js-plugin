//! 插件配置(方案 C:声明式 schema + 合并校验)。
//! JS:helix.define_config(name, schema) / helix.get_config(name) / helix.get_config_docs(name)
//! Rust 侧 build_plugin_configs 合并用户 config.toml 覆盖并校验,结果经 cache_configs 写入。
//! 设计:docs/superpowers/specs/2026-08-15-plugin-config-design.md

use crate::host::host_state;
use crate::host::HostState;
use crate::state::{config_schemas, configs, plugin_overrides, with_config_schemas};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsString, JsValue};

/// 按 schema 注册表 + 已缓存的用户覆盖重建合并结果:
/// `(合并 JSON, 校验错误)`。**纯函数**（不写缓存），helix-term 拿到后 `cache_configs`。
///
/// 未知 **key** 会进 `errors`(拼错的键否则静默回退默认值);未知**段**不报 ——
/// 该插件可能只是还没 `helix.load`(懒加载),报了就是噪音。`[plugins]` 顶层非表项
/// (全局插件设置,如 `plugin_dir`)同样忽略。
///
/// 为什么要它：schema 是**插件脚本执行时**才注册的，而缓存只在启动与 `:config-reload` 构建。
/// 插件 load/reload 之后不重建 ⇒ 新注册的 schema 不在 CONFIGS 里，`get_config` 拿到 null
/// （默认值全丢），reload 后新增的键也没有默认值。
pub fn rebuild_configs(st: &HostState) -> (String, Vec<String>) {
    let user_all: serde_json::Value =
        serde_json::from_str(&plugin_overrides(st)).unwrap_or(serde_json::Value::Null);
    let user_all = user_all.as_object().cloned().unwrap_or_default();
    let mut out = serde_json::Map::new();
    let mut errors: Vec<String> = Vec::new();
    for (name, schema_json) in config_schemas(st) {
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
        // 拼错的键必须可见:静默忽略 = 用户以为配置生效了(默认值静悄悄在用)
        for key in user.keys() {
            if !schema_obj.contains_key(key) {
                errors.push(format!("{name}.{key}: 未在 define_config 中声明（已忽略）"));
            }
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
    let st = host_state(ctx);
    with_config_schemas(st, |m| {
        m.insert(name, json);
    });
    Ok(JsValue::undefined())
}

/// 读取合并后的配置:helix.get_config(name) → 对象(含默认值);未声明 → null
///
/// **自愈**:缓存里没有 `name` 但 schema 里有时就地重建一次。这条是给"脚本**顶层**读配置"用的 ——
/// 插件加载时先 `define_config` 再 `const CFG = helix.get_config(...)`,而宿主的重建发生在这整段
/// 脚本**跑完之后**,不就地补一手拿到的就是上一轮的缓存(null)。
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
    let st = host_state(ctx);
    let mut cfg = cached_config(st, &name);
    if cfg.is_none() && with_config_schemas(st, |m| m.contains_key(&name)) {
        // 校验错误这次先丢(宿主在脚本跑完后还会再重建一次并显示)
        let (json, _) = rebuild_configs(st);
        crate::state::cache_configs(st, &json);
        cfg = cached_config(st, &name);
    }
    let Some(cfg) = cfg else {
        return Ok(JsValue::null());
    };
    // serde_json Value → JsValue(经 JSON.parse)
    let cfg_json = serde_json::to_string(&cfg).unwrap_or_else(|_| "null".to_string());
    // JSON 对象字面量直接 eval 会被当语句块;用 JSON.parse 包裹(Rust {:?} 转义与 JS 字符串兼容)
    let code = format!("JSON.parse({cfg_json:?})");
    Ok(ctx
        .eval(boa_engine::Source::from_bytes(code.as_bytes()))
        .unwrap_or(JsValue::null()))
}

/// 从合并缓存里取某个插件的配置(缓存空 / 没有该名字 ⇒ None)
fn cached_config(st: &HostState, name: &str) -> Option<serde_json::Value> {
    let json = configs(st);
    if json.is_empty() {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(&json).ok()?;
    parsed.as_object()?.get(name).cloned()
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
    let schema = with_config_schemas(host_state(ctx), |m| m.get(&name).cloned());
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

    /// 配置状态是按宿主的 ⇒ 单测统一用默认宿主的那份
    fn st() -> std::rc::Rc<crate::host::HostState> {
        crate::host::state_handle_or_init()
    }

    fn load_script(script: &str) {
        with_engine(|engine| {
            let _ = engine.eval(Source::from_bytes(script.as_bytes()));
            Ok::<(), anyhow::Error>(())
        })
        .unwrap();
    }

    /// 插件**脚本顶层**读配置(模块级 `const CFG = helix.get_config(...)`)必须拿到
    /// 默认值 + 用户覆盖 —— 宿主的重建发生在这整段脚本跑完之后,不就地自愈就只能读到 null。
    #[test]
    fn get_config_at_script_top_level_sees_overrides() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::init();
        crate::state::cache_plugin_overrides(&st(), r#"{"top":{"flag":false}}"#);
        // 一段脚本:先声明、紧接着在**同一段顶层代码**里读(正是插件文件的写法)
        load_script(
            r#"helix.define_config("top", {flag: {type: "boolean", default: true}});
globalThis.__top = helix.get_config("top");"#,
        );
        with_engine(|engine| {
            let v = engine
                .global_object()
                .get(JsString::from("__top"), engine)
                .unwrap();
            let o = v.as_object().expect("顶层读应拿到对象");
            assert!(
                !o.get(JsString::from("flag"), engine)
                    .unwrap()
                    .as_boolean()
                    .unwrap(),
                "用户覆盖 false 应生效(默认是 true)"
            );
        });
        crate::state::cache_plugin_overrides(&st(), "{}");
    }

    /// rebuild_configs:覆盖只作用于已声明 schema;未声明的段忽略;类型非法 → 报错 + 默认值
    #[test]
    fn rebuild_merges_overrides_over_defaults() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::init();
        load_script(
            r#"helix.define_config("rb", {n: {type: "number", default: 1}, s: {type: "string", default: "a"}});"#,
        );
        crate::state::cache_plugin_overrides(&st(), r#"{"rb":{"n":3},"ghost":{"x":1}}"#);
        let (json, errors) = rebuild_configs(&st());
        assert!(errors.is_empty(), "无错误: {errors:?}");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["rb"]["n"], 3, "用户覆盖生效");
        assert_eq!(v["rb"]["s"], "a", "未覆盖用默认值");
        assert!(v.get("ghost").is_none(), "未声明 schema 的段忽略");

        // 非法类型 → 报错 + 回退默认(不崩)
        crate::state::cache_plugin_overrides(&st(), r#"{"rb":{"n":"oops"}}"#);
        let (json, errors) = rebuild_configs(&st());
        assert_eq!(errors.len(), 1, "类型错误报一条: {errors:?}");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["rb"]["n"], 1, "非法值回退默认");

        // 拼错的键必须可见:静默忽略 = 用户以为配置生效了
        crate::state::cache_plugin_overrides(&st(), r#"{"rb":{"n":3,"typo":1}}"#);
        let (json, errors) = rebuild_configs(&st());
        assert_eq!(errors.len(), 1, "未声明的键报一条: {errors:?}");
        assert!(errors[0].contains("typo"), "错误里点到具体键: {errors:?}");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v["rb"].get("typo").is_none(), "未声明的键不进入合并结果");

        // 未声明 schema 的段不报(可能只是懒加载插件还没 load),也不进结果
        crate::state::cache_plugin_overrides(&st(), r#"{"ghost":{"x":1}}"#);
        let (json, errors) = rebuild_configs(&st());
        assert!(errors.is_empty(), "未知段不报错: {errors:?}");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("ghost").is_none(), "未知段不进结果");

        // 覆盖源被替换后不再残留旧键
        crate::state::cache_plugin_overrides(&st(), "{}");
        let (json, _) = rebuild_configs(&st());
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
        // 缓存还没建(脚本顶层读配置的形态)→ 自愈:就地按 schema 默认值算出来,不是 null
        load_script(r#"globalThis.__g = helix.get_config("ft");"#);
        with_engine(|engine| {
            let v = engine
                .global_object()
                .get(JsString::from("__g"), engine)
                .unwrap();
            let o = v.as_object().expect("自愈后应是对象");
            assert!(
                !o.get(JsString::from("show_hidden"), engine)
                    .unwrap()
                    .as_boolean()
                    .unwrap(),
                "自愈取到 schema 默认值 false"
            );
        });
        // 合并缓存 → get_config 返回对象
        crate::state::cache_configs(&st(), r#"{"ft":{"show_hidden":true}}"#);
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
