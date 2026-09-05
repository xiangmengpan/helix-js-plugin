# Window 模式二期：叶=窗口 统一模型（含 :vsplit 改道）

日期:2026-09-05
状态:待审查
对应:window-mode 一期(2026-08-14-window-mode-design.md)之后的架构收敛

## 0. 决策记录(讨论结论)

- fork 窗口对象模型定为:**一叶一窗口,叶可指向同一 doc**(同文件多叶、修改互通必须保留)。
- `:vsplit`/`:hsplit` 的"同 buffer 双视图"语义不是历史包袱,是真实需求(同文件两处对照+编辑同步);实现方式从"view tree 内分裂"改为"LayoutTree 开新叶指向同一 doc"。
- window mode 成为**所有无参窗口结构操作**的唯一入口(创建/聚焦/换位/缩放/关闭/最小化/最大化);带参操作(如 `:vsplit <path>` 开文件)保留在命令行。
- 输入路由与"活动叶子"必须统一:叶子可编辑是"叶=窗口"成立的前提。

## 1. 背景与问题

一期引入 compositor `LayoutTree` + window mode(C-w 全局拦截),可容纳 editor/BufferLeaf/terminal/panel 叶子。但存在两棵互不可见的窗口树:

```
树 A: compositor LayoutTree(窗口模式/叶子渲染)
        leaf 0 = 编辑器(EditorView) | leaf N = BufferLeaf/terminal/panel ...
树 B: helix 自家 view tree(editor.tree,core,helix-view/tree.rs)
        :vsplit/:hsplit → editor.switch() → Tree::split 插入第二个 view
        EditorView::render 遍历 tree.views() 画在 leaf 0 内部
```

问题(均已代码核实):

1. **`:vsplit` 产物逃出 window mode**:第二 view 在 leaf 0 内部渲染,vsplit 产生的是 core view tree 节点,不是 LayoutTree 叶子;window mode 的 `neighbor_leaf/focus/remove` 只认 LayoutTree leaf id → 无法聚焦/关闭。旧的上游 C-w view-tree 导航键被 window mode 全局拦截不可达 → 该窗**渲染出来但无任何键能控制**(相对上游的功能回归)。
2. **BufferLeaf 不可编辑**:`buffer_leaf.rs` 明示 view 独立持有(不进 editor.tree)。core 输入路由 `current!` = `view_mut!(editor)` = `editor.tree.get_mut(tree.focus)`(macros.rs)——**view 必须在 editor.tree 里才能成为编辑目标**。BufferLeaf 的 view 不在 → 按键 Ignored 兜底到 leaf 0,tree.focus 未指向它 → 编辑落在主视图文档上(phase-1 妥协,spec 标注"多 view/跨叶移动二期")。
3. **命令入口三套分裂**:`:vsplit[:hsplit] [path]` / `:vsplit-new[:hsplit-new]` / `helix.buffer_open(path,{split})`,分别走 editor 命令层(view tree)与 JS API(LayoutTree 叶),语义不同步。

## 2. 目标与非目标

**做:**
- 窗口对象模型统一:**几何布局唯一来源 = LayoutTree;叶 = 窗口 = 一个可编辑 view**。
- core view tree 退化为 **view 收纳/遍历/可见性检查容器**,不再承担几何布局与"一叶多 view 渲染"。
- 叶子激活 ⇄ `editor.tree.focus` 同步 → 任意叶子(含 BufferLeaf/未来 terminal 编辑态)都是 core 编辑路由的合法目标。
- `:vsplit`/`:hsplit`(无参与带参)、`:vsplit-new`/`:hsplit-new`、`goto_file`(gf)等分裂入口**改道为创建 LayoutTree 叶**(同 doc 复制 view / 新 buffer / 打开文件)。
- window mode 键位表新增创建键:`v`/`s` 同 doc 分屏(对应 vsplit/hsplit 方向)、`n` 新空 buffer。
- `EditorView`(leaf 0)改为只渲染 `tree.focus`(或活动叶自己的 view),移除整树遍历渲染。
- 同 doc 多叶语义 = helix 语义:同一 Document(undo 栈/内容共享、编辑互通),各 view 光标/滚动独立(与今天 `:vsplit` 一致)。

**不做(后续):**
- anchored 锚定叶、鼠标点击命中布局(P4 遗留,不进本期)。
- 跨叶移动 buffer、多 view 单叶(一叶多 view 的滚动同步视图)等 view-tree 高级特性——本期明确一叶一 view。
- terminal 叶子内编辑(terminal 有自己的输入通道,不受影响;本期只保证 terminal 作为叶子可被 window mode 控制,已实现)。

## 3. 目标架构

### 3.1 职责重划

| 结构 | 现在 | 之后 |
|---|---|---|
| `LayoutTree`(term 层) | 几何布局 + 叶子渲染 | 几何布局 + 叶子渲染(不变) |
| `editor.tree`(core) | 布局 + 焦点 + 可见性 | **view 收纳** + 焦点(id) + 可见性遍历(布局结构废弃) |
| `EditorView`(leaf 0) | 渲染整棵 view tree | 只渲染 `tree.focus` view |
| `BufferLeaf` 等叶子 | view 游离,不可编辑 | view 注册进 editor.tree,激活即编辑 |

