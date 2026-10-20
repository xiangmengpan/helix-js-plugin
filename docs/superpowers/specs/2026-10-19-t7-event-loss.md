# T7 · 事件在跨线程时"消失" —— 规格

> 状态：**待确认**（C 类改动，按 §2 铁律 5 先出 spec）。
> 上游：D3 spec [`2026-10-19-d3-test-flake.md`](2026-10-19-d3-test-flake.md) §9.4。
> 本文**订正**了那份 spec 与 `plugin_async.rs` 文件头的一个假设（见 §2.3）。

## 1. 现象

`cargo test --test integration plugin --test-threads=64` 连跑 5 次：**4 次全绿，1 次红**，
红的是 `plugin_async::plugin_async_run`：

```
pump_until 超时：预算 5s，实际等待 5.00s，条件：run_async 的 .then 回包；最后一次状态栏: ""
```

即：`:async-demo` 命令跑了，但 5 秒内 `.then` 的回包**从未到达主线程** —— 不是"慢"，
`pump_until` 的 5s 足够覆盖正常的 <100ms。

`plugin_async.rs` 头部自述这个隐患存在已久，理由写的是"multi_thread 下主任务 await 时可能
迁移 OS 线程，线程本地 TERM_EVENTS 通道滞留旧线程 → ~1/22 随机丢事件"。

## 2. 已证实的事实

### 2.1 通道是线程本地的（核心代码）

```rust
// helix-js/src/state.rs:794-816
thread_local! {
    static ASYNC_EVENTS:    RefCell<Option<WakeSender<AsyncEvent>>> = …;
    static ASYNC_EVENTS_RX: RefCell<Option<Receiver<AsyncEvent>>>  = …;
    static LSP_RESULTS:     …      static LSP_RESULTS_RX: …
    static TERM_EVENTS:     …      static TERM_EVENTS_RX: …
}
```

`init()`（`helix-js/src/lib.rs:76-96`）对每个线程**各自建一套通道**（槽为空就 `std::sync::mpsc::channel()`）。
`drain_async_events()`（`shell.rs:838`）先 `init()` 再读**本线程**的 `RX`。
worker 侧 `spawn_async_op`（`shell.rs:573`）在**发起线程**克隆 sender，所以只要发起线程 = 消费线程，就不会丢。

### 2.2 跨线程不可见 —— 确定性实验（已跑，非推测）

在 A 线程 `init()` 并取 sender，发一条事件；A 自己 drain；再发一条，交给 B 线程 drain；最后回 A drain：

```
PROBE on_a=1  on_b=0  on_a2=1
```

结论：**B 线程看不到 A 通道里的事件**（`on_b=0`），而事件并没消失（回 A 还能拿到 1 条），
它是**留在原线程的通道里**。所以"丢事件"的准确说法是：**消费方换了线程**。

### 2.3 订正："测试主任务迁移 OS 线程"这个解释站不住

`Application` 持有 `JsHost`，而 `JsHost` 持有 boa 的 `Context`（`!Send`，Rc 系内部结构）
⇒ `Application: !Send` ⇒ 持有它的 future **不能被 tokio 在 worker 之间搬**。
所以"主任务迁移"不能解释本现象；真正在做 JS/泵的**另一个线程**还没被定位（见 §3 T1）。

### 2.4 这是一条**被写明过的设计决定**，不是遗漏

`helix-js/src/host.rs:10-27` 的"状态所有权地图（G11）"里有一行：

> | `TERM_EVENTS` / `ASYNC_EVENTS` / `LSP_RESULTS`（含 `_RX`） | **thread_local** | worker → 主线程通道，必须 `Send` |

论证在 `docs/superpowers/specs/2026-10-17-host-ownership-design.md`。
所以改它之前必须先**证明那条理由不成立**。本文认为它把两件事合在了一起：

- **`Sender` 必须 `Send`** —— 成立，且搬进宿主后仍然成立（worker 照样只拿 `WakeSender` 克隆）；
- **`Receiver` 放哪** —— 与 `Send` 无关。它只需要"被**消费它的那段代码**取到"。
  现在取到与否取决于**线程**；若搬进 `JsHost`，则取决于**App**（宿主随 `Application` 走）。

⇒ 搬进宿主是**兼容**那条理由的；但只有在 T1 证明"消费确实发生在别的线程"之后才值得做
（否则 1/5 另有其因，见 §2.5）。

### 2.5 还没解释的部分（不许猜）

若"发起线程 = 消费线程"恒成立，则通道分裂**不该发生**，1/5 的失败就得另找原因
（例如：worker 的 `send` 失败、事件被别的 App 消费、`.then` 的 resolve 注册在了另一个宿主上）。
T1 的职责就是把这条钉死。

### 2.6 因此修复方向有两种，代价差一个数量级

- 若某处 JS 确实跑在第二线程上 ⇒ 通道随线程分裂；
- 若只是 `init()` 在第二线程先建了一套 ⇒ 也是分裂，但可以通过"通道跟着宿主走"根除。

## 3. 方案

### T1（必须第一步做）：定位"第二线程"是谁

**不先定位不许改代码** —— 改错的代价是"给一个 1/5 的偶发 bug 加一层看起来对的机制"。

- 在 `init()` 里加**临时**诊断（env 开关 `HX_DBG_JS_THREAD=1`）：创建通道时打印
  `(线程 id, 通道种类)`；在 `drain_async_events` 打印"本线程 + 排到几条"；
  在 `spawn_async_op` 打印"发起线程 id"。
