# G12（C 批）· `job::dispatch_blocking` 闭包 → `UiOp`(pane 族子集)

日期:2026-10-17
状态:**待审查**(铁律 5:批准前不动代码)
关联:[`../architecture-audit.md`](../../architecture-audit.md) §5 G12 · G7(已完成,`apply_ui_requests` 689 → 59 行)

## 0. 决策记录(需要你拍板的四点)

1. **只做一个子集:pane 族(24 个 handler)**。全仓 `job::dispatch*` 75 处,其中 `ui_requests/pane.rs` 占 **24**、`term.rs` 8、`doc.rs` 5、`editor.rs` 2、其余各 1。pane 族是「同构、纯 compositor 变更、且已有 16+ 条集成测试」的那一族 —— 收益最大、风险最小。
2. **保持「一个请求一次 dispatch」**。当前每个 arm 各自 `job::dispatch_blocking`;本项**不合并**成批(合并会改变与其他 UI 工作的相对时序,属行为变化,铁律 2 不允许)。
3. **`UiOp` 只描述数据,不装 `Box<dyn Component>`**。组件构造放在 `apply` 里,这样 op 能 `derive(Debug, PartialEq)`、能被单测直接断言(否则就要 `Application`)。
4. **不动 `UiRequest` 枚举**(92 变体 / 16 文件 match)—— 沿用 G7 已记录的「不做」。

## 1. 目标

把 pane 族的「UI 变更」从**不透明闭包**变成**可断言的值**:

- 之前:`fn handle_zoom_leaf(req) -> Result<()>` 里塞 `job::dispatch_blocking(move |_e, compositor| compositor.zoom_leaf(id))` —— 想验证「这个请求到底要做什么」只能起一个 `Application`。
- 之后:`fn handle_zoom_leaf(req) -> Result<Option<UiOp>>` 返回 `Some(UiOp::Zoom { id })`,由分派处统一执行 —— 单测直接 `assert_eq!`。

## 2. 现状与约束(已核实)

### 2.1 pane.rs 的 24 个 dispatch 点(按闭包体分类)

| 闭包体(行数) | 用的东西 | 对应 handler |
|---|---|---|
| 13 行 | `compositor.register_panel` / `split_leaf_with_ratio` | `handle_open_panel`(含 rail 判定) |
| 5 行 | `compositor.close_panel_by_id` | `handle_close_panel` |
| 4 行 | `compositor.set_panel_side` + `editor.set_error` | `handle_move_panel` |
| 16 行 | `split_leaf_prealloc` + `set_layout_change_kind` + `sync_layout_cache` | `handle_split_leaf`(在闭包里构造 `PluginTerminal`/`PluginPanel`) |
| 4 行 | `close_leaf_clean` + kind + sync | `handle_close_leaf` |
| 5 行 | `set_float` + kind + sync | `handle_pane_float` |
| 10 行 | `layout_tree().stack_new_with_sibling` 等 + kind + sync | `handle_stack_create` |
| 14 行 | `layout_tree()` 轮转 | `handle_stack_activate` |
| 4 行 ×4 | `layout_tree().stack_*` / `raise/pin/embed` | `handle_stack_remove` / `pane_raise` / `pane_pin` / `pane_embed` |
| 2 行 ×7 | `zoom_leaf` / `unzoom` / `resize_leaf` / `minimize_leaf` / `set_leaf_fixed` / `equalize_leaf` / `focus_leaf` | 同名 handler |
| 4 行 ×4 | `resize_leaf_dir` / `swap_leaves` / `focus_leaf_dir` / `swap_leaf_dir` + `editor.set_error` | 同名 handler |
| 11 行 | `layout_tree().restore(&json)` + `sync_editor_focus` / `editor.set_error` | `handle_cache_layout` |

合计 **24 个 dispatch / 约 190 行闭包体**;其中 8 个用到 `editor`(全部是 `set_error` 或 `sync_editor_focus`)。

### 2.2 派发语义(必须保持)

- `job::dispatch_blocking` 的闭包在**事件循环的下一轮**拿 `(&mut Editor, &mut Compositor)` 执行。
- 一个请求 = 一次 dispatch = 一次闭包执行;顺序即请求顺序。
- `set_layout_change_kind(..)` 必须在紧随的 `sync_layout_cache()` **之前**调用(否则 `layout-change` 事件的 `kind` 为 null —— 这是 [`../../plugin-api.md`](../../plugin-api.md) §4 记录过的坑)。

