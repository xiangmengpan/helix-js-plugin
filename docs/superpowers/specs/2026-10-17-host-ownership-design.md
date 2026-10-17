# G11（C 批）· 双状态所有权收尾：`state.rs` ↔ `host.rs` 设计规格

日期:2026-10-17
状态:**待审查**(铁律 5:批准前不动代码)
关联:[`../architecture-audit.md`](../../architecture-audit.md) §5 G11 · Phase D 计划(`docs/superpowers/plans/2026-10-14-js-host-phaseD-and-boundary.md`)· G3(已完成)

## 0. 决策记录(需要你拍板的两点)

1. **不做「把 6 个 `OnceLock` + 12 个 `thread_local` 全部搬进 `JsHost`」**。理由不是成本,而是**物理上不该**:
   - `JsHost` 持有 boa 的 `Context`,是 `!Send`;
   - 但 `TERM_EVENTS` / `ASYNC_EVENTS` / `LSP_RESULTS` 这些是**跨线程通道**(worker → 主线程),
     必须 `Send`;`TERM_WORKERS` / `TERM_MASTERS` 是**进程级进程表**(pty 的 fd / 子进程句柄),
     跟某个 JS 宿主没有一一对应关系。
   - 强行搬进去只能靠 `unsafe impl Send` 或再引入一层全局表 —— 前者是 UB 温床,后者是把同一份状态换个位置放。
   → 结论:**这些"进程级"是它们的本质属性**,不是遗漏。要收的是**说法**与**唯一读点**。
2. **`ACTIVE: Cell<Option<NonNull<JsHost>>>` 保留裸指针,但把「人工契约」变成「一处读点 + 不变量测试」**。
   换成 `Rc<RefCell<JsHost>>` 会让 Phase D 的成果**退化**:现在 `pump()` 拿 `&mut JsHost`,
   「JS 回调里再调一次 render」是**编译不过**;换成 `RefCell` 会变成**运行期 panic**。
   裸指针的真正风险是"宿主析构前没 `clear_active`",这个有 `host_cleared_before_drop` 守着,
   本项再把 5 个分散读点收成 1 个,并补 2 条不变量测试。

## 1. 目标

把「双状态」从**含糊的债务**变成**写清楚的所有权地图**:

- 哪些状态**按宿主**(每引擎一份)、哪些**进程级**(跨线程/进程表)、为什么;
- `ACTIVE` 的读点收成 1 个函数,所有访问器经它;
- 注释里**不再有互相矛盾的两套说法**(现状:模块头说"队列仍是进程级(Phase B)",
  字段注释说"现在按宿主隔离",两种说法并存于同一文件);
- 补 2 条可执行的不变量测试,替代"靠人记得"。

## 2. 现状与约束(已核实)

### 2.1 已经是「按宿主」的(不需要动)

| 状态 | 位置 | 说明 |
|---|---|---|
| 引擎 | `JsHost.engine` | 一份 `boa::Context` |
| echo 消息 | `JsHost.messages` | `with_messages` 已走宿主 |
| 脚本导出缓存 | `JsHost.exports` | `helix.load` 按脚本名命中,**换根不失效** ⇒ 必须每宿主一份 |
| 已加载脚本清单 | `JsHost.loaded_scripts` | 同上 |
| UI 请求 | **按宿主分桶** | 队列本体在 `state.rs`,但按 `host_key` 分桶;消费端在 helix-term(16 个调用点,签名冻结) |
| 权限状态 | `JsHost.permissions` | C-2 |

### 2.2 仍是「进程级」的(本项**判定为合理**,只补文档)

| 状态 | 类型 | 为什么不能进宿主 |
|---|---|---|
| `PICKER_STATE` / `VIEWPORT` / `LAST_LAYOUT` / `CONFIGS` / `DIAGNOSTICS` | `OnceLock<Mutex<…>>` | 消费端是 helix-term 的**帧路径**(每帧读 viewport/layout),签名冻结;搬进宿主只会多一层取用 |
| `UI_REQUESTS` | `OnceLock<UiRequests>` | 分桶存储(§2.1);本体必须是 `Send`(worker 也 push) |
| `TERM_WAKE` | `Mutex<Option<Arc<dyn Fn + Send + Sync>>>` | 跨线程唤醒回调(非 JS 宿主概念) |
| `TERM_EVENTS(_RX)` / `ASYNC_EVENTS(_RX)` / `LSP_RESULTS(_RX)` | `thread_local!` | worker → 主线程通道,**必须 `Send`** |
| `TERM_WORKERS` / `TERM_MASTERS` | `thread_local!` | pty 的进程/fd 表,与"哪个 JS 宿主"无关 |
| `NEXT_ASYNC_ID` | `thread_local!` | 单调 id 分配器 |

### 2.3 `ACTIVE` 的 5 个读点(要收成 1 个)

