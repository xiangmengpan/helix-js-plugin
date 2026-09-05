# Window Rail（侧栏轨道）设计规格：filetree 固定全高边缘 + 键盘浏览

日期:2026-09-05
状态:待审查
前置:window-mode 二期(叶=窗口模型,2026-09-05-window-mode-phase2-leaf-model-design.md)之后

## 0. 决策记录(讨论结论)

- filetree(及同类浏览面板)作为 **rail(侧栏轨道)叶子**:贴左/右边缘、占 LayoutTree 全高、**永不参与 window-mode 分裂/换位/最小化/放大**;窗口模式的操作只发生在 main 区域。
- 采用方案 A(布局内 rail + 操作政策),不引入 compositor 侧栏区(方案 B):filetree 已是 PluginPanel 叶子且交互(onKey 浏览/点击/JS 渲染)已通,只补"rail 政策"。
- 键盘模型:`C-w h`(左 rail)/`C-w l`(右 rail)→ 聚焦 rail 进入浏览态;j/k/Enter 由 filetree onKey 浏览;`l` **与** `Esc` 都保留,离开 rail 焦点回 main 活动叶。
- `Enter` 打开文件:在 **当前 main 活动窗口**打开并切换过去(filetree 保持打开)。v1 不做"记住上次窗口"。
- 每侧一条 rail;bottom/其他面板维持普通叶;v1 不做轨内多组件 tab、不做拖动调宽。

## 1. 目标

让 filetree 变成编辑器右侧/左侧"固定侧栏":任何时候占满屏幕高度、不被窗口模式切割;可 `C-w h/l` 聚焦后用 j/k/Enter 直接浏览打开文件,`l`/`Esc` 回到编辑区。

## 2. 现状与问题(已代码核实)

- filetree 走 `helix.open_panel({side:"left", size:32, render, onKey})` → LayoutTree 里插入**普通叶子**,与编辑器/终端叶平级。
- 因此它参与全部 window-mode 操作:活动在它身上时 `C-w v/s/n` 会切它、`z` 折叠、`f` 独占、swap 可把它换离边缘 → 失去"全高贴边"。
- 已有 `fixed` 集合(免疫 swap/resize/remove/minimize/equalize、可被焦点穿越),但 **fixed 不免疫"被当分裂目标"**,也无"贴边全高"结构保证。
- filetree 的键盘浏览(onKey handle_key:j/k/Enter/o/E…)已实现;PluginPanel 支持 onKey 与节点焦点路由——只差"稳定贴边 + 可达聚焦"。

## 3. 目标架构(LayoutTree 内 rail)

### 3.1 结构

- `LayoutTree` 新增轨道登记:
  ```
  rail: Option<LeafRail>        // 每侧一条;left 优先文件树
  LeafRail { leaf_id: u64, side: Side }  // Side: Left | Right
  ```
- **结构不变式**:根节点恒为 `Split(rail叶 | main子树)`(左 rail,rail=first)或 `Split(main子树 | rail叶)`(右 rail)。main 内的一切分裂/嵌套只发生在 main 子树。rail 叶永不成为分裂目标 → 全高天然成立。
- `open_panel({ side: "left"|"right" })` 升级为"注册 rail":若该侧已有 rail → 关闭旧 rail 换新(先关后开);`close_leaf`/`x` 作用于 rail = 关闭 rail(回到无轨布局)。其他 side("bottom")维持普通叶。

### 3.2 操作政策(集中在 LayoutTree + window-mode 入口)

| 操作 | 目标为 rail 时 |
|---|---|
| `C-w v/s/n` 分裂 | 不分裂 rail:操作落到 main 活动叶(焦点先/同时落到 main) |
| `H/J/K/L` 交换、`C-hjkl` resize | 免疫(rail 份额不动) |
| `z` 最小化 / `f` 放大 / 等分 | 免疫 |
| `x` / `close_leaf` | 关闭 rail(恢复单 main 布局;文档仍保留) |
| `hjkl` 方向聚焦 | main 内互跳时**跳过 rail**;main 边缘叶向 rail 侧按方向键 → 聚焦 rail(见 3.3) |
| 模式内 `Enter` 确认 | rail 聚焦态=留在 rail 浏览;main 聚焦态语义不变 |

`is_rail(id)` 统一判断;全部入口在改动后过同一护栏(参考二期"叶子激活⇄focus 同步"的归拢经验,收敛到 LayoutTree 方法层,避免散落)。

### 3.3 焦点与键盘模型

```
main 最左叶 -- C-w h --> rail(left) 聚焦[浏览态]
main 最右叶 -- C-w l --> rail(right) 聚焦[浏览态](若存在)
rail 浏览态:
   j/k/Enter/o/E… → filetree onKey 正常浏览(PluginPanel 消费)
   l(向 main 侧) 或 Esc → 离开 rail,焦点回 main 活动叶(记忆离开前的叶优先)
   C-w 组 → 若 v/s/n:不分割,落回 main 后分裂;其余对 rail 免疫
   Enter 打开文件 → 在 main 当前活动窗口打开(editor.open Replace 落 leaf0/活动 main 叶)并聚焦 main;filetree 保持打开、选中态保留
```

