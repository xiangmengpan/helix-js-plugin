# 任务 2 实现报告：helix-js — LSP 响应分发 resolve/reject + path 注入

计划：`docs/superpowers/plans/2026-08-26-js-lsp-request.md` 任务 2
Commit：`4997c6d86 feat(js): LSP 响应分发 resolve/reject + path 注入`

## 现状与接手点

任务 1 已提交（`6ff5d21cb`）。工作区里 `helix-js/src/lsp.rs` 已有一份未提交的
响应分发实现（上一会话遗留），但依赖的 `with_lsp_results` / `with_lsp_results_rx`
在 state.rs 中不存在，编译失败（RED 确认：12 个编译错误，其中 2 个 E0425 + 一堆
E0277 JsError 非 Send/Sync 的 `?` 转换错误）。

## 实现内容（TDD：RED → GREEN）

1. **state.rs**：新增 `LSP_RESULTS` / `LSP_RESULTS_RX` thread-local 通道槽
   （`WakeSender<LspResult>` / `mpsc::Receiver<LspResult>`，与 ASYNC_EVENTS 同模式）
   + `with_lsp_results` / `with_lsp_results_rx` 访问器。
2. **lib.rs `init()`**：与 ASYNC_EVENTS 并列创建 LSP_RESULTS 通道
   （`wake_sender` 机制，worker 发结果时唤醒 JS 泵）。
3. **lsp.rs**：修复计划示例代码的编译问题——`resolve_lsp` 内
   `js_json_parse(...)?` 的 JsError → anyhow 转换不成立（JsError 非 Send/Sync）。
   改为 match：解析失败 → reject 该 promise（错误信息进 e.message），promise 不悬空。
   行为优于计划（计划代码在此路径会直接编译失败）。
4. **lsp.rs 既有（继承）**：`LspResult` / `lsp_result_tx` / `drain_lsp_results` /
   `resolve_lsp` / `inject_location_paths` + `file_uri_to_path`（剥 `file://` 前缀 +
   百分号解码，未新增 `url` 依赖，按计划优先简单剥离）。
5. **测试**：`lsp_result_dispatch`（Ok(None)→null / Ok(Some(json))→parse 后 resolve /
   Err→reject 带 message，经 `pump_jobs` 兑现）、`location_path_injection`
   （Location uri / LocationLink target_uri / 单对象形态 / 非 file:// 不加 path）。

## 验证

- RED：`cargo test -p helix-js lsp_result_dispatch` → 编译失败
  （`with_lsp_results`/`with_lsp_results_rx` 未定义 + E0277）
- GREEN：`cargo test -p helix-js` → 64 passed; 0 failed
  （含 `lsp_result_dispatch`、`location_path_injection`、任务 1 的 `lsp_request_enqueue_shape`）
- 依赖侧：`cargo check -p helix-term` → Finished（API 纯增量，无破坏）

## 自审

- **fmt**：helix-js 全 crate 存在大范围既有 rustfmt 漂移（commands.rs/layout.rs/
  popup.rs/shell.rs 等所有文件均被 `cargo fmt --check` 标记，含任务 1 已提交代码），
  非本任务引入，未做全 crate 重排（越界）。本任务代码与文件内既有风格一致。
- **YAGNI**：`file_uri_to_path` 用字符串剥离而非 url crate——计划明确"优先用简单剥离，
  避免新增依赖"。
- **边界**：`resolve_lsp` 幂等（未知/已兑现 id → no-op）；畸形服务端 JSON → reject
  而非悬空 promise；`inject_location_paths` 对非 `file://` URI 不加 path。

## 交接

任务 3（helix-term 泵循环）可用 `helix_js::drain_lsp_results` / `helix_js::resolve_lsp` /
`helix_js::take_lsp_requests` / `helix_js::lsp_result_tx()`（均 pub，lib.rs 已
`pub use lsp::*`）。
