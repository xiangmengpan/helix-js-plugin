# 多 server code_actions 修复实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** `code_actions()` 列表项携带 `_serverId`,`execute_code_action()` 按它用对应 server 执行——修复多 server 项目 action 发到错误 server 的缺陷(批次 4 遗留)。

**架构:** `handle_lsp_code_actions` 保留 ls_id 并注入列表项;execute 与段 A 应用解析 `_serverId` → `language_server_by_id` 反查 → 剥离后执行;缺失/查不到回退第一个。mock 双 server 场景验证。

**技术栈:** Rust(helix-term + helix-lsp + slotmap Key)、tokio 集成测试。

**规格:** `docs/superpowers/specs/2026-08-31-js-multiserver-codeactions-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-term/src/application.rs` | `handle_lsp_code_actions` 注入 `_serverId`;`handle_lsp_execute_code_action`/段 A 解析 `_serverId` + 回退 |
| `helix-term/src/bin/mock_lsp.rs` | code_actions 场景按 argv[2] 标识返回不同 title/edit |
| `helix-term/tests/test/plugin_lsp_mock.rs` | 双 server 测试(或新文件) |
| `docs/plugin-api.md` | code_actions 文档注明 `_serverId` 内部字段 |

关键实现细节(执行时必读):

- **`_serverId` 数值**:`LanguageServerId` 是 `slotmap::new_key_type!`(helix-core/src/diagnostic.rs:83),`.0` 是 u64(Display 实现可见);序列化 `"_serverId": id.0`(或 slotmap Key::data,按编译)。反查:`editor.language_server_by_id(LanguageServerId::from(u64))`——**LanguageServerId::from(u64) 是否可用,按 slotmap new_key_type 的 From 实现确认**(new_key_type 提供 `from(u64)`?执行时验证,不可用则用 `slotmap::Key::from(data)` 或其它构造方式,按编译调整)。
- **`handle_lsp_code_actions`**(application.rs:1883):`for (future, ls_id) in futures`(现在是 `_ls_id`),每项序列化后注入:

```rust
for (future, ls_id) in futures {
    if let Ok(Some(list)) = future.await {
        actions.extend(list.into_iter().filter(|action| {
            matches!(
                action,
                lsp::CodeActionOrCommand::Command(_)
                    | lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction { disabled: None, .. })
            )
        }).map(|action| {
            let mut v = serde_json::to_value(&action).unwrap_or(serde_json::Value::Null);
            v["_serverId"] = serde_json::json!(ls_id.0);
            v
        }));
    }
}
```

  (actions 类型从 `Vec<CodeActionOrCommand>` 变 `Vec<serde_json::Value>`;result 序列化相应简化)
- **execute 与段 A 统一处理**:`handle_lsp_execute_code_action` 不再做 server_exists 检查(段 A 统一);段 A `ExecuteAction(v)`:

```rust
helix_js::LspApply::ExecuteAction(v) => {
    let mut v = v;
    let server_id = v.get("_serverId").and_then(|s| s.as_u64()).map(LanguageServerId::from);
    if let Some(obj) = v.as_object_mut() { obj.remove("_serverId"); }
    let action: lsp::CodeActionOrCommand = serde_json::from_value(v)?;
    let (_, doc) = current_ref!(editor);
    let server = match server_id.and_then(|id| editor.language_server_by_id(id)) {
        // 有 _serverId 且存在 → 用对应 server;缺失/查不到 → 回退第一个(兼容)
        Some(s) => Some(s),
        None => doc.language_servers_with_feature(LanguageServerFeature::CodeAction).next(),
    };
    let Some(server) = server else {
        anyhow::bail!("execute code action: no code action server");
    };
    helix_view::action::Action::lsp(server.id(), action).execute(editor);
    Ok(r#"{"applied":true}"#.to_string())
}
```

  (`LanguageServerId::from(u64)` 构造按实际 API;`language_server_by_id` 返回 `Option<&Client>`)
- **mock code_actions 场景扩展**(mock_lsp.rs):读 `args.get(2)` 作为来源标识(缺省 "A"):

```rust
"textDocument/codeAction" if scenario == "code_actions_basic" => {
    let tag = args.get(2).map(String::as_str).unwrap_or("A");
    let uri = msg["params"]["textDocument"]["uri"].clone();
    let mut changes = serde_json::Map::new();
    changes.insert(uri.as_str().unwrap_or("").to_string(), json!([{
        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } },
        "newText": format!("ONE-{tag}"),
    }]));
    json!([{ "title": format!("mock-fix-{tag}"), "kind": "quickfix", "edit": { "changes": changes } }])
}
```

  (既有单 server 测试 args 无 tag → 缺省 "A",title "mock-fix-A"/edit "ONE-A"——**注意:既有测试断言 title "mock-fix" 与 edit "ONE"!缺省从无 tag 变 "A" 会破坏既有断言**——执行时二选一:① 既有测试断言同步改(改 "mock-fix-A"/"ONE-A")或 ② mock 缺省 tag 为空字符串(无 tag → "mock-fix-"/"ONE-"?破坏更甚)。**建议:既有测试断言同步更新为带 tag 形态,并保留一个无 tag 单 server 测试确认缺省行为**——执行时按最简破坏面处理)
