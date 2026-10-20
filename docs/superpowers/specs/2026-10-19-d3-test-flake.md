# D3 · 并行下的测试 flake —— 规格

> 状态：**待确认**（按§2 铁律 5，C 类改动先出 spec，确认后才动代码）。
> 本文只写"是什么、为什么、怎么改、怎么验收"，不写实现。计划拆任务在确认后另出。

## 1. 现象（可复现）

`cargo integration-test` 在默认线程数下偶发红；把测试用**大字符串过滤**塞进一个进程时更明显。

**复现命令（136s 稳定复现 4 条）**：

```bash
BIN=$(ls -t target/debug/deps/integration-* | grep -v '\.d$' | head -1)
"$BIN" plugin --test-threads=64
# => 165 passed / 4 failed / 136.61s
```

**默认线程数下的另一次**：`cargo test -p helix-term --features integration --test integration plugin`
⇒ 162 passed / **7 failed** / **899s**（平时同类模块单独跑合计约 60s）。

**单跑全部绿**：`plugin_lsp_mock` 13 条、`plugin_terminal_hooks` 27 条、`plugin_draw`、`plugin_async` 均通过。

## 2. 四条失败与真实报错（一次复现即得，非推测）

| 用例 | 报错 | 归类 |
|---|---|---|
| `plugin_async::plugin_async_run` | `get_status().unwrap()` on `None`（`plugin_async.rs:43`） | **A** 异步未完成就断言 |
| `plugin_terminal_hooks::picker_ui_opens` | `pick-diag: … all=0 shown=0` ⇒ 断言"应取到文件条目" | **A** |
| `plugin_draw::picker_ui_draws_with_draw_api` | picker 只有表头、无条目 ⇒ 滚动位置断言失败 | **A** |
| `plugin_cross_buffer::plugin_by_path_relative_resolves_from_cwd` | `Error: No such file or directory (os error 2)` | **B** 进程级 cwd |

## 3. 根因

### A · "idle" ≠ "异步工作都回来了"（3/4）

`Application::event_loop_until_idle` 在 integration 特征下的返回条件是**收到一次 `IdleTimer`**：

```rust
EditorEvent::IdleTimer => { self.handle_idle_timeout().await;
    #[cfg(feature = "integration")] { return true; } }
```

而 integration 循环在**每个事件之后**都 `reset_idle_timer()`，定时器是 `sleep(idle_timeout)`（默认 **250ms**）。
所以"idle" 的语义是 **"250ms 内没有事件"**，不是"worker 都干完了"。

被测的产物来自**另一条线程/任务**：`helix.run_async` / `spawn` / `read_tree` / `helix.lsp.*`
⇒ 结果什么时候回到主线程，与"有没有事件"无关。测试紧接着 `get_status().unwrap()` 就是在赌
"250ms 足够"，机器一忙（64 线程 / 16 核）就赌输：`None` 或 `all=0`。

证据：同一批用例在空载下 100% 通过；加压后失败点全部落在"读状态栏"的断言上，且**没有一条**
报"值不对"，都是"还没到"。

### B · cwd 是进程级全局，锁只保护写者（1/4）

- `:cd` 真改进程 cwd（`helix-view` 的 `apply_directory_change` 走 `helix_stdx::env::set_current_working_dir`）；
- `helix_stdx::env` 还带**缓存** ⇒ 不只真实 cwd，连 `helix.cwd()` 读到的也是全局值；
- 现有 `CWD_LOCK`（`helpers.rs:584`）只让**会 chdir 的用例**互相串行，
  但**读 cwd 的用例**（相对路径 `by_path`、`:open <相对路径>`、`helix_loader::find_workspace`）
  没有拿锁 ⇒ 别人 chdir 的窗口里它们会读到错的目录，甚至读到**已被删除的临时目录**（ENOENT）。

已确认：三处 `:cd` 用例（`plugin_cwd` / `plugin_cross_buffer` / `plugin_draw`）**都**拿了锁；
所以剩下那条 ENOENT **不是"忘了加锁"**，而是"写者持锁期间读者仍在读"这一类。
⇒ 需要先定位到具体 `?` 站点（下一步第 1 项），不能靠猜。

## 4. 方案

### A1 · 新增有界等待助手 `pump_until`（核心）

```rust
/// 发键后**等到条件成立**（有界），而不是"等到 idle"。
/// 超时失败时打印：等了多久、最后一次状态栏、失败条件。
pub async fn pump_until(
    app: &mut Application,
    keys: &str,
    budget: Duration,          // 默认 5s
    cond: impl Fn(&Application) -> bool,
) -> anyhow::Result<()>;
```

语义：发键一次 → 循环 `event_loop_until_idle` 直到 `cond` 为真或超时。
必须避免忙等（`event_loop_until_idle` 本身已在等定时器/事件）。

