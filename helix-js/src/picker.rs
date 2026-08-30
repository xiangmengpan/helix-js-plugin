//! helix.picker：插件 picker 源表 + define/run（任务 2；term 侧驱动在任务 3）。
//! define 注册 {columns, items, ...} 到源表；run 取源调 items（同步数组或 Promise 续体），
//! 把候选行经 UiRequest::OpenPicker 交给 helix-term 渲染。

use boa_engine::object::builtins::JsFunction;
use boa_engine::object::FunctionObjectBuilder;
use boa_engine::{Context, JsError, JsObject, JsString, JsValue, NativeFunction};

use crate::state::with_picker_sources;
use crate::types::UiRequest;

/// picker 候选行（纯数据，跨线程）：cells = 各列文本；payload = 原样数据（action/preview 用）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RowSpec {
    pub cells: Vec<String>,
    pub payload: Vec<String>,
}

/// helix.picker.define(name, { columns, items, preview?, action? })：注册源到源表
pub(crate) fn js_picker_define(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "picker.define: name must be a string",
            )))
        })?;
    let Some(config) = args.get(1).and_then(|v| v.as_object()) else {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "picker.define: config must be an object",
        ))));
    };
    // 校验 columns：字符串数组
    let columns = config.get(JsString::from("columns"), context)?;
    columns
        .as_object()
        .and_then(|o| o.get(JsString::from("length"), context).ok())
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "picker.define: columns must be an array",
            )))
        })?;
    // 校验 items：可调用
    let items = config.get(JsString::from("items"), context)?;
    if !items.is_callable() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "picker.define: items must be a function",
        ))));
    }
    with_picker_sources(|m| {
        m.insert(name, config.into());
    });
    Ok(JsValue::undefined())
}

/// helix.picker.run(name)：取源 → 调 items（同步数组或 Promise）→ push UiRequest::OpenPicker
pub(crate) fn js_picker_run(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "picker.run: name must be a string",
            )))
        })?;
    let config = with_picker_sources(|m| m.get(&name).cloned()).ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "picker.run: source '{name}' not defined"
        ))))
    })?;
    let config_obj = config.as_object().unwrap().clone();
    let items_fn = config_obj.get(JsString::from("items"), context)?;
    let items_fn = items_fn
        .as_callable()
        .and_then(JsFunction::from_object)
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "picker.run: source has no callable items",
            )))
        })?;
    let result = items_fn.call(&JsValue::undefined(), &[], context)?;
    // 同步数组 → 立即 push；Promise → .then 续体里 push
    push_open_picker(&name, result, context)?;
    Ok(JsValue::undefined())
}

/// 把 items 结果（同步数组或 Promise）转成 OpenPicker 请求 push 出去
fn push_open_picker(
    name: &str,
    items_result: JsValue,
    context: &mut Context,
) -> boa_engine::JsResult<()> {
    if let Some(promise) = items_result.as_promise() {
        // async items：注册 .then 续体，resolve 后 push（含行数据）
        let name = JsString::from(name);
        let resolve = NativeFunction::from_copy_closure_with_captures(
            |_, args, name: &JsString, ctx| {
                let name = name.to_std_string_escaped();
                let value = args.first().cloned().unwrap_or_default();
                let rows = parse_rows(value, ctx)?;
                push_request(UiRequest::OpenPicker { source: name, rows });
                Ok(JsValue::undefined())
            },
            name,
        );
        let f = FunctionObjectBuilder::new(context.realm(), resolve).build();
        promise.then(Some(f), None, context);
        Ok(())
    } else {
        let rows = parse_rows(items_result, context)?;
        push_request(UiRequest::OpenPicker {
            source: name.to_string(),
            rows,
        });
        Ok(())
    }
}

fn push_request(req: UiRequest) {
    // 不能用 crate::init()——脚本 eval / pump_jobs 期间 CONTEXT 已被借用，会 RefCell 冲突；
    // UI_REQUESTS 静态队列入口处（load_script 等）已初始化，这里 get_or_init 兜底即可
    let _ = crate::state::UI_REQUESTS.get_or_init(Default::default);
    crate::state::UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(req);
}

/// items 行 → Vec<RowSpec>；行 = 数组 [c1,c2] 或对象 {cells, payload}
fn parse_rows(value: JsValue, context: &mut Context) -> boa_engine::JsResult<Vec<RowSpec>> {
    let rows_arr = value
        .as_object()
        .filter(|o| o.is_array())
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "picker: items must return an array of rows",
            )))
        })?;
    let length = rows_arr
        .get(JsString::from("length"), context)?
        .as_number()
        .unwrap_or(0.0) as usize;
    let mut rows = Vec::with_capacity(length);
    for i in 0..length {
        let row = rows_arr.get(i, context)?;
        rows.push(parse_row(row, context)?);
    }
    Ok(rows)
}

