# 隐式宿主调用点清点（P4-1 任务 1）

> 2026-10-22。计划见 `docs/superpowers/plans/2026-10-22-explicit-host.md`。
> **临时产物**：任务 4 完成（隐式层删除）后删除本文件；结论并入 `helix-js/src/host.rs` 模块头。

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
