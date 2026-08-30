# LSP 超时路径测试 + mock 卫生收尾实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** 用 mock 基设补上 LSP 请求超时路径的集成测试(客户端 per-server timeout → reject),并收尾 mock 基设的卫生 Minor。

**架构:** mock_lsp.rs 加 `no_response` 场景(initialize 响应、其余方法永不响应);helpers 加 `mock_lsp_loader_with_timeout`(TOML 注入短 timeout);测试断言 promise reject 的 Timeout 错误。

**技术栈:** Rust(helix-term 测试基设)、LSP 客户端层既有 timeout 机制(批次 4 澄清:tokio::time::timeout + per-server timeout 配置)。

**规格:** `docs/superpowers/specs/2026-08-29-js-lsp-timeout-test-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-term/src/bin/mock_lsp.rs` | uri helper、rename 显式场景名、头注释场景表、`no_response` 场景 |
| `helix-term/tests/test/helpers.rs` | `mock_lsp_loader_with_timeout` 变体 |
| `helix-term/tests/test/plugin_lsp_mock.rs` | 1 条超时测试 |

关键实现细节(执行时必读):

- **`no_response` 场景**:`respond()` 里 `"initialize"` 走正常响应;`no_response` 时其它所有方法 `return None`(不响应、不退出,进程活着等客户端 timeout 触发)。注意**不能**在 initialize 前就挂——客户端 initialize 成功才发业务请求。
- **uri helper**:rename 第二文件分支 `file_uri(path)`;当前文件分支回显请求 uri 不动。注释注明 Unix 语义(`file://<abs>`),未来可换 `Url::from_file_path`。
- **rename 显式名**:`"textDocument/rename" if matches!(scenario, "rename_cross_file" | "rename_stale")`(capabilities 已是显式双名)。
- **timeout 变体**:

```rust
/// mock_lsp_loader + 显式 timeout(秒);超时测试用短值(如 1s)
pub fn mock_lsp_loader_with_timeout(
    scenario: &str,
    extra_args: &[&str],
    timeout_secs: u64,
) -> helix_core::syntax::Loader {
    let mut args = vec![scenario.to_string()];
    args.extend(extra_args.iter().map(|s| s.to_string()));
    let toml = format!(
        r#"
[[language]]
name = "mock"
scope = "source.mock"
file-types = ["mock"]
language-servers = ["mock-lsp"]

[language-server.mock-lsp]
command = "{bin}"
args = {args}
timeout = {timeout}
"#,
        bin = env!("CARGO_BIN_EXE_mock_lsp"),
        args = serde_json::to_string(&args).unwrap(),
        timeout = timeout_secs,
    );
    test_syntax_loader(Some(toml))
}
```

- **测试**:

```rust
// LSP 超时路径:mock 永不响应 → 客户端 per-server timeout(1s)→ promise reject
#[tokio::test(flavor = "multi_thread")]
async fn plugin_lsp_mock_timeout_rejects() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("timeout.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("mock-timeout", async () => {
            try {
                const r = await helix.lsp.hover();
                helix.echo("resolved:" + String(r));
            } catch (e) {
                helix.echo("err:" + e.message);
            }
        });
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(file, None)
            .with_config(test_config_with_lsp())
            .with_lang_loader(mock_lsp_loader_with_timeout("no_response", &[], 1))
            .build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            // 断言绑同一步:1s 超时在 idle 内触发,reject → catch → echo
            (Some(":mock-timeout<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert!(status.as_ref().contains("Timeout"), "status: {status}");
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

- **断言注意**:Error::Timeout 的 Display 形如 `Timeout(<id>)`;`e.message` 可能带前后缀——用 `contains("Timeout")` 而非精确匹配(按实际输出调整)。
- **测试耗时**:~1s(timeout 1s)+ 余量;若 idle 循环内 1s 超时未触发(harness idle 语义),改用 2s 或按实际调。
- **测试命令**:`cargo build -p helix-term`(实现者);`cargo test -p helix-term --features integration --test integration plugin_lsp_mock`(控制者);clippy 零;fmt clean。

---

### 任务 1:mock 卫生 + no_response + 超时测试(单任务)

**文件:** mock_lsp.rs、helpers.rs、plugin_lsp_mock.rs

- [ ] **步骤 1:mock 卫生收尾 + no_response 场景**

`helix-term/src/bin/mock_lsp.rs`:
1. 头注释加场景表(场景 × 能力 × 响应,含 no_response)
2. `fn file_uri(path: &str) -> String` + rename 第二文件分支改用
3. rename 前缀匹配改显式 `matches!(scenario, "rename_cross_file" | "rename_stale")`
4. `no_response` 场景:`initialize` 正常,其余方法 return None(在 match 前判断:`if scenario == "no_response" && method != "initialize" { return None; }`)

- [ ] **步骤 2:helpers timeout 变体**(见关键细节代码)

- [ ] **步骤 3:写失败测试 + 转绿**

`plugin_lsp_mock.rs` 加超时测试(见关键细节)。运行:
`cargo build -p helix-term`(mock 编译)后由控制者跑 `cargo test -p helix-term --features integration --test integration plugin_lsp_mock`——预期 10 条全绿。若超时未在 idle 内触发,按实际调 timeout 值/断言。

- [ ] **步骤 4:全量验证 + Commit**

```bash
cargo build -p helix-term
cargo test -p helix-term --features integration --test integration plugin_lsp_mock   # 控制者
cargo clippy --all-targets 2>&1 | tail -3
cargo fmt --all --check
```

预期:10 条 PASS;clippy 零;fmt clean。

```bash
git add helix-term/src/bin/mock_lsp.rs helix-term/tests
git commit -m "test: LSP 超时路径(mock no_response + 短 timeout → reject)+ mock 卫生(uri helper/显式场景/场景表注释)"
```

---

## 自检记录

**规格覆盖度:**
- 2.1 卫生(uri helper/显式场景/场景表)→ 任务 1 步骤 1 ✓
- 2.2 no_response + timeout 变体 + 测试 → 任务 1 步骤 1-3 ✓
- 3 验证(10 条/不回归/clippy/fmt)→ 任务 1 步骤 4 ✓

**占位符扫描:** 无 TODO/待定;代码为实际实现代码;断言格式与超时触发时机注明按实际调整。✓

**类型一致性:** no_response 场景名、mock_lsp_loader_with_timeout 签名、file_uri 函数在三文件一致。✓
