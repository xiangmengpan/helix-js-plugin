# 09 · 深入 helix-lsp：LSP 客户端

> `helix-lsp` 实现语言服务器协议（LSP）客户端：启动/管理语言服务器进程、发送请求与
> 通知、接收并分发服务器消息。`helix-lsp-types` 提供协议类型定义。
> 主线：`helix-lsp/src/lib.rs`（Registry）、`client.rs`（Client）、`transport.rs`（通信）、
> `jsonrpc.rs`（JSON-RPC 类型）。

## 核心概念

- **每文档可关联多个语言服务器**，每个服务器一个 `Client`（一个子进程 + 通信管道）；
- **请求-响应**：编辑器发请求带自增 id，Transport 层把响应按 id 路由回等待者；
- **通知**：单向消息（如 didChange、诊断），不需响应；
- **能力协商**：初始化握手后缓存 `ServerCapabilities`，发请求前先检查服务器是否支持
  （`supports_feature` / 各 `xxx_provider` 字段）。

## 1. 类型层：helix-lsp-types

`helix-lsp/src/lib.rs` 顶部：

```rust
pub use helix_lsp_types as lsp;   // 协议类型统一以 lsp:: 前缀使用
```

`helix-lsp-types/src/lib.rs` 是完整的 LSP 协议类型定义（基于
[LSP 规范](https://microsoft.github.io/language-server-protocol/specification)），
包含：

- **请求/通知结构**：`request::GotoDefinition`、`request::Hover`、
  `notification::DidChangeTextDocument` 等，每个类型带 `Params`/`Result` 关联类型
  并实现 serde 序列化（方法名 `const METHOD`）；
- **基础类型**：`Position`、`Range`、`TextDocumentIdentifier`、`Diagnostic`、
  `ServerCapabilities`、`ClientCapabilities` 等；
- **枚举风格**：协议里的联合类型用 `OneOf<A, B>`（如 `GotoDefinitionResponse`）；
- 工具：`Position` 与 char 索引互转在 `helix-lsp/src/lib.rs` 的 `util` 模块
  （`lsp_range_to_range`、`range_to_lsp_range`，注意 LSP 用 UTF-16 偏移，见
  `Client::offset_encoding`）。

## 2. JSON-RPC 层：jsonrpc.rs

```rust
pub enum Call {                    // 服务器 → 客户端的消息
    MethodCall(MethodCall),        // 服务器主动请求（如 workspace/applyEdit）
    Notification(Notification),    // 服务器通知（如 publishDiagnostics）
    Invalid { id: Id },            // 无法解析的消息（带 id 便于调试）
}
```

`Request` / `Notification` 枚举覆盖所有 LSP 方法（每个变体携带对应 Params）。
`Payload` 是客户端的消息载体（请求/通知/响应/错误）。

## 3. Transport：进程通信（transport.rs）

`Transport::start` 接收服务器子进程的 stdin/stdout/stderr，启动三个 tokio 任务：

| 任务 | 职责 |
|------|------|
| `recv` | 读 stdout：解析 LSP 消息帧（`Content-Length` 头 + JSON body），产出 `Call`，经 channel 发给 `Registry.incoming` |
| `send` | 写 stdin：消费发送队列，处理注入请求；检测到 `exit` 已 flush 时通知 `shutdown_flushed` |
| `err` | 读 stderr：服务器日志转发到 Helix 日志 |

```rust
pub struct Transport {
    pending_requests: Mutex<HashMap<jsonrpc::Id, Sender<Result<Value>>>>, // 等待响应的请求
    ...
}
```

响应到达时按 `id` 查 `pending_requests`，把结果发给等待方（oneshot channel），
从而把"异步收到的响应"与"发起请求处的 future"对接起来。

## 4. Client：一个语言服务器（client.rs）

```rust
pub struct Client {
    id: LanguageServerId,
    name: String,
    _process: Child,                       // 服务器子进程句柄
    server_tx: UnboundedSender<Payload>,   // 发给服务器的消息
    request_counter: AtomicU64,            // 请求 id 自增
    capabilities: OnceCell<lsp::ServerCapabilities>, // 初始化后缓存
    root_path: PathBuf,
    workspace_folders: Mutex<Vec<lsp::WorkspaceFolder>>,
    req_timeout: u64,
}
```

### 生命周期

1. **启动**：`Client::start`（`client.rs:211`）spawn 子进程、建 Transport、发
   `initialize` 请求、等待 `initialized` 通知，完成**初始化握手**；
2. **使用**：文档打开/变更/保存/关闭时发对应通知
   （`text_document_did_open` / `did_change` / `did_save` / `did_close`）；
3. **关闭**：`shutdown()` 发 `shutdown` 请求 → `exit()` 发 `exit` 通知 → 等 flush
   （`wait_shutdown_flushed`），超时则 `force_shutdown` 杀进程。

### 请求 API（返回 future）

所有"问服务器要数据"的操作返回 `Option<impl Future>`，**服务器不支持时返回 None**：

```rust
pub fn goto_definition(&self, text_document, position, work_done_token)
    -> Option<impl Future<Output = Result<Option<GotoDefinitionResponse>>>> {
    let capabilities = self.capabilities.get().unwrap();
    match capabilities.definition_provider {
        Some(OneOf::Left(true) | OneOf::Right(_)) => (),
        _ => return None,   // 服务器不支持 → 编辑器回退到本地跳转
    }
    Some(self.goto_request::<lsp::request::GotoDefinition>(...))
}
```

同类方法：`text_document_hover`、`text_document_completion`、`text_document_document_highlight`、
`text_document_formatting`、`text_document_signature_help`、`text_document_diagnostic` 等。

### 通知 API

```rust
pub fn notify<R: lsp::notification::Notification>(&self, params: R::Params)
// 例：Client::notify::<lsp::notification::DidChangeConfiguration>(...)
```

### 同步（document 同步）

- `text_document_did_open` / `did_close`：打开/关闭时全量文本；
- `text_document_did_change`（`client.rs:1081`）：增量同步——把 ChangeSet 转成
  LSP 的 `TextDocumentContentChangeEvent`（`changeset_to_changes`，`client.rs:971`）；
- `offset_encoding`（`client.rs:413`）：UTF-8/UTF-16 编码协商（`initialize` 时
  通过 `positionEncodings` 能力）。

## 5. Registry：管理多个服务器（lib.rs）

```rust
pub struct Registry {
    inner: SlotMap<LanguageServerId, Arc<Client>>,
    inner_by_name: HashMap<LanguageServerName, Vec<Arc<Client>>>, // 同名多实例（多 workspace）
    syn_loader: Arc<ArcSwap<syntax::Loader>>,
    pub incoming: SelectAll<UnboundedReceiverStream<(LanguageServerId, Call)>>, // 全部服务器的入站消息
    pub file_event_handler: file_event::Handler,  // 文件系统事件（watcher）
}
```

- `Editor.language_servers` 就是 `Registry`（`helix-view/src/editor.rs`）；
- 服务器按需启动：打开文档时 `Registry::start_client`（`lib.rs:625`），语言配置里的
  `language-server` 命令 spawn 进程；
- 同名服务器可有多个实例（不同 workspace 根），`inner_by_name` 管理；
- 入站消息汇聚到 `incoming` 流 → `EditorEvent::LanguageServerMessage` →
  `application.rs::handle_language_server_message`（见
  [07-deep-view.md](./07-deep-view.md) 第 7 节）。

## 6. 请求-响应完整链路

```
编辑器命令（如 gd 跳转定义）
 └─ doc.language_servers → Client::goto_definition(...) → Future
     └─ goto_request：构造 JSON-RPC 请求（自增 id）→ server_tx → Transport.send → 服务器 stdin
     └─ 同时把 id 注册进 Transport.pending_requests
服务器处理 → stdout 响应
 └─ Transport.recv 解析 → 按 id 查 pending_requests → oneshot 送达 future
 └─ 命令代码用 cx.callback(future, |editor, compositor, result| ...)
     └─ job 完成 → 主循环执行回调（跳转、高亮等）
```

## 7. 服务器主动消息

服务器可主动发请求/通知（`Call`），客户端处理（`application.rs` 的
`handle_language_server_message`）：

| 类型 | 示例 | Helix 处理 |
|------|------|-----------|
| 通知 | `publishDiagnostics` | 更新 `handlers.diagnostics` → 重绘 |
| 通知 | `window/workDoneProgress` | `LspProgressMap` 进度跟踪 |
| 通知 | `textDocument/publishDiagnostics` | 诊断缓存 |
| 请求 | `workspace/applyEdit` | 把 LSP 编辑转成 Transaction 应用 |
| 请求 | `window/showMessageRequest` | 弹提示框 |

## 8. 代码位置指引

| 主题 | 位置 |
|------|------|
| 客户端 | `helix-lsp/src/client.rs` |
| 服务器管理 | `helix-lsp/src/lib.rs`（Registry） |
| 通信 | `helix-lsp/src/transport.rs` |
| JSON-RPC 类型 | `helix-lsp/src/jsonrpc.rs` |
| 文件系统事件 | `helix-lsp/src/file_event.rs`、`file_operations.rs` |
| 坐标转换工具 | `helix-lsp/src/lib.rs`（`pub mod util`） |
| 协议类型 | `helix-lsp-types/src/lib.rs` |
| 消费方 | `helix-view/src/handlers/lsp.rs`、`helix-term/src/application.rs` |

> 相关教程：请求如何发起与回调（[04-tutorial-edit.md](./04-tutorial-edit.md) 第 7 节）、
> 异步任务调度（[08-deep-term.md](./08-deep-term.md) 第 5 节）。
