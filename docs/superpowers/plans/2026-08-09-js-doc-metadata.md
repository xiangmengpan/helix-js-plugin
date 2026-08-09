# 插件 doc 元数据实现计划（特性 D）

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development 或 superpowers:executing-plans 实现此计划。步骤使用复选框（`- [ ]`）语法跟踪进度。
> **本次执行策略：** 自主执行——TDD 后测试通过即提交；控制者自动合并；失败自动重试。

**目标：** `helix.register_command(name, fn, doc?)` 支持第三参说明，输入 `:命令` 时在命令行提示区显示（`command_line_doc` 回退）。

---

### 任务 1：helix-js doc 存储（单任务）

**文件：**
- 修改：`helix-js/src/lib.rs`
- 修改：`helix-term/src/commands/typed.rs`（command_line_doc 回退）
- 创建：`helix-term/tests/test/plugin_doc.rs`（集成测试；integration.rs 的 mod 声明由控制器合并时统一加——不要改 integration.rs，验证时临时加、跑通后移除）

- [ ] **步骤 1：编写失败的 helix-js 单测**

在 `tests` 模块追加（沿用 `TEST_LOCK`）：

```rust
#[test]
fn command_docs() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.register_command("doc1", () => {}, "first doc");
        helix.register_command("nodoc", () => {});
        "#,
    )
    .unwrap();
    assert_eq!(command_doc("doc1"), Some("first doc".to_string()));
    assert_eq!(command_doc("nodoc"), None);
    assert_eq!(command_doc("missing"), None);
    // 非法 doc 类型 → 报错
    assert!(load_script(r#"helix.register_command("bad", () => {}, 42);"#).is_err());
}
```

- [ ] **步骤 2：运行确认失败**

`cargo test -p helix-js` → 编译错误（`command_doc` 未定义）。

- [ ] **步骤 3：实现 helix-js**

在 `helix-js/src/lib.rs`：

thread_local 追加：

```rust
    static COMMAND_DOCS: RefCell<HashMap<String, String>> = const { RefCell::new(HashMap::new()) };
```

`js_register_command` 末尾（插入 REGISTRY 后）追加：

```rust
    // 可选第三参：命令说明
    if let Some(doc_arg) = args.get(2) {
        if !doc_arg.is_null_or_undefined() {
            let doc: String = doc_arg.try_js_into(context)?;
            COMMAND_DOCS.with(|d| d.borrow_mut().insert(name.clone(), doc));
        }
    }
```

> 注意：js_register_command 的 `name` 变量当前是 String（`try_js_into`）——确认变量名与所有权，插入 REGISTRY 后 name 仍可用则直接用，否则 clone。`is_null_or_undefined` 若不存在用 `is_null() || is_undefined()`。

公共函数：

```rust
/// 插件命令说明（未注册返回 None）
pub fn command_doc(name: &str) -> Option<String> {
    init();
    COMMAND_DOCS.with(|d| d.borrow().get(name).cloned())
}
```

- [ ] **步骤 4：helix-js 测试通过**

`cargo test -p helix-js` → 11 个 PASS；clippy 0 警告。

- [ ] **步骤 5：集成测试 + command_line_doc 回退**

创建 `helix-term/tests/test/plugin_doc.rs`（临时 mod 验证，跑通后移除 integration.rs 改动）：

```rust
use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_command_doc_shown() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("doc.js");
    std::fs::write(
        &plugin_path,
        r#"helix.register_command("docdemo", () => {}, "演示命令说明");"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            // 输入命令名，检查提示区 doc——若 prompt doc 无法从测试断言，此步改为
            // 输入完整命令并执行（证明命令可用），doc 显示靠单测 command_doc 覆盖
            (
                Some(":docdemo"),
                Some(&|app| {
                    // prompt 组件存在即可（doc 文本显示在 prompt 内，难以外部断言）
                    let prompt_open = app
                        .compositor
                        .has_component(std::any::type_name::<helix_term::ui::Prompt>());
                    assert!(prompt_open, "prompt should be open while typing");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
```

> 集成测试的 doc 显示断言若不可行（prompt 内部渲染），退化为"单测 command_doc 覆盖 + 集成验证命令可用"，报告中注明。

`command_line_doc`（typed.rs:4408）改为：

```rust
fn command_line_doc(input: &str) -> Option<Cow<'_, str>> {
    let (command, _, _) = command_line::split(input);
    let command = TYPABLE_COMMAND_MAP.get(command);
    let command = match command {
        Some(cmd) => cmd,
        None => {
            // 插件命令 doc 回退
            return helix_js::command_doc(&command_line::split(input).0)
                .map(Cow::Owned);
        }
    };
    // ...既有逻辑不变...
}
```

> 注意：`command_line::split(input)` 调用两次（一次取 command、一次在回退里）——重构为一次赋值更干净：`let (command, _, _) = command_line::split(input);` 后 `TYPABLE_COMMAND_MAP.get(&command)` 与回退都用 `command` 变量。以实际代码结构为准。

- [ ] **步骤 6：跑测试**

`cargo test -p helix-term --features integration --test integration plugin_doc`（临时 mod）→ PASS；`command_line` 回归 PASS；`cargo check -p helix-term` 无警告。

- [ ] **步骤 7：Commit**

```bash
git add helix-js helix-term
git commit -m "feat(js): plugin command docs shown in command line prompt"
```

---

## 自检记录

- 规格覆盖：register_command 第三参、command_doc、command_line_doc 回退、测试。
- 已知风险：js_register_command 的 name 所有权（clone 处理）；command_line_doc 重构（一次 split）；集成测试 doc 显示断言可行性（退化路径已注明）。
