# API:文档编辑与选区

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

命令的 `ctx` 是**只读快照 + 编辑队列**：

```js
ctx.path                  // string | null，文件路径
ctx.text                  // string，全文
ctx.cursor                // { row, col }，主光标位置（0-based）
ctx.selection             // { anchor: {row,col}, head: {row,col} }
ctx.doc                   // 文档对象（含编辑方法，见下）
ctx.doc.insert(row, col, text)              // 插入
ctx.doc.replace(sr, sc, er, ec, text)       // 替换区间
ctx.doc.delete(sr, sc, er, ec)              // 删除区间
```

- 坐标为 0-based 行列，基于命令开始时的**原始快照**（多次编辑互不偏移）。
- 越界自动 clamp；一次命令的所有编辑 = 一个事务 = 一次撤销。
- 编辑后游标被自动重映射（在插入处插入后，游标落在插入文本之后）。

## `helix.begin_edit()` / `helix.end_edit()`(批量编辑事务)

```js
helix.begin_edit();
await something();          // async 命令跨 await:泵循环每帧取编辑会被拆事务
ctx.doc.insert(0, 0, "a");
helix.end_edit();           // 期间所有编辑合并为一个事务 = 一次撤销
```

- 可嵌套(深度计数)；`end_edit` 深度归零时才放行编辑应用。
- **未配对 begin**(begin 后无 end):下个命令/事件入口复位深度并丢弃积压编辑(不崩)。
- 同步命令本身已是一个事务,本 API 主要用于 async 跨 await 场景。

**优缺点**：优点：async 命令跨 await 保持一次撤销。局限：必须成对调用；未配对会丢弃编辑。

## `helix.by_path(path)`(跨 buffer 访问)

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
- 目标 buffer 在命令期间被关闭 → 编辑应用时报错入状态栏(不崩)
- 同一命令内同时用 `ctx.doc` 与 `by_path(当前文件路径)` 改同一 buffer → 视为两个目标,撤销两次

**优缺点**：优点：跨 buffer 批量编辑(重构类插件)。局限：只查已打开 buffer(不自动打开)；快照坐标跨 await 会过期。

## `helix.set_virtual_text(path, row, col, text, style)` / `helix.set_highlight(path, sr, sc, er, ec, style)`(装饰/标记)

给已打开 buffer 加装饰(不修改文本;命令/事件期间累积,应用时**整体替换**该 buffer 的插件装饰):

```js
helix.set_virtual_text("src/main.rs", 1, 4, " // TODO", "ui.help");
helix.set_highlight("src/main.rs", 0, 0, 5, 0, "ui.selection");   // [start, end) 半开区间
helix.set_virtual_text("src/main.rs");                            // 清除该 buffer 全部装饰
```

- `style`: 主题 scope 字符串或 null(解析失败/省略 = 不渲染)
- 路径规则与 `by_path` 一致(规范化匹配,未打开静默忽略);坐标在命令结束时按当时文本换算,不随文档变更重映射——监听 `doc-change` 重推
- 装饰按 buffer 存储,所有窗口共享;buffer 关闭或脚本重载时清空

**优缺点**：优点：不污染文本的标记层;整体替换 + 热重载清理。局限：坐标不随文本变更重映射(需 doc-change 重推);只读层。

## `helix.set_cursor(row, col)` / `helix.set_selection(ar, ac, hr, hc)` / `helix.set_selection([...])`

```js
helix.set_cursor(3, 10);                          // 移动主光标
helix.set_selection(1, 0, 3, 5);                  // 设置单选区
helix.set_selection([                             // 多选区(数组形态)
  { anchor: { row: 0, col: 0 }, head: { row: 0, col: 2 } },
  { anchor: { row: 2, col: 1 }, head: { row: 2, col: 4 } },
]);
```

- 多选区按 range 起点排序,主光标 = 排序后末位(与 helix 原生多光标一致)；空数组/元素缺字段报错。
- 写操作队列化,命令返回后应用；光标请求在编辑事务之前应用。

```js
helix.register_command("upper", (ctx) => {
  const a = ctx.selection.anchor, h = ctx.selection.head;
  if (a.row !== h.row) { helix.echo("single-line only"); return; }
  const line = ctx.text.split("\n")[a.row];
  const lo = Math.min(a.col, h.col), hi = Math.max(a.col, h.col);
  ctx.doc.replace(a.row, lo, a.row, hi, line.slice(lo, hi).toUpperCase());
});
```

**优缺点**：优点：多选区数组形态,与原生多光标一致。局限：无多光标编辑能力(API 只操作主光标/选区,见 plugin-api §19)。
