//! `helix-dap` 的测试接缝：**内存假 adapter**。
//!
//! 为什么重要：这个 crate 之前**零测试**，而"去 `block_on` / 多会话"都要改它的 async 路径 ——
//! 没有能断言"请求/响应/事件三路都通"的夹具，那些改动就只能靠肉眼看。
//!
//! 做法：`Client::streams(reader, writer, …)`（`client.rs`）本来就接受任意
//! `AsyncBufRead`/`AsyncWrite`，所以用两对 `tokio::io::duplex` 当管道，测试侧手写
//! **DAP 线格式**（`Content-Length: N\r\n\r\n<json>`，见 `transport.rs`）——不需要起进程、
//! 不需要真 adapter、CI 里不会缺 `lldb-dap`。
//!
//! 约定：`Client::call` 有 20s 硬超时；测试用 `tokio::time::timeout(2s)` 包住，
//! 免得"adapter 少回一条"变成 20 秒的红灯。

use crate::registry::DebugAdapterId;
use crate::{Client, Payload};
use serde_json::{json, Value};
use slotmap::SlotMap;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::mpsc::UnboundedReceiver;

/// 单条测试的超时（远小于 `Client::call` 的 20s，失败得看得见）
const T: Duration = Duration::from_secs(5);

/// 造一个 `DebugAdapterId`（`slotmap` 的 key 只能由 SlotMap 发）
fn adapter_id() -> DebugAdapterId {
    let mut sm: SlotMap<DebugAdapterId, ()> = SlotMap::with_key();
    sm.insert(())
}

/// 假 adapter 的两端：`read` = adapter 读客户端发来的帧，`write` = adapter 回帧。
struct Fake {
    read: BufReader<DuplexStream>,
    write: DuplexStream,
}

/// 起一个内存 adapter。返回（客户端，事件流，adapter 端）。
fn fake() -> (Client, UnboundedReceiver<(DebugAdapterId, Payload)>, Fake) {
    let (to_adapter, from_client) = tokio::io::duplex(64 * 1024);
    let (to_client, from_adapter) = tokio::io::duplex(64 * 1024);
    let id = adapter_id();
    let (client, rx) = Client::streams(
        Box::new(BufReader::new(from_adapter)),
        Box::new(to_adapter),
        None,
        id,
        None,
    )
    .expect("streams should accept duplex pipes");
    (
        client,
        rx,
        Fake {
            read: BufReader::new(from_client),
            write: to_client,
        },
    )
}

impl Fake {
    /// 读一帧（头 + 体）；`None` = 对端关了。
    async fn recv(&mut self) -> Option<Value> {
        let mut len: Option<usize> = None;
        let mut line = String::new();
        loop {
            line.clear();
            if self.read.read_line(&mut line).await.ok()? == 0 {
                return None;
            }
            if line == "\r\n" {
                break;
            }
            if let Some(rest) = line.strip_prefix("Content-Length: ") {
                len = rest.trim().parse().ok();
            }
        }
        let mut body = vec![0u8; len?];
        self.read.read_exact(&mut body).await.ok()?;
        serde_json::from_slice(&body).ok()
    }

    /// 回一帧。
    async fn send(&mut self, v: Value) {
        let body = serde_json::to_string(&v).unwrap();
        self.write
            .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
            .await
            .unwrap();
        self.write.write_all(body.as_bytes()).await.unwrap();
        self.write.flush().await.unwrap();
    }

    /// 回一个**成功响应**（`request_seq` 从收到的请求里抄）。
    async fn ok(&mut self, req: &Value, body: Value) {
        self.send(json!({
            "type": "response",
            "request_seq": req["seq"],
            "success": true,
            "command": req["command"],
            "body": body,
        }))
        .await;
    }

    /// 回一个**失败响应**。
    async fn fail(&mut self, req: &Value, message: &str, body: Value) {
        self.send(json!({
            "type": "response",
            "request_seq": req["seq"],
            "success": false,
            "command": req["command"],
            "message": message,
            "body": body,
        }))
        .await;
    }

