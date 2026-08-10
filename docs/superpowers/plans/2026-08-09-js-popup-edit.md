# 弹窗编辑实现计划（v7）

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development 或 superpowers:executing-plans 实现此计划。步骤使用复选框（`- [ ]`）语法跟踪进度。
> **本次执行策略：** 自主执行——TDD 后测试通过即提交；控制者自动合并；失败自动重试。

**目标：** 弹窗 `onKey(key, doc)` 获得可编辑 doc——菜单选中即插入。

---

### 任务 1：弹窗编辑（单任务）

**文件：**
- 修改：`helix-js/src/lib.rs`
- 修改：`helix-term/src/ui/plugin_popup.rs`
- 创建：`helix-term/tests/test/plugin_popup_edit.rs`（集成测试；integration.rs 的 mod 声明控制器合并时统一加——不要改 integration.rs，验证时临时加、跑通后移除）

- [ ] **步骤 1：编写失败的 helix-js 单测**

在 `tests` 模块追加（沿用 `TEST_LOCK`）；并适配既有 popup_key 相关测试（新签名需传 ctx 参数）：

```rust
#[test]
fn popup_onkey_edits_doc() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.open_popup({
            render: () => ["a", "b"],
            onKey: (key, doc) => {
                if (key.name === "Enter") {
                    doc.insert(doc.cursor.row, doc.cursor.col, "XYZ");
                    return "close";
                }
                return "handled";
            },
        });
        "#,
    )
    .unwrap();
    let id = match take_ui_requests()[0] {
        UiRequest::OpenPopup { id } => id,
    };
    let ctx = CommandContext {
        path: Some("/tmp/p.rs".into()),
        text: "line1\nline2".into(),
        cursor: (1, 2),
        selection: ((0, 0), (0, 0)),
    };
    let key = PluginKey { name: "Enter".into(), shift: false, ctrl: false, alt: false };
    assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Close);
    assert_eq!(
        take_edits(),
        vec![Edit { start: (1, 2), end: (1, 2), insert: "XYZ".into() }]
    );
    close_popup(id).unwrap();
}
```

> 既有 `popup_lifecycle` / `popup_default_keys_and_validation` 测试里的 `popup_key(id, &key)` 调用都要加第三个参数 `&CommandContext{...}`（构造最小 ctx 即可，text 可为空）。

- [ ] **步骤 2：运行确认失败**

`cargo test -p helix-js` → 编译错误（`popup_key` 签名变化 + 新测试）。

- [ ] **步骤 3：实现 helix-js**

`popup_key` 签名与调用改：

```rust
pub fn popup_key(id: u64, key: &PluginKey, ctx: &CommandContext) -> Result<PopupKeyResult> {
    init();
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized");
        let callbacks = POPUPS.with(|p| p.borrow().get(&id).cloned())
            .ok_or_else(|| anyhow!("popup {id} not open"))?;
        let Some(on_key) = callbacks.on_key else {
            return Ok(if key.name == "Esc" { PopupKeyResult::Close } else { PopupKeyResult::Ignored });
        };
        let func = on_key.as_callable().and_then(JsFunction::from_object)
            .ok_or_else(|| anyhow!("popup {id} onKey is not a function"))?;
        let key_obj = /* 既有 key 对象构建不变 */;
        let doc = doc_to_js(ctx, engine).map_err(|e| anyhow!("failed to build popup doc: {e}"))?;
        let undefined = JsValue::undefined();
        let value: JsValue = func
            .call(&undefined, &[JsValue::from(key_obj), doc], engine)
            .map_err(|e| anyhow!("popup {id} onKey failed: {e}"))?;
        // 返回值解析不变
    })
}
```

> 注意：`doc_to_js` 是既有函数（事件 doc 构建，含 cursor + 编辑方法）——直接复用。`func.call(&undefined, &[key, doc], engine)` 两参数调用形式与 emit_event 的 mode-change 一致。

- [ ] **步骤 4：helix-js 测试通过**

`cargo test -p helix-js` → 17 个 PASS；clippy 0 警告。

- [ ] **步骤 5：helix-term 接线 + 集成测试**

`helix-term/src/ui/plugin_popup.rs` `handle_event`（当前签名 `handle_event(&mut self, event: &Event, _ctx: &mut Context)`）：

a) `_ctx` 改 `cx`，Key 分支改为：

```rust
        // 构建当前文档快照（弹窗是模态层，打开期间文档不变；每次按键重新序列化）
        let ctx = {
            let (view, doc) = current_ref!(cx.editor);
            let text = doc.text();
            let pos = doc.selection(view.id).primary().cursor(text.slice(..));
            let line = text.char_to_line(pos);
            let col = pos - text.line_to_char(line);
            let primary = doc.selection(view.id).primary();
            let anchor_line = text.char_to_line(primary.anchor);
            let head_line = text.char_to_line(primary.head);
            CommandContext {
                path: doc.path().map(|p| p.to_string_lossy().into_owned()),
                text: text.to_string(),
                cursor: (line, col),
                selection: (
                    (anchor_line, primary.anchor - text.line_to_char(anchor_line)),
                    (head_line, primary.head - text.line_to_char(head_line)),
                ),
            }
        };
        match helix_js::popup_key(self.id, &key, &ctx) {
            Ok(PopupKeyResult::Close) => { /* 既有 Close 逻辑 */ }
            Ok(PopupKeyResult::Handled) => EventResult::Consumed(None),
            Ok(PopupKeyResult::Ignored) | Err(_) => EventResult::Ignored(None),
        }
```

