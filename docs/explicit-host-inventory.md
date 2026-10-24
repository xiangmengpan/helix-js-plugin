# "隐式宿主"重构总结（P4-1：3a / 3b / 4a L1–L4）

> 2026-10-22 起，2026-10-22 收口。计划见 `docs/superpowers/plans/2026-10-22-explicit-host.md`。
>
> 本文原是一份**清点表**（"哪些地方在摸隐式宿主"）。重构做完后改成**总结**：
> 目标、分层、每层的实测数字、剩下的边界、以及踩过的坑。
> **仍然有存活价值的判据是每节的"可复跑度量"** —— 后续改动请照它们复核。

## 数字（脚本统计，非手抄）

| 指标 | 数量 | 怎么数 |
|---|---|---|
| `helix-js` 内部隐式宿主访问 | **106** | `grep -c "with_host_ref_or_init\|try_with_host_ref\|with_host_ref\b"`（state.rs 76 / host.rs 22 / 其余 8） |
| 直连隐式宿主的 `pub fn` | **48** | 逐函数体扫描 |
| `helix-term` 调 `helix_js::*` | **197 处 / 95 个函数** | 去掉 `use` 行 |
| 其中"显式优先 + 隐式回落"双路径 | **40** | `None => helix_js::…` |
| `js: None` 构造的 `Context` | **2（src）/ 11（tests）** | `auto_save.rs:88`、`commands.rs:188` |

## 两个计划里没写到的结构事实（这次清点才看清）

### ① 原生函数**拿不到** `&mut JsHost` —— `ACTIVE` 就是为它存在的

boa 的回调签名是 `fn(&JsValue, &[JsValue], &mut Context)`。`Context` 是**从宿主里借出来的**
（`JsHost.engine: RefCell<Context>`）⇒ 回调里不可能再拿 `&mut JsHost`（那要第二次借用宿主）。
所以 `state.rs` 的 76 处隐式访问不是"懒得传参"，是**类型上拿不到**。

**出路**：boa 0.22 有 `Context::insert_data / get_data::<T>()`（宿主自定义数据）。
把宿主句柄（不透明 id 或指针）**存进引擎自己**，原生函数用 `ctx.get_data::<HostHandle>()` 取回。
指针的生命周期随之变成结构性事实（句柄活在引擎里，引擎活在宿主里），而不是"注册期覆盖生存期"的约定。
→ 这是任务 4 的**前置改造**，计划里原来没写，已补进去。

### ② 有 2 个 `js: None` 的 `Context`（src）—— 原因不是线程，是**回调签名不带宿主**

原先的判断（"后台 job 在 worker 线程上，真的没有宿主"）**是错的**，3a 实施时纠正：

- `job::dispatch_blocking` 只是把闭包**投递**到 `JOB_QUEUE`；闭包最终在**主线程**执行
  （`Application` 的循环里 `jobs.handle_callback(...)`）。
- 真正的缺口是 **`EditorCompositorCallback` 的签名只有 `(&mut Editor, &mut Compositor)`** ——
  宿主在 `Application` 里，传不进来，于是调用点只能写 `js: None`，再落回隐式指针。

→ 修法不是"改投递"，而是**把宿主加进回调签名**（已做，见"任务 3a"一节）。
这也让 `ACTIVE` 的存在理由收窄到两类：**原生回调**（引擎借用中）与**测试/同步泵**。

## 分类（按调用方能不能拿到宿主）

| 类 | 判据 | 数量（约） | 处理 |
|---|---|---|---|
| **(a) 有宿主可用** | `Application`（`self.js_host`）或 `compositor::Context.js: Some` | 8 已改（见任务 2）；剩余 ~30 属主路径待改 | 改成宿主方法 |
| **(b) 结构上拿不到** | 原生回调（引擎借用中）、worker 线程 job、布局原语内部 | 106（helix-js 内部）+ 11（job） | ① 原生回调 → 任务 4 的 `insert_data`；② job → 任务 3 改投递；③ 布局原语 → 任务 3 挪到调用方 |
| **(c) 仅测试** | 单测/集成用例里的无宿主路径 | 9 | 保留，收进 `cfg(test)` |

## 任务 3a 实际做了什么（本轮）

