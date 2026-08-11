# 异步 fs API 实现计划（方案二-A）

> 自主执行（方案二窗口内）。TDD → 通过 → 提交；控制者自动合并（并行 wave，冲突由控制者解决）。

### 任务 1：异步 fs（单任务）

**文件：** `helix-js/src/lib.rs`、`helix-term/src/application.rs`

- [ ] **步骤 1：失败单测**（helix-js tests，TEST_LOCK；临时目录）

```rust
#[test]
fn async_fs() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    let dir = std::env::temp_dir().join(format!("helix-js-fs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "hello fs").unwrap();
    std::fs::write(dir.join("b.js"), "x").unwrap();

    load_script(&format!(r#"
        helix.register_command("fsd", () => {
            helix.read_file_async("{dir}/a.txt", (err, content) => {
                helix.echo("read:" + (err ?? "") + ":" + (content ?? ""));
            });
            helix.write_file_async("{dir}/out.txt", "written", (err) => {
                helix.echo("write:" + (err ?? "ok"));
            });
            helix.stat_async("{dir}/a.txt", (err, st) => {
                helix.echo("stat:" + st.size + ":" + st.is_dir);
            });
            helix.glob_async("{dir}/*.js", (err, paths) => {
                helix.echo("glob:" + paths.length);
            });
        });
    "#)).unwrap();

    let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
    assert!(run_command("fsd", &ctx).unwrap());
    // 轮询 drain_async_events 直到四个回调都到（wait_for_async 辅助，仿 wait_for_term_event）
    wait_for_async(|e| matches!(e, AsyncEvent::FsRead(_, _)));
    wait_for_async(|e| matches!(e, AsyncEvent::FsWrite(_, _)));
    wait_for_async(|e| matches!(e, AsyncEvent::FsStat(_, _)));
    wait_for_async(|e| matches!(e, AsyncEvent::FsGlob(_, _)));
    // resolve 全部（事件里带 id）→ 断言回调 echo
    let msgs = take_messages();
    assert!(msgs.iter().any(|m| m == "read::hello fs"), "{msgs:?}");
    assert!(msgs.iter().any(|m| m == "write:ok"), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.starts_with("stat:8:false")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m == "glob:1"), "{msgs:?}");
    assert_eq!(std::fs::read_to_string(dir.join("out.txt")).unwrap(), "written");

    // 错误路径：读不存在 → err 非空
    load_script(&format!(r#"
        helix.register_command("fsbad", () => {
            helix.read_file_async("{dir}/nope.txt", (err, content) => {
                helix.echo("bad:" + (err !== null ? "err" : "noerr"));
            });
        });
    "#)).unwrap();
    assert!(run_command("fsbad", &ctx).unwrap());
    wait_for_async(|e| matches!(e, AsyncEvent::FsRead(_, _)));
    // resolve → 断言
    assert!(take_messages().iter().any(|m| m == "bad:err"));

    // 类型校验
    assert!(load_script(r#"helix.read_file_async(42, () => {});"#).is_err());
    assert!(load_script(r#"helix.read_file_async("x", 42);"#).is_err());

    let _ = std::fs::remove_dir_all(&dir);
}
```

> 注意：`wait_for_async` 轮询辅助（仿既有 wait_for_term_event）。resolve 语义：事件带 id，从事件取 id 调 resolve_async_event。回调参数：read → (err, content)；write → (err)；stat → (err, {size, is_dir, mtime})；glob → (err, paths 数组)。glob 实现：worker 里手写 `*`/`**`/`?` 基础匹配（walk 目录树；无新依赖）——或 workspace 已有 globset（helix 用 globset 0.4）直接用其 Pattern 匹配路径字符串。以最简可用为准。

- [ ] **步骤 2：运行失败** → 编译错误。
- [ ] **步骤 3：实现 helix-js**
  - `AsyncEvent` 枚举 + `ASYNC_EVENTS`/`ASYNC_EVENTS_RX` 全局 + `ASYNC_CALLBACKS`/`NEXT_ASYNC_ID`（泄漏容器）
  - 一次性 worker：spawn thread 做操作 → tx.send(AsyncEvent(id, result))（失败 → Err 字符串）
  - 四个原生函数（校验 + 注册回调 + spawn）
  - `drain_async_events()` / `resolve_async_event(id, event)`（调回调：err 参数 null 或字符串；成功后从注册表移除）
- [ ] **步骤 4：helix-js 通过**（31 个，clippy 0）
- [ ] **步骤 5：helix-term 泵点**：render() 里 term 事件处理后补 `drain_async_events` → `resolve_async_event`（错误 set_error）→ 顺带 drain edits/messages/cursor（回调可能编辑）
- [ ] **步骤 6：集成测试**（tests/test/plugin_fsasync.rs，临时 mod）：`:fs-demo` 读临时文件 → echo → 状态栏断言
- [ ] **步骤 7：回归**（plugin 组、command_line、cargo check/clippy）
- [ ] **步骤 8：Commit** `feat(js): async fs APIs (read/write/stat/glob)`

---

## 自检
- 风险：glob 实现选择；worker 一次性线程生命周期；resolve 与 term 事件泵的先后（不冲突——不同 channel）。
