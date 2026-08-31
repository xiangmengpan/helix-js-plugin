# 装饰按行索引 + 粘贴支持实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** ① 装饰渲染从每帧全量 O(n) 降为二分 O(log n + k)(按行索引);② 插件 input 支持 `Event::Paste` 批量插入(修复粘贴进不了 input + 一次 onChange)。

**架构:** 装饰应用时排序存储,渲染时按可见 char 范围二分裁剪;input 批量插入纯函数 + popup/panel 的 Paste 分支。

**技术栈:** Rust(helix-js + helix-term + helix-view)、tokio 集成测试。

**规格:** `docs/superpowers/specs/2026-08-31-js-decor-index-paste-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-term/src/commands/typed.rs` | `apply_plugin_decorations` 应用时排序(virtual_text/highlights 按 char 索引) |
| `helix-view/src/view.rs` | `text_annotations` 加可见 char 范围参数 + 二分裁剪插件段 |
| `helix-term/src/ui/editor.rs` | 可见范围计算传入 text_annotations;插件高亮段二分裁剪 |
| `helix-js/src/input.rs` | 批量插入 `input_insert_batch`(光标处插整段) |
| `helix-term/src/ui/plugin_popup.rs` + `plugin_panel.rs` | `handle_event` 加 `Event::Paste` 分支 |
| 测试 | helix-term 单测(排序/二分)+ integration(粘贴) |

关键实现细节(执行时必读):

- **装饰排序存储**(typed.rs:4774 `PluginDecorations { virtual_text, highlights }` 构建处):构建后各 `sort_unstable_by_key`(virtual_text 按 char_idx;highlights 按 start)。**注意渲染端(editor.rs / view.rs)的"组内排序合并"逻辑仍保留**(二分裁剪后子集内仍需按 style 分组 + 合并重叠——见下)。
- **可见 char 范围计算**(editor.rs,view_offset.anchor 是 char 索引 + inner.height 行数):

```rust
// 可见 char 范围 [anchor, anchor + 可见行总长)(粗算:anchor 行到第 height 行的边界)
let visible_start = view_offset.anchor;
let visible_end = {
    // anchor 所在行 → 前进 height 行(用 doc.text() 的行边界;超出 clamp 到文本末尾)
    // 实现:从 anchor 的行首算,跳过 height 行的 \n
    ...
};
```

  (精确实现:用 `doc.text()` 从 anchor 出发数 height 个换行;有现成工具如 `text.line_to_char(line + height)` 之类,执行时按文本 API 最简实现。**简化许可**:可见范围可以粗算(如 anchor 起的所有行),二分只需"不漏"可见行——宁可多取(稍宽的 char 范围)不可少取,正确性优先)
- **二分辅助**(view.rs 或 editor.rs 私有 fn):

```rust
/// 排序数组上取 [start, end) 的子区间(二分;空数组快速返回)
fn slice_range<T: PartialOrd>(items: &[T], key: fn(&T) -> usize, start: usize, end: usize) -> &[T] {
    if items.is_empty() { return &[]; }
    let lo = items.partition_point(|t| key(t) < start);
    let hi = items.partition_point(|t| key(t) < end);
    &items[lo..hi]
}
```

- **view.rs text_annotations 签名变更**:`text_annotations(&self, doc, theme)` → 加 `visible: Option<(usize, usize)>`(可见 char 范围;None = 全量,兼容 picker 等其它调用方?——**grep 调用方确认:仅 editor.rs:93 一处,直接加必填参数**)。插件段:`if !doc.plugin_decorations.virtual_text.is_empty()` 前先二分取子集,子集再按 style 分组(既有逻辑),分组时**跳过空组**。注意:二分取子集后,子集内按 char_idx 有序(全局排序的子集仍有序)——按 style 分组会打乱顺序?分组后每组内顺序 = 原顺序的子序列(仍有序)——**但 TextAnnotations 要求组内有序,分组保序 ✓;OverlayHighlights 要求组内不重叠 → 仍需排序+合并**。
- **editor.rs 插件高亮段**:同样先二分取可见子集,再按 style 分组 + 排序合并(既有逻辑)。
- **input 批量插入**(input.rs):

```rust
/// 光标处插入整段文本(粘贴用);返回新值(触发 onChange 一次)
pub fn input_insert_batch(state: &mut InputState, text: &str) -> String {
    let mut chars: Vec<char> = state.value.chars().collect();
    let mut ins: Vec<char> = text.chars().collect();
    let rest: Vec<char> = chars.split_off(state.cursor);
    state.value = chars.into_iter().chain(ins).chain(rest).collect();
    state.cursor += ins.len();
    state.value.clone()
}
```

  dispatch 层加 `dispatch_input_paste(popup_id, node_id, text)`(仿 dispatch_input_key:查 state → input_insert_batch → onChange)。
- **Paste 分支**(plugin_popup.rs / plugin_panel.rs 的 handle_event,`let Event::Key(...) else` 之前):

