# Task 1 报告：mock 二进制 + 接线 + format/rename 测试

## 状态：DONE

## 实现内容

### 1. mock LSP server 二进制（`helix-term/src/bin/mock_lsp.rs`，新）
- stdio JSON-RPC（Content-Length 帧），场景经 `argv[1]` 选择，`argv[2..]` 为附加参数（第二文件路径）
- 全场景覆盖（任务 2 也无需再改 bin）：initialize/hover/completion/definition/documentSymbol/formatting/rename/codeAction/initialize_only
- shutdown/exit → 退出；未知通知忽略；stdin EOF → 退出；不支持的方法 → result null
- `Cargo.toml` 加 `[[bin]] name = "mock_lsp"`（复用 helix-term 已有 serde_json 依赖）
- Unix 直接拼接 `file://<abs>`（注释说明；测试环境为 Unix）

### 2. helpers 接线（`helix-term/tests/test/helpers.rs`）
- `test_config_with_lsp()`：test_config + `lsp.enable = true`（launch_language_servers 的开关）
- `mock_lsp_loader(scenario, extra_args)`：TOML 覆盖合并进默认语言配置——`.mock` 文件类型 → `mock-lsp`，command = `env!("CARGO_BIN_EXE_mock_lsp")`，args = [场景, 附加...]（JSON 数组即合法 TOML 内联数组）

### 3. 4 条集成测试（`helix-term/tests/test/plugin_lsp_mock.rs`，新；`integration.rs` 注册 mod）
1. `plugin_lsp_mock_format_applies`：mock 返回 TextEdit("one"→"ONE") → 自动应用 + 摘要 `{"applied":true}`
2. `plugin_lsp_mock_rename_cross_file`：跨 buffer——当前 + 第二文件都变 + `files:2`
3. `plugin_lsp_mock_rename_stale_version`：过期版本 → `apply_workspace_edit` 校验失败 → resolve null，编辑不落地
4. `plugin_lsp_mock_no_capability_resolves_null`：initialize_only（无能力）→ hover 回 null

## TDD 证据
- RED：仅测试 + helpers（无 bin）→ `cargo test --no-run` 编译失败：
  `error: environment variable CARGO_BIN_EXE_mock_lsp not defined at compile time`（helpers.rs:272）
- GREEN：加 bin + Cargo.toml → `cargo test -p helix-term --features integration --test integration plugin_lsp_mock`：
  `test result: ok. 4 passed; 0 failed`（5.06s）

## 验证
- `cargo build -p helix-term`：通过（含新 bin）
- `cargo test ... plugin_lsp`：7 passed（3 旧 + 4 新）
- 手工冒烟：initialize 帧 → `{"capabilities":{"documentFormattingProvider":true}}` ✓（首次冒烟 panic 是冒烟命令 Content-Length 手算错误，非二进制缺陷）
- clippy（--all-targets）：新文件零警告（helpers.rs:505 为既有 `reload_file` 警告）；fmt clean
- 提交：`6b9cbb090`

## 与简报的偏离
- **rename_stale mock 版本 0 → 1**：简报写 0（"恒过期"），但代码事实是测试 buffer 初始 `version = 0`（`helix-view/src/document.rs`：`version: 0`，仅 `apply_impl` 变更时递增；这些测试全程无编辑）。mock 返回 0 == 文档版本 → 不触发 `DocumentChanged` → 编辑会应用、断言 `stale:null` 必挂。改 1 后恒过期（1 != 0）→ 确定性 null，符合简报意图。测试断言本身未动。

## 自审
- 完整性：简报步骤 1-4 全部落地；bin 已含任务 2 的 code_actions 场景（简报步骤 1 明定"全场景"）
- YAGNI：未加锁/未加抽象；测试沿用 plugin_lsp.rs 的 get_status 模式（无 DOC_CHANGE_TEST_LOCK，与既有 LSP 测试同风险档——echo 与 take_messages 同 pump 内完成）
- 潜在竞态（既有模式同款，非本任务引入）：`:plugin-load` 后的 idle 周期（250ms）内完成 initialize；若在 initialize 前触发请求，`client.capabilities.get().unwrap()` 会 panic——多轮运行未现

## 疑虑 / 记录
- 全量并行下 2 个既有测试 flaky：`plugin_terminal_hooks::buffer_traversal_and_focus`、`commands::write::test_write_concurrent`——单独跑、模块组跑均过；base（stash 掉本任务改动）全量跑更早 SIGABRT（87 tests 后 abort）。判定为环境并行负载不稳（项目进度台账已有 TEST_LOCK 中毒/消息队列竞态记录），与本任务改动无关。
- mock 测试每次各起一个 mock_lsp 进程（kill_on_drop 清理），无残留。
