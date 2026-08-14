# Window 模式(方案 1 一期)设计规格

日期:2026-08-14
状态:待审查
对应方案:方案 1(原生 WindowMode,全 Rust 核心)+ 一期 BufferLeaf

## 1. 目标

为布局树提供**全局**窗口管理模式:任何叶子焦点(编辑器/终端/面板)下按 C-w 进入,模式内按键直调布局树原生操作,不依赖 JS 插件与焦点类型。

## 2. 一期范围

**做:**
- WindowMode 状态机 + 全局拦截
- 键位:hjkl 聚焦 · HJKL 交换 · C-hjkl 宽/高 ±5% · x 关闭 · z 最小化/还原 · f 最大化/还原 · Esc/C-w 退出
- `LayoutTree.fixed: HashSet<u64>` + `helix.layout_fix(leaf_id, bool)`(fixed 叶子不参与 swap/resize/close/minimize/equalize,可被焦点穿过)
- `BufferLeaf` 单 view 编辑器叶子 + `helix.buffer_open(path, { leaf_id?, split })`
- 状态栏 `[WINDOW]` 指示(默认元素 + replace 模式 ctx 透传)
- which-key.js 的 C-w 段下线(全局拦截后不可达)
- 弹窗/菜单打开自动退模式(compositor.push 检测)

**不做(后续):**
- anchored 绝对 rect 锚定叶子(P2)、鼠标点击命中(P4)
- 多 view 叶子、buffer 跨叶移动、命令作用域(二期)

## 3. 架构

### 3.1 状态机(compositor.rs)

```rust
enum WindowMode {
    Inactive,
    Active,
}
```

进出规则:
- 任何焦点按 C-w(且 Inactive)→ Active,消费事件
- Active 内:Esc 或 C-w → Inactive,消费
- Active 内:命中键位表 → 直调 `main_tree` 原生方法,消费
- Active 内:其他键 → Ignored(保持模式,不穿透)
- `compositor.push()` 推入弹窗/菜单类 layer 时若 Active → 自动 Inactive(否则弹窗按键被模式吞掉,死锁)

### 3.2 拦截点(compositor.handle_event)

现有入口顺序:macro recording → layers → main_tree。插入:

```
1. macro recording 检查(macro recording 状态下不进入/不处理模式键,避免录进宏)
2. WindowMode::Active → 处理模式键(见 3.3),命中则 return
3. Event::Key(C-w) 且 Inactive → 进模式,return Consumed
4. 原流程(layers → main_tree)
```

### 3.3 键位表 → LayoutTree 方法(全部已存在,零新增核心逻辑)

| 键 | 操作 | 调用 |
|---|---|---|
| h/j/k/l | 方向聚焦 | `neighbor_leaf(active, dir, side)` → `focus(id)` |
| H/J/K/L | 方向交换 | `neighbor_leaf(...)` → `swap(active, id)` |
| C-h / C-l | 宽度 -5% / +5% | `resize_leaf_dir(active, H, ∓0.05)` |
| C-j / C-k | 高度 -5% / +5% | `resize_leaf_dir(active, V, ∓0.05)` |
| x | 关闭叶子 | `remove(active)`(焦点迁移已有) |
| z | 最小化/还原 | `set_minimized(active, !当前)` |
| f | 最大化/还原 | `zoom(active)` / `unzoom()` |
| Esc / C-w | 退出 | `Inactive` |

方向映射:`h`/`l` → `SplitDir::H`(first_side 由 `neighbor_leaf` 祖先链回溯决定);`j`/`k` → `SplitDir::V`。

### 3.4 数据模型(LayoutTree)

```rust
pub struct LayoutTree {
    // ...现有字段
    fixed: HashSet<u64>, // 新增
}
```

- `swap` / `resize_leaf_dir` / `remove` / `set_minimized` / `equalize` 入口:目标在 fixed → 直接 false/无操作
- `neighbor_leaf`/`focus_dir` 可穿过 fixed 叶子
- `get_layout` 输出加 `fixed: bool` 字段