`EditorCompositor` 回调（以及 `job::dispatch` / `dispatch_blocking`）的签名加第三个参数
`js: Option<&mut JsHost>`：

- `helix-term/src/job.rs`：类型别名 + `handle_callback(..., js, ...)` + 两个分发入口；
- `Application` 的两个 `handle_callback` 调用点传 `self.js_host.as_deref_mut()`；
- 71 处闭包字面量补 `_js`（按 `EditorCompositor(Box::new(` / `job::dispatch(` 等 opener 精确扫描）
  + 5 处 `let call = move |…|`；
- `auto_save::request_auto_save` 与 `Context::block_try_flush_writes` 不再写死 `js: None`。

**结果：`grep -rn "js: None" helix-term/src` = 0**。剩下的 40 处 `None => helix_js::…`
仍可达（同步泵 `call(..., None)` 与测试），删除它们属于任务 4。

## 任务 2 实际做了什么（本轮）

只做 **(a) 类里最干净的一刀**：`Application` 已经拿着宿主，却绕道线程局部指针去写。

| 调用点 | 改法 |
|---|---|
| `Application::new` 的 4 个写入（`set_plugin_roots` / `set_layouts_dir` / `set_editor_version` / `set_runtime_dirs`） | `js_host.set_*(…)`（宿主方法） |
| `Application::render` 的 `cache_buffers` / `cache_diagnostics` / `snapshot_wanted` | `self.js_host.as_deref_mut()` + 宿主方法，`None` 保留回落 |
| `Application::handle_*` 的 `take_lsp_requests` | 同上 |

- 新增宿主方法 8 个（`helix-js/src/host.rs`）：`cache_buffers` / `cache_diagnostics` /
  `snapshot_wanted` / `set_plugin_roots` / `set_layouts_dir` / `set_editor_version` /
  `set_runtime_dirs` / `take_lsp_requests`。
- **`None` 分支一个没删**：现在还不能证明"宿主恒在"（见发现 ②）。删它们属于任务 3/4，
  前置条件是 job 投递化 + 原生回调的 `insert_data`。

## 下一轮要改的先决顺序（对计划的修正）

1. ~~任务 3a：job 回调带上宿主~~（**已完成**，见上）。
2. **任务 3b**：布局原语事件挪到调用方（原计划的任务 3）。
3. **任务 4a**：boa `Context::insert_data` 放宿主句柄，原生回调改走它（**新发现的前置**）。
4. **任务 4b**：删 `ACTIVE` / `set_active` / `clear_active` / `park_or_forget`，
   把 `js_host` 从 `Option<Box<…>>` 变成"Drop 时不需要停放"的普通所有权。

## 附：一个与本次无关的既有 flake

`test::commands::write::test_overwrite_protection`：全量并行跑时偶发失败，
**隔离跑 5/5 通过**（本次实测）。它在 2026-10-21 的 P3-2 全量跑里也失败过（早于 P4-1 改动）⇒
既有测试隔离问题，不是本次引入。

---

## 任务 3b 的实测尺寸（2026-10-22，**未落地，已回退**）

计划里写的是"把布局事件从 `sync_layout_cache` 内部挪到调用方"。实测：

| 事实 | 数字 |
|---|---|
| 调 `sync_layout_cache()` 的 `Compositor` 方法 | **27 个**（每个 1 处） |
| 这些方法的全仓调用点 | **约 78 处**（`compositor.rs` 内部 26、`ui_requests/ui_op.rs` 18、`ui/plugin_*` 6、测试 17，其余在 `ui/layout.rs` —— 那里是 `LayoutTree` 的**同名方法**，不需要改） |
| 级联方向 | 从布局原语**向上**传到 `Compositor::handle_event` 与 `window_mode_*` 这层输入 API |

两次机械改法都试过并**已回退**：

1. 第一版脚本处理零参数调用时把 `self.sync_layout_cache()` 改成 `self.sync_layout_cachejs)` —— 直接改坏语法。
2. 第二版修好扫描器后，签名 + 调用点改到 27 个方法，剩余报错集中在**上游**函数
   （`handle_event` / `adopt_orphan_views` / `window_mode_swap` / `window_mode_resize_signed` /
   `window_mode_minimize` / `window_mode_zoom`）—— 给它们补参数又会让它们的调用点报错，
   级联到整个 `Compositor` 输入面。