b) 编辑/光标/消息 drain（在 match 之前，无论结果）：

```rust
        let cursor_reqs = helix_js::take_cursor_requests();
        if !cursor_reqs.is_empty() {
            if let Err(err) = crate::commands::typed::apply_cursor_requests(cx.editor, &cursor_reqs) {
                cx.editor.set_error(format!("plugin popup cursor failed: {err}"));
            }
        }
        let edits = helix_js::take_edits();
        if !edits.is_empty() {
            if let Err(err) = crate::commands::typed::apply_plugin_edits(cx.editor, &edits) {
                cx.editor.set_error(format!("plugin popup edit failed: {err}"));
            }
        }
        let msgs = helix_js::take_messages();
        if !msgs.is_empty() {
            cx.editor.set_status(msgs.join(" "));
        }
```

> 注意：`apply_cursor_requests` / `apply_plugin_edits` 在 typed.rs 是 pub(crate) 还是私有——若私有需提升为 pub(crate)。`current_ref!` 的导入（`helix_view::current_ref!`）。Close 分支的既有 Callback 里还有 take_messages（echo 处理）——编辑/光标在 handle_event 内同步应用后，Callback 里只留 close_popup + pop + take_messages（若消息已被上面 drain 走，Callback 里 take_messages 为空——把消息 drain 移到 handle_event 后，Callback 里删掉 take_messages 逻辑，避免双消费；以既有代码结构为准做最小调整）。

c) 集成测试 `tests/test/plugin_popup_edit.rs`（临时 mod 验证，跑通后移除 integration.rs 改动）：

```rust
use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_popup_edits_document() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("pe.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("snippet.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("snippet", () => {
            const items = ["AAA", "BBB", "CCC"];
            let idx = 0;
            helix.open_popup({
                render: () => items.map((s, i) => (i === idx ? "> " + s : "  " + s)),
                onKey: (key, doc) => {
                    if (key.name === "Down") { idx = Math.min(items.length - 1, idx + 1); return "handled"; }
                    if (key.name === "Up") { idx = Math.max(0, idx - 1); return "handled"; }
                    if (key.name === "Enter") { doc.insert(doc.cursor.row, doc.cursor.col, items[idx]); return "close"; }
                    if (key.name === "Esc") return "close";
                    return "ignore";
                },
            });
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some(":snippet<ret>"),
                Some(&|app| {
                    let popup_type = std::any::type_name::<helix_term::ui::Popup<helix_term::ui::PluginPopup>>();
                    assert!(app.compositor.has_component(popup_type), "popup open");
                }),
            ),
            // Down×2 选中 CCC，Enter 插入 + 关闭
            (
                Some("j j<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "CCChello\n", "snippet inserted at cursor");
                    let popup_type = std::any::type_name::<helix_term::ui::Popup<helix_term::ui::PluginPopup>>();
                    assert!(!app.compositor.has_component(popup_type), "popup closed after Enter");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
```

> 注意：snippet 弹窗的键盘输入——popup 打开后是模态层，`j`/`<ret>` 会送到 PluginPopup（Compositor 把事件给栈顶层）。但 `j` 键：PluginPopup 的 onKey 只处理 Down/Up/Enter/Esc——`j` 会走 onKey 返回 "ignore"（fallthrough）→ 事件穿透到编辑器 → 编辑器 normal 模式 `j` 是下移（移动光标）——不影响测试（我们不关心光标）。为稳妥，测试用 `<down>` 键序列（`<down><down><ret>`）而不是 `j j<ret>`——`<down>` 直接是 KeyCode::Down ✓。以既有测试的键序列语法为准（`parse_macro` 支持 `<down>`）。

- [ ] **步骤 6：跑测试**

`cargo test -p helix-term --features integration --test integration plugin_popup_edit`（临时 mod）→ PASS；`plugin` 全组回归 PASS；`cargo check -p helix-term` 无警告；clippy 无新增警告。

- [ ] **步骤 7：Commit**

```bash
git add helix-js helix-term
git commit -m "feat(js): popup onKey receives editable doc handle"
```

---

## 自检记录

- 规格覆盖：popup_key 第三参 + doc 参数、drain 应用、测试。
- 已知风险：既有 popup_key 测试适配（签名变化）；apply_cursor_requests/apply_plugin_edits 的可见性（提升 pub(crate)）；Close 分支与 handle_event 的消息 drain 双消费（最小调整）；`<down>` 键序列语法（参照既有测试）。
