# 跨 buffer 访问(by_path)实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** 插件可用 `helix.by_path(path)` 读/改其它已打开 buffer——返回与 `ctx.doc` 同构的 doc 对象,编辑按目标 buffer 路由、每 buffer 一次撤销。

**架构:** helix-term 命令/事件入口把全部已打开 buffer 的 `{path, text}` 快照放进 `CommandContext.docs`;helix-js 把它存进内部全局,`js_by_path` 同步查快照返回 doc 对象;doc 对象的编辑方法把目标 path 记进 `Edit.doc`;`apply_plugin_edits` 按 `Edit.doc` 分组,每组对目标 doc 一个 Transaction。

**技术栈:** Rust(helix-js boa 运行时 + helix-term)、boa_engine、helix-stdx path 工具、tokio 集成测试。

**规格:** `docs/superpowers/specs/2026-08-28-js-cross-buffer-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-js/src/types.rs` | `Edit.doc: Option<String>`、`CommandContext.docs: Vec<DocSnapshot>`、新 `pub struct DocSnapshot { path, text }` |
| `helix-js/src/state.rs` | 新 `DOC_SNAPSHOTS: RefCell<Vec<DocSnapshot>>` + `set_doc_snapshots`/`with_doc_snapshots` |
| `helix-js/src/commands.rs` | `js_by_path`、`doc_to_js` 重构出 `build_doc_object(path, text, cursor, target, engine)`、`js_doc_insert/replace/delete` 从 `this._target` 读目标、`run_command`/`emit_event_impl` 入口存 docs |
| `helix-js/src/lib.rs` | 注册 `by_path`(1 参);~31 处测试 `CommandContext {` 构造器 + 5 处 `Edit {` 测试构造器补字段(sed);新单测 |
| `helix-js/Cargo.toml` | 加 `helix-stdx = { path = "../helix-stdx" }` |
| `helix-term/src/commands/typed.rs` | `run_plugin_command`(~4348)与 `emit_plugin_event_impl`(~4443)填 `docs`;`apply_plugin_edits`(~4530)按 doc 分组 |
| `helix-term/src/ui/plugin_panel.rs:106`、`plugin_popup.rs:155` | 渲染 ctx 显式 `docs: vec![]`(每帧不序列化全文) |
| `helix-term/src/commands/typed.rs:5341` | 测试 ctx 构造器补 `docs: vec![]` |
| `helix-term/tests/integration.rs` | 注册 `mod plugin_cross_buffer;` |
| `helix-term/tests/test/plugin_cross_buffer.rs`(新) | 跨 buffer 集成测试 |
| `docs/plugin-api.md` | `by_path` 文档一节 |

关键实现细节(执行时必读):

- **helix-js 无 ctx 参数**:native 函数(`js_by_path`)拿不到正在执行的 `CommandContext`,靠 state.rs 全局路由——`run_command`/`emit_event_impl` 入口把 `ctx.docs` 拷进 `DOC_SNAPSHOTS`(与 `with_engine`/`CURRENT_EDITS` 同款模式);生命周期 = 本次命令/事件,下次入口覆盖。
- **doc 对象的目标传递**:`js_doc_*` 是 `from_fn_ptr`(无闭包捕获),目标 path 存在对象属性 `_target` 上(native 方法被调用时 `this` = 接收者);`ctx.doc` 的 `_target = null` → `Edit.doc = None`(现状),`by_path` doc 的 `_target = path` → `Some(path)`。方法被剥离调用(`const f = d.insert; f(...)`)时 `this` 非对象 → 视为 None,安全。
- **路径匹配**:参数经 `helix_stdx::path::canonicalize`(展开 `~`、相对 join cwd、词法 normalize——与文档打开时同一函数,见 helix-view document.rs:1021/1347),与快照 path 精确比较。
- **坐标语义**:与现状一致——编辑坐标基于命令开始时的快照,应用时用目标 doc 当时文本。不新增竞态。
- **panel/popup 渲染回调里的 by_path**:读的是最近一次命令/事件入口的快照(过期但不崩)。渲染回调不走 run_command/emit_event,ctx.docs 为空的声明只服务编译器;当前无插件在渲染里调 by_path,需要精确语义时再按渲染 ctx 传 docs(`ponytail:` 边界)。
- **sed 机械化字段补齐**(编译错误会兜底枚举遗漏):
  - `sed -i 's/CommandContext { /CommandContext { docs: vec![], /g' helix-js/src/*.rs helix-term/src/commands/typed.rs helix-term/src/ui/plugin_panel.rs helix-term/src/ui/plugin_popup.rs`
  - `sed -i 's/Edit { start:/Edit { doc: None, start:/g' helix-js/src/*.rs`
  - 两条 sed 后 `cargo build` 按编译错误手工补漏。