### 由此得到两条可选路线（需要决策）

- **(a) 全量线程化**：给 27 个方法 + 6 个上游函数加 `js` 参数，约 **90 处**改动；
  语义**完全不变**（事件仍在变更点同步发、处理器仍看到当时的布局）；
  纯机械但要一轮完整验证，且 `LayoutTree` 必须**保持**不认识 JS（几何层不该知道宿主）。
- **(b) 改成"意图队列 + 按请求 flush"**：`sync_layout_cache` 只写快照 + 记一个变更点，
  由 `apply_ui_requests`（手里有宿主）在每个请求后 drain。代码只动 ~40 行，
  但**语义变化**：同一个请求内的多个变更点会都看到"该请求结束时的布局"。
  现有逐点用例只断言"窗格数非递减且首<尾"，所以**仍会绿**——正因如此，
  这会是一次**测试覆盖不到的行为变化**，必须显式确认（`js_layout_change_batch` 开关也正好一起删，
  审计里本来就建议删它）。

> 我的建议：如果"处理器看到变更点当时的布局"是契约，选 (a)；如果接受 (b) 的语义并愿意把
> 逐点用例改成"窗口内合并"，选 (b)。两者都不该在没有明确决定的情况下混着做。

---

## 3b 的三次尝试（2026-10-22）—— 全部回退，结论：**不可脚本化**

用户选了 (a) 全量线程化。三次机械改法，三次都改坏，全部回退：

| 尝试 | 做法 | 失败原因 |
|---|---|---|
| 1 | 正则给闭包补参数 | 误伤 89 个普通闭包（`retain`/`sort_by`） |
| 2 | 只改 `EditorCompositor(Box::new(` 之后的闭包 | 不适用于 3b（这里没有闭包） |
| 3 | 确定性扫描：35 个签名 + 136 个调用点，按"就近 fn 签名"决定传 `js` / `cx.js.as_deref_mut()` / `None` | **闭包内外的上下文判错**：`ui_op.rs` 的闭包是 `Send`（`dispatch_blocking`），却被塞了 `cx.js…` ⇒ 捕获非 `Send` 的 `cx`；以及 `plugin_terminal.rs` 里同一个方法的调用被重复追加三次 |

### 根因（值得记住）

判"这个调用点能不能拿到宿主"不是文本问题，是**作用域问题**：同名方法在
`LayoutTree`（几何层，绝不能拿宿主）和 `Compositor`（UI 层，有宿主）上都有；
`cx` 在**外层函数**里可见 ≠ 在**内层闭包**里可用（闭包可能是 `Send`，`cx` 不是）。
正则/文本扫描无法可靠回答这两种作用域问题。

### 因此 3b 只能**逐层手工**做，并按层提交

**第 1 层已完成（2026-10-22）**：`sync_layout_cache(js: Option<&mut JsHost>)` ——
27 个调用点**各自显式声明**它能不能给宿主：

- 3 处传 `cx.js.as_deref_mut()`（所在函数本来就有 `cx`：`pane_mode_key` / `pane_cycle_or_stack` / `window_mode_close`）；
- 24 处传 `None`（所在函数没有宿主可传，**行为不变**——内部仍走临时回落，第 3 层再删）；
- 顺便发现 `ui_op.rs::UiOp::apply(self, editor, compositor)` **根本没有宿主参数** ⇒
  下一层给它加 `js`，那 10 处就能一起迁。

第 1 层验收：`0` 编译错误、413 集成全绿、`plugin_layout` 逐点/批量 2 条用例绿。


建议顺序（每层独立提交、每层跑全量）：

1. **只被 `ui_op.rs` 与测试调用**的方法（无上游级联）——大约 10 个，先做这批；
2. 级联到 `handle_event` / `window_mode_*` 的那批（8 个上游函数）；
3. 最后改 `sync_layout_cache` 里的 `emit_layout_change_kind` → `host.emit_layout_change(kind)`，
   并删掉"只能走隐式解析"的那段注释。

每层的验收：`cargo build -p helix-term` 0 error + 413 集成全绿 + `plugin_layout` 的
逐点/批量两条用例（它们钉的正是这里的行为）。

---

## 3b 完成（2026-10-22，四层，四个提交）

