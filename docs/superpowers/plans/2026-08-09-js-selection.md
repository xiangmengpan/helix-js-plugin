# 选区/光标 API 实现计划（v5）

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development 或 superpowers:executing-plans 实现此计划。步骤使用复选框（`- [ ]`）语法跟踪进度。
> **本次执行策略：** 自主执行——TDD 后测试通过即提交；控制者自动合并；失败自动重试。

**目标：** 插件读主选区（`ctx.selection`）、写光标/选区（`helix.set_cursor` / `helix.set_selection`），快照坐标、光标请求先于编辑应用。

---

### 任务 1：helix-js 选区/光标（单任务）

**文件：**
- 修改：`helix-js/src/lib.rs`
- 修改：`helix-term/src/commands/typed.rs`（ctx 构造补 selection、apply_cursor_requests、drain 接线）
- 创建：`helix-term/tests/test/plugin_selection.rs`（集成测试；integration.rs 的 mod 声明控制器合并时统一加——不要改 integration.rs，验证时临时加、跑通后移除）

- [ ] **步骤 1：编写失败的 helix-js 单测**

在 `tests` 模块追加（沿用 `TEST_LOCK`）：

```rust
#[test]
fn selection_and_cursor() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.register_command("selcmd", (ctx) => {
            helix.set_cursor(2, 3);
            helix.set_selection(0, 1, 0, 5);
            helix.echo("sel:" + ctx.selection.anchor.row + "," + ctx.selection.anchor.col + "-" + ctx.selection.head.row + "," + ctx.selection.head.col);
        });
        "#,
    )
    .unwrap();
    let ctx = CommandContext {
        path: None,
        text: "abc\ndef\nghi".into(),
        cursor: (0, 0),
        selection: ((0, 1), (0, 5)),
    };
    assert!(run_command("selcmd", &ctx).unwrap());
    let reqs = take_cursor_requests();
    assert_eq!(
        reqs,
        vec![
            CursorRequest::SetCursor { row: 2, col: 3 },
            CursorRequest::SetSelection { anchor: (0, 1), head: (0, 5) },
        ]
    );
    assert_eq!(take_messages(), vec!["sel:0,1-0,5"]);

    // 类型错误 → 命令失败，队列清空
    load_script(r#"helix.register_command("badsel", (ctx) => { helix.set_cursor("x", 0); });"#).unwrap();
    assert!(run_command("badsel", &ctx).unwrap() == false || take_cursor_requests().is_empty());
    // 注：badsel 的 set_cursor 报错会使 run_command 返回 Err——改为明确断言：
}
```

> 注意：`badsel` 用例——`set_cursor("x", 0)` 在 JS 内抛错（try_js_into 失败），run_command 会返回 Err。改为：
> ```rust
> assert!(run_command("badsel", &ctx).is_err());
> assert!(take_cursor_requests().is_empty());
> ```
> 且**既有全部 CommandContext 构造点**（doc_edits_queue、doc_edit_validation、event 测试、popup 测试等）都要补 `selection: ((0, 0), (0, 0))` 字段——逐个更新（编译错误会指出来，机械补充即可）。

- [ ] **步骤 2：运行确认失败**

`cargo test -p helix-js` → 编译错误（`selection` 字段、`CursorRequest`、`take_cursor_requests` 未定义 + 既有构造点缺字段）。

- [ ] **步骤 3：实现 helix-js**

`CommandContext` 增加字段：

```rust
pub struct CommandContext {
    pub path: Option<String>,
    pub text: String,
    pub cursor: (usize, usize),
    /// 主选区（anchor, head）行列对
    pub selection: ((usize, usize), (usize, usize)),
}
```

新类型（`Edit` 附近）：

```rust
/// 光标/选区请求（命令返回后由 helix-term 应用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorRequest {
    SetCursor { row: usize, col: usize },
    SetSelection { anchor: (usize, usize), head: (usize, usize) },
}
```

thread_local 追加（既有块内）：

```rust
    static CURSOR_REQUESTS: RefCell<Vec<CursorRequest>> = const { RefCell::new(Vec::new()) };
```

`run_command` 开头清空（与 CURRENT_EDITS 并列）：

```rust
    CURSOR_REQUESTS.with(|c| c.borrow_mut().clear());
```