### 3.2 输入路由统一(核心机制)

现有 `LayoutTree::handle_event` 已做 Ignored 兜底到 leaf 0(`layout.rs:800-830`):活动叶子组件 Ignored 且非 0 → 交给 leaf 0 的 EditorView → EditorView 按 `tree.focus` 处理键位。**因此只要满足**:

1. 叶子的 view **注册进 editor.tree**(叶子组件能通过 `tree.get(view_id)` 取到);
2. 叶子变为活动时 **`editor.tree.focus = 该叶的 view_id`**;

则现有 core 编辑键位/宏/文档操作**零改动**命中正确文档。叶子组件无需各自实现键位循环。

同步点(全部需要 `tree.focus` 跟随):
- window mode `hjkl` 聚焦、Enter 确认、`x` 关闭后的焦点迁移
- LayoutTree 内部 `focus(id)`(buffer_open 后新叶自动激活等)
- 关闭叶子后回落到 leaf 0(现有 active() 回落已保证)→ tree.focus 回落

### 3.3 view 收纳方式(core 侧,实现期先验证)

目标:view 可被 `tree.get(id)` 取到、可被 `tree.traverse()`/`views()` 枚举(供"doc 是否仍被其他窗口显示"等可见性检查),但不参与几何。

验证点(编码时用最小 spike 验证,方向为 spec 预设):
- 在 tree 的 arena 中追加 view 节点、挂到根容器下(根容器布局结构对叶子渲染已无影响);
- 或新增小 helper(如 `add_view_flat(view)`)把 view 挂为根容器的直接子节点;
- 关键回归:`remove_empty_scratch` 等 core 逻辑经 `tree.traverse()` 枚举"doc 是否显示于其他 view"——BufferLeaf view 入树后应被正确计入(行为正确且为期望)。
- **不得**依赖容器 layout 做任何几何计算的新代码路径。

### 3.4 分裂命令改道(entry-point 清单)

以下所有入口改道为"在 LayoutTree 当前活动叶旁创建新叶",按场景决定叶内容:

| 命令/入口 | 现状(core) | 改道后 |
|---|---|---|
| `:vsplit`/`:hsplit`(无参) | `split()→editor.switch(id, Action::Split)` tree 内分裂同 doc | 新叶 + 同 doc 新 view(复制光标/滚动,复用现有 split() 的 selection/offset 匹配) |
| `:vsplit <path>`/`:hsplit <path>` | open_impl + Split action | 新叶 + 打开 path 的 doc |
| `:vsplit-new`/`:hsplit-new` | `editor.new_file(Split)` | 新叶 + 新空 buffer doc |
| `goto_file`(`gf`) | goto_file_impl + Split action | 同上(打开目标文件到新叶) |
| `helix.buffer_open(path,{split})` | 已建 BufferLeaf 叶 | 语义不变,补:view 入树 + 激活同步(3.2) |
| 启动多文件参数 | application.rs 内 Split | 每个文件开一叶(leaf 0 显示首个) |

带参形式保留原命令名(命令行/脚本入口不变),只是落地从"core 树内分裂"变为"LayoutTree 叶"。无参 `:vsplit` 也保留为别名(等同 window mode `v`),避免破坏肌肉记忆与脚本。

### 3.5 window mode 键位表增量

```
一期已有:  hjkl 聚焦 | HJKL 交换 | C-hjkl 缩放 | x 关 | z 缩 | f 放大 | Esc/C-w/Enter 退
二期新增:  v       同 doc 分屏(与 :vsplit 同向,右侧)
           s       同 doc 分屏(与 :hsplit 同向,下侧)   ← 方向映射沿用一期 C-hjkl 的 H/V 语义
           n       新空 buffer 分屏(取代 :vsplit-new/:hsplit-new 无参语义)
```
方向选择:`v` → SplitDir::H(first_side=false,右),`s` → SplitDir::V(下),`n` 沿用当前活动叶方向缺省(或与 v/s 同键选向——实现期按 UX 测试定,spec 预设 v/s/n 三键)。

### 3.6 EditorView 渲染收敛

`EditorView::render` 的 `for (view, is_focused) in editor.tree.views()` 整树循环改为**只渲染 `tree.focus`**;BufferLeaf.render 已用 `view.area = area` 就地布局(render_view 按 view.area 裁剪)——叶子渲染路径不变。leaf 0 与 BufferLeaf 渲染同一 doc 时不得双画(入树后由"leaf 0 只画 focus"保证,若 focus 的 view 由 BufferLeaf 持有且 BufferLeaf 活动 → leaf 0 应跳过)。

## 4. 数据流

