# completion 增强(snippet 源 + 匹配高亮 + 行渲染钩子)实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** 原生 completion 补上 friendly-snippets 式自定义 snippet 源、nucleo 匹配高亮、可选 JS 行渲染钩子(`set_completion_render`),达到 cmp 级补全体验。

**架构:** 三块独立:① `CompletionProvider::Snippet` variant + term 侧 `CompletionItem::Snippet`(accept 时经现有 `generate_transaction_from_snippet` 展开占位符);② score() 里 `Atom::indices` 拿匹配区间存进 CompletionItem 渲染字段,format() 分段高亮(theme key `ui.completion.match`);③ `helix.set_completion_render` 钩子照 `set_component_render` 模式(helix-js 注册 + term 渲染调用),返回 null/抛错回退原生两列。

**技术栈:** Rust(helix-core + helix-term + helix-view + helix-js boa)、serde_json、nucleo(已依赖)、tui。

**规格:** `docs/superpowers/specs/2026-08-29-js-completion-enhance-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-core/src/completion.rs` | `CompletionProvider` 加 `Snippet` variant |
| `helix-term/src/handlers/completion/item.rs` | 加 `SnippetCompletionItem` struct + `CompletionItem::Snippet` variant + filter_text/provider/provider_priority 分支 |
| `helix-term/src/handlers/completion/snippet.rs` | **新建**:读 `~/.config/helix/snippets/<lang>.json`(回退 all.json)→ 构建 Snippet 候选 |
| `helix-term/src/handlers/completion/request.rs` | `request_incomplete_completion_list` 挂 snippet handler(spawn_blocking) |
| `helix-term/src/ui/completion.rs` | format() Snippet 分支(kind "snippet"/15)、accept Snippet 分支(展开)、匹配高亮渲染、行渲染钩子调用 |
| `helix-js/src/popup.rs` | `js_set_completion_render` + `render_completion_row`(照 set_component_render) |
| `helix-js/src/lib.rs` | 注册 native fn + 单测 |
| `runtime/themes/base16_default_theme.toml` | `ui.completion.match` 主题 key(默认 underline) |
| `docs/plugin-api.md` | set_completion_render 文档一节 |

关键实现位置(执行时必读):

- **触发链路**:`trigger_auto_completion`(handlers/completion.rs:118)→ `CompletionEvent::AutoTrigger` → request.rs `handle_event` → `request_incomplete_completion_list`(约 225 行)spawn LSP(`request_completions_from_language_server`)/path(`path_completion`)/word(`word::completion`)。snippet 在 word 之后加一个 spawn_blocking。
- **排序语义**(completion.rs score() 末尾):升序 `(score<=min_score, Reverse(preselect), provider_priority(), Reverse(score), i)`。LSP priority=-(server index)(≤0),Other(word/path)=1。**Snippet 用 0**(LSP 后、Word 前)。
- **匹配引擎**:score() 用 `Atom::new(pattern, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy, false)`。`Atom::indices(Utf32Str, &mut matcher, &mut indices) -> Option<u32>`(nucleo-matcher pattern.rs:331)返回 score 并填充匹配位置;**indices 是 grapheme 位置**(picker.rs:796 注释)。
- **menu Item trait**:`Item::format(&self, data) -> Row` 无匹配参数——高亮区间存 `CompletionItem.match_indices: Vec<u32>` 渲染字段,score() 里经 `update_options()` 返回的 `&mut Vec<CompletionItem>` 更新。
- **accept 展开**:`CompletionItem::Lsp` 分支走 `lsp_item_to_transaction`(ui/completion.rs:591);snippet 分支直接 `Snippet::parse(body)` + `util::generate_transaction_from_snippet(doc.text(), selection, edit_offset, replace_mode, snippet, &mut doc.snippet_ctx())`。**edit_offset 覆盖光标前已输入单词**(参照 word.rs 的 edit_diff:光标前单词范围),否则会残留前缀。
- **JS hook 存储模式**:照 `js_set_component_render`(popup.rs:129)+ `render_component`(popup.rs:951);注册校验函数类型,调用返回 `Result<Content>`。
- **snippet 目录**:`helix_loader::config_dir().join("snippets")`(与 typed.rs:5305 的 plugins 目录同法)。

