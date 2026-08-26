# JS LSP 请求 API 实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 为 JS 插件增加 4 个主动 LSP 请求方法（`helix.lsp.hover/completion/goto_definition/document_symbols`），返回 Promise，透传 LSP 原始 JSON；顺带扩展 `open_file` 支持行列定位。

**架构：** JS 侧 `helix.lsp.*` 入队 `LspRequest{id, method, pos?}` + 注册 Promise → helix-term 泵循环 `take_lsp_requests` 找到当前 buffer 的 language server，`tokio::spawn` 发请求 → 结果 JSON 序列化后经 `LSP_RESULTS` 通道回主线程 → `resolve_lsp` 按 id 兑现 Promise。与现有 `run_async` 管道同构，执行端从 std thread 换成 tokio runtime。

**技术栈：** boa 0.21（JS 引擎）、helix-lsp（LSP 客户端）、tokio（请求 future）、serde_json（透传序列化）、helix-view（doc/selection/align_view）。

**规格：** `docs/superpowers/specs/2026-08-26-js-lsp-request-design.md`（已批准）

---

### 任务 1：helix-js — LSP 请求入队 + Promise 注册（API 面）

**文件：**
- 创建：`helix-js/src/lsp.rs`
- 修改：`helix-js/src/lib.rs`（注册 `helix.lsp` 命名空间对象）
- 修改：`helix-js/src/state.rs`（加 `with_lsp_promises`）
- 修改：`helix-js/src/shell.rs`（`init()` 里初始化 `LSP_RESULTS` 通道，与 ASYNC_EVENTS 并列——看任务 2）

- [ ] **步骤 1：编写失败的测试**（helix-js 单测，`helix-js/src/lsp.rs` 底部 `#[cfg(test)]` 模块，仿 `diagnostics.rs` 测试的 `TEST_LOCK` 模式）

```rust
#[test]
fn lsp_request_enqueue_shape() {
    let _guard = TEST_LOCK.lock().unwrap();
    crate::init();
    let mut ctx = Context::default();
    // 无参 → pos=None（helix-term 侧取当前光标）
    let p = js_lsp_hover(&JsValue::undefined(), &[], &mut ctx).unwrap();
    assert!(p.is_object(), "hover 应返回 Promise");
    let reqs = take_lsp_requests();
    assert_eq!(reqs.len(), 1);
    assert!(matches!(reqs[0].method, LspMethod::Hover));
    assert!(reqs[0].pos.is_none());
    // 带位置覆盖
    let pos = object! { row: 5, col: 3 };
    let _ = js_lsp_goto_definition(&JsValue::undefined(), &[pos.into()], &mut ctx).unwrap();
    let reqs = take_lsp_requests();
    assert_eq!(reqs[0].pos, Some((5, 3)));
    // id 自增
    assert_ne!(reqs[0].id, reqs[1].id);
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test -p helix-js lsp_request_enqueue_shape`
预期：FAIL（`js_lsp_hover` / `take_lsp_requests` 未定义）

- [ ] **步骤 3：实现 lsp.rs 核心（入队 + Promise 注册）**

```rust
// helix-js/src/lsp.rs
use boa_engine::builtins::promise::ResolvingFunctions;
use boa_engine::builtins::promise::JsPromise;
use boa_engine::{js_string, Context, JsError, JsString, JsValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspMethod { Hover, Completion, GotoDefinition, DocumentSymbols }

#[derive(Debug)]
pub struct LspRequest {
    pub id: u64,
    pub method: LspMethod,
    /// 字符坐标覆盖；None = 当前光标（helix-term 泵处理时取）
    pub pos: Option<(u16, u16)>,
}

thread_local! { static LSP_REQUESTS: RefCell<Vec<LspRequest>> = const { RefCell::new(Vec::new()) }; }

pub fn take_lsp_requests() -> Vec<LspRequest> {
    LSP_REQUESTS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// 通用入口：解析可选位置对象 → 入队 → 注册 Promise
fn enqueue_lsp_request(
    method: LspMethod,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let pos = match args.first() {
        Some(obj) if obj.is_object() => {
            let row = opt_u16(&obj.as_object().unwrap().get(js_string!("row"), context)?, context, "row")?;
            let col = opt_u16(&obj.as_object().unwrap().get(js_string!("col"), context)?, context, "col")?;
            Some((row, col))
        }
        _ => None,
    };
    let (promise, resolving) = JsPromise::new_pending(context);
    let id = crate::state::next_async_id();
    crate::state::with_lsp_promises(|m| m.insert(id, resolving));
    LSP_REQUESTS.with(|q| q.borrow_mut().push(LspRequest { id, method, pos }));
    Ok(promise.into())
}

pub(crate) fn js_lsp_hover(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    enqueue_lsp_request(LspMethod::Hover, args, ctx)
}
// 其余三个 js_lsp_completion / js_lsp_goto_definition / js_lsp_document_symbols 同款模板
```