| 层 | 提交 | 内容 | 验收 |
|---|---|---|---|
| 1 | `adf0b6c1c` | `sync_layout_cache(js)`；27 个调用点显式声明（3 处 `cx.js`，24 处 `None`） | 0 error / 413 绿 |
| 2 | `f115d6786` | `UiOp::apply` 带上 `js`；10 处 `None` 转正 | 0 error / plugin_layout 绿 |
| 3 | `a0b99a8df` | 24 个布局原语 + 5 个上游函数带 `js`；全部调用点给参 | 同上 |
| 4 | `a0b99a8df` | `None` 分支改为**入队**，帧首 `flush_layout_change(cx.js)` 补发；不再调 `emit_layout_change_kind` | 同上 |

**结果**：`grep -rn "sync_layout_cache(None)" helix-term/src` = **0**；
布局路径上不再触达隐式活动指针。

### 这一轮踩的坑（都是"同名方法"与"闭包作用域"）

1. `LayoutTree`（`ui/layout.rs`）有同名方法 `set_float`/`unzoom`/`split_leaf` ⇒ 脚本按方法名匹配
   会把几何层的调用也加上宿主参数。**必须按接收者 `self|compositor` 锚定，并排除 `main_tree`/`tree`。**
2. `dispatch_blocking(move |editor, compositor, _js| …)` 里的调用要用 `_js`，
   用外层函数的 `cx` 会让闭包捕获非 `Send` 的 `cx`（编译错误：`DynAccess<Config> cannot be sent`）。
3. `js: Option<&mut …>` 参数要写 `mut js`（`as_deref_mut()` 需要可变借用），但别重复写（`mut mut`）。

### 剩余（P4-1 的 4a/4b）

`ACTIVE` 仍然存在，服务于**原生回调**（`Context` 从宿主借出，回调里拿不到 `&mut JsHost`）
与测试。4a（`Context::insert_data` 放句柄）与 4b（删 `ACTIVE`）是另一层重构，
规模不小于 3b，需要单独一轮。

---

## 4a 可行性验证（2026-10-22）—— 机制成立，规模比 3b 大

### 结论先说

`Context::insert_data / get_data` 这条路**可行且不需要 `unsafe`**：宿主状态用
`Rc<HostState>` 存进引擎，原生函数 `ctx.get_data::<Rc<HostState>>()` 取回 ——
**不比线程局部指针多一次解引用之外的成本**，反而把"指针活得比宿主久"的约定
变成类型关系（`HostState` 由 `Rc` 持有，引擎里的那份就是它的一个引用）。

最小原型（40 行，已验证后删除）：

```rust
#[derive(Default)]
struct HostState { hits: RefCell<u32> }

fn js_ping(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let st = ctx.get_data::<Rc<HostState>>().expect("必须由 JsHost::new 注入");
    *st.hits.borrow_mut() += 1;
    Ok(JsValue::undefined())
}

let state = Rc::new(HostState::default());
let mut ctx = Context::default();
ctx.insert_data(Rc::clone(&state));      // ← 宿主建引擎时注入
// …注册原生函数、eval("helix.ping(); helix.ping()")…
assert_eq!(*state.hits.borrow(), 2);     // ✓ 实测通过
```

### 规模（脚本统计，非估算）

| 项 | 数量 | 改动形态 |
|---|---|---|
| `JsHost` 字段 | **62**（1 个 engine + 61 个状态） | 拆成 `JsHost { engine, state: Rc<HostState> }`，字段搬到 `HostState` |
| 访问器（函数体直接触达隐式宿主） | **49** | 签名加 `st: &HostState` |
| 访问器调用点（生产代码，helix-js 内） | **60** | 其中 **32（53%）所在函数已有 `ctx`** ⇒ 一行 `ctx.get_data()`；**28** 需要把 `&HostState` 透传进来 |
| `with_engine(` 调用点 | **63** | 这些只能拿 `&mut JsHost`（原生回调里引擎已借出，不能调它） |
| 直接 `Context::default()` 的测试 | **8** | 4 处（watch/lsp/diagnostics 的单测）需要改成经 `JsHost::new` 或用测试助手注入 `HostState` |

合计约 **180 处编辑**（3b 是 ~140），分布两个 crate。

### 建议的落地分层（每层独立提交）

