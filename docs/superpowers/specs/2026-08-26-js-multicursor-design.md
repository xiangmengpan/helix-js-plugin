# 设计:多光标/多选区 set_selection 数组

日期:2026-08-26
状态:草案(待审核)

## 1. 动机

`helix.set_selection(ar, ac, hr, hc)` 只支持单选区。批量编辑/多光标插件(重构、批量改名、多行操作)需要一次设置多个选区。目标:set_selection 扩展接受数组,向后兼容现有 4 参形态。

## 2. API

```js
set_selection(ar, ac, hr, hc)                     // 兼容现状:单选区
set_selection([{ anchor: {row, col}, head: {row, col} }, ...])  // 多选区
```

- 数组形态:每个元素 `{ anchor, head }`,坐标 0-based 行列(与 doc.cursor 一致)
- 空数组 → JS 报错(`set_selection: empty selection array`)
- 元素缺字段/类型错误 → JS 报错
- 多选区按 anchor 排序(引擎处理,helix SelectionSet 要求有序)

## 3. 实现

### 3.1 helix-js(types.rs + commands.rs)

`CursorRequest` 加变体:

```rust
pub enum CursorRequest {
    SetCursor { row: usize, col: usize },
    SetSelection { anchor: (usize, usize), head: (usize, usize) },
    SetSelections(Vec<((usize, usize), (usize, usize))>),  // 新增
}
```

`js_set_selection` 分支:
- 第 1 参是数组 → 解析为 `SetSelections`
- 否则走现有 4 参逻辑(SetSelection)

### 3.2 helix-term(commands/typed.rs apply_cursor_requests)

现有逻辑逐条 `doc.set_selection`(覆盖语义)。多选区需要一次构造 `SelectionSet`:

- `SetSelections` → 解析每个 (anchor, head) 为 `Selection`,排序(按 anchor char 位置)后构造 `SelectionSet`
- `doc.set_selection(view.id, SelectionSet)`(helix-view Document::set_selection 接受 SelectionSet——核对签名)
- 与 SetCursor/SetSelection 混合:requests 里既有单又有多 → 按序应用(后者覆盖;文档注明混用语义 = 顺序应用)

### 3.3 主光标

SelectionSet 的 primary(主光标)= 排序后最后一个(helix 语义,与原生多光标一致)。`set_cursor` 仍设主光标。

## 4. 边界

- 排序/去重:anchor 排序;重叠选区不合并(helix SelectionSet 允许?——若不允许需去重,实现时核对 helix-core SelectionSet 构造约束)
- 混用:单/多选区 request 顺序应用,后者覆盖
- 数量:无上限(与 helix 原生多光标一致)

## 5. 验证

- **helix-js 单测**:数组解析(多选区/空数组报错/缺字段报错)、4 参兼容
- **integration 测试**(plugin_selection 或扩展):set_selection 数组 → 断言选区数/光标数;排序正确
- 测试环境默认禁 LSP 不影响

## 6. 规模

- helix-js types.rs/commands.rs + helix-term apply_cursor_requests,约 1 任务。