    /// 推一个事件（不等请求）。
    async fn event(&mut self, event: &str, body: Value) {
        self.send(json!({ "type": "event", "event": event, "body": body }))
            .await;
    }
}

/// `initialize` → 响应 `capabilities` → 回填到 `client.caps`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_fills_capabilities() {
    let (mut client, _rx, mut fake) = fake();
    let (res, _) = tokio::join!(
        async {
            tokio::time::timeout(T, client.initialize("fake-adapter".to_string()))
                .await
                .expect("initialize should not hang")
        },
        async {
            let req = fake
                .recv()
                .await
                .expect("adapter should receive initialize");
            assert_eq!(req["command"], "initialize");
            assert_eq!(
                req["arguments"]["adapterID"], "fake-adapter",
                "adapterID 要进载荷（多 adapter 时靠它区分）"
            );
            // 字段名是 DAP 规范的 `linesStartAt1`（Rust 侧叫 `lines_start_at_one`，
            // 名字与 JSON 名不同 —— 断言里用线上名字，别跟着 Rust 字段名写）
            assert_eq!(
                req["arguments"]["linesStartAt1"], true,
                "我们声明 1 起始 ⇒ BreakpointStore 的换算依赖这条"
            );
            fake.ok(
                &req,
                json!({"supportsConfigurationDoneRequest": true, "supportsConditionalBreakpoints": true}),
            )
            .await;
        }
    );
    res.expect("initialize ok");
    let caps = client.capabilities();
    assert_eq!(caps.supports_conditional_breakpoints, Some(true));
    assert_eq!(caps.supports_configuration_done_request, Some(true));
}

/// `setBreakpoints` 往返：请求里的行号是 1 起始，响应回填 adapter 的 `id/verified`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_breakpoints_roundtrip() {
    let (client, _rx, mut fake) = fake();
    let path = std::path::PathBuf::from("/tmp/a.rs");
    let (res, _) = tokio::join!(
        async {
            tokio::time::timeout(
                T,
                client.set_breakpoints(
                    path.clone(),
                    vec![
                        crate::SourceBreakpoint {
                            line: 4,
                            column: None,
                            condition: Some("x > 1".into()),
                            hit_condition: None,
                            log_message: None,
                        },
                        crate::SourceBreakpoint {
                            line: 9,
                            column: None,
                            condition: None,
                            hit_condition: None,
                            log_message: None,
                        },
                    ],
                ),
            )
            .await
            .expect("set_breakpoints should not hang")
        },
        async {
            let req = fake
                .recv()
                .await
                .expect("adapter should receive setBreakpoints");
            assert_eq!(req["command"], "setBreakpoints");
            assert_eq!(req["arguments"]["source"]["path"], "/tmp/a.rs");
            assert_eq!(req["arguments"]["breakpoints"][0]["line"], 4);
            assert_eq!(req["arguments"]["breakpoints"][0]["condition"], "x > 1");
            fake.ok(
                &req,
                json!({"breakpoints": [
                    {"id": 7, "verified": true, "line": 4},
                    {"id": 8, "verified": false, "line": 9, "message": "no executable"}
                ]}),
            )
            .await;
        }
    );
    let bps = res
        .expect("setBreakpoints ok")
        .expect("body.breakpoints 应有值");
    assert_eq!(bps.len(), 2);
    assert_eq!(bps[0].id, Some(7));
    assert!(bps[0].verified);
    assert_eq!(bps[1].message.as_deref(), Some("no executable"));
}

