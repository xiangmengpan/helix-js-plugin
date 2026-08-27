# 多光标/多选区 set_selection 数组实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** `helix.set_selection` 扩展接受数组(多选区),兼容现有 4 参单选区形态。helix-core 的 `Selection` 本身即多选区(`ranges` + `primary_index`),直接构造 `Selection::new`。

**架构：** `CursorRequest` 加 `SetSelections(Vec<((usize,usize),(usize,usize))>)` 变体;`js_set_selection` 第一参为数组时解析为多选区;`apply_cursor_requests` 构造 `Selection::new(smallvec![Range...], primary_index)` → `doc.set_selection`(normalize 自动排序,primary = 末位)。

**技术栈：** boa 0.21、helix-core Selection/Range(smallvec)、helix-view Document::set_selection。

**规格：** `docs/superpowers/specs/2026-08-26-js-multicursor-design.md`(已批准)

---

### 任务 1：helix-js + helix-term — SetSelections 变体与解析/应用

**文件：**
- 修改：`helix-js/src/types.rs`(`CursorRequest` 281 行加变体)
- 修改：`helix-js/src/commands.rs`(`js_set_selection` 734 行数组分支)
- 修改：`helix-term/src/commands/typed.rs`(`apply_cursor_requests` 4477 行处理新变体)

- [ ] **步骤 1：编写失败的测试**

helix-js(commands.rs 测试或 lib.rs,仿现有):
```rust
#[test]
fn set_selection_array() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    // 数组形态 → SetSelections 入队
    load_script(
        r#"
        helix.set_selection([{ anchor: {row: 0, col: 0}, head: {row: 0, col: 2} },
                             { anchor: {row: 2, col: 1}, head: {row: 2, col: 4} }]);
        "#,
    )
    .unwrap();
    let reqs = take_cursor_requests();
    assert!(matches!(&reqs[0], CursorRequest::SetSelections(v) if v.len() == 2));
    // 空数组 → JS 报错
    assert!(load_script(r#"helix.set_selection([]);"#).is_err());
    // 缺字段 → JS 报错
    assert!(load_script(r#"helix.set_selection([{ anchor: {row:0,col:0} }]);"#).is_err());
    // 4 参兼容
    load_script(r#"helix.set_selection(0, 0, 1, 1);"#).unwrap();
    let reqs = take_cursor_requests();
    assert!(matches!(&reqs[0], CursorRequest::SetSelection { .. }));
}
```

integration(helix-term/tests/test/,新建或扩展):set_selection 数组 → 断言 doc 选区数 = 2、光标数 = 2、排序正确(乱序输入 → 引擎排序)。

- [ ] **步骤 2：运行确认失败**

运行：`cargo test -p helix-js --lib set_selection_array` + integration
预期：FAIL(编译错误:变体不存在 / JS 解析未实现)。

- [ ] **步骤 3：实现**

`types.rs` CursorRequest(281 行):
```rust
pub enum CursorRequest {
    SetCursor { row: usize, col: usize },
    SetSelection { anchor: (usize, usize), head: (usize, usize) },
    SetSelections(Vec<((usize, usize), (usize, usize))>),
}
```

`commands.rs` `js_set_selection`:第一参是数组 → 解析:
```rust
// 仿 obj_opt_str 的 boa 读取模式:数组迭代,每元素 { anchor: {row, col}, head: {row, col} }
// 元素缺字段/类型错 → Err;空数组 → Err("set_selection: empty selection array")
// 解析完 push CursorRequest::SetSelections(...)
// 非数组 → 现有 4 参逻辑
```

`typed.rs` `apply_cursor_requests`:
```rust
helix_js::CursorRequest::SetSelections(sel) => {
    use helix_core::{Range, Selection};
    let mut ranges: SmallVec<[Range; 1]> = sel.iter().map(|(a, h)| {
        Range::new(pos_to_char(text, a.0, a.1), pos_to_char(text, h.0, h.1))
    }).collect();
    // 排序(normalize 也会做,但 primary_index 要基于排序后位置)
    // 简单:排序 ranges 后 primary = len-1,构造 Selection::new(ranges, len-1)
    Selection::new(ranges, ranges.len() - 1)
}
// 现有 SetCursor/SetSelection 分支保持
```

- [ ] **步骤 4：运行确认通过**

运行：`cargo test -p helix-js --lib` + `cargo test -p helix-term --features integration --test integration plugin_selection`(或新测试名)
预期：全 PASS;`cargo clippy -p helix-js -p helix-term --all-targets` 零警告。

- [ ] **步骤 5：Commit**

```bash
git add helix-js/src/types.rs helix-js/src/commands.rs helix-term/src/commands/typed.rs helix-term/tests/test/plugin_selection.rs helix-term/tests/integration.rs
git commit -m "feat: set_selection 数组形态多选区(Selection::new, 兼容 4 参)"
```