```rust
let Event::Paste(contents) = event else {
    // 非按键非粘贴:忽略(现状)
    return EventResult::Ignored(None);
};
// 仅焦点在 input 时消费:批量插入 + 一次 onChange + drain;否则忽略(冒泡给编辑器正文粘贴)
if let Some(fid) = self.focus.as_ref() {
    if helix_js::input_has_state(self.id, fid) {
        if helix_js::dispatch_input_paste(self.id, fid, contents).is_ok() {
            drain_msgs(cx);
            return EventResult::Consumed(None);
        }
    }
}
EventResult::Ignored(None)
```

  (drain_msgs 闭包在 popup 的 handle_event 内已定义;panel 需要加——参照 popup 的形态。**注意**:现在 handle_event 结构是 `let Event::Key(key_event) = event else { return Ignored };`——Paste 分支要在 key_to_plugin_key 之前处理,或在 `else` 分支里先判 Paste。执行时按最小改动安排结构)
- **单行 input 粘贴含 \n**:input_insert_batch 直接插整段(含 \n)——与逐字符粘贴的差异:逐字符时 input_edit 对 \n?查现有 input_edit 是否处理 "\n" 键(单行时——multiline 分支处理 Enter;单行 Enter 不走 input_edit)。粘贴含 \n 到单行 input:直接插入(值含 \n)——**与"逐字符粘贴"的既有行为一致即可(执行时确认逐字符行为,保持一致)**。
- **测试命令**:`cargo build -p helix-term`(实现者);`cargo test -p helix-term lsp`(单测)与 integration(控制者);clippy 零;fmt clean(并行批次 fmt 漂移不算,只保证本批次文件)。

---

### 任务 1:装饰二分(排序存储 + 可见范围 + 单测)

**文件:** typed.rs、view.rs、editor.rs

- [ ] **步骤 1:排序存储 + 二分辅助 + 单测(红)**

1. `typed.rs` apply_plugin_decorations:构建 PluginDecorations 后 sort(virtual_text 按 char_idx / highlights 按 start)
2. 二分辅助 `slice_range`(放 view.rs 或独立 util,按可测试性)
3. helix-term 单测(application.rs tests mod 或新):slice_range 正确性(空/边界/跨区间);排序后有序断言

预期:slice_range 单测 RED(函数不存在)→ 实现 → GREEN。

- [ ] **步骤 2:可见范围传入 + 二分裁剪**

1. `view.rs` text_annotations 加 `visible: (usize, usize)` 参数;插件段二分取子集 → 分组(既有)
2. `editor.rs`:计算可见 char 范围(见关键细节)传入 text_annotations;插件高亮段二分取子集 → 分组合并(既有)
3. 既有 plugin_decorations 集成测试不回归(控制者跑)

- [ ] **步骤 3:提交**

```bash
git add helix-term helix-view
git commit -m "perf(view,term): 装饰渲染按可见范围二分裁剪——O(log n + k)替代每帧全量;应用时排序存储"
```

---

### 任务 2:粘贴支持(批量插入 + Paste 分支 + integration)

**文件:** input.rs、plugin_popup.rs、plugin_panel.rs、integration 测试、plugin-api.md

- [ ] **步骤 1:批量插入 + 单测**

1. `input.rs`:`input_insert_batch` + `dispatch_input_paste`
2. helix-js 单测(光标处插入/光标推进/含 \n)

- [ ] **步骤 2:Paste 分支(popup + panel)+ 写失败 integration**

1. plugin_popup.rs / plugin_panel.rs handle_event 加 Paste 分支(见关键细节;drain 按各文件现状)
2. integration(新 plugin_paste.rs 或并入 plugin_panel_focus.rs):
   - multiline input 聚焦 → 手动 pump 发送 `Event::Paste("line1\nline2")` → 值含整段(白盒)+ onChange 一次
   - 未聚焦 → Paste → 编辑器正文粘贴(doc 变)
   - 单行 input 粘贴含 \n 行为

   **测试手动 pump + `tx.send(Ok(Event::Paste(...)))`**(仿 plugin_lsp_mock 手动 pump;Paste 是 crossterm Event,harness 需支持构造)。

- [ ] **步骤 3:转绿 + 文档 + 提交**

```bash
cargo build -p helix-term
cargo test -p helix-js
cargo test -p helix-term --features integration --test integration plugin_paste   # 控制者
cargo clippy --all-targets 2>&1 | tail -3
```

`docs/plugin-api.md` input 小节注明:聚焦 input 时终端粘贴(bracketed paste)直接插入(一次 onChange);未聚焦冒泡给编辑器。

```bash
git add helix-js helix-term docs/plugin-api.md
git commit -m "feat(js,term): 插件 input 支持 Paste 批量插入——聚焦时一次插入整段+一次 onChange;未聚焦冒泡编辑器"
```

---

## 自检记录

**规格覆盖度:**
- 2.1 装饰二分(排序存储/可见范围/二分)→ 任务 1 ✓
- 2.2 粘贴(批量插入/Paste 分支/单行 \n 一致)→ 任务 2 ✓
- 3.1 装饰单测 + 回归 → 任务 1 步骤 1、3 ✓
- 3.2 粘贴 3 测试 → 任务 2 步骤 2-3 ✓

**占位符扫描:** 无 TODO/待定;可见 char 范围计算标注"执行时按文本 API 最简实现,宁可多取不可少取"。✓

**类型一致性:** `visible` 参数、`slice_range` 签名、`input_insert_batch`/`dispatch_input_paste` 跨任务一致。✓
