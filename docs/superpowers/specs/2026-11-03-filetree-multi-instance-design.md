# filetree 双实例 / 双 root 设计（§7.2 的收口）

> **状态：第 1–5 步已实现（2026-11-03）——双实例 + 持久化 v2 已落地；第 6 步（文档收尾）基本完成，见 §12.4。**
> 前作：[`2026-11-02-filetree-search-design.md`](2026-11-02-filetree-search-design.md)（P0–P3 + §13/§14 补做）。
> 本文件只解决一件事：**同一时刻挂两个文件树面板、两个不同的 root**。
>
> 行号口径：`plugin.js:N` 指 2026-11-03 的源文件（1660+ 行），改动后会漂移。

---

## 0. 一句话

> 现在 `S` 是**一个模块级单例**（`plugin.js:463`），面板的一切（root/树/光标/模式/搜索/过滤/索引）
> 都挂在这一个对象上。**开第二个面板 = 两个面板抢同一份状态**：后开的会覆盖 `S.panel_id`，
> 前一个面板从此关不掉、渲染的是别人的树、按键改的是别人的光标。
>
> 收口方式：把 `S` 拆成 **`G`（编辑器全局）+ `P`（每面板一份实例）**，
> `render` / `onKey` 变成**按实例生成的闭包**，命令层统一走"目标实例解析"。

---

## 1. 现状：坏在哪（可复现的三步）

| 步骤 | 现象 | 机制 |
|---|---|---|
| `:filetree` 开面板 A | 正常 | `S.panel_id = helix.open_panel(...)`（`plugin.js:1159`） |
| 再调一次"开"（比如用别的命令/键位再触发 `open_panel`） | 出现**两个**面板，但它们共用一棵树 | `open_panel` 不检查已有实例，直接又开一个并覆盖 `S.panel_id` |
| 按 `q` / `Esc` | 只关掉**后一个**，前一个面板永久残留（`S.panel_id` 已指向别人） | `close_panel`（`:1167`）只认 `S.panel_id` |

而且即使关掉一个，另一个的 `root` / `cursor` / `filter` 也都是"两个人共用一支笔"。

**引用量**（`grep` 实测）：`plugin.js` 里 **234 行、282 处** `S.xxx`；按字段拆：

| 类别 | 字段（出现次数） | 小计 |
|---|---|---|
| **实例内**（每面板一份） | `cursor`44 · `query`38 · `mode`24 · `tree`23 · `root`22 · `filter`15 · `flat`13 · `scope`12 · `pre_query`12 · `grep`12 · `eager_done`4 · `panel_id`10 | **229（81%）** |
| **全局**（编辑器事实 / 全局偏好） | `show_hidden`11 · `git`10 · `diag`8 · `opened`8 · `prompt_value`8 · `follow`5 · `current`3 | **53（19%）** |

> 结论：这是个**机械但要彻底**的重构 —— 81% 的引用会换宿主，靠"局部补丁"只会造出第二批幽灵状态。

---

## 2. 关键未知：怎么知道"哪个实例被聚焦"？（已实测，答案不理想）

设计里必须回答"`:filetree-reveal` 该落在哪个面板"。查证结果：

| 想要的东西 | 现状 |
|---|---|
| `open_panel` 返回的**实例 id** | ✅ 有，插件自己存着 |
| `pane.list()` 的 `id` | ❌ 那是**布局叶子 id**，与实例 id 不是同一套；条目里**不含**实例 id |
| `pane.list()` 的 `focused` | ✅ 有，但只对**叶子 id** 有意义 ⇒ 插件无法把自己的实例对上 |
| `pane.focus_panel(id)` | ✅ 单向（实例 → 聚焦），不能反查 |
| 焦点变化事件 | ❌ 没有（`pane-mode-change` 是 `C-p` 平级模式，不是焦点） |

**因此**：精确判定当前不可用。方案取

> **`G.active` = 最近一次收到按键的实例**（面板 `onKey` 只有聚焦叶子才会收到键），
> `open_panel` 之后立刻把自己设为 active；命令想显式指定就用 `helix.pane.focus_panel(id)` 把焦点推过去再操作。