- PluginPanel(rail)在浏览态对未知键 `Consumed`(不穿透到编辑器键位,避免"filetree 聚焦时误编辑")。
- `Esc` 两种语境:filetree 自身可能用 Esc 折叠/退层(看 onKey 实现)→ **在 rail 上 Esc 优先让 filetree 消费**,仅当 filetree 未消费时才作为"离开 rail"。实现期以 filetree 实际 onKey 返回值为准(返回 "handled"/"close" 语义),spec 预设:filetree 消费优先,l 恒为"回 main"。

### 3.4 布局/渲染

- LayoutTree 布局给 rail 叶整高份额(现有 root split 计算即满足,rail 恒定 first/second 子树根);main 区域 = 总宽减 rail 宽。
- rail 与 main 之间沿用叶子分隔/焦点边框渲染(现状);rail 叶固定宽 = open_panel size。
- `dump()/get_layout()`:叶子信息加 `rail: bool`;JS 可感知自己是否在 rail。

### 3.5 JS API 面(增量)

- `helix.open_panel` 的 `side:"left"/"right"` → rail 语义(见 3.1);新增可选项 `rail: true` 显式声明(缺省:left/right 即 rail,兼容 filetree 现状零改动)。
- 可选:`helix.get_layout()` 已含 rail 标记,无需新 API;关闭复用 `close_leaf`。

## 4. 边界行为

| 场景 | 行为 |
|---|---|
| 左 rail 已开,再 open_panel(left) | 关闭旧、注册新(文件树重开=同侧,无感) |
| rail 聚焦时 `x` | 关闭 rail → 焦点落 main |
| 仅剩 rail 与 main(1+1)时对 main 分裂 | 正常:main 子树内分;rail 不动 |
| zoom main 某叶 | 仅 main 内放大;rail 保持显示(rail 不随 zoom 隐藏,否则全高边栏消失) |
| 终端/面板聚焦 C-w h | 进入左 rail;终端 Insert 直通豁免沿用 |
| filetree 打开文件路径已在 main | editor.open 聚焦已有 doc(不重开) |
| 拖拽/调宽 | v1 不做;rail 宽 = open_panel size,想改宽重开或后续加键 |

## 5. 测试计划

- 单元(layout.rs):
  - rail 注册后 root 形状 = Split(rail|main);is_rail 判定
  - 分裂/交换/最小化/放大目标为 rail → 免疫/落 main
  - main 内分裂后 rail 仍占全高(结构断言)
  - dump 含 rail 标记
- 集成(helpers 渲染断言):
  - `:filetree`(或 open_panel left)后:C-w h 聚焦 rail,渲染右边界完整(全高)
  - rail 聚焦态 j/k 移动选中、Enter 打开文件到 main 活动窗并聚焦 main(doc.text 变化 + active 回 main)
  - `l`/`Esc` 离开 rail 回 main
  - rail 聚焦按 C-w v → main 分裂而非 rail
  - main 分裂/放大/最小化后 rail 仍贴边全高(渲染/leaf 断言)
  - `x` 关 rail → 单 main;文件树再开恢复
  - 右 rail(open_panel right)对称用例
- 回归:window_split/splits/window_mode/plugin(close_leaf/buffer_open 等)、helix-view。

## 6. 里程碑

1. LayoutTree rail 结构:登记/is_rail/root 形状保持 + dump 标记(单元)
2. 操作政策护栏:分裂/交换/最小化/放大/remove 对 rail 免疫或落 main(单元+集成)
3. 焦点模型:C-w h/l 进出 rail、l/Esc 回 main、filetree 浏览键消费(集成)
4. Enter 打开文件到 main 活动窗并聚焦 + filetree 保持
5. 文档(README/plugin-api 窗口段)+ 全量回归

每期独立可交付;filetree 用户从 1 期即可感知"不再被分割"。

## 7. 风险

- **filetree 与 main 的 Esc 争夺**:rail 上 Esc 语义(折叠 vs 离开)以 filetree onKey 返回为准,需实测 filetree handle_key 的 Esc 分支;冲突则调 filetree 脚本(用户侧)或护栏顺序。
- **zoom 语义**:rail 若随 main zoom 隐藏会破坏"始终可见"预期——spec 已定 rail 不随 zoom;实现期验证 zoom 渲染路径。
- **活动叶记忆**:l/Esc 回 main 的目标叶 = 进入 rail 前的活动叶;若期间 main 布局变化(被 x 关),回退到 main 首个叶。
- 既有测试中 open_panel(left/right) 的面板语义变化(filetree 相关 plugin 测试需同步断言 rail 形状)。
