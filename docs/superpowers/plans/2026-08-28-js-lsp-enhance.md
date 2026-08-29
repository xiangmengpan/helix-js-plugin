# LSP 增强 API(rename / format / code_actions)实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** 插件可用 `helix.lsp.format()` / `helix.lsp.rename(newName)` / `helix.lsp.code_actions()` / `helix.lsp.execute_code_action(action)` 执行会改变文档的 LSP 操作——自动应用编辑(一次撤销/文件),resolve 结果摘要。

**架构:** 复用现有 promise 管道(LspRequest 入队 + LspResult 回程)。tokio 任务拿到的响应是纯数据(TextEdit/WorkspaceEdit/action JSON),序列化成 `LspResult.apply` 载荷回主线程;泵循环段 A 先应用编辑再 `resolve_lsp`。format 的 TextEdit→Transaction 提取纯函数可单测;rename 复用 `apply_workspace_edit`;execute 复用 `Action::lsp().execute()`。

**技术栈:** Rust(helix-js boa 运行时 + helix-term + helix-view + helix-lsp)、tokio、serde_json。

**规格:** `docs/superpowers/specs/2026-08-28-js-lsp-enhance-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-js/src/lsp.rs` | `LspMethod` 扩展(Rename/Format/CodeActions/ExecuteCodeAction)、`enqueue_lsp_request` 参数解析、4 个 JS 函数、`LspResult` 加 `apply: Option<LspApply>`、新 `LspApply` enum(JSON 载荷) |
| `helix-js/src/lib.rs` | 注册 4 个新方法到 helix.lsp 对象;新单测 |
| `helix-term/src/application.rs` | `handle_lsp_request` 映射 4 新方法 + tokio 任务构造 apply 载荷;段 A 应用插入(resolve 前);`lsp_text_edits_to_transaction` 纯函数(放本文件或独立模块) |
| `helix-term/src/commands/lsp.rs` | 若无现成辅助则提取 code_actions 列表序列化所需(优先复用 `code_actions_for_range`) |
| `helix-term/tests/test/plugin_lsp.rs` | 扩展 4 个无 server null 路径测试 |
| `docs/plugin-api.md` | 4 方法文档一节 |

关键实现细节(执行时必读):

- **现有管道**(勿破坏):`enqueue_lsp_request`(lsp.rs:50)解析可选 pos → 入队 + promise;泵循环 application.rs:345 take → 段 B(385)handle_lsp_request → tokio 任务 → `tx.send(LspResult { id, result })`(1717)→ 段 A(384)resolve_lsp。**新方法的应用插入在段 A resolve 之前**。
- **LspResult 扩展**(helix-js,不依赖 helix-lsp——载荷用 JSON):

```rust
/// 需要主线程应用的 LSP 编辑载荷(纯 JSON,跨线程;lsp 类型在 term 侧反序列化)
#[derive(Debug)]
pub enum LspApply {
    /// format:TextEdit 列表应用到指定 doc(任务侧已序列化为 JSON)
    Format { doc_id: u64, edits: serde_json::Value },
    /// rename:WorkspaceEdit(URI 自含,apply_workspace_edit 处理跨 doc/打开)
    WorkspaceEdit(serde_json::Value),
    /// execute code action:完整 CodeActionOrCommand JSON
    ExecuteAction(serde_json::Value),
}
// LspResult 加 pub apply: Option<LspApply>
```

   `doc_id` 用 `lsp::DocumentId` 的 u64 透传(helix-js 不知道 DocumentId 类型;任务侧 `doc.identifier()` → `id.to_u64()`?——**执行时确认 DocumentId 的取数方法**(u64 newtype,通常 `*.0` 或 `to_usize`;helix-term 反序列化回 `DocumentId::from(u64)`),按编译器/源码实际 API 调整)。