这条启发式是**单调自纠正**的：用户一按该面板的键就立刻正确；唯一的窗口是"用鼠标/`:pane focus` 切了焦点但还没按键"，
此时命令会落到上一次的 active 上 —— 代价可接受，且**写进文档**（并把"核心补 `panel_id`"列进 §9 建议）。

---

## 3. 三个候选

| | **A. 实例化（推荐）** | B. 快照换入换出 | C. 单面板 + 双 root 切 tab |
|---|---|---|---|
| 做法 | `G`/`P` 拆分 + 注册表 + 闭包工厂 | 保持单份 `S`，焦点变化时把状态存/取 | 还是**一个**面板，`root` 变成数组，`Tab` 在 root 间切视图 |
| 能否并排看两棵树 | ✅ | ✅ | ❌（同一时刻只显示一个 root） |
| 依赖焦点事件 | 不需要 | **需要**（核心没有）⇒ 不可行 | 不需要 |
| 改动量 | 大（~282 处引用重新归位） | 中 | **小（~40 行）** |
| 风险 | 状态归属错一处就"两个面板互相串味" | 换入换出漏一处 = 状态错位，且难测 | 不解决用户诉求（并排） |

**结论**：A 是要交付的形态；**C 作为 A 的第 1 步**（先把"多 root"建模出来，但只显示一个），
这样即便 A 中途搁置，C 也是可用的中间态。

---

## 4. A 的详细设计

### 4.1 数据结构

```js
// 全局（编辑器事实 + 全局偏好）：与"哪个面板"无关
const G = {
  panels: new Map(),     // id -> P
  active: null,          // 最近一次收到按键的实例 id（见 §2）
  current: null,         // 当前编辑文件（buffer-open 更新）
  opened: null,          // Set<path> 已打开的文件
  diag: null,            // Map<path, {severity,count}>
  last_cwd: null,        // cwd-change 时用来判断"哪些实例的 root 就是旧 cwd"
  show_hidden: CFG.show_hidden === true,
  follow: true,
  prompt_value: "",
};

// 实例（每面板一份）：全部"视图状态"
function new_panel(root) {
  return {
    id: null,            // helix.open_panel 的返回值
    root, tree: null, cursor: 0,
    mode: "tree",        // tree | find | filter | grep
    scope: "tree", query: "", pre_query: null,
    filter: "",          // 已应用的过滤表达式
    flat: null,          // 全库索引（含 nucleo 会话）
    grep: null,          // 内容搜索结果
    git: null,           // Map<path, 单字母>（**每个 root 一份**：两个实例可能是两个仓库）
    eager_done: false,
  };
}
```

**为什么 `git` 进实例、`diag`/`opened` 留全局**：前者按 root 走（两个仓库的状态不能混），
后者按**路径**索引、与 root 无关（同一个文件在两个树里就该显示同一条诊断）。

**为什么 `show_hidden`/`follow` 留全局**：它们是"用户偏好"而不是"这个视图怎么摆"；
两个面板一个显示隐藏文件一个不显示，只会让人怀疑自己按错了 —— 真要区分，留给 P3 之后的"每实例覆盖"。

### 4.2 函数改造：只动"非纯函数"，纯函数原样

这是把 282 处引用**压缩到可控范围**的关键：`plugin.js` 里已经有一批**不碰 `S`** 的纯函数
（`match_name` / `node_match` / `filter_rows` / `match_tree` / `highlight_segments` / `flat_items` /
`grep_groups` / `compile_filter` / `dsl_hit` / `glob_to_regex` / `parse_git_status` / `worst_severity` /
`diag_hit` / `opened_marker` / `rel_parts` / `find_node` / `find_parent` / `collect_expanded` /
`window_rows` / `scroll_top` / `sort_entries` / `shq`），**一个都不改**（27 个 node 用例保持全绿）。

需要加 `P`（或 `P` 换成 `(p, …)`）的非纯函数清单：

