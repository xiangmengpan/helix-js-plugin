# 装饰/标记 API(set_virtual_text / set_highlight)实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** 插件可用 `helix.set_virtual_text(path, row, col, text, style)` 加行内文本、`helix.set_highlight(path, sr, sc, er, ec, style)` 加区域高亮;命令/事件期间累积,应用时按 doc 整体替换,推空集/只传 path = 清除。

**架构:** helix-js 新装饰请求队列(与编辑队列同构:同 take 点、同 txn 门控、同复位);helix-term `apply_plugin_decorations` 按 doc 应用(坐标换算成 char 索引存进 `Document.plugin_decorations`);渲染在 `view.text_annotations`(virtual text,按 style 分组 add_inline_annotations)与 `ui/editor.rs` overlays(区域高亮,按 style 分组 Homogeneous)注入。

**技术栈:** Rust(helix-js boa 运行时 + helix-term + helix-view)、boa_engine、helix-stdx path 工具、tokio 集成测试。

**规格:** `docs/superpowers/specs/2026-08-28-js-decorations-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-js/src/types.rs` | 新 `DecorationKind` / `DecorationRequest` |
| `helix-js/src/state.rs` | 新 `DECORATION_REQUESTS: RefCell<Vec<DecorationRequest>>` + 访问器 |
| `helix-js/src/commands.rs` | `js_set_virtual_text` / `js_set_highlight`;`take_decorations`;复位点清空;`load_script_named` 挂 reload-clear |
| `helix-js/src/lib.rs` | 注册两函数(5/7 参);新单测 |
| `helix-view/src/document.rs` | `PluginDecorations` / `PluginInlineAnnotation` / `PluginHighlight` 类型 + Document 字段 |
| `helix-view/src/view.rs` | `text_annotations`(~458)注入插件 virtual text |
| `helix-term/src/ui/editor.rs` | overlays(~160)注入插件区域高亮 |
| `helix-term/src/commands/typed.rs` | `apply_plugin_decorations` + 命令/事件入口 take+apply |
| `helix-term/src/application.rs` | 泵循环(~412)take+apply |
| `helix-term/src/ui/plugin_panel.rs`、`plugin_popup.rs` | onKey 路径 take+apply |
| `helix-term/tests/integration.rs` | 注册 `mod plugin_decorations;` |
| `helix-term/tests/test/plugin_decorations.rs`(新) | 集成测试 |
| `docs/plugin-api.md` | 两函数文档一节 |

关键实现细节(执行时必读):

- **与编辑队列同构**:`take_decorations` 镜像 `take_edits`——txn 深度>0 返回空(积压),否则 drain。复位:在每个 `with_edits(|c| c.clear())` 清空点(命令入口/事件入口/错误路径)旁加 `with_decoration_requests(|c| c.clear())`(grep `with_edits(|c| c.clear())` 定位全部)。
- **take/apply 5 个调用点**(与 `take_edits`/`apply_plugin_edits` 同点):typed.rs run_plugin_command(~4383)、emit_plugin_event_impl(~4468)、application.rs 泵循环(~412)、plugin_panel.rs(~127)、plugin_popup.rs(~176)。
- **Clear 语义**:`set_virtual_text(path)`(text 省略/undefined)→ `DecorationKind::Clear`(清该 doc 的 virtual text + 高亮);`load_script_named` 入口 push `{ doc: None, kind: Clear }`(脚本重载不继承旧装饰;term 侧 doc: None = 清空所有 doc)。
- **路径 canonicalize** 在 push 时(与 `js_by_path`/`Edit.doc` 同款);未打开 doc 应用时静默忽略。
- **坐标**:应用时按该 doc 当时文本 `pos_to_char`(typed.rs 已有)换算 char 索引存储;不随事务重映射(插件 doc-change 重推)。
- **渲染样式**:`style: Option<String>`(主题 scope);解析失败或 None → 该组跳过不渲染(即"默认无样式")。
- **OverlayHighlights 组内不重叠**:同 style 组排序 + 合并重叠后构造 `Homogeneous`。
- **测试命令**:`cargo test -p helix-js`;`cargo test -p helix-term --features integration --test integration plugin_decorations`;clippy `cargo clippy --all-targets` 零警告。

