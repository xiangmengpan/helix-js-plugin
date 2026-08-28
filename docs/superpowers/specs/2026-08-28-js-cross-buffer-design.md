# 设计:跨 buffer 访问(by_path 读文本/批量改)

日期:2026-08-28
状态:已批准(brainstorming 三轮问答 + 三节设计确认)

关联:docs/superpowers/specs/2026-08-26-js-batch-edit-design.md(批量事务)、docs/superpowers/specs/2026-08-15-buffer-traversal-design.md(未执行的 buffers() 列表,与本设计正交)

## 1. 动机

插件只能操作**当前 buffer**(命令 ctx 的 doc;`doc_*` 编辑均隐式指向当前文档)。缺:
- 读取其它已打开 buffer 的文本(跨文件分析、对照查看)
- 一个命令批量修改多个 buffer(重构类插件:改 A 时同步改 B/C)

原始需求(2026-08-26-p0-complete.md 下一步候选 #5):「跨 buffer 访问:by_path 读文本/批量改」。

## 2. 设计

### 2.1 API

```js
// 按路径查已打开 buffer;未打开 → null
const other = helix.by_path("src/main.rs");
// → { path: "/abs/src/main.rs", text: "...", cursor: { row: 0, col: 0 } }
//   + insert(row, col, str) / replace(sr, sc, er, ec, str) / delete(sr, sc, er, ec)

// 改其它 buffer:与 ctx.doc 方法同构,编辑入队时目标记为该 path
other.insert(0, 0, "// header\n");
```

- 返回对象与 `ctx.doc` 同构(复用 `doc_to_js` 构造模式,取值改自快照)
- 参数非字符串 → TypeError(与 `open_file(42)` 报错一致)
- `cursor` 恒为 `{row: 0, col: 0}`:后台 buffer 无 view selection(helix-view `Document.selections` 是 `HashMap<ViewId, Selection>`),如实反映"无光标"
- 路径匹配:参数经 `helix_stdx::path::canonicalize` 规范化(与打开文档时同一函数,见 helix-view document.rs:1021/1347:展开 `~`、相对路径 join cwd、词法去 `./`/`..`/`//`),与快照 path(打开时已用同一函数规范化)精确比较——`by_path("src/main.rs")` 能对上 `/abs/cwd/src/main.rs`。helix-js 需新增 helix-stdx workspace 依赖(零新外部 crate)

### 2.2 数据流(方案 B:CommandContext 携带快照)

1. `CommandContext`(helix-js/src/types.rs)加字段 `docs: Vec<DocSnapshot>`;`DocSnapshot { path: String, text: String }`
2. 命令入口 `run_plugin_command`(helix-term typed.rs:4348):遍历 `editor.documents`,收集所有 `path().is_some()` 的文档 → `docs`
3. 事件入口 `emit_plugin_event_impl`:同样提供
4. panel/popup 渲染入口(plugin_panel.rs:106 / plugin_popup.rs:155):显式 `docs: vec![]`——每帧调用,不序列化全文(沿用 StatuslineCtx 的取舍:轻量 ctx 不含 doc.text)
5. helix-js `js_by_path`(commands.rs):在 ctx.docs 里 normalize 匹配 → 构造 doc 对象;其方法推 `Edit { doc: Some(path), .. }`
6. lib.rs 测试构造器 ~15 处加 `docs: vec![]`(编译器兜底)

成本:命令/事件入口每次调用序列化全部打开 buffer 文本。命令是用户触发(非每帧),文档数通常 < 50;若未来大 workspace + 高频场景,可改懒加载。`ponytail: 全量序列化快照,大 workspace 每命令开销见 3.4;需要时改按需懒取`

### 2.3 编辑路由

`Edit`(helix-js/src/types.rs)加目标字段:

```rust
pub struct Edit {
    pub doc: Option<String>,  // Some(path) = 目标其它 buffer;None = 当前 buffer(现状)
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub insert: String,
}
```

- `js_doc_insert/replace/delete`(commands.rs 三处)推 `doc: None`——当前 buffer 行为不变
- `by_path` 返回对象的三个方法推 `doc: Some(path)`

`apply_plugin_edits`(helix-term typed.rs:4530)按 doc 分组:

```rust
// 现状:全部 edits → 当前 doc 一个 Transaction
// 改为:按 edit.doc 分组 → 每组:
//   pos_to_char 用该 doc 自己的 text
//   排序 + 重叠检查
//   该 doc 一个 Transaction, doc.apply
// 当前 doc 组(None)走原有逻辑;其它组按 path 查 editor.documents
//   查不到(命令期间被关闭)→ bail → "plugin edit failed" 状态栏报错(不崩)
```

- 一个命令改 N 个 buffer = N 个 Transaction = 每个 buffer 一次撤销(Q4-A;helix undo 本身 per-document)
- `begin_edit/end_edit` 语义不变:仍只控制 take_edits 出队时机,出队后按 doc 分组应用

### 2.4 边界

- 只操作**已打开** buffer:`by_path` 未命中 → `null`;编辑目标在命令期间被关闭 → 应用时报错入状态栏(不崩)
- **快照语义**:`by_path` 的 text 是命令开始时的快照(与 `ctx.text` 一致);编辑坐标基于该快照,应用时用 doc 当时文本——异步命令跨 await 的过期坐标风险与现状相同,不新增
- scratch(无 path)buffer 不在 `docs` 里:无路径可查,天然排除
- 只读 buffer:不新增校验(现状 doc 编辑也不查),保持一致性
- 不拉入 2026-08-15 buffers() 设计(列表/focus)——本设计只做 by_path 读写,`buffers()` 若需要另起

## 3. 验证

### 3.1 helix-js 单测

- `by_path` 参数非字符串报错
- 未命中返回 null
- 命中返回 path/text
- 返回对象的方法推 `Edit.doc == Some(path)`;当前 doc 方法仍推 None

### 3.2 integration 测试(新 plugin_cross_buffer 或并入现有)

1. 命令里 by_path 读另一 buffer 文本并断言
2. 命令里改另一 buffer → 该 buffer 一次 undo 回退(每 doc 一事务)
3. 一个命令混合改当前 + 另一 buffer → 各自撤销独立
4. by_path 未打开路径 → null,命令不崩
5. async 命令 begin_edit → by_path 改另一 buffer → end_edit → 一次应用

## 4. 规模

- helix-js:types.rs(Edit/CommandContext/DocSnapshot)、commands.rs(js_by_path + 三方法推 doc)、lib.rs(注册 by_path + 测试构造器)
- helix-term:typed.rs(run_plugin_command/emit_plugin_event_impl 填 docs;apply_plugin_edits 分组)、plugin_panel.rs/plugin_popup.rs(docs: vec![])
- 测试:helix-js 单测 + integration
- 约 2 任务:① 快照与读取(types/commands/lib + 单测)② 编辑路由(Edit.doc + apply_plugin_edits 分组 + integration)
