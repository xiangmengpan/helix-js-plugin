# 设计:JS LSP 请求 API(A4)

日期:2026-08-26
状态:已批准(分节讨论确认)

## 背景与目标

JS 插件目前对 LSP 只有只读的诊断快照(`helix.diagnostics()` + `lsp-diagnostics` 事件)。插件无法主动向语言服务器发请求,写不了补全弹窗、go-to-def、符号大纲这三类高价值插件。本设计为插件增加主动 LSP 请求能力。

**首批方法范围(用户选定 B):**

| 方法 | LSP 请求 | 返回 |
|---|---|---|
| `helix.lsp.hover()` | `textDocument/hover` | `Hover \| null` |
| `helix.lsp.completion()` | `textDocument/completion` | `CompletionItem[] \| null` |
| `helix.lsp.goto_definition()` | `textDocument/definition` | `Location[] \| null` |
| `helix.lsp.document_symbols()` | `textDocument/documentSymbol` | `DocumentSymbol[] \| null` |

后续方法(rename/formatting/code_actions/references 等)是机械模板,按需补。

## API 形状

```js
// 全部位于 helix.lsp 命名空间,全部返回 Promise
const hover = await helix.lsp.hover();              // → Hover | null
const items = await helix.lsp.completion();         // → CompletionItem[] | null
const locs  = await helix.lsp.goto_definition();    // → Location[] | null
const syms  = await helix.lsp.document_symbols();   // → DocumentSymbol[] | null

// 位置覆盖(可选,默认当前 buffer + 光标,调用时快照)
await helix.lsp.hover({ row, col });
```

### 数据面:透传原始 LSP JSON

- 响应由 `serde_json::to_value` 序列化后经 JSON.parse 原样给 JS,字段名保持 LSP 协议名(`contents`/`items`/`label`/`detail`/`children`/`selectionRange` 等)
- **唯一便利字段**:`goto_definition` 返回的每个 `Location` 附 `path`(由 `uri` 解析,`file://` 前缀剥除,URI percent-decoding 做基础处理)。插件跳转/显示直接用 `path`,不必自己解析
- `completion` 返回的 `CompletionItem` 是**完整协议对象**(含 `insertText`/`snippet`/`documentation`/`data` 回传字段),零丢失——后续 completion 联动浮层(方案 B)依赖完整 item 做插入

### 位置语义

- 字符坐标 (row, col),与 `set_cursor` 一致;缺省 = 调用时当前光标位置快照(用户移动光标不影响已发出请求的结果)
- Rust 侧按 server 的 offset_encoding 转换(复用 helix `doc.position(view_id, offset_encoding)` 现成函数)
- buffer 始终是当前的,不做跨 buffer 请求(P0 范围外)

### 空/错语义

| 情形 | 行为 |
|---|---|
| 无 LSP server(文件类型无配置/server 未启动) | `resolve(null)` |
| server 不支持该功能(`supports_feature` 检查) | `resolve(null)` |
| server 返回错误响应(LSP 协议 error) | `reject(Error(message))` |
| 连接断开/请求超时前异常 | `reject(Error(message))` |
| 请求挂起不返回 | Promise 悬挂(P0 无超时,文档注明) |

"没结果"是插件常态(文件类型不匹配、功能未启用),不该触发异常路径;只有真正出错的调用才 reject,插件只需为"发请求"兜底。

## 异步桥接架构

```
JS 插件                          helix-js (主线程)              helix-term (tokio runtime)
─────                          ─────────────────             ────────────────────────
await helix.lsp.hover()
  → 入队 LspRequest{id, method,   │
     row, col 快照} + 存 Promise   │
        ────────────────────────> 每帧泵 take_lsp_requests()
                                   │ 当前 editor 找 language server
                                   │ (buffer 语言匹配, supports_feature 检查)
                                   │ spawn future(hover 等)
                                   │ 完成 → serde_json 序列化 → 响应 channel
  resolve/reject Promise <──────── 每帧泵 drain_lsp_responses(id 匹配)
```