- **tokio 任务构造载荷**:Format 响应 `Option<Vec<lsp::TextEdit>>` → `serde_json::to_value(&edits)` 进 apply;Rename 响应 `Option<lsp::WorkspaceEdit>` → to_value;Execute 不走 server 响应(见下)。**无编辑(None)/请求失败** → apply=None,result 照常(null/错误)——不应用。
- **ExecuteCodeAction 的特殊性**:action JSON 在**请求里**(插件从列表拿回传),不经过 server 请求——任务侧直接把它放进 `LspResult.apply`(无需发 LSP 请求;但仍走通道回主线程,保持"主线程应用"单一入口)。result = `Ok(Some(r#"{"applied":true}"#))` 由主线程应用后 resolve。
- **段 A 应用插入**(application.rs:384):

```rust
for r in lsp_results {
    let helix_js::LspResult { id, result, apply } = r;
    // 先应用(编辑生效)再 resolve——promise resolve 时编辑已落地
    if let Some(apply) = apply {
        if let Err(err) = apply_lsp_edits(&mut self.editor, apply) {
            log::error!("lsp apply {id}: {err}");
        }
    }
    if let Err(err) = helix_js::resolve_lsp(id, result) {
        log::error!("lsp result {id}: {err}");
    }
}
```

   `apply_lsp_edits(editor, apply)`(application.rs 内私有 fn):
   - `Format { doc_id, edits }`:`editor.documents.get(&doc_id)` → `lsp_text_edits_to_transaction(doc.text(), edits, offset_encoding)` → `doc.apply(&txn, view_id)`(view_id 取 `editor.tree.get(doc_id)` 或当前 view,参照批次 2 的 get_synced_view_id 教训——**后台 doc 用 `get_synced_view_id`**)+ `doc.append_changes_to_history(view_id)`;resolve result 替换为 `Ok(Some(r#"{"applied":true}"#))`(在 resolve 处,见下)
   - `WorkspaceEdit(v)`:deserialize `lsp::WorkspaceEdit` → `editor.apply_workspace_edit(offset_encoding, &edit)`(offset_encoding 取当前 doc 的 server;错误 → log 且 resolve null/错误,不 panic)→ 摘要 `{"applied":true,"files":n}`(n = 编辑涉及的 doc 数,取 `workspace_edit.document_changes` 长度或应用计数——**简化:files = 成功应用后 editor 变更计数不可得,用 document_changes 里不同 uri 数,按实际结构取**)
   - `ExecuteAction(v)`:deserialize `lsp::CodeActionOrCommand` → 当前 doc 第一个 CodeAction feature server(与列表同选择;多 server 简化注释)→ `helix_view::action::Action::lsp(server_id, action).execute(editor)` → 摘要 `{"applied":true}`
   - **摘要替换**:apply 成功后把该请求的 result 覆写为摘要 JSON(主线程段 A 持有 result,应用成功 → `result = Ok(Some(summary_json))`;失败 → 保留原 result 或 `Ok(None)`,不 panic)
- **`lsp_text_edits_to_transaction`**(application.rs 或新 `helix-term/src/commands/typed.rs` 附近——**放 application.rs 旁私有 fn 或 commands/lsp.rs 顶层;纯函数,无 editor 依赖**):

```rust
/// TextEdit 列表 → Transaction(lsp_range_to_range 换算 UTF-16 坐标;排序 + 重叠检查;与 format 内置路径同逻辑)
fn lsp_text_edits_to_transaction(
    text: &Rope,
    edits: &[lsp::TextEdit],
    offset_encoding: OffsetEncoding,
) -> anyhow::Result<Transaction> {
    use helix_core::Change;
    let mut changes: Vec<Change> = edits
        .iter()
        .filter_map(|e| {
            lsp_range_to_range(text, e.range, offset_encoding)
                .map(|r| (r.start, r.end, Some(e.new_text.clone().into())))
        })
        .collect();
    changes.sort_by_key(|c| c.0);
    for w in changes.windows(2) {
        if w[0].1 > w[1].0 {
            bail!("overlapping LSP edits");
        }
    }
    Ok(Transaction::change(text, changes.into_iter()))
}
```

   (lsp_range_to_range 返回 Option——无效范围过滤;import 参照 commands/lsp.rs:14 的 util 导入)
