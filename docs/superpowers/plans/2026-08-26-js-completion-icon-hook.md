# 内置补全 kind 图标钩子实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 提供 `helix.set_completion_icon(fn)` 钩子,JS 插件为内置补全菜单的 kind 提供 nerd font 图标;未注册/返回空/抛错时回退现有默认(kind 文本/■色块)。

**架构：** 复用 `set_buffer_icon` 的成熟模式:helix-js 存 JsValue hook(thread_local + Box::leak,防 SIGABRT),提供 `completion_kind_icon(kind: u8) -> Option<String>` 查询函数;helix-term `ui/completion.rs` 的 format 里先查钩子,有图标用图标,否则用现有 kind 渲染(含 COLOR 的 ■ 色块)。kind 数字从现有 match 顺带产出(LSP 协议编号 1-25),不引入 lsp-types 内部访问。

**技术栈：** boa 0.21(JS 引擎)、lsp-types 0.94(`CompletionItemKind` newtype)、tui(menu Row/Cell)、helix-view(theme Style)。

**规格：** `docs/superpowers/specs/2026-08-26-js-completion-icon-hook-design.md`(已批准)

---

### 任务 1：helix-js — hook 存储 + set_completion_icon API + 查询函数

**文件：**
- 修改：`helix-js/src/state.rs`(加 `COMPLETION_ICON_HOOK` + `with_completion_icon_hook`,仿 `BUFFER_ICON_HOOK` 65-67 行/208-213 行)
- 修改：`helix-js/src/popup.rs`(加 `js_set_completion_icon` + `pub fn completion_kind_icon`,仿 `js_set_buffer_icon` 391 行/`bufferline_icon` 1233 行)
- 修改：`helix-js/src/lib.rs`(builder 注册 `set_completion_icon` 1 参,`set_buffer_icon` 注册行 92 附近)

- [ ] **步骤 1：编写失败的测试**(`helix-js/src/popup.rs` 底部 `#[cfg(test)]` 模块,仿现有测试的 `TEST_LOCK` + `crate::init()` 模式)

```rust
#[test]
fn completion_icon_hook() {
    let _guard = crate::tests::TEST_LOCK.lock().unwrap();
    crate::init();
    // 未注册 → None
    assert!(crate::popup::completion_kind_icon(7).is_none());
    // 注册后返回图标;kind 参数透传
    crate::load_script(
        r#"
        helix.set_completion_icon((kind) => "i" + kind);
        "#,
    )
    .unwrap();
    assert_eq!(crate::popup::completion_kind_icon(7).as_deref(), Some("i7"));
    // 返回空串 → None(回退)
    crate::load_script(
        r#"
        helix.set_completion_icon((kind) => kind === 7 ? "" : "x");
        "#,
    )
    .unwrap();
    assert!(crate::popup::completion_kind_icon(7).is_none());
    // 抛错 → None
    crate::load_script(
        r#"
        helix.set_completion_icon((kind) => { throw new Error("boom"); });
        "#,
    )
    .unwrap();
    assert!(crate::popup::completion_kind_icon(7).is_none());
    // 缺参/非函数 → JS 报错
    assert!(crate::load_script(r#"helix.set_completion_icon("x");"#).is_err());
}
```

- [ ] **步骤 2：运行确认失败**

运行：`cargo test -p helix-js --lib completion_icon_hook`
预期：FAIL(编译错误 `completion_kind_icon` 未定义 + `set_completion_icon` 不存在)。

- [ ] **步骤 3：实现**(三个文件)

`state.rs`(仿 BUFFER_ICON_HOOK,66 行后):
```rust
// 持有 JsValue：线程退出时内容泄漏（同上）
static COMPLETION_ICON_HOOK: RefCell<Option<&'static mut Option<JsValue>>> = const { RefCell::new(None) };
```
`state.rs`(仿 with_buffer_icon_hook,208 行后):
```rust
/// 访问 COMPLETION_ICON_HOOK：同上（内容泄漏）
pub(crate) fn with_completion_icon_hook<T>(f: impl FnOnce(&mut Option<JsValue>) -> T) -> T {
    COMPLETION_ICON_HOOK.with(|h| {
        let mut slot = h.borrow_mut();
        f(slot.get_or_insert_with(|| Box::leak(Box::default())))
    })
}
```
`popup.rs`(js_set_buffer_icon 391 行后,仿之):
```rust
pub(crate) fn js_set_completion_icon(_this: &JsValue, args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let undefined = JsValue::undefined();
    let hook = args.first().unwrap_or(&undefined);
    if !hook.is_callable() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "set_completion_icon: expected a function",
        ))));
    }
    crate::state::with_completion_icon_hook(|h| *h = Some(hook.clone()));
    Ok(JsValue::undefined())
}
```
`popup.rs`(bufferline_icon 1233 行后,仿之,传 kind 数字):
```rust
/// 调补全 kind 图标钩子；未注册 / 返回空 / 非字符串 / 抛错 → None。
pub fn completion_kind_icon(kind: u8) -> Option<String> {
    crate::init();
    crate::state::with_engine(|engine| {
        let hook = crate::state::with_completion_icon_hook(|h| h.clone());
        let hook = hook?;
        let func = hook.as_callable().and_then(JsFunction::from_object)?;
        let arg = JsValue::from(kind);
        let undefined = JsValue::undefined();
        let value: JsValue = func.call(&undefined, &[arg], engine).ok()?;
        let s: String = value.try_js_into(engine).ok()?;
        if s.is_empty() { None } else { Some(s) }
    })
}
```
`lib.rs`(`set_buffer_icon` 注册行 92 后):
```rust
.function(NativeFunction::from_fn_ptr(popup::js_set_completion_icon), JsString::from("set_completion_icon"), 1)
```

