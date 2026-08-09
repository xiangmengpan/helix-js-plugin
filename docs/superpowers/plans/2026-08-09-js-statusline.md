# 状态栏元素实现计划（并行特性 B）

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development 或 superpowers:executing-plans 实现此计划。步骤使用复选框（`- [ ]`）语法跟踪进度。

**目标：** `helix.set_statusline(fn)`——插件在状态栏右侧渲染自定义文本。

---

### 任务 1：helix-js 状态栏钩子（StatuslineCtx + js_set_statusline + statusline_text）

**文件：**
- 修改：`helix-js/src/lib.rs`
- 测试：`helix-js/src/lib.rs` 内 `#[cfg(test)] mod tests`

- [ ] **步骤 1：编写失败的测试**

在 `tests` 模块追加（沿用既有 `TEST_LOCK`）：

```rust
#[test]
fn statusline_hook() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    assert_eq!(statusline_text(&StatuslineCtx { path: None, mode: "normal".into(), cursor: (0, 0) }), None);

    load_script(
        r#"
        helix.set_statusline((ctx) => ctx.mode + ":" + ctx.cursor.row);
        "#,
    )
    .unwrap();
    let ctx = StatuslineCtx { path: Some("/tmp/a.rs".into()), mode: "insert".into(), cursor: (3, 7) };
    assert_eq!(statusline_text(&ctx), Some("insert:3".to_string()));

    // 返回 null → None
    load_script(r#"helix.set_statusline(() => null);"#).unwrap();
    assert_eq!(statusline_text(&ctx), None);
    // 抛错 → None
    load_script(r#"helix.set_statusline(() => { throw new Error("boom"); });"#).unwrap();
    assert_eq!(statusline_text(&ctx), None);
    // set_statusline(null) 清除
    load_script(r#"helix.set_statusline(null);"#).unwrap();
    assert_eq!(statusline_text(&ctx), None);
    // 非法参数 → JS 报错
    assert!(load_script(r#"helix.set_statusline(42);"#).is_err());
}
```

- [ ] **步骤 2：运行测试确认失败**

运行：`cargo test -p helix-js`
预期：编译错误（`statusline_text`、`StatuslineCtx` 未定义）。

- [ ] **步骤 3：编写最少实现代码**

在 `helix-js/src/lib.rs`：

```rust
/// 状态栏钩子收到的轻量上下文（不含 doc.text，避免每帧克隆全文）
pub struct StatuslineCtx {
    pub path: Option<String>,
    pub mode: String,
    pub cursor: (usize, usize),
}
```

thread_local 追加：

```rust
    static STATUSLINE_HOOK: RefCell<Option<JsValue>> = RefCell::new(None);
```

`init()` 的 helix 对象追加：

```rust
    .function(NativeFunction::from_fn_ptr(js_set_statusline), JsString::from("set_statusline"), 1)
```

原生函数与公共函数：

```rust
fn js_set_statusline(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let arg = args.first().cloned().unwrap_or(JsValue::null());
    if arg.is_null_or_undefined() {
        STATUSLINE_HOOK.with(|h| *h.borrow_mut() = None);
    } else if arg.as_callable().is_some() {
        STATUSLINE_HOOK.with(|h| *h.borrow_mut() = Some(arg));
    } else {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.set_statusline: expected a function or null",
        ))));
    }
    Ok(JsValue::undefined())
}

/// 调状态栏钩子；无钩子 / 返回 null / 非字符串 / 抛错 → None
pub fn statusline_text(ctx: &StatuslineCtx) -> Option<String> {
    init();
    let hook = STATUSLINE_HOOK.with(|h| h.borrow().clone())?;
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized");
        let func = hook.as_callable().and_then(JsFunction::from_object)?;
        let ctx_obj = ObjectInitializer::new(engine)
            .property(
                JsString::from("path"),
                match &ctx.path {
                    Some(p) => JsValue::from(JsString::from(p.clone())),
                    None => JsValue::null(),
                },
                Attribute::all(),
            )
            .property(JsString::from("mode"), JsValue::from(JsString::from(ctx.mode.clone())), Attribute::all())
            .property(
                JsString::from("cursor"),
                JsValue::from(
                    ObjectInitializer::new(engine)
                        .property(JsString::from("row"), JsValue::from(ctx.cursor.0 as f64), Attribute::all())
                        .property(JsString::from("col"), JsValue::from(ctx.cursor.1 as f64), Attribute::all())
                        .build(),
                ),
                Attribute::all(),
            )
            .build();
        let undefined = JsValue::undefined();
        let value: JsValue = func.call(&undefined, &[JsValue::from(ctx_obj)], engine).ok()?;
        value.try_js_into::<String>(engine).ok()
    })
}
```

> 注意：`is_null_or_undefined` 是 boa JsValue 方法（若不存在用 `value.is_null() || value.is_undefined()`）。调用形式 `func.call(&undefined, &[args], engine)` 与既有代码一致。`?` 在闭包内对 Option 的用法（`STATUSLINE_HOOK.with(...)?` 不行——with 返回的不是 Option？把 hook 克隆判断改成先取 Option 再 unwrap/提前返回，以编译为准）。

