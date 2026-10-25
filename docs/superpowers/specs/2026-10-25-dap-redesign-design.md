# DAP 整体重构 —— 设计

> 决策已定：**整体重构** · 目标语言 **Rust**（adapter = `lldb-dap`）· 配置**两个都读**
> （`.helix/launch.toml` 优先，回落 `.vscode/launch.json`）。
> 计划见 [`../plans/2026-10-25-dap-redesign.md`](../plans/2026-10-25-dap-redesign.md)。

## 1. 目标 / 非目标

**目标**

1. 能用：Rust 项目里"启动前打断点 → 启动 → 停在断点 → 看变量 → 步进 → REPL 求值"全程不用改全局配置。
2. 不阻塞：删掉 DAP 路径上的 `block_on`（现在是在 UI 线程里等 adapter）。
3. 多会话：结构上支持同时多个调试会话，命令可指定会话；单会话仍是默认心智模型。
4. 插件化：UI 交给 JS 插件（`dapui`），核心只留协议与会话真相 —— 与仓里 LSP/picker/statusline 的路线一致。
5. 可测：配置解析、事件 payload、UI 渲染都能在无真实 adapter 的情况下断言。

**非目标**

- 不做 `.vscode/launch.json` 全量兼容（`preLaunchTask` / `compounds` / `inputs` 明确不支持，遇到就报错）。
- 不做内存视图 / 反汇编 / 多线程时间线。
- 不动 server-manager 的安装机制（只加一条 `lldb-dap` 配方）。
- 不删 `:debug-*` 命令，不破坏 `languages.toml [language.debugger]`（老配置继续有效）。

## 2. 现状：三条硬约束（代码为据）

| # | 事实 | 位置 |
|---|---|---|
| 1 | adapter 启动 / 初始化 / 设断点全用 `block_on` 在同步路径上等 | `helix-dap/src/registry.rs:40,41,54`、`helix-view/src/handlers/dap.rs:126` |
| 2 | 单会话写死：`dap_launch` 直接拒绝第二个；`current_client_id: Option<_>`（自带 TODO）；`debugger!` 宏没会话时**静默 return** | `commands/dap.rs:246`、`registry.rs:19`、`handlers/dap.rs:17` |
| 3 | 插件面为 0；UI 只有 Picker/Prompt（变量、线程、栈帧、模板选择各一个 Picker） | `helix-js/src/api.rs`（无 dap）、`commands/dap.rs:245,516` |

已知缺口：配置只能写全局 `languages.toml`；无变量树/REPL/watch；断点只能逐行 toggle 且无列表；
无当前行指示与内联值；进度（`ProgressState`）到不了 UI。

**好消息**：UI 需要的原语已经有了 —— `set_virtual_text` / `set_highlight`（`helix.d.ts:701,703`）、
`open_panel({side,size,render,onKey,focusable})`、`el` / `draw.*`、`set_component_render`。
**坏消息**：`runtime/server-manager-builtin.toml` 只有 `debugpy`，**没有 `lldb-dap` 配方**。

## 3. 架构：三层

```
┌─ L1 helix-dap（协议内核，async，不认识 UI）─────────────────┐
│ Transport → Client → Session{client, caps, threads, frames,│
│   stopped, launch_args, verified_bps, progress}            │
│ Registry{SlotMap<SessionId,Session>, active, incoming}      │
│ BreakpointStore{path → [Breakpoint]}（跨会话共享）           │
│ 产出：SessionSnapshot + DapEvent（不再直接改 Editor）        │
└────────────────────────────────────────────────────────────┘
        ↓ EditorEvent::DebuggerEvent → 泵进 L2
┌─ L2 helix-term + helix-js（命令 / API / 事件）──────────────┐
│ :debug-*（保留，改走新内核） + helix.dap.*（新）             │
│ 配置层：launch.toml / launch.json / languages.toml 三者合并  │
│ 事件：dap-session|stopped|continued|exited|terminated|      │
│       output|thread|breakpoint|progress                    │
└────────────────────────────────────────────────────────────┘
        ↓ 纯 JS
┌─ L3 插件 dapui（新，runtime/plugins/dapui/）───────────────┐
│ 右 panel：Scopes/Variables 树（折叠/展开/求值）             │
│ 底 panel：REPL / Output（手绘 tab）                        │
│ 断点 & Watches 列表；set_virtual_text 内联值；当前行高亮     │
│ statusline 的 `dap` 段                                    │
└────────────────────────────────────────────────────────────┘
```

