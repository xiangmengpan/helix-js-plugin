//! server 后台任务模块：`helix.server.task(items, cb)` 注册回调并请求
//! `UiRequest::ServerTask{task_id, items}`；helix-term 侧单 worker 串行执行，
//! 事件(phase/progress/done/error)经 `push_event` 跨线程投递回
//! **提交它的那个宿主**的事件桶，主线程 `drain_events` + `resolve_events`
//! 调 JS 回调(单对象事件)。
//! 回调注册表在编辑器主线程，按宿主隔离（thread_local）；事件通道同样**按宿主分桶**
//! （见 `host::UiRequests`/`host::TaskChannels`）——worker 没有宿主，所以事件按
//! **task_id → 提交它的宿主** 路由（task_id 进程内全局唯一，见 `next_task_id`）。
//! 唤醒：通道为 WakeSender——worker 每推一条事件即触发宿主注册的 term wake
//! (request_redraw → 事件循环 33ms 内重绘再 pump)。无此唤醒时 worker 进度/done
//! 只在下一次按键或单发 idle 才被 resolve(长下载全程冻结)——见 I1。

use anyhow::{anyhow, Result};
use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsString, JsValue};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::state::{wake_sender, with_engine, WakeSender, UI_REQUESTS};
use crate::types::{ServerTaskEvent, ServerTaskItem, ServerTaskReq, UiRequest};

/// 事件桶（**按宿主分桶**）：发送端 WakeSender = worker 推事件即唤醒主循环。
///
/// 为什么不能再是"进程级单通道"（原实现，注释写着"编辑器单线程运行，无并发互偷"）：
/// 集成套件在**同一进程**里并行跑多个 `Application`（各一个宿主，各一个线程）。
/// 全局通道 + 全局接收端的形态下，**任何**宿主的 `resolve_events()` 都会先把别人的
/// 事件 drain 走，而它的 `CALLBACKS` 是 thread_local ⇒ 那些事件被"无对应回调，静默
/// 跳过"丢掉（实测：全并行下 arsenal 批量安装卡在 `0% (0/2)` 且 `pending_jobs()==0`
/// —— job 跑完了，事件却被别的宿主吃掉）。分桶后每个宿主只看得见自己那一桶。
type TaskChannel = (
    WakeSender<ServerTaskEvent>,
    std::sync::mpsc::Receiver<ServerTaskEvent>,
);
static TASK_CHANNELS: OnceLock<Mutex<HashMap<u64, TaskChannel>>> = OnceLock::new();

/// task_id → **提交它的宿主** key。worker 线程没有宿主，不能按"当前宿主"投递；
/// task_id 在主线程分配（`js_server_task`），分配时顺手记下当下宿主即可。
/// 未登记的 task_id（纯单测直接 `push_event`）⇒ 回落本线程宿主/哨兵桶。
static TASK_HOSTS: OnceLock<Mutex<HashMap<u64, u64>>> = OnceLock::new();

/// "没有宿主"的哨兵桶（worker / 未 `init()` 的单测线程）。真实宿主 key 是堆地址，
/// 不可能等于 `u64::MAX`。用它而不是 `current_host_key_or_init()`：取事件这种
/// **读**操作不该在 worker 线程上现建一份引擎。
const NO_HOST: u64 = u64::MAX;

/// 本线程宿主的桶 key（无宿主 ⇒ 哨兵桶）
fn local_host_key() -> u64 {
    crate::host::try_current_host_key().unwrap_or(NO_HOST)
}

/// 事件归属的宿主：优先用提交时登记值；未登记（单测直接推、同线程 push+drain）
/// 回落到**本线程**宿主
fn task_host_key(task_id: u64) -> u64 {
    TASK_HOSTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&task_id)
        .copied()
        .unwrap_or_else(local_host_key)
}