fn parse_row(value: JsValue, context: &mut Context) -> boa_engine::JsResult<RowSpec> {
    let err = |msg: &str| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "picker: {msg}"
        ))))
    };
    let Some(obj) = value.as_object() else {
        return Err(err("each row must be an array or object"));
    };
    if obj.is_array() {
        // 数组行：cells = payload = 数组元素字符串
        let cells = read_cells(&obj, context)?;
        return Ok(RowSpec {
            payload: cells.clone(),
            cells,
        });
    }
    // 对象行 {cells:[..], payload?:[..]}；payload 缺省 = cells
    let cells_obj = obj
        .get(JsString::from("cells"), context)?
        .as_object()
        .ok_or_else(|| err("row.cells must be an array"))?;
    let cells = read_cells(&cells_obj, context)?;
    let payload = match obj.get(JsString::from("payload"), context)? {
        v if v.is_undefined() => cells.clone(),
        v => read_cells(
            &v.as_object().ok_or_else(|| err("row.payload must be an array"))?,
            context,
        )?,
    };
    Ok(RowSpec { cells, payload })
}

/// 读 JS 数组元素为字符串（数字/布尔经 to_string 归一）
fn read_cells(arr: &JsObject, context: &mut Context) -> boa_engine::JsResult<Vec<String>> {
    let length = arr
        .get(JsString::from("length"), context)?
        .as_number()
        .unwrap_or(0.0) as usize;
    let mut out = Vec::with_capacity(length);
    for i in 0..length {
        let v = arr.get(i, context)?;
        out.push(v.to_string(context)?.to_std_string_escaped());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load_script, pump_jobs, take_ui_requests};
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn picker_source_exists(name: &str) -> bool {
        with_picker_sources(|m| m.contains_key(name))
    }

    #[test]
    fn picker_define_and_validate() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        // 非法：columns 非数组 / items 非函数 → 报错
        assert!(
            load_script(r#"helix.picker.define("x", { columns: "nope", items: () => [] });"#)
                .is_err()
        );
        assert!(
            load_script(r#"helix.picker.define("x", { columns: ["a"], items: 42 });"#).is_err()
        );
        // 合法注册后可读
        load_script(
            r#"helix.picker.define("f", { columns: ["name", "path"], items: () => [["a.rs", "src/a.rs"]] });"#,
        )
        .unwrap();
        assert!(picker_source_exists("f"));
    }

    #[test]
    fn picker_run_unknown_source_errors() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        assert!(load_script(r#"helix.picker.run("nope");"#).is_err());
    }

    #[test]
    fn picker_run_pushes_open_picker_sync_rows() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        load_script(
            r#"helix.picker.define("f", { columns: ["name"], items: () => [["a.rs", "x"], { cells: ["b.rs"], payload: ["b.rs", "extra"] }] });"#,
        )
        .unwrap();
        load_script(r#"helix.picker.run("f");"#).unwrap();
        let reqs = take_ui_requests();
        let rows = match &reqs[0] {
            UiRequest::OpenPicker { source, rows } => {
                assert_eq!(source, "f");
                rows
            }
            other => panic!("expected OpenPicker, got {other:?}"),
        };
        assert_eq!(rows.len(), 2);
        // 数组行：cells = payload = 数组元素字符串
        assert_eq!(rows[0].cells, vec!["a.rs", "x"]);
        assert_eq!(rows[0].payload, vec!["a.rs", "x"]);
        // 对象行：cells/payload 分离
        assert_eq!(rows[1].cells, vec!["b.rs"]);
        assert_eq!(rows[1].payload, vec!["b.rs", "extra"]);
    }

    #[test]
    fn picker_run_async_items_pushes_after_pump() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        load_script(
            r#"helix.picker.define("f", { columns: ["name"], items: async () => [["a.rs"]] });"#,
        )
        .unwrap();
        load_script(r#"helix.picker.run("f");"#).unwrap();
        // Promise 未兑现前不 push；pump_jobs 执行 .then 续体后 push
        assert!(take_ui_requests().is_empty());
        pump_jobs().unwrap();
        let reqs = take_ui_requests();
        match &reqs[0] {
            UiRequest::OpenPicker { source, rows } => {
                assert_eq!(source, "f");
                assert_eq!(rows[0].cells, vec!["a.rs"]);
            }
            other => panic!("expected OpenPicker, got {other:?}"),
        }
    }
}