- **测试命令**:`cargo test -p helix-js`、`cargo test -p helix-term --features integration --test integration plugin_cross_buffer`(integration 是 feature 门控)。

---

### 任务 1:快照与读取(API 侧)

**文件:** types.rs、Cargo.toml、state.rs、commands.rs、lib.rs、typed.rs(两入口)、plugin_panel.rs、plugin_popup.rs、plugin-api.md

- [ ] **步骤 1:字段落地 + 全仓构造器补齐**

1. `helix-js/src/types.rs`:
   - `Edit` 加字段,`CommandContext` 加字段,新增结构:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub doc: Option<String>,  // Some(path) = 目标其它 buffer;None = 当前 buffer(现状)
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub insert: String,
}

/// 命令/事件入口携带的其它已打开 buffer 快照(只读;路径已 canonicalize)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocSnapshot {
    pub path: String,
    pub text: String,
}
```

   `CommandContext` 末尾加 `pub docs: Vec<DocSnapshot>,`(注释:`其它已打开 buffer 快照;渲染入口(panel/popup)显式传空`)。
2. `helix-js/Cargo.toml` `[dependencies]` 加 `helix-stdx = { path = "../helix-stdx" }`。
3. 跑上面两条 sed,然后 `cargo build -p helix-js` 按编译错误手工补漏(遗漏点用 `cargo build -p helix-term` 再兜一轮)。
4. 预期:全仓编译通过,行为未变(现有测试不涉及新字段)。

- [ ] **步骤 2:写失败单测(by_path 未注册)**

`helix-js/src/lib.rs` 测试模块加(放在 `doc_edit_validation` 附近;`TEST_LOCK` 惯例同现有测试):

```rust
#[test]
fn by_path_reads_other_buffer() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.register_command("read-other", () => {
            const d = helix.by_path("/tmp/other.rs");
            if (d === null) throw new Error("null");
            if (d.path !== "/tmp/other.rs") throw new Error("path mismatch");
            if (d.text !== "other text") throw new Error("text mismatch");
        });
        "#,
    )
    .unwrap();
    let ctx = CommandContext {
        path: Some("/tmp/current.rs".into()),
        text: "current".into(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: vec![DocSnapshot { path: "/tmp/other.rs".into(), text: "other text".into() }],
    };
    run_command("read-other", &ctx).unwrap();
}
```

预期:FAIL(`helix.by_path is not a function` → run_command Err → unwrap panic)。

- [ ] **步骤 3:实现读取链路**

1. `helix-js/src/state.rs`(仿 `CURRENT_EDITS` 的 `RefCell` 模式):

```rust
pub(crate) static DOC_SNAPSHOTS: RefCell<Vec<DocSnapshot>> = const { RefCell::new(Vec::new()) };
pub(crate) fn set_doc_snapshots(docs: Vec<DocSnapshot>) { DOC_SNAPSHOTS.replace(docs); }
pub(crate) fn with_doc_snapshots<T>(f: impl FnOnce(&Vec<DocSnapshot>) -> T) -> T { f(&DOC_SNAPSHOTS.borrow()) }
```

2. `helix-js/src/commands.rs`:
   - `run_command`(~427)在 `with_txn_depth(|d| *d = 0)` 后加 `crate::state::set_doc_snapshots(ctx.docs.clone());`
   - `emit_event_impl`(~489 链)同样在复位 txn 深度后加 `crate::state::set_doc_snapshots(ctx.docs.clone());`(handler 抛错路径的复位只清队列/深度、函数即中止,无需重复设置快照)。
   - 新增:

```rust
pub(crate) fn js_by_path(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.by_path: path must be a string")))
    })?;
    let canonical = helix_stdx::path::canonicalize(&path).to_string_lossy().into_owned();
    let hit = crate::state::with_doc_snapshots(|s| s.iter().find(|d| d.path == canonical).cloned());
    let Some(hit) = hit else { return Ok(JsValue::null()) };
    build_doc_object(Some(&hit.path), &hit.text, (0, 0), Some(&hit.path), context)
}
```

3. `helix-js/src/lib.rs` 注册(builder 链,紧跟 `js_end_edit` 后):

```rust
.function(NativeFunction::from_fn_ptr(commands::js_by_path), JsString::from("by_path"), 1)
```

- [ ] **步骤 4:单测转绿**

运行:`cargo test -p helix-js by_path`
预期:PASS。若 `build_doc_object` 未定义(步骤 5 才重构),先给最小版——见步骤 5 的签名,直接按步骤 5 的实现写即可。

- [ ] **步骤 5:doc_to_js 重构出 build_doc_object + 编辑目标传递**

`helix-js/src/commands.rs`:

1. `doc_to_js`(~354)拆出通用构造器,`doc_to_js` 变成薄壳:

```rust
/// 构造 doc 对象 { path, text, cursor, _target } + insert/replace/delete。
/// _target = 编辑目标 path(by_path 的 doc);ctx.doc 为 null → 编辑推 Edit.doc = None。
fn build_doc_object(
    path: Option<&str>,
    text: &str,
    cursor: (usize, usize),
    target: Option<&str>,
    engine: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let cursor_obj = ObjectInitializer::new(engine)
        .property(JsString::from("row"), JsValue::from(cursor.0 as f64), Attribute::all())
        .property(JsString::from("col"), JsValue::from(cursor.1 as f64), Attribute::all())
        .build();
    Ok(JsValue::from(
        ObjectInitializer::new(engine)
            .property(
                JsString::from("path"),
                match path { Some(p) => JsValue::from(JsString::from(p)), None => JsValue::null() },
                Attribute::all(),
            )
            .property(JsString::from("text"), JsValue::from(JsString::from(text)), Attribute::all())
            .property(JsString::from("cursor"), cursor_obj, Attribute::all())
            // 内部目标:by_path 的 doc 用它路由编辑;ctx.doc 为 null。插件可见但无害。
            .property(
                JsString::from("_target"),
                match target { Some(t) => JsValue::from(JsString::from(t)), None => JsValue::null() },
                Attribute::all(),
            )
            .function(NativeFunction::from_fn_ptr(js_doc_insert), JsString::from("insert"), 3)
            .function(NativeFunction::from_fn_ptr(js_doc_replace), JsString::from("replace"), 5)
            .function(NativeFunction::from_fn_ptr(js_doc_delete), JsString::from("delete"), 4)
            .build(),
    ))
}