## 3. 目标架构

新文件 `helix-term/src/commands/typed/ui_requests/ui_op.rs`(预估 220–260 行):

```rust
/// 一次 UI 变更的**描述**(不是闭包)。
///
/// 为什么是值:handler 可以返回它 ⇒ 单测直接断言「这个请求要做什么」,
/// 不必起 Application;执行点收敛到 `apply` 一处,compositor 的调用顺序可见。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum UiOp {
    OpenPanel { id: u64, side: PanelSide, size: u16, focusable: bool, rail: bool },
    ClosePanel { id: u64 },
    MovePanel { id: u64, side: PanelSide },
    SplitLeaf { id: u64, dir: SplitDir, first: bool, size: u16, kind: String },
    CloseLeaf { id: u64 },
    SetFloat { id: u64 },
    StackNewWithSibling { id: u64 },
    StackRotate { id: u64, member: u64 },      // 原 stack_activate
    StackRemoveMember { id: u64 },
    RaiseLeaf { id: u64 },
    PinLeaf { id: u64, on: bool },
    EmbedLeaf { id: u64 },
    Zoom { id: u64 },
    Unzoom,
    ResizeLeaf { id: u64, ratio: f64 },
    ResizeLeafDir { id: u64, dir: SplitDir, delta: f64 },
    SwapLeaves { id1: u64, id2: u64 },
    Minimize { id: u64, minimized: bool },
    SetFixed { id: u64, fixed: bool },
    FocusLeafDir { id: u64, dir: SplitDir },
    SwapLeafDir { id: u64, dir: SplitDir },
    Equalize,
    FocusLeaf { id: u64 },
    RestoreLayout { json: String },
}

impl UiOp {
    /// 唯一的执行点。`kind` 与 `sync_layout_cache` 的配对在这里集中保证,
    /// 不再依赖每个 handler 自己记得写两行。
    pub(crate) fn apply(self, editor: &mut Editor, compositor: &mut Compositor);
}

/// 分派:保持「一个请求一次 dispatch」。
pub(crate) fn dispatch(op: Option<UiOp>) { if let Some(op) = op { job::dispatch_blocking(move |editor, compositor| op.apply(editor, compositor)); } }
```

handler 改形(以 zoom 为例,其余同构):

```rust
pub(crate) fn handle_zoom_leaf(ZoomLeafReq { id }: ZoomLeafReq) -> anyhow::Result<Option<UiOp>> {
    Ok(Some(UiOp::Zoom { id }))
}
```

`apply_ui_requests` 的 arm 仍然**一行**:

```rust
helix_js::UiRequest::ZoomLeaf(req) => dispatch(pane::handle_zoom_leaf(req)?),
```

### 3.1 `apply` 里保留什么

- `OpenPanel`:side 白名单校验**留在 handler**(`bail!` 是「请求非法」,不是 UI 变更),`apply` 只做 rail/普通分裂二选一。
- `SplitLeaf`:组件构造(`PluginTerminal` / `PluginPanel`)从闭包搬进 `apply`,`kind` 以 `String` 形式带过来(与现状一致)。
- `RestoreLayout`:JSON 解析 + `editor.set_error` 全在 `apply`(需要 `editor`)。
- **`set_layout_change_kind` + `sync_layout_cache` 的配对从 handler 移进 `apply`** —— 这是本项顺带的收益:现在 24 个闭包里这行是手写的,漏一处就产出 `kind: null` 的事件。

## 4. 边界与错误处理

- handler **不做 UI 工作**,只做「请求 → op」的映射与参数校验;非法参数仍 `bail!`(与现状同样的错误文案)。
- `apply` 里的失败(如 `restore` 的 JSON 错误)走 `editor.set_error`,与现状一致。
- **不合并 dispatch**:一次请求一次 `dispatch_blocking`,顺序不变。
- 非 pane 族(`term.rs` 8 / `doc.rs` 5 / …还要 22 处)**本项不动**;它们的闭包里有异步/进程语义,需要各自的设计。

## 5. 测试计划与完成判据

