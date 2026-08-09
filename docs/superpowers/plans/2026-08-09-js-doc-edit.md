# JS 文档修改 API 实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 让 JS 插件修改当前文档：`ctx.doc.insert / replace / delete`，编辑队列化、命令结束原子应用为一个事务。

**架构：** 复用既有接线。helix-js 增加 thread_local 编辑队列 + 三个原生函数（挂在 ctx 的 doc 对象上）+ `take_edits()`；helix-term 分发钩子在 `run_command` 成功后 drain edits → row/col 转 char 索引 → `Transaction::change` → `doc.apply`。坐标基于命令开始时的原始快照，`doc.apply` 的 selection 重映射自动处理游标。

**技术栈：** 既有 helix workspace。无新依赖。

---

## 文件结构

- 修改：`helix-js/src/lib.rs` — Edit 类型、三个原生函数、take_edits、ctx doc 方法、单测
- 修改：`helix-term/src/commands/typed.rs` — 分发钩子应用编辑
- 修改：`helix-term/tests/test/plugin.rs` — 集成测试

---

### 任务 1：helix-js 编辑 API（Edit + 三个原生函数 + take_edits）

**文件：**
- 修改：`helix-js/src/lib.rs`
- 测试：`helix-js/src/lib.rs` 内 `#[cfg(test)] mod tests`

- [ ] **步骤 1：编写失败的测试**

在 `tests` 模块追加（沿用既有 `TEST_LOCK`）：

```rust
#[test]
fn doc_edits_queue() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.register_command("edit", (ctx) => {
            ctx.doc.insert(1, 2, "ab");
            ctx.doc.replace(0, 0, 0, 5, "new");
            ctx.doc.delete(3, 0, 4, 0);
        });
        "#,
    )
    .unwrap();
    let ctx = CommandContext { path: None, text: String::new(), cursor: (1, 2) };
    run_command("edit", &ctx).unwrap();
    let edits = take_edits();
    assert_eq!(
        edits,
        vec![
            Edit { start: (1, 2), end: (1, 2), insert: "ab".into() },
            Edit { start: (0, 0), end: (0, 5), insert: "new".into() },
            Edit { start: (3, 0), end: (4, 0), insert: String::new() },
        ]
    );
    // take_edits 清空
    assert!(take_edits().is_empty());
}

#[test]
fn doc_edit_validation() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    // 类型错误 → 命令失败（run_command 返回 Err），且队列被清空
    load_script(
        r#"
        helix.register_command("bad1", (ctx) => { ctx.doc.insert("x", 0, "a"); });
        helix.register_command("bad2", (ctx) => { ctx.doc.replace(0, 0, 0, 0, 42); });
        helix.register_command("bad3", (ctx) => { ctx.doc.delete(0, 0, 0); });
        "#,
    )
    .unwrap();
    let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0) };
    assert!(run_command("bad1", &ctx).is_err());
    assert!(take_edits().is_empty());
    assert!(run_command("bad2", &ctx).is_err());
    assert!(run_command("bad3", &ctx).is_err());
    assert!(take_edits().is_empty());
    // 正常命令运行后队列仍有值（供 helix-term 消费）
    load_script(r#"helix.register_command("ok", (ctx) => { ctx.doc.insert(0, 0, "z"); });"#).unwrap();
    run_command("ok", &ctx).unwrap();
    assert_eq!(take_edits().len(), 1);
}
```

> 注意：`run_command` 每次调用开始时清空队列（避免跨命令残留）。若 `bad1` 报错发生在入队前（参数解析失败），队列自然为空——断言 `take_edits().is_empty()` 验证的正是这一点。

- [ ] **步骤 2：运行测试确认失败**

运行：`cargo test -p helix-js`
预期：编译错误（`Edit`、`take_edits` 未定义）。

- [ ] **步骤 3：编写最少实现代码**

在 `helix-js/src/lib.rs`：

公开类型（放 `CommandContext` 附近）：

```rust
/// 一次文档编辑请求（坐标基于命令开始时的原始快照，0-based 行列）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub insert: String,
}
```