| 组 | 函数 |
|---|---|
| 视图 | `rows_for_view(p)` · `tree_rows(p)` · `node_at(p,i)` · `count_visible(p)` · `move(p,d)` |
| 树/根 | `set_root(p,dir)` · `reload_tree(p)` · `reveal(p,path)` · `context_dir(p)` · `eager_load_once(p)` |
| 搜索 | `enter_find(p)` · `find_key(p,k)` · `refresh_find(p)` · `enter_filter(p)` · `filter_key(p,k)` · `enter_grep(p,seed)` · `start_grep(p)` · `grep_key(p,k)` · `grep_rows(p)` · `flat_rows(p)` · `load_flat(p)` · `drop_flat(p)` |
| 面板 | `open_panel(root?)` → 建实例并入注册表 · `close_panel(target)` · `toggle_panel()` · `make_render(p)` · `make_key(p)` |
| 全局拉取 | `seed_opened()` · `poll_diagnostics()` · `refresh_git(p)` · `git_mark(p,path)` · `diag_mark(path)` |

`render` / `onKey` 变成工厂（**这是整件事的支点**）：

```js
function make_render(p) {
  return (_focus, ctx) => render_with(p, ctx);   // 现在的 render 主体，S → p
}
function make_key(p) {
  return (key) => { G.active = p.id; return handle_key_with(p, key); };
}
p.id = helix.open_panel({ side: "left", size: 32, render: make_render(p), onKey: make_key(p),
                          onClose: () => { G.panels.delete(p.id); if (G.active === p.id) G.active = first_id(); } });
```

顺带解决一个隐患：`onClose` 里摘注册表 —— 现在 `close_panel` 之外的关闭路径（布局恢复、
`:plugin-reload`）不会清 `S.panel_id`，`popup_exists` 那类自愈机制面板没有。

### 4.3 命令与事件语义（都要固化，否则双实例会"各自以为自己是唯一"）

| 入口 | 新语义 |
|---|---|
| `:filetree` | 无面板 → 开一个（root = cwd）；有 1 个 → 关它；有 2 个 → 关 **active** |
| `:filetree-2`（新增）或 `:filetree-open <dir>` | 显式开第二个实例（root = 参数/当前文件所在目录）；已到 `max_panels` 时 echo 说明 |
| `:filetree-reveal` | 目标实例按**优先级**取：① active；② root 是当前文件祖先的那个；③ 唯一的那个；④ 都没有就先开一个 |
| `:filetree-find` | 用 **active** 实例的 root 做 `rg --files`（没有实例 → cwd） |
| `:filetree-search <词>` | 目标同 `reveal`；没有面板就先开 |
| `buffer-open` 的跟随（`F`） | 只作用于 active 实例；active 处于搜索态时跳过（现状保持） |
| `cwd-change` | 对每个实例：`p.root === G.last_cwd`（或 `p.root === null`）→ `set_root(p, new_cwd)`；其它保持（**不再**一刀切重设） |
| `R` | 只刷 active 实例 |
| `H` / `F` | 改全局（`G`），所有实例下一帧都变 |
| `:plugin-unload` / reload | 遍历注册表：每个实例 `drop_flat(p)`；`save_state()` 只写一次 |

`max_panels`：默认 **2**（配置项，`doc` 写清）。理由：rail 全局只有一条（`helix.d.ts` 的
`PanelOptions.rail` 注释），且 32 列 × 2 之后编辑器只剩半屏。

### 4.4 §7.1 的口径同时定论

`docs/plugin-api.md:155` 写"filetree 即 rail 模式"，但 `plugin.js:1159` 的 `open_panel` **没传 `rail: true`**（也没传 `onClose`）（§7.1 待确认项）。双实例下必须选一个：

- **两个都不 rail**（推荐，也是现状）：两个面板都是普通分屏叶子，`Esc`/`l` 的语义完全由插件定义；
- 或"第一个 rail、第二个 split"：会引出"谁是第一个"以及 rail 的 `Esc`/裸 `l` 回主区与插件键位的冲突。

本设计取**前者**，并顺手把 `plugin-api.md:155` 那句改成"filetree 用的是普通侧边面板（非 rail）"。

### 4.5 持久化 v2

```jsonc
{
  "version": 2,
  "panels": [
    { "root": "/p/a", "filter": "*.rs", "expanded": ["/p/a/src", "/p/a/src/api"] },
    { "root": "/p/b", "filter": "",     "expanded": [] }
  ]
}
```