```
window mode h → LayoutTree.focus(leaf B) → 同步 editor.tree.focus = B.view_id
     ↓
普通按键 → LayoutTree::handle_event → B(component) Ignored → 兜底 leaf 0 EditorView
     ↓
EditorView 键位 → core 命令 → current! = tree.get(tree.focus=B.view) → 编辑 B 的 doc
     ↓
编辑 → doc 变更 → 所有指向该 doc 的叶(view A/B)下次渲染可见(同 doc 同步天然成立)
```

## 5. 边界与错误处理

| 场景 | 行为 |
|---|---|
| 同 doc 两叶,一叶关闭 | doc 仍被另一叶显示 → 不关文档(复用 remove_empty_scratch 的遍历检查) |
| 关闭最后一份 doc 的叶 | 回落空 scratch / 原编辑器行为 |
| mode 内 `x` 关闭活动叶 | 焦点迁移到相邻叶 → tree.focus 同步;仅剩 leaf 0 → 回落 |
| `:vsplit` 目标 path 打开失败 | 新叶不建,报错(同 open_impl 现状) |
| terminal/panel 叶激活 | 不设 tree.focus(它们无 view);window mode 照常控制(已实现) |
| 宏录制中窗口操作 | 沿用一期:mode 键不录 |
| 旧会话遗留的 view-tree 多 view(升级前 :vsplit 产物) | 迁移策略:启动时检测 tree 内非 focus view → 逐个补建叶并激活同步(见 P3) |

## 6. 测试计划

- 单元(core/tree):`add_view_flat`(或等价)后 get/traverse 可见;`remove_empty_scratch` 对"doc 在另一叶"不再删。
- 单元(layout.rs):叶子激活回调同步 tree.focus(注入 stub editor 状态断言)。
- 集成(helpers 渲染断言):
  - `:vsplit`(无参)→ 两叶,右叶渲染同 buffer 内容;左叶编辑 → 右叶内容同步(同 doc)
  - window mode 内 `v`/`s`/`n` 创建后 `hjkl` 可聚焦新叶、`x` 可关、编辑落在正确叶
  - `:vsplit <path>` / `gf` 打开到新叶,新叶可编辑
  - `:vsplit-new` → 新空 buffer 叶
  - BufferLeaf(buffer_open)在 P1 后:**编辑真正落进该叶**(P1 的验收断言)
  - window mode `x` 关闭同 doc 一叶后另一叶仍正常编辑、doc 未关
  - 回归:一期全部 window-mode 测试、plugin terminal/panel 测试
- 全部既有 integration 测试串行锁下通过(见前一交接:PLUGIN_TEST_LOCK)。

## 7. 分期里程碑

**P1 — 输入路由地基(让任意叶子可编辑)**
1. `EditorView::render` 改只渲染 tree.focus(移除以防双画)
2. BufferLeaf view 注册进 editor.tree(收纳方式见 3.3 spike)
3. 叶子激活(含 LayoutTree.focus 所有调用点)→ 同步 tree.focus
4. 验收:buffer_open 叶聚焦后编辑命中该叶;同 doc 同步生效;既有测试全绿

**P2 — 分裂命令改道**
1. `:vsplit/:hsplit`(无参)→ 新叶 + 同 doc view(复制光标/滚动)
2. `:vsplit <path>/:hsplit <path>`、`gf` → 新叶 + 打开文件
3. `:vsplit-new/:hsplit-new` → 新叶 + 空 buffer
4. 启动多文件参数分行开叶
5. 验收:`:vsplit` 产物可被 window mode 完全控制;无 :vsplit 逃逸回归(升级遗留另行处理)

**P3 — window mode 创建键 + 收尾**
1. window mode 键位表加 `v`/`s`/`n`
2. 旧 view-tree 多 view 会话迁移(启动补建叶)
3. which-key 提示更新(模式键位含创建)
4. 文档(plugin-api/README 窗口段)更新

**P4 — 测试与回归**
- 补全 §6 全部集成断言;跑全套件(含串行锁)回归;clippy/fmt。

每期独立可交付;P1 后 BufferLeaf 即转正(可编辑),P2 后 :vsplit 逃逸消失,P3 完成交互闭环。

## 8. 风险

- **core 手术面**:3.3 的 view 收纳触碰 helix-view tree——spike 先行,若 Tree arena/遍历假设不成立则调整收纳方式(仍满足"get/traverse 可见 + 不参与布局"即可);不改动 core 的 doc/view 语义本体。
- **双画/漏画**:leaf 0 只画 focus 与 BufferLeaf 自画的交接——测试覆盖同 doc 两叶渲染。
- **tree.focus 同步遗漏点**:任何把 LayoutTree 活动叶换掉的路径若漏同步,表现为"聚焦了 A 叶却编辑 B"——P1 用集成断言钉死,并在 LayoutTree::focus 收敛为单一入口(带同步)降低遗漏面。
- **既有 integration 断言**:多个测试假设"编辑器=leaf 0 整树渲染"(如 vsplit 相关 window_mode 测试),P2 需同步改写。
- 上游升级:fork 对 tree 的改动是"使用方式"而非"语义改写",冲突面小。
