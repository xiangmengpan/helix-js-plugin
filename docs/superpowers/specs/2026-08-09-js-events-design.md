# 设计：helix JS 事件钩子（v3）

日期：2026-08-09
状态：已批准

## 目标

让 JS 插件订阅编辑器事件：保存前、模式切换、缓冲区打开/关闭。事件处理器接收与命令 ctx.doc 同构的文档对象（可编辑，复用编辑队列机制），使"保存时自动修改文档"（format-on-save）成为可能。

## 新增 JS API

```js
helix.on("save", (doc) => { ... });          // 保存前触发
helix.on("mode-change", (mode, doc) => { ... });  // mode: "normal" | "insert" | "select"
helix.on("buffer-open", (doc) => { ... });
helix.on("buffer-close", (doc) => { ... });
```

- `doc` 参数：与命令 ctx.doc 同构的对象（path/text/cursor 只读 + insert/replace/delete 编辑队列）
- `helix.on` 校验：事件名必须在白名单（save / mode-change / buffer-open / buffer-close），回调必须是函数；否则 JS 报错
- 同名事件可注册多个处理器，按注册顺序调用
- 处理器抛错：错误写入状态栏（set_error），不阻断主流程
- 处理器可调用 helix.echo；可对 doc 入队编辑（在事件挂点 drain + 应用）

## 事件语义

| 事件 | 时机 | 挂点 |
|------|------|------|
| `save` | 预处理（trim/format）之后、实际写入**之前**；处理器编辑先应用再保存 | `write_impl` |
| `mode-change` | 模式切换后，mode 为 "normal"/"insert"/"select" | `insert_mode` / `normal_mode` / `select_mode` |
| `buffer-open` | 打开成功后 | `open_impl` |
| `buffer-close` | 关闭前（doc 仍可读） | close 命令路径 |

- save 处理器编辑：一次保存的编辑 = 一个撤销点（沿用命令语义）
- **autosave（idle 自动保存）不触发 save 钩子**——明确取舍（自动保存路径绕过 write_impl）
- buffer-close 的挂点只覆盖关闭命令路径；`:q` 退出编辑器等路径不触发（PoC 边界）

## 实现

### helix-js（src/lib.rs）

- thread_local：`EVENT_HANDLERS: RefCell<HashMap<String, Vec<JsValue>>>`
- 原生函数 `js_on`：校验事件名白名单 + 函数类型，追加到处理器列表
- 公共函数：`pub fn emit_event(name: &str, ctx: &CommandContext) -> Result<()>`
  - 开始时清空编辑队列（防跨事件残留）
  - 对每个处理器：构造 doc 对象（复用 ctx_to_js）→ 调用（mode-change 额外传 mode 字符串）→ 错误转 anyhow
- `pub fn has_handlers(name: &str) -> bool` — 挂点快速判断（无处理器时零成本跳过）

### helix-term

- `apply_plugin_edits`（typed.rs）提升为 `pub(crate)`，供各挂点复用
- 新增共享辅助（typed.rs 或新模块）：`emit_plugin_event(cx, name, extra_mode: Option<&str>) -> Result<()>`——构建 CommandContext（current_ref! 序列化，同分发钩子）、emit_event、drain edits 并应用
- 四个挂点：
  1. `write_impl`：预处理块之后、`doc.append_changes_to_history` 之前（编辑需在保存历史之前应用，保证撤销点正确）——先实测确定插入点，若 append_changes_to_history 之前应用会导致撤销点混乱则调整
  2. `insert_mode` / `normal_mode` / `select_mode`：模式设置后调用（mode 名映射）
  3. `open_impl`：`editor.open` 成功后
  4. close 命令路径：`cx.editor.close` 调用前

## 测试

- **helix-js 单测**：注册/白名单校验/多处理器顺序/emit 时编辑队列清空/处理器抛错传播
- **集成测试**：
  - save：插件 `helix.on("save", doc => doc.insert(0,0,"pre-"))` → 打开文件 → `:w` → 断言磁盘文件内容含 "pre-" 且缓冲区文本更新
  - mode-change：`helix.on("mode-change", (m) => helix.echo("mode:"+m))` → `i` 进入插入 → 断言状态栏 "mode:insert"
  - buffer-open：`helix.on("buffer-open", (doc) => helix.echo("opened:"+doc.doc.path))` → `:open <file>` → 断言状态栏

## 非目标（YAGNI）

- doc-change（高频，需防抖）、事件取消/阻止、异步事件
- autosave 触发 save、非 close 命令的关闭路径触发 buffer-close
- 事件处理器带非当前文档上下文

## 涉及文件

- `helix-js/src/lib.rs`（EVENT_HANDLERS、js_on、emit_event、has_handlers、单测）
- `helix-term/src/commands/typed.rs`（write_impl / open_impl / close 挂点、apply_plugin_edits 提升、共享辅助）
- `helix-term/src/commands.rs`（三个模式命令挂点）
- `helix-term/tests/test/plugin.rs`（集成测试）