/// 记下 task_id 归属的宿主（**必须在提交方线程调**：此刻的宿主就是收事件的那个）。
///
/// 两处调用点，各自守住一类路径：
/// · `js_server_task`（JS 提交，task_id 在这里分配）—— 主路径；
/// · helix-term 的 `server_tasks::submit`（集成/单测直接提交，不经 JS）—— 否则 worker
///   线程按"当前宿主"回落会落到**它自己的**（空）桶，事件永远到不了提交方。
pub fn register_task_host(task_id: u64) {
    TASK_HOSTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(task_id, local_host_key());
}

// 持有 JsValue：线程退出时内容泄漏（与 CONTEXT 同哲学，见 state.rs 顶部注释）
thread_local! {}

/// task_id **进程内全局唯一**（原来是 thread_local ⇒ 每个宿主都从 1 重新数）。
/// 路由表是 `task_id → 宿主`，两个宿主同时用到同一个 id 就会**串台**（事件投给
/// 另一个宿主的桶，对方拿自己的同 id 回调接收 ⇒ 静默错投）⇒ 必须全局唯一。
static NEXT_TASK_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn with_callbacks<T>(f: impl FnOnce(&mut HashMap<u64, JsValue>) -> T) -> T {
    crate::host::with_host_ref_or_init(|h| f(&mut h.server_task_callbacks.borrow_mut()))
}

fn next_task_id() -> u64 {
    NEXT_TASK_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

/// 供 helix-term 的 worker 线程投递任务事件（任意线程可调；worker 串行执行，
/// seq 由 worker 每批次内自 0 递增）。按 `task_id` 投回**提交它的宿主**的桶；
/// 无对应回调时事件仍入队，resolve 跳过。
pub fn push_event(ev: ServerTaskEvent) {
    let key = task_host_key(ev.task_id);
    let mut chans = TASK_CHANNELS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (tx, _rx) = chans.entry(key).or_insert_with(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        (wake_sender(tx), rx)
    });
    let _ = tx.send(ev); // WakeSender::send 触发注册的 term wake(主循环重绘)
}