- 与 `run_async` 管道同构(JS 发 → Rust 执行 → 结果回),区别:执行在 tokio runtime(helix-lsp 客户端是 async),不在 std thread
- **泵点**:helix-term `application.rs` 现有泵循环加 `take_lsp_requests()` 处理段(与 `take_edits`/`take_messages`/`drain_async_events` 并列);响应经现有 `wake_sender` 机制唤醒 JS 泵
- **Promise 管理**:请求入队分配自增 id,`helix-js` 存 `id → (resolve, reject)`;响应按 id 分发。P0 不做超时(悬挂风险文档注明)
- **无 server 短路**:泵到请求时发现无 server/不支持 → 直接回 `null` 响应,不进 future(零异步开销)

### helix-term 侧请求处理(伪码)

```rust
// application.rs 泵循环内
for req in helix_js::take_lsp_requests() {
    let (server, doc) = /* 当前 buffer 的 language server */;
    let Some(server) = server.filter(|s| s.supports_feature(req.method)) else {
        helix_js::resolve_lsp(req.id, None);   // null 响应
        continue;
    };
    let pos = doc.position(view_id, server.offset_encoding());  // 或 req 覆盖位置
    let future = match req.method {
        Method::Hover => server.text_document_hover(doc.identifier(), pos, None),
        // ... 四方法
    };
    tokio::spawn(async move {
        let out = match future.await {
            Ok(Ok(v)) => serde_json::to_value(v).ok(),
            _ => ErrPayload,   // error → reject 路径
        };
        helix_js::resolve_lsp(req.id, out);
    });
}
```

> 注:go-to-def 的 `path` 便利字段在序列化前注入(遍历 Location,`uri` → `path`)。

## open_file 扩展

```js
// 现有:只打开
helix.open_file("/path/to/x.rs");

// 扩展:打开并定位(第二参数可选)
helix.open_file("/path/to/x.rs", { row: 10, col: 4 });

// goto_definition 插件用法
const locs = await helix.lsp.goto_definition();
if (locs?.length) {
  const l = locs[0];
  helix.open_file(l.path, { row: l.range.start.row, col: l.range.start.character });
}
```

- `UiRequest::OpenFile` 加可选 `row/col`(字符坐标);helix-term 处理时带行列则打开后跳转,复用 helix 现有打开+定位逻辑(`ensure_cursor_in_view` 语义)
- 文件已打开 → 跳到已有 buffer 视图并定位,不重复开 buffer
- 不传第二参数行为与现在完全一致(向后兼容)

## 测试策略

- **helix-js 单测**:
  - 请求入队形状(方法/id/位置快照正确)
  - 响应分发:按 id resolve 正确形状(hover 对象 / 数组 / null);reject 带 message
  - `path` 解析(`file://` 前缀剥离、percent-decode)
- **helix-term 集成测试**(`tests/test/`):
  - 无 server 短路全链路:无 LSP 文件 → 插件 `helix.lsp.hover()` → resolve null
  - `open_file` 带行列:打开后定位到指定行(现有 popup/panel harness)
- 真 LSP 数据请求不进自动化(需真实 server):手动验证 + demo 插件
- **demo 插件**:`plugins/features/lsp-hover/index.js`——`:lsp-hover` 命令,光标处请求 hover,弹窗渲染内容;go-to-def 跳转示例

## 文档

`docs/plugin-api.md` 新增 "LSP 请求" 章节:4 方法签名、返回字段表(指向 LSP 协议)、空/错语义表、示例插件代码。

## 范围外(明确不做)

- completion 输入时自动联动浮层(P0 第二项,单独设计;本设计只做"拿数据")
- 超时/取消机制(悬挂风险文档注明)
- 跨 buffer 请求、指定 server 选择
- 除四方法外的其他 LSP 方法(机械模板,按需补)
- Windows 路径细节(Linux 优先,percent-decode 基础处理)