- 读入：`version === 1` 的旧文件按"单实例"迁移（`{root, filter, expanded}` → `panels: [ … ]`）。
- 写入：`close_panel` / `onUnload` 时把**所有存活实例**写回，`panels` 截断到 `max_panels`。
- 恢复时机：`open_panel` 时按 root 匹配**同一条**记录（现状是"只认最后一个 root"，双实例下会互相覆盖）。
- 仍然**默认关**（`persist = false`），理由见前作 §12.2（插件没有写 `config.toml` 的 API）。

### 4.6 全局数据的拉取点（别按实例重复做）

`seed_opened()` / `poll_diagnostics()` 是**全局事实**，只需要在"人-rate 事件"里各跑一次：
现在它们被放在 `handle_key` 顶部（`plugin.js:1653` 起），双实例下会变成"按实例跑" —— 无害但浪费。
改造后移到 `make_key` 的外层（被任一实例的按键触发一次即可）。

---

## 5. 迁移步骤（每步都能编译 + 全绿，可单独 revert）

| 步 | 内容 | 验收 |
|---|---|---|
| **1** | 建 `G`/`new_panel()`；`S` 暂时保留为"唯一实例"的别名（`const S = new_panel(null)`），行为零变化 | node 27 绿 · 集成 19+1 绿 |
| **2** | 纯函数不动；非纯函数加 `P` 实参，把 `S.` 机械替换成参数（分批：视图 → 树/根 → 搜索 → 面板） | 每批都跑 node + filetree 集成组 |
| **3** | `render`/`onKey` 改工厂 + `G.panels` 注册表 + `onClose` 摘表；此时仍**只允许一个实例**（开第二个直接 echo 拒绝） | 现有一切用例绿 |
| **4** | 放开第二个实例：`max_panels`、active 判定、§4.3 的命令语义 | 新增双实例集成用例（见 §6） |
| **5** | 持久化 v2 + v1 迁移 | 单测（纯 encode/decode）+ 一次手工 `:filetree` 重启验证 |
| **6** | 文档收尾：插件文件头、`plugins/README.md`、`docs/plugin-api.md:155`、本文件状态 | `check_api_surface` / `check_doc_surface` 全绿 |

第 2 步是**唯一的大块机械劳动**（≈230 处替换）。脚本化建议：按字段名逐组替换
（`S.cursor` → `p.cursor` …），每替换一组就 `node --check` + 跑 node 测试；纯函数里出现 `S.` 的一律是信号（说明它不是纯函数，要人工看一眼）。

---

## 6. 测试矩阵

| 层 | 用例 | 断言 |
|---|---|---|
| node | `new_panel` 形状（纯数据，无 helix 依赖） | 两个实例字段互不共享（`tree`/`flat`/`grep` 不是同一对象引用） |
| node | 现有 27 个纯逻辑用例 | 不改一行，全绿（重构不碰纯函数） |
| 集成 | 双实例隔离 | 开 A(root=dirA) + B(root=dirB)：A 里展开 `src`、B 里不动；`H` 全局生效；两边 `cursor` 不同步 |
| 集成 | 关闭其一 | 关 A 后 B 仍可用（root/光标/过滤都在），且 `:filetree` 的语义按 §4.3 |
| 集成 | reveal 目标 | 焦点在 B 时 `:filetree-reveal` 只改 B；文件在 A 的 root 下时按优先级②落 A |
| 集成 | `:cd` | A.root=旧 cwd → A 跟着变；B.root=别的 → 不动 |
| 集成 | 搜索各归各 | A 里 `/foo` 过滤、B 里 `G bar` 内容搜索，互不串味 |
| 集成 | 上限 | 第 3 个实例被拒 + 状态栏有说明 |
| 纯逻辑 | state v2 往返 | `decode(encode(x)) == x`；v1 文件迁移成单实例 |

---

## 7. 风险与回滚

| 风险 | 缓解 |
|---|---|
| **状态归属错一处** ⇒ 两个面板互相串味，且难复现 | 步骤 2/3 拆批 + 每批跑测试；把"哪些字段属于谁"写死在 `new_panel()` 的注释里 |
| active 判定不准（§2） | 命令里用优先级阶梯兜底；文档明说"鼠标切焦点后未按键"的窗口 |
| `:filetree` 的 toggle 语义变化让老用户困惑 | 保持"无→开，有 1→关它"的行为不变，只在有 2 个时才需要 active |
| 两实例各起一个 `git status` / `rg` | 进程本来就异步；git 结果按 root 缓存（已在实例里），不会重复读 |
| 内存（两棵树 + 两个 nucleo 会话） | `close_panel` / `onUnload` 已经有 `drop_flat`；上限 2 |
| 回滚 | 每步单独 revert；第 3 步之后若发现方向不对，退回"只允许一个实例"就是当前生产行为 |