/// 取走并清空**本宿主**待处理的任务事件（主线程轮询用；幂等）。
/// 别人的桶一行不碰 —— 这是跨宿主互偷的隔离点。
pub fn drain_events() -> Vec<ServerTaskEvent> {
    let mut events = Vec::new();
    let Some(chans) = TASK_CHANNELS.get() else {
        return events;
    };
    let key = local_host_key();
    let mut chans = chans
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_tx, rx)) = chans.get_mut(&key) {
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
                // 无回调(未知 task_id / 已由别的宿主处理)⇒ 静默跳过。
                // 跨宿主互偷由桶隔离负责(见 TASK_CHANNELS):本宿主只 drain 自己的桶。
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
            let _: JsValue = crate::permissions::with_callback_owner(
                &crate::permissions::CbKey::server_task(task_id),
                || func.call(&JsValue::undefined(), &[JsValue::from(obj)], engine),
            )
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
    // B1:后台任务会下载/安装/卸载(写盘 + 起子进程)⇒ 与 `helix.run` 同一能力。
    let arr = args
        .first()
        .and_then(|v| v.as_object())
        .filter(|o| o.is_array())
        .and_then(|o| JsArray::from_object(o.clone()).ok())
        .ok_or_else(|| {
            crate::js_err("helix.server.task: items must be an array of { op, name, version? }")
        })?;
    let cb = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if cb.as_callable().is_none() {
        return Err(crate::js_err(
            "helix.server.task: callback must be a function",
        ));
    }
    let len = arr.length(ctx).map_err(|e| crate::js_err(format!("{e}")))?;
    if len == 0 {
        return Err(crate::js_err("helix.server.task: items must not be empty"));
    }
    let mut items = Vec::with_capacity(len as usize);
    for i in 0..len {
        let elem = arr
            .get(i, ctx)
            .map_err(|e| crate::js_err(format!("item {i}: {e}")))?;
        let obj = elem.as_object().ok_or_else(|| {
            crate::js_err(format!(
                "helix.server.task: item {i} must be {{ op, name, version? }}"
            ))
        })?;
        let mut get_str = |key: &str| -> boa_engine::JsResult<String> {
            let v = obj.get(JsString::from(key), ctx)?;
            if v.is_null_or_undefined() {
                Ok(String::new())
            } else {
                v.try_js_into(ctx).map_err(|_| {
                    crate::js_err(format!(
                        "helix.server.task: item {i} '{key}' must be a string"
                    ))
                })
            }
        };
        let op = get_str("op")?;
        let name = get_str("name")?;
        if op.is_empty() {
            return Err(crate::js_err("helix.server.task: item op required"));
        }
        let version = {
            let v = obj.get(JsString::from("version"), ctx)?;
            if v.is_null_or_undefined() {
                None
            } else {
                // 与 op/name 一致:给了但非字符串 → 报错(不静默丢弃——调用方传错类型
                // 却静默无版本,会让 needs_version 配方以"配方未给 version"失败,难排查)
                Some(
                    v.as_string()
                        .map(|s| s.to_std_string_escaped())
                        .ok_or_else(|| {
                            crate::js_err(format!(
                                "helix.server.task: item {i} 'version' must be a string"
                            ))
                        })?,
                )
            }
        };
        items.push(ServerTaskItem { op, name, version });
    }
    let task_id = next_task_id();
    crate::permissions::bind_callback(crate::permissions::CbKey::server_task(task_id));
    // 登记归属宿主：worker 回推事件时据此找回正确的事件桶（此刻还在提交方线程）
    register_task_host(task_id);
    with_callbacks(|m| {
        m.insert(task_id, cb);
    });
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::ServerTask(ServerTaskReq { task_id, items }));
    Ok(JsValue::from(task_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{set_term_wake, take_ui_requests, with_engine};
    use crate::types::ServerTaskEvent;
    use boa_engine::{object::builtins::JsArray, JsString, Source};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// 模拟主线程泵：worker(term) 推入的事件 drain + resolve 到回调
    fn deliver_event(ev: ServerTaskEvent) {
        push_event(ev);
        resolve_events().unwrap();
    }

    /// M-e:version 字段给了但非字符串 → Err(与 op/name 同款校验;不静默丢弃)
    #[test]
    fn task_version_must_be_string_or_absent() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        with_engine(|ctx| {
            let cb = ctx.eval(Source::from_bytes("() => {}")).unwrap();
            // 逐 case eval 数组字面量(避免借用纠缠),验 version 缺失/字符串/非字符串
            for (src, expect_ok) in [
                ("[{op:'install',name:'demo'}]", true),
                ("[{op:'install',name:'demo',version:'1.0.0'}]", true),
                ("[{op:'install',name:'demo',version:5}]", false),
                ("[{op:'install',name:'demo',version:true}]", false),
            ] {
                let arr = ctx.eval(Source::from_bytes(src.as_bytes())).unwrap();
                let r = js_server_task(&JsValue::undefined(), &[arr, cb.clone()], ctx);
                assert_eq!(
                    r.is_ok(),
                    expect_ok,
                    "{src}: is_ok 应={expect_ok}, got {:?}",
                    r.map(|_| ())
                );
                if !expect_ok {
                    let e = r.unwrap_err().to_string();
                    assert!(e.contains("version' must be a string"), "{e}");
                }
            }
        });
    }

    /// I1:worker 推事件必须触发宿主注册的 term wake(→ request_redraw → 主循环重绘)。
    /// 无此唤醒时 progress/done 只在下一次按键/单发 idle 才 resolve(长下载全程冻结)。
    /// push_event 走 WakeSender 通道(set_term_wake 可重注册,与顺序无关)。
    #[test]
    fn push_event_fires_registered_wake() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        let fired = Arc::new(AtomicBool::new(false));
        let fired2 = fired.clone();
        set_term_wake(Box::new(move || {
            fired2.store(true, Ordering::SeqCst);
        }));
        push_event(ServerTaskEvent {
            task_id: 4242,
            seq: 0,
            name: "demo".into(),
            kind: "phase".into(),
            msg: None,
            bytes: None,
            total: None,
        });
        assert!(
            fired.load(Ordering::SeqCst),
            "server task 事件推送须触发注册的 term wake(主循环重绘源)"
        );
        drain_events(); // 清残留事件防串扰(无回调,resolve 本就静默跳过)
    }

    /// 参数校验：首参非数组 / 回调不可调 / 空数组 → Err（不消耗 taskId）
    #[test]
    fn task_validates_args() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
                crate::types::UiRequest::ServerTask(ServerTaskReq { task_id, items }) => {
                    Some((task_id, items))
                }
                _ => None,
            })
            .expect("入队 UiRequest::ServerTask");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].op, "install");
        assert_eq!(items[0].name, "demo");
        // task_id 进程内全局递增(next_task_id):**不**假定"首个一定是 1" ——
        // 同一进程里别的测试可能已提交过任务(故只钉"≥1 且递增";唯一性由
        // task_events_are_not_stolen_across_hosts 用两次提交的差集守着)。
        assert!(tid >= 1, "首个 taskId 从 1 起(与 next_task_id 一致)");
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
            format!("id:{tid} ev:progress:demo:10 ev:done:demo:0"),
            "JS 侧看到的 id 必须与入队的 task_id 一致(task_id 现在进程内全局递增)"
        );
    }

    /// **跨宿主隔离(回归)**:A 宿主提交的 task 事件不得被 B 宿主的 `drain_events()` 取走。
    ///
    /// 原实现是**进程级单通道**(注释写着"编辑器单线程运行,无并发互偷"):全并行集成跑
    /// 里同一进程有多个 `Application`,**任何**宿主的 `resolve_events()` 都会先把别人的
    /// 事件 drain 走,而它的 `CALLBACKS` 在别的线程 ⇒ 事件被"无回调,静默跳过"丢掉。
    /// 实测症状:arsenal 批量安装卡在 `0% (0/2)` 而 `pending_jobs()==0`(job 已跑完)。
    ///
    /// 断言的是**身份**(drain 到的 task_id 必须是自己那条),不只数量 —— 数量相等
    /// 但内容是别人的,正是这个 bug 的形状。A 的事件**故意留着不 drain** 再切到 B,
    /// 否则"无事件可偷"也会绿(第一版就是这么写的:变异实验把两个宿主合并成同一个桶,
    /// 测试照样绿 ⇒ 假绿,已改)。
    #[test]
    fn task_events_are_not_stolen_across_hosts() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        // A = 本线程默认宿主。两个 id 必须不同:路由表是 task_id → 宿主,
        // 两个宿主同时用同一个 id 会把事件投错桶(静默错投)。
        let a1 = next_task_id();
        register_task_host(a1);
        let a2 = next_task_id();
        register_task_host(a2);
        assert_ne!(a1, a2, "task_id 必须进程内唯一(task_id→宿主 路由表的前提)");
        push_event(ServerTaskEvent {
            task_id: a1,
            seq: 0,
            name: "a".into(),
            kind: "progress".into(),
            msg: None,
            bytes: None,
            total: None,
        });

        // B = 同线程另一个宿主(既有测试同款做法:host.rs 的 two_hosts_are_isolated)
        let mut other = crate::host::JsHost::new();
        crate::host::set_active(&mut other);
        let b1 = next_task_id();
        register_task_host(b1);
        push_event(ServerTaskEvent {
            task_id: b1,
            seq: 0,
            name: "b".into(),
            kind: "progress".into(),
            msg: None,
            bytes: None,
            total: None,
        });
        let seen_b: Vec<u64> = drain_events().iter().map(|e| e.task_id).collect();
        assert_eq!(
            seen_b,
            vec![b1],
            "B 只能取到自己的事件(旧实现会把 A 的 a1 一起取走;共用同一桶时同理)"
        );

        crate::host::clear_active();
        // 回到 A:A 的事件**一直没被动过**(没被 B 吃掉,而是留给了真正的主人)
        let seen_a: Vec<u64> = drain_events().iter().map(|e| e.task_id).collect();
        assert_eq!(seen_a, vec![a1], "切回 A 后 A 的桶仍完好");
        let _ = a2; // 只为见证 id 唯一性,不参与投递
        std::mem::forget(other); // 引擎不可 drop
    }
}