`host.rs` 里 `ACTIVE.with(|a| a.get())` 出现在:`try_with_host_ref`、`with_host_ref`、
`try_current_host_key`、`current_host_key`、`has_host`(以及写点 `set_active`/`clear_active`/`pump`)。

人工契约(注释):`set_active` 的调用者必须保证注册期覆盖宿主生存期,析构前 `clear_active()`;
`Application::drop` 已做,`host_cleared_before_drop` 守着。**违规 = UB**。

### 2.4 测试现状

- 已有:`host_cleared_before_drop`(契约)、`two_hosts_are_isolated`(set_active/clear_active/嵌套恢复)。
- 缺:切回后 `current_host_key` 是否**稳定**、`clear_active` 后查询类入口是否**回落 DEFAULT_HOST 而不建新宿主**。

## 3. 目标架构(只做 G11a)

### G11a(本次实施)

1. **唯一读点**:
   ```rust
   /// `ACTIVE` 的**唯一读点**。所有访问器经它 —— 裸指针读点越少,审计面越小。
   fn active_ptr() -> Option<NonNull<JsHost>> { ACTIVE.with(|a| a.get()) }
   ```
   5 处 `ACTIVE.with(...)` 改为 `active_ptr()`。
2. **所有权地图写进代码**:`host.rs` 模块头换成一张表(§2.1/§2.2 的浓缩版)+ 一句
   「新增状态时先问:它是跨线程的吗?是 ⇒ 进程级;否 ⇒ 宿主字段」。
   删掉三处 Phase B 遗留说法(模块头 / 结构体注释 / `two_hosts_are_isolated` 的注释),
   改成指向本 spec 的结论。
3. **补 2 条不变量测试**
   - `host_key_is_stable_per_host_and_falls_back`:同一宿主两次 `current_host_key` 相同;
     `set_active(other)` 后不同;`clear_active` 后回落到 DEFAULT_HOST 的 key(同前)。
   - `queries_do_not_create_host_after_clear`:`clear_active` 后调 `has_host()` / `try_current_host_key()`
     **不得**触发 `init()`(断言 DEFAULT_HOST 仍为空 / 引擎数不变)。
4. **不改任何所有权**、不改 `ACTIVE` 的类型、不改 `pump` 的 `&mut` 排他性。

### G11b(明确**不做**,记为结论)

把 §2.2 的进程级状态搬进宿主。理由见 §0.1。若将来真的需要"两个宿主完全隔离地跑插件",
正确形态是**两个进程**(helix-term 侧已有 `server_tasks` 的进程模型可参考),而不是在一个进程里塞两个引擎。

## 4. 边界与错误处理

- 查询类入口**永不建宿主**(`try_with_*` 系列;这是既有约定,本项加测试钉住)。
- `clear_active` 之后,写操作入口(`with_engine` 等)照常 panic("host not registered")——
  这是**期望行为**,不改成静默。
- 不引入新的 `unsafe`;不改变 `unsafe` 块的 SAFETY 注释所依赖的前提。

## 5. 测试计划与完成判据

```bash
cargo test -p helix-js                       # 基线 176(170 passed / 6 ignored),本项 +2 条
cargo test -p helix-js -- --ignored
cargo xtask api-check                        # 134 成员 / 20 事件 / 15 检查点 不变
cargo integration-test                       # 377(宿主生命周期与插件命令路径)
cargo fmt --all --check
cargo clippy --workspace --all-targets --features integration -- -D warnings
```

| 指标 | 现状 | 目标 |
|---|---|---|
| `ACTIVE` 读点 | 5 处 | **1 处**(`active_ptr()`) |
| `host.rs` 里"Phase B / 仍是进程级"的矛盾说法 | 4 处 | **0** |
| 不变量测试 | 2 条 | **4 条** |
| `cargo test -p helix-js` | 176 | **178**(+2) |
| 377 集成测试 / api-check | 绿 | **不变** |

## 6. 里程碑

| 阶段 | 内容 | 预估 |
|---|---|---|
| M1 | `active_ptr()` + 5 处替换 + 2 条测试 | 1 小时 |
| M2 | 模块头/结构体/测试注释的所有权地图 | 40 分钟 |
| M3 | 全量验证 + 记账 | 30 分钟 |

## 7. 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| "收口读点"时漏改一处 | 行为不一致(某入口仍读裸 ACTIVE) | 全仓 grep `ACTIVE.with` 必须只剩 1 处;clippy 无新增 |
| 新测试在并行下互相踩宿主 | flaky | 走 `crate::tests::TEST_LOCK`(与既有宿主测试一致) |
| 把"进程级合理"当成"可以不管" | 未来有人再提同样的重构 | 结论写进代码注释 + 本 spec,`architecture-audit.md` G11 引用 |