**判据（写给后续所有测试）**：断言的值若来自
`run_async` / `spawn` / `read_*_async` / `stat_async` / `glob_async` / `read_tree` /
`helix.lsp.*` / `helix.server.rows|task` 的**回调**，就必须用 `pump_until`，不许用 `pump` + `unwrap`。

### A2 · 迁移清单（只搬"结果来自异步"的断言）

| 文件 | 现象 | 处理 |
|---|---|---|
| `plugin_async.rs` | 3 条命令各读一次状态栏 | 改 `pump_until(app, ":async-demo<ret>", 5s, |a| status 含 "async:")` |
| `plugin_lsp_mock.rs` | **9 处** `get_status().unwrap()`（LSP 响应经 promise 回来） | 同上；超时预算 5s（mock 本地响应，正常 <50ms） |
| `plugin_terminal_hooks.rs::picker_ui_opens` | `all=0` | 等 `pick-diag` 的 `all>0` |
| `plugin_draw.rs::picker_ui_draws_with_draw_api` | picker 无条目 | 等首行出现再断言滚动位置 |

范围量化：全仓测试里 `get_status().unwrap()` **127 处**，其中**只有 9 处**跟着异步来源
（其余读的是命令的同步 echo）。所以本项不是"全仓重写"，是**按判据挑 12–15 处**。

### B1 · 先定位，再决定（不要先改）

1. 给该用例里所有 `?` 站点加 `.context("…")`（`current_dir` / `tempdir` / `AppBuilder::build` / `:open`），
   一次复现即可看出到底是 syscall 还是文件缺失。
2. 若是 `std::env::current_dir()` 失败 ⇒ 根因是"cwd 指向已删目录"，
   修法：`CwdGuard` 里在**恢复前**校验 prev 仍存在，并在失败时把 prev 打印出来（诊断优于重试）。
3. 若是"相对路径解析错" ⇒ 让**读侧**也拿 `CWD_LOCK`（只有 3 个文件、4 个用例需要），
   或直接改成绝对路径（优先：不引入新锁竞争）。

**原则**：`CWD_LOCK` 的正确范围是"任何依赖进程 cwd 的代码路径"，不只是"会 chdir 的测试"。
若读完发现读侧用例 >10 个，则改用"测试进程内 cwd 永不改变"的方案
（`CwdGuard` 只在持锁时短暂切换，其它用例全部用绝对路径）——**但这是 B2，需另议**。

### A3/B1 的替代方案（评估过，不选）

| 方案 | 否决理由 |
|---|---|
| 调大 `idle_timeout`（如 2s） | 400 条用例各多等一次 ⇒ 全量 +数百秒；且掩盖问题，机器更忙时照样红 |
| `--test-threads=1` | 136s → 约 600s；CI 成本翻四倍，且掩盖并发缺陷 |
| 失败自动重试 | 把真 bug 洗成 flake；仓库铁律不允许 |
| 调大 LSP per-request timeout（20s） | 不是超时问题：请求根本没发出去/根本没回来，测试侧等太短 |

## 5. 验收标准

1. 复现命令 `"$BIN" plugin --test-threads=64` **连跑 5 次全绿**（当前 1 次即红 4 条）。
2. `cargo integration-test`（默认线程，397 条）**连跑 3 次全绿**。
3. `pump_until` 的 docstring 写明第 4 节的判据，并在 `docs/CONTRIBUTING.md` 的测试小节加一行指路。
4. 超时路径**被验证过**：故意让某条用例等一个不成立的条件 ⇒ 失败信息里能看到
   "等了 Xs / 最后状态栏 / 期望什么"（而不是 `Option::unwrap` panic）。
5. 不新增 `#[ignore]`；不减少任何现有用例。

## 6. 影响面与风险

| 风险 | 缓解 |
|---|---|
| `pump_until` 条件永不成立 ⇒ 每条用例多等 5s | 预算按来源分级：进程/fs 5s、LSP mock 5s；超时报错信息足够定位 |
| 循环里 `event_loop_until_idle` 在条件已满足时仍空转一圈 | 进循环前先判一次 |
| 迁移时手滑改坏断言 | 每条迁移**只换等待方式，断言字符串一字不改**；对照 git diff 评审 |
| B1 定位不出来的话卡住 | 该项独立成任务，允许单独延后（A 类不依赖它） |

## 7. 任务拆分（确认后细化成 plan）

1. **T1** `pump_until` + 自测（含超时路径）——先做，A2 依赖它
2. **T2** 迁移 `plugin_async`（3 处）→ 复现命令验证
3. **T3** 迁移 `plugin_lsp_mock`（9 处）→ 复现命令验证
4. **T4** 迁移 `picker_ui_opens` + `picker_ui_draws_with_draw_api`（2 处）
5. **T5** B1 定位（加 context → 复现 → 定位到站点）+ 按结论修
6. **T6** 文档（判据 + CONTRIBUTING 一行）+ 5 次/3 次验收跑