---

### 任务 1:枚举与候选类型扩展(core + item.rs + format 分支)

**文件:**
- 修改:`helix-core/src/completion.rs`(CompletionProvider 枚举)
- 修改:`helix-term/src/handlers/completion/item.rs`
- 修改:`helix-term/src/ui/completion.rs`(format kind 分支)
- 测试:`helix-term/src/ui/completion.rs` 底部 tests mod

- [ ] **步骤 1:写失败测试(provider_priority 与 kind)**

在 `helix-term/src/ui/completion.rs` 的 `#[cfg(test)]` mod 里加(参照现有 `format_kind_text_without_hook` 测试):

```rust
#[test]
fn snippet_item_kind_and_priority() {
    use crate::handlers::completion::{CompletionItem, SnippetCompletionItem};
    use helix_core::completion::CompletionProvider;
    let item = CompletionItem::Snippet(SnippetCompletionItem {
        label: "fn".into(),
        body: "function ${1:name}(${2:params}) {\n\t${0}\n}".into(),
        description: Some("Function declaration".into()),
        provider_priority: 0,
    });
    assert_eq!(item.provider(), CompletionProvider::Snippet);
    assert_eq!(item.provider_priority(), 0);
    // format:kind 文本 "snippet"、kind_num 15
    let row = CompletionItem::format(&item, &Style::default());
    assert_eq!(cells(&row), vec!["fn".to_string(), "snippet".to_string()]);
}
```

预期:FAIL(CompletionItem::Snippet / SnippetCompletionItem 未定义)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term snippet_item_kind_and_priority 2>&1 | tail -20`
预期:编译错误,`SnippetCompletionItem` 未找到。

- [ ] **步骤 3:实现枚举扩展**

`helix-core/src/completion.rs`:

```rust
pub enum CompletionProvider {
    Lsp(LanguageServerId),
    Path,
    Word,
    Snippet,
}
```

`helix-term/src/handlers/completion/item.rs`:在 `LspCompletionItem` 旁加:

```rust
#[derive(Debug, PartialEq, Clone)]
pub struct SnippetCompletionItem {
    pub label: Cow<'static, str>,
    pub body: String,
    pub description: Option<String>,
    pub provider_priority: i8,
}
```

`CompletionItem` enum 加 variant `Snippet(SnippetCompletionItem)`(与 Lsp 同款,注意已有 `#[allow(clippy::large_enum_variant)]`,Snippet 也是大 variant,不动 lint)。

`impl CompletionItem` 三个函数加分支(provider_priority 返回 0;`filter_text` 返回 `&item.label`;provider 返回 `CompletionProvider::Snippet`);`preselect` 返回 false(在 `CompletionItem::Other(_) => false` 处并列)。`format()` 的 kind 匹配加 `CompletionItem::Snippet(_) => ("snippet".into(), 15)`(ui/completion.rs,`CompletionItem::Other` 分支旁);`deprecated` 匹配加 `CompletionItem::Snippet(_) => false`;`filter_text`(ui/completion.rs 顶部 impl)加 `CompletionItem::Snippet(item) => &item.label`。

其他 `match self` 穷尽处(Rust 编译器会指出来):item.rs 的 `PartialEq<helix_core::CompletionItem> for helix_core::CompletionItem`(加 `CompletionItem::Snippet(_) => false`)、ui/completion.rs accept 的 match(任务 3 补,先加 `CompletionItem::Snippet(_) => (Transaction::new(doc.text()), None, None)` 占位,任务 3 替换)。

- [ ] **步骤 4:运行测试确认通过**

运行:`cargo test -p helix-term snippet_item_kind_and_priority 2>&1 | tail -20` 和 `cargo build -p helix-term 2>&1 | tail -10`
预期:PASS + 无编译错误。

- [ ] **步骤 5:Commit**

