# 设计：异步 fs API（方案二-A）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

插件异步读写文件/查询/glob，不阻塞编辑器。与 `run_async` 同构（worker 线程 + 回调）。

## 新增 JS API

```js
helix.read_file_async(path, (err, content) => {...})     // 读文件（UTF-8 lossy）
helix.write_file_async(path, content, (err) => {...})     // 写文件（覆盖）
helix.stat_async(path, (err, stat) => {...})              // stat: { is_dir, size, mtime }
helix.glob_async(pattern, (err, paths) => {...})          // glob（相对 CWD；* / ** / ?）
```

- 回调在主线程事件循环执行；回调内可 echo/编辑/开弹窗（同 run_async 语义）
- 同步保留：`read_dir`（既有）；不新增同步大函数

## 实现

- **helix-js**：
  - 新全局 channel：`ASYNC_EVENTS: (Sender<AsyncEvent>, Mutex<Receiver<AsyncEvent>>)`；`AsyncEvent { FsRead(u64, Result<String,String>), FsWrite(u64, Result<(),String>), FsStat(u64, Result<(bool,u64,i64),String>), FsGlob(u64, Result<Vec<String>,String>) }`
  - thread_local（泄漏模式）：`ASYNC_CALLBACKS: HashMap<u64, JsValue>` + `NEXT_ASYNC_ID`
  - 原生函数：`js_read_file_async` / `js_write_file_async` / `js_stat_async` / `js_glob_async`（校验 path 字符串 + 回调函数 → 注册 → spawn 一次性 worker 线程做操作 → 结果经 channel 发回）
  - 公共：`drain_async_events() -> Vec<AsyncEvent>`、`resolve_async_event(id, event) -> Result<()>`（调回调，err 参数为 null 或错误字符串）、`async_callback 清理`
  - glob：无新依赖——用 `glob` crate？检查 workspace 是否有（helix 用 globset——不同 API）。最简：worker 里手写 glob（`*`/`**`/`?` 支持的基础匹配，walk 目录）——或加 `glob` crate（轻）。以实现最简为准：若 workspace 有 globset 可直接用（helix 已有），否则手写基础匹配
- **helix-term**：render 泵点在既有 term 事件之后补 `drain_async_events` → `resolve_async_event` → 顺带 drain edits/messages/cursor

## 测试

- 单测：read 往返（写临时文件 → read_async → 内容一致）；write（写 → fs 读一致）；stat（is_dir/size）；glob（临时目录 `*.js`）；错误路径（读不存在 → err 非 null）；类型校验
- 集成：插件 `:fs-demo` 调 read_file_async 读临时文件 → echo 内容 → 状态栏断言

## 涉及文件

- `helix-js/src/lib.rs`（channel/注册表/四个原生函数/公共函数/单测）
- `helix-term/src/application.rs`（泵点）