- [ ] **步骤 4：运行确认通过**

运行：`cargo test -p helix-js --lib`
预期：completion_icon_hook 及其余测试全 PASS。

- [ ] **步骤 5：Commit**

```bash
git add helix-js/src/state.rs helix-js/src/popup.rs helix-js/src/lib.rs
git commit -m "feat(js): set_completion_icon 钩子 + completion_kind_icon 查询(仿 set_buffer_icon)"
```

### 任务 2：helix-term — completion.rs format 接线

**文件：**
- 修改：`helix-term/src/ui/completion.rs`(format 28-101 行:kind match 产出 (Spans, u8),format 优先钩子图标)

- [ ] **步骤 1：编写失败的测试**(`helix-term/src/ui/completion.rs` 底部加 `#[cfg(test)]` 模块)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use helix_term::handlers::completion::{CompletionItem, LspCompletionItem};

    fn lsp_item(kind: Option<lsp::CompletionItemKind>) -> CompletionItem {
        CompletionItem::Lsp(LspCompletionItem {
            item: lsp::CompletionItem { label: "foo".into(), kind, ..Default::default() },
            provider: 0,
            resolved: false,
            provider_priority: 0,
        })
    }

    #[test]
    fn format_kind_text_without_hook() {
        // 未注册钩子 → kind 文本
        let row = CompletionItem::format(&lsp_item(Some(lsp::CompletionItemKind::METHOD)), &Style::default());
        let cells = row.to_cells().map(|c| c.content.to_string()).collect::<Vec<_>>();
        assert_eq!(cells, vec!["foo".to_string(), "method".to_string()]);
    }

    #[test]
    fn format_kind_icon_with_hook() {
        // 注册钩子 → kind cell 是图标字符
        helix_js::init();
        helix_js::load_script(r#"helix.set_completion_icon((k) => "i" + k);"#).unwrap();
        let row = CompletionItem::format(&lsp_item(Some(lsp::CompletionItemKind::METHOD)), &Style::default());
        let cells = row.to_cells().map(|c| c.content.to_string()).collect::<Vec<_>>();
        assert_eq!(cells, vec!["foo".to_string(), "i2".to_string()]);
    }

    #[test]
    fn format_other_item_ignores_hook() {
        // 非 LSP 候选:无数字 kind,kind_num=0;钩子只在 1-25 返回 → 0 回退到 kind 字符串
        helix_js::init();
        helix_js::load_script(r#"helix.set_completion_icon((k) => k >= 1 && k <= 25 ? "i" + k : "");"#).unwrap();
        let item = CompletionItem::Other(helix_core::CompletionItem {
            label: "foo".into(), kind: helix_core::CompletionItemKind::Other("word".into()),
        });
        let row = CompletionItem::format(&item, &Style::default());
        let cells = row.to_cells().map(|c| c.content.to_string()).collect::<Vec<_>>();
        assert_eq!(cells, vec!["foo".to_string(), "word".to_string()]);
    }
}
```

- [ ] **步骤 2：运行确认失败**

运行：`cargo test -p helix-term completion::tests::format_`
预期：FAIL(编译错误或断言不匹配)。注意：`helix_core::CompletionItem` 的字段名以 `helix-core/src/completion.rs` 实际定义为准(实现者先读该文件核对 label/kind 字段与构造方式);`helix_term::handlers` 路径可见性如不对,按 `completion.rs` 实际 use 调整测试引用。

- [ ] **步骤 3：实现**(`completion.rs` format)

把 28-101 行的 kind match 重构为产出 `(Spans, u8)`(现有每个分支的文本/■色块 Spans 保留,顺带加数字,数字与 LSP 协议一致:TEXT=1 … TYPE_PARAMETER=25,COLOR=16),format 里:

```rust
let (kind_spans, kind_num) = match self { /* 重构后的 match,含数字 */ };
let kind_cell = match helix_js::completion_kind_icon(kind_num) {
    Some(icon) => menu::Cell::from(Span::raw(icon)),
    None => menu::Cell::from(kind_spans),
};
menu::Row::new([menu::Cell::from(label), kind_cell])
```

注意:
- COLOR 分支(75-96 行)返回带 ■ 色块的 Spans,必须保留在回退路径
- `CompletionItem::Other`(非 LSP)分支无数字:kind_num 用 0(钩子对 0 返回空则回退,测试 3 已用 1-25 范围钩子验证)

- [ ] **步骤 4：运行确认通过**

运行：`cargo test -p helix-term completion::tests::format_` 和 `cargo test -p helix-js --lib`
预期：全 PASS。再跑 `cargo clippy -p helix-term -p helix-js 2>&1 | tail -5` 确认零警告。

- [ ] **步骤 5：Commit**

```bash
git add helix-term/src/ui/completion.rs
git commit -m "feat(term): 内置补全菜单 kind 图标钩子接线(回退默认文本/■色块)"
```

### 手动验证(两任务后)

- 注册:`helix.load("features/input-completion/index.js")` 后 init.js 加一行 `helix.set_completion_icon((k) => ICONS.getCompletionKindIcon(k));`(icons.js 已加载时)
- ts 文件里打字触发内置 C-x 补全 → 候选行 kind 显示图标
- 不注册/钩子返回空 → 显示默认 kind 文本