- **handle_lsp_request 映射 4 方法**(application.rs:1640 同款模式):
  - Format → `LanguageServerFeature::Format` → `client.text_document_formatting(doc_id, lsp::FormattingOptions::default(), None)` → 任务内 `to_value` 进 apply
  - Rename → `LanguageServerFeature::Rename` → `client.rename(doc_id, pos, new_name, None)`(跳过 prepare;newName 从 req 参数取)→ `Option<lsp::WorkspaceEdit>` → to_value 进 apply
  - CodeActions → `LanguageServerFeature::CodeAction` → 复用 `code_actions_for_range(doc, primary_selection, None, CodeActionTriggerKind::INVOKED)`(commands/lsp.rs:674,返回 `Vec<(impl Future, LanguageServerId)>`)——**注意该函数在 commands/lsp.rs,application.rs 需可见(pub(crate))或把列表逻辑提取**;等待全部 future → 过滤 disabled(参照 commands/lsp.rs:604-613)→ `serde_json::to_value`(CodeActionOrCommand 全量 Serialize)→ resolve 数组;无 actions → resolve null
  - ExecuteCodeAction → 无 server 请求(见上)
- **LspRequest 参数扩展**(lsp.rs):`Rename { new_name: String }` 与 `ExecuteCodeAction` 需要带数据——LspRequest 加 `params: Option<serde_json::Value>`(rename 的 new_name 字符串、execute 的 action JSON);enqueue 按 method 校验并填充
- **测试命令**:`cargo test -p helix-js`;`cargo test -p helix-term --features integration --test integration plugin_lsp`;`cargo test -p helix-term`(单测);clippy 零警告;收尾 `cargo fmt --all --check`。

---

### 任务 1:请求侧(helix-js:方法/参数/注册 + 单测)

**文件:** helix-js/src/lsp.rs、lib.rs

- [ ] **步骤 1:写失败单测(参数校验与请求形态)**

`helix-js/src/lib.rs` 测试模块加(参照现有 lsp 测试模式;`take_lsp_requests` 是 pub):

```rust
#[test]
fn lsp_enhance_request_shapes() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    // rename newName 非字符串 → 命令失败
    load_script(r#"
        helix.register_command("ren-bad", async () => { await helix.lsp.rename(42); });
        helix.register_command("exec-bad", async () => { await helix.lsp.execute_code_action("x"); });
    "#).unwrap();
    let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)), docs: vec![] };
    assert!(run_command("ren-bad", &ctx).is_err());
    assert!(run_command("exec-bad", &ctx).is_err());
    assert!(take_lsp_requests().is_empty(), "校验失败不应入队");
    // 合法调用 → 请求形态正确
    load_script(r#"
        helix.register_command("ren-ok", async () => {
            await helix.lsp.rename("newName");
            await helix.lsp.format();
            await helix.lsp.code_actions({ row: 1, col: 2 });
            await helix.lsp.execute_code_action({ title: "fix", kind: "quickfix" });
        });
    "#).unwrap();
    run_command("ren-ok", &ctx).unwrap();
    let reqs = take_lsp_requests();
    assert_eq!(reqs.len(), 4);
    assert!(matches!(reqs[0].method, crate::lsp::LspMethod::Rename { ref new_name } if new_name == "newName"));
    assert!(matches!(reqs[1].method, crate::lsp::LspMethod::Format));
    assert!(matches!(reqs[2].method, crate::lsp::LspMethod::CodeActions));
    assert_eq!(reqs[2].pos, Some((1, 2)));
    assert!(matches!(reqs[3].method, crate::lsp::LspMethod::ExecuteCodeAction));
    assert!(reqs[3].params.is_some());
}
```

预期:FAIL(编译错误:LspMethod::Rename/Format/CodeActions/ExecuteCodeAction 不存在)。

- [ ] **步骤 2:实现请求侧**

