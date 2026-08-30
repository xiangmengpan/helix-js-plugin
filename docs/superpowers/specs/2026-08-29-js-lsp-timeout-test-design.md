# 设计:LSP 超时路径测试 + mock 卫生收尾

日期:2026-08-29
状态:已批准(brainstorming 一轮问答 + 两节设计确认)

关联:docs/superpowers/specs/2026-08-28-js-mock-lsp-design.md(批次 5,mock 基设)、docs/superpowers/specs/2026-08-28-js-lsp-enhance-design.md(批次 4,超时语义已澄清:客户端层 per-server timeout)

## 1. 动机

批次 4 澄清了 LSP 请求超时语义(客户端层 `tokio::time::timeout` + per-server `timeout` 配置,默认 20s,到时 reject),但**超时路径零测试**——批次 5 的 mock 基设让补测便宜。本批次同时收尾批次 5 终审的 mock 卫生 Minor。

## 2. 设计

### 2.1 mock 卫生收尾(mock_lsp.rs)

- `fn file_uri(path: &str) -> String`:rename 第二文件分支的 uri 拼接集中处理(Unix 语义注释,未来可换 `Url::from_file_path`);当前文件分支已回显请求 uri 不动
- rename 场景前缀匹配 `starts_with("rename_")` → 显式 `"rename_cross_file" | "rename_stale"`(与 capabilities 写法对齐,防未来场景误继承)
- 头注释加**场景表**(场景 × 能力 × 响应,防漂移)

### 2.2 no_response 场景 + 超时测试

- **`no_response` 场景**:initialize 正常响应,其它所有方法**不响应**(return None 不退出)——请求挂起 → 客户端 timeout 触发
- `mock_lsp_loader_with_timeout(scenario, extra_args, timeout_secs)`(helpers.rs):TOML 加 `timeout = <n>`(现有 mock_lsp_loader 保持)
- 测试 1 条(plugin_lsp_mock.rs):
  - plugin:`try { await helix.lsp.hover(); echo("resolved:" + r); } catch (e) { echo("err:" + e.message); }`
  - loader:no_response + timeout 1s
  - 断言:status 含 "Timeout"(Error::Timeout 的 Display 格式,按实际调整)
  - 耗时 ~1-2s

## 3. 验证

- `cargo test -p helix-term --features integration --test integration plugin_lsp_mock`(10 条)
- 既有 9 条不回归;clippy 零;fmt clean

## 4. 规模

- mock_lsp.rs(uri helper/显式场景/注释/no_response)、helpers.rs(timeout 变体)、plugin_lsp_mock.rs(1 测试)
- 单任务即可;执行走 SDD 子代理 + 审查