```bash
git add helix-core/src/completion.rs helix-term/src/handlers/completion/item.rs helix-term/src/ui/completion.rs
git commit -m "feat(term): CompletionProvider::Snippet + CompletionItem::Snippet variant(priority 0 排序,kind snippet/15)"
```

---

### 任务 2:snippet handler(读 JSON + 候选构建 + 挂 request.rs)

**文件:**
- 创建:`helix-term/src/handlers/completion/snippet.rs`
- 修改:`helix-term/src/handlers/completion.rs`(mod snippet)
- 修改:`helix-term/src/handlers/completion/request.rs`
- 测试:`helix-term/src/handlers/completion/snippet.rs` 底部 tests mod

- [ ] **步骤 1:写失败测试(JSON 反序列化 + 候选构建)**

`snippet.rs` 底部:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const GOOD: &str = r#"{
      "fn": { "prefix": "fn", "body": ["function ${1:name}() {", "\t${0}", "}"], "description": "Function" },
      "for": { "prefix": ["for", "forof"], "body": ["for (const ${1:x} of ${2:iter}) {", "\t${0}", "}"] }
    }"#;

    #[test]
    fn parse_snippets_json() {
        let parsed: HashMap<String, SnippetDef> = serde_json::from_str(GOOD).unwrap();
        assert_eq!(parsed.len(), 2);
        let f = &parsed["fn"];
        assert_eq!(f.prefix, vec!["fn".to_string()]);
        assert_eq!(f.body.join("\n"), "function ${1:name}() {\n\t${0}\n}");
        assert_eq!(f.description.as_deref(), Some("Function"));
        // 多 prefix:首 prefix 为 label
        let fo = &parsed["for"];
        assert_eq!(fo.prefix, vec!["for".to_string(), "forof".to_string()]);
    }

    #[test]
    fn parse_bad_json_does_not_panic() {
        // 非法 JSON → Err(不是 panic);构建候选返回空
        let items = build_items(r#"{ "broken" "#).unwrap_or_default();
        assert!(items.is_empty());
    }

    #[test]
    fn build_items_uses_first_prefix_and_body() {
        let items = build_items(GOOD).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            CompletionItem::Snippet(s) => {
                assert_eq!(s.label.as_ref(), "fn");
                assert!(s.body.contains("${1:name}"));
                assert_eq!(s.description.as_deref(), Some("Function"));
                assert_eq!(s.provider_priority, 0);
            }
            _ => panic!("expected snippet item"),
        }
    }
}
```

预期:FAIL(`SnippetDef`/`build_items` 未定义)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term parse_snippets_json 2>&1 | tail -10`
预期:编译错误,`SnippetDef` 未找到。

- [ ] **步骤 3:实现 snippet.rs**

