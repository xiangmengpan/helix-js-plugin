//! server 后台任务模块：`helix.server.task(items, cb)` 注册回调并请求
//! `UiRequest::ServerTask{task_id, items}`；helix-term 侧单 worker 串行执行，
//! 事件(phase/progress/done/error)经 `push_event` 跨线程投递回本模块通道，
//! 主线程 `drain_events` + `resolve_events` 调 JS 回调(单对象事件)。
//! 回调注册表在编辑器主线程（与 watch.rs 同哲学：全局 mpsc 通道，发送端
//! 供 worker 任意线程投递；接收端主线程 drain）。

use anyhow::{anyhow, Result};
use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsString, JsValue};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::state::{with_engine, UI_REQUESTS};
use crate::types::{ServerTaskEvent, ServerTaskItem, UiRequest};

/// 事件通道：全局（发送端供 term worker 线程投递；接收端主线程 drain）。
/// 与 WATCH_CHANNEL 同哲学——回调注册表在编辑器主线程，全局通道免去
/// "接收端线程归属"坑（worker 先于主线程首次 drain 建通道会把 thread_local
/// 接收端建错线程，事件静默丢失）；编辑器单线程运行，无并发互偷问题。
static TASK_CHANNEL: OnceLock<
    Mutex<(
        std::sync::mpsc::Sender<ServerTaskEvent>,
        std::sync::mpsc::Receiver<ServerTaskEvent>,
    )>,
> = OnceLock::new();