（`opt_u16` 从 `crate::popup` 导入——它已是 pub(crate)。）

- [ ] **步骤 4：state.rs 加 promise 表**

```rust
// state.rs，仿 with_async_promises
thread_local! { static LSP_PROMISES: RefCell<HashMap<u64, ResolvingFunctions>> = const { RefCell::new(HashMap::new()) }; }
pub(crate) fn with_lsp_promises<T>(f: impl FnOnce(&mut HashMap<u64, ResolvingFunctions>) -> T) -> T {
    LSP_PROMISES.with(|m| f(&mut m.borrow_mut()))
}
```

- [ ] **步骤 5：lib.rs 注册 `helix.lsp` 命名空间**

```rust
// lib.rs 的 init() 注册段，在 builder.build() 之前（对象属性）
let lsp_obj = boa_engine::object::ObjectInitializer::new(&mut engine)
    .function(NativeFunction::from_fn_ptr(lsp::js_lsp_hover), JsString::from("hover"), 1)
    .function(NativeFunction::from_fn_ptr(lsp::js_lsp_completion), JsString::from("completion"), 1)
    .function(NativeFunction::from_fn_ptr(lsp::js_lsp_goto_definition), JsString::from("goto_definition"), 1)
    .function(NativeFunction::from_fn_ptr(lsp::js_lsp_document_symbols), JsString::from("document_symbols"), 1)
    .build();
builder.property("lsp", lsp_obj, Attribute::READONLY | Attribute::NON_ENUMERABLE);
```

（注意：注册段里 `engine` 与 `builder` 是否同时可变借用——参考现有 `builder.function(...)` 链，`ObjectInitializer::new(&mut engine)` 在 builder 构建前独立完成即可，先建对象再链到 builder。）

- [ ] **步骤 6：运行测试验证通过**

运行：`cargo test -p helix-js lsp_request_enqueue_shape`
预期：PASS

- [ ] **步骤 7：Commit**

```bash
git add helix-js/src/lsp.rs helix-js/src/lib.rs helix-js/src/state.rs
git commit -m "feat(js): helix.lsp.* 请求入队 + Promise 注册"
```

### 任务 2：helix-js — 响应分发（resolve/reject + Location path 注入）

**文件：**
- 修改：`helix-js/src/lsp.rs`（`resolve_lsp` / `drain_lsp_results` / `inject_location_paths`）
- 修改：`helix-js/src/state.rs`（`LSP_RESULTS` 通道静态 + init 初始化）

- [ ] **步骤 1：编写失败的测试**

```rust
#[test]
fn lsp_result_dispatch() {
    let _guard = TEST_LOCK.lock().unwrap();
    crate::init();
    let mut ctx = Context::default();
    // 先入队拿 id，手动 resolve null → .then 收到 null
    let p = js_lsp_hover(&JsValue::undefined(), &[], &mut ctx).unwrap();
    let id = take_lsp_requests()[0].id;
    // 注入 null
    resolve_lsp(id, Ok(None)).unwrap();
    // promise 兑现；.then 结果由 pump_jobs 泵 → 用 run 一个脚本验证
    let script = r#"
        globalThis.__seen = null;
        p.then(v => { globalThis.__seen = v; });
        __js_pump();
    "#;
    // 见下：pump_jobs 由 crate 内部驱动，测试里用 crate::pump_jobs()
}

#[test]
fn location_path_injection() {
    let mut v = serde_json::json!([{"uri": "file:///a/b.rs", "range": {"start": {"line":1,"character":0}, "end": {"line":1,"character":1}}}]);
    inject_location_paths(&mut v);
    assert_eq!(v[0]["path"], "/a/b.rs");
    // LocationLink 形态（target_uri）
    let mut v2 = serde_json::json!([{"target_uri": "file:///c/d.rs", "origin_selection_range": {}}]);
    inject_location_paths(&mut v2);
    assert_eq!(v2[0]["path"], "/c/d.rs");
}
```

