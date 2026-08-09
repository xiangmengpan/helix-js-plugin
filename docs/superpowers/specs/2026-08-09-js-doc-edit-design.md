# 设计：helix JS 文档修改 API（A3）

日期：2026-08-09
状态：已批准

## 目标

让 JS 插件能修改当前文档：插入 / 替换 / 删除文本。API 基于命令 ctx 的 doc 对象，编辑队列化、命令结束原子应用为一个事务（一次撤销）。

## 新增 JS API

命令 ctx 的 `doc` 对象增加三个方法（均为队列操作，不立即生效）：

```js
ctx.doc.insert(row, col, text)              // 在 (row, col) 插入 text
ctx.doc.replace(sr, sc, er, ec, text)       // 用 text 替换 (sr,sc)..(er,ec) 区间
ctx.doc.delete(sr, sc, er, ec)              // 删除 (sr,sc)..(er,ec) 区间
```

- 坐标均为 0-based 行/列（与 ctx.cursor 一致）
- 命令返回后，所有排队编辑一次性应用为一个 Transaction（**一个命令 = 一个事务 = 一次撤销**）
- 位置基于命令开始时的原始文本快照；命令内多次编辑互不偏移
- row/col 越界：clamp 到文档边界（行超界→最后一行，列超界→行尾），不报错
- 参数类型错误（非数字/非字符串、缺参）：JS 报错，命令失败，队列清空
- 只作用于当前文档（ctx 所属文档）；不能指定其他缓冲区

## 实现

### helix-js

- 公开类型：`pub struct Edit { pub start: (usize, usize), pub end: (usize, usize), pub insert: String }`
- thread_local 编辑队列：`CURRENT_EDITS: RefCell<Vec<Edit>>`（`run_command` 开始时清空）
- 原生函数 `js_doc_insert` / `js_doc_replace` / `js_doc_delete`（挂在 ctx 的 doc 对象上）：类型校验 → clamp 坐标 → 入队
- `pub fn take_edits() -> Vec<Edit>` — 取走并清空（仿 take_messages）
- `ctx_to_js` 中 doc 对象挂这三个方法

### helix-term（typed.rs 分发钩子）

`run_command` 成功分支（drain messages / ui_requests 之后）追加：

1. `let edits = helix_js::take_edits();`
2. 非空时：`let (view, doc) = current!(cx.editor);`
3. row/col → char 索引（`text.line_to_char(row) + col`，clamp 到 `len_chars`）
4. 按 start 排序、重叠则报错（不应用）
5. `let txn = Transaction::change(doc.text(), changes);`
6. `doc.apply(&txn, view.id);`

- 游标：`doc.apply` 的 selection 重映射自动处理（光标处插入后游标落在插入文本之后）
- 编辑失败（重叠/事务构建错误）：`cx.editor.set_error`，不部分应用

## 测试

- **helix-js 单测**：insert/replace/delete 入队坐标正确；越界 clamp；类型错误报错；take_edits 清空
- **集成测试**（helix-term）：插件命令在光标处插入 → 断言文档文本变化；再次运行命令（同插件二次调用）验证追加；撤销（`u`）一步回滚到原始文本

## 非目标（YAGNI）

- 非当前缓冲区编辑、多光标语义、命令内编辑可见性（读到的仍是原始快照）
- 编辑结果回调、异步编辑
- 选择区设置 API（改游标/选区本身）

## 涉及文件

- `helix-js/src/lib.rs`（Edit 类型、三个原生函数、take_edits、ctx doc 方法、单测）
- `helix-term/src/commands/typed.rs`（分发钩子应用编辑）
- `helix-term/tests/test/plugin.rs`（集成测试）