### 5.1 新增单测(`ui_op.rs` 内的 `#[cfg(test)]`,不经 Application)

| 测试 | 断言 |
|---|---|
| `zoom_leaf_builds_op` | `handle_zoom_leaf(ZoomLeafReq{ id: 7 })? == Some(UiOp::Zoom { id: 7 })` |
| `split_leaf_maps_direction` | `"right" → (SplitDir::H, first=false)`、`"left" → (H,true)`、`"bottom"/"top"`;非法 dir 返回 `Err` 且文案含 `split: unknown dir` |
| `open_panel_maps_side_and_rail` | 三个合法 side 的映射 + `rail` 原样带进 op;非法 side `Err` |
| `cache_layout_carries_json` | op 里 `json` 与入参逐字相同 |
| `close_leaf_has_no_error_branch` | 返回 `Some(UiOp::CloseLeaf{..})`,不触碰 editor |

### 5.2 验证命令

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features integration -- -D warnings
cargo integration-test                                    # 全量 377
cargo test -p helix-term --features integration --test integration window_mode      # 16 条:布局/焦点
cargo test -p helix-term --features integration --test integration plugin_panel     # 2 条
cargo test -p helix-term --features integration --test integration plugin_layout    # 5 条:layout-change kind
```

### 5.3 完成判据(数字)

| 指标 | 现状 | 目标 |
|---|---|---|
| `ui_requests/pane.rs` 里的 `job::dispatch_blocking` | **24** | **0** |
| 全仓 `job::dispatch*` | 75 | **51**(−24) |
| pane 族 handler 直接单测 | 0 | **≥5 条** |
| `apply_ui_requests` 行数 | 59 | **≤ 90**(每个 pane arm 仍是 1 行,只是多了 `dispatch(...)`) |
| 集成测试 | 377 passed / 0 failed | **完全相同** |
| `layout-change` 的 `kind` | 24 处手写配对 | `apply` 内集中保证 |

## 6. 里程碑

| 阶段 | 内容 | 预估 |
|---|---|---|
| M1 | 立 `UiOp` + `apply` + `dispatch`,只迁移 7 个「2 行闭包」(zoom/unzoom/resize/minimize/fix/equalize/focus) | 1.5 小时 |
| M2 | 迁移 4 个带 `editor.set_error` 的(`resize_leaf_dir`/`swap_leaves`/`focus_leaf_dir`/`swap_leaf_dir`) | 1 小时 |
| M3 | 迁移 stack 族 4 个 + raise/pin/embed + float + close_leaf | 1.5 小时 |
| M4 | 迁移三个复杂的(`open_panel` / `split_leaf` / `cache_layout`) | 2 小时 |
| M5 | 5 条新单测 + 全量验证 + 记账 | 1 小时 |

每阶段独立可提交(同一项内可分多次 commit,但**门禁每次都要绿**)。

## 7. 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| dispatch 时序变化 | 布局事件顺序错乱、`kind` 错配 | 严格「一请求一次 dispatch」;M1 先做无 `kind` 的 7 个 |
| `apply` 里借用 `editor` + `compositor` | 编译期就挡住(闭包签名已给出两者) | 按闭包原本的用法,不改所有权 |
| 组件构造搬进 `apply` 后类型变多 | `UiOp` 需要 `use` 一堆 UI 类型 | op 里只放**数据**(`kind: String`),UI 类型留在 `apply` |
| handler 返回值从 `()` 变成 `Option<UiOp>` | 43 个 arm 都要跟着改? | 只改 pane 的 24 个;其余 handler 保持 `Result<()>`,`apply_ui_requests` 里两类 arm 并存(编译期就会指出漏改) |
| 单测变成「测试实现」 | 断言 `Some(UiOp::Zoom{id})` 只是复述 | 只对**映射逻辑**(dir/side 白名单、json 透传)写断言,不为每个 op 写一条;真正的行为仍靠 377 条集成测试 |

---

**为什么现在能安全做**:G7 已把 43 个 arm 拆成独立 handler(每族一文件),所以本项只需改 `pane.rs` 一个文件 + 新增 `ui_op.rs`;D6 修好的 `--features integration` clippy 门禁能挡住「测试代码里的 lint」;`window_mode`(16)/ `plugin_layout`(5)覆盖了布局树的主要变更路径。
