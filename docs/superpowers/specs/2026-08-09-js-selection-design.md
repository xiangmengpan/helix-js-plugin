# 设计：选区/光标 API（v5）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

插件可读当前主选区、可设置光标/选区，解锁"选中文本转换"类插件。

## 新增 JS API

```js
// 读（命令 ctx，与 cursor 并列）
ctx.selection.anchor        // { row, col }
ctx.selection.head          // { row, col }；点光标时 anchor == head == ctx.cursor

// 写（队列化，命令返回后应用）
helix.set_cursor(row, col);                     // 移动主光标
helix.set_selection(anchorRow, anchorCol, headRow, headCol);
```

- 坐标 0-based 行列，与既有 API 一致；越界在 helix-term clamp（同 apply_plugin_edits 的 to_char）
- 参数类型错误/缺参 → JS 报错
- 应用顺序：**光标/选区请求先应用，再应用编辑事务**——edits 的 selection 重映射把光标正确地推进插入文本之后（快照坐标语义）
- 只操作主光标/主选区；多光标不在范围
- 光标移动不进入撤销历史（helix 标准行为，非事务）

## 实现

### helix-js

- `CommandContext` 增加：`pub selection: ((usize, usize), (usize, usize))`（anchor, head 行列对）
- 新类型：`pub enum CursorRequest { SetCursor { row: usize, col: usize }, SetSelection { anchor: (usize, usize), head: (usize, usize) } }`
- thread_local：`CURSOR_REQUESTS: RefCell<Vec<CursorRequest>>`（run_command 开始时清空，仿 CURRENT_EDITS）
- 原生函数：`js_set_cursor`（2 参）、`js_set_selection`（4 参）——数字校验入队
- 公共函数：`pub fn take_cursor_requests() -> Vec<CursorRequest>`
- `ctx_to_js`：ctx 对象加 `selection: { anchor: { row, col }, head: { row, col } }`

### helix-term

- 两个 ctx 构造点（命令分发钩子、emit_plugin_event）补 selection：`doc.selection(view.id).primary()` → `range.anchor`/`range.head`（char）→ to_char 反转为行列
- 新辅助 `apply_cursor_requests(editor, reqs)`：drain 后在编辑应用**之前**执行——SetCursor → `Selection::point(char)`；SetSelection → `Selection::single(anchor, head)`；坐标经 to_char clamp（复用/提取）
- 调用点：命令分发 Ok(true) 分支 + emit_plugin_event（编辑应用前）

## 测试

- **helix-js 单测**：set_cursor/set_selection 入队坐标正确；类型错误（字符串/缺参）报错；take_cursor_requests 清空；ctx.selection 对象形状（anchor/head 行列）
- **集成测试**（tests/test/plugin_selection.rs）：`:select` 演示命令——读 ctx.selection，选中的文本 `toUpperCase` 替换，`set_selection` 保持选区 → 断言文本与选区；`set_cursor` 移动光标 → 断言光标位置

## 非目标

- 多光标（只主选区）、选区拖拽交互、光标动画
- 事件 ctx 的 selection（只有命令 ctx 有；事件 doc 无——YAGNI）
- 选区读取的逐字符偏移 API

## 涉及文件

- `helix-js/src/lib.rs`（CommandContext.selection、CursorRequest、两个原生函数、take_cursor_requests、ctx_to_js、单测）
- `helix-term/src/commands/typed.rs`（两个 ctx 构造点补 selection、apply_cursor_requests、drain 接线）
- `helix-term/tests/test/plugin_selection.rs`（新）