- **双 server 测试**:语言定义手写 TOML(挂两个 server):

```toml
[[language]]
name = "mock2"
scope = "source.mock2"
file-types = ["mock2"]
language-servers = ["mock-lsp-a", "mock-lsp-b"]

[language-server.mock-lsp-a]
command = "<bin>"
args = ["code_actions_basic", "A"]

[language-server.mock-lsp-b]
command = "<bin>"
args = ["code_actions_basic", "B"]
```

  测试(plugin_lsp_mock.rs 或新文件):`.mock2` 文件 + 该 loader → 插件列 actions(断言两个 title "mock-fix-A"/"mock-fix-B" 且 `_serverId` 不同)→ execute 第一个 → 断言文本 "ONE-A"(判别器:修复前固定取第一个 server——第一个 server 是 A,execute 第一个 action 恰好也是 A,那怎么区分?**第二个 action 才是判别器**:execute 第二个(action 带 B 的 _serverId)→ 修复前固定第一个 server(A)执行 → 文本 "ONE-A"(错);修复后 → "ONE-B"(对)——**测试 2 用 execute 第二个 action 断言 "ONE-B"**)
- **测试命令**:`cargo build -p helix-term`(实现者);`cargo test -p helix-term --features integration --test integration plugin_lsp_mock`(控制者);clippy 零;fmt clean(并行批次 fmt 漂移不算)。

---

### 任务 1:mock 扩展 + 双 server 测试(先红)

**文件:** mock_lsp.rs、plugin_lsp_mock.rs、helpers.rs(如需)

- [ ] **步骤 1:mock code_actions 场景 tag 化**(见关键细节;同步更新既有单 server 测试断言)
- [ ] **步骤 2:写失败双 server 测试**

`plugin_lsp_mock.rs`(或新文件)加(手写双 server TOML 经 `test_syntax_loader`):

```rust
// 双 server:列表带 _serverId,execute 按对应 server 执行(判别器:execute 第二个 action → "ONE-B")
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_multiserver_execute_routes_to_owner() -> anyhow::Result<()> {
    // .mock2 文件 + 双 server loader
    // 插件:actions = await code_actions(); echo titles; await execute(actions[1]); echo result
    // 断言:列表 title "mock-fix-A"|"mock-fix-B" 且 _serverId 不同;execute 第二个后文本 "ONE-B"
}
```

  预期:FAIL(列表无 _serverId,execute 固定第一个 → "ONE-A" ≠ "ONE-B")。

- [ ] **步骤 3:提交**

```bash
git add helix-term/src/bin helix-term/tests
git commit -m "test: mock code_actions 场景 tag 化 + 双 server 测试(execute 路由判别器,先红)"
```

---

### 任务 2:_serverId 注入/解析/应用 + 回归

**文件:** application.rs、plugin_lsp_mock.rs(转绿)、plugin-api.md

- [ ] **步骤 1:handle_lsp_code_actions 注入 _serverId**(见关键细节)
- [ ] **步骤 2:execute 与段 A 解析 + 回退**(见关键细节;`LanguageServerId::from(u64)` 构造按实际 API)
- [ ] **步骤 3:转绿 + 文档 + 提交**

```bash
cargo build -p helix-term
cargo test -p helix-term --features integration --test integration plugin_lsp_mock   # 控制者
cargo clippy --all-targets 2>&1 | tail -3
```

`docs/plugin-api.md` code_actions 小节注明:`_serverId` 内部字段(execute 按它路由;手动构造的 action 无此字段 → 用当前 buffer 第一个 CodeAction server;单 server 项目可忽略)。

```bash
git add helix-term/src/application.rs helix-term/tests docs/plugin-api.md
git commit -m "fix(term): execute_code_action 按 _serverId 路由对应 server——多 server 项目 action 不再错发;缺失回退第一个(兼容)"
```

---

## 自检记录

**规格覆盖度:**
- 2.1 API(_serverId 内部字段/execute 路由/回退)→ 任务 2 步骤 1-2、任务 1 步骤 2 ✓
- 2.2 数据流(注入/解析/剥离/mock 双 server)→ 任务 1 步骤 1-2、任务 2 步骤 1-2 ✓
- 2.3 边界(消失 server 回退/手动构造回退/可枚举/单 server 无害)→ 任务 2 步骤 2 ✓
- 3.1 双 server 测试 3 项 → 任务 1 步骤 2 ✓
- 3.2 回归(单 server 既有测试)→ 任务 1 步骤 1(断言同步)、任务 2 步骤 3 ✓

**占位符扫描:** 无 TODO/待定;LanguageServerId::from 构造标注"按实际 API 调整"。✓

**类型一致性:** `_serverId` 注入/解析/剥离三处一致;mock tag 在 mock/测试/断言一致。✓
