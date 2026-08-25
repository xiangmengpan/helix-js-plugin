//! fs-watcher 模块：`helix.watch(path, cb)` / `helix.unwatch(id)`。
//! JS 侧：校验参数、注册回调（watcher id → JsValue）、入队 UiRequest::Watch/Unwatch；
//! helix-term 侧建 notify watcher，变更经 `push_watch_event` 投递回本模块的通道，
//! 主线程 `drain_watch_events` + `resolve_watch_events` 调 JS 回调（events 数组）。
//! 设计：docs/superpowers/specs/2026-08-15-fs-watcher-design.md

use anyhow::{anyhow, Result};
use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsString, JsValue};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::state::{with_engine, UI_REQUESTS};
use crate::types::{UiRequest, WatchChange, WatchEvent};

/// 事件通道：全局（发送端供 notify 回调线程投递；接收端主线程 drain）。
/// 与 TERM_EVENTS 的 thread_local 不同——watch 回调注册表（WATCH_CALLBACKS）在
/// 编辑器主线程，全局通道免去"接收端线程归属"坑（notify 线程先于主线程
/// 首次 drain 建通道会把 thread_local 接收端建错线程，事件静默丢失）；
/// 编辑器单线程运行，不存在并发测试互偷问题。接收端 Send，可安全共存于 Mutex。
static WATCH_CHANNEL: OnceLock<
    Mutex<(
        std::sync::mpsc::Sender<WatchEvent>,
        std::sync::mpsc::Receiver<WatchEvent>,
    )>,
> = OnceLock::new();

// 持有 JsValue：线程退出时内容泄漏（与 CONTEXT 同哲学，见 state.rs 顶部注释）
thread_local! {
    static WATCH_CALLBACKS: RefCell<Option<&'static mut HashMap<u64, JsValue>>> = const { RefCell::new(None) };
    static NEXT_WATCH_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

fn with_watch_callbacks<T>(f: impl FnOnce(&mut HashMap<u64, JsValue>) -> T) -> T {
    WATCH_CALLBACKS.with(|m| {
        let mut slot = m.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}

fn next_watch_id() -> u64 {
    NEXT_WATCH_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}

/// 供 helix-term 的 notify 回调线程投递 watcher 事件（任意线程可调）。
/// 无对应回调（已 unwatch / 未知 id）时事件仍入队，resolve 时按 id 查不到回调即跳过。
pub fn push_watch_event(id: u64, kind: &str, path: String) {
    let ch = WATCH_CHANNEL.get_or_init(|| Mutex::new(std::sync::mpsc::channel()));
    let (tx, _rx) = &mut *ch.lock().unwrap();
    let _ = tx.send(WatchEvent::Changed(id, vec![WatchChange { kind: kind.to_string(), path }]));
}

/// 取走全部待处理 watcher 事件（主线程轮询用；幂等）
pub fn drain_watch_events() -> Vec<WatchEvent> {
    crate::init();
    let mut events = Vec::new();
    if let Some(ch) = WATCH_CHANNEL.get() {
        let (_tx, rx) = &mut *ch.lock().unwrap();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
    }
    events
}

/// 把全部待处理 watcher 事件投递到对应 id 的 JS 回调。
/// 回调签名：(events) → events: [{kind, path}, ...]（设计文档 §2）。
/// 无回调（已 unwatch / 未知 id）→ 静默跳过；回调执行失败 → Err。
pub fn resolve_watch_events() -> Result<()> {
    crate::init();
    let events = drain_watch_events();
    for event in events {
        let WatchEvent::Changed(id, changes) = event;
        with_engine(|engine| -> Result<()> {
            let cb = with_watch_callbacks(|m| m.get(&id).cloned());
            let Some(cb) = cb else { return Ok(()) };
            let func = cb
                .as_callable()
                .and_then(JsFunction::from_object)
                .ok_or_else(|| anyhow!("watch {id} callback not callable"))?;
            let arr = JsArray::new(engine);
            for c in &changes {
                let item = ObjectInitializer::new(engine)
                    .property(
                        JsString::from("kind"),
                        JsValue::from(JsString::from(c.kind.clone())),
                        Attribute::all(),
                    )
                    .property(
                        JsString::from("path"),
                        JsValue::from(JsString::from(c.path.clone())),
                        Attribute::all(),
                    )
                    .build();
                arr.push(item, engine)
                    .map_err(|e| anyhow!("watch {id} event build failed: {e}"))?;
            }
            let _: JsValue = func
                .call(&JsValue::undefined(), &[JsValue::from(arr)], engine)
                .map_err(|e| anyhow!("watch {id} callback failed: {e}"))?;
            Ok(())
        })?;
    }
    Ok(())
}

/// 注册 watcher：`helix.watch(path, cb)` → watcher id。
/// 回调注册进 WATCH_CALLBACKS，请求入队 UiRequest::Watch（helix-term 建 notify watcher）。
pub(crate) fn js_watch(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let path: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| JsError::from_opaque(JsValue::from(JsString::from(
            "helix.watch: path must be a string",
        ))))?;
    if path.is_empty() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.watch: path must not be empty",
        ))));
    }
    let cb = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if cb.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.watch: callback must be a function",
        ))));
    }
    let id = next_watch_id();
    with_watch_callbacks(|m| {
        m.insert(id, cb);
    });
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::Watch { id, path });
    Ok(JsValue::from(id))
}

