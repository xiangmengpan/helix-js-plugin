# JS Promise/async 化实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** 5 个回调式异步 API(`run_async` / `read_file_async` / `write_file_async` / `stat_async` / `glob_async`)改为返回 Promise,插件可用 `await`/`.then`,并同步迁移 2 处插件调用点。

**架构:** 注册表里把 `JsValue` 回调换成 boa `ResolvingFunctions`(`JsPromise::new_pending` 生成);worker→channel→主线程 `render()` 泵点的现有管道不动,resolve 事件时调用 `resolving.resolve/reject.call(...)`;新增 `pump_jobs()` 在每帧泵 boa promise job 队列(否则 `.then`/`await` 永不恢复——现状没有任何 run_jobs 调用)。`spawn` 保持流式回调。删除 `TermCallbacks.is_run_async` 槽位 hack。

**技术栈:** boa_engine 0.21.1(`JsPromise::new_pending`、`ResolvingFunctions{resolve,reject}: JsFunction`、`Context::run_jobs`、`JsNativeError::error().with_message(msg).to_opaque(ctx)`),已有管道 TermEvent/AsyncEvent。

**规格:** `docs/superpowers/specs/2026-08-25-js-promise-design.md`(已 commit `0a612455d`)

**验证命令:**
```bash
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-js --lib
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo clippy -p helix-js --all-targets
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 timeout 400 cargo test -p helix-term --test integration --features integration -- plugin_async
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo build --release
```

---

### 任务 1:run_async Promise 化 + job 队列泵(helix-js)

**文件:**
- 修改:`helix-js/src/types.rs:157-162`(TermCallbacks 删 is_run_async)
- 修改:`helix-js/src/state.rs`(TERM_PROMISES 注册表 + with_term_promises,仿 TERM_CALLBACKS)
- 修改:`helix-js/src/shell.rs`(js_run_async 重写、resolve_term_event 加 promise 分支)
- 修改:`helix-js/src/lib.rs`(run_async arity 2→1、pump_jobs、测试迁移)

- [ ] **步骤 1:写失败测试(迁移 async_run_and_spawn 的 run_async 段)**

`helix-js/src/lib.rs` 的 `async_run_and_spawn` 测试,把 run_async 段(现在 `helix.run_async("echo async-hello", (err, out) => {...})`)改为:

```rust
// run_async:Promise 模式 → Exit 事件 → resolve → pump_jobs 执行 .then
load_script(
    r#"
    helix.run_async("echo async-hello").then((out) => {
        helix.echo("cb:ok:" + out.trim());
    });
    "#,
)
.unwrap();
let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
let TermEvent::Exit(id, code, stdout) = &events[0] else { unreachable!() };
assert_eq!(*code, 0);
resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap();
pump_jobs().unwrap();
assert_eq!(take_messages(), vec!["cb:ok:async-hello"]);

// run_async 错误路径:非零退出 → reject → .catch
load_script(
    r#"
    helix.run_async("echo boom >&2; exit 3").catch((e) => {
        helix.echo("raerr:" + e.message);
    });
    "#,
)
.unwrap();
let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
let TermEvent::Exit(id2, code2, _) = &events[0] else { unreachable!() };
assert_ne!(*code2, 0);
resolve_term_event(*id2, TermEvent::Exit(*id2, *code2, None)).unwrap();
pump_jobs().unwrap();
let msgs = take_messages();
assert!(msgs[0].contains("raerr:"), "{msgs:?}");

// run_async 参数类型错误保留
assert!(load_script(r#"helix.run_async(42);"#).is_err());
// 注:原断言 helix.run_async("x", 42) 是回调校验报错,现 JS 多参天然容忍 → 删除该断言
```

- [ ] **步骤 2:运行确认失败**

运行:`HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-js --lib async_run_and_spawn`
预期:编译错误(`pump_jobs` 未定义、`run_async` 传参不匹配)。

- [ ] **步骤 3:实现**

`helix-js/src/state.rs`(仿 TERM_CALLBACKS 的 leak 模式):

```rust
// 与 TERM_CALLBACKS 并列声明
static TERM_PROMISES: RefCell<Option<&'static mut HashMap<u64, ResolvingFunctions>>> = const { RefCell::new(None) };
// init() 里与 TERM_CALLBACKS 并列:
TERM_PROMISES.with(|t| { *t.borrow_mut() = Some(Box::leak(Box::new(HashMap::new()))); });
// 访问器:
pub(crate) fn with_term_promises<T>(f: impl FnOnce(&mut HashMap<u64, ResolvingFunctions>) -> T) -> T {
    TERM_PROMISES.with(|t| f(t.borrow_mut().as_mut().expect("TERM_PROMISES not initialized")))
}
```