thread_local 追加（既有块内）：

```rust
    static CURRENT_EDITS: RefCell<Vec<Edit>> = RefCell::new(Vec::new());
```

`run_command` 开头追加（清空上次残留）：

```rust
    CURRENT_EDITS.with(|c| c.borrow_mut().clear());
```

`ctx_to_js` 的 doc 对象追加三个方法（在既有 `.property(...)` 之后）：

```rust
    let doc = ObjectInitializer::new(engine)
        .property("path", /* 不变 */)
        .property("text", /* 不变 */)
        .function(NativeFunction::from_fn_ptr(js_doc_insert), JsString::from("insert"), 3)
        .function(NativeFunction::from_fn_ptr(js_doc_replace), JsString::from("replace"), 5)
        .function(NativeFunction::from_fn_ptr(js_doc_delete), JsString::from("delete"), 4)
        .build();
```

原生函数与公共函数：

```rust
fn js_doc_insert(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let row: usize = args.get(0).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let col: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let insert: String = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    CURRENT_EDITS.with(|c| c.borrow_mut().push(Edit { start: (row, col), end: (row, col), insert }));
    Ok(JsValue::undefined())
}

fn js_doc_replace(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let sr: usize = args.get(0).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let sc: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let er: usize = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let ec: usize = args.get(3).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let insert: String = args.get(4).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    CURRENT_EDITS.with(|c| c.borrow_mut().push(Edit { start: (sr, sc), end: (er, ec), insert }));
    Ok(JsValue::undefined())
}

fn js_doc_delete(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let sr: usize = args.get(0).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let sc: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let er: usize = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let ec: usize = args.get(3).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    CURRENT_EDITS.with(|c| c.borrow_mut().push(Edit { start: (sr, sc), end: (er, ec), insert: String::new() }));
    Ok(JsValue::undefined())
}

/// 取走并清空编辑队列（helix-term 在命令返回后消费）
pub fn take_edits() -> Vec<Edit> {
    init();
    std::mem::take(&mut *CURRENT_EDITS.with(|c| c.borrow_mut()))
}
```

> 注意：`usize` 的 `TryFromJs` 在 boa 0.21 存在（整数转换，负数/小数/非数字报错）。若 `try_js_into::<usize>` 不可用，改用 `f64` 后 `as usize`（负数会被 clamp 成 0——坐标语义上可接受，但测试的"类型错误报错"分支会变弱；优先用 usize 直接转换，编译不过再降级并注明）。

- [ ] **步骤 4：运行测试确认通过**

运行：`cargo test -p helix-js`
预期：既有 5 个 + 新增 2 个测试全部 PASS。

- [ ] **步骤 5：Commit**

```bash
git add helix-js
git commit -m "feat(js): add ctx.doc.insert/replace/delete edit queue"
```

---

### 任务 2：helix-term 应用编辑（分发钩子 + 集成测试）

**文件：**
- 修改：`helix-term/src/commands/typed.rs`
- 修改：`helix-term/tests/test/plugin.rs`
- 测试：`cargo test -p helix-term --features integration --test integration plugin`

- [ ] **步骤 1：编写失败的集成测试**

在 `helix-term/tests/test/plugin.rs` 追加：

```rust
#[tokio::test(flavor = "multi_thread")]
async fn plugin_edit_document() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("t.txt");
    std::fs::write(&file, "world\n")?;
    let plugin_path = dir.path().join("edit_plugin.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("inshello", (ctx) => {
            ctx.doc.insert(ctx.cursor.row, ctx.cursor.col, "hello ");
        });
        helix.register_command("delfirst", (ctx) => {
            ctx.doc.delete(0, 0, 0, 5);
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some(":inshello<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "hello world\n");
                }),
            ),
            // 整个命令 = 一次撤销
            (
                Some("u"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "world\n");
                }),
            ),
            (
                Some(":delfirst<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "d\n");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
```

> 预期行为推演：初始 "world\n"，光标 (0,0)。:inshello → "hello world\n"（光标移到 5）。u 撤销 → "world\n"（光标回 0）。:delfirst 删 (0,0)-(0,5)="world" → "d\n"。

