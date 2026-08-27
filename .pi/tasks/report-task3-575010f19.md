# 任务 3 实现报告：helix-term — LSP 泵循环接入（真链路）

计划：`docs/superpowers/plans/2026-08-26-js-lsp-request.md` 任务 3
Commit：`575010f19 feat(term): LSP 请求泵循环接入（无 server → null 链路）`

## 实现内容（TDD：RED → GREEN）

1. **集成测试** `helix-term/tests/test/plugin_lsp.rs`（新文件，注册进 integration.rs）：
   `plugin_lsp_no_server_resolves_null` — txt 文件（测试配置 lsp.enable=false，无 language
   server）下 `:lsp-hover` / `:lsp-syms` 两条 async 命令处理器走全链路：
   `helix.lsp.hover()` → 入队 → 泵 段 B → 无 server → 同步 resolve null → pump_jobs 执行
   async 续体 → `helix.echo` → 状态栏断言 `hover:null` / `syms:null`。
   实测确认 `register_command` 支持 async fn（run_command 调用后 promise 由泵循环的
   pump_jobs 驱动续体，无需 .then 改写）。

2. **application.rs 泵循环**（`pump_term_events`）：
   - 段 A：`drain_lsp_results()` → `resolve_lsp(id, result)`，置于 pump_jobs 之前
     （.then/async 续体同帧跑）。
   - 段 B：`take_lsp_requests()` → `handle_lsp_request(&self.editor, req)`，同样在
     pump_jobs 前（无 server 的同步 resolve 同帧兑现）。
   - **早退路径修复（计划未覆盖）**：原早退条件只查 term/async 事件空；LSP 结果/请求
     单独到达时会被早退吞掉 → promise 永不兑现。现将 `drain_lsp_results` /
     `take_lsp_requests` 一并 drain 并纳入空判断。

3. **`handle_lsp_request` 私有函数**（文件尾部）：
   - 方法 → `LanguageServerFeature` + `LspReqKind` 映射（Hover/Completion/
     GotoDefinition/DocumentSymbols）。
   - 无 server / capabilities 不支持（client 方法返回 None）→ 同步 `resolve_lsp(Ok(None))`，
     promise 不悬空。
   - 位置：显式 (row, col) 越界 **clamp 到文档末尾**（防 ropey `char_to_line` panic，
     JS 输入不可信）；None → `doc.position(view.id, offset_encoding)`（当前光标快照）。
   - 四个 client 方法返回不同的 opaque future 类型，经 `lsp_json` 泛型 helper 统一
     box 为 `Pin<Box<dyn Future<Output = Result<Option<Value>, String>> + Send>>`，
     `tokio::spawn` 发请求；goto_definition 响应注入 path（`inject_location_paths`）；
     结果经 `lsp_result_tx()` 克隆回 LSP_RESULTS 通道（WakeSender 唤醒渲染泵）。
   - 段 A 里 `LspResult` 先解构再 resolve（避免 partial move 后 log 借用）。

4. **helix-js/state.rs**：`WakeSender::send` 由 `pub(crate)` 提为 `pub`——helix-term 是
   跨 crate 调用方（`lsp_result_tx()` 返回的 WakeSender 本就作为跨 crate API 暴露）。

## TDD 证据

- RED：`cargo test -p helix-term --features integration --test integration plugin_lsp_no_server_resolves_null`
  → FAILED（`get_status().unwrap()` 得 None：请求无人处理、promise 不兑现、echo 未执行）。
- GREEN：同一命令 → `ok`。
- 回归：`cargo test -p helix-js` → 64 passed（任务 1/2 单测不受影响）。

## 验证

- `cargo check -p helix-term --features integration` → 无 error。
- `cargo test -p helix-term --features integration --test integration "plugin_"` → 69 passed;
  1 failed（`plugin_reload_command`）。**已用 `git stash` 复跑基线确认该失败为既有并行
  抖动**（无本任务改动时同样 68 passed; 1 failed），与本次变更无关；单跑通过。
- fmt：本任务改动行全部 fmt-clean（state.rs 的 drift 为既有全 crate 问题，任务 2 已记录，
  非本次引入）。

## 自审

- **YAGNI**：`LspReqKind` 枚举 + `lsp_json` helper 为合并 4 种 client future 类型的最小
  手段，无多余抽象；未新增依赖。
- **边界**：越界坐标 clamp；无 server/不支持 → null 短路（两处）；服务端错误/畸形 JSON
  由任务 2 的 resolve_lsp 处理（reject 不悬空）；`WakeSender::send` 失败仅丢弃（编辑器
  退出竞态，与现有 async 事件通道同语义）。
- **ponytail 标注**：无故意简化留下的已知上限（本任务无全局锁/朴素启发式）。

## 交接

任务 4（open_file 行列定位）可直接在 `plugin_lsp.rs` 追加测试，`helix.lsp.*` 四方法
已在真链路可用（本任务验证 null 路径；真实 server 路径同构，仅待有 LSP 环境手动验证——
计划任务 5 步骤 3）。