（resolve 后 `.then` 的执行依赖 `pump_jobs()`——测试里调 `crate::pump_jobs()` 后再断言 `globalThis.__seen`；参考 `shell.rs` 现有测试对 promise 的处理方式。）

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test -p helix-js lsp_result_dispatch location_path_injection`
预期：FAIL

- [ ] **步骤 3：实现响应分发**

```rust
// lsp.rs
/// 响应通道（helix-term 泵回结果用；与 ASYNC_EVENTS 同款 WakeSender）
pub struct LspResult { pub id: u64, pub result: Result<Option<String>, String> }
// state.rs:
thread_local! { static LSP_RESULTS: RefCell<Option<WakeSender<LspResult>>> = const { RefCell::new(None) }; }
// init() 里与 ASYNC_EVENTS 一起建通道（用同一 wake_sender 机制唤醒 JS 泵）

pub fn drain_lsp_results() -> Vec<LspResult> {
    crate::init();
    crate::state::with_lsp_results_rx(|r| {
        let mut out = Vec::new();
        if let Some(rx) = r.as_mut() { while let Ok(e) = rx.try_recv() { out.push(e); } }
        out
    })
}

pub fn resolve_lsp(id: u64, result: Result<Option<String>, String>) -> Result<()> {
    crate::init();
    crate::state::with_engine(|engine| {
        let resolving = crate::state::with_lsp_promises(|m| m.remove(&id));
        let Some(resolving) = resolving else { return Ok(()) };
        let undefined = JsValue::undefined();
        let outcome = match result {
            Ok(Some(json)) => {
                // JSON.parse → resolve(json)
                let parsed = crate::layout::js_json_parse(json, engine, "lsp result")?;
                resolving.resolve.call(&undefined, &[parsed], engine).map(|_| ())
            }
            Ok(None) => resolving.resolve.call(&undefined, &[JsValue::null()], engine).map(|_| ()),
            Err(e) => {
                let err = JsError::from_opaque(JsValue::from(JsString::from(e)));
                resolving.reject.call(&undefined, &[err.into()], engine).map(|_| ())
            }
        };
        outcome.map_err(|e| anyhow!("lsp {id} settle failed: {e}"))
    })
}

/// goto_definition 响应注入 path 便利字段（纯函数，可单测）
pub fn inject_location_paths(v: &mut serde_json::Value) {
    fn inject_one(obj: &mut serde_json::Map<String, serde_json::Value>) {
        let uri = obj.get("uri").or_else(|| obj.get("target_uri"));
        if let Some(serde_json::Value::String(u)) = uri {
            if let Ok(path) = url::Url::parse(u).map(|url| url.to_file_path().unwrap_or_default()) {
                obj.insert("path".into(), serde_json::Value::String(path.to_string_lossy().into_owned()));
            }
        }
    }
    match v {
        serde_json::Value::Array(items) => for item in items.iter_mut() {
            if let serde_json::Value::Object(map) = item { inject_one(map); }
        },
        serde_json::Value::Object(map) => inject_one(map),
        _ => {}
    }
}
```

（`crate::layout::js_json_parse` 是现有 JSON.parse 封装——确认其可见性，若非 pub(crate) 则提升。`url` crate 已是 helix-lsp 的依赖，helix-js 侧若不可用则用字符串前缀剥离替代：`u.strip_prefix("file://").unwrap_or(u)`——优先用简单剥离，避免新增依赖。）

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test -p helix-js lsp_result_dispatch location_path_injection`
预期：PASS

- [ ] **步骤 5：Commit**

```bash
git add helix-js/src/lsp.rs helix-js/src/state.rs
git commit -m "feat(js): LSP 响应分发 resolve/reject + path 注入"
```

### 任务 3：helix-term — 泵循环接入（真链路）

**文件：**
- 修改：`helix-term/src/application.rs`（泵循环两段：drain_lsp_results → take_lsp_requests）

