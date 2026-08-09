# command palette 集成实现计划（并行特性 A）

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development 或 superpowers:executing-plans 实现此计划。步骤使用复选框（`- [ ]`）语法跟踪进度。

**目标：** 让插件命令出现在 `:space` 命令面板。改动极小（palette 数据源 + 集成测试）。

**背景：** 执行路径已通（v1 补全 + v4 MappableCommand 回退），只缺 palette 数据源。

---

### 任务 1：palette 数据源 + 集成测试（单任务）

**文件：**
- 修改：`helix-term/src/commands.rs`（command_palette）
- 创建：`helix-term/tests/test/plugin_palette.rs`
- 注意：`helix-term/tests/integration.rs` 的 `mod plugin_palette;` 声明由**控制器在合并时统一加**（另一个并行特性也在加 mod）——你不要改 integration.rs，避免并行冲突

- [ ] **步骤 1：编写失败的集成测试**

创建 `helix-term/tests/test/plugin_palette.rs`（先不注册 mod——测试文件单独存在即可，用 `cargo test --test integration plugin_palette` 跑会因 mod 未声明失败，这是预期的 TDD 红灯；或者你本地临时在 integration.rs 加 mod 跑通后**移除**，报告中注明你验证过——以能跑通为准，但**不要提交 integration.rs 的改动**）：

```rust
use super::*;

use helix_core::diagnostic::Severity;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_command_in_palette() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("pal.txt");
    std::fs::write(&file, "data\n")?;
    let plugin_path = dir.path().join("pal.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("pal-hello", () => { helix.echo("from-palette"); });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            // 打开命令面板，输入命令名过滤，回车执行
            (
                Some(":space"),
                Some(&|app| {
                    assert!(
                        app.compositor.find::<helix_term::ui::picker::Picker<helix_term::commands::MappableCommand>>().is_some()
                            || app.compositor.has_component(std::any::type_name::<helix_term::ui::picker::Picker<helix_term::commands::MappableCommand>>()),
                        "command palette should open"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
```

> 注意：`:space` 的 key 序列在测试宏里怎么写（`<space>` 或 `" "`）以既有 picker 测试为准（grep `:space` 或 `command_palette` 的既有测试）。palette 过滤输入后选中目标命令再 `<ret>` 的导航，参照既有 palette 测试（如 `test::commands::palette` 相关用例）——若找到既有 palette 测试，直接仿照其选择流程；断言点放在"插件命令被选中执行后状态栏出现 from-palette"。

- [ ] **步骤 2：实现 palette 数据源**

`helix-term/src/commands.rs` `command_palette`（L3597 附近）的 commands 迭代器追加：

```rust
            let commands = MappableCommand::STATIC_COMMAND_LIST.iter().cloned().chain(
                typed::TYPABLE_COMMAND_LIST
                    .iter()
                    .map(|cmd| MappableCommand::Typable {
                        name: cmd.name.to_owned(),
                        args: String::new(),
                        doc: cmd.doc.to_owned(),
                    }),
            ).chain(
                helix_js::command_names().into_iter().map(|name| MappableCommand::Typable {
                    name,
                    args: String::new(),
                    doc: String::new(),
                }),
            );
```

> 若 palette 对命令列表有排序/去重（`reverse_map()` 或后续 collect 逻辑），插件命令条目照常参与即可。

- [ ] **步骤 3：跑测试确认通过**

运行：`cargo test -p helix-term --features integration --test integration plugin_palette`（临时 mod 声明验证后移除）
预期：PASS。若 palette 导航在测试里不可行（选择流程复杂），退化为：断言 palette 打开后输入过滤能匹配到插件命令名（prompt 过滤是 fuzzy——输入 "pal-hello" 后断言选中项），并在报告中说明。

回归：`cargo test -p helix-term --features integration --test integration command_line` PASS；`cargo check -p helix-term` 无警告。

- [ ] **步骤 4：Commit**

```bash
git add helix-term/src/commands.rs helix-term/tests/test/plugin_palette.rs
git commit -m "feat(term): list plugin commands in the command palette"
```

---

## 自检记录

- 规格覆盖：palette 数据源 + 执行（现成路径）+ 集成测试。
- 已知风险：palette 导航/过滤在测试宏里的键序列写法（以既有 picker/palette 测试为参照）；`integration.rs` mod 声明留给控制器（避免并行冲突）。