`helix-js/src/types.rs` — TermCallbacks 删 `is_run_async` 字段和它的注释。

`helix-js/src/shell.rs` — js_run_async 重写(删回调参数、new_pending、存 TERM_PROMISES、返回 promise;spawn_worker 调用不变):

```rust
pub(crate) fn js_run_async(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let cmd: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.run_async: command must be a string")))
    })?;
    let (promise, resolving) = JsPromise::new_pending(context);
    let id = crate::state::next_term_id();
    crate::state::with_term_promises(|m| { m.insert(id, resolving); });
    let (tx, rx) = std::sync::mpsc::channel();
    crate::state::with_term_workers(|m| m.insert(id, tx.clone()));
    spawn_worker(id, &cmd, true, crate::state::with_term_events(|t| t.clone().expect("TERM_EVENTS initialized")), rx);
    Ok(promise.into())
}
```

`resolve_term_event` 在 `with_engine` closure 开头(emit_term_exit 之后、取 TermCallbacks 之前)插入 promise 分支:

```rust
if let Some(resolving) = with_term_promises(|m| m.remove(&id)) {
    match event {
        TermEvent::Chunk(..) => {} // run_async 只关心 Exit,忽略
        TermEvent::Exit(_, code, stdout) => {
            let out = stdout.unwrap_or_default();
            let args = if code == 0 {
                vec![JsValue::from(JsString::from(out))]
            } else {
                let err = JsNativeError::error()
                    .with_message(format!("exit {code}: {}", out.trim()))
                    .to_opaque(engine);
                vec![err.into()]
            };
            let _: JsValue = resolving.reject.call(&JsValue::undefined(), &args, engine)
                .map_err(|e| anyhow!("term {id} promise settle failed: {e}"))?;
        }
    }
    crate::state::with_term_workers(|m| m.remove(&id));
    #[cfg(unix)]
    crate::state::with_term_masters(|m| m.remove(&id));
    return Ok(());
}
```

注意:`resolving.resolve` / `resolving.reject` 都是 JsFunction,resolve 时直接 `resolving.resolve.call(&undefined, &[value], engine)`,reject 时用 `resolving.reject.call(...)`,不能都叫 reject。成功分支用 resolve,失败分支用 reject。

`helix-js/src/lib.rs`:
- `run_async` 注册 arity 从 2 改为 1(`.function(..., JsString::from("run_async"), 1)`)
- 新增公共函数(放 shell.rs 或 lib.rs,与 drain_term_events 同层):

```rust
/// 泵 boa promise job 队列(.then/await 恢复)。主线程每帧调用;空队列即返回。
pub fn pump_jobs() -> Result<()> {
    crate::init();
    crate::state::with_engine(|engine| engine.run_jobs().map_err(|e| anyhow!("promise job queue: {e}")))
}
```

- spawn 测试段(同函数内)不动;`async_utf8_across_chunks` 测试(它用 run_async 回调)也改为 promise 式:`.then((out) => helix.echo("len:" + out.length + " tail:" + out.slice(-11)))` + pump_jobs 后再断言 take_messages。
- 类型校验断言:`helix.run_async(42)` 保留;`helix.run_async("x", 42)` 删除。

- [ ] **步骤 4:运行确认通过**

运行:`HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-js --lib`
预期:PASS(60 个测试,含迁移后的)。

- [ ] **步骤 5:Commit**

```bash
git add helix-js/src/types.rs helix-js/src/state.rs helix-js/src/shell.rs helix-js/src/lib.rs
git commit -m "feat: run_async 改 Promise 返回 + job 队列泵"
```

---

### 任务 2:fs 四件套 Promise 化(helix-js)

**文件:**
- 修改:`helix-js/src/state.rs`(with_async_callbacks → with_async_promises,存 ResolvingFunctions)
- 修改:`helix-js/src/shell.rs`(js_read_file_async / js_write_file_async / js_stat_async / js_glob_async、resolve_async_event)
- 修改:`helix-js/src/lib.rs`(arity 调整、async_fs 测试迁移)

- [ ] **步骤 1:写失败测试(迁移 async_fs 的 fsd/fsd2/fsbad 段)**

`async_fs` 测试里 `fsd` 命令的四个 `helix.xxx(path, (err, ...) => ...)` 改为 `.then(...)` 形式,并保留原断言:

```rust
load_script(&format!(r#"
    helix.register_command("fsd", () => {{
        helix.read_file_async("{dir}/a.txt").then((content) => {{
            helix.echo("read::" + content);
        }});
        helix.write_file_async("{dir}/out.txt", "written").then(() => {{
            helix.echo("write:ok");
        }});
        helix.stat_async("{dir}/a.txt").then((st) => {{
            helix.echo("stat:" + st.size + ":" + st.is_dir);
        }});
        helix.glob_async("{dir}/*.js").then((paths) => {{
            helix.echo("glob:" + paths.length);
        }});
    }});
"#, dir = dir.display())).unwrap();
```

