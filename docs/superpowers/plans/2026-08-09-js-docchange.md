# doc-change 事件实现计划（特性 C）

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development 或 superpowers:executing-plans 实现此计划。步骤使用复选框（`- [ ]`）语法跟踪进度。
> **本次执行策略：** 自主执行——TDD 后测试通过即提交；控制者自动合并；失败自动重试。

**目标：** `helix.on("doc-change", (doc) => ...)`——用 helix idle 定时器（250ms）做防抖，当前文档 revision 前进时触发。

---

### 任务 1：doc-change 事件（白名单 + idle 挂点 + 集成测试）

**文件：**
- 修改：`helix-js/src/lib.rs`（白名单 + 单测）
- 修改：`helix-term/src/application.rs`（handle_idle_timeout 挂点 + last_revision 状态）
- 创建：`helix-term/tests/test/plugin_docchange.rs`（集成测试；integration.rs mod 声明控制器合并时加——不要改 integration.rs，验证时临时加、跑通后移除）

- [ ] **步骤 1：编写失败的 helix-js 单测**

在 `tests` 模块追加（沿用 `TEST_LOCK`）：

```rust
#[test]
fn doc_change_event() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.on("doc-change", (doc) => { helix.echo("changed:" + doc.cursor.row); });
        "#,
    )
    .unwrap();
    assert!(has_handlers("doc-change"));
    let ctx = CommandContext { path: None, text: "x".into(), cursor: (2, 0) };
    emit_event("doc-change", &ctx, None).unwrap();
    assert_eq!(take_messages(), vec!["changed:2"]);
    // 未注册的事件名仍然报错
    assert!(load_script(r#"helix.on("bogus", () => {});"#).is_err());
}
```

- [ ] **步骤 2：运行确认失败**

`cargo test -p helix-js` → FAIL（`doc-change` 不在白名单，`helix.on` 报错）。

- [ ] **步骤 3：实现 helix-js**

`EVENT_WHITELIST` 改为：

```rust
const EVENT_WHITELIST: [&str; 5] = ["save", "mode-change", "buffer-open", "buffer-close", "doc-change"];
```

- [ ] **步骤 4：helix-js 测试通过**

`cargo test -p helix-js` → 12 个 PASS；clippy 0 警告。

- [ ] **步骤 5：集成测试（失败先行）**

创建 `helix-term/tests/test/plugin_docchange.rs`（临时 mod 验证，跑通后移除 integration.rs 改动）：

```rust
use super::*;

use helix_core::diagnostic::Severity;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_doc_change_event() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("dc.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("docchange.js");
    std::fs::write(
        &plugin_path,
        r#"helix.on("doc-change", (doc) => { helix.echo("changed"); });"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            // 修改文本：i + 输入 + esc → idle 触发 doc-change → 状态栏 "changed"
            (
                Some("ix<esc>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "changed");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
```

> 注意：idle 时序——`ix<esc>` 后 event_loop_until_idle 会等 idle 触发（既有测试依赖同机制）。若状态栏断言不稳定（idle 未触发或时序竞争），在键序列后追加一个无害按键（如 `<esc>`）或调整断言为重试式（`app.editor.get_status()` 轮询？harness 不支持轮询——改用"触发后多送一个键事件"确保 idle 重排），以实测为准，报告中注明最终形态。

- [ ] **步骤 6：实现 application.rs 挂点**

`handle_idle_timeout`（application.rs:621）开头（构造 cx 之前）追加：

```rust
        // JS 插件 doc-change 钩子：idle 间隔 = 防抖；revision 前进才触发
        if helix_js::has_handlers("doc-change") {
            use std::cell::Cell;
            thread_local! {
                static LAST_REVISION: Cell<Option<usize>> = const { Cell::new(None) };
            }
            let (_, doc) = helix_view::current_ref!(self.editor);
            let rev = doc.history.get().current_revision();
            let changed = LAST_REVISION.with(|c| {
                let prev = c.get();
                if prev != Some(rev) {
                    c.set(Some(rev));
                    true
                } else {
                    false
                }
            });
            if changed {
                let _ = typed::emit_plugin_event(&mut self.editor, "doc-change", None);
            }
        }
```

> 注意：`doc.history` 是 pub 字段 `Cell<History>`，`doc.history.get().current_revision()`。`helix_view::current_ref!` 的导入路径以 application.rs 既有代码为准。`typed::emit_plugin_event(&mut self.editor, ...)` 的借用：current_ref! 的借用必须在 emit_plugin_event 前结束（revision 提取后立即 drop——把 `let (_, doc)` 与 rev 计算放一个块里，借用在块尾结束；emit_plugin_event 需要 &mut editor，先释放 & 借用）。若 self.editor 的借用冲突，先 `let rev = { let (_, doc) = current_ref!(self.editor); doc.history.get().current_revision() };` 再 emit。

- [ ] **步骤 7：跑测试**

`cargo test -p helix-term --features integration --test integration plugin_docchange`（临时 mod）→ PASS；`plugin` 全组回归 PASS；`cargo check -p helix-term` 无警告；clippy 无新增警告。

- [ ] **步骤 8：Commit**

```bash
git add helix-js helix-term
git commit -m "feat(js): doc-change event with idle-timer debounce"
```

---

## 自检记录

- 规格覆盖：白名单、idle 挂点 + revision 防抖、集成测试。
- 已知风险：idle 时序在测试 harness 的稳定性（重排方案已注明）；current_ref! 与 emit_plugin_event 的借用顺序（块作用域方案）；thread_local LAST_REVISION 在测试并行下的语义（每个线程独立——集成测试同线程，无碍）。