- 用同一条复现命令跑到红（本机 1/5；必要时连跑 5–10 次，每次 ~150s）。
- 判据：红的那个用例里，`ASYNC_EVENTS` 的创建线程 ≠ drain 线程 ⇒ 拿到具体线程号与调用栈
  （`RUST_BACKTRACE=1`）。

**产出**：一句可验证的结论（谁在第二线程上跑了什么），写回本文 §2.3。

### T2（主修，视 T1 结论）：把三条通道从 thread-local 搬进 `JsHost`

理由：`JsHost` 是**每 App 一份**且随 `Application` 一起移动 —— 通道跟着它走，
"哪个线程消费"就不再影响可见性。

```rust
pub struct JsHost {
    // 新增：每宿主一份的通道（sender 给 worker 克隆，receiver 由宿主自己 drain）
    async_rx: std::sync::mpsc::Receiver<AsyncEvent>,
    lsp_rx:   std::sync::mpsc::Receiver<LspResult>,
    term_rx:  std::sync::mpsc::Receiver<TermEvent>,
    async_tx: WakeSender<AsyncEvent>,   // 供 worker 克隆
    …
}
```

- `init()` 保留 thread-local 兜底（无宿主时，如 helix-js 单测/CLI 一次性脚本）——**双状态是有代价的**：
  这正是 G11 处理过的那类问题，必须立一条不变量并加测试（见 T3）。
- 不变量：**一个事件只能被创建它的那条通道消费一次**；`drain_*` 优先取宿主，无宿主才取 thread-local。

### T3：测试（先红后绿）

1. **跨线程可见性**（T1 的实验固化成测试，反向断言）：
   ```rust
   // 现状是 on_b == 0；修好后应为 on_b == 1
   #[test] fn async_event_is_visible_from_any_thread_of_the_same_host()
   ```
   注意：这条在**宿主版**落地前必然红，作为 T2 的验收锚点；thread-local 兜底路径仍允许
   "跨线程不可见"，所以测试必须在**有宿主**的前提下写。
2. **同一事件不被消费两次**：宿主 drain 一次后，再 drain 为空（防双状态重复交付）。
3. **两个 App 在一个进程里不串台**（集成测试已隐含此约束）：A 的 worker 事件只被 A 收到。
   —— 这条是方案的关键风险点，必须显式钉住。

### T4：把两个"已知隐患"注释换成事实

- `plugin_async.rs` 文件头的"主任务迁移 OS 线程"解释（§2.3 已证伪）改成 T1 的实测结论。
- D3 spec §9.4 的 T7 条目 → 指向本文。

## 4. 不做（附理由）

| 方案 | 否决理由 |
|---|---|
| 把 `pump_until` 预算从 5s 调到 30s | 事件若留在别的线程，等多久都不会到；只是把 flake 变成"更慢的 flake" |
| 所有通道改成进程级 `static`（不跟宿主） | 同进程两个 App 会互相偷事件（id 空间也更会撞）。这正是把通道搬进宿主而不是搬成全局的原因 |
| 给该用例加 `#[ignore]` | 掩盖真实缺陷；且 `--ignored` 也不会让它更稳 |
| 在测试里 sleep 后重试整条命令 | `:async-demo` 有副作用（重复 spawn 进程）；且重试会掩盖问题（仓库铁律不许） |

## 5. 验收

- [ ] T1 结论写回本文，含线程号与"谁在第二线程上跑了什么"
- [ ] 修好后 `"$BIN" plugin --test-threads=64` **连跑 10 次全绿**（当前 1/5 红）
- [ ] T3 的三条测试全绿，其中"跨线程可见"在修前**确实红过**（记录到 §8 变更记录）
- [ ] `cargo test -p helix-js`（含 `--ignored`）与 `cargo integration-test` 全绿
- [ ] 没有新增 `#[ignore]`；`api-check` 三道不变（通道不是 JS API 成员）
- [ ] 无宿主路径（helix-js 单测 / 一次性脚本）行为不变

## 6. 风险

| 风险 | 缓解 |
|---|---|
| 双状态（宿主 + thread-local）导致重复交付或漏交付 | T3-2 与 T3-3 两条测试钉住；`drain_*` 只走一条路径 |
| `JsHost` 增加 `Receiver`（!Sync）影响 `Send` 约束 | `JsHost` 本来就 `!Send`（含 boa `Context`），不新增约束；但 worker 只拿 `Sender`（`WakeSender: Send+Clone`） |
| 改 `init()` 影响 helix-js 单测（它们大量依赖 thread-local 兜底） | 兜底保留；单测路径不变 |
| 一次性排查成本高（1/5 概率，单次 150s） | T1 只加诊断不改语义，可与其它 pytest 任务并行跑 |

## 7. 任务拆分（确认后细化）

1. **T1** 诊断 + 复现 + 定位（不改行为）
2. **T2** 通道搬进 `JsHost`（含兜底与不变量注释）
3. **T3** 三条测试（跨线程可见 / 不重复交付 / 两 App 不串台）
4. **T4** 把证伪的注释与 spec 链接更新
5. 验收跑：10 次加压 + 全量 + helix-js 单测

## 8. 变更记录

| 日期 | 事项 | 证据 |
|---|---|---|
| 2026-10-19 | 立项；确定性实验证明"跨线程不可见、事件留在原线程" | 探针输出 `on_a=1 on_b=0 on_a2=1`（A 线程 init 后由 B drain） |
| 2026-10-19 | 订正 D3 spec 的"主任务迁移"假设 | `Application: !Send`（持 boa `Context`）⇒ 不能被 tokio 搬线程 |
