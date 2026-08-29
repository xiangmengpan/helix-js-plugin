# mock LSP server 测试基础设施实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** 提供确定性 mock LSP server(stdio JSON-RPC 二进制),解锁批次 4 LSP 应用路径(format/rename/code_actions)与现有 4 查询方法的真实响应集成测试。

**架构:** `helix-term/src/bin/mock_lsp.rs`(helix-term `[[bin]]` target,integration 测试经 `env!("CARGO_BIN_EXE_mock_lsp")` 拿路径)。场景经 **argv**(per-server 配置,无并行竞争)。测试用 `test_syntax_loader(overrides)` 注入 `mock` 语言 + `lsp.enable = true` config 启真 server。

**技术栈:** Rust(helix-term + serde_json)、LSP stdio 线协议(Content-Length 帧)。

**规格:** `docs/superpowers/specs/2026-08-28-js-mock-lsp-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-term/src/bin/mock_lsp.rs`(新) | mock server:stdio JSON-RPC 主循环 + 场景分发 + 各场景固定响应 |
| `helix-term/Cargo.toml` | 加 `[[bin]] name = "mock_lsp" path = "src/bin/mock_lsp.rs"` |
| `helix-term/tests/test/helpers.rs` | 加便捷方法:`test_config_with_lsp()`(test_config + lsp.enable=true)与 `mock_lsp_loader(scenario, extra_args)`(test_syntax_loader overrides 注入 mock 语言+server) |
| `helix-term/tests/integration.rs` | 注册 `mod plugin_lsp_mock;` |
| `helix-term/tests/test/plugin_lsp_mock.rs`(新) | 8 条集成测试 |
| `docs/plugin-api.md` | 不需要(测试设施,无 API 变更) |

关键实现细节(执行时必读):

- **线协议**:LSP over stdio = `Content-Length: <n>\r\n\r\n<body>` 帧(参照 helix-lsp/src/transport.rs:128/204)。mock 主循环:读帧 → serde_json 解析 → 按场景响应(写回同帧格式)→ shutdown/exit 退出。**读 EOF 时干净退出**(客户端关闭 stdin)。
- **请求-响应关联**:JSON-RPC `id` 原样回填;通知(无 id)不响应。
- **initialize 能力声明随场景**:

```toml
# 场景 → capabilities 子集
initialize_only → {}  # 最小,query 回 null
hover_basic → hover_provider: true
completion_basic → completion_provider: { triggerCharacters: [] }
goto_definition_basic → definition_provider: true
symbols_basic → document_symbol_provider: true
format_basic → document_formatting_provider: true
rename_cross_file / rename_stale → rename_provider: true
code_actions_basic → code_action_provider: true
```

- **响应数据**:
  - hover_basic → `Hover { contents: MarkupContent { kind: "markdown", value: "mock hover" }, range: null }`
  - completion_basic → `[ { label: "mock-item" } ]`
  - goto_definition_basic → `{ uri: <请求的 uri>, range: { start: {0,0}, end: {0,1} } }`
  - symbols_basic → `[ { name: "MockSymbol", kind: 13, range: {0,0}-{0,1}, selectionRange: 同 } ]`
  - format_basic → `[ { range: {start:{0,0}, end:{0,3}}, newText: "ONE" } ]`("one"→"ONE")
  - rename_cross_file → `WorkspaceEdit { document_changes: Edits([ 当前 uri(版本=请求版本, TextEdit 把 "old"→"new"), argv[2] 的文件 uri(版本 null, TextEdit 把 "old"→"new") ]) }`——**argv[2] = 第二文件绝对路径,由测试在 lang TOML 的 args 里传入**
  - rename_stale → 同 rename_cross_file 但版本固定 `0`(恒过期 → apply_workspace_edit DocumentChanged → null,**无需 sleep,确定性**)
  - code_actions_basic → `[ { title: "mock-fix", kind: "quickfix", edit: { changes: { <请求 uri>: [TextEdit 把 "one"→"ONE"] } } } ]`(edit 直接带,不触发 resolve_code_action)