pub(crate) fn doc_to_js(ctx: &CommandContext, engine: &mut Context) -> boa_engine::JsResult<JsValue> {
    build_doc_object(ctx.path.as_deref(), &ctx.text, ctx.cursor, None, engine)
}
```

2. 三个编辑函数从 `this._target` 读目标(替换 `_this` 忽略逻辑),`js_doc_insert` 示例:

```rust
/// 从 doc 对象读编辑目标:this._target 为字符串 → Some(path);null/非对象 → None(当前 buffer)
fn edit_target(_this: &JsValue, context: &mut Context) -> boa_engine::JsResult<Option<String>> {
    let Some(obj) = _this.as_object() else { return Ok(None) };
    let v = obj.get(JsString::from("_target"), context)?;
    if v.is_null_or_undefined() {
        Ok(None)
    } else {
        v.try_js_into(context).map(Some)
    }
}
```

`js_doc_insert` 里 `crate::state::with_edits(...)` 的 push 改为:

```rust
let doc = edit_target(_this, context)?;
crate::state::with_edits(|c| c.push(Edit { doc, start: (row, col), end: (row, col), insert }));
```

`js_doc_replace`/`js_doc_delete` 同样加 `let doc = edit_target(_this, context)?;` 并填 `doc` 字段。

3. `cargo build -p helix-js` 编译通过;现有 `doc_edit_validation` 等测试须仍绿(`cargo test -p helix-js`)。

- [ ] **步骤 6:单测编辑路由**

`helix-js/src/lib.rs` 测试模块加:

```rust
#[test]
fn by_path_edits_target_other_buffer() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.register_command("edit-other", () => {
            const d = helix.by_path("/tmp/other.rs");
            if (d === null) throw new Error("null");
            d.insert(0, 0, "X");
        });
        helix.register_command("edit-current", (ctx) => {
            ctx.doc.insert(0, 0, "Y");
        });
        "#,
    )
    .unwrap();
    let ctx = CommandContext {
        path: Some("/tmp/current.rs".into()),
        text: "current".into(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: vec![DocSnapshot { path: "/tmp/other.rs".into(), text: "other text".into() }],
    };
    run_command("edit-other", &ctx).unwrap();
    let edits = take_edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].doc.as_deref(), Some("/tmp/other.rs"));
    run_command("edit-current", &ctx).unwrap();
    let edits = take_edits();
    assert_eq!(edits[0].doc, None, "ctx.doc 编辑仍指向当前 buffer");
}
```

预期:PASS。

- [ ] **步骤 7:term 入口填 docs + 渲染入口显式空**

`helix-term/src/commands/typed.rs`:

1. `run_plugin_command`(~4335,`current_ref!` 之前)加快照收集:

```rust
let docs = cx.editor.documents.values()
    .filter_map(|d| d.path().map(|p| helix_js::DocSnapshot {
        path: p.to_string_lossy().into_owned(),
        text: d.text().to_string(),
    }))
    .collect();
