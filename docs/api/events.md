# API:事件钩子

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

## `helix.on(event, fn)`

| 事件 | 触发时机 | 回调签名 |
|------|---------|---------|
| `save` | 保存**前**(编辑先应用再保存 → format-on-save) | `(doc)` |
| `mode-change` | 模式切换后 | `(mode, doc)`，mode: `"normal"\|"insert"\|"select"` |
| `buffer-open` | `:open` 打开文件后 | `(doc)` |
| `buffer-close` | 关闭(quit/force_quit 路径)前 | `(doc)` |
| `doc-change` | 文档文本变化,编辑停顿 250ms 后(idle 防抖) | `(doc)` |
| `theme-change` | 主题变化后 | `(doc)`(同 buffer-open 签名) |
| `lsp-diagnostics` | 诊断更新 | `(docId, diags)` |
| `cursor-move` | 光标移动(帧级节流) | `(docId, {row, col, mode})` |
| `selection-change` | 选区变化(帧级节流) | `(docId, {count, primary})` |
| `component-event` | 组件鼠标事件 | `(id, {kind, x, y})`(返回 true 消费) |
| `term-open/term-mode-change/term-exit/term-close/term-resize/term-title/term-key` | 终端系 | `(ptyId, ...)`(term-key 返回 normal/pass/consume/minimize/close) |
| `layout-change` | 布局变更后(分屏/关闭/浮窗/堆叠/raise/pin/embed;唯一汇聚点是 `Compositor::sync_layout_cache`) | `(kind, doc)` **或** `(doc)` —— **形状会变,见文末** |

`doc-change` 的 `doc` 额外带 `changes`(防抖窗口内变更的**合并范围**,无变更时 `[]`)：

```js
helix.on("doc-change", (doc) => {
  doc.changes  // [{ oldRange: {start:{row,col}, end:{row,col}},
                //    newRange: {start:{row,col}, end:{row,col}} }]
});
```

- 合并语义:窗口内多次变更合并为包围范围(old = 各 old 起点最小..终点最大,new 同理);单次变更即其本身。
- 坐标是各变更发生时刻的坐标(非当前文本),窗口语义由插件自行处理。
- `doc` 与命令的 `ctx.doc` 同构(含 cursor 与编辑方法)。
- 同事件可注册多个处理器,按注册顺序调用;处理器抛错 → 状态栏报错,不阻断主流程。
- `save` 处理器编辑 = 保存前变更(一次撤销)。

```js
// format-on-save：保存时清理行尾空白
helix.on("save", (doc) => {
  const lines = doc.text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const t = lines[i].replace(/[ \t]+$/, "");
    if (t !== lines[i]) doc.replace(i, t.length, i, lines[i].length, "");
  }
});
```

**优缺点**
- 优点：事件丰富(文档/模式/终端/光标/诊断);多处理器按序;抛错不阻断;doc-change 带合并范围(自动格式化/补全联动基础)。
- 局限：事件只带当前文档(其他分屏不触发);doc 是只读快照 + 编辑队列;`save` 前触发语义需注意(编辑在保存前应用);doc-change 250ms 防抖(快速连续输入会合并)。

### `layout-change`：**回调参数形状会变**(坑)

它与其它事件不同:**同一个事件名有两种回调形态**,取决于"本次变更有没有种类"。

| 本次变更 | 回调实参 |
|---|---|
| 带种类 | `(kind, doc)`,`kind` 是字符串,取值:`split` / `close` / `float` / `embed` / `raise` / `pin` / `stack-create` / `stack-activate` / `stack-remove` |
| 不带种类 | `(doc)` —— **没有占位参数**,第一个参数就是 doc 对象 |

⇒ **不要写** `helix.on("layout-change", (kind) => …)`:在无种类的路径下 `kind` 拿到的是**事件 doc 对象**,
不会报错,只会静默出错。必须自己判类型:

```js
helix.on("layout-change", (a, b) => {
  const kind = typeof a === "string" ? a : null;   // 无种类 ⇒ null
  const doc  = typeof a === "string" ? b : a;
  helix.echo(`layout: ${kind ?? "(无种类)"}`);
});
```

- **为什么形状会变**:`emit_event_impl` 有 `extra` 时传 `[extra, doc]`,没有时只传 `[doc]`(`helix-js/src/commands.rs:1142-1145`)。
- **无种类是常见情形**(实测 2026-10-10):`helix-term/src` 里 `sync_layout_cache()` 有 **37 处**调用点,
  设种类的只有 **10 处** ⇒ 多数路径发出的 `layout-change` 不带 `kind`。收到"无种类"是预期行为,不是 bug。
- **触发时机**:唯一汇聚点是 `Compositor::sync_layout_cache`(`helix-term/src/compositor.rs:1427`),布局真的变了才发。
- **排障**:该事件的处理器抛错只见 `log::warn`(`compositor.rs:1427`),**不**走状态栏;用 `:log-open` 看日志。
