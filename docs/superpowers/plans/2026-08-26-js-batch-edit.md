# 批量编辑事务(begin_edit/end_edit)实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** `helix.begin_edit()/end_edit()` 声明事务边界:期间编辑积压,end 后合并应用为一个 Transaction(一次撤销),解决 async 命令跨 await 被每帧泵循环拆事务的问题。

**架构：** helix-js 加线程局部事务深度 `EDIT_TXN_DEPTH: Cell<usize>`;`take_edits` 入口拦截——深度 >0 时返回空(编辑留在队列,不消费);end 后下一帧泵循环取到积压 → `apply_plugin_edits` 一次应用 = 一个事务。所有调用路径(泵循环/命令/弹窗)共用 take_edits,单点拦截。

**技术栈：** boa 0.21、thread_local Cell(仿现有 state.rs 模式)。

**规格：** `docs/superpowers/specs/2026-08-26-js-batch-edit-design.md`(已批准)

---

### 任务 1：helix-js — 事务深度 + begin/end API + take_edits 拦截

**文件：**
- 修改：`helix-js/src/state.rs`(加 `EDIT_TXN_DEPTH: Cell<usize>` + `with_txn_depth` 访问器,仿 `NEXT_MAP_ID` 60 行模式)
- 修改：`helix-js/src/commands.rs`(`js_begin_edit`/`js_end_edit` + `take_edits` 拦截)
- 修改：`helix-js/src/lib.rs`(builder 注册 `begin_edit`/`end_edit` 0 参,`set_completion_icon` 附近)

- [ ] **步骤 1：编写失败的测试**(`helix-js/src/lib.rs` 测试模块,仿现有 `TEST_LOCK` 模式)

```rust
#[test]
fn batch_edit_transaction() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.register_command("be", (ctx) => {
            helix.begin_edit();
            ctx.doc.insert(0, 0, "a");
            // 事务打开时 take_edits 返回空(编辑积压)
            if (take_edits_for_test().length !== 0) throw new Error("txn open should hold edits");
            helix.end_edit();
        });
        "#,
    )
    .unwrap();
    // 直接测 take_edits 拦截语义:
    // 1. 未开启事务:编辑入队后可取走
    // 2. begin 后编辑入队,take_edits 返回空(积压)
    // 3. end 后 take_edits 取到积压
    // (JS 侧无法直接调 take_edits,用 Rust 侧辅助验证;上面脚本里 take_edits_for_test 是 Rust 注册的测试辅助)
}
```

> 注:测试辅助 `take_edits_for_test` 若不可行(测试环境 JS 无法触达 Rust 函数),改为 Rust 侧直接驱动:
> 用 `with_edits` push 编辑 + 调 `begin_edit`(load_script 里调用)→ `take_edits()` 断言空 → 调 `end_edit` → `take_edits()` 断言取到。begin/end 通过 load_script 的 `helix.begin_edit()` 调用(注册后),编辑队列用 Rust 侧 `with_edits` 注入。

- [ ] **步骤 2：运行确认失败**

运行：`cargo test -p helix-js --lib batch_edit_transaction`
预期：FAIL(`begin_edit`/`end_edit` 未注册 → JS 报错,或 take_edits 未拦截)。

- [ ] **步骤 3：实现**

`state.rs`(NEXT_MAP_ID 60 行后):
```rust
// 批量编辑事务深度(begin_edit/end_edit);>0 时 take_edits 积压
static EDIT_TXN_DEPTH: Cell<usize> = const { Cell::new(0) };
```
`state.rs`(访问器,with_cursor_requests 280 行附近):
```rust
pub(crate) fn with_txn_depth<T>(f: impl FnOnce(&mut usize) -> T) -> T {
    EDIT_TXN_DEPTH.with(|d| f(&mut d.get()))
}
```
> 注:Cell 不可借 &mut,用 `d.get()/d.set()` 或改 RefCell。若用 Cell:`with_txn_depth` 直接 get/set,不传 &mut。以编译为准。

`commands.rs`(js_echo 附近):
```rust
pub(crate) fn js_begin_edit(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    crate::state::with_txn_depth(|d| *d += 1);
    Ok(JsValue::undefined())
}

pub(crate) fn js_end_edit(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    crate::state::with_txn_depth(|d| *d = d.saturating_sub(1));
    Ok(JsValue::undefined())
}
```

`commands.rs`(`take_edits` 819 行):
```rust
pub fn take_edits() -> Vec<Edit> {
    if crate::state::with_txn_depth(|d| *d > 0) {
        return Vec::new(); // 事务打开:积压不消费
    }
    crate::state::with_edits(std::mem::take)
}
```

`lib.rs`(builder 注册,set_completion_icon 附近):
```rust
.function(NativeFunction::from_fn_ptr(commands::js_begin_edit), JsString::from("begin_edit"), 0)
.function(NativeFunction::from_fn_ptr(commands::js_end_edit), JsString::from("end_edit"), 0)
```

- [ ] **步骤 4：运行确认通过**

运行：`cargo test -p helix-js --lib`
预期：batch_edit_transaction 及其余全 PASS;`cargo clippy -p helix-js --all-targets` 零警告。

- [ ] **步骤 5：Commit**

```bash
git add helix-js/src/state.rs helix-js/src/commands.rs helix-js/src/lib.rs
git commit -m "feat(js): begin_edit/end_edit 批量编辑事务——take_edits 深度>0 时积压,合并一次撤销"
```

### 手动/集成验证(可并入任务 1 或留最终)

- integration(async 命令跨 await 合并一次撤销)若测试环境可行则加;不行则文档注明手工验证路径。