```

   `CommandContext` 构造加 `docs,`。借用:docs 是 owned Vec,收集后 `current_ref!` 再借用,无冲突。
2. `emit_plugin_event_impl`(~4443)同样收集并填 `docs`(参数已是 `editor: &mut Editor`,遍历 `editor.documents.values()`)。
3. `plugin_panel.rs:106`、`plugin_popup.rs:155`、`typed.rs:5341` 的构造器在 sed 后已是 `docs: vec![]`,确认即可(渲染 ctx 每帧,不序列化全文——设计决策)。
4. 运行:`cargo build -p helix-term`;预期通过。

- [ ] **步骤 8:plugin-api.md 文档**

`docs/plugin-api.md` 加一节(仿 `begin_edit` 小节格式,位置在 doc 编辑 API 附近):

```markdown
### `helix.by_path(path)`(跨 buffer 访问)

按路径查**已打开**的 buffer,返回与 `ctx.doc` 同构的只读快照对象(路径已规范化,相对路径基于启动时 cwd):

```js
const other = helix.by_path("src/main.rs");
// → { path: "/abs/src/main.rs", text: "...", cursor: { row: 0, col: 0 } }
//   + insert(row, col, str) / replace(sr, sc, er, ec, str) / delete(sr, sc, er, ec)
// 未打开 → null(不会自动打开文件)
```

- 编辑方法与 `ctx.doc` 一致,但作用于目标 buffer;一个命令改多个 buffer 时每个 buffer 一次撤销
- `cursor` 恒为 {row: 0, col: 0}(后台 buffer 无视图光标)
- 快照是命令开始时的文本;坐标基于该快照,跨 await 的过期坐标风险与 `ctx.doc` 相同
- scratch(无路径)buffer 不可查
```

- [ ] **步骤 9:Commit**

```bash
git add helix-js helix-term docs/plugin-api.md
git commit -m "feat(js,term): by_path 跨 buffer 读取(CommandContext 携带 docs 快照,doc 对象同构 ctx.doc)"
```

---

### 任务 2:编辑路由与集成测试(term 侧)

**文件:** typed.rs(apply_plugin_edits)、integration.rs、plugin_cross_buffer.rs(新)

- [ ] **步骤 1:写失败集成测试**

`helix-term/tests/integration.rs` 的 mod 列表加 `mod plugin_cross_buffer;`(放在 `mod plugin_docchange;` 后)。

新建 `helix-term/tests/test/plugin_cross_buffer.rs`(仿 plugin_doc.rs 结构,`use super::*;`):

```rust
use super::*;

// by_path 读另一 buffer 文本(:open 打开第二个文件后,命令里 by_path 读取)
#[tokio::test(flavor = "multi_thread")]
async fn plugin_by_path_reads_other_buffer() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file1 = dir.path().join("a.txt");
    std::fs::write(&file1, "one\n")?;
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file2, "two\n")?;
    let plugin_path = dir.path().join("read.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"helix.register_command("show-other", () => {{
                const d = helix.by_path("{}");
                helix.echo(d === null ? "null" : d.text.trim());
            }});"#,
            file2.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file1, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(&format!(":open {}<ret>", file2.display())), None),
            (
                Some(":show-other<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "two");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 未打开路径 → null,命令不崩
#[tokio::test(flavor = "multi_thread")]
async fn plugin_by_path_missing_returns_null() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file1 = dir.path().join("a.txt");
    std::fs::write(&file1, "one\n")?;
    let plugin_path = dir.path().join("null.js");
    std::fs::write(
        &plugin_path,
        r#"helix.register_command("show-null", () => {
            helix.echo(helix.by_path("/nonexistent-helix-xyz.rs") === null ? "null" : "hit");
        });"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file1, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some(":show-null<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "null");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