---

## 8. 工作量

| 步 | 估量 |
|---|---|
| 1（G/P 骨架 + 别名） | ~40 行新增，2 处改动 |
| 2（机械重构 230 处） | 大头：**半天到一天**，按字段分批 |
| 3（工厂 + 注册表 + onClose） | ~60 行 |
| 4（第二实例 + 命令语义 + 上限） | ~80 行 + 4 个集成用例 |
| 5（持久化 v2 + 迁移 + 单测） | ~60 行 + 2 个 node 用例 |
| 6（文档） | ~30 行 |

整体在"一次专注的会话"量级，**不建议**和搜索/过滤类的功能改动并行做（它们的函数签名会一起变）。

---

## 9. 建议的核心增强（可选，但能消掉本文最大的不确定性）

| 增强 | 改动量 | 收益 |
|---|---|---|
| `pane.list()` 的条目加一个 `panel_id`（`PluginPanel::id()`），无则 `null` | `helix-term/src/compositor.rs::sync_panes_cache` 一处 + `helix.d.ts` 一行 | 插件能**精确**判定哪个实例被聚焦，`G.active` 的启发式可以退休 |
| 或发一个 `pane-focus` 事件（`{leaf_id, panel_id?}`） | 同上 + 一次 emit | 同上，且不用轮询 |

两者都属"事件面不完整"类问题（与 §13.1 的 `:buffer-close` 缺事件、§14.1 的 `lsp-diagnostics` 缺发射点是同一族）。

---

## 10. 落地记录：第 1–3 步（2026-11-03）

> **状态：第 1–3 步已实现，仍只允许一个实例。** 第 4 步（放开第二个实例 + active 优先级阶梯）、
> 第 5 步（持久化 v2）、第 6 步（文档收尾）未做。

### 10.1 做了什么

| 内容 | 结果 |
|---|---|
| `S` 单例 → **`G`（全局）+ `new_panel()`（实例）** | `plugin.js:463` 一带；归属注释写在块首（加字段前必读） |
| **39 个非纯函数**参数化（`panel` 首参 + 过渡期 `panel = panel || LIVE` 回退） | 纯函数一个没动 —— 27 个 node 用例原样全绿 |
| **7 个入口**（`open_panel`/`close_panel`/`toggle_panel`/`pick_open`/`on_buffer_open`/`render`/`handle_key`）自己解析目标实例 | `target_panel()`（第 3 步返回唯一实例；第 4 步换成 active 阶梯） |
| `render`/`onKey` → **工厂**：`make_render(panel)` / `make_key(panel)` | 状态进闭包（支持双实例的支点）；`G.active` 在按键时更新 |
| **`G.panels` 注册表 + `onClose` 摘表** | 布局恢复 / `:plugin-reload` 关面板也会清状态（以前没有 `onClose` ⇒ 留尸） |
| `open_panel` 第二个实例**直接拒绝**（echo 说明） | 第 3 步的护栏；第 4 步放开 |
| `cwd-change` 语义改对 | 只重设"root 就是旧 cwd"的实例（用 `G.last_cwd`），为双实例铺路 |
| 导出改为 `G` / `new_panel` / `render_with` | node 测试改用显式实例（不再有"全局单例"依赖） |

引用量复核：`plugin.js` 里 **`S.` 出现 0 处**；`panel = panel || LIVE` **39 处**（第 4 步要逐条删掉，
删完就等于"每个调用点都必须传实例"，是第 4 步的验收条件之一）。

### 10.2 过程中踩到的两个坑（都是脚本重构的经典错，写下来防复发）

1. **偏移漂移**：先收集所有 `match.start()` 再逐个插入，会让后续偏移全部失效 ——
   第一次就是这么做，结果把 `return "handled"` 改成了 `rpanel, eturn "handled"` 这种鬼东西。
   正确姿势：**每改一处就重新扫描**（函数级 fixpoint），或从后往前插入。