`init()` 的 helix 对象追加：

```rust
    .function(NativeFunction::from_fn_ptr(js_set_cursor), JsString::from("set_cursor"), 2)
    .function(NativeFunction::from_fn_ptr(js_set_selection), JsString::from("set_selection"), 4)
```

原生函数：

```rust
fn js_set_cursor(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let row: usize = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let col: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    CURSOR_REQUESTS.with(|c| c.borrow_mut().push(CursorRequest::SetCursor { row, col }));
    Ok(JsValue::undefined())
}

fn js_set_selection(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let ar: usize = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let ac: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let hr: usize = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let hc: usize = args.get(3).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    CURSOR_REQUESTS.with(|c| c.borrow_mut().push(CursorRequest::SetSelection {
        anchor: (ar, ac),
        head: (hr, hc),
    }));
    Ok(JsValue::undefined())
}
```

公共函数：

```rust
/// 取走并清空光标/选区请求队列（helix-term 消费）
pub fn take_cursor_requests() -> Vec<CursorRequest> {
    init();
    std::mem::take(&mut *CURSOR_REQUESTS.with(|c| c.borrow_mut()))
}
```

`ctx_to_js` 外层对象追加 selection（cursor 对象之后）：

```rust
        .property(
            JsString::from("selection"),
            JsValue::from(
                ObjectInitializer::new(engine)
                    .property(
                        JsString::from("anchor"),
                        JsValue::from(
                            ObjectInitializer::new(engine)
                                .property(JsString::from("row"), JsValue::from(ctx.selection.0 .0 as f64), Attribute::all())
                                .property(JsString::from("col"), JsValue::from(ctx.selection.0 .1 as f64), Attribute::all())
                                .build(),
                        ),
                        Attribute::all(),
                    )
                    .property(
                        JsString::from("head"),
                        JsValue::from(
                            ObjectInitializer::new(engine)
                                .property(JsString::from("row"), JsValue::from(ctx.selection.1 .0 as f64), Attribute::all())
                                .property(JsString::from("col"), JsValue::from(ctx.selection.1 .1 as f64), Attribute::all())
                                .build(),
                        ),
                        Attribute::all(),
                    )
                    .build(),
            ),
            Attribute::all(),
        )
```

> 注意：`ctx.selection.0 .0` 语法（元组字段解引用）以编译为准；嵌套 ObjectInitializer 的借用冲突（`&mut engine` 嵌套）按既有 doc_to_js 的先建内层再建外层的模式处理。

- [ ] **步骤 4：helix-js 测试通过**

`cargo test -p helix-js` → 15 个 PASS；clippy 0 警告。

- [ ] **步骤 5：helix-term 接线 + 集成测试**

a) 提取共享坐标辅助（apply_plugin_edits 的 to_char 提出来）：

```rust
/// 0-based 行列 → char 索引（越界 clamp）
fn pos_to_char(text: &Rope, row: usize, col: usize) -> usize {
    let line = row.min(text.len_lines().saturating_sub(1));
    let line_start = text.line_to_char(line);
    (line_start + col).min(text.len_chars())
}
```

apply_plugin_edits 改用 pos_to_char；新增：

```rust
/// 应用插件光标/选区请求（在编辑事务之前——事务的 selection 重映射会把
/// 快照坐标的光标正确推进）
fn apply_cursor_requests(editor: &mut Editor, reqs: &[helix_js::CursorRequest]) -> anyhow::Result<()> {
    use helix_core::Selection;
    let (view, doc) = current!(editor);
    let text = doc.text();
    for req in reqs {
        let selection = match req {
            helix_js::CursorRequest::SetCursor { row, col } => {
                Selection::point(pos_to_char(text, *row, *col))
            }
            helix_js::CursorRequest::SetSelection { anchor, head } => {
                let a = pos_to_char(text, anchor.0, anchor.1);
                let h = pos_to_char(text, head.0, head.1);
                Selection::single(a, h)
            }
        };
        doc.set_selection(view.id, selection);
    }
    Ok(())
}
```

> 注意：`Selection::single` / `Selection::point` 签名以 helix-core 为准（可能带 Assoc 参数——`Selection::single(anchor, head)` 若需要 assoc 用默认）。`Rope` 类型导入以 apply_plugin_edits 既有代码为准。