/// 事件流：`stopped` 事件能到消费方，并能被 `Event::parse` 解析
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_reach_consumer_and_parse() {
    let (_client, mut rx, mut fake) = fake();
    fake.event(
        "stopped",
        json!({"reason": "breakpoint", "threadId": 1, "allThreadsStopped": true}),
    )
    .await;
    let (_id, payload) = tokio::time::timeout(T, rx.recv())
        .await
        .expect("事件不该超时")
        .expect("事件流不该关");
    let Payload::Event(ev) = payload else {
        panic!("期望 Event, 拿到 {payload:?}");
    };
    assert_eq!(ev.event, "stopped");
    match crate::Event::parse(&ev.event, ev.body).expect("stopped 可解析") {
        crate::Event::Stopped(body) => {
            // 注意 `reason` 是 String（DAP 里 reason 必填），`thread_id` 是 newtype
            assert_eq!(body.reason, "breakpoint");
            assert_eq!(body.thread_id.map(|t| t.to_string()).as_deref(), Some("1"));
            assert_eq!(body.all_threads_stopped, Some(true));
        }
        other => panic!("解析成 {other:?}"),
    }
}

/// 反向请求（adapter → 客户端，如 `runInTerminal`）能到消费方；`reply` 能回过去
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reverse_request_and_reply() {
    let (client, mut rx, mut fake) = fake();
    fake.send(json!({
        "type": "request",
        "seq": 41,
        "command": "runInTerminal",
        "arguments": {"args": ["/bin/true"], "cwd": "/tmp"},
    }))
    .await;
    let (_id, payload) = tokio::time::timeout(T, rx.recv())
        .await
        .expect("反向请求不该超时")
        .expect("事件流不该关");
    let Payload::Request(req) = payload else {
        panic!("期望 Request, 拿到 {payload:?}");
    };
    assert_eq!(req.command, "runInTerminal");
    assert_eq!(req.seq, 41);
    // 回执
    let (_res, _) = tokio::join!(
        async {
            tokio::time::timeout(T, client.reply(41, "runInTerminal", Ok(json!({}))))
                .await
                .expect("reply 不该超时")
        },
        async {
            let resp = fake.recv().await.expect("adapter 应收到回执");
            assert_eq!(resp["type"], "response");
            assert_eq!(resp["request_seq"], 41);
            assert_eq!(resp["success"], true);
        }
    );
}

/// 失败响应必须**变成错误**（不是被当成空 body 吃掉）—— 这条钉住 `f37bee941` 的行为
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_response_surfaces_as_error() {
    let (client, _rx, mut fake) = fake();
    let (res, _) = tokio::join!(
        async {
            tokio::time::timeout(
                T,
                client.call::<crate::requests::Threads>(Some(crate::requests::ThreadsArguments {})),
            )
            .await
            .expect("call 不该超时")
        },
        async {
            let req = fake.recv().await.expect("adapter 应收到 threads");
            assert_eq!(req["command"], "threads");
            fake.fail(&req, "not supported", json!({"error": "no threads here"}))
                .await;
        }
    );
    let err = res.expect_err("失败响应必须是 Err");
    let msg = err.to_string();
    assert!(
        msg.contains("no threads here"),
        "错误里要带 adapter 给的 body（现在 transport 用 body 组错误串）: {msg}"
    );
}

/// 线格式自检：`Transport` 写出来的帧长与头一致（假 adapter 依赖这条才能解析）
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_format_has_content_length_header() {
    let (client, _rx, mut fake) = fake();
    let (_, _) = tokio::join!(
        async {
            let _ = tokio::time::timeout(
                T,
                client.call::<crate::requests::Threads>(Some(crate::requests::ThreadsArguments {})),
            )
            .await;
        },
        async {
            // 自己解析一遍头，断言 Content-Length 与体长一致
            let mut len: Option<usize> = None;
            let mut line = String::new();
            loop {
                line.clear();
                fake.read.read_line(&mut line).await.unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(rest) = line.strip_prefix("Content-Length: ") {
                    len = rest.trim().parse().ok();
                }
            }
            let len = len.expect("必须有 Content-Length 头");
            let mut body = vec![0u8; len];
            fake.read.read_exact(&mut body).await.unwrap();
            let v: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(v["type"], "request");
            assert_eq!(v["seq"], 1, "seq 从 1 开始（DAP 规范：首条消息 seq = 1）");
            // 回一条，别让 `call` 干等 5s 超时（那样这条测试会白占 5 秒）
            fake.ok(&v, json!({"threads": []})).await;
        }
    );
}