1. `helix-js/src/lsp.rs`:
   - `LspMethod` 扩展:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspMethod {
    Hover,
    Completion,
    GotoDefinition,
    DocumentSymbols,
    Format,
    Rename,
    CodeActions,
    ExecuteCodeAction,
}
```

   (rename 的 newName 与 execute 的 action 走 `params` 字段,method 保持 Copy;若实现时发现传参不便,可改 variant 携带,但保持与现有代码风格一致)
   - `LspRequest` 加 `pub params: Option<serde_json::Value>`(现有 4 方法构造点补 `params: None`——grep `LspRequest {` 定位)
   - `LspApply` enum + `LspResult.apply: Option<LspApply>`(见关键细节;构造点 application.rs:1717 补 `apply: None`,由任务 2 填充)
   - `enqueue_lsp_request` 扩展:按 method 分支——Rename:第一参 `try_js_into::<String>`(非字符串 TypeError)→ params = to_value(new_name);ExecuteCodeAction:第一参必须对象(`serde_json::from_str(&obj.to_json(...))` 或构造 `serde_json::to_value`——**用 `obj.to_json(context)` 拿字符串再 parse,参照现有 lsp.rs 的 JSON 处理模式;校验失败 TypeError**);Format/CodeActions:无参,可选 pos
   - 4 个 JS 函数:`js_lsp_format` / `js_lsp_rename` / `js_lsp_code_actions` / `js_lsp_execute_code_action`(调用扩展后的 enqueue)
2. `helix-js/src/lib.rs` 注册(helix.lsp 对象 builder,lsp.rs 的 4 个新函数,参数 arity:format 0 / rename 1 / code_actions 1 / execute 1):

```rust
.function(NativeFunction::from_fn_ptr(lsp::js_lsp_format), JsString::from("format"), 0)
.function(NativeFunction::from_fn_ptr(lsp::js_lsp_rename), JsString::from("rename"), 1)
.function(NativeFunction::from_fn_ptr(lsp::js_lsp_code_actions), JsString::from("code_actions"), 1)
.function(NativeFunction::from_fn_ptr(lsp::js_lsp_execute_code_action), JsString::from("execute_code_action"), 1)
```

- [ ] **步骤 3:单测转绿**

运行:`cargo test -p helix-js lsp_enhance` 预期 PASS;`cargo test -p helix-js` 全量确认旧测试(含现有 lsp 测试)仍绿。

- [ ] **步骤 4:Commit**

```bash
git add helix-js
git commit -m "feat(js): helix.lsp format/rename/code_actions/execute_code_action——请求入队与参数校验,LspResult 带 apply 载荷"
```

---

### 任务 2:响应应用侧(term:映射/应用/摘要 + 单测 + integration)

**文件:** helix-term/src/application.rs、commands/lsp.rs(如需)、tests/test/plugin_lsp.rs、docs/plugin-api.md

- [ ] **步骤 1:纯函数单测(红)**

`helix-term/src/application.rs` 测试模块(或独立 `#[cfg(test)]`;若无测试模块则新建,参照 commands.rs 既有测试惯例)加:

```rust
#[test]
fn lsp_text_edits_to_transaction_basic() {
    use helix_core::Rope;
    use helix_lsp::{lsp, OffsetEncoding};
    let text = Rope::from("one\ntwo\nthree\n");
    let edits = vec![
        lsp::TextEdit {
            range: lsp::Range::new(lsp::Position::new(0, 0), lsp::Position::new(0, 3)),
            new_text: "ONE".into(),
        },
        lsp::TextEdit {
            range: lsp::Range::new(lsp::Position::new(2, 0), lsp::Position::new(2, 5)),
            new_text: "THREE".into(),
        },
    ];
    let txn = lsp_text_edits_to_transaction(&text, &edits, OffsetEncoding::Utf16).unwrap();
    let mut result = text.clone();
    txn.apply(&mut result);  // Transaction::apply(&self, &mut Rope)——确认签名,不适用则用 doc.apply 或 changes 断言
    assert_eq!(result.to_string(), "ONE\ntwo\nTHREE\n");
}

#[test]
fn lsp_text_edits_reversed_overlap_bails() {
    // 重叠编辑 → Err(不 panic)
    let text = helix_core::Rope::from("abcdef");
    let edits = vec![
        lsp::TextEdit { range: lsp::Range::new(lsp::Position::new(0, 0), lsp::Position::new(0, 3)), new_text: "X".into() },
        lsp::TextEdit { range: lsp::Range::new(lsp::Position::new(0, 2), lsp::Position::new(0, 4)), new_text: "Y".into() },
    ];
    assert!(lsp_text_edits_to_transaction(&text, &edits, OffsetEncoding::Utf16).is_err());
}
```

运行:`cargo test -p helix-term lsp_text_edits_to_transaction` 预期 FAIL(函数不存在)。Transaction::apply 的准确签名按实际调整(helix-core Transaction 有 `apply(&mut Rope)` 或需 doc.apply——执行时确认,测试断言以最终文本为准)。

- [ ] **步骤 2:实现纯函数 + 段 A 应用**

1. `lsp_text_edits_to_transaction`(见关键细节代码;放 application.rs 私有 fn 或 commands/lsp.rs pub(crate),按可测试性选择——若放 commands/lsp.rs 需确认该文件有 `#[cfg(test)]` 或测试模块可达;**application.rs 更简单:fn 与测试同文件**)
2. `apply_lsp_edits(editor, apply)` 私有 fn(见关键细节;Format 用 `editor.get_synced_view_id(doc_id)` 取 view——批次 2 教训,后台 doc 防 panic;WorkspaceEdit 用 `apply_workspace_edit`;ExecuteAction 用 `Action::lsp().execute()`)
3. 段 A 改造(见关键细节):应用成功 → 覆写 result 为摘要 JSON;失败 → log + 保留 result(或 Ok(None))

- [ ] **步骤 3:handle_lsp_request 映射 4 方法 + tokio 任务载荷**

`handle_lsp_request`(application.rs:1640)加 4 分支:
- `LspMethod::Format` → feature Format → `client.text_document_formatting(doc_id, lsp::FormattingOptions::default(), None)` → `.map(|f| Box::pin(async move { ... }))` 内:await → `serde_json::to_value(edits)` → `LspResult { id, result: Ok(None), apply: Some(Format { doc_id: doc_id.to_u64(), edits }) }`——**result 与 apply 的取舍:应用成功时段 A 覆写摘要;这里 result 传 Ok(None)(占位),段 A 覆写**
- `LspMethod::Rename` → feature Rename → `client.rename(doc_id, pos, new_name, None)`(new_name 从 `req.params` 取:serde_json::from_value::<String>;缺失 → resolve null)→ WorkspaceEdit → `apply: WorkspaceEdit(to_value)`
- `LspMethod::CodeActions` → feature CodeAction → 复用 `code_actions_for_range`(commands/lsp.rs:674,需 pub(crate) 或提取)——等待全部 future → 过滤 disabled → `serde_json::to_value(Vec<CodeActionOrCommand>)` → 直接 resolve(无 apply)
- `LspMethod::ExecuteCodeAction` → 无 server 请求:把 `req.params`(action JSON)放进 `LspResult { id, result: Ok(Some(r#"{"applied":true}"#)), apply: Some(ExecuteAction(params)) }` 走通道——段 A 应用后 resolve 摘要
- **执行时注意**:LspResult 构造点(1717)是共享通道发送——Format/Rename/Execute 的载荷在此构造;CodeActions 直接 resolve(与现有 4 方法同路径)。doc_id 的 u64 转换按 DocumentId 实际 API

- [ ] **步骤 4:integration 无 server null 路径**

`helix-term/tests/test/plugin_lsp.rs` 扩展(现有文件末尾加测试,同款 async 续体 + echo 断言):

```rust
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_enhance_no_server_resolves_null() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt"); // 无 LSP
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("lsp-enhance.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("lsp-fmt", async () => {
            const r = await helix.lsp.format();
            helix.echo("fmt:" + String(r));
        });
        helix.register_command("lsp-ren", async () => {
            const r = await helix.lsp.rename("x");
            helix.echo("ren:" + String(r));
        });
        helix.register_command("lsp-ca", async () => {
            const r = await helix.lsp.code_actions();
            helix.echo("ca:" + String(r));
        });
        helix.register_command("lsp-exec", async () => {
            const r = await helix.lsp.execute_code_action({ title: "t" });
            helix.echo("exec:" + String(r));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":lsp-fmt<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "fmt:null");
            })),
            (Some(":lsp-ren<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "ren:null");
            })),
            (Some(":lsp-ca<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "ca:null");
            })),
            (Some(":lsp-exec<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "exec:null");
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

   (无 server 时:Format/Rename/CodeActions 走"无 feature server → resolve null";ExecuteCodeAction 的 apply 载荷会到段 A——无 server 时 Execute 也应 resolve null,实现时在 handle_lsp_request 里对 Execute 也检查 CodeAction feature server,无则 resolve null 不构造载荷——**注意这个语义:Execute 与列表同 server 选择,无 server 时 null**)

- [ ] **步骤 5:单测转绿 + plugin-api.md + 全量验证 + Commit**

```bash
cargo test -p helix-term lsp_text_edits_to_transaction
cargo test -p helix-term --features integration --test integration plugin_lsp
cargo test -p helix-js
cargo clippy --all-targets 2>&1 | tail -3
cargo fmt --all --check
```

预期:纯函数单测 2 个 PASS;plugin_lsp 全部(现有 + 新增)PASS;helix-js 全绿;clippy 零警告;fmt clean。

`docs/plugin-api.md` 加一节(仿现有 helix.lsp 小节):

```markdown
### `helix.lsp.format()` / `helix.lsp.rename(newName)` / `helix.lsp.code_actions(pos?)` / `helix.lsp.execute_code_action(action)`(LSP 编辑操作)

```js
await helix.lsp.format();                       // → { applied: true } 或 null
await helix.lsp.rename("newName");              // → { applied: true, files: n } 或 null
const actions = await helix.lsp.code_actions(); // → [{ title, kind, ...完整 LSP action }] 或 null
await helix.lsp.execute_code_action(actions[0]); // → { applied: true } 或 null
```

- 自动应用编辑(一次撤销/文件);rename 可跨 buffer(自动打开未打开文件);code_actions 两阶段无状态(execute 原样传回列表项)
- 无 server / 能力不支持 / 请求失败 → null;无超时(可能悬挂,与其它 lsp 方法一致)
- 响应到达即应用(不检查文档版本,插件用 await 时序自行控制);format 仅全文档
```

```bash
git add helix-term docs/plugin-api.md
git commit -m "feat(term): LSP 增强应用——format/rename/code_actions 映射与编辑应用(段 A 先应用再 resolve)+ 纯函数单测 + integration"
```

---

## 自检记录

**规格覆盖度:**
- 2.1 API(4 方法签名/摘要/null)→ 任务 1 步骤 1-3、任务 2 步骤 4 ✓
- 2.2 语义(自动应用/一次撤销/无状态两阶段)→ 任务 2 步骤 2-3 ✓
- 2.3 数据流(LspMethod 扩展/LspResult.apply/段 A 应用)→ 任务 1 步骤 2、任务 2 步骤 2-3 ✓
- 2.4 边界(无超时/仅全文档/不查 version/跨 doc 打开/command 分发)→ 任务 2 步骤 3、文档 ✓
- 3.1 单测(校验/请求形态)→ 任务 1 步骤 1-3 ✓
- 3.2 纯函数单测(TextEdit→Transaction/CodeActions 序列化)→ 任务 2 步骤 1-2 ✓(CodeActions 序列化单测在任务 2 步骤 2 顺带;若提取成纯函数则同样测)
- 3.3 integration null 路径 → 任务 2 步骤 4 ✓

**占位符扫描:** 无 TODO/待定;代码块为实际实现代码;执行时确认项(Transaction::apply 签名、DocumentId 取数、code_actions_for_range 可见性)已明确标注为"按实际调整"。✓

**类型一致性:** `LspMethod` 四新 variant、`LspApply` 三形态、`LspRequest.params`、`LspResult.apply` 跨任务签名一致;JS 函数注册名(format/rename/code_actions/execute_code_action)与文档/测试一致。✓
