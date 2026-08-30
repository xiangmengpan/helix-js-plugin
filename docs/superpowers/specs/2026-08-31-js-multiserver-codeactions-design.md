# 设计:多 server code_actions 修复(_serverId)

日期:2026-08-31
状态:已批准(brainstorming 一轮问答 + 三节设计确认)

关联:docs/superpowers/specs/2026-08-28-js-lsp-enhance-design.md(批次 4,execute 多 server 简化遗留)、docs/superpowers/specs/2026-08-28-js-mock-lsp-design.md(批次 5,mock 测试基设)

## 1. 动机

批次 4 的已知缺陷(审查记录"记入后续批次"):`code_actions()` 列表聚合**全部** CodeAction server 的 action,但 `execute_code_action()` 固定取当前 doc **第一个** server——多 server 项目(rust-analyzer + 其它)中,插件回传的 action 可能在错误 server 上执行。本设计用内部字段 `_serverId` 修复,保持向后兼容。

## 2. 设计

### 2.1 API

- `code_actions()` 列表项加**内部字段 `_serverId`**(LanguageServerId 数值;文档注明内部字段)
- `execute_code_action(action)`:**action 带 `_serverId` → 用对应 server 执行**;无 `_serverId`(插件手动构造)→ 回退第一个 CodeAction server(现状,兼容)
- 现有插件读 title/kind 不受影响(新增字段,非包装)

### 2.2 数据流

- `handle_lsp_code_actions`(application.rs:1883):保留 `ls_id`,每项 `serde_json::to_value` 后注入 `"_serverId": ls_id 数值`(LanguageServerId 是 slotmap Key,取值按实际 API,如 `data()`)
- `handle_lsp_execute_code_action` + 段 A `ExecuteAction` 应用(application.rs:2014):解析 `_serverId` → `editor.language_server_by_id` 反查 → 剥离 `_serverId` 后反序列化 `lsp::CodeActionOrCommand` → 用该 server 执行;缺失/查不到 → 回退第一个
- **mock 双 server 测试**:语言定义挂两个 server(`mock-lsp-a`/`mock-lsp-b`);mock 的 code_actions 场景按 argv[2] 标识返回不同 title/edit(args = ["code_actions_basic", "A"] → title "mock-fix-A" + edit "one"→"ONE-A")

### 2.3 边界

- `_serverId` 指向消失的 server → 回退第一个(不崩)
- 无 `_serverId`(手动构造)→ 回退第一个(兼容批次 4 早期用法)
- `_serverId` 可枚举内部字段(与批次 5 `_target` 同款;不可枚举会丢在 to_json 序列化里)
- 单 server 项目:列表项带 `_serverId`(无害),execute 用它,行为一致

## 3. 验证

### 3.1 integration(双 server)

1. 双 server 下 `code_actions()` 列表含两个 action,`_serverId` 不同、title 可区分
2. execute 第一个 → 应用**对应 server** 的 edit(白盒断言 "ONE-A" 而非 "ONE-B";判别器:修复前 execute 固定第一个 server)
3. 手动构造 action(无 _serverId)→ 回退第一个 server

### 3.2 回归

- 单 server 的既有 code_actions 测试(plugin_lsp_mock code_actions_execute)不回归

## 4. 规模

- helix-term:application.rs(handle_lsp_code_actions 注入 _serverId / execute 解析 / 段 A 应用)
- mock_lsp.rs(code_actions 场景 argv[2] 标识)、helpers.rs(双 server loader 或测试手写 TOML)
- 测试:integration(plugin_lsp_mock 或新文件)
- 约 2 任务:① mock 扩展 + 双 server 测试(先红)② _serverId 注入/解析/应用 + 回归