1. **L1 形状**：引入 `HostState`（先只放 3–5 个字段）+ `JsHost { engine, state }` +
   `insert_data(Rc::clone)`；再加一个 `with_state(ctx, f)` 助手，**迁一个访问器**做真实闭环。
   验收：这一个访问器的调用点全部走 `ctx.get_data()`，`ACTIVE` 仍存在但不再被它使用。
2. **L2 搬字段**：把 61 个字段搬进 `HostState`（编译器驱动，纯机械）。
3. **L3 迁访问器**：49 个访问器分 3 批（state.rs 内部 → 各模块 → 边角），
   每批 15–20 个；每批跑 `cargo test -p helix-js --lib`。
4. **L4 删 `ACTIVE`**：`DEFAULT_HOST`/`set_active`/`clear_active`/`park_or_forget` 全部删除；
   8 处单测改成 `JsHost::new()`（它自己会 `insert_data`）。
   > **实际执行结果见文末「总结」**：L4 没有照这条删 thread_local ——
   > ①`Drop`/跨线程/每帧 UI 这三类**结构上**需要环境句柄，实测它们的正确形态是
   > "具名捕获入口 + 显式 API"而不是删除；②跨线程那批**根本不需要** `Arc`/`Mutex` 化。

### 风险（比 3b 高的地方）

- **测试要改**：现在单测可以裸建 `Context::default()`；改完必须经 `JsHost::new` 注入状态，
  否则 `get_data` 返回 `None` ⇒ 需要 `with_state` 明确 panic 并给出原因（不要静默）。
- **`park_or_forget` 语义**：宿主在 Application 之间停放/接管；`Rc<HostState>` 随引擎走，
  这点比裸指针**更安全**（引擎到哪，状态到哪）。
- **`with_engine` 的 63 处**：它们是"需要引擎"的调用点，不能靠 `get_data` 解决
  （原生回调里引擎已借出）⇒ 这些函数继续接收 `&mut JsHost`，属于 3a/3b 已铺好的路。

---

## 总结（重构完成）

### 一句话

原来"引擎"是一段 thread_local 状态（`ACTIVE` + `DEFAULT_HOST`），**没有名字、无法指名、
无法多份**；重构后它是一个**显式对象** `JsHost { engine, state: Rc<HostState> }`，
状态由 `&HostState` / `&mut JsHost` 传递，thread_local 只剩两个**受控**用途。

### 分层与提交

| 层 | 内容 | 代表提交 |
|---|---|---|
| **3a** | job 回调带上宿主（生产代码里不再有 `js: None`） | `5c229643d` |
| **3b** | 布局路径四层：`sync_layout_cache(js)` → `UiOp::apply(js)` → 布局原语(js) → 帧首批量 | `adf0b6c1c` / `f115d6786` / `a0b99a8df` |
| **4a L1** | 引入 `HostState` + `Context::insert_data` 注入引擎 + 迁一个访问器闭环 | `fa41a6bd2` |
| **4a L2** | 61 个字段搬进 `HostState`（`JsHost` 只剩 `engine` + `state`） | `588291b2f` |
| **4a L3** | 40 个访问器显式化（4 批），调用点分三种形态（`ctx` / `st` / 具名捕获） | `b71c2ff9d` … `ba3f87474` |
| **4a L4-③** | UI 闭包捕获状态：`RenderContext.state`、渲染路径、`Drop` 例外 | `81048ee9c` … `1e9d949a4` |
| **4a L4-④** | `pump()` 把 `&HostState` 交给闭包；34 个 pump callee 全部显式 | `8e145dc17` / `a076c3dbd` … `9441f3104` |

### 收口数字（都可复跑）

```bash
# ① helix-term 生产代码里还有多少"隐式取态"调用 ⇒ 0
grep -rn "_current(" helix-term/src --include=*.rs | grep "helix_js::" | grep -v tests | wc -l

# ② pump 作用域内还有多少隐式访问(遍历 34 个 callee 体内的 *_current/with_state_or_init/host_state)⇒ 0
#    (脚本见 `9441f3104` 的提交正文)

# ③ 跨线程路径是否触达 HostState ⇒ 0(9 个 thread::spawn 闭包,生产代码)
```