### 3.5 BufferLeaf(ui/buffer_leaf.rs 新文件)

```rust
pub struct BufferLeaf { view_id: ViewId }
```

- 渲染:复用 `EditorView::render_view`(抽为可调用的静态函数或委托),渲染该 view 到叶子 rect
- 按键:忽略(Ignored)→ 事件路由走布局焦点,Ignored 兜底到编辑器叶子(id=0)照常处理编辑键
- 生命周期:与叶子同生共死;view 从 `Editor.tree` 分配

### 3.6 状态栏指示

- `RenderContext` 加 `window_mode: bool`
- 默认模式:新增状态栏元素(如 `StatusLineElement::WindowMode`),Active 时渲染 `[WINDOW]`
- replace 模式(用户现状):`StatuslineCtx` 加 `window_mode: bool` 字段,statusline.js 的 render 读 `ctx.window_mode` 拼段(Rust 构造 ctx 处取值:`compositor.window_mode.is_active()`)

## 4. JS API 面(一期最小)

```
helix.buffer_open(path, { leaf_id?, split?: "h"|"v" }) → leaf_id
    // 无 leaf_id:在活动编辑器叶子分屏;split 指定方向
    // 实现:UiRequest::OpenBufferLeaf → 新建 BufferLeaf 叶子(split_leaf_prealloc 同款)
    // 一期:无 leaf_id 且活动叶子非编辑器 → 报错提示
helix.layout_fix(leaf_id, bool) → bool
    // UiRequest::LayoutFix → LayoutTree.fixed 增删
helix.get_layout() 输出加 fixed 字段
```

## 5. 与现有插件交接

- which-key.js:删除 C-w 段绑定(注释说明由 Window 模式接管)
- lib/layout.js:`layout-*` 命令与 API 保留(命令行/插件直接调用仍可用);C-w 键位段删除
- 默认 helix C-w 组(rotate_view 等)被全局拦截,不可达——文档声明放弃,后续可做进模式键位表

## 6. 边界行为

| 场景 | 行为 |
|---|---|
| 终端焦点按 C-w | 进模式(全局拦截,终端无感知);字面 C-w 用现有 C-\ 穿透 |
| 面板焦点按 C-w | 进模式 |
| 模式内打开弹窗 | push 时自动退模式 |
| minimized 叶子为 active | z 还原;模式操作对 minimized 叶子跳过(同 fixed 语义) |
| zoom 态 | f 还原;hjkl 对 zoomed 树不生效(active() 返回 zoomed 叶子) |
| 模式内关闭最后可操作叶子 | 保持模式;无可操作叶子(仅编辑器)时退出 |
| macro recording | 模式键不录(拦截在 recording 检查之前) |
| fixed 叶子 | 不被 swap/resize/close/minimize/equalize;可被焦点穿过 |

## 7. 测试

- 单元(layout.rs):fixed 跳过 swap/resize/close;get_layout 含 fixed 字段
- 集成(helpers.rs 渲染断言):
  - C-w 进模式 → 状态栏含 `[WINDOW]`(默认与 replace 各一)
  - C-w h/j/k/l 后 `get_layout().active` 变化(构造 h/v split 布局)
  - H/L 交换后树结构断言
  - C-w x 关闭叶子
  - C-w z/f 最小化/最大化往返
  - Esc 退出 → 状态栏指示消失、按键回编辑器
  - 弹窗打开自动退模式
  - buffer_open 新叶子渲染指定 buffer 内容

## 8. 里程碑

1. WindowMode 状态机 + 拦截 + 键位 + 状态栏 + which-key 下线
2. fixed 集合 + layout_fix + get_layout 字段
3. BufferLeaf + buffer_open
4. 测试补齐

## 9. 风险

- 全局拦截吃掉默认 C-w 组:文档声明,可接受
- BufferLeaf 渲染复用 render_view 需要把 `EditorView::render_view` 抽成可调用形式(小重构)
- 状态栏 replace 与默认两路都要改:遗漏任一则指示不显示