---

### 任务 1:队列与 API(helix-js 全链路 + 单测)

**文件:** types.rs、state.rs、commands.rs、lib.rs

- [ ] **步骤 1:写失败单测(push 与取走)**

`helix-js/src/lib.rs` 测试模块加(沿用 `TEST_LOCK` + `init()` 惯例):

```rust
#[test]
fn decorations_queue_and_take() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.register_command("dec1", () => {
            helix.set_virtual_text("/tmp/a.rs", 0, 0, "hi", "ui.help");
            helix.set_highlight("/tmp/a.rs", 0, 0, 1, 2, "ui.selection");
            helix.set_virtual_text("/tmp/nope.rs");
        });
        "#,
    )
    .unwrap();
    let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)), docs: vec![] };
    run_command("dec1", &ctx).unwrap();
    let reqs = take_decorations();
    assert_eq!(reqs.len(), 3);
    // 路径已 canonicalize(相对→绝对:用绝对路径输入,断言原样)
    assert_eq!(reqs[0].doc.as_deref(), Some("/tmp/a.rs"));
    match &reqs[0].kind {
        crate::types::DecorationKind::VirtualText { row, col, text, style } => {
            assert_eq!((*row, *col), (0, 0));
            assert_eq!(text, "hi");
            assert_eq!(style.as_deref(), Some("ui.help"));
        }
        other => panic!("expected VirtualText, got {other:?}"),
    }
    match &reqs[1].kind {
        crate::types::DecorationKind::Highlight { sr, sc, er, ec, style } => {
            assert_eq!((*sr, *sc, *er, *ec), (0, 0, 1, 2));
            assert_eq!(style.as_deref(), Some("ui.selection"));
        }
        other => panic!("expected Highlight, got {other:?}"),
    }
    assert!(matches!(reqs[2].kind, crate::types::DecorationKind::Clear));
}
```

预期:FAIL(编译错误:take_decorations/DecorationKind 不存在)。

- [ ] **步骤 2:实现队列与 take/reset**

1. `helix-js/src/types.rs`:

```rust
/// 插件装饰请求:set_virtual_text / set_highlight 入队,应用时按 doc 整体替换
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecorationKind {
    VirtualText { row: usize, col: usize, text: String, style: Option<String> },
    Highlight { sr: usize, sc: usize, er: usize, ec: usize, style: Option<String> },
    /// 清除该 doc 全部插件装饰(set_virtual_text 只传 path 时产生)
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecorationRequest {
    /// Some(canonical path) = 目标 doc;None = 清空所有 doc(仅脚本重载产生)
    pub doc: Option<String>,
    pub kind: DecorationKind,
}
```

2. `helix-js/src/state.rs`(仿 `CURRENT_EDITS`):

```rust
pub(crate) static DECORATION_REQUESTS: RefCell<Vec<DecorationRequest>> = const { RefCell::new(Vec::new()) };
pub(crate) fn with_decoration_requests<T>(f: impl FnOnce(&mut Vec<DecorationRequest>) -> T) -> T {
    f(&mut DECORATION_REQUESTS.borrow_mut())
}
```

3. `helix-js/src/commands.rs`:

```rust
/// 取走并清空装饰请求队列(事务开启时积压);与 take_edits 同构
pub fn take_decorations() -> Vec<DecorationRequest> {
    crate::init();
    if crate::state::with_txn_depth(|d| *d > 0) {
        return Vec::new();
    }
    crate::state::with_decoration_requests(std::mem::take)
}
```

   复位:grep `with_edits(|c| c.clear())`,在每处旁加 `crate::state::with_decoration_requests(|c| c.clear());`(命令入口、事件入口、handler 抛错路径、js_reload)。

4. `helix-js/src/lib.rs` 注册:

```rust
.function(NativeFunction::from_fn_ptr(commands::js_set_virtual_text), JsString::from("set_virtual_text"), 5)
.function(NativeFunction::from_fn_ptr(commands::js_set_highlight), JsString::from("set_highlight"), 7)
```

- [ ] **步骤 3:实现两个 JS 函数**