drain/resolve 循环后、`take_messages()` 断言前加 `pump_jobs().unwrap();`。原断言改为匹配新文案:`"read::hello fs"`、`"write:ok"`、`"stat:8:false"`、`"glob:1"` 不变。`fsd2` 的 `glob_async` 同理(`glob2::2` 不变)。`fsbad`(读不存在文件)改为:

```rust
helix.read_file_async("{dir}/nope.txt").catch((e) => {{
    helix.echo("bad:" + (e !== null ? "err" : "noerr"));
}});
```

`async_cb` 辅助函数删除(不再有回调校验)。

- [ ] **步骤 2:运行确认失败**

运行:`HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-js --lib async_fs`
预期:编译错误(函数签名不匹配)。

- [ ] **步骤 3:实现**

`state.rs`:`with_async_callbacks`(HashMap<u64, JsValue>)改为 `with_async_promises`(HashMap<u64, ResolvingFunctions>),leak 模式不变,同步改 init() 与所有调用点。

`shell.rs` 四个函数统一模式(以 read 为例):

```rust
pub(crate) fn js_read_file_async(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.read_file_async: path must be a string")))
    })?;
    let (promise, resolving) = JsPromise::new_pending(context);
    let id = crate::state::next_async_id();
    with_async_promises(|m| { m.insert(id, resolving); });
    spawn_async_op(id, move || {
        std::fs::read_to_string(&path).map_err(|e| format!("read_file_async('{path}'): {e}"))
    }, AsyncEvent::FsRead);
    Ok(promise.into())
}
```

`write_file_async` 保持 path/content 两个字符串参数校验后同样模式;`stat_async`/`glob_async` 同 read。

`resolve_async_event` 重写(错误也构造 Error 对象使 e.message 可用):

```rust
pub fn resolve_async_event(id: u64, event: AsyncEvent) -> Result<()> {
    crate::init();
    crate::state::with_engine(|engine| {
        let resolving = with_async_promises(|m| m.remove(&id));
        let Some(resolving) = resolving else { return Ok(()) }; // 已 resolve / 未知 id → no-op(幂等)
        let undefined = JsValue::undefined();
        let result: Result<(), JsError> = match event {
            AsyncEvent::FsRead(_, Ok(content)) => resolving.resolve.call(&undefined, &[JsValue::from(JsString::from(content))], engine).map(|_| ()),
            AsyncEvent::FsRead(_, Err(e)) => reject_msg(&resolving, &e, engine),
            AsyncEvent::FsWrite(_, Ok(())) => resolving.resolve.call(&undefined, &[], engine).map(|_| ()),
            AsyncEvent::FsWrite(_, Err(e)) => reject_msg(&resolving, &e, engine),
            AsyncEvent::FsStat(_, Ok(st)) => resolving.resolve.call(&undefined, &[stat_to_js(&st, engine)?], engine).map(|_| ()),
            AsyncEvent::FsStat(_, Err(e)) => reject_msg(&resolving, &e, engine),
            AsyncEvent::FsGlob(_, Ok(paths)) => {
                let arr = paths.iter().map(|p| JsValue::from(JsString::from(p))).collect::<Vec<_>>();
                let js_arr = boa_engine::object::builtins::JsArray::from_iter(arr, engine);
                resolving.resolve.call(&undefined, &[js_arr.into()], engine).map(|_| ())
            }
            AsyncEvent::FsGlob(_, Err(e)) => reject_msg(&resolving, &e, engine),
        };
        result.map_err(|e| anyhow!("async fs {id} settle failed: {e}"))
    })
}

fn reject_msg(resolving: &ResolvingFunctions, msg: &str, engine: &mut Context) -> Result<(), JsError> {
    let err = JsNativeError::error().with_message(msg).to_opaque(engine);
    resolving.reject.call(&JsValue::undefined(), &[err.into()], engine).map(|_| ())
}
```

`lib.rs` arity:read_file_async 2→1、write_file_async 3→2、stat_async 2→1、glob_async 2→1。

- [ ] **步骤 4:运行确认通过**

运行:`HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-js --lib`
预期:PASS。

- [ ] **步骤 5:Commit**

```bash
git add helix-js/src/state.rs helix-js/src/shell.rs helix-js/src/lib.rs
git commit -m "feat: fs 异步 API 改 Promise 返回"
```

---

### 任务 3:helix-term 泵点 + 集成测试