/// 停止 watcher：`helix.unwatch(id)` —— 移除回调 + 入队 UiRequest::Unwatch。
pub(crate) fn js_unwatch(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| JsError::from_opaque(JsValue::from(JsString::from(
            "helix.unwatch: id must be a number",
        ))))?;
    with_watch_callbacks(|m| {
        m.remove(&id);
    });
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::Unwatch { id });
    Ok(JsValue::undefined())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::take_ui_requests;
    use boa_engine::Source;
    use std::sync::Mutex;

    // 多个测试共享全局运行时（WATCH_CALLBACKS/UI_REQUESTS/WATCH_CHANNEL），锁串行化
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn eval_fn(src: &str) -> JsValue {
        with_engine(|engine| engine.eval(Source::from_bytes(src)).unwrap())
    }

    /// 参数校验：非字符串路径 / 空路径 / 回调非函数 → Err
    #[test]
    fn watch_validates_args() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        let mut c = Context::default();
        assert!(
            js_watch(&JsValue::undefined(), &[JsValue::from(42)], &mut c).is_err(),
            "路径非字符串 → Err"
        );
        assert!(
            js_watch(&JsValue::undefined(), &[JsValue::from(JsString::from(""))], &mut c).is_err(),
            "空路径 → Err"
        );
        assert!(
            js_watch(&JsValue::undefined(), &[JsValue::from(JsString::from("/tmp/hx")), JsValue::from(7)], &mut c).is_err(),
            "回调非函数 → Err"
        );
        assert!(
            js_unwatch(&JsValue::undefined(), &[JsValue::from(JsString::from("x"))], &mut c).is_err(),
            "unwatch 非数字 → Err"
        );
    }

    /// 注册：返回数字 id；回调入注册表；入队 UiRequest::Watch { id, path }
    #[test]
    fn watch_registers_and_enqueues() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        let mut c = Context::default();
        let cb = c.eval(Source::from_bytes("() => {}")).unwrap();
        let ret = js_watch(
            &JsValue::undefined(),
            &[JsValue::from(JsString::from("/tmp/hx-watch")), cb],
            &mut c,
        )
        .unwrap();
        let id = ret.as_number().unwrap() as u64;
        assert!(id >= 1, "watch 返回递增 id");
        assert!(with_watch_callbacks(|m| m.contains_key(&id)), "回调已注册");
        let reqs = take_ui_requests();
        assert!(
            reqs.iter()
                .any(|r| matches!(r, UiRequest::Watch { id: i, path } if *i == id && path == "/tmp/hx-watch")),
            "入队 UiRequest::Watch"
        );
    }

    /// push_watch_event → resolve：回调收到 [{kind, path}] 数组；
    /// 未注册 id 静默跳过；resolve 幂等。
    #[test]
    fn watch_resolve_delivers_events() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        let cb = eval_fn(
            r#"(events) => { globalThis.__got = (globalThis.__got || "") + events.length + ":" + events[0].kind + ":" + events[0].path + ";"; }"#,
        );
        with_watch_callbacks(|m| {
            m.insert(1, cb);
        });
        push_watch_event(1, "modify", "/tmp/hx/a.txt".to_string());
        push_watch_event(1, "create", "/tmp/hx/b.txt".to_string());
        assert!(resolve_watch_events().is_ok());
        with_engine(|engine| {
            let got = engine.global_object().get(JsString::from("__got"), engine).unwrap();
            assert_eq!(
                got.as_string().unwrap().to_std_string_escaped(),
                "1:modify:/tmp/hx/a.txt;1:create:/tmp/hx/b.txt;",
                "按入队顺序逐条回调"
            );
        });
        // 未注册回调的 id → 静默跳过
        push_watch_event(999, "delete", "/tmp/hx/c.txt".to_string());
        assert!(resolve_watch_events().is_ok());
        // 空队列 → no-op
        assert!(resolve_watch_events().is_ok());
    }

    /// unwatch：移除回调 + 入队 UiRequest::Unwatch
    #[test]
    fn unwatch_removes_and_enqueues() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        let mut c = Context::default();
        let cb = c.eval(Source::from_bytes("() => {}")).unwrap();
        let ret = js_watch(
            &JsValue::undefined(),
            &[JsValue::from(JsString::from("/tmp/hx-unwatch")), cb],
            &mut c,
        )
        .unwrap();
        let id = ret.as_number().unwrap() as u64;
        assert!(with_watch_callbacks(|m| m.contains_key(&id)));
        js_unwatch(&JsValue::undefined(), &[JsValue::from(id)], &mut c).unwrap();
        assert!(!with_watch_callbacks(|m| m.contains_key(&id)), "回调已移除");
        let reqs = take_ui_requests();
        assert!(
            reqs.iter()
                .any(|r| matches!(r, UiRequest::Unwatch { id: i } if *i == id)),
            "入队 UiRequest::Unwatch"
        );
    }
}