b) 两个 ctx 构造点补 selection（命令分发钩子 + emit_plugin_event）：

```rust
    let primary = doc.selection(view.id).primary();
    let anchor_pos = primary.anchor;  // char 索引
    let head_pos = primary.head;
    let anchor_line = text.char_to_line(anchor_pos);
    let head_line = text.char_to_line(head_pos);
    // ... CommandContext { ..., selection: ((anchor_line, anchor_pos - text.line_to_char(anchor_line)), (head_line, head_pos - text.line_to_char(head_line))) }
```

> 注意：`range.anchor`/`range.head` 是 char 索引（Range 字段）；与既有 cursor 的 pos 计算同源。构造代码与 cursor 的行列计算并列。

c) drain 接线：命令分发 `Ok(true)` 分支，在 `take_edits` 之前：

```rust
                    let cursor_reqs = helix_js::take_cursor_requests();
                    if !cursor_reqs.is_empty() {
                        if let Err(err) = apply_cursor_requests(cx.editor, &cursor_reqs) {
                            cx.editor.set_error(format!("plugin cursor failed: {err}"));
                        }
                    }
```

emit_plugin_event 同样在 take_edits/apply_plugin_edits 之前加（apply_cursor_requests(editor, &reqs)）。

d) 集成测试 `tests/test/plugin_selection.rs`（临时 mod 验证，跑通后移除 integration.rs 改动）：

```rust
use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_selection_upper() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("sel.txt");
    std::fs::write(&file, "hello world\n")?;
    let plugin_path = dir.path().join("sel.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("upper", (ctx) => {
            const a = ctx.selection.anchor, h = ctx.selection.head;
            if (a.row !== h.row) { helix.echo("single-line only"); return; }
            const line = ctx.doc.text.split("\n")[a.row];
            const lo = Math.min(a.col, h.col), hi = Math.max(a.col, h.col);
            const upper = line.slice(lo, hi).toUpperCase();
            ctx.doc.replace(a.row, lo, a.row, hi, upper);
        });
        helix.register_command("jumpend", (ctx) => { helix.set_cursor(0, 11); });
        "#,
    )?;

    // 初始选中 "hello"（#[hello|]# 标记）
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).with_input_text("#[hello|]# world\n").build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some(":upper<ret>"),
                Some(&|app| {
                    let (view, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "HELLO world\n");
                    let sel = doc.selection(view.id).primary();
                    assert_eq!(sel.anchor, 0, "anchor after upper");
                    assert_eq!(sel.head, 5, "head after upper");
                }),
            ),
            (
                Some(":jumpend<ret>"),
                Some(&|app| {
                    let (view, doc) = current_ref!(app.editor);
                    let pos = doc.selection(view.id).primary().cursor(doc.text().slice(..));
                    assert_eq!(pos, 11, "cursor after jumpend");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
```

> 注意：`with_input_text` 与 `with_file` 可共存（input 覆盖文件文本）。选区标记语法 `#[hello|]#`（anchor=head=选区两端）参照 tests/test/ 既有用法（`test::print` 格式）。`:upper` 后选区 anchor/head 应为 0..5（replace 同长度，重映射不变）。若 `Selection::single` 的 Assoc 默认使 anchor/head 顺序与预期不符，以实测为准调整断言。

- [ ] **步骤 6：跑测试**

`cargo test -p helix-term --features integration --test integration plugin_selection`（临时 mod）→ PASS；`plugin` 全组回归 PASS；`cargo check -p helix-term` 无警告；clippy 无新增警告。

- [ ] **步骤 7：Commit**

```bash
git add helix-js helix-term
git commit -m "feat(js): selection and cursor API (ctx.selection, set_cursor, set_selection)"
```

---

## 自检记录

- 规格覆盖：ctx.selection 读、set_cursor/set_selection 写、快照坐标 + 顺序语义、只主选区、测试。
- 已知风险：CommandContext 新字段导致全部既有构造点编译错误（机械补充）；Selection::point/single 的 Assoc 参数签名（以编译为准）；嵌套 ObjectInitializer 借用（先内后外）；with_input_text 选区标记语法（参照既有测试）。
