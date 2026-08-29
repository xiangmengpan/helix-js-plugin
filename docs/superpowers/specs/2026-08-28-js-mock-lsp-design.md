# 设计:mock LSP server 测试基础设施

日期:2026-08-28
状态:已批准(brainstorming 两轮问答 + 三节设计确认)

关联:docs/superpowers/specs/2026-08-28-js-lsp-enhance-design.md(批次 4 应用路径是本设施要解锁测试的对象)

## 1. 动机

批次 3-4 的 LSP 功能(format/rename/code_actions 自动应用、execute 分发)是**零 integration 覆盖**的最大盲区——测试环境默认 `lsp.enable: false` 且无 mock server,集成测试只能走"无 server → resolve null"路径。本设施提供确定性 mock LSP server,解锁真实响应/编辑应用的端到端测试。

## 2. 设计

### 2.1 mock server 二进制

- 位置:`helix-term/src/bin/mock_lsp.rs`(helix-term crate 的 `[[bin]]` target)——integration 测试用 `env!("CARGO_BIN_EXE_mock_lsp")` 拿路径
- 协议:stdio JSON-RPC(与 helix-lsp 客户端同传输);主循环读 stdin 逐条 MethodCall,按场景响应;shutdown/exit 干净退出;不支持的方法回 null
- **场景经 argv**(非 env——env 是进程级,并行测试互踩;server 命令行 args 是 per-server 配置,无竞争)
- 场景集(initialize 能力声明随场景):
  - `initialize_only`:只答 initialize + shutdown/exit,其余回 null(最小能力)
  - `hover_basic` / `completion_basic` / `goto_definition_basic` / `symbols_basic`:各查询方法固定响应
  - `format_basic`:formatting → 固定 TextEdit("one"→"ONE")
  - `rename_cross_file`:rename → WorkspaceEdit(当前文件 + 另一文件)
  - `code_actions_basic`:codeAction → 固定 CodeAction(带 edit)
  - 可扩展:加场景 = 加测试

### 2.2 测试接线

- 语言定义注入:`test_syntax_loader(overrides)` TOML——语言 `mock`(扩展名 `.mock`)+ `[language-server.mock-lsp] command = env!("CARGO_BIN_EXE_mock_lsp"), args = ["<场景>"]`
- LSP 启用:`AppBuilder::with_config` 设 `editor.lsp.enable = true`(helpers 可加便捷方法)
- 每个测试:`.mock` 文件 + 启用 LSP 的 config + mock 语言 loader → 插件 load → 断言
- 并行安全:mock 是 per-app 进程 spawn,各自 stdio,无全局状态,无需锁

### 2.3 测试清单(新文件 `helix-term/tests/test/plugin_lsp_mock.rs`,8 条)

1. format 应用 + 摘要:await format → doc 文本 "one"→"ONE" + resolve `{applied:true}`
2. rename 跨 buffer:当前 + 第二文件都变 + files 计数
3. rename 版本过期:请求后编辑 doc → mock 返回旧 version → 应用失败 → resolve null(不落地陈旧编辑)
4. code_actions 列表 + execute:列表 JSON(title/kind 可读)→ execute → `{applied:true}` + 文本变化
5-8. 查询方法真实响应:hover 内容可读("mock hover")、completion 数组、goto_definition location、document_symbols 数组

### 2.4 边界

- 场景经 argv(无并行竞争);不支持的方法回 null
- 超时测试不做(选项 C 排除;后续可加"永不响应"场景 + 短 timeout 配置)
- mock 进程随 app 关闭退出(泄漏防护);`initialize_only` 顺带验证"能力未声明 → null"路径

## 3. 验证

- `cargo test -p helix-term --features integration --test integration plugin_lsp_mock`(8 条)
- 既有 plugin_lsp(null 路径)与其它测试不回归;clippy 零警告;fmt clean
- mock 二进制自身:启动/初始化/响应正确性由集成测试端到端验证(不另设单测)

## 4. 规模

- helix-term:src/bin/mock_lsp.rs(新,~150 行场景分发)、Cargo.toml([[bin]])、tests/test/plugin_lsp_mock.rs(新,8 测试)、helpers.rs(可选便捷方法)
- 约 2 任务:① mock 二进制 + 接线 + format/rename 测试 ② code_actions + 查询方法测试 + 边界
