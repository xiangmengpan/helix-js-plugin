# 设计：异步 API + 流式终端（v8）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

`helix.run_async`（完成回调式异步 shell）+ `helix.spawn`/`term_write`/`term_kill`（流式进程：实时输出块 + stdin 写入 + 可杀）。不阻塞编辑器主线程。终端插件的地基。

## 新增 JS API

```js
// 完成回调式：命令结束后调 cb(err, out)
helix.run_async("git status", (err, out) => { ... });

// 流式：spawn 返回句柄 id
const id = helix.spawn({
  cmd: "bash",                 // sh -c 语义
  onChunk: (chunk) => { ... }, // 输出块（stdout+stderr 合并），UTF-8 lossy
  onExit: (code) => { ... },   // 退出码（信号终止传负值或 128+signal，以实测为准）
});
helix.term_write(id, "ls\n");  // 写 stdin
helix.term_kill(id);           // 杀进程（SIGKILL）
```

- 参数校验：cmd 字符串、回调必须函数；spawn 的 cmd/onChunk 必填、onExit 可省
- 回调在主线程事件循环执行（`render()` 泵点）；回调内可编辑/echo（drain 应用）
- 块大小 4KB；输出不截断（终端场景需要全量；run_async 保持 64KB 截断）
- 无超时（同 run，ponytail）；Unix-only（sh）

## 实现

### helix-js（核心）

- 全局 channel：`TermEvent { Chunk(u64, String), Exit(u64, i32) }`——`static TERM_EVENTS: OnceLock<(Sender<TermEvent>, Mutex<Receiver<TermEvent>>)>`；spawn 时 clone Sender 给 worker
- worker 注册表（全局 Mutex）：`HashMap<u64, Sender<TermCtrl>>`，`TermCtrl { Write(String), Kill }`——term_write/kill 经此发消息
- 回调注册表（thread_local，泄漏模式）：`TERM_CALLBACKS: RefCell<Option<&'static mut HashMap<u64, TermCallbacks { on_chunk: JsValue, on_exit: JsValue }>>>`；`NEXT_TERM_ID: Cell<u64>`
- worker 线程：spawn child（stdout+stderr 管道合并读取，逐 4KB 读块 → send Chunk）→ 循环 select stdin 控制消息（Write→child.stdin.write_all；Kill→child.kill+break）→ EOF 后 wait → send Exit
- 公共函数：
  - `pub fn drain_term_events() -> Vec<TermEvent>`（锁 Receiver Mutex，try_recv 循环）
  - `pub fn resolve_term_event(id: u64, event: TermEvent) -> Result<()>`（主线程：查回调注册表 → 调 on_chunk/on_exit；调用前不建 doc ctx——回调无 doc 参数）
  - `pub fn term_write(id: u64, text: &str) -> Result<()>`（查 worker 注册表 → send Write）
  - `pub fn term_kill(id: u64) -> Result<()>`（send Kill）
- 原生函数：`js_run_async`（校验 + 注册回调 + spawn 一次性 worker）、`js_spawn`（校验 + 注册回调 + 返回 id）、`js_term_write`、`js_term_kill`

### helix-term（泵点）

- `Application::render()` 开头：`let events = helix_js::drain_term_events();` 非空时逐个 `resolve_term_event`，然后 drain `take_cursor_requests`/`take_edits`/`take_messages` 应用（复用既有辅助）——回调的编辑/echo 即时生效
- 无 render 时事件滞留（下一次 render 处理——render 由事件/重绘触发，异步回调到达后需触发重绘：resolve 后 `helix_event::request_redraw()` 或 editor.needs_redraw = true，以既有模式为准）

## 测试

- **helix-js 单测**：
  - run_async：spawn "echo hi" → 轮询 drain_term_events（带超时循环，最多 ~5s）→ resolve → 断言 onChunk 类回调（run_async 用 Exit 事件携带 stdout）或直接断言 drain 到的事件内容；`resolve_term_event` 后 take_messages 断言回调执行
  - spawn 流式：spawn `cat`（sh -c "cat"）→ term_write("hi\n") → 轮询 drain → Chunk("hi\n") 到达
  - term_kill：spawn `sleep 100` → kill → 轮询 Exit 事件到达（快）
  - 类型校验（cmd 非字符串/回调非函数 → 报错）
- **集成测试**（tests/test/plugin_async.rs）：插件 `:async-demo` 调 run_async("echo async-ok", cb echo out) → 事件循环泵 → 状态栏 "async-ok"（测试 harness 的 event_loop_until_idle 会跑 render？——若 render 泵点时序不稳，在断言前加一个无害按键触发重绘，实测后定）

## 非目标

- PTY 分配（无交互式 TUI 程序——bash 非交互模式即可用）、stderr 分离（合并）
- 超时/取消机制、进程组/信号细化（只有 Kill）
- 环境变量/工作目录控制

## 涉及文件

- `helix-js/src/lib.rs`（TermEvent/TermCtrl/channel/worker/注册表/四个原生函数/公共函数/单测）
- `helix-term/src/application.rs`（render 泵点）
- `helix-term/tests/test/plugin_async.rs`（新）
