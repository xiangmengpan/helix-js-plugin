# yank-error 实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** 任何 `set_error` 的错误可事后复制(`:yank-error [reg]`,默认 `+` 寄存器)。

**架构:** `Editor.last_error: Option<Cow<str>>` 在 `set_error` 里记录(唯一改动点,所有错误路径自动覆盖);`:yank-error` 命令 Validate 时读它写寄存器,与 `yank_diagnostic` 平行。

**技术栈:** Rust(helix-view + helix-term)。

**规格:** `docs/superpowers/specs/2026-08-30-js-yank-error-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-view/src/editor.rs` | `Editor` 加 `last_error` 字段;`set_error` 记录 |
| `helix-term/src/commands/typed.rs` | `yank_error` 命令 + 注册(照 yank_diagnostic) |
| `helix-term/tests/test/plugin_yank_error.rs` | **新建**:integration |
| `helix-term/tests/integration.rs` | mod 注册 |

关键实现位置(执行时必读):

- **参照 `yank_diagnostic`**(typed.rs:3020-3045):`fn yank_diagnostic(cx, args, event)` + Validate 守卫 + `ensure!(s.chars().count() == 1)` 参数解析 + `cx.editor.registers.write(reg, diag)?` + `set_status`;注册在 typed.rs:4192(`name: "yank-diagnostic", fun: yank_diagnostic`)
- **`set_error`**(helix-view/src/editor.rs:1546):`self.status_msg = Some((error, Severity::Error))` 前加 `self.last_error = Some(error.clone())`;`error` 已是 `Cow<'static, str>`
- **Editor struct**:找 `pub status_msg` 字段(editor.rs 里 struct 定义),旁边加 `last_error` 字段;初始化处(Default/impl)补 `last_error: None`
- **registers.write**:`helix_view::register::Registers::write(&mut self, reg: char, values: impl IntoIterator<Item = String>) -> Result<()>`(照 yank_diagnostic 用法)

---

### 任务 1:Editor.last_error + set_error 记录

**文件:**
- 修改:`helix-view/src/editor.rs`
- 测试:editor.rs 或现有 editor 测试 mod

- [ ] **步骤 1:写失败测试**

editor.rs 的测试(若 editor.rs 无 tests mod,放 helix-view 现有单测位置;找不到就建最小 mod):

```rust
#[test]
fn set_error_records_last_error() {
    let mut editor = Editor::default(); // 或现有测试构造
    assert!(editor.last_error.is_none());
    editor.set_error("boom");
    assert_eq!(editor.last_error.as_deref(), Some("boom"));
    editor.set_error("boom2"); // 覆盖
    assert_eq!(editor.last_error.as_deref(), Some("boom2"));
}
```

预期:FAIL(`last_error` 字段不存在)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-view set_error_records_last_error 2>&1 | tail -5`

- [ ] **步骤 3:实现**

editor.rs:
- struct Editor 里 `pub status_msg` 附近加 `pub last_error: Option<Cow<'static, str>>`
- 初始化处补 `last_error: None`
- `set_error` 加一行记录(见规格)

- [ ] **步骤 4:运行测试确认通过**

运行:`cargo test -p helix-view set_error_records_last_error 2>&1 | tail -5` + `cargo build -p helix-term`(Editor 字段改动影响所有构造点,编译错会列出,逐个补 `..Default::default()` 或 `last_error: None`)

- [ ] **步骤 5:Commit**

```bash
git add helix-view/src/editor.rs
git commit -m "feat(view): Editor.last_error 记录——set_error 统一入口捕获最近错误"
```

---

### 任务 2:yank-error 命令 + integration

**文件:**
- 修改:`helix-term/src/commands/typed.rs`
- 创建:`helix-term/tests/test/plugin_yank_error.rs`
- 修改:`helix-term/tests/integration.rs`

- [ ] **步骤 1:写失败测试(integration)**

plugin_yank_error.rs(照 plugin_lsp.rs 的 test_key_sequences 模式):

```rust
// 插件 load 失败 → set_error → :yank-error 复制到寄存器
#[tokio::test(flavor = "multi_thread")]
async fn plugin_load_failure_yankable() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let bad_plugin = dir.path().join("bad.js");
    std::fs::write(&bad_plugin, "this is not valid js {{{")?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            // load 失败 → set_error("...failed...")
            (Some(&format!(":plugin-load {}<ret>", bad_plugin.display())), None),
            // yank-error 到 + 寄存器
            (Some(":yank-error<ret>"), None),
            // 校验寄存器内容包含错误文本
            (Some(":echo \"x\"<ret>"), Some(&|app| {
                let reg = app.editor.registers.read('+');
                assert!(reg.iter().any(|r| r.contains("failed")));
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

**注意**:`app.editor.registers.read('+')` 的返回类型照 yank 相关现有测试(看 tests 里有没有寄存器断言先例;若没有,改为通过 `:yank-error` 后 yank 粘贴验证,或直接查 registers)。`:plugin-load` 失败的错误文本是什么(typed.rs plugin_load 的报错格式),断言用子串匹配宽松些。

预期:FAIL(`:yank-error` 未注册)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term --features integration --test integration plugin_yank_error 2>&1 | tail -15`
预期:FAIL(unknown command)。

- [ ] **步骤 3:实现命令**

typed.rs 照 yank_diagnostic(3020)加:

```rust
fn yank_error(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let reg = match args.first() {
        Some(s) => {
            ensure!(s.chars().count() == 1, format!("Invalid register {s}"));
            s.chars().next().unwrap()
        }
        None => '+',
    };
    let Some(err) = cx.editor.last_error.clone() else {
        bail!("No error to yank");
    };
    cx.editor.registers.write(reg, [err.to_string()])?;
    cx.editor.set_status(format!("Yanked error to register {reg}"));
    Ok(())
}
```

注册(4192 旁):

```rust
static_ref!(yank_error, "yank-error", "yank-error [register]", "Copy the most recent error message to a register");
```

照 `yank_diagnostic` 的注册写法(static_ref! 或 typed command 宏——看 yank_diagnostic 注册处 4192-4195 的确切形式照抄)。

integration.rs:mod plugin_yank_error;

- [ ] **步骤 4:测试转绿**

运行:`cargo test -p helix-term --features integration --test integration plugin_yank_error 2>&1 | tail -15` + `cargo build -p helix-term`
预期:PASS + 编译过。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/commands/typed.rs helix-term/tests/test/plugin_yank_error.rs helix-term/tests/integration.rs
git commit -m "feat(term): :yank-error 复制最近错误(默认 + 寄存器,无错误报错)+ integration"
```

---

## 收尾

- [ ] `cargo fmt --all --check` + `cargo clippy -p helix-view -p helix-term 2>&1 | tail -3`(既有 warning 除外)+ `cargo test -p helix-view` + `cargo test -p helix-term --lib`
- [ ] 交接文档 `docs/superpowers/handoff/2026-08-30-js-yank-error.md`(简短:验证状态 + 边界:最近一条、不覆盖剪贴板)
- [ ] Commit 交接