`helix-js/src/commands.rs`(路径 canonicalize 复用 `js_by_path` 的模式):

```rust
pub(crate) fn js_set_virtual_text(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.set_virtual_text: path must be a string")))
    })?;
    let canonical = helix_stdx::path::canonicalize(&path).to_string_lossy().into_owned();
    let kind = match args.get(3) {
        // text 省略/undefined = 清除该 doc 全部插件装饰
        Some(v) if v.is_null_or_undefined() => DecorationKind::Clear,
        _ => {
            let row: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
            let col: usize = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
            let text: String = args.get(3).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
            let style: Option<String> = match args.get(4) {
                Some(v) if !v.is_null_or_undefined() => Some(v.try_js_into(context)?),
                _ => None,
            };
            DecorationKind::VirtualText { row, col, text, style }
        }
    };
    crate::state::with_decoration_requests(|c| c.push(DecorationRequest { doc: Some(canonical), kind }));
    Ok(JsValue::undefined())
}

pub(crate) fn js_set_highlight(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.set_highlight: path must be a string")))
    })?;
    let canonical = helix_stdx::path::canonicalize(&path).to_string_lossy().into_owned();
    let sr: usize = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let sc: usize = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let er: usize = args.get(3).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let ec: usize = args.get(4).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let style: Option<String> = match args.get(5) {
        Some(v) if !v.is_null_or_undefined() => Some(v.try_js_into(context)?),
        _ => None,
    };
    crate::state::with_decoration_requests(|c| c.push(DecorationRequest {
        doc: Some(canonical),
        kind: DecorationKind::Highlight { sr, sc, er, ec, style },
    }));
    Ok(JsValue::undefined())
}
```

- [ ] **步骤 4:单测转绿 + 补 txn 门控/复位/校验测试**

运行:`cargo test -p helix-js decorations` 预期 PASS。然后加:

```rust
#[test]
fn decorations_txn_holds_and_reset() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    // 事务门控:begin 后 take 空,end 后取到
    load_script(r#"
        helix.register_command("dec-txn", () => {
            helix.begin_edit();
            helix.set_virtual_text("/tmp/t.rs", 0, 0, "x");
            helix.end_edit();
        });
        helix.register_command("dec-bad", () => { helix.set_virtual_text(42); });
    "#).unwrap();
    let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)), docs: vec![] };
    // 无事务时 set 后立即可取(单测直调 set → take 验证普通路径)——上面的命令已含 begin/end,
    // 取到即证明 end 后放行
    run_command("dec-txn", &ctx).unwrap();
    assert_eq!(take_decorations().len(), 1);
    // 类型校验:path 非字符串 → 命令失败
    assert!(run_command("dec-bad", &ctx).is_err());
    // 命令开始复位:上一命令残留不跨命令
    assert!(take_decorations().is_empty());
}
```

- [ ] **步骤 5:load_script_named 挂 reload-clear + 单测**

`helix-js/src/commands.rs` 的 `load_script_named`(~1101)入口(求值前)加:

```rust
// 脚本(重)加载不继承旧装饰:全局 Clear(term 侧 doc: None = 清空所有 doc)
crate::state::with_decoration_requests(|c| c.push(DecorationRequest { doc: None, kind: DecorationKind::Clear }));
```

单测:

```rust
#[test]
fn decorations_cleared_on_script_load() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    assert!(take_decorations().is_empty());
    load_script("helix.register_command('x', () => {});").unwrap();
    let reqs = take_decorations();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].doc, None);
    assert!(matches!(reqs[0].kind, crate::types::DecorationKind::Clear));
}
```

- [ ] **步骤 6:全量验证 + Commit**

```bash
cargo test -p helix-js
cargo clippy --all-targets 2>&1 | tail -3
```

预期:helix-js 全绿(含旧测试),clippy 零警告。

```bash
git add helix-js
git commit -m "feat(js): 装饰请求队列——set_virtual_text/set_highlight 入队,take_decorations 镜像 take_edits,脚本重载全局 Clear"
```

---

### 任务 2:应用/存储/渲染(term + view + integration)