先跑**现有** apply 逻辑(它忽略 `edit.doc`)运行这两个测试:读测试(PASS,读取链路任务 1 已通)、null 测试(PASS)。——本步骤的"失败"在下一个测试(编辑路由)才体现;若想严格 TDD,可把步骤 2 的编辑测试挪到这里先写,步骤 2 变纯实现。

- [ ] **步骤 2:apply_plugin_edits 按 doc 分组**

`helix-term/src/commands/typed.rs` 重写 `apply_plugin_edits`(~4530):

```rust
/// 把插件 Edit(0-based 行列,原始快照坐标)按目标 buffer 分组,每组一个 Transaction。
/// None = 当前 buffer(现状);Some(path) = 其它 buffer。每个被改 buffer 一次撤销。
pub(crate) fn apply_plugin_edits(
    editor: &mut Editor,
    edits: &[helix_js::Edit],
) -> anyhow::Result<()> {
    use helix_core::Change;

    // 按目标分组,保持组内顺序
    let mut groups: Vec<(Option<String>, Vec<&helix_js::Edit>)> = Vec::new();
    for edit in edits {
        if let Some(g) = groups.iter_mut().find(|(p, _)| p == &edit.doc) {
            g.1.push(edit);
        } else {
            groups.push((edit.doc.clone(), vec![edit]));
        }
    }

    let build_txn = |text: &Rope, group: &[&helix_js::Edit]| -> anyhow::Result<Transaction> {
        let to_char = |(row, col): (usize, usize)| pos_to_char(text, row, col);
        let mut changes: Vec<Change> = group
            .iter()
            // 用户把 (start,end) 传反时 swap 规范化,避免 helix 的 debug_assert 崩溃
            .map(|edit| {
                let (s, en) = (to_char(edit.start), to_char(edit.end));
                let (from, to) = if s <= en { (s, en) } else { (en, s) };
                (from, to, Some(edit.insert.clone().into()))
            })
            .collect();
        changes.sort_by_key(|c| c.0);
        for w in changes.windows(2) {
            if w[0].1 > w[1].0 {
                bail!("overlapping edits");
            }
        }
        Ok(Transaction::change(text, changes.into_iter()))
    };

    // 先取当前 view id(复制,释放借用),供后台 doc 应用事务
    let current_view_id = current!(editor).0;

    for (target, group) in &groups {
        match target {
            None => {
                let (view, doc) = current!(editor);
                let text = doc.text().clone();
                let txn = build_txn(&text, group)?;
                doc.apply(&txn, view.id);
            }
            Some(path) => {
                // 命令期间被关闭 → 报错(不崩),与"plugin edit failed"惯例一致
                let id = editor
                    .documents
                    .iter()
                    .find(|(_, d)| d.path().is_some_and(|p| p.to_string_lossy() == path.as_str()))
                    .map(|(id, _)| *id)
                    .ok_or_else(|| anyhow!("plugin edit: buffer '{path}' not open"))?;
                let doc = editor
                    .document_mut(id)
                    .ok_or_else(|| anyhow!("plugin edit: buffer '{path}' disappeared"))?;
                let text = doc.text().clone();
                let txn = build_txn(&text, group)?;
                doc.apply(&txn, current_view_id);
            }
        }
    }
    Ok(())
}
```

注意:`use helix_core::{Rope, Transaction};` 已在文件顶部(Transaction 已用;Rope 若未 import 则加)。`pos_to_char` 已有。

- [ ] **步骤 3:集成测试——编辑其它 buffer + 每 doc 一次撤销**

`plugin_cross_buffer.rs` 加:

```rust
// 改另一 buffer:内容生效,当前 buffer 不受影响;undo 一次回退该 buffer
#[tokio::test(flavor = "multi_thread")]
async fn plugin_edit_other_buffer_undo_per_doc() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file1 = dir.path().join("a.txt");
    std::fs::write(&file1, "one\n")?;
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file2, "two\n")?;
    let plugin_path = dir.path().join("edit.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"helix.register_command("edit-other", () => {{
                const d = helix.by_path("{}");
                if (d !== null) d.insert(0, 0, "X");
            }});"#,
            file2.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file1, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(&format!(":open {}<ret>", file2.display())), None),
            (Some(":edit-other<ret>"), None),
            (
                // 当前 doc = file2,已被插入 "X"
                Some(&format!(":open {}<ret>", file1.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "one\n", "file1 不被跨 buffer 编辑影响");
                }),
            ),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "Xtwo\n");
                }),
            ),
            // 一次 undo 回退 file2 的插入(每 doc 一事务)
            (Some("u"), None),
            (
                Some(&format!(":open {}<ret>", file1.display())),
                None,
            ),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "two\n", "undo 回退跨 buffer 编辑");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

- [ ] **步骤 4:集成测试——混合改当前 + 其它,撤销独立**

```rust
// 一个命令混合改当前与另一 buffer:各自独立撤销
#[tokio::test(flavor = "multi_thread")]
async fn plugin_mixed_edits_undo_independent() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file1 = dir.path().join("a.txt");
    std::fs::write(&file1, "one\n")?;
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file2, "two\n")?;
    let plugin_path = dir.path().join("mix.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"helix.register_command("mix", (ctx) => {{
                ctx.doc.insert(0, 0, "A");
                const d = helix.by_path("{}");
                if (d !== null) d.insert(0, 0, "B");
            }});"#,
            file2.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file1, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":mix<ret>"), None),
            (
                // 当前 doc = file1
                Some("u"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "one\n", "undo 回退当前 buffer 的 A");
                }),
            ),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "Btwo\n", "另一 buffer 的 B 未被连带撤销");
                }),
            ),
            (Some("u"), None),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "two\n", "第二处 undo 独立回退 B");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

- [ ] **步骤 5:集成测试——async + begin_edit 跨 buffer 一次事务**

```rust
// async 命令:begin_edit → by_path 改其它 buffer → await → 再改 → end_edit → 一次撤销
#[tokio::test(flavor = "multi_thread")]
async fn plugin_async_begin_edit_cross_buffer() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file1 = dir.path().join("a.txt");
    std::fs::write(&file1, "one\n")?;
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file2, "two\nthree\n")?;
    let plugin_path = dir.path().join("async-edit.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"
            helix.register_command("async-xedit", () => {{
                helix.begin_edit();
                const d = helix.by_path("{}");
                if (d !== null) d.insert(0, 0, "X");
                helix.run_async("echo 1").then(() => {{
                    const d2 = helix.by_path("{}");
                    if (d2 !== null) d2.insert(1, 0, "Z");
                    helix.end_edit();
                }});
            }});
            "#,
            file2.display(),
            file2.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file1, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":async-xedit<ret>"), None),
            (Some(&format!(":open {}<ret>", file2.display())), None),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "Xtwo\nZthree\n", "跨 await 两次编辑一次应用");
                }),
            ),
            // 一次 undo 回退两处(同一事务)
            (Some("u"), None),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "two\nthree\n");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

说明:`:async-xedit` 后接两次 `:open file2`——第一次切换过去(等 pump 消化 async 回调),第二次断言内容(同一 doc 重复 :open 幂等)。若断言竞态,把第二次 :open 换成带 pump 的空步或直接合并断言。

- [ ] **步骤 6:全量验证 + Commit**

```bash
cargo test -p helix-js
cargo test -p helix-term --features integration --test integration plugin_cross_buffer
cargo clippy --all-targets 2>&1 | tail -5   # 零警告
```

预期:helix-js 全绿(含旧测试);plugin_cross_buffer 5 个测试 PASS;clippy 零警告。

```bash
git add helix-term/tests
git commit -m "feat(term): apply_plugin_edits 按 Edit.doc 分组——跨 buffer 批量改每 doc 一次撤销 + 集成测试"
```

---

## 自检记录

**规格覆盖度:**
- 2.1 API(by_path 签名/返回/null/cursor/path 规范化)→ 任务 1 步骤 2-4、7、8 ✓
- 2.2 数据流(CommandContext.docs / 两入口填 / 渲染入口空 / 内部路由)→ 任务 1 步骤 1、3、7 ✓
- 2.3 编辑路由(Edit.doc / 分组 / 关闭报错)→ 任务 2 步骤 2 ✓
- 2.4 边界(null、快照语义、scratch 排除、begin_edit 不变)→ 任务 2 步骤 3-5 ✓
- 3.1 单测(类型错/未命中/命中/路由)→ 任务 1 步骤 2、4、6 ✓
- 3.2 集成测试 5 条 → 任务 2 步骤 1、3、4、5 ✓

**占位符扫描:** 无 TODO/待定;所有代码块为实际实现代码。✓

**类型一致性:** `DocSnapshot{path,text}`、`Edit.doc: Option<String>`、`CommandContext.docs: Vec<DocSnapshot>`、`build_doc_object(path,text,cursor,target,engine)` 在任务 1/2 间签名一致;`js_by_path` 注册名 `by_path` 与文档/测试一致。✓
