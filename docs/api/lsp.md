# API:LSP

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

插件可主动向当前 buffer 的语言服务器发请求。八个方法都挂在 `helix.lsp` 命名空间下,全部返回 Promise;响应为 LSP 协议原始 JSON(字段名与 LSP 协议一致)。请求在**调用时**快照当前光标位置,之后移动光标不影响已发出的请求。

```js
const hover = await helix.lsp.hover();            // → Hover | null
const items = await helix.lsp.completion();       // → CompletionItem[] | {isIncomplete, items} | null
const locs  = await helix.lsp.goto_definition();  // → Location | Location[] | LocationLink[] | null
const syms  = await helix.lsp.document_symbols(); // → DocumentSymbol[] | null

// 位置覆盖(可选):字符坐标 (row, col);缺省 = 当前光标快照;row/col 须同传
await helix.lsp.hover({ row: 5, col: 3 });
```

| 方法 | LSP 请求 | 返回 |
|------|----------|------|
| `helix.lsp.hover()` | `textDocument/hover` | `Hover \| null` |
| `helix.lsp.completion()` | `textDocument/completion` | `CompletionItem[] \| {isIncomplete, items} \| null` |
| `helix.lsp.goto_definition()` | `textDocument/definition` | `Location \| Location[] \| LocationLink[] \| null` |
| `helix.lsp.document_symbols()` | `textDocument/documentSymbol` | `DocumentSymbol[] \| null` |

**唯一便利字段**:`goto_definition` 返回的每项附 `path`(由 `uri`/`targetUri` 解析,剥 `file://` + 百分号解码)。跳转直接用 `path`。

**标量形态**:LSP server 可返回**单条** `Location`(非数组),JS 收到裸对象——遍历前先 `Array.isArray` 归一。

### LSP 编辑操作

```js
await helix.lsp.format();                        // → { applied: true } 或 null
await helix.lsp.rename("newName");               // → { applied: true, files: n } 或 null
const actions = await helix.lsp.code_actions();  // → [{ title, kind, ...完整 LSP action }] 或 null
await helix.lsp.execute_code_action(actions[0]); // → { applied: true } 或 null
```

- 自动应用编辑(一次撤销/文件);rename 可跨 buffer(自动打开未打开文件);code_actions 两阶段无状态。
- code_actions 列表项带内部字段 `_serverId`(execute 按它路由);手动构造无此字段 → 用当前 buffer 第一个 CodeAction server。
- 无 server/能力不支持/请求失败/协议错误 → resolve null;超时 = 语言服务器 `timeout` 配置(默认 20s)到时 reject `Error`——promise 不悬挂。
- format 不校验文档版本(插件用 await 时序控制);rename 校验版本(过期 → null);format 仅全文档。

### 空/错语义

| 情形 | 行为 |
|------|------|
| 无 LSP server / 不支持该功能 | `resolve(null)` |
| server 返回 LSP 协议 error / 连接断开 | `reject(Error(message))` |
| 请求挂起不返回 | 超时(默认 20s,per-server 可配)→ `reject(Error("Timeout..."))` |

“没结果”是插件常态,只 resolve `null` 不抛错;只有真正出错的调用才 reject。

### 示例:hover 弹窗 + 跳转定义

完整 demo 见 `plugins/features/lsp-hover/index.js`:

```js
helix.register_command("lsp-hover", async () => {
  const hover = await helix.lsp.hover();
  if (!hover) return helix.echo("no hover");
  const content = hover.contents;
  // contents 可能是字符串 / {value} / {kind,value} / 数组
  const text = Array.isArray(content)
    ? content.map((c) => (typeof c === "string" ? c : c.value ?? "")).join("\n")
    : (typeof content === "string" ? content : content.value ?? "");
  helix.open_popup({ width: 60, height: 10, render: () => text.split("\n") });
});
```

**优缺点**
- 优点：查询/编辑全链路(hover/completion/定义/符号/format/rename/code_actions);自动应用编辑;失败 resolve null 不悬挂;超时拒绝防悬挂;`path` 便利字段免 URI 解析。
- 局限：execute_code_action 的 block_on 冻结主线程(与内置菜单同款);format 不校验文档版本(需插件自行控制时序);code_actions 多 server 路由依赖 `_serverId`(手动构造的 action 可能发到默认 server)。