**切分判据**：内核只回答"协议说了什么"；桥只回答"这些真相怎么暴露"；UI 只回答"怎么画"。
任何"画什么/放哪"的决策都不许出现在 L1。

**async 怎么落地（已实现，与初版设计的差异记在这里）**：命令层（`commands/*`）与
`Callback::EditorCompositor` 都是**同步**的，所以"有地方 await"不成立。最终用的是两条现成机制的组合：

1. **`DapHandle`（廉价请求句柄）**：`Clone` 一次 = 两个 `Arc`/`Rc` 克隆 + 一个 `Copy` 的 id，
   只带 `server_tx` + 序号计数器 ⇒ 返回的 future **不借 `Client`**，可以 `'static` 地丢进
   `jobs.callback`。于是"单请求"类动作（continue/step/pause/设断点/取 scopes/variables/栈帧）
   全部走 `jobs.callback(async { … }, 回主线程改 editor)`，**没有一处 `block_on`**。
   另外 `send_only()`（不等回包）专供 `pause` 这类"结果由事件告知"的请求。
2. **`Editor.dap_intents` 队列**：Picker 回调（`thread_picker` 之类）手上只有 `&mut Editor`、
   拿不到 `Jobs`，所以它们只**排队**；由事件循环的 `pump_term_events`（唯一同时握有
   `&mut Editor` 与 `&mut Jobs` 的地方）调 `commands::dap::dispatch_dap_intents` 派发。
   队列元素是 `DapIntent`（`SyncBreakpoints` / `Pause` / `SwitchThread` / `Variables`）。

**为什么不用 `tokio::spawn`**：那会绕开 `&mut Editor` 的所有权模型（回主线程还得再造一条通道）；
`jobs.callback` 本来就是"异步 + 主线程续体"的现成设施（`dap_restart` 早就在用）。

**起会话的顺序**（`dap_start_impl`）：`reserve()` 先占 id → `jobs.callback` 里
`spawn_raw().await` + 句柄发 `initialize` → 发 `launch`/`attach` → **回调**里
`insert_client()` 并回填 `caps`。`initialized`/`stopped` 事件在此之前只是排在通道里，
插入后照常派发。

## 4. L1 契约

```rust
// 已实现形态（P0）：会话状态暂时仍在 Client 里（caps/threads/frames/progress/…）；
// 把它抽成独立 Session 结构是下一步（28 处调用点要一起动，见计划 P0-T3 的说明）。
pub struct Registry {
    inner: SlotMap<DebugAdapterId, Option<Client>>,  // Option：id 必须先于 spawn 确定
    current_client_id: Option<DebugAdapterId>,
    pub incoming: SelectAll<UnboundedReceiverStream<(DebugAdapterId, Payload)>>,
}
impl Registry {
    pub fn reserve(&mut self) -> DebugAdapterId;                  // 事件流要 id ⇒ 先占坑
    pub async fn spawn_raw(id, socket, cfg) -> Result<(Client, Rx)>;  // 只起传输，不 initialize
    pub async fn spawn_client(id, socket, cfg) -> Result<(Client, Rx)>; // + initialize
    pub fn insert_client(&mut self, id, client, rx, cfg, make_current) -> Result<DebugAdapterId>;
    pub async fn start_client(&mut self, socket, cfg) -> Result<DebugAdapterId>; // 给 async 上下文
    pub fn session_ids(&self) -> Vec<DebugAdapterId>;            // 多会话列表（:debug-sessions）
    pub fn active_id(&self) -> Option<DebugAdapterId>;
    pub fn set_active_client(&mut self, id);                     // 只认已就绪的槽
    ...
}
/// 断点**唯一真相**（已实现，`helix-dap/src/breakpoints.rs`）
pub struct BreakpointStore { /* BTreeMap<path, BTreeMap<line, Breakpoint>> */ }
```

- 断点**存在内核**（不再挂在 `Editor.breakpoints`；Editor 只保留只读快照给 gutter 渲染）。
- 会话 `Initialized` 时用 `join_all` **并行**推全部断点（现在是 for 里串行 `block_on`）。
- `attach` 之后从 `setBreakpoints` 响应回填 `verified/id/message`，并入事件 `dap-breakpoint`。
- `Session` 的 `Debug` 输出（`Stopped.reason/text`）保留在快照里，UI 自己决定显示。

