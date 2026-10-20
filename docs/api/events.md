# API:事件钩子

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

## `helix.on(event, fn)`

| 事件 | 触发时机 | 回调签名 |
|------|---------|---------|
| `save` | 保存**前**(编辑先应用再保存 → format-on-save) | `(doc)` |
| `mode-change` | 模式切换后 | `(mode, doc)`，mode: `"normal"\|"insert"\|"select"` |
| `cwd-change` | 工作目录**真的**变了(`:cd` / `:lcd` / `:pop-directory`，发在 chdir **之后**) | `(doc, payload)`，见下方「cwd-change」 |
| `buffer-open` | `:open` 打开文件后 | `(doc)` |
| `buffer-close` | 关闭(quit/force_quit 路径)前 | `(doc)` |
| `doc-change` | 文档文本变化,编辑停顿 250ms 后(idle 防抖);**逐文档**派发(后台 buffer 的变更也发) | `(doc)` |
| `theme-change` | 主题变化后 | `(doc)`(同 buffer-open 签名) |
| `lsp-diagnostics` | 诊断更新 | `(docId, diags)` |
| `cursor-move` | 光标移动(帧级节流) | `(docId, {row, col, mode})` |
| `selection-change` | 选区变化(帧级节流) | `(docId, {count, primary})` |
| `component-event` | 组件鼠标事件 | `(id, {kind, x, y})`(返回 true 消费) |
| `term-open/term-mode-change/term-exit/term-close/term-resize/term-title/term-key` | 终端系 | `(ptyId, ...)`(term-key 返回 normal/pass/consume/minimize/close) |
| `layout-change` | 布局变更后(分屏/关闭/浮窗/堆叠/raise/pin/embed;唯一汇聚点是 `Compositor::sync_layout_cache`) | `(kind, doc)`,`kind` 缺省时为 **`null`**(形状**恒定**,见文末) |
| `startup` | 初始文件处理之后(启动完成) | `(doc, payload)`,见下方「startup 的 payload」 |

### `cwd-change`：别在加载时缓存 cwd

```js
helix.on("cwd-change", (doc, ev) => {
  // ev.cwd      新目录（绝对路径）
  // ev.prev_cwd 旧目录；未知时为 null
  // 想跟随的用户态在这里重取：
  tree_root = ev.cwd;          // 别用加载时缓存的 helix.cwd()
});
```

- 事件发在 **chdir 之后** ⇒ 处理器里 `helix.cwd()` 已经返回新目录，两者应当相等。
- 只在**真的换成功**时发；`:cd` 到不存在的目录 → 报错、不发事件。
- 面板没开时也可以监听：这是插件刷新"自己缓存的 cwd"的唯一时机（核心里 cwd 一直是现读的）。

### `startup` 的 payload

```js
helix.on("startup", (doc, ev) => {
  // ev.open_paths = 命令行让打开的东西数量(文件/目录参数 + --tutor + 管道 stdin)
  if (doc.path) return;        // 打开了文件
  if (ev.open_paths > 0) return;  // 指向目录(会弹选择器)/ --tutor / 管道内容
  open_welcome_screen();       // 只有这里才是真正的"空启动"
});
```

**为什么不用 `doc.path` 一处判断**(两次实测 bug 的结论):下面三种情况当前 buffer **没有路径**,
但用户**已经指定了要打开什么** —— 只看 `doc.path` 会误判成空启动:

| 启动方式 | 当前 buffer | `open_paths` |
|---|---|---|
| `hx`(无参) | 无名空 buffer | **0** |
| `hx note.txt` | 有路径 | 1 |
| `hx <目录>` | 无名空 buffer + **文件选择器浮层** | 1 |
| `hx --tutor` | tutor 内容(刻意 `set_path(None)`) | 1 |
| `cat x \| hx` | 管道内容(无路径) | 1 |

`doc-change` 的 `doc` 额外带 `changes`(防抖窗口内变更的**合并范围**,无变更时 `[]`)：

```js
helix.on("doc-change", (doc) => {
  doc.type     // "doc-change"
  doc.path     // 被改的文档（后台 buffer 也在内）
  doc.doc_id   // 逐文档派发时靠它分辨（同一轮 idle 可能来多条）
  doc.changes  // [{ oldRange: {start:{row,col}, end:{row,col}},
                //    newRange: {start:{row,col}, end:{row,col}} }]
});
```

**逐文档**：所有有变更的已打开文档各发一条（当前文档**最后**发）。后台文档的 `doc` 是它自己的
快照（`path`/`text`/`changes` 都是它的），但**没有光标** ⇒ `cursor` 恒为 `{row:0,col:0}`、
`selection` 为零长。想据此刷新缓存的监听方只该看 `path`/`doc_id`/`changes`。

**首次见到某文档不发事件**（只登记 revision）：否则启动后第一轮 idle 会给每个打开的文件发一条。

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
- 局限：除 `doc-change` 外的事件只带**当前**文档(其他分屏不触发);doc 是只读快照 + 编辑队列;`save` 前触发语义需注意(编辑在保存前应用);doc-change 250ms 防抖(快速连续输入会合并)。

### `layout-change`:形状**恒定** `(kind, doc)`

`kind` 是种类字符串,或本次变更没有种类时的 **`null`**;`doc` **恒在第二参**。

```js
helix.on("layout-change", (kind, doc) => {
  helix.echo(`layout: ${kind ?? "(无种类)"}`);
});
```

- **种类取值**(`helix-term/src/commands/typed.rs` 里 10 处 `set_layout_change_kind`):
  `split` / `close` / `float` / `embed` / `raise` / `pin` / `stack-create` / `stack-activate` / `stack-remove`。
- **`kind === null` 是常见情形,不是 bug**:`sync_layout_cache()` 有 37 处调用点,设种类的只有 10 处（实测 `grep`）
  ⇒ 多数路径不带种类。
- **历史坑(2026-10-13 已修)**:此前缺种类时只传 `[doc]`(没有占位参数)⇒ 写 `(kind) => …` 的插件会
  **静默**把事件 doc 对象当成 kind;而 `(doc) => …` 又会在带种类的路径上把 kind 当 doc。现在两条
  路径形状一致 ⇒ 判 `kind === null` 即可。回归测试:`helix-js/src/lib.rs` 的
  `layout_change_args_are_kind_or_null_then_doc`。
- **触发时机**:唯一汇聚点是 `Compositor::sync_layout_cache`,布局真的变了才发。
- **排障**:该事件的处理器抛错只见 `log::warn`(`compositor.rs`),**不**走状态栏;用 `:log-open` 看日志。