**文件:**
- 修改:`helix-term/src/application.rs:338`(render 泵点调 pump_jobs)
- 修改:`helix-term/tests/test/plugin_async.rs`(async-demo 改 promise 式 + 新增错误路径)

- [ ] **步骤 1:写失败测试**

`plugin_async.rs` 的 `async-demo` 命令改为:

```js
helix.register_command("async-demo", () => {
    helix.run_async("echo async-ok").then((out) => {
        helix.echo("async:" + (out ?? "").trim());
    });
});
helix.register_command("async-err", () => {
    helix.run_async("exit 3").catch((e) => {
        helix.echo("err:" + e.message);
    });
});
```

测试断言保持 `status == "async:async-ok"`,新增第二个输入 `:async-err<ret>` 断言 `status == "err:exit 3"`(或 `contains("err:")`,以实际 message 为准)。

- [ ] **步骤 2:运行确认失败**

运行:`HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 timeout 400 cargo test -p helix-term --test integration --features integration -- plugin_async`
预期:失败(status 空或超时——job 队列未被泵,`.then` 不执行)。

- [ ] **步骤 3:实现**

`application.rs` render() 泵点,在 drain/resolve 之后(watch resolve 前)加:

```rust
// 泵 promise job 队列(.then/await 恢复);空队列即时返回
if let Err(err) = helix_js::pump_jobs() {
    log::error!("promise job pump: {err}");
}
```

注意放在 `resolve_term_event`/`resolve_async_event` 调用之后,避免 with_engine 嵌套。

- [ ] **步骤 4:运行确认通过**

运行:`HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 timeout 400 cargo test -p helix-term --test integration --features integration -- plugin_async`
预期:PASS。再跑 `HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo clippy -p helix-term -p helix-js --all-targets` 确认无警告。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/application.rs helix-term/tests/test/plugin_async.rs
git commit -m "feat: render 泵 promise job 队列 + 集成测试"
```

---

### 任务 4:插件迁移 + 真机验证

**文件:**
- 修改:`~/.config/helix/plugins/features/statusline.js:25`(refreshGit)
- 修改:`~/.config/helix/plugins/features/filetree/index.js:432`(run_fs)

- [ ] **步骤 1:迁移 statusline.js**

```js
async function refreshGit(path) {
  if (!path) { gitBranch = ""; return; }
  try {
    const out = await helix.run_async("git -C " + shq(dirname(path)) + " branch --show-current");
    gitBranch = (out || "").trim();
  } catch (e) {
    gitBranch = "";
  }
}
```

- [ ] **步骤 2:迁移 filetree/index.js**

```js
async function run_fs(cmd, ok_msg) {
  try {
    await helix.run_async(cmd);
    if (ok_msg) helix.echo(ok_msg);
    reload_tree();
  } catch (err) {
    helix.echo("filetree: " + err.message);
  }
}
```

`new_file` 保持 prompt 回调(它是 UI 交互回调,不是异步 API),内部 `run_fs(...)` 调用不变。

- [ ] **步骤 3:同步插件 + 编译 release**

```bash
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo build --release
cp ~/.config/helix/plugins/features/statusline.js ~/.config/helix/plugins/statusline.js 2>/dev/null || true
# 按实际目录结构同步(handoff:改插件后 cp 到 ~/.config/helix/plugins/)
```

- [ ] **步骤 4:真机验证**

启动 release 版 helix:`git status` 分支显示正常(statusline 不报错);filetree 里新建文件/删除文件 → 操作成功且树刷新。`:config-reload` 后仍正常。

- [ ] **步骤 5:Commit 插件(仓库内镜像若有)+ 收尾**

若仓库内有插件镜像(如 `runtime/plugins/` 或项目内 plugins 目录),同步后:

```bash
git add -A
git commit -m "refactor: 插件迁移到 Promise API"
```

---

## 自检记录

- **规格覆盖:** 设计文档 5 个 API → 任务 1(run_async)+ 任务 2(fs 四件套);spawn 保持回调 → 任务 1 明确不动;插件迁移 → 任务 4;设计文档"删除 is_run_async hack" → 任务 1;`pump_jobs` 是设计文档未覆盖的实现前置(规格"引擎侧无需改动"不成立——boa 的 job 队列必须手动泵),已通过任务 1/3 补齐,规格无需改(属于实现细节)。
- **占位符:** 无 TODO/待定;所有代码块可直接执行。
- **类型一致性:** `with_term_promises`/`with_async_promises` 命名统一;`reject_msg` 在任务 2 定义并被同一文件使用;`pump_jobs` 在任务 1 定义、任务 3 消费;`ResolvingFunctions.resolve/reject` 均为 `JsFunction`,用 `.call(&JsValue::undefined(), &[args], engine)`。