```rust
use std::{borrow::Cow, collections::HashMap, sync::Arc};

use helix_core::completion::CompletionProvider;
use helix_event::TaskHandle;
use helix_stdx::rope::RopeSliceExt as _;
use helix_view::{document::SavePoint, handlers::completion::ResponseContext, Document, Editor, ViewId};

use super::{request::Trigger, CompletionItem, CompletionItems, CompletionResponse, SnippetCompletionItem};

const SNIPPETS_DIR: &str = "snippets";

#[derive(Debug, serde::Deserialize)]
struct SnippetDef {
    #[serde(default)]
    prefix: Vec<String>,
    body: Vec<String>,
    description: Option<String>,
}

/// 读 ~/.config/helix/snippets/<lang>.json;无 <lang> 再试 all.json。
/// 返回 (文件路径, 解析出的候选)。
pub(crate) fn load_snippets(language: &str) -> Option<(std::path::PathBuf, Vec<CompletionItem>)> {
    let dir = helix_loader::config_dir().join(SNIPPETS_DIR);
    for name in [language, "all"] {
        let path = dir.join(format!("{name}.json"));
        if path.exists() {
            let items = build_items(&std::fs::read_to_string(&path).ok()?)?;
            return Some((path, items));
        }
    }
    None
}

fn build_items(raw: &str) -> Option<Vec<CompletionItem>> {
    let parsed: HashMap<String, SnippetDef> = serde_json::from_str(raw).ok()?;
    Some(
        parsed
            .into_iter()
            .filter_map(|(_, def)| {
                let label: Cow<'static, str> = def.prefix.into_iter().next()?.into();
                let body = def.body.join("\n");
                Some(CompletionItem::Snippet(SnippetCompletionItem {
                    label,
                    body,
                    description: def.description,
                    provider_priority: 0,
                }))
            })
            .collect(),
    )
}

pub(super) fn completion(
    editor: &Editor,
    trigger: Trigger,
    handle: TaskHandle,
    savepoint: Arc<SavePoint>,
) -> Option<impl FnOnce() -> CompletionResponse> {
    let (view, doc) = current_ref!(editor);
    let language = doc.language()?;
    let path = helix_loader::config_dir().join(SNIPPETS_DIR).join(format!("{language}.json"));
    let path = if path.exists() {
        path
    } else {
        helix_loader::config_dir().join(SNIPPETS_DIR).join("all.json")
    };
    if !path.exists() {
        return None;
    }
    Some(move || {
        if handle.is_canceled() {
            return CompletionResponse {
                items: CompletionItems::Other(Vec::new()),
                provider: CompletionProvider::Snippet,
                context: ResponseContext {
                    is_incomplete: false,
                    priority: 0,
                    savepoint,
                },
            };
        }
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        let items = build_items(&raw).unwrap_or_default();
        CompletionResponse {
            items: CompletionItems::Other(items),
            provider: CompletionProvider::Snippet,
            context: ResponseContext {
                is_incomplete: false,
                priority: 0,
                savepoint,
            },
        }
    })
}
```

注:`build_items` 用 `pub(crate)` 供单测;`load_snippets` 若无人用可删(保留单测用 `build_items` 即可)。

`helix-term/src/handlers/completion.rs`:mod 区加 `mod snippet;`(word 旁)。

`request.rs` 在 word::completion 之后加:

```rust
    if let Some(snippet_completion_request) =
        snippet::completion(editor, trigger, handle.clone(), savepoint)
    {
        requests.spawn_blocking(snippet_completion_request);
    }
```

import 加 `use super::snippet;`(参照 `use super::word;`)。

- [ ] **步骤 4:运行测试确认通过**

运行:`cargo test -p helix-term parse_snippets_json parse_bad_json_does_not_panic build_items_uses_first_prefix_and_body 2>&1 | tail -10`
预期:3 个 PASS。再跑 `cargo build -p helix-term 2>&1 | tail -10`(编译通过)。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/handlers/completion/snippet.rs helix-term/src/handlers/completion.rs helix-term/src/handlers/completion/request.rs
git commit -m "feat(term): snippet completion 源(friendly-snippets JSON,<lang>.json 回退 all.json,priority 0)"
```

---

### 任务 3:accept 展开(占位符 tab 跳转)

**文件:**
- 修改:`helix-term/src/ui/completion.rs`
- 测试:`helix-term/src/ui/completion.rs` tests mod

- [ ] **步骤 1:写失败测试(纯函数 snippet_item_to_transaction)**

参照 `lsp_item_to_transaction`(ui/completion.rs:591)的模式,提取同款纯函数供 Snippet variant 复用并单测。**签名不依赖 Document**(text + selection + snippet_ctx 由调用方传入),保证单测可用。ui/completion.rs tests mod 加:

```rust
#[test]
fn snippet_item_to_transaction_replaces_prefix() {
    use helix_core::snippets::SnippetCtx;
    let rope = Rope::from("fn");
    let selection = Selection::point(2); // 光标在 "fn" 后
    let body = "function ${1:name}() {\n\t${0}\n}";
    let (transaction, snippet) =
        snippet_item_to_transaction(&rope, &selection, body, 2, false, &mut SnippetCtx::default());
    assert!(snippet.is_some());
    let out = transaction.apply(&rope).to_string();
    assert!(out.starts_with("function")); // 前缀 "fn" 被 body 替换
}
```

(`SnippetCtx` 在 `helix_core::snippets`;`Selection::point`/`Rope` 在现有测试 import 里。若 `SnippetCtx::default()` 不存在则 `SnippetCtx::new()`——看 helix_core/snippets 的构造。)

预期:FAIL(`snippet_item_to_transaction` 未定义)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term snippet_item_to_transaction_replaces_prefix 2>&1 | tail -10`
预期:FAIL(或编译错误)。