// 持有 JsValue：线程退出时内容泄漏（与 CONTEXT 同哲学，见 state.rs 顶部注释）
thread_local! {
    static CALLBACKS: RefCell<Option<&'static mut HashMap<u64, JsValue>>> = const { RefCell::new(None) };
    static NEXT_TASK_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

fn with_callbacks<T>(f: impl FnOnce(&mut HashMap<u64, JsValue>) -> T) -> T {
    CALLBACKS.with(|m| {
        let mut slot = m.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

fn next_task_id() -> u64 {
    NEXT_TASK_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}

/// 供 helix-term 的 worker 线程投递任务事件（任意线程可调；worker 串行执行，
/// seq 由 worker 每批次内自 0 递增）。无对应回调时事件仍入队，resolve 跳过。
pub fn push_event(ev: ServerTaskEvent) {
    let ch = TASK_CHANNEL.get_or_init(|| Mutex::new(std::sync::mpsc::channel()));
    let (tx, _rx) = &mut *ch.lock().unwrap();
    let _ = tx.send(ev);
}

/// 取走全部待处理任务事件（主线程轮询用；幂等）
pub fn drain_events() -> Vec<ServerTaskEvent> {
    let mut events = Vec::new();
    if let Some(ch) = TASK_CHANNEL.get() {
        let (_tx, rx) = &mut *ch.lock().unwrap();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
    }
    events
}

/// 把全部待处理任务事件投递到对应 task_id 的 JS 回调。
/// 回调签名：(ev) → ev: {task_id, seq, name, kind, msg, bytes, total}。
/// 无回调(未知 task_id) → 静默跳过；done/error 后不移除回调
/// （JS 端按 task_id 自行管理；worker 不会再为该 id 发事件，二次注册由 JS 负责）。
pub fn resolve_events() -> Result<()> {
    crate::init();
    let events = drain_events();
    for ev in events {
        let task_id = ev.task_id;
        with_engine(|engine| -> Result<()> {
            let cb = with_callbacks(|m| m.get(&task_id).cloned());
            let Some(cb) = cb else {
                return Ok(());
            };
            let func = cb
                .as_callable()
                .and_then(JsFunction::from_object)
                .ok_or_else(|| anyhow!("server task {task_id} callback not callable"))?;
            let obj = ObjectInitializer::new(engine)
                .property(
                    JsString::from("task_id"),
                    JsValue::from(ev.task_id),
                    Attribute::all(),
                )
                .property(
                    JsString::from("seq"),
                    JsValue::from(ev.seq),
                    Attribute::all(),
                )
                .property(
                    JsString::from("name"),
                    JsValue::from(JsString::from(ev.name.clone())),
                    Attribute::all(),
                )
                .property(
                    JsString::from("kind"),
                    JsValue::from(JsString::from(ev.kind.clone())),
                    Attribute::all(),
                )
                .property(
                    JsString::from("msg"),
                    ev.msg
                        .as_ref()
                        .map(|m| JsValue::from(JsString::from(m.clone())))
                        .unwrap_or(JsValue::null()),
                    Attribute::all(),
                )
                .property(
                    JsString::from("bytes"),
                    ev.bytes.map(JsValue::from).unwrap_or(JsValue::null()),
                    Attribute::all(),
                )
                .property(
                    JsString::from("total"),
                    ev.total.map(JsValue::from).unwrap_or(JsValue::null()),
                    Attribute::all(),
                )
                .build();
            let _: JsValue = func
                .call(&JsValue::undefined(), &[JsValue::from(obj)], engine)
                .map_err(|e| anyhow!("server task {task_id} callback failed: {e}"))?;
            Ok(())
        })?;
    }
    Ok(())
}

/// 注册后台任务：`helix.server.task(items, cb)` → task_id。
/// items: [{op, name, version?}, ...]（op: install/update/remove/unmanage；
/// name: 配方名；version: 可选显式版本）；cb: 进度/结果回调(单对象事件)。
/// 回调注册进 CALLBACKS，请求入队 UiRequest::ServerTask（term 建单 worker 串行执行）。
pub(crate) fn js_server_task(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let arr = args
        .first()
        .and_then(|v| v.as_object())
        .filter(|o| o.is_array())
        .and_then(|o| JsArray::from_object(o.clone()).ok())
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.server.task: items must be an array of { op, name, version? }",
            )))
        })?;
    let cb = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if cb.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.server.task: callback must be a function",
        ))));
    }
    let len = arr
        .length(ctx)
        .map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(format!("{e}")))))?;
    if len == 0 {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.server.task: items must not be empty",
        ))));
    }
    let mut items = Vec::with_capacity(len as usize);
    for i in 0..len {
        let elem = arr.get(i, ctx).map_err(|e| {
            JsError::from_opaque(JsValue::from(JsString::from(format!("item {i}: {e}"))))
        })?;
        let obj = elem.as_object().ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "helix.server.task: item {i} must be {{ op, name, version? }}"
            ))))
        })?;
        let mut get_str = |key: &str| -> boa_engine::JsResult<String> {
            let v = obj.get(JsString::from(key), ctx)?;
            if v.is_null_or_undefined() {
                Ok(String::new())
            } else {
                v.try_js_into(ctx).map_err(|_| {
                    JsError::from_opaque(JsValue::from(JsString::from(format!(
                        "helix.server.task: item {i} '{key}' must be a string"
                    ))))
                })
            }
        };
        let op = get_str("op")?;
        let name = get_str("name")?;
        if op.is_empty() {
            return Err(JsError::from_opaque(JsValue::from(JsString::from(
                "helix.server.task: item op required",
            ))));
        }
        let version = obj
            .get(JsString::from("version"), ctx)?
            .as_string()
            .map(|s| s.to_std_string_escaped());
        items.push(ServerTaskItem { op, name, version });
    }
    let task_id = next_task_id();
    with_callbacks(|m| {
        m.insert(task_id, cb);
    });
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::ServerTask { task_id, items });
    Ok(JsValue::from(task_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{take_ui_requests, with_engine};
    use crate::types::ServerTaskEvent;
    use boa_engine::{object::builtins::JsArray, JsString, Source};

    /// 模拟主线程泵：worker(term) 推入的事件 drain + resolve 到回调
    fn deliver_event(ev: ServerTaskEvent) {
        push_event(ev);
        resolve_events().unwrap();
    }

    /// 参数校验：首参非数组 / 回调不可调 / 空数组 → Err（不消耗 taskId）
    #[test]
    fn task_validates_args() {
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
        crate::init();
        with_engine(|ctx| {
            let cb = ctx.eval(Source::from_bytes("() => {}")).unwrap();
            assert!(
                js_server_task(&JsValue::undefined(), &[JsValue::from(7), cb.clone()], ctx)
                    .is_err(),
                "首参非数组 → Err"
            );
            assert!(
                js_server_task(
                    &JsValue::undefined(),
                    &[JsValue::from(JsString::from("x")), JsValue::from(8)],
                    ctx
                )
                .is_err(),
                "回调非函数 → Err"
            );
            let empty = JsValue::from(JsArray::new(ctx).unwrap());
            assert!(
                js_server_task(&JsValue::undefined(), &[empty, cb.clone()], ctx).is_err(),
                "空数组 → Err"
            );
        });
    }

    /// 注册 + 事件回投：echo "id:"+id → UiRequest::ServerTask；模拟 term 逐事件
    /// 回传 progress/done，回调按送达顺序 echo，消息序列对上。
    #[test]
    fn task_api_pushes_request_and_events_deliver() {
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
        crate::init();
        crate::state::take_ui_requests(); // 清
        crate::load_script(
            r#"
        helix.register_command("sm-task", () => {
            const id = helix.server.task([{ op: "install", name: "demo" }], (ev) => {
                helix.echo("ev:" + ev.kind + ":" + ev.name + ":" + (ev.bytes || 0));
            });
            helix.echo("id:" + id);
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
        assert!(crate::run_command("sm-task", &ctx).unwrap());
        // 取第一个 ServerTask 请求(其余测试可能并发残留其它请求,find 而非下标 0)
        let reqs = take_ui_requests();
        let (tid, items) = reqs
            .into_iter()
            .find_map(|r| match r {
                crate::types::UiRequest::ServerTask { task_id, items } => Some((task_id, items)),
                _ => None,
            })
            .expect("入队 UiRequest::ServerTask");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].op, "install");
        assert_eq!(items[0].name, "demo");
        assert_eq!(tid, 1, "首个 taskId 从 1 起(与 next_server_task_id 一致)");
        // 模拟 term 回传(带 JS 分配的 task_id)
        deliver_event(ServerTaskEvent {
            task_id: tid,
            seq: 0,
            name: "demo".into(),
            kind: "progress".into(),
            msg: None,
            bytes: Some(10),
            total: Some(100),
        });
        deliver_event(ServerTaskEvent {
            task_id: tid,
            seq: 1,
            name: "demo".into(),
            kind: "done".into(),
            msg: Some("ok".into()),
            bytes: None,
            total: None,
        });
        assert_eq!(
            crate::take_messages().join(" "),
            "id:1 ev:progress:demo:10 ev:done:demo:0"
        );
    }
}
