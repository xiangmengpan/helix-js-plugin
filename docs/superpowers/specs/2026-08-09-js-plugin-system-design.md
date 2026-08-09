# 设计：helix JS 插件系统（PoC）

日期：2026-08-09
状态：已批准

## 目标

在 helix 源码中嵌入 JavaScript 插件能力，证明"JS 插件在编辑器里跑得通"这一路径可行。

范围：**A2** —— 插件可读文档/光标、可输出消息、可注册命令（`:name` 调用）。不支持修改缓冲区（A3 留待后续）。

## 架构

新 workspace member `helix-js`，与现有 crate 并列（helix-core / helix-view / helix-term ...）。

分层原则：`helix-js` **不依赖 helix-view / helix-term**。插件命令的上下文是纯数据：

```rust
pub struct CommandContext {
    pub path: Option<String>,   // 当前文档路径（无路径则为 None）
    pub text: String,           // 当前文档全文
    pub cursor: (usize, usize), // (row, col)
}
```

`helix-term` 负责从编辑器状态序列化出 `CommandContext`，再调用运行时。这样 helix-js 可独立测试、引擎可替换。

### 运行时

`helix-js` 内部：

- boa `Context`（引擎）
- 命令注册表 `HashMap<String, JsValue>`，存 boa 函数句柄
- 全局单例 `OnceLock<Mutex<PluginRuntime>>`
  - `ponytail:` 全局锁；将来若支持多编辑器实例/多进程再改按实例持有

### 引擎选型

**boa**（纯 Rust，零 C 依赖）。已批准。运行时接口封装薄层，将来换 rquickjs/deno_core 只改 helix-js 内部。

## JS API 面

```js
helix.register_command("hello", (ctx) => {
  helix.echo("cursor: " + ctx.cursor.row + "," + ctx.cursor.col);
});
```

- `helix.register_command(name: string, fn: (ctx) => void)`
  - 校验：name 非空、不含空白字符；非法时报错到消息栏
- `helix.echo(text: string)` — 消息栏输出
- `ctx` 只读快照：
  - `ctx.doc.path: string | null`
  - `ctx.doc.text: string`
  - `ctx.cursor.row: number`, `ctx.cursor.col: number`

## 集成点（helix-term）

全部位于 `helix-term/src/commands/typed.rs`（位置已核对）：

1. **分发**：`execute_command_line`（~L4125）静态 `TYPABLE_COMMAND_MAP` miss 且 `PromptEvent::Validate` 时，直接按名查插件注册表 → 序列化状态 → 调 JS。不构造 TypableCommand（其 `fun` 是无捕获的 fn 指针，无法携带命令名）。
2. **补全**：`complete_command_line`（~L4258）fuzzy 列表 merge 插件命令名。
3. **` :plugin-load <path>`** 新 TypableCommand：读文件 → `runtime.load_script()` → 注册。
4. **启动自动加载**：扫描 `~/.config/helix/plugins/*.js` 依次加载（失败不阻断启动，仅报错到消息栏）。

> 注：`TYPABLE_COMMAND_MAP` 是 `Lazy<HashMap>` 静态表，插件命令不进静态表，只进插件注册表和补全列表。

## 错误处理

- JS 抛错 / 脚本加载失败 / 命令注册失败：`cx.editor.set_error(...)`（anyhow 包装）
- 启动加载失败不中断编辑器启动

## 测试

helix-js 内一个测试（`cargo test -p helix-js`）：

1. 加载一段内联脚本（注册命令 + echo）
2. 用假 `CommandContext` 运行命令
3. 断言 echo 输出被捕获且内容正确

## 非目标（YAGNI）

- 文档修改（A3）
- 事件钩子、键位绑定、UI API
- 热重载、插件市场、沙箱/权限模型
- 除 boa 外的引擎支持

## 涉及文件

- `Cargo.toml`（workspace members 增加 helix-js）
- `helix-js/Cargo.toml`（新建，依赖 boa_engine）
- `helix-js/src/lib.rs`（新建：运行时 + 注册表 + JS API + 测试）
- `helix-term/Cargo.toml`（依赖 helix-js）
- `helix-term/src/commands/typed.rs`（分发钩子 + 补全合并 + `:plugin-load`）
- `helix-term/src/application.rs` 或 `main.rs`（启动自动加载）