- [ ] **步骤 4：运行测试确认通过**

运行：`cargo test -p helix-js`
预期：既有 9 个 + 新增 1 个测试全部 PASS；clippy 0 警告。

- [ ] **步骤 5：Commit**

```bash
git add helix-js
git commit -m "feat(js): add helix.set_statusline hook"
```

---

### 任务 2：statusline 渲染接线 + 集成测试

**文件：**
- 修改：`helix-term/src/ui/statusline.rs`
- 创建：`helix-term/tests/test/plugin_statusline.rs`
- 注意：`integration.rs` 的 mod 声明由控制器合并时统一加——你不要改 integration.rs

- [ ] **步骤 1：编写失败的集成测试**

创建 `helix-term/tests/test/plugin_statusline.rs`（临时在本地 integration.rs 加 mod 跑通后移除，不要提交该文件改动）：

```rust
use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_statusline_renders() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("st.txt");
    std::fs::write(&file, "data\n")?;
    let plugin_path = dir.path().join("statusline.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.set_statusline((ctx) => "PLUGIN|" + ctx.mode + "|" + ctx.cursor.row);
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some("i"), // 进入 insert 模式，触发重渲染
                Some(&|app| {
                    // 构造 RenderContext 渲染状态栏到独立 surface（仿 bufferline 集成测试）
                    let (view, doc) = current_ref!(app.editor);
                    let area = helix_view::graphics::Rect::new(0, 0, 200, 1);
                    let mut buf = tui::buffer::Buffer::empty(area);
                    let mut rc = helix_term::ui::statusline::RenderContext::new(
                        &app.editor,
                        doc,
                        view,
                        true,
                        &helix_view::progress::ProgressSpinners::default(),
                    );
                    helix_term::ui::statusline::render(&mut rc, area, &mut buf);
                    let rendered: String = buf.content.iter().map(|c| c.symbol.as_str()).collect();
                    assert!(
                        rendered.contains("PLUGIN|insert|0"),
                        "statusline missing plugin text: {rendered:?}"
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

> 注意：`RenderContext::new` 的 `ProgressSpinners` 类型与构造路径以编译为准（`helix_view::progress::ProgressSpinners` 或 `helix_term::ui::statusline` 附近的导入）。`focused: true` 对应 active 样式。若 RenderContext 测试构造不可行，退化为：断言插件 hook 在 render 期间被调用（hook 里 echo 一个标记 → 断言状态栏/消息），并在报告中说明。

- [ ] **步骤 2：运行测试确认失败**

运行：`cargo test -p helix-term --features integration --test integration plugin_statusline`
预期：FAIL——渲染文本不含 "PLUGIN|"。

- [ ] **步骤 3：实现渲染接线**

`helix-term/src/ui/statusline.rs` `render` 函数右侧元素渲染之后、函数结尾前追加：

```rust
    // JS 插件状态栏钩子：渲染在右侧元素之后
    let plugin_text = helix_js::statusline_text(&helix_js::StatuslineCtx {
        path: context.doc.path().map(|p| p.to_string_lossy().into_owned()),
        mode: match context.editor.mode {
            helix_view::document::Mode::Normal => "normal".to_string(),
            helix_view::document::Mode::Insert => "insert".to_string(),
            helix_view::document::Mode::Select => "select".to_string(),
        },
        cursor: {
            let pos = context.doc.selection(context.view.id).primary().cursor(context.doc.text().slice(..));
            (context.doc.text().char_to_line(pos), pos - context.doc.text().line_to_char(context.doc.text().char_to_line(pos)))
        },
    });
    if let Some(text) = plugin_text {
        // 渲染到右侧：从右往左定位，或直接追加在右侧元素之后
        // 具体实现参照右侧元素的 set_string 模式（以 statusline.rs 右侧渲染代码为准）
    }
```

> 右侧渲染的落点：找到右侧元素的渲染循环（`config.statusline.right`），在其之后用同样方式 `surface.set_string(...)` 或追加到 parts。样式用 `base_style`。若右侧有对齐逻辑（右对齐到 viewport 右侧），插件文本右对齐追加在其后。以代码实际结构为准，保证"渲染在右侧元素之后、不破坏既有对齐"。

- [ ] **步骤 4：跑测试确认通过**

运行：`cargo test -p helix-term --features integration --test integration plugin_statusline`
预期：PASS。

回归：`cargo test -p helix-term --features integration --test integration command_line` PASS；`cargo test -p helix-js` PASS；`cargo check -p helix-term` 无警告。

- [ ] **步骤 5：Commit**

```bash
git add helix-term/src/ui/statusline.rs helix-term/tests/test/plugin_statusline.rs
git commit -m "feat(term): render plugin statusline hook in the status bar"
```

---

## 自检记录

- 规格覆盖：set_statusline 单例/清除/校验、轻量 ctx、右侧渲染、测试。
- 已知风险：RenderContext/ProgressSpinners 构造路径（以编译为准，退化路径已注明）；右侧渲染对齐细节（以 statusline.rs 实际结构为准）；`integration.rs` mod 声明留给控制器。