2. **"已经有 panel 了"的误判**：给调用点补第一个实参时用 `(?!panel)` 当守卫，会把
   `node_at(panel.cursor)`（字段改名后本来就把 panel 当**第二**个实参用）当成"已补过"而跳过 ⇒
   `node_at` 收到 `idx = panel`，`rows[panelObject]` 是 `undefined`，于是**按键静默失效**
   （不报错、光标不动）。教训：这类"参数顺序"错误在 JS 里不会抛，只能靠**人工检查 + 集成测试**抓；
   已专门加了一条：凡是"首实参恰好是 `panel.`"的调用，都要补成 `(panel, panel.…)`。

### 10.3 验证（本次实际执行）

| 命令 | 结果 |
|---|---|
| `node --test plugins/filetree/tests/filetree.test.js` | 27 passed（测试改用 `new_panel()` / `G`） |
| `cargo test -p helix-term --features integration --test integration filetree` | 19 passed |
| `cargo test ... --test integration plugin_lsp` | 17 passed（诊断事件未受影响） |
| `tsc --noEmit` · `check_api_surface` · `check_capability_surface` · `check_doc_surface` · `check_signatures` | 全绿 |
| `cargo xtask sync-runtime` | 已同步 |

### 10.4 第 4 步的待办（清单化，接手即可开工）

1. 删掉 39 处 `panel = panel || LIVE`（删完才能证明"每个调用点都传了实例"）。
2. `target_panel()` → `G.active` 优先 + §4.3 的优先级阶梯（active → root 含当前文件 → 唯一 → 新建）。
3. 放开 `open_panel` 的护栏；加 `max_panels`（默认 2）配置项与"超限"提示。
4. 新增集成用例（§6 的 8 类）。
5. `:filetree` 在两个实例下的语义（关 active）。

---

## 11. 落地记录：第 4 步（2026-11-03）

> **状态：双实例已放开。** 剩余：§5 的第 5 步（持久化 v2 / v1 迁移）与第 6 步（文档收尾里
> `plugin-api.md:155` 的 rail 口径订正）。

### 11.1 做了什么

| 内容 | 结果 |
|---|---|
| **删掉 39 处** `panel = panel || LIVE` 过渡回退 | 现在"每个调用点必须传实例"，漏传会立刻炸（而不是悄悄用别人的状态） |
| `LIVE` 变量删除，注册表 `G.panels` + `G.active` 接管 | 目标解析见下 |
| **`target_panel()` 阶梯** | `G.active` → 只剩一个 → `null` |
| **`panel_for_path(path)`** | root 是 `path` 祖先的实例（`path_join` 判前缀，不手拼 `/`） |
| `resolve_panel(path)` = 归属优先 → active | `:filetree-reveal` 与 `buffer-open` 的跟随都用它 |
| `open_panel(root?)` 接受 root、返回实例；`MAX_PANELS`（配置 `max_panels`，默认 2）超限 echo 拒绝 | 新面板**必须** `set_root`（显式给 root 时以前会漏建树 ⇒ 空面板，实测踩到） |
| 开第二个面板前 `helix.pane.focus(0)` | 不这么做的话第二个面板会**切分第一个面板**（实测 8 列 vs 24 列）；先回编辑器才是 32+32 |
| `close_panel(target?)` / `toggle_panel()` 语义 | 无 → 开；有 → 关**目标**（active；只剩一个就是它） |
| 新命令 `:filetree-open [dir]` | 显式开第二个（root = 参数 / 当前文件所在目录 / cwd）；`:filetree` 仍是开/关切换，不能用来加第二个 |
| `export` 补 `close` / `target` | 别的插件与集成测试都能用 |

### 11.2 一处**按实测改掉**的设计

§4.3 原本写 reveal 的优先级是"① active → ② root 是祖先"。落地时发现 ① 会让 reveal 把 **active 面板的
root 整个拽走**（因为文件常常不属于 active 的 root）。改成 **归属优先**：

```
resolve_panel(path) = panel_for_path(path) || target_panel()
```

"文件属于谁"是无歧义信息，先用它更符合预期；只有不在任何 root 下时才动 active（那本来就只能改 root）。

### 11.3 验证

