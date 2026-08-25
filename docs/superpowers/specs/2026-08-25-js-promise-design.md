# 设计:helix JS Promise/async 化

日期:2026-08-25
状态:草案(待审查)

## 背景:不用 Promise 的复杂度(现状事实)

现有异步 API 全部是回调式(`run_async` / `read_file_async` / `write_file_async` / `stat_async` / `glob_async`),插件代码因此产生四类复杂度:

### 1. 调用方拿不到结果,只能副作用 + 全局变量

`statusline.js:25`:

```js
let gitBranch = "";
function refreshGit(path) {
  helix.run_async("git -C " + shq(dirname(path)) + " branch --show-current", (err, out) => {
    gitBranch = err ? "" : (out || "").trim();  // 只能写全局,无返回值
  });
}
```

渲染函数读 `gitBranch` 时无法区分"查询中"与"无分支",也没有显式失效/缓存语义。

### 2. 竞态:乱序返回覆盖新结果

连续两次 `refreshGit(a)` → `refreshGit(b)`,若 a 的查询后返回,`gitBranch` 被旧值覆盖。Promise 化后 `let b = await ...` 天然串行,或用请求序号守卫。

### 3. 嵌套金字塔 + 错误处理重复

`filetree/index.js:432` 附近:

```js
function new_file() {
  prompt("新建文件", "", (name) => {
    if (!n) return;
    run_fs("touch ...", "已创建");   // 内部又是 run_async 回调
  });
}
function run_fs(cmd, ok_msg) {
  helix.run_async(cmd, (err, out) => {
    if (err) { helix.echo("filetree: " + err + ...); }   // 每个回调重复判 err
    else { ...; reload_tree(); }
  });
}
```

操作链 A→B→C 每层缩进 + 1 个 `if (err)`,错误处理无法统一,也没有 `finally`。

### 4. 无法组合

- 无 `Promise.all`:两个异步结果(如 git 分支 + 远程状态)要合并时,得手写双回调计数器
- 无顺序串联:上一个结果喂给下一个命令,只能嵌套
- 无统一 try/catch:一个链条里的错误要每个环节各自处理

### 实现侧(helix-js)的现状成本

- `run_async` 把同一个回调塞进 `TermCallbacks { on_chunk, on_exit }` 两个槽(`shell.rs:309`,hack)——Chunk 事件对它无意义,只为了走 Exit 分支拿 stdout
- 回调是 `JsValue`(!Send),worker 线程不能直接调,必须经 channel 泵到主线程 `render()`,且 thread_local 容器必须 `Box::leak`(已有注释说明)
- 每次事件到达都 `JsFunction::from_object` 重新解包

Promise 化不消除泵点(boa 的 `ResolvingFunctions` 同样是 !Send),但把"每次事件都 call 一次"变成"仅一次 resolve/reject",并消除 run_async 的槽位 hack。

## 方案对比

| 方案 | 做法 | 优点 | 缺点 |
|---|---|---|---|
| **C(推荐)** | 全量替换为 Promise,删回调式 | 最干净:单一语义、漏传回调即 TypeError、无迁移包袱残留 | 需同步改 2 处插件调用点 |
| A | 回调式函数不传回调时返回 Promise(双模式) | 现有插件零改动 | 双语义认知负担;漏传回调且忘 await → 静默丢结果零报错(Node 因同类问题放弃双模式,改开 `fs.promises`) |
| B | 新增独立 Promise 函数,旧函数留作 deprecated | 语义干净、迁移渐进 | 长期双入口;自用项目无渐进迁移需求 |

选 C 的理由:本项目是自用 fork,回调调用点全项目仅 2 处(statusline.js `refreshGit`、filetree.js `run_fs`),一次 commit 改完,迁移成本≈0。没有外部生态兼容压力,不应为省 2 处改动背负双模式的混乱。

## API 设计

```js
// 新写法:全部返回 Promise
const out = await helix.run_async("git status");
try {
  const data = await helix.read_file_async("/path");
  const st = await helix.stat_async("/path");
  const files = await helix.glob_async("**/*.rs");
  await helix.write_file_async("/path", "content");
} catch (e) {
  helix.echo("failed: " + e.message);
}
```