- [ ] **步骤 3:实现 accept 分支(含纯函数)**

ui/completion.rs 在 `lsp_item_to_transaction` 旁加同款纯函数:

```rust
/// Snippet 候选 → transaction + RenderedSnippet。edit_offset 覆盖光标前已输入单词(删除前缀)。
fn snippet_item_to_transaction(
    text: &Rope,
    selection: &Selection,
    body: &str,
    trigger_offset: usize,
    replace_mode: bool,
    snippet_ctx: &mut SnippetCtx,
) -> (Transaction, Option<RenderedSnippet>) {
    let primary_cursor = selection.primary().cursor(text.slice(..));
    // 光标前单词范围(删除已输入 prefix);无单词则不替换
    let edit_offset = {
        let cursor = helix_core::movement::move_prev_word_start(
            text.slice(..),
            core::Range::point(primary_cursor),
            1,
        );
        if cursor.head == primary_cursor {
            None
        } else {
            Some((cursor.head as i128 - primary_cursor as i128, 0))
        }
    };
    let Ok(snippet) = Snippet::parse(body) else {
        log::error!("Failed to parse snippet: {body:?}");
        return (Transaction::new(text), None);
    };
    let (transaction, snippet) = util::generate_transaction_from_snippet(
        text,
        selection,
        edit_offset,
        replace_mode,
        snippet,
        snippet_ctx,
    );
    (transaction, Some(snippet))
}
```

accept 路径(约 226 行 `let (transaction, additional_edits, snippet) = match item.clone()`),`CompletionItem::Other` 分支旁加:

```rust
CompletionItem::Snippet(item) => {
    let (view, doc) = current!(editor);
    let text = doc.text();
    let selection = doc.selection(view.id);
    let mut ctx = doc.snippet_ctx();
    let (transaction, snippet) = snippet_item_to_transaction(
        text,
        selection,
        &item.body,
        trigger_offset,
        replace_mode,
        &mut ctx,
    );
    (transaction, None, snippet)
}
```

(同时删掉任务 1 步骤 3 放的占位分支。`replace_mode` 来自 `editor.config().completion_replace`,参照 LSP 分支现有取法;`SnippetCtx`/`RenderedSnippet`/`Selection` import 在 ui/completion.rs 现有 use 中补。)

- [ ] **步骤 4:运行测试确认通过**

运行:`cargo test -p helix-term snippet_item_to_transaction_replaces_prefix 2>&1 | tail -10`
预期:PASS。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/ui/completion.rs
git commit -m "feat(term): snippet 候选 accept 展开——snippet_item_to_transaction 纯函数(前缀替换 + ActiveSnippet)"
```

---

### 任务 4:匹配高亮

**文件:**
- 修改:`helix-term/src/handlers/completion/item.rs`(match_indices 渲染字段)
- 修改:`helix-term/src/ui/completion.rs`(score 更新 indices + format 分段高亮)
- 修改:`runtime/themes/base16_default_theme.toml`(`ui.completion.match`)
- 测试:`helix-term/src/ui/completion.rs` tests mod

- [ ] **步骤 1:写失败测试(高亮分段)**

```rust
#[test]
fn highlight_matched_indices() {
    // match_indices 覆盖 label 的 grapheme 位置
    let item = CompletionItem::Other(helix_core::completion::CompletionItem {
        transaction: Transaction::new(&Rope::from("x")),
        label: "formatName".into(),
        kind: "word".into(),
        documentation: None,
        provider: helix_core::completion::CompletionProvider::Word,
    });
    let mut item = item;
    item.set_match_indices(vec![0, 1, 2]); // "for" 匹配 "fmt" 前缀
    let row = CompletionItem::format(&item, &Style::default());
    // label 被分段:匹配段(带样式)+ 未匹配段;cells 拼接后仍等于原 label
    let joined: String = row.0.iter().map(|c| c.content.clone()).collect::<String>();
    assert_eq!(joined, "formatName");
}
```

预期:FAIL(`set_match_indices` 未定义)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term highlight_matched_indices 2>&1 | tail -10`
预期:编译错误。