- [ ] **步骤 2：运行测试确认失败**

运行：`cargo test -p helix-term --features integration --test integration plugin::plugin_edit_document`
预期：FAIL——`:inshello` 后文本仍是 "world\n"（编辑未应用）。

> 注意：不要在测试里用 `(None, Some(fn))` 迭代——无按键时 `event_loop_until_idle` 会挂起（已在 plugin_bufferline_icons 测试中踩过）。

- [ ] **步骤 3：实现分发钩子应用编辑**

`helix-term/src/commands/typed.rs` 既有 `Ok(true)` 分支（drain ui_requests 之后）追加：

```rust
                    // 应用插件文档编辑（一个命令 = 一个事务）
                    let edits = helix_js::take_edits();
                    if !edits.is_empty() {
                        if let Err(err) = apply_plugin_edits(cx, &edits) {
                            cx.editor.set_error(format!("plugin edit failed: {err}"));
                        }
                    }
```

新增函数（放在 `plugin_load` 附近）：

```rust
/// 把插件 Edit（0-based 行列，原始快照坐标）转成 Transaction 并应用
fn apply_plugin_edits(cx: &mut compositor::Context, edits: &[helix_js::Edit]) -> anyhow::Result<()> {
    use helix_core::{Change, Transaction};

    let (view, doc) = current!(cx.editor);
    let text = doc.text();
    let to_char = |(row, col): (usize, usize)| -> usize {
        let line = row.min(text.len_lines().saturating_sub(1));
        let line_start = text.line_to_char(line);
        (line_start + col).min(text.len_chars())
    };
    let mut changes: Vec<Change> = edits
        .iter()
        .map(|e| Change::new(to_char(e.start), to_char(e.end), e.insert.clone()))
        .collect();
    changes.sort_by_key(|c| c.start);
    for w in changes.windows(2) {
        if w[0].end > w[1].start {
            bail!("overlapping edits");
        }
    }
    let txn = Transaction::change(text, changes);
    doc.apply(&txn, view.id);
    Ok(())
}
```

> 注意：`current!` 宏与 `Change`/`Transaction` 的导入（typed.rs 顶部可能已有部分）；`bail!` 需要 `anyhow::bail` 在作用域。`doc.apply` 的 selection 重映射自动把游标移到插入文本之后。重叠检测在排序后做；单个编辑内 end<start 的情况（用户传反）会让 Change::new 的 start>end——helix changeset 对此未定义，若编译/运行暴露此问题，在 to_char 后对每对 (start,end) 做 swap 规范化，并在报告中注明。

- [ ] **步骤 4：运行集成测试确认通过**

运行：`cargo test -p helix-term --features integration --test integration plugin`
预期：4 个测试（plugin_load_and_run_command / plugin_popup_open_and_close / plugin_bufferline_icons / plugin_edit_document）全部 PASS。

回归：`cargo test -p helix-term --features integration --test integration command_line` PASS；`cargo test -p helix-js` PASS。

- [ ] **步骤 5：Commit**

```bash
git add helix-term
git commit -m "feat(term): apply plugin document edits as one transaction"
```

---

## 自检记录

- **规格覆盖度**：insert/replace/delete 队列 + clamp + 原子事务 → 任务 1 步骤 3 + 任务 2 步骤 3；一次撤销 → 任务 2 集成测试 `u` 断言；类型校验 → 任务 1 doc_edit_validation。
- **占位符扫描**：无"待定/TODO"。一处实现不确定性（usize TryFromJs、end<start 规范化）已注明降级路径。
- **类型一致性**：`Edit { start: (usize, usize), end: (usize, usize), insert: String }` 任务 1 定义、任务 2 消费；`take_edits() -> Vec<Edit>` 签名跨任务一致。
- **已知风险**：`Transaction::change` 对重叠/乱序 changes 的行为（已排序 + 重叠检测）；`len_lines`/`line_to_char`/`len_chars` 均为 Rope 方法（helix-term 已大量使用）；`current!` 宏在本文件已有使用（L171 附近）。