## 8. 待确认的决策点

| # | 问题 | 建议 |
|---|---|---|
| Q1 | 异步等待预算取 5s 够吗？（CI 上最慢的 fs 用例当前耗时 ~1.2s） | 取 5s；超时只影响失败诊断，不影响通过路径耗时 |
| Q2 | B 类修法选"读侧加锁"还是"读侧改绝对路径"？ | **绝对路径**优先（不动锁），只有改不动的地方才加锁 |
| Q3 | 是否顺手把 64 线程加压跑加进 CI？ | **不加**（CI 是 16 线程口径，加了只会让 CI 慢且偶发红）；改为在本文记录命令，供本地排查 |

---

## 9. 执行结果与订正（2026-10-19，T1–T3 已落地）

### 9.1 做了什么

| 任务 | commit | 结果 |
|---|---|---|
| T1 | `72d993a92` | `helpers.rs` 新增 `pump_until` / `pump_until_probe` / `status_text`；自测 4 条（通过路径耗时、超时诊断、探针重发、空串语义） |
| T2 | `b26db17f3` | `plugin_async` 两条命令改 `pump_until`（断言字符串未改） |
| T3 | `94a529701` | `plugin_lsp_mock` 9 条用例改 `pump_until`；顺带收掉同文件两处手写重试 |

### 9.2 复现命令 5 次结果（`--test-threads=64`，`plugin` 过滤）

```
run1 ok  170 passed / 0 failed / 180s
run2 ok  170 passed / 0 failed / 145s
run3 ok  170 passed / 0 failed / 140s
run4 ok  170 passed / 0 failed / 149s
run5 FAILED  169 passed / 1 failed / 150s   ← plugin_async::plugin_async_run
```

基线（改动前）：`165 passed / 4 failed`（另一次默认线程：`162 / 7 / 899s`）。

### 9.3 两个订正（原判断有误）

1. **两条 picker 用例属 B 类（cwd），不是 A 类（异步）。**
   `picker_ui_opens` 的 `all=0` 与 `picker_ui_draws_with_draw_api` 的空表，根因是
   picker 的 `files` 源读**进程 cwd**：并行时别的用例 `chdir` 走了（甚至删了临时目录），
   枚举自然为空。**证据**：另一会话的 `CwdScope`（scoped chdir + 只读锁）落地后，
   T1–T3 未碰这两条用例，它们也不再失败（5 次全绿）。
   ⇒ **T4 取消**：给它们加 `pump_until` 只会把 cwd 竞争藏进 5s 等待里（更坏）。

2. **`plugin_async_run` 的残留失败不是"等得不够"，是事件丢了。**
   该文件头部早有记录：multi_thread 下测试主任务会迁移 OS 线程，而
   `ASYNC_EVENTS` / `LSP_RESULTS`（`helix-js/src/state.rs:808-816`）是 **thread-local**，
   drain 发生在新线程上 ⇒ 事件留在旧线程的通道里，5s 也等不到。
   `pump_until` 修的是"晚到"，修不了"丢失"。⇒ 单列为 **T7（未做）**。

### 9.4 T7（已定位，见 [`2026-10-19-t7-event-loss.md`](2026-10-19-t7-event-loss.md) §9）

| 选项 | 代价 / 风险 |
|---|---|
| a. 把 `ASYNC_EVENTS`/`LSP_RESULTS` 由 thread-local 改进程级 | 修根因；但同进程多 App（集成测试）会串台，需要配合现有的 `TEST_LOCK` 语义重新设计所有权 |
| b. 测试侧把 `App` 钉在固定线程（专用 runtime + `spawn_blocking` 驱动） | 不动产品代码；但 harness 大改，且 `close()` 依赖 `block_in_place` |
| c. 接受 1/5 级别残留，改成"失败信息明确指向 T7" | 成本最低；但验收标准"5 次全绿"达不到 |

**结论（2026-10-20 实测）**：不是通道串线程，而是**进程 worker 线程在高倍超订下跑不完**
（5 个 worker 只有 3 个 `send Exit`，无 `spawn FAILED`）⇒ 选 c，产品侧不改；
"通道搬进宿主"撤回。详见 T7 spec §9。

### 9.5 验收对照

- [x] `harness_util` 4 passed（含超时诊断）
- [x] `plugin_async` / `plugin_lsp_mock` 单独跑全绿
- [x] `--test-threads=64` 5 次中 **4 次**全绿（原 4 条已知 flake 中 3 条消失）
- [ ] 5 次全绿 —— 被 T7 阻塞（见 9.4）
- [x] `git diff` 无任何 `assert_eq!` 期望字符串改动
- [x] 无新增 `#[ignore]`，用例总数 397 → 401