- **签名去回调,返回 Promise**:5 个 API(`run_async` / `read_file_async` / `write_file_async` / `stat_async` / `glob_async`)删除回调参数,返回 Promise。多余参数 → 沿用原有类型校验报错(参数个数校验保持,非函数参数按类型错误处理)
- **resolve 值**:与旧回调第二参一致——`run_async` → stdout 字符串;`read_file_async` → 文件内容;`stat_async` → stat 对象;`glob_async` → 字符串数组;`write_file_async` → undefined
- **reject**:exit code ≠ 0 或 IO 错误 → reject `Error`(message = stderr/stdout 截断或错误文案),不再走 `(err, out)` 二元组
- **run_async 的 Chunk 事件**:不再需要(仅 Exit 时 resolve/reject),删除 `TermCallbacks.is_run_async` 与 on_chunk 槽位 hack
- **spawn / term_write / term_kill**:保持流式回调,不做 Promise(流式输出语义不适合一次性 resolve;YAGNI)
- 引擎侧无需改动:`await` 由 boa 0.21 原生支持(async_function 模块,已存在)
- **插件迁移**(同 commit 完成):statusline.js `refreshGit` 改 async 函数 + `await helix.run_async(...)` 后赋值,竞态问题顺带用请求序号守卫或忽略(迁移时按现状保持简单);filetree.js `run_fs` 改 async 函数 + try/catch 复用现有错误文案

## 实现

### helix-js(`shell.rs` + `state.rs`)

- `state.rs`:注册表改造——`TermCallbacks` 的 `on_chunk`/`on_exit`/`is_run_async` 改为存 `ResolvingFunctions`(run_async 与 fs async 共用)。`spawn` 保留 `TermCallbacks`(on_chunk/on_exit 回调不变)
- 各 `js_*_async` 函数:删回调参数;`let (promise, resolving) = JsPromise::new_pending(context)?;` 存入注册表,返回 `promise`
- `resolve_term_event`:
  - `TermEvent::Chunk` → 仅 spawn 回调有 on_chunk 时调用;run_async/fs 注册表项忽略
  - `TermEvent::Exit` → `resolving.resolve(stdout)` 或 `resolving.reject(...)`,并移除注册表项;spawn 回调走原路径
- 复用现有 `with_engine` 泵点(`application.rs:338`),零改动 helix-term

### helix-term

- 无改动(泵点已存在);唯一注意:promise resolve 后若插件 await 链里调用 echo/编辑,现有 drain 逻辑已覆盖

## 测试

- **helix-js 单测**:
  - `run_async("echo hi")` → 轮询 drain → resolve 后断言 promise 结果 "hi\n"(经 `JsPromise::state`/then 或直接断言注册表被清 + 值可查)
  - exit code ≠ 0 → reject 路径
  - 现有 run_async 回调式测试改为 Promise 式(同步迁移)
  - `write_file_async` resolve undefined
- **集成测试**(tests/test/plugin_async.rs 追加):插件命令 `:p-demo` 用 `await helix.run_async("echo p-ok")` 后 echo 结果 → 状态栏 "p-ok";错误命令 → catch 后 echo 错误路径

## 非目标(YAGNI)

- spawn/流式 Promise 化、Promise 超时/取消、AbortController
- fetch/网络 API(独立特性)
- 回调兼容层(旧插件按现状一次性迁移,不留双模式)

## 涉及文件

- `helix-js/src/shell.rs`(各 js_*_async 删回调、new_pending、resolve/reject 分发)
- `helix-js/src/state.rs`(注册表改造:run_async/fs 用 ResolvingFunctions,spawn 保留回调)
- `helix-js/src/lib.rs`(单测迁移)
- `helix-term/tests/test/plugin_async.rs`(集成测试)
- `~/.config/helix/plugins/features/statusline.js`、`~/.config/helix/plugins/features/filetree/index.js`(迁移 2 处调用点)