| 指标 | 重构前 | 重构后 |
|---|---|---|
| `JsHost` 上的状态字段 | 61 | **0**(全在 `HostState`) |
| 显式访问器 `fn xxx(st: &HostState, …)` | 0 | **84** |
| helix-term 生产代码隐式取态 | 197 处调用(其中 40 处双路径) | **0** |
| pump 作用域内隐式访问 | 32 | **0** |
| 跨线程触达宿主的 spawn 闭包 | 0(评估确认无需改造) | 0 |

### `ACTIVE` / `DEFAULT_HOST` 现在只剩什么用

1. **引擎已借出的原生回调**：boa 的回调签名是
   `fn(&JsValue, &[JsValue], &mut Context)`，`Context` 从宿主里借出 ⇒ 回调里拿不到
   `&mut JsHost`。这类地方走 `with_engine(…)` / `host_state(ctx)`。
2. **`pump()` 自身的作用域**：把 `ACTIVE` 设为该次调用内有效（嵌套时恢复），
   让 callee 内部那些仍走隐式解析的路径（引擎相关）能工作。

除这两类外，状态一律**显式传递**。少数"结构上没有 `Context`"的边界用**具名捕获入口**
`host::state_handle()` / `state_handle_or_init()` —— 生产代码共 **29 处调用**
（`grep -rn "state_handle" helix-term/src helix-js/src | grep -v tests`），
集中在 `application.rs`(12) 与 `compositor.rs`(10)，其余 7 处（`typed.rs` / `ui/editor.rs` /
`ui/js_picker.rs` / `ui/js_surface.rs` / `ui/plugin_terminal.rs` / `ui/statusline.rs` /
`helix-js/src/commands.rs` 各 1），另有 `host.rs` 的 2 处定义。上面那条 grep 会把定义与注释
一起数到 **34**，**调用点是 29**（2026-10-23 复核）。
关键不在数量而在**形态**：全部是"一行捕获 + 显式 API"，没有一处是"在访问器里各自摸 state"。

### 踩过的坑（值得留给下一轮）

1. **`Rc<HostState>` 不能塞进 `Send` 构造的组件**：`PluginTerminal` 在
   `dispatch_blocking` 的 `Send` 闭包里构造 ⇒ 捕获 `Rc` 会让它 `!Send`（编译错误作证）。
   ⇒ `Drop` 改用"drop 时捕获一次"。
2. **`with_js` 返回 `None` ≠ "没有 JS"**：它只表示"当前 `Context` 没带宿主"，
   环境宿主可能仍然存在（面板布局期渲染就是这一类）。把这些分支改成 `Err` 会让
   `filetree_*` 等 5 条集成用例直接红。
3. **批量正则改调用点会命中函数定义**：`pub fn xxx(` 里的 `xxx(` 会被同一条规则替换，
   造成签名污染与括号失衡。**可靠做法**：先改签名 → 让编译器列出全部调用点 →
   按行号逐个改（前几批用的就是这条）。
4. **`RefCell` → `Mutex` 的诱惑**：曾担心"跨线程需要 `Arc<HostState>` + `Mutex` 化"，
   实测**不需要**——worker 只用 `mpsc` 通道与线程局部表，从不碰 `HostState`。
   顺手也免掉了"重入 panic 变静默死锁"这个更贵的风险。
5. **同名方法/宏参数会误导脚本**：`LayoutTree` 有同名 `set_float`/`unzoom`；
   测试宏里带 `st` 参数时批量替换尤其危险。

### 仍然存在的边界（都不是"隐式"，是"没有 `Context`"）

| 位置 | 为什么 | 用什么 |
|---|---|---|
| `ui/js_surface::refresh_global` | 布局期（`required_size`）拿不到 `&mut Context` | `state_handle_or_init()` |
| `ui/plugin_terminal.rs::drop` | `Drop::drop(&mut self)` 无参数 | `state_handle()` |
| helix-term `with_js` 的 `None` 分支 | 当前 `Context` 未带宿主 | `state_handle_or_init()` |
| `helix_js::load_script` | 测试/内联入口，无宿主句柄 | `state_handle_or_init()` |

以上四类都是**单点捕获**（`grep state_handle` 一网打尽），而不是"每个访问器各自摸 state" ——
这是重构真正的收益：**模糊的隐式解析被压缩成一个具名入口 + 两类受控 thread_local**。