- [ ] **步骤 3:实现**

**A. match_indices 字段(3 个 struct 各加):**

- `helix-core/src/completion.rs` 的 `CompletionItem` 加 `pub match_indices: Vec<u32>`(构造点 word.rs:80、path.rs:117/123 及单测处补 `match_indices: Vec::new()`,编译错会列出全部)
- `helix-term/src/handlers/completion/item.rs` 的 `LspCompletionItem`、`SnippetCompletionItem` 各加 `pub match_indices: Vec<u32>`
- **PartialEq**:`LspCompletionItem` 的 `#[derive(PartialEq)]` 改为手动 impl(忽略 match_indices,防 replace_option 失效);`SnippetCompletionItem` 与 core 的 derive 保持(无 replace 场景)

**B. menu::Item trait 加默认方法**(`helix-term/src/ui/menu.rs`):

```rust
pub trait Item: Sync + Send + 'static {
    type Data: Sync + Send + 'static;
    fn format(&self, data: &Self::Data) -> Row<'_>;
    /// 匹配高亮位置(grapheme index);None = 不高亮
    fn match_indices(&self) -> Option<&[u32]> {
        None
    }
}
```

`CompletionItem`(term 侧)实现它,返回三个 variant 的 match_indices 字段引用。picker 等其他 Item 实现不受影响(默认 None)。

**C. score() 里填 indices**(ui/completion.rs):`score()` 的 `update_options()` 返回 `(matches, options: &mut Vec<CompletionItem>)`,全量分支:

```rust
matches.clear();
matches.extend(options.iter().enumerate().filter_map(|(i, option)| {
    let text = option.filter_text();
    let mut indices = Vec::new();
    let s = pattern.indices(Utf32Str::new(text, &mut buf), &mut matcher, &mut indices);
    if let Some(score) = s {
        option.match_indices = indices; // options 是 &mut
        Some((i as u32, score as u32 / 3))
    } else {
        None
    }
}));
```

增量分支(`retain_mut`)同法更新(score 保留时也重算 indices)。

**D. menu.rs render 处 patch**(有 theme;参照 picker.rs:774-810 的 grapheme 遍历):

```rust
let rows = options.iter().map(|option| {
    let mut row = option.format(&self.editor_data);
    if let Some(indices) = option.match_indices() {
        if !indices.is_empty() {
            let style = theme.try_get("ui.completion.match").unwrap_or_default();
            highlight_row(&mut row, indices, style);
        }
    }
    row
});
```

menu.rs(或 ui/completion.rs)加纯函数(可单测):

```rust
/// 按 grapheme 位置 patch Row 第一列的匹配段样式(参照 picker.rs 高亮遍历)。
pub fn highlight_row(row: &mut Row, indices: &[u32], style: Style) {
    let Some(cell) = row.cells.first_mut() else { return };
    let Some(spans) = cell.content.lines.first_mut() else { return };
    let mut span_list = Vec::new();
    let mut current = String::new();
    let mut current_style = Style::default();
    let mut grapheme_idx = 0u32;
    let mut iter = indices.iter();
    let mut next = iter.next().copied().unwrap_or(u32::MAX);
    for span in &spans.0 {
        for grapheme in span.content.graphemes(true) {
            let s = if grapheme_idx == next {
                next = iter.next().copied().unwrap_or(u32::MAX);
                span.style.patch(style)
            } else {
                span.style
            };
            if s != current_style {
                if !current.is_empty() {
                    span_list.push(Span::styled(std::mem::take(&mut current), current_style));
                }
                current_style = s;
            }
            current.push_str(grapheme);
            grapheme_idx += 1;
        }
    }
    if !current.is_empty() {
        span_list.push(Span::styled(current, current_style));
    }
    spans.0 = span_list;
}
```