## 5. L2 API 契约（`helix.dap.*`）

> **施工版见 [`2026-10-25-dap-api-p1.md`](2026-10-25-dap-api-p1.md)**：逐成员签名/校验/resolve 形状、
> 快照 JSON 字段级形状、9 个事件的 payload 与触发点、caps 与文档门禁、`dapui` 契约、可测性。
> 下面是概要（两处已同步更新：成员数 17、`launch`/`attach` 需 `process` 能力）。

风格：**动作/查询 = Promise**（照抄 `helix.lsp.*` 的 `JsPromise::new_pending`），**状态 = 快照 + 事件**（照抄 `helix.server.*`）。

```ts
namespace dap {
  sessions(): SessionInfo[]                    // 同步快照 {id,name,state,thread?,frame?,reason?}
  configs(): DapConfigInfo[]                   // 合并后的可用配置 {name,source:"toml"|"json"|"lang",language,request}
  launch(name?: string, args?: object, id?: number): Promise<SessionInfo>
  attach(name?: string, args?: object): Promise<SessionInfo>
  terminate(id?: number): Promise<void>
  restart(id?: number): Promise<void>
  continue(id?): Promise<void>; pause(thread?: number, id?): Promise<void>
  step_in(id?); step_out(id?); next(id?): Promise<void>
  threads(id?): Promise<Thread[]>
  stack_frames(thread: number, id?): Promise<StackFrame[]>
  scopes(frame: number, id?): Promise<Scope[]>
  variables(reference: number, id?): Promise<Variable[]>
  eval(expr: string, frame?: number, id?): Promise<{result:string; type?:string; variablesReference:number}>
  set_breakpoint(path: string, row: number, opts?: {condition?,hit?,log?}): Promise<void>
  remove_breakpoint(path: string, row: number): Promise<void>
  list_breakpoints(path?: string): BreakpointInfo[]   // 同步快照
  clear_breakpoints(path?: string): Promise<void>
  request(name: string, args?: object, id?: number): Promise<unknown>  // 逃生口
  set_exception_breakpoints(filters: string[]): Promise<void>
}
```

- `id` 省略 ⇒ active 会话；没有 active ⇒ **报错**（消息里点明"没有活动调试会话，先 :debug-launch"），不再静默。
- `request` 是逃生口：新能力不必每次改核心（`payload` 原样透传）。
- 所有 Promise 的 reject 消息带 DAP 的 `message`（`f37bee941` 已经做了"暴露失败响应"，继续沿用）。

**事件表**（`helix-js/src/commands.rs` 的事件白名单 + `types.rs` 的 payload）：

| 事件 | payload |
|---|---|
| `dap-session` | `{id, kind:"created"\|"removed", name, state}` |
| `dap-stopped` | `{id, reason?, thread_id?, text?, all_threads_stopped?}` |
| `dap-continued` | `{id, thread_id?, all_threads_continued?}` |
| `dap-exited` / `dap-terminated` | `{id, exit_code?, restart?}` |
| `dap-output` | `{id, category, output}` |
| `dap-thread` | `{id, reason, thread_id}` |
| `dap-breakpoint` | `{id?, path, line, verified, message?}` |
| `dap-progress` | `{id, kind:"start"\|"update"\|"end", text, percentage?}` |

**命令映射**（`commands/dap.rs` 全部改走新内核，行为兼容）：`:debug-launch`（Picker 里现在既有
`templates` 也有 launch 配置，带来源标注）／`:debug-configs`（列配置）／`:debug-sessions`（切会话）。

## 6. 配置层（两个都读）

**两件事必须分开**：

- **adapter 定义**（起哪个进程、transport、`port_arg`、`quirks`）→ **只**来自 `languages.toml [language.debugger]`
  （用户手写或 server-manager 写入的 managed 段）。
- **launch 配置**（起什么程序、什么参数）→ 来自下面两级文件。

**优先级（高 → 低）**

1. `<workspace>/.helix/launch.toml`（自有格式，`[[config]]`，可带注释）
2. `<workspace>/.vscode/launch.json`（VS Code 子集：`type` `request` `name` `program` `args` `cwd` `env` `attach`）
3. `languages.toml [language.debugger].templates`（现有机制，兜底，永远可用）