**文件:** document.rs、view.rs、editor.rs、typed.rs、application.rs、plugin_panel.rs、plugin_popup.rs、integration.rs、plugin_decorations.rs(新)、plugin-api.md

- [ ] **步骤 1:Document 存储字段 + 渲染注入**

1. `helix-view/src/document.rs`(放 InlineAnnotation 相关附近):
```rust
/// 插件装饰(per-doc,所有 view 共享);坐标 = char 索引(应用时换算),不随事务重映射
#[derive(Debug, Clone, Default)]
pub struct PluginDecorations {
    pub virtual_text: Vec<PluginInlineAnnotation>,
    pub highlights: Vec<PluginHighlight>,
}

#[derive(Debug, Clone)]
pub struct PluginInlineAnnotation {
    pub char_idx: usize,
    pub text: Tendril,
    pub style: Option<String>,  // 主题 scope;None/解析失败 = 不渲染
}

#[derive(Debug, Clone)]
pub struct PluginHighlight {
    pub start: usize,
    pub end: usize,
    pub style: Option<String>,
}
```

   `Document` 结构体加 `pub plugin_decorations: PluginDecorations,`,构造处初始化为 `PluginDecorations::default()`(grep Document 结构体初始化点,一般 1-2 处;Tendril 已在 document.rs 可用或经 text_annotations 引入,若无则 `use helix_core::text_annotations::...`——按编译器提示)。

2. `helix-view/src/view.rs` `text_annotations`(~458)inlay hints 段后加(theme 参数已有):

```rust
// 插件 virtual text:按 style 分组注入(同组共享一个 style 是 TextAnnotations 的形态)
if !doc.plugin_decorations.virtual_text.is_empty() {
    let mut groups: Vec<(Option<String>, Vec<helix_core::text_annotations::InlineAnnotation>)> = Vec::new();
    for a in &doc.plugin_decorations.virtual_text {
        let ann = helix_core::text_annotations::InlineAnnotation::new(a.char_idx, a.text.clone());
        if let Some(g) = groups.iter_mut().find(|(s, _)| s == &a.style) {
            g.1.push(ann);
        } else {
            groups.push((a.style.clone(), vec![ann]));
        }
    }
    for (scope, anns) in groups {
        let style = scope.as_deref().and_then(|s| theme.and_then(|t| t.find_highlight(s)));
        text_annotations.add_inline_annotations(anns, style);
    }
}
```

3. `helix-term/src/ui/editor.rs` overlays(~160,`highlight_focused_view_elements` push 之后)加:

```rust
// 插件区域高亮:按 style 分组,组内排序合并重叠(OverlayHighlights 假设组内不重叠)
let plugin_hl = &doc.plugin_decorations.highlights;
if !plugin_hl.is_empty() {
    use std::ops::Range;
    let mut groups: Vec<(Option<String>, Vec<Range<usize>>)> = Vec::new();
    for h in plugin_hl {
        let r = h.start..h.end;
        if let Some(g) = groups.iter_mut().find(|(s, _)| s == &h.style) {
            g.1.push(r);
        } else {
            groups.push((h.style.clone(), vec![r]));
        }
    }
    for (scope, mut ranges) in groups {
        ranges.sort_unstable_by_key(|r| r.start);
        let mut merged: Vec<Range<usize>> = Vec::new();
        for r in ranges {
            if let Some(last) = merged.last_mut() {
                if r.start <= last.end {
                    last.end = last.end.max(r.end);
                    continue;
                }
            }
            merged.push(r);
        }
        if let Some(style) = scope.as_deref().and_then(|s| theme.find_highlight(s)) {
            overlays.push(syntax::OverlayHighlights::Homogeneous { highlight: style, ranges: merged });
        }
    }
}
```

   `theme` 与 `syntax` 在该函数作用域内确认可用(theme 已用于内建 overlay;`use helix_core::syntax` 或已 import,按编译器提示)。

- [ ] **步骤 2:apply_plugin_decorations + 5 调用点**

> **TDD 顺序:先写本任务步骤 3 的红测试(替换语义/未打开忽略),确认 FAIL(字段已存在但未接线 → 断言挂),再实现本步;实现后转绿。**

`helix-term/src/commands/typed.rs`(apply_plugin_edits 附近)加:

```rust
/// 把插件装饰请求按 doc 应用:Clear → 清空;VirtualText/Highlight → 追加(整体替换)。
/// doc: None(仅脚本重载产生)→ 清空所有 doc。未打开 path → 静默忽略(尽力而为层)。
pub(crate) fn apply_plugin_decorations(
    editor: &mut Editor,
    reqs: &[helix_js::DecorationRequest],
) -> anyhow::Result<()> {
    use helix_view::document::{PluginDecorations, PluginHighlight, PluginInlineAnnotation};

    // 全局清空(脚本重载):先独立处理,避免与 per-doc 借用交错
    let has_global_clear = reqs.iter().any(|r| r.doc.is_none());
    if has_global_clear {
        for doc in editor.documents.values_mut() {
            doc.plugin_decorations = PluginDecorations::default();
        }
    }
    let per_doc = reqs.iter().filter(|r| r.doc.is_some());
    let mut by_doc: Vec<(&String, Vec<&helix_js::DecorationRequest>)> = Vec::new();
    for req in per_doc {
        let path = req.doc.as_ref().unwrap();
        if let Some(g) = by_doc.iter_mut().find(|(p, _)| p == &path) {
            g.1.push(req);
        } else {
            by_doc.push((path, vec![req]));
        }
    }
    for (path, group) in by_doc {
        let Some(id) = editor
            .documents
            .iter()
            .find(|(_, d)| d.path().is_some_and(|p| p.to_string_lossy() == path.as_str()))
            .map(|(id, _)| *id)
        else {
            continue;  // 未打开 → 忽略
        };
        let doc = editor.document_mut(id).expect("found above");
        let text = doc.text().clone();
        let mut virtual_text = Vec::new();
        let mut highlights = Vec::new();
        for req in group {
            match &req.kind {
                helix_js::DecorationKind::Clear => {
                    virtual_text.clear();
                    highlights.clear();
                }
                helix_js::DecorationKind::VirtualText { row, col, text: t, style } => {
                    virtual_text.push(PluginInlineAnnotation {
                        char_idx: pos_to_char(&text, *row, *col),
                        text: t.clone().into(),
                        style: style.clone(),
                    });
                }
                helix_js::DecorationKind::Highlight { sr, sc, er, ec, style } => {
                    highlights.push(PluginHighlight {
                        start: pos_to_char(&text, *sr, *sc),
                        end: pos_to_char(&text, *er, *ec),
                        style: style.clone(),
                    });
                }
            }
        }
        doc.plugin_decorations = PluginDecorations { virtual_text, highlights };
    }
    Ok(())
}
```

   5 个 take/apply 调用点(与 `take_edits`/`apply_plugin_edits` 同点,grep 定位精确行):每处 `apply_plugin_edits` 调用后加:

```rust
let decorations = helix_js::take_decorations();
if !decorations.is_empty() {
    if let Err(err) = apply_plugin_decorations(editor, &decorations) {
        editor.set_error(format!("plugin decorations failed: {err}"));
    }
}
```

   (typed.rs 命令/事件入口、application.rs 泵循环、plugin_panel.rs / plugin_popup.rs onKey;各文件已有 `editor`/`cx.editor` 引用,按所在作用域命名)。

- [ ] **步骤 3:写失败集成测试(替换语义 + Clear)**

`helix-term/tests/integration.rs` 加 `mod plugin_decorations;`。新建 `helix-term/tests/test/plugin_decorations.rs`:

```rust
use super::*;

// 命令里 set_virtual_text/set_highlight 当前 doc → 白盒断言 plugin_decorations 字段与 char 坐标
#[tokio::test(flavor = "multi_thread")]
async fn plugin_decorations_applied_and_replaced() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "one\ntwo\n")?;  // char 索引:line2 起点 = 4
    let plugin_path = dir.path().join("dec.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"
            helix.register_command("dec1", () => {{
                helix.set_virtual_text("{f}", 1, 1, "X", "ui.help");
                helix.set_highlight("{f}", 0, 0, 1, 3, "ui.selection");
            }});
            helix.register_command("dec2", () => {{
                helix.set_virtual_text("{f}", 0, 0, "Y");
            }});
            helix.register_command("dec-clear", () => {{
                helix.set_virtual_text("{f}");
            }});
            "#,
            f = file.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":dec1<ret>"), None),
            (
                Some(":dec1<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    let pd = &doc.plugin_decorations;
                    assert_eq!(pd.virtual_text.len(), 1);
                    assert_eq!(pd.virtual_text[0].char_idx, 5);  // (1,1) → line2 起点4 + 1
                    assert_eq!(pd.virtual_text[0].text.to_string(), "X");
                    assert_eq!(pd.virtual_text[0].style.as_deref(), Some("ui.help"));
                    assert_eq!(pd.highlights.len(), 1);
                    assert_eq!((pd.highlights[0].start, pd.highlights[0].end), (0, 4));  // (0,0)-(1,3):line1 全 + line2 前3
                    assert_eq!(pd.highlights[0].style.as_deref(), Some("ui.selection"));
                }),
            ),
            // 替换语义:dec2 只推一个 virtual text → 高亮消失、旧 virtual text 被替换
            (Some(":dec2<ret>"), None),
            (
                Some(":dec2<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.plugin_decorations.virtual_text.len(), 1);
                    assert_eq!(doc.plugin_decorations.virtual_text[0].char_idx, 0);
                    assert_eq!(doc.plugin_decorations.virtual_text[0].text.to_string(), "Y");
                    assert!(doc.plugin_decorations.highlights.is_empty(), "整体替换:旧高亮不残留");
                }),
            ),
            // Clear:只传 path → 全部清空
            (Some(":dec-clear<ret>"), None),
            (
                Some(":dec-clear<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert!(doc.plugin_decorations.virtual_text.is_empty());
                    assert!(doc.plugin_decorations.highlights.is_empty());
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 未打开 path → 不崩、不写入任何 doc
#[tokio::test(flavor = "multi_thread")]
async fn plugin_decorations_unknown_path_ignored() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("dec-unknown.js");
    std::fs::write(
        &plugin_path,
        r#"helix.register_command("dec-unknown", () => {
            helix.set_virtual_text("/nonexistent-helix-xyz.rs", 0, 0, "X");
        });"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":dec-unknown<ret>"), None),
            (
                Some(":dec-unknown<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert!(doc.plugin_decorations.virtual_text.is_empty(), "未打开 path 静默忽略");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

先跑(此时 apply 未接线,装饰字段为空):`cargo test -p helix-term --features integration --test integration plugin_decorations` — 断言挂(替换语义测试的"整体替换"断言失败),确认 RED。步骤 3 实现后转 GREEN。

- [ ] **步骤 4:集成测试——doc-change 重推 + async begin_edit**

```rust
// doc-change 监听重推:替换语义保证无残留(插件在 doc-change 里重推)
#[tokio::test(flavor = "multi_thread")]
async fn plugin_decorations_repush_on_doc_change() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("dec-docchange.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"
            helix.on("doc-change", (doc) => {{
                // 每次变更重推:位置跟随变更后的文本
                helix.set_virtual_text("{f}", 0, doc.text.length, "!", null);
            }});
            helix.register_command("dec-init", () => {{
                helix.set_virtual_text("{f}", 0, 0, "Z");
            }});
            "#,
            f = file.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":dec-init<ret>"), None),
            (
                Some(":dec-init<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.plugin_decorations.virtual_text.len(), 1);
                    assert_eq!(doc.plugin_decorations.virtual_text[0].char_idx, 0);
                }),
            ),
            // 编辑触发 doc-change → handler 重推(替换):只剩新位置
            (Some("iX<esc>"), None),
            (
                Some("iX<esc>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.plugin_decorations.virtual_text.len(), 1);
                    // "Xone\n" 长度 5 → 重推位置 (0,5) → char_idx 5;旧的 (0,0) 被替换
                    assert_eq!(doc.plugin_decorations.virtual_text[0].char_idx, 5);
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// async + begin_edit:装饰与编辑同 hold,结束一起应用
#[tokio::test(flavor = "multi_thread")]
async fn plugin_decorations_async_begin_edit() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("dec-async.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"
            helix.register_command("dec-async", () => {{
                helix.begin_edit();
                helix.set_virtual_text("{f}", 0, 0, "A");
                helix.run_async("echo 1").then(() => {{
                    helix.set_highlight("{f}", 0, 0, 1, 0, "ui.selection");
                    helix.end_edit();
                }});
            }});
            "#,
            f = file.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":dec-async<ret>"), None),
            // 后续键驱动 pump,等 async 回调 settle;断言最终状态
            (Some(":dec-async<ret>"), None),
            (
                Some(":dec-async<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.plugin_decorations.virtual_text.len(), 1);
                    assert_eq!(doc.plugin_decorations.virtual_text[0].text.to_string(), "A");
                    assert_eq!(doc.plugin_decorations.highlights.len(), 1);
                    assert_eq!(doc.plugin_decorations.highlights[0].start, 0);
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

说明:async 测试的重复 `:dec-async` 是让 pump 消化 `.then` 回调(与批次 2 async 测试同款技巧;若断言竞态,在断言前补一个空 pump 键)。doc-change 测试依赖批次 1 的 doc-change 事件 + 防抖窗口,`iX<esc>` 触发。

- [ ] **步骤 5:plugin-api.md 文档**

`docs/plugin-api.md` 加一节(仿 by_path 小节):

```markdown
### `helix.set_virtual_text(path, row, col, text, style)` / `helix.set_highlight(path, sr, sc, er, ec, style)`(装饰/标记)

给已打开 buffer 加装饰(不修改文本;命令/事件期间累积,应用时**整体替换**该 buffer 的插件装饰):

```js
helix.set_virtual_text("src/main.rs", 1, 4, " // TODO", "ui.help");
helix.set_highlight("src/main.rs", 0, 0, 5, 0, "ui.selection");   // [start, end) 半开区间
helix.set_virtual_text("src/main.rs");                            // 清除该 buffer 全部装饰
```

- `style`: 主题 scope 字符串或 null(解析失败/省略 = 不渲染)
- 路径规则与 `by_path` 一致(规范化匹配,未打开静默忽略);坐标基于命令开始时的快照,不随文档变更重映射——监听 `doc-change` 重推
- 装饰按 buffer 存储,所有窗口共享;buffer 关闭或脚本重载时清空
```

- [ ] **步骤 6:全量验证 + Commit**

```bash
cargo build -p helix-term
cargo test -p helix-term --features integration --test integration plugin_decorations
cargo test -p helix-js
cargo clippy --all-targets 2>&1 | tail -3
```

预期:build 通过;plugin_decorations 4 测试 PASS;helix-js 全绿;clippy 零警告。

```bash
git add helix-view helix-term docs/plugin-api.md
git commit -m "feat(term,view): 装饰应用与渲染——Document.plugin_decorations 按 doc 替换,text_annotations/overlays 注入 + 集成测试"
```

---

## 自检记录

**规格覆盖度:**
- 2.1 API(两函数签名/Clear 形式/参数校验)→ 任务 1 步骤 1-4 ✓
- 2.2 语义(整体替换/生命周期/坐标快照/txn 镜像)→ 任务 1 步骤 4、任务 2 步骤 2 ✓
- 2.3 数据流(队列/应用/重载清空)→ 任务 1 步骤 2、5、任务 2 步骤 2 ✓
- 2.4 渲染注入(text_annotations/overlays)→ 任务 2 步骤 1 ✓
- 2.5 边界(样式回退/重叠合并/未打开忽略/LSP 共存)→ 任务 2 步骤 1、2 ✓
- 3.1 单测(校验/take/txn/复位)→ 任务 1 步骤 1、4、5 ✓
- 3.2 集成 6 条 → 任务 2 步骤 3、4 ✓

**占位符扫描:** 无 TODO/待定;代码块为实际实现代码。✓

**类型一致性:** `DecorationKind`/`DecorationRequest`/`PluginDecorations`/`PluginInlineAnnotation`/`PluginHighlight` 跨任务签名一致;`js_set_virtual_text` 注册名与文档/测试一致;`pos_to_char`/`add_inline_annotations`/`OverlayHighlights::Homogeneous` 均为既有 API。✓