(`row.cells`/`cell.content` 均 pub(helix-tui table.rs:33/84);`Spans(pub Vec<Span>)`(helix-tui text.rs:213);`Span.content: Cow<str>` + `Span.style`。)

**E. theme key**:`runtime/themes/base16_default_theme.toml` 的 `ui.completion` 旁加:

```toml
"ui.completion.match" = { modifiers = ["underline"] }
```

(参照同文件其他 `ui.*` key 写法;base16 模板若为变量映射风格则照抄。) 同时确认 `helix-view/src/theme.rs` 无白名单限制(Theme::get 缺省回退,无需注册)。

- [ ] **步骤 4:运行测试确认通过**

运行:`cargo test -p helix-term highlight_matched_indices 2>&1 | tail -10`
预期:PASS(测试断言调整后:通过 `CompletionItemWithIndices { item, match_indices: vec![0,1,2] }.format(...)` 验证拼接 label 完整;实际高亮样式 patch 在 menu.rs,由 `highlight_row` 单测覆盖:构造 Row + indices,断言分段后样式段数)。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/ui/menu.rs helix-term/src/ui/completion.rs helix-term/src/handlers/completion/item.rs runtime/themes/base16_default_theme.toml
git commit -m "feat(term): completion 匹配高亮——menu::Item::match_indices + Atom::indices + ui.completion.match"
```

---

### 任务 5:set_completion_render 行渲染钩子 + benchmark

**文件:**
- 修改:`helix-js/src/popup.rs`
- 修改:`helix-js/src/lib.rs`
- 修改:`helix-term/src/ui/completion.rs`
- 修改:`docs/plugin-api.md`
- 测试:`helix-js/src/popup.rs` tests mod

- [ ] **步骤 1:写失败测试(注册/读取/抛错回退)**

popup.rs tests mod(参照 set_component_render 测试,lib.rs:2908 附近):

```rust
#[test]
fn completion_render_hook() {
    crate::init();
    // 未注册 → Err
    assert!(crate::popup::render_completion_row("foo", "method", 2, "lsp", None, false, &[]).is_err());
    // 注册后 → 行文本
    load_script(r#"helix.set_completion_render((ctx) => [{ type: "text", text: ctx.label + "|" + ctx.provider, style: "ui.completion" }]);"#).unwrap();
    let content = crate::popup::render_completion_row("foo", "method", 2, "lsp", None, false, &[0, 1]).unwrap();
    let joined = match &content {
        Content::Lines(lines) => lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.text.as_str())
            .collect::<String>(),
        _ => panic!("expected lines"),
    };
    assert_eq!(joined, "foo|lsp");
    // 抛错 → Err(回退原生两列)
    load_script(r#"helix.set_completion_render(() => { throw new Error("x"); });"#).unwrap();
    assert!(crate::popup::render_completion_row("foo", "method", 2, "lsp", None, false, &[]).is_err());
}
```

(Content::Lines 断言方式照 component_render_registration 测试,lib.rs:2920-2924;`s.text` 是该 fork helix-tui Span 的字段,参照现有测试。)

预期:FAIL(`js_set_completion_render`/`render_completion_row` 未定义)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-js completion_render_hook 2>&1 | tail -10`
预期:编译错误。

- [ ] **步骤 3:实现**

popup.rs 照 `js_set_completion_render`(popup.rs:129)模式加:

```rust
/// 补全行渲染钩子注册:helix.set_completion_render(fn)
pub(crate) fn js_set_completion_render(
    _this: &JsValue,
    args: &[JsValue],
    engine: &mut Engine,
) -> JsResult<JsValue> {
    let Some(func) = args.first().and_then(|v| v.as_object()) else {
        return Err(JsError::from_opaque("set_completion_render: expected a function".into()));
    };
    crate::state::with_completion_render_hook(|h| *h = Some(JsFunction::from_object(func).unwrap()));
    Ok(JsValue::undefined())
}

/// 调用补全行渲染钩子;未注册/抛错 → Err(调用方回退原生两列)。
pub fn render_completion_row(
    label: &str,
    kind: &str,
    kind_num: u8,
    provider: &str,
    detail: Option<&str>,
    deprecated: bool,
    match_indices: &[u32],
) -> Result<Content> {
    let engine = crate::state::with_engine(|e| e.clone())...; // 参照 render_component 实现
    ...
    // ctx 对象:{ label, kind, kindNum, provider, detail, deprecated, matchIndices }
    // 返回 Content(元素树:type text/row/col,参照 render_component 的返回构建)
}
```

state.rs 加 `with_completion_render_hook`(照 `with_completion_icon_hook` 模式,promise.rs/popup.rs 现有)。

lib.rs:189 附近注册表加:

```rust
NativeFunction::from_fn_ptr(popup::js_set_completion_render),
JsString::from("set_completion_render"),
```

ui/completion.rs `format()`(menu::Item for CompletionItem,即现有 `impl menu::Item for CompletionItem` 的 format,任务 4 已加 match_indices)开头:

```rust
if let Ok(content) = helix_js::render_completion_row(
    label.as_ref(),
    &kind_text,
    kind_num,
    provider_str,
    detail,
    deprecated,
    &self.match_indices(),
) {
    if let Some(row) = content_to_row(content) {
        return row;
    }
}
// 回退原生两列
```

`content_to_row`:遍历 Content::Lines 的 spans → `menu::Row(vec![menu::Cell])`(label 一段;若 JS 返回多元素,拼成多 Cell——**本批次只支持单行单列拼接**:每 span 一个 Cell 会破坏 Table 对齐,改为全部拼接进第一个 Cell 的 content);style 字符串经 theme 查表(参照 set_statusline 现有 style 解析)。detail 从 `CompletionItem::Lsp` 取 `item.detail.as_deref()`;provider_str 从 `self.provider()` 映射(`"lsp"`/`"word"`/`"path"`/`"snippet"`);kind_text/kind_num 复用 format 里已有的 kind 计算。

- [ ] **步骤 4:测试转绿 + 文档 + benchmark**

运行:`cargo test -p helix-js completion_render_hook 2>&1 | tail -10` 和 `cargo build -p helix-term 2>&1 | tail -10`
预期:PASS + 编译通过。

`docs/plugin-api.md` 加一节(set_completion_render 签名、ctx 字段表、回退语义)。

**benchmark**:`~/.config/helix/init.js` 临时加 3 列渲染钩子(icon + label + provider),开大文件触发补全,`time` 或编辑器内感受帧率;记录结果(每帧 ~20 行 × 钩子耗时)到 `docs/superpowers/handoff/2026-08-29-completion-enhance.md`,若 >8ms/帧,文档注明性能上限并建议降级方案。

- [ ] **步骤 5:Commit**

```bash
git add helix-js/src/popup.rs helix-js/src/lib.rs helix-js/src/state.rs helix-term/src/ui/completion.rs docs/plugin-api.md
git commit -m "feat(js,term): set_completion_render 行渲染钩子(抛错/未注册回退原生两列)+ plugin-api 文档"
```

---

## 收尾(所有任务完成后)

- [ ] 运行 `cargo fmt --all --check` + `cargo clippy -p helix-term -p helix-js 2>&1 | tail -20`(零警告)
- [ ] 运行 `cargo test -p helix-js 2>&1 | tail -10` + `cargo test -p helix-term 2>&1 | tail -10`(全量)
- [ ] **手动验证 snippet(integration 不可行)**:`~/.config/helix/snippets/rust.json` 放一个测试 snippet(如 `fn`),开一个 rust 文件进 insert 打 `fn`,确认候选出现、选中后占位符 tab 跳转可用;完事删除测试文件。原因:integration 的 config dir 不可控(读真实 `~/.config/helix`),自动化会污染开发者环境
- [ ] 写交接文档 `docs/superpowers/handoff/2026-08-29-completion-enhance.md`(验证状态、benchmark 数据、已知边界)
- [ ] Commit 交接文档