- [ ] **步骤 1：编写失败的集成测试**（`helix-term/tests/test/plugin_lsp.rs`，仿 `plugin_async.rs` 模板）

```rust
use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_no_server_resolves_null() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt"); // 无 LSP 配置的普通 txt
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("lsp.js");
    std::fs::write(&plugin_path, r#"
        helix.register_command("lsp-hover", async () => {
            const res = await helix.lsp.hover();
            helix.echo("hover:" + String(res));
        });
        helix.register_command("lsp-syms", async () => {
            const res = await helix.lsp.document_symbols();
            helix.echo("syms:" + String(res));
        });
    "#)?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":lsp-hover<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "hover:null");
            })),
            (Some(":lsp-syms<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "syms:null");
            })),
        ],
        false,
    ).await?;
    Ok(())
}
```

（注意：async 命令处理器 + 泵循环时序——`:lsp-hover` 命令本身在 JS 里是 async fn，命令执行器是否 await？看现有 run_async 测试是 `.then` 回调方式。若 `register_command` 不支持 async fn，改用 `.then` 写法：`helix.lsp.hover().then(res => helix.echo(...))`。**以现有命令执行器能力为准，不支持 async fn 就用 .then**。）

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test -p helix-term --test integration plugin_lsp_no_server_resolves_null`
预期：FAIL（echo 不到 hover:null，请求无人处理）

- [ ] **步骤 3：application.rs 泵循环接入**

```rust
// 泵循环内，async_events resolve 段之后、pump_jobs 之前插入两段：

// 段 A：LSP 响应 → 兑现 Promise（须在 pump_jobs 前，.then 才能同帧跑）
let lsp_results = helix_js::drain_lsp_results();
for r in lsp_results {
    if let Err(err) = helix_js::resolve_lsp(r.id, r.result) {
        log::error!("lsp result {r:?}: {err}");
    }
}