同名 `name` 逐字段覆盖（tol 覆盖 json）；`configs()` 里每项带 `source`。

**`type` → adapter 映射**：先按名字精确匹配任一语言的 `debugger.name`（`lldb-dap` / `codelldb`），
再按当前文档语言回退到该语言的 `debugger`；都没有 ⇒ 报错"没有适配器能处理 type=…"。

**变量替换（两边都支持，最小集）**：
`${workspaceFolder}` `${file}` `${fileBasename}` `${fileDirname}` `${fileBasenameNoExtension}`
`${env:VAR}` `${cwd}`。不认识的占位符 ⇒ **报错**（不静默留字面量）。
`preLaunchTask` / `compounds` / `inputs` ⇒ 明确报"不支持"。

**`launch.toml` 形状**

```toml
[[config]]
name = "run crate"
language = "rust"
request = "launch"          # launch | attach
program = "${workspaceFolder}/target/debug/myapp"
args = ["--flag"]
cwd = "${workspaceFolder}"
env = { RUST_BACKTRACE = "1" }
```

## 7. L3 插件 `dapui`（runtime/plugins/dapui/plugin.js）

- 右 panel（`side:"right"`, `size:32`, `focusable`）：Scopes → Variables 树，`el("button")` 折叠，
  `dap.variables` 懒加载，Enter 求值 / `e` 内联显示。
- 底 panel（`side:"bottom"`, `size:12`）：手绘 tab（`REPL` / `Output`），`el("input")` 输入 → `dap.eval`。
- 断点列表：`:dapui-breakpoints` 弹 Picker（`dap.list_breakpoints()`），`d` 删除、`c` 改条件。
- 编辑器内：`set_highlight` 当前执行行、`set_virtual_text` 行尾内联值（受 `[plugins.dapui] inline_values` 控制）。
- statusline 段：`dap`（会话名 + 状态），注册进 statusline 的段表。
- 配置：`[plugins.dapui] inline_values = true` / `auto_open = true` / `variables_max_depth = 2`。

## 8. 验收（每期独立可交付）

| 期 | 内容 | 验收 |
|---|---|---|
| **P0** | 删 `block_on`（3 处）、Registry 多会话、`debugger!` 报错、`BreakpointStore`、`lldb-dap` 配方 | `cargo test -p helix-dap -p helix-view -p helix-term --features integration` 全绿；手动：启动前打断点 → `:debug-launch` → 停在断点；两个会话可并存 |
| **P1** | `helix.dap.*` + 9 个事件 + `dapui` v1（变量树 + 断点列表 + statusline 段） | `cargo xtask api-check` 绿；`helix-js` 单测覆盖 API 契约（无 adapter，用桩会话）；`node --test` 覆盖 dapui 渲染 |
| **P2** | 配置层（launch.toml + launch.json + 变量替换）+ REPL + 内联值 + 会话切换 UI | 配置解析单测（三种来源 + 覆盖 + 变量替换 + 不支持项报错）；手动跑通 Rust 三种 launch（binary / args / attach pid） |

## 9. 风险与取舍

- **async 改造是最大风险**：真正的根因不是 `block_on` 本身，而是**命令层同步**（§3 的 `DapIntent` 已说明
  路径）。"把 `block_on` 换掉"如果没做 intent 队列，就会变成到处 `tokio::spawn` + 绕过 `&mut Editor`，
  那是另一种债。所以 P0 的第一件事是 intent 队列，不是删 `block_on`。
- **`helix-dap` 目前零测试**（`rg '#\[test\]' helix-dap/src` 为空）：P0 必须先把 `Client::streams`
  这个已有接缝用起来（内存 duplex 假 adapter），否则"去阻塞"改完无法验证。
- **多会话会让 UI 复杂**：默认仍是"一个会话"，UI 只在 >1 时显示 `#id` 前缀。
- **launch.json 兼容性是有边界的承诺**：只做子集，不认识的字段**报错**，避免"看起来支持其实静默忽略"。
- **不做 lldb-dap 的完整能力适配**：`capabilities` 里的差异（条件断点/日志断点/`runInTerminal`）继续
  沿用现有 `quirks` + capability 检查，不扩。
