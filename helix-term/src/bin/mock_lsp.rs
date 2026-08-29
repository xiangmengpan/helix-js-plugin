//! 测试用 mock LSP server：stdio JSON-RPC（Content-Length 帧）。
//! 场景经 argv[1] 选择（per-server 配置，无并行竞争）；argv[2..] 为场景附加参数（如第二文件路径）。
//! 不支持的方法 → null；shutdown/exit → 退出；stdin EOF → 退出。
//! 注：file:// URI 拼接为 Unix 直接拼接（本仓测试环境为 Unix）；Windows 需按 lsp::Url::from_file_path 语义。

use serde_json::json;
use std::io::{BufRead, BufReader, Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scenario = args
        .first()
        .map(String::as_str)
        .unwrap_or("initialize_only");
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
        let Some(response) = respond(&msg, scenario, &args) else {
            continue;
        };
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
            // 请求的 textDocument 是 TextDocumentIdentifier(无 version)——响应里的 version 由 mock 定：
            // 正常场景 null(不校验版本,恒可应用)；rename_stale 场景 1(测试 buffer 初始 version=0，
            // 恒过期 → DocumentChanged → null,确定性；计划文档原写 0 与初始版本相等不触发过期)
            let version = if scenario == "rename_stale" {
                json!(1)
            } else {
                json!(null)
            };
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
        "textDocument/codeAction" if scenario == "code_actions_basic" => json!([
            { "title": "mock-fix", "kind": "quickfix", "edit": { "changes": { "uri": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "newText": "ONE" } ] } } }
        ]),
        _ => return Some(json!({ "jsonrpc": "2.0", "id": id, "result": null })),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn capabilities(scenario: &str) -> serde_json::Value {
    let mut caps = serde_json::Map::new();
    let set =
        |caps: &mut serde_json::Map<String, serde_json::Value>, key: &str, v: serde_json::Value| {
            caps.insert(key.to_string(), v);
        };
    match scenario {
        "hover_basic" => set(&mut caps, "hoverProvider", json!(true)),
        "completion_basic" => set(
            &mut caps,
            "completionProvider",
            json!({ "triggerCharacters": [] }),
        ),
        "goto_definition_basic" => set(&mut caps, "definitionProvider", json!(true)),
        "symbols_basic" => set(&mut caps, "documentSymbolProvider", json!(true)),
        "format_basic" => set(&mut caps, "documentFormattingProvider", json!(true)),
        "rename_cross_file" | "rename_stale" => set(&mut caps, "renameProvider", json!(true)),
        "code_actions_basic" => set(&mut caps, "codeActionProvider", json!(true)),
        _ => {}
    }
    json!(caps)
}