- **插件端测试形态**(仿 plugin_lsp.rs):`helix.register_command("x", async () => { const r = await helix.lsp.format(); helix.echo(JSON.stringify(r)); })` → 断言状态栏 + doc 文本
- **helpers 便捷方法**:

```rust
/// test_config + 启用 LSP(默认测试禁 LSP)
pub fn test_config_with_lsp() -> Config {
    let mut config = test_config();
    config.editor.lsp.enable = true;
    config
}

/// 注入 mock 语言定义(扩展名 .mock)+ mock-lsp server(command = 本 crate 的 mock_lsp 二进制,args = [scenario, ...extra])
pub fn mock_lsp_loader(scenario: &str, extra_args: &[&str]) -> helix_core::syntax::Loader {
    let args_list: Vec<String> = std::iter::once(scenario.to_string())
        .chain(extra_args.iter().map(|s| s.to_string()))
        .collect();
    let args_toml = args_list
        .iter()
        .map(|a| format!("    \"{a}\","))
        .collect::<Vec<_>>()
        .join("\n");
    let overrides = format!(
        r#"
[[language]]
name = "mock"
scope = "source.mock"
file-types = ["mock"]
language-servers = ["mock-lsp"]
roots = []

[language-server.mock-lsp]
command = "{}"
args = [
{args_toml}
]
"#,
        env!("CARGO_BIN_EXE_mock_lsp"),
        args_toml = args_toml,
    );
    test_syntax_loader(Some(&overrides))
}
```

  (`CARGO_BIN_EXE_mock_lsp` 在 helix-term 的测试目标内可用——同 crate 的 bin target。`roots = []` 避免 tempdir 根标记缺失问题;根回退 cwd, mock 不关心)
- **测试命令**:`cargo test -p helix-term --features integration --test integration plugin_lsp_mock`(integration feature 门控);`cargo build -p helix-term`(编译 mock bin);clippy 零;fmt clean。

---

### 任务 1:mock 二进制 + 接线 + format/rename 测试

**文件:** src/bin/mock_lsp.rs(新)、Cargo.toml、helpers.rs、integration.rs、plugin_lsp_mock.rs(新,前 4 条测试)

- [ ] **步骤 1:mock 二进制(全场景)**

`helix-term/src/bin/mock_lsp.rs`(Cargo.toml 加 `[[bin]]`):