// 段 B：LSP 请求 → 发往 language server
for req in helix_js::take_lsp_requests() {
    handle_lsp_request(&self.editor, req);
}
```

```rust
// 同文件 private fn（仿 goto_single_impl 的取 server 方式）
fn handle_lsp_request(editor: &Editor, req: helix_js::LspRequest) {
    use helix_lsp::lsp as lsp; // 按 helix-term 现有 import 风格
    let (feature, mk) = match req.method {
        helix_js::LspMethod::Hover => (LanguageServerFeature::Hover, LspReqKind::Hover),
        helix_js::LspMethod::Completion => (LanguageServerFeature::Completion, LspReqKind::Completion),
        helix_js::LspMethod::GotoDefinition => (LanguageServerFeature::GotoDefinition, LspReqKind::GotoDefinition),
        helix_js::LspMethod::DocumentSymbols => (LanguageServerFeature::DocumentSymbols, LspReqKind::DocumentSymbols),
    };
    let Some((view, doc)) = editor.tree().get(editor.tree().id).map(|(v, d)| (v, d)) else { return };
    let Some(ls) = doc.language_servers_with_feature(feature).next() else {
        let _ = helix_js::resolve_lsp(req.id, Ok(None));
        return;
    };
    let offset_encoding = ls.offset_encoding();
    let pos = match req.pos {
        Some((row, col)) => {
            let text = doc.text();
            let char_off = text.line_to_char(row as usize).saturating_add(col as usize);
            helix_lsp::util::pos_to_lsp_pos(text, char_off, offset_encoding)
        }
        None => doc.position(view.id, offset_encoding),
    };
    let client = ls.clone(); // Arc<Client>
    let doc_id = doc.identifier();
    let future = match mk {
        LspReqKind::Hover => client.text_document_hover(doc_id, pos, None),
        LspReqKind::Completion => client.completion(doc_id, pos, None, lsp::CompletionContext {
            trigger_kind: lsp::CompletionTriggerKind::INVOKED, trigger_character: None,
        }),
        LspReqKind::GotoDefinition => client.goto_definition(doc_id, pos, None),
        LspReqKind::DocumentSymbols => client.document_symbols(doc_id),
    };
    let Some(future) = future else {
        // capabilities 不支持 → null
        let _ = helix_js::resolve_lsp(req.id, Ok(None));
        return;
    };
    let tx = helix_js::lsp_result_tx(); // 与 ASYNC_EVENTS 同款：暴露克隆
    tokio::spawn(async move {
        let result = match future.await {
            Ok(Some(v)) => match serde_json::to_value(&v) {
                Ok(mut json) => {
                    if matches!(req_kind_is_goto) { helix_js::inject_location_paths(&mut json); }
                    Ok(Some(json.to_string()))
                }
                Err(e) => Err(e.to_string()),
            },
            Ok(None) => Ok(None),
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(helix_js::LspResult { id: req.id, result });
    });
}
```

（`editor.tree().get(id)` 的当前 view/doc 获取方式以 `current_ref!` 宏在 application.rs 的可达性为准——application.rs 里有 `self.editor`，用 `crate::commands::typed` 现有 helper 或 `current_ref!` 宏。）

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test -p helix-term --test integration plugin_lsp_no_server_resolves_null`
预期：PASS

- [ ] **步骤 5：Commit**

```bash
git add helix-term/src/application.rs helix-term/tests/test/plugin_lsp.rs helix-term/tests/integration.rs
git commit -m "feat(term): LSP 请求泵循环接入（无 server → null 链路）"
```

### 任务 4：open_file 扩展（行列定位）

**文件：**
- 修改：`helix-js/src/types.rs`（`UiRequest::OpenFile` 加 `row/col`）
- 修改：`helix-js/src/popup.rs`（`js_open_file` 解析第二参数）
- 修改：`helix-js/src/lib.rs:346` 附近现有单测（`UiRequest::OpenFile` 匹配解构需更新）
- 修改：`helix-term/src/commands/typed.rs:4581`（OpenFile 分支加定位）

- [ ] **步骤 1：编写失败的集成测试**（追加到 `plugin_lsp.rs`）

```rust
#[tokio::test(flavor = "multi_thread")]
async fn plugin_open_file_with_position() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let src = dir.path().join("src.txt");
    let lines: Vec<String> = (0..10).map(|i| format!("line{i}")).collect();
    std::fs::write(&src, lines.join("\n") + "\n")?;
    let plugin_path = dir.path().join("open.js");
    std::fs::write(&plugin_path, r#"
        helix.register_command("jump10", () => {
            helix.open_file(PLUGIN_FILE, { row: 9, col: 4 });
        });
    "#)?;
    // PLUGIN_FILE 用绝对路径注入——写成实际路径字符串
    test_key_sequences(
        &mut AppBuilder::new().with_file(src, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":jump10<ret>"), Some(&|app| {
                let (view, doc) = current_ref!(app.editor);
                let pos = doc.selection(view.id).primary().cursor(doc.text().slice(..));
                let line = doc.text().char_to_line(pos);
                assert_eq!(line, 9, "光标应跳到第 9 行");
            })),
        ],
        false,
    ).await?;
    Ok(())
}
```

（注意：插件里 `PLUGIN_FILE` 需替换为 `src` 的实际绝对路径——测试内用 `format!` 拼路径字符串。）

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test -p helix-term --test integration plugin_open_file_with_position`
预期：FAIL（open_file 忽略第二参数，光标停在原处）

- [ ] **步骤 3：实现扩展**

```rust
// types.rs
pub enum UiRequest {
    // ... 其他变体
    OpenFile {
        path: String,
        row: Option<u16>,
        col: Option<u16>,
    },
}

// popup.rs js_open_file：解析可选第二参数对象 { row, col }
pub(crate) fn js_open_file(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsString::from("open_file: path must be a string"))
    })?;
    let (row, col) = match args.get(1) {
        Some(obj) if obj.is_object() => {
            let o = obj.as_object().unwrap();
            let row = opt_u16(&o.get(JsString::from("row"), ctx)?, ctx, "row")?;
            let col = opt_u16(&o.get(JsString::from("col"), ctx)?, ctx, "col")?;
            (Some(row), Some(col))
        }
        _ => (None, None),
    };
    UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::OpenFile { path, row, col });
    Ok(JsValue::undefined())
}

