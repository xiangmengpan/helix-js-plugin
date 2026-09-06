//! server manager 行数据回传(term → JS)：`helix.server.rows(cb)` 入队
//! `UiRequest::ServerListRows{id}`；term 侧在 apply 时算好行、经 [`deliver`]
//! 在主线程回调 cb(rows)。回调注册表在编辑器主线程（与 watch.rs 同哲学，
//! 但 push/deliver 都发生在主线程，无需跨线程通道）。
//!
//! 行对象：{ name, kind, languages: [...], installed, version, installable }

use anyhow::{anyhow, Result};
use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsString, JsValue};
use std::cell::RefCell;
use std::collections::HashMap;

use crate::state::{with_engine, UI_REQUESTS};
use crate::types::{ServerRow, UiRequest};

thread_local! {
    static CALLBACKS: RefCell<Option<&'static mut HashMap<u64, JsValue>>> = const { RefCell::new(None) };
    static NEXT_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

fn with_callbacks<T>(f: impl FnOnce(&mut HashMap<u64, JsValue>) -> T) -> T {
    CALLBACKS.with(|m| {
        let mut slot = m.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

/// `helix.server.rows(cb)`：注册一次性回调并请求当前行数据；返回 request id。
pub fn js_server_rows(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let cb = args.first().cloned().unwrap_or(JsValue::undefined());
    if cb.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.server.rows: callback function required",
        ))));
    }
    let id = NEXT_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    with_callbacks(|m| {
        m.insert(id, cb);
    });
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::ServerListRows { id });
    Ok(JsValue::from(id))
}

/// term 侧回投行数据（主线程 apply 阶段调用）：查回调 → 构造行数组 → 调 cb → 移除回调。
pub fn deliver(id: u64, rows: Vec<ServerRow>) -> Result<()> {
    crate::init();
    with_engine(|engine| -> Result<()> {
        let cb = with_callbacks(|m| m.get(&id).cloned());
        let Some(cb) = cb else {
            return Ok(());
        };
        let func = cb
            .as_callable()
            .and_then(JsFunction::from_object)
            .ok_or_else(|| anyhow!("server rows {id} callback not callable"))?;
        let arr = JsArray::new(engine).map_err(|e| anyhow!("JsArray::new: {e}"))?;
        for r in &rows {
            let langs = JsArray::new(engine).map_err(|e| anyhow!("JsArray::new: {e}"))?;
            for l in &r.languages {
                langs
                    .push(JsValue::from(JsString::from(l.clone())), engine)
                    .map_err(|e| anyhow!("row langs build: {e}"))?;
            }
            let item = ObjectInitializer::new(engine)
                .property(
                    JsString::from("name"),
                    JsValue::from(JsString::from(r.name.clone())),
                    Attribute::all(),
                )
                .property(
                    JsString::from("kind"),
                    JsValue::from(JsString::from(r.kind.clone())),
                    Attribute::all(),
                )
                .property(
                    JsString::from("languages"),
                    JsValue::from(langs),
                    Attribute::all(),
                )
                .property(
                    JsString::from("installed"),
                    JsValue::from(r.installed),
                    Attribute::all(),
                )
                .property(
                    JsString::from("local"),
                    JsValue::from(r.local),
                    Attribute::all(),
                )
                .property(
                    JsString::from("version"),
                    r.version
                        .as_ref()
                        .map(|v| JsValue::from(JsString::from(v.clone())))
                        .unwrap_or(JsValue::null()),
                    Attribute::all(),
                )
                .property(
                    JsString::from("description"),
                    JsValue::from(JsString::from(r.description.clone())),
                    Attribute::all(),
                )
                .property(
                    JsString::from("homepage"),
                    r.homepage
                        .as_ref()
                        .map(|v| JsValue::from(JsString::from(v.clone())))
                        .unwrap_or(JsValue::null()),
                    Attribute::all(),
                )
                .property(
                    JsString::from("installable"),
                    JsValue::from(r.installable),
                    Attribute::all(),
                )
                .build();
            arr.push(item, engine)
                .map_err(|e| anyhow!("row build: {e}"))?;
        }
        let _: JsValue = func
            .call(&JsValue::undefined(), &[JsValue::from(arr)], engine)
            .map_err(|e| anyhow!("server rows {id} callback failed: {e}"))?;
        with_callbacks(|m| {
            m.remove(&id);
        });
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::take_ui_requests;

    #[test]
    fn rows_request_pushes_ui_request_and_callback_receives_data() {
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
        crate::init();
        // 缺参报错
        assert!(crate::load_script(r#"helix.server.rows();"#).is_err());
        crate::state::take_ui_requests(); // 清空
        crate::load_script(
            r#"
            helix.register_command("sm-rows", () => {
                helix.server.rows((rows) => {
                    helix.echo("rows:" + rows.length + ":" + rows[0].name + ":" + rows[0].installed + ":" + rows[0].version);
                });
            });
            "#,
        )
        .unwrap();
        let ctx = crate::CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(crate::run_command("sm-rows", &ctx).unwrap());
        let reqs = take_ui_requests();
        let id = match &reqs[0] {
            UiRequest::ServerListRows { id } => *id,
            other => panic!("expected ServerListRows, got {other:?}"),
        };
        deliver(
            id,
            vec![ServerRow {
                name: "rust-analyzer".into(),
                kind: "lsp".into(),
                languages: vec!["rust".into()],
                installed: true,
                local: false,
                version: Some("rust-analyzer 1.2.3".into()),
                description: "".into(),
                homepage: None,
                installable: true,
            }],
        )
        .unwrap();
        assert_eq!(
            crate::take_messages(),
            vec!["rows:1:rust-analyzer:true:rust-analyzer 1.2.3"]
        );
        // 一次性:再次 deliver 同 id 无回调,静默跳过
        deliver(id, Vec::new()).unwrap();
    }
}