/// `Transport` 的进度事件包装：`progress_start/update/end` 三连（状态栏文本用）
#[test]
fn progress_state_text() {
    let mut client_progress = crate::ProgressState::new("Indexing".into(), None, None);
    assert_eq!(client_progress.status_line(), "Debug: Indexing");
    client_progress.update(Some("src/a.rs".into()), Some(42));
    assert_eq!(
        client_progress.status_line(),
        "Debug: Indexing - src/a.rs (42%)"
    );
    assert_eq!(
        client_progress.end_status_line(None),
        "Debug: Indexing - src/a.rs"
    );
    assert_eq!(
        crate::ProgressState::new("Build".into(), None, None).end_status_line(None),
        "Debug: Build finished"
    );
}

/// `DapHandle` 是"请求句柄"：`Clone` 出来的两份共享同一个序号计数器与同一个发送通道
/// （命令层把请求丢进后台任务就靠这个——克隆出去之后不再借 `Client`）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dap_handle_clone_shares_channel_and_seq() {
    let (client, _rx, mut fake) = fake();
    let a = client.handle();
    let b = a.clone();
    assert_eq!(a.id(), b.id());

    let (fut_a, fut_b, _) = tokio::join!(
        async {
            tokio::time::timeout(
                T,
                a.call::<crate::requests::Threads>(Some(crate::requests::ThreadsArguments {})),
            )
            .await
            .expect("a 不超时")
        },
        async {
            tokio::time::timeout(
                T,
                b.call::<crate::requests::Threads>(Some(crate::requests::ThreadsArguments {})),
            )
            .await
            .expect("b 不超时")
        },
        async {
            // 两个请求必须带**不同**的 seq（同一个计数器），并按序到达 adapter
            let first = fake.recv().await.expect("第一个请求");
            let second = fake.recv().await.expect("第二个请求");
            assert_ne!(
                first["seq"], second["seq"],
                "seq 必须唯一：{first} / {second}"
            );
            fake.ok(&first, json!({"threads": []})).await;
            fake.ok(&second, json!({"threads": []})).await;
        }
    );
    fut_a.expect("a ok");
    fut_b.expect("b ok");
}

/// `send_only`（fire-and-forget）：非阻塞发出、`server_tx` 不关、**不会**让事件流 panic。
///
/// 这条测试盯的是一个真崩过的位置：没有回包消费方时，transport 以前会把 `Payload::Response`
/// 往前推，而 `Client::recv` 里那一支是 `unreachable!()`。现在那边丢弃、这边也改成跳过。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_only_does_not_poison_event_stream() {
    let (client, mut rx, mut fake) = fake();
    let handle = client.handle();
    handle
        .send_only::<crate::requests::Pause>(crate::requests::PauseArguments {
            thread_id: fake_thread_id(1),
        })
        .expect("入队即可（不阻塞）");

    // adapter 照常回包 —— 这个回包没有消费方，必须被安静丢掉
    let req = fake.recv().await.expect("adapter 应收到 pause");
    assert_eq!(req["command"], "pause");
    fake.ok(&req, json!({})).await;

    // 随后压一个事件：如果回包把事件流弄崩了，这里就收不到
    fake.event("continued", json!({"threadId": 1})).await;
    let (_id, payload) = tokio::time::timeout(T, rx.recv())
        .await
        .expect("事件不该超时")
        .expect("事件流不该关");
    match payload {
        Payload::Event(ev) => assert_eq!(ev.event, "continued"),
        other => panic!("期望 continued 事件，拿到 {other:?}（回包应被丢弃，不该出现在这里）"),
    }
}

/// 造一个 `ThreadId`：`ThreadId` 是 newtype，序列化/反序列化走 DAP 协议
fn fake_thread_id(n: isize) -> crate::ThreadId {
    serde_json::from_value(json!(n)).expect("ThreadId 可从数字反序列化")
}