| 命令 | 结果 |
|---|---|
| `node --test plugins/filetree/tests/filetree.test.js` | **30 passed**（+`target_panel` 阶梯、`panel_for_path`、**两实例状态隔离**） |
| `cargo test ... --test integration filetree` | **22 passed**（+双面板并排 / reveal 归属优先 / `:filetree` 关 active），68s |
| `cargo test ... --test integration plugin_lsp` | 17 passed |
| `tsc` · 4 个 check 脚本 · `cargo xtask sync-runtime` | 全绿 / 已同步 |

### 11.4 第 5–6 步待办

1. 持久化 **v2**：`{version:2, panels:[{root,filter,expanded}, …]}`；v1 迁移；写回时截断到 `max_panels`；
   读入按 root 匹配（现在 `save_state` 仍然只写**目标**实例的那一条 —— 双实例下会丢另一个的状态）。
2. `plugin-api.md:155` 的 "filetree 即 rail 模式" 改为"普通侧边面板（非 rail）"（§4.4 定论）。
3. 可选：`pane.list()` 加 `panel_id` / 发 `pane-focus` 事件（§9），有了就能退休 `G.active` 的启发式。

---

## 12. 落地记录：第 5 步 · 持久化 v2（2026-11-03）

### 12.1 形状与语义

```jsonc
{ "version": 2,
  "panels": [ { "root": "/p/a", "filter": "*.rs", "expanded": ["/p/a/src"] },
              { "root": "/p/b", "filter": "",     "expanded": [] } ] }
```

| 规则 | 行为 |
|---|---|
| 一条记录 = 一个 **root**（与"哪个面板"解耦） | 下次开机按 root 匹配回来 ⇒ 两个面板各自找回自己的折叠/过滤，不会互相覆盖 |
| 写：**当前所有实例** | `close_panel` 里在摘注册表**之前**写 ⇒ 关掉最后一个面板时"上次的位置"仍记得 |
| 写：注册表为空时不写 | 关到空不该把记忆抹成 `panels: []` |
| 条数 / 每条展开数 | 截断到 `max_panels` / 每条封顶 200 个展开目录 |
| 同 root 重复 | 后者胜（保持首次出现的顺序） |
| 读：v1 老文件 | `{version:1, root, filter, expanded}` ⇒ 迁移成一条，不白丢老用户的记忆 |
| 读：坏文件 / 形状不对 | 一律空表（读盘路径不能因为一个坏文件崩） |

三个纯函数（node 可测，无 helix 依赖）：`state_encode(entries, max)` · `state_decode(raw)` ·
`state_for_root(root)`（按 root 取一条）。`save_state()` / `load_state()` 是薄壳，
在 `persist=true` 时才碰盘。

### 12.2 顺手修掉的一处会炸的代码

`onUnload` 里还留着第 3 步之前的 `save_state(panel)` / `drop_flat(panel)` —— 但 `onUnload` 是
**模块级回调**，作用域里根本没有 `panel` ⇒ 一执行就 `ReferenceError`。现在改成：

```js
onUnload: () => {
  save_state();                                  // 写全部实例
  for (const panel of G.panels.values()) drop_flat(panel);  // 释放每个 nucleo 会话
}
```

### 12.3 验证

| 命令 | 结果 |
|---|---|
| `node --test plugins/filetree/tests/filetree.test.js` | **35 passed**（+v2 往返/去重/封顶、v1 迁移、坏文件降级、`save_state` 写全部实例、`state_for_root`、首次运行空表） |
| `cargo test ... --test integration filetree` | 22 passed |
| `tsc` · 4 个 check 脚本 · `cargo xtask sync-runtime` | 全绿 / 已同步 |

> 测试桩改成 `get_config() => ({ persist: true })` + 内存文件系统（`write_plugin_file` / `read_file`），
> 这样"写盘 → 读回"的**接线**也被覆盖，而不是只测纯函数。桩不会真的写开发机的配置目录。

### 12.4 剩余（§5 第 6 步 + 可选核心增强）

- 文档收尾已完成一半（`plugins/README.md`、`docs/plugin-api.md:155` 的 rail 口径已订正，
  插件文件头也写了双实例），剩下的是本文件的"状态"行按最终形态再核一遍。
- 可选：`pane.list()` 加 `panel_id` / 发 `pane-focus` 事件（§9）—— 有了就能退休 `G.active` 启发式。