// typed.rs OpenFile 分支
helix_js::UiRequest::OpenFile { path, row, col } => {
    let path = PathBuf::from(path);
    job::dispatch_blocking(move |editor, _compositor| {
        if let Err(err) = editor.open(&path, Action::Replace) {
            editor.set_error(format!("open_file: {err}"));
        } else if let (Some(row), Some(col)) = (row, col) {
            let (view, doc) = current_ref!(editor);
            let pos = Selection::point(doc.text().line_to_char(row as usize).saturating_add(col as usize));
            doc.set_selection(view.id, pos);
            align_view(doc, view, Align::Center);
        }
    });
}
```

- [ ] **步骤 4：修复 lib.rs 现有单测**（`lib.rs:346` 附近 `matches!(&reqs[0], UiRequest::OpenFile { path } ...)` 需加 `row: None, col: None` 匹配）

- [ ] **步骤 5：运行测试验证通过**

运行：`cargo test -p helix-js && cargo test -p helix-term --test integration plugin_open_file_with_position`
预期：全部 PASS

- [ ] **步骤 6：Commit**

```bash
git add helix-js/src/types.rs helix-js/src/popup.rs helix-js/src/lib.rs helix-term/src/commands/typed.rs helix-term/tests/test/plugin_lsp.rs
git commit -m "feat(js): open_file 支持行列定位"
```

### 任务 5：demo 插件 + 文档

**文件：**
- 创建：`plugins/features/lsp-hover/index.js`
- 修改：`docs/plugin-api.md`（新增 "LSP 请求" 章节）

- [ ] **步骤 1：写 demo 插件**

```js
// plugins/features/lsp-hover/index.js
// :lsp-hover 光标处 hover 弹窗;:lsp-goto 跳转到定义
helix.register_command("lsp-hover", async () => {
  const hover = await helix.lsp.hover();
  if (!hover) return helix.echo("no hover");
  const text = Array.isArray(hover.contents)
    ? hover.contents.map(c => c.value ?? c).join("\n")
    : hover.contents.value ?? hover.contents;
  helix.open_popup({
    render: () => helix.el({ type: "col", children: [
      { type: "text", content: String(text) },
    ]}),
  });
});

helix.register_command("lsp-goto", async () => {
  const locs = await helix.lsp.goto_definition();
  if (!locs?.length) return helix.echo("no definition");
  const l = locs[0];
  helix.open_file(l.path, { row: l.range.start.line, col: l.range.start.character });
});
```

- [ ] **步骤 2：写文档章节**（`docs/plugin-api.md` 追加，含 4 方法签名、空/错语义表、示例）

- [ ] **步骤 3：手动验证**（需有 LSP 的环境，如 Rust/TS 项目）

```bash
cargo build
# 打开一个带 LSP 的项目文件,`:plugin-load plugins/features/lsp-hover/index.js`
# `:lsp-hover` 应弹窗显示 hover;`:lsp-goto` 应跳转定义
```

- [ ] **步骤 4：Commit**

```bash
git add plugins/features/lsp-hover/index.js docs/plugin-api.md
git commit -m "feat(plugins): lsp-hover demo + plugin-api LSP 章节"
```

---

## 自检

**规格覆盖度：**
- 4 方法 API 形状 → 任务 1（注册/入队）
- 透传原始 JSON / null / reject 语义 → 任务 2（分发）
- goto_* path 注入 → 任务 2（`inject_location_paths`）
- 位置覆盖 + 默认光标快照 → 任务 1（`pos: Option<(u16,u16)>`，None=泵处理时光标，与设计"调用时快照"同帧等价）+ 任务 3（转换）
- 异步桥接（tokio spawn + 通道回泵）→ 任务 3
- 无 server / 不支持 → null 短路 → 任务 3（两处：无 feature server → null；方法返回 None → null）
- open_file 扩展 → 任务 4
- 测试策略（js 单测 + term 集成 + 手动）→ 各任务对应
- 文档 + demo → 任务 5

**占位符扫描：** 无 TODO/待定；任务 3 的"以现有命令执行器能力为准"是风险标注（实现时先验证 async fn 命令支持，不支持则用 `.then`），非占位符。

**类型一致性：** `LspRequest{id, method, pos}` / `LspResult{id, result: Result<Option<String>, String>}` 在任务 1-3 一致；`resolve_lsp(id, Result<Option<String>, String>)` 一致；`UiRequest::OpenFile{path, row, col}` 任务 4 内一致。`LspMethod` 枚举 4 变体全任务一致。