```rust
//! 测试用 mock LSP server:stdio JSON-RPC(Content-Length 帧)。
//! 场景经 argv[1] 选择(per-server 配置,无并行竞争);argv[2..] 为场景附加参数(如第二文件路径)。
//! 不支持的方法 → null;shutdown/exit → 退出;stdin EOF → 退出。

use std::io::{BufRead, BufReader, Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scenario = args.first().map(String::as_str).unwrap_or("initialize_only");
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    loop {
        // 读 Content-Length 帧
        let mut content_length = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 {
                return; // EOF
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                content_length = v.trim().parse::<usize>().ok();
            }
        }
        let len = content_length.unwrap_or(0);
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).unwrap();
        let msg: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let Some(response) = respond(&msg, scenario, &args) else { continue };
        let json = serde_json::to_string(&response).unwrap();
        write!(out, "Content-Length: {}\r\n\r\n{}", json.len(), json).unwrap();
        out.flush().unwrap();
    }
}

/// 返回 Some(响应) 或 None(通知不响应 / 已退出)
fn respond(msg: &serde_json::Value, scenario: &str, args: &[String]) -> Option<serde_json::Value> {
    let method = msg.get("method").and_then(|m| m.as_str())?;
    let id = msg.get("id").cloned();
    if method == "shutdown" {
        return Some(json!({ "jsonrpc": "2.0", "id": id, "result": null }));
    }
    if method == "exit" || (id.is_none() && method != "initialized") {
        return None; // exit 通知 / 未知通知 → 忽略
    }
    let result = match method {
        "initialize" => json!({
            "capabilities": capabilities(scenario),
        }),
        "initialized" => return None,
        "textDocument/hover" if scenario == "hover_basic" => json!({
            "contents": { "kind": "markdown", "value": "mock hover" },
        }),
        "textDocument/completion" if scenario == "completion_basic" => json!([
            { "label": "mock-item" }
        ]),
        "textDocument/definition" if scenario == "goto_definition_basic" => {
            let uri = msg["params"]["textDocument"]["uri"].clone();
            json!({ "uri": uri, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } } })
        }
        "textDocument/documentSymbol" if scenario == "symbols_basic" => json!([
            { "name": "MockSymbol", "kind": 13, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }, "selectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } } }
        ]),
        "textDocument/formatting" if scenario == "format_basic" => json!([
            { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "newText": "ONE" }
        ]),
        "textDocument/rename" if scenario.starts_with("rename_") => {
            let uri = msg["params"]["textDocument"]["uri"].clone();
            // 请求的 textDocument 是 TextDocumentIdentifier(无 version)——响应里的 version 由 mock 定:
            // 正常场景 null(不校验版本,恒可应用);rename_stale 场景 0(恒过期 → DocumentChanged → null,确定性)
            let version = if scenario == "rename_stale" { json!(0) } else { json!(null) };
            let mut edits = vec![json!({
                "textDocument": { "uri": uri, "version": version },
                "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "newText": "new" } ],
            })];
            if let Some(second) = args.get(1) {
                let second_uri = format!("file://{}", second);
                edits.push(json!({
                    "textDocument": { "uri": second_uri, "version": null },
                    "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "newText": "new" } ],
                }));
            }
            json!({ "documentChanges": edits })
        }
        "textDocument/codeAction" if scenario == "code_actions_basic" => {
            let uri = msg["params"]["textDocument"]["uri"].clone();
            json!([
                { "title": "mock-fix", "kind": "quickfix", "edit": { "changes": { uri: [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "newText": "ONE" } ] } } }
            ])
        }
        _ => return Some(json!({ "jsonrpc": "2.0", "id": id, "result": null })),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn capabilities(scenario: &str) -> serde_json::Value {
    let mut caps = serde_json::Map::new();
    let set = |caps: &mut serde_json::Map<String, serde_json::Value>, key: &str, v: serde_json::Value| {
        caps.insert(key.to_string(), v);
    };
    match scenario {
        "hover_basic" => set(&mut caps, "hoverProvider", json!(true)),
        "completion_basic" => set(&mut caps, "completionProvider", json!({ "triggerCharacters": [] })),
        "goto_definition_basic" => set(&mut caps, "definitionProvider", json!(true)),
        "symbols_basic" => set(&mut caps, "documentSymbolProvider", json!(true)),
        "format_basic" => set(&mut caps, "documentFormattingProvider", json!(true)),
        "rename_cross_file" | "rename_stale" => set(&mut caps, "renameProvider", json!(true)),
        "code_actions_basic" => set(&mut caps, "codeActionProvider", json!(true)),
        _ => {}
    }
    json!(caps)
}
```

  (`json!` 宏需要 serde_json;bin target 用 helix-term 的依赖——serde_json 已是 helix-term 依赖 ✓。`file://` uri 拼接:Unix 路径直接 `file://<abs>`;若 Windows 兼容需要 uri 构造,按 `lsp::Url::from_file_path` 的语义——**本仓测试环境是 Unix,直接拼接即可,注释说明**)
  `json!` 宏需要 `use serde_json::json;`——加到文件顶部。

  验证:`cargo build -p helix-term`(编译 bin);手工冒烟可选(echo 一个 initialize 帧管道进二进制)。

- [ ] **步骤 2:helpers 便捷方法**

`helix-term/tests/test/helpers.rs` 加 `test_config_with_lsp()` 与 `mock_lsp_loader()`(见关键细节代码;`use helix_core::syntax::Loader` 已 import;`CARGO_BIN_EXE_mock_lsp` 在测试目标内可用)。

- [ ] **步骤 3:写失败测试(format + rename)**

`helix-term/tests/integration.rs` 加 `mod plugin_lsp_mock;`。新建 `helix-term/tests/test/plugin_lsp_mock.rs`:

```rust
use super::*;

// format:mock 返回 TextEdit("one"→"ONE")→ 自动应用 + 摘要 resolve
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_format_applies() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("fmt.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-fmt", async () => {
            const r = await helix.lsp.format();
            helix.echo("fmt:" + JSON.stringify(r));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("format_basic", &[]))
            .build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":mock-fmt<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), r#"fmt:{"applied":true}"#);
            })),
            (Some(":mock-fmt<ret>"), Some(&|app| {
                let (_, doc) = current_ref!(app.editor);
                assert_eq!(doc.text().to_string(), "ONE\n", "format 编辑已应用");
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}

// rename 跨 buffer:当前 + 第二文件都变 + files 计数
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_rename_cross_file() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file_a = dir.path().join("a.mock");
    let file_b = dir.path().join("b.mock");
    std::fs::write(&file_a, "old\n")?;
    std::fs::write(&file_b, "old\n")?;
    let plugin_path = dir.path().join("ren.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-ren", async () => {
            const r = await helix.lsp.rename("new");
            helix.echo("ren:" + JSON.stringify(r));
        });
        "#,
    )?;
    let b_path = file_b.display().to_string();
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file_a, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("rename_cross_file", &[&b_path]))
            .build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":mock-ren<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                // files = workspace_edit_file_count(Edits 按条目)= 2
                assert_eq!(status.as_ref(), r#"ren:{"applied":true,"files":2}"#);
            })),
            (Some(&format!(":open {}<ret>", file_b.display())), Some(&|app| {
                let (_, doc) = current_ref!(app.editor);
                assert_eq!(doc.text().to_string(), "new\n", "第二文件被 rename 编辑");
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}

// rename 版本过期:mock 恒返回 version 0 → apply_workspace_edit 校验失败 → resolve null
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_rename_stale_version() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "old\n")?;
    let plugin_path = dir.path().join("ren-stale.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-ren-stale", async () => {
            const r = await helix.lsp.rename("new");
            helix.echo("stale:" + String(r));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("rename_stale", &[]))
            .build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":mock-ren-stale<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "stale:null", "过期版本 → null");
            })),
            (Some(":mock-ren-stale<ret>"), Some(&|app| {
                let (_, doc) = current_ref!(app.editor);
                assert_eq!(doc.text().to_string(), "old\n", "陈旧编辑不落地");
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}

// initialize_only:能力最小 → 查询回 null(顺带验证 capabilities 未声明路径)
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_no_capability_resolves_null() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("nocap.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-nocap", async () => {
            const r = await helix.lsp.hover();
            helix.echo("nocap:" + String(r));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("initialize_only", &[]))
            .build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":mock-nocap<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "nocap:null");
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

  预期:`cargo test -p helix-term --features integration --test integration plugin_lsp_mock` — 先跑(接线尚未完整?不,本步骤同时写了接线与测试;**严格 TDD:先只写测试 + 最小接线(helpers/loader 已加)→ 跑 → FAIL(mock bin 不存在/测试期望与实际不符)→ 加 bin 场景 → GREEN**。若实现顺序不便拆分,至少保证:format 测试在 mock 二进制落地前是红的(server 启动失败 → resolve null → 断言挂)后再转绿)。

- [ ] **步骤 4:转绿 + 提交**

```bash
cargo build -p helix-term
cargo test -p helix-term --features integration --test integration plugin_lsp_mock
```

  预期:4 条 PASS(server 正常启动、响应应用、摘要正确、stale 返回 null)。

```bash
git add helix-term/src/bin helix-term/Cargo.toml helix-term/tests
git commit -m "feat(test): mock LSP server 二进制(stdio JSON-RPC,argv 场景)+ 接线 + format/rename 应用测试"
```

---

### 任务 2:code_actions + 查询方法测试

**文件:** plugin_lsp_mock.rs(续 4 条)、plugin-api.md 不需要

- [ ] **步骤 1:code_actions 列表 + execute 测试**

```rust
// code_actions 两阶段:列表 JSON 可读 → execute 应用 edit
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_code_actions_execute() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("ca.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-ca", async () => {
            const actions = await helix.lsp.code_actions();
            if (actions === null || actions.length === 0) { helix.echo("ca:empty"); return; }
            const first = actions[0];
            helix.echo("ca:" + first.title + "|" + first.kind);
            const r = await helix.lsp.execute_code_action(first);
            helix.echo("exec:" + JSON.stringify(r));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("code_actions_basic", &[]))
            .build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":mock-ca<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "ca:mock-fix|quickfix");
            })),
            (Some(":mock-ca<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), r#"exec:{"applied":true}"#);
            })),
            (Some(":mock-ca<ret>"), Some(&|app| {
                let (_, doc) = current_ref!(app.editor);
                assert_eq!(doc.text().to_string(), "ONE\n", "code action 的 edit 已应用");
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

- [ ] **步骤 2:查询方法真实响应测试(hover/completion/goto/symbols)**

```rust
// 查询方法首次真实响应测试:内容结构可读
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_query_methods_real_responses() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("query.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-hover", async () => {
            const r = await helix.lsp.hover();
            helix.echo("hover:" + (r ? r.contents.value : "null"));
        });
        helix.register_command("mock-completion", async () => {
            const r = await helix.lsp.completion();
            helix.echo("comp:" + (r && r.length ? r[0].label : "null"));
        });
        helix.register_command("mock-def", async () => {
            const r = await helix.lsp.goto_definition();
            helix.echo("def:" + (r ? (Array.isArray(r) ? r[0].range.start.line : r.range.start.line) : "null"));
        });
        helix.register_command("mock-syms", async () => {
            const r = await helix.lsp.document_symbols();
            helix.echo("syms:" + (r && r.length ? r[0].name : "null"));
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader("hover_basic", &[]))
            .build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":mock-hover<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert_eq!(status.as_ref(), "hover:mock hover");
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

  说明:completion/goto/symbols 各自需要对应场景(completion_basic / goto_definition_basic / symbols_basic)——**每条查询方法一个独立测试**(loader 场景不同),hover 之外各写一条(共 4 条:query_real_hover / query_real_completion / query_real_goto / query_real_symbols),断言形态同上(读 label/line/name)。**执行时按实际响应 JSON 结构调整断言**(如 goto_definition 响应是单 Location,`r.range.start.line` 或数组形态;以 mock 返回值为准)。

- [ ] **步骤 3:全量验证 + 提交**

```bash
cargo test -p helix-term --features integration --test integration plugin_lsp_mock
cargo test -p helix-term --features integration --test integration plugin_lsp   # 既有 null 路径不回归
cargo test -p helix-js
cargo clippy --all-targets 2>&1 | tail -3
cargo fmt --all --check
```

  预期:plugin_lsp_mock 8 条 PASS;plugin_lsp 既有 3 条 PASS;helix-js 79 PASS;clippy 零;fmt clean。

```bash
git add helix-term/tests
git commit -m "test: code_actions 两阶段 + 4 查询方法真实响应测试(mock LSP)"
```

---

## 自检记录

**规格覆盖度:**
- 2.1 mock 二进制(位置/协议/argv 场景/场景集)→ 任务 1 步骤 1 ✓
- 2.2 测试接线(loader 注入/lsp.enable/并行安全)→ 任务 1 步骤 2 ✓
- 2.3 测试清单 8 条 → 任务 1 步骤 3(4 条)+ 任务 2 步骤 1-2(4 条)✓
- 2.4 边界(不支持方法回 null/无超时测试/进程随 app 关闭/initialize_only)→ 任务 1 步骤 1、3 ✓

**占位符扫描:** 无 TODO/待定;代码块为实际实现代码。执行时确认项(mock 响应 JSON 与客户端反序列化兼容、uri 拼接、TDD 顺序)已注明。✓

**类型一致性:** mock 场景名(format_basic/rename_cross_file/rename_stale/code_actions_basic/hover_basic/completion_basic/goto_definition_basic/symbols_basic/initialize_only)在 mock 二进制、helpers loader、测试三处一致;helpers 方法名(test_config_with_lsp/mock_lsp_loader)跨测试一致。✓
