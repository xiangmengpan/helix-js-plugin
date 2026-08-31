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
