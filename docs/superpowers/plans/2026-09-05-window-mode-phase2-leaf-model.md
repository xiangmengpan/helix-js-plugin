# Window 模式二期（叶=窗口统一模型）实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 让 window mode 统一控制所有窗口：叶=可编辑窗口（LayoutTree 唯一几何源），`:vsplit` 家族改道产生 LayoutTree 叶而非 core view-tree 分裂，同 doc 多叶编辑互通保留。

**架构：** view 仍注册于 `editor.tree`（core 路由 `current!` = `tree.get(tree.focus)`，可见性检查 `tree.traverse()` 依赖它），但 tree 不再承担几何布局；叶子激活时同步 `editor.tree.focus`，编辑命令经 leaf 0 键位处理命中活动叶的 view/doc。`EditorView`（leaf 0）从"整树遍历渲染"收敛为"渲染自己持有的 view"。

**技术栈：** Rust（helix-view core tree / helix-term compositor LayoutTree / window mode）；既有集成测试框架 `test_key_sequences`（须持 `PLUGIN_TEST_LOCK`）。

**规格：** `docs/superpowers/specs/2026-09-05-window-mode-phase2-leaf-model-design.md`

---

## 文件结构（将新建/修改）

- `helix-view/src/tree.rs`：新增注册不抢焦点的 view 的入口（spike 后定形：如 `register_flat(view) -> ViewId`——入 arena + 挂根容器 + **不动 focus/不 recalculate 几何**）。（修改）
- `helix-view/src/editor.rs`：`Editor::switch/new_file` 的 Split 分支不再作为主分裂路径（P2 起弃用），保留 Replace；提供按 id 同步 focus 的既有能力（`tree.focus` 赋值已公开）。如需提供 `sync_focus_to_view(view_id)` 辅助则加。（修改）
- `helix-term/src/ui/editor.rs`：`EditorView::render` 由整树循环改渲染单 view；`EditorView` 持有自己 view 的 id 或渲染回调；render 前用自身 area 校正 `view.area`。（修改）
- `helix-term/src/ui/buffer_leaf.rs`：view 由游离改为注册态；`handle_event` 不再 Ignored 到底——激活时兜底编辑已因 tree.focus 同步生效（若需保持 Ignored 也行，验证后定）。（修改）
- `helix-term/src/ui/layout.rs`：叶子激活单一入口（`focus(id)` 及 split/remove 的焦点迁移点）暴露"活动叶的 view_id"供 compositor 同步；`LayoutTree` 增加"每叶可查询其持有 view"。（修改）
- `helix-term/src/compositor.rs`：window mode 键位加 `v/s/n`；所有叶子激活/关闭/拆分路径后调用 `sync_editor_focus(cx)`（设 `editor.tree.focus = 活动叶的 view_id`，无 view 的叶/terminal 不设）。（修改）
- `helix-term/src/commands/typed.rs`：`vsplit/hsplit(无参)`、`vsplit_new/hsplit_new`、`open_impl` Split 分支、`goto_file` Split 分支改道为"compositor 开叶 + 新 view 注册"；保留命令名与签名。（修改）
- `helix-term/src/commands.rs`：`split()`（6013 附近）/`goto_file_impl`(1347/1351) 改道。（修改）
- `helix-term/src/application.rs`：启动多文件参数分行开叶（208/236）。（修改）
- 测试：`helix-term/tests/test/window_mode.rs`（扩展）、新 `helix-term/tests/test/window_split.rs`（vsplit 改道断言）、既有 plugin/buffer_open 相关测试同步修正。

---

## 阶段 P1：输入路由地基（让任意叶子可编辑）

### 任务 1：spike——tree 注册"不参与布局的 view"

**文件：** `helix-view/src/tree.rs`、`helix-view/src/tree.rs` 测试模块

- [ ] **步骤 1：写失败测试（spike 用最小断言）**

在 `tree.rs` 测试模块添加（参考既有测试风格，`tree.rs:740` 附近为测试区）：
```rust
#[test]
fn register_flat_view_is_gettable_and_traversable() {
    let mut tree = Tree::new(Rect::default());
    let v1 = tree.register_flat(View::new(DocumentId::default(), vec![]));
    assert_eq!(tree.get(v1).id, v1, "arena 可取");
    assert!(
        tree.traverse().any(|(id, _)| id == v1),
        "traverse 可见（供 remove_empty_scratch 等检查）"
    );
}
```
预期：FAIL（`register_flat` 不存在）。

- [ ] **步骤 2：确认 register_flat 的语义与 `insert`/`remove` 差异**

读 `tree.rs` 现状 `insert`(106)/`remove`/`recalculate`/`traverse`(444)。`insert` 会：挂到 focus 的父容器、`self.focus = node`、`recalculate()`。设计目标（spec §3.3）：入 arena、可 traverse，但不动 focus、不 recalculate。实现 `register_flat`：arena insert + 挂到根容器 children + 不 recalculate。**若挂根容器会破坏既有单 view 树的不变式（如 recalculate 依赖 children 布局），退回"瞬态 insert+remove"保留 id 方案并在 `register_flat` 内封装**——验收不变式只有"get/traverse 可见 + focus 不变"。

- [ ] **步骤 3：实现 `register_flat`**

按步骤 2 结论实现（优先：arena insert、根容器 children 追加、不动 focus、不 recalculate；并在方法文档写明"不参与几何布局，叶子渲染各自设 view.area"）。

- [ ] **步骤 4：跑测试确认通过**

运行：`cargo test -p helix-view tree::tests::register_flat_view_is_gettable_and_traversable`
预期：PASS；`cargo test -p helix-view` 全绿（无回归）。

- [ ] **步骤 5：Commit**

```bash
git add helix-view/src/tree.rs
git commit -m "feat(view): tree.register_flat——注册不参与布局/不动焦点的 view(spike 验收 get/traverse)"
```

### 任务 2：EditorView 渲染收敛——leaf 0 只渲染自己的 view

**文件：** `helix-term/src/ui/editor.rs`（render 1676-1740、render_view 76）、`helix-term/src/compositor.rs`（set_main_editor）

- [ ] **步骤 1：写失败测试（集成，防双画）**

`helix-term/tests/test/window_mode.rs` 增（`buffer_open_creates_leaf_with_content` 旁）：
```rust
#[tokio::test(flavor = "multi_thread")]
async fn editor_leaf_renders_single_view_not_whole_tree() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;
    // 通过 :vsplit 制造第二个 view 后,leaf 0 渲染不应出现"两个 buffer 内容重叠/双画"
    // (先期用 buffer_open 一个不同文件,断言 leaf0 区不渲染第二个文件内容)
    ...
}
```
（实现细节随 P1 落地校准；关键断言：leaf 0 区域只含自身 view 的内容，不含树中其他 view 的内容。）

- [ ] **步骤 2：跑测试确认失败**
运行：`cargo test -p helix-term --test integration --features integration editor_leaf_renders_single_view_not_whole_tree`
预期：FAIL（现状整树循环渲染，双画或内容重叠）。

- [ ] **步骤 3：实现收敛**

`EditorView` 增加自身 view 定位：构造时不持有 view id，渲染时按如下规则（先验证现状后再定最简）——
- 规则 A（推荐先试）：leaf 0 渲染 `tree.focus` 且仅当 **focus 的 view 属于 leaf 0 的持有**（leaf 0 持有 = 启动时首个 view，记在 compositor/LayoutTree 侧：leaf 0 组件可带 `view_id: Option<ViewId>`，`set_main_editor` 后由首个 view 填充）。
- 实现：`render` 去掉 `for (view, _) in tree.views()` 循环，改为取"leaf 0 持有的 view"（必要时在 render 前 `view.area = area` 校正——render_view 依赖 view.area），渲染单份。
- 若渲染正确性依赖 `tree.focus`（autoInfo/光标等 core 状态），保留"leaf 0 活动时 tree.focus=其 view"的不变式即可。

- [ ] **步骤 4：跑测试确认通过 + 全量回归**

运行步骤 2 命令与 `cargo test -p helix-term --test integration --features integration window_mode`（须持锁串行）
预期：PASS；既有 window_mode/plugin 测试不回归。

- [ ] **步骤 5：Commit**
```bash
git add helix-term/src/ui/editor.rs helix-term/src/compositor.rs helix-term/tests/test/window_mode.rs
git commit -m "refactor(ui): EditorView 只渲染自身 view,不再整树遍历(防多叶双画)"
```

### 任务 3：叶子激活 ⇄ tree.focus 同步

**文件：** `helix-term/src/ui/layout.rs`（focus 231 / handle_event 800 / remove 焦点迁移）、`helix-term/src/compositor.rs`（window_mode_focus/close、split_leaf 调用点）

- [ ] **步骤 1：写失败测试**

集成（window_split 或 window_mode.rs）：两叶（leaf0 编辑器 + buffer_open 的 BufferLeaf B），window mode `l`/`hjkl` 聚焦 B 后输入 `i` + 文本，断言 **B 的 doc 被编辑**（非 leaf0 的 doc）。先确认现状失败（B 不可编辑，编辑落 leaf0 doc）。

- [ ] **步骤 2：跑测试确认失败**
运行：`cargo test -p helix-term --test integration --features integration <新测试名>`
预期：FAIL（编辑落在 leaf0 的 doc，或报错）。

- [ ] **步骤 3：实现同步**

- `LayoutTree`：叶子持有"其组件是否承载 view、view_id 多少"的查询（`active_view_id() -> Option<ViewId>`：active 组件若为 BufferLeaf/EditorView 则返回其 view_id）。
- `compositor.rs`：window mode 的聚焦（`window_mode_focus`）、关闭迁移（`window_mode_close`）、Enter 确认、`split_leaf`/`split_leaf_prealloc` 后、`remove` 后——统一在返回前调 `self.sync_editor_focus(cx)`：`if let Some(vid) = main_tree.active_view_id() { cx.editor.tree.focus = vid; }`（terminal/panel 无 view → 不设）。
- **收敛单一入口**：若 `LayoutTree::focus(id)` 无法直接改 editor（无引用），则 compositor 层包装所有会变更活动叶的调用，保证不漏（spec §8：遗漏面收敛）。在 `LayoutTree::focus` 等变更点加注释"调用方必须随后 sync_editor_focus"或经回调，实现期按借用结构选一。

- [ ] **步骤 4：BufferLeaf 转正**

`buffer_leaf.rs`：view 由瞬态 insert+remove 游离态改为 `editor.tree.register_flat(view)` 注册（typed.rs:4976 附近 OpenBufferLeaf 的 hack 移除）；`doc.ensure_view_init(id)` 保留；render 不变（`view.area = area`）。BufferLeaf 仍 `Ignored`（按键兜底 leaf 0 → leaf 0 键位处理按 tree.focus=该叶 view 命中正确 doc）——验证兜底链成立即不改。

- [ ] **步骤 5：跑测试 + 全量回归**

运行：步骤 2 测试 + `cargo test -p helix-view` + `cargo test -p helix-js` + `cargo test -p helix-term --test integration --features integration plugin window_mode buffer_open`（PLUGIN_TEST_LOCK 串行下）
预期：编辑落 B 的 doc；同 doc 双叶编辑互通断言通过；无回归。

- [ ] **步骤 6：Commit**
```bash
git add helix-term/src/ui/layout.rs helix-term/src/compositor.rs helix-term/src/ui/buffer_leaf.rs helix-term/src/commands/typed.rs
git commit -m "feat(ui): 叶子激活同步 editor.tree.focus——BufferLeaf 可编辑(同 doc 多叶编辑互通)"
```

**P1 验收：** buffer_open 叶聚焦后可编辑且编辑命中自身 doc；同 doc 两叶编辑互通；window mode 全部既有功能无回归。

---

## 阶段 P2：分裂命令改道（消灭 :vsplit 逃逸）

### 任务 4：:vsplit/:hsplit 无参 → 同 doc 新叶

**文件：** `helix-term/src/commands/typed.rs`（vsplit 2224/hsplit 2238、open_impl）、`helix-term/src/commands.rs`（split() 6013 附近）、`helix-term/src/compositor.rs`

- [ ] **步骤 1：写失败测试**

新 `helix-term/tests/test/window_split.rs`（持 PLUGIN_TEST_LOCK）：
```rust
#[tokio::test(flavor = "multi_thread")]
async fn vsplit_creates_controllable_same_doc_leaf() -> anyhow::Result<()> {
    // :vsplit(无参) → 两叶;右叶渲染同 buffer;window mode 可聚焦/编辑右叶;左叶编辑→右叶同步
    ...
}
```
断言：`:vsplit` 后 LayoutTree 叶数=2、类型含 editor +（同 doc 叶）；window mode `l` 聚焦新叶后编辑命中**同一 doc**；一边编辑另一边渲染同步。现状预期 FAIL（vsplit 走 view-tree 分裂，叶数仍 1）。

- [ ] **步骤 2：跑测试确认失败**
运行：`cargo test -p helix-term --test integration --features integration window_split`
预期：FAIL（叶数断言 1≠2 等）。

- [ ] **步骤 3：实现改道**

`:vsplit`/`:hsplit` 无参分支不再走 `editor.switch(id, Action::Split)`；改为：
1. `editor.open`/取当前 doc（不改动 focus 的方式取 doc_id）；
2. `tree.register_flat(new_view(doc_id))` 注册新 view（复制当前 view 的 selection/offset——复用现有 split() 里"match selection/offset"逻辑片段，commands.rs:6013 附近）；
3. `compositor.split_leaf(dir, ...)` 插入承载该 view 的叶组件（BufferLeaf 泛化或同款）；
4. 活动叶迁移 → sync_editor_focus。
- 方向：`:vsplit`=H 右，`:hsplit`=V 下（沿用现状语义）。
- `editor.switch`/`Tree::split` 的 Split 分支自此不再被命令路径触发（core 保留以兼容未迁移路径，见任务 6）。

- [ ] **步骤 4：跑测试 + 回归**

运行步骤 1 测试 + window_mode/plugin 全套件回归。
预期：PASS；无 :vsplit 逃逸（产物可被 window mode 控制）。

- [ ] **步骤 5：Commit**
```bash
git add helix-term/src/commands/typed.rs helix-term/src/commands.rs helix-term/src/compositor.rs helix-term/tests/test/window_split.rs
git commit -m "feat(ui): :vsplit/:hsplit 无参改道 LayoutTree 叶(同 doc 双视图,可被 window mode 控制)"
```

### 任务 5：带参与新建变体改道（:vsplit path / gf / -new / 启动多文件）

**文件：** `typed.rs`（vsplit_new 2252/hsplit_new 2262、open_impl）、`commands.rs`（goto_file_impl 1347/1351）、`application.rs`（208/236）

- [ ] **步骤 1：写失败测试**（window_split.rs）

`:vsplit <path>` 开文件到新叶且新叶可编辑；`:vsplit-new` 开空 buffer 新叶；`gf` 打开目标到新叶；启动多文件参数各占一叶（AppBuilder 多文件）。按各入口补断言，现状预期 FAIL。

- [ ] **步骤 2：跑测试确认失败**
运行：`cargo test -p helix-term --test integration --features integration window_split`
预期：FAIL。

- [ ] **步骤 3：实现**

- `open_impl(…, Action::Split)`、`goto_file_impl(…, Split)`、`new_file(Split)`（typed.rs:754 的 new_file 命令与 -new 变体）：改道为"新 doc（Load/新建）+ register_flat + split_leaf"。
- `application.rs`：多文件启动逐文件开叶（首个 leaf 0，其余 split_leaf 新叶，方向沿用现状 Split 语义）。
- 抽出共用 helper（如 `term 层函数 open_in_new_leaf(editor, compositor, doc_id, dir)`），四个入口共用，杜绝复制。

- [ ] **步骤 4：跑测试 + 回归**
运行步骤 1 测试与全套件。
预期：PASS。

- [ ] **步骤 5：Commit**
```bash
git add helix-term/src/commands/typed.rs helix-term/src/commands.rs helix-term/src/application.rs helix-term/tests/test/window_split.rs
git commit -m "feat(ui): vsplit path/gf/-new/启动多文件 改道新叶(共用 open_in_new_leaf)"
```

### 任务 6：核心 Split 语义收口

**文件：** `helix-view/src/editor.rs`（switch 1942 Split 分支、new_file 2075）、`helix-view/src/tree.rs`（split）

- [ ] **步骤 1：审计残留触发点**

grep `Action::VerticalSplit|HorizontalSplit|tree.split(` 全仓（term 层命令已改道的应只剩未被迁移的调用）。把每处归入"已改道（删除）"/"core 内部保留（注释说明）"。

- [ ] **步骤 2：决定与收口**

若 term 层零调用 → 删除命令路径中对 Split action 的依赖；`Editor::switch`/`new_file` 的 Split 分支标记 `#[allow(dead_code)]` 或删除（含其测试），并在文档注明"分裂统一走 LayoutTree"。若仍有合法触发（如宏回放旧序列）则保留并注释。
运行 `cargo test -p helix-view -p helix-term` 全量确认无回归。

- [ ] **步骤 3：Commit**
```bash
git add helix-view/src/editor.rs helix-view/src/tree.rs helix-term/src/commands/typed.rs
git commit -m "refactor(core): 分裂语义收口至 LayoutTree——废弃 editor Split action 命令路径"
```

**P2 验收：** 所有分裂入口产物为 LayoutTree 叶且被 window mode 完全控制；无 :vsplit 逃逸路径残留。

---

## 阶段 P3：window mode 创建键 + 收尾

### 任务 7：window mode 键位 v/s/n

**文件：** `helix-term/src/compositor.rs`（227-296 键位表）、`which-key`/hint 源

- [ ] **步骤 1：写失败测试**（window_mode.rs）

C-w 进模式 → `v` 右分同 doc、`s` 下分同 doc、`n` 新空 buffer；`x` 可关新叶；Esc 退出后编辑正常。现状预期 FAIL（`v`/`s`/`n` 被模式吞掉无操作）。

- [ ] **步骤 2：跑测试确认失败**
预期：FAIL。

- [ ] **步骤 3：实现**

键位表新增分支（映射到任务 4/5 的叶创建 helper）：
- `v` → 同 doc 分屏 H（右）
- `s` → 同 doc 分屏 V（下）
- `n` → 新空 buffer 分屏（方向沿用 v/s 或当前叶方向，按 UX 测试定——预设 v/s 键选向、`n` 用当前活动叶方向缺省 V）
创建后保持模式（可连续操作），Esc 退出；创建后的活动叶同步 tree.focus。

- [ ] **步骤 4：跑测试 + which-key 提示更新**
运行步骤 1 测试；更新模式键位提示（一期 hint 源，compositor.window_mode_hint）含 v/s/n。
预期：PASS。

- [ ] **步骤 5：Commit**
```bash
git add helix-term/src/compositor.rs helix-term/tests/test/window_mode.rs
git commit -m "feat(window): 模式键 v/s/n 创建同 doc/新 buffer 叶"
```

### 任务 8：旧 view-tree 多 view 会话迁移 + 文档

- [ ] **步骤 1：写迁移测试**

模拟"升级前遗留"：构造 tree 内非 focus 的第二个 view（直接调 tree.split 或历史 :vsplit 路径），启动后断言：自动补建 LayoutTree 叶包裹该 view、可被 window mode 控制、无双画。现状 FAIL。

- [ ] **步骤 2：实现启动迁移**

启动流程（application.rs 建 Editor 后）检测 `tree` 中非 leaf0 持有的 view → 逐个 `split_leaf` 补叶并 register/绑定；随后正常 sync。

- [ ] **步骤 3：文档更新**

`docs/plugin-api.md` 与 README 窗口段：明确"窗口=叶；分裂入口统一走 window mode 或 :vsplit 族命令（同为新叶）；core view-tree 不再承担窗口分裂"。

- [ ] **步骤 4：跑测试 + Commit**
运行：全套件 + 迁移测试 PASS。
```bash
git add helix-term/src/application.rs helix-term/tests/test/window_mode.rs docs/
git commit -m "feat(ui): 启动迁移遗留 view-tree 分裂为叶 + 文档更新"
```

---

## 阶段 P4：测试与回归

### 任务 9：补全断言 + 全量回归

- [ ] **步骤 1：按 spec §6 逐条核对补测**
同 doc 关闭一叶不关文档（remove_empty_scratch 遍历检查）；mode 内 x 关活动叶后 focus/tree.focus 迁移正确；terminal/panel 叶激活不设 tree.focus 仍可控制；宏录制不录模式键。
- [ ] **步骤 2：全量回归**
`cargo test -p helix-view`、`cargo test -p helix-js`、`cargo test -p helix-term --test integration --features integration`（全套，含 PLUGIN_TEST_LOCK 串行）、`cargo clippy -p helix-view -p helix-term`、`cargo fmt`。
预期：全绿。
- [ ] **步骤 3：Commit**
```bash
git add -A
git commit -m "test(window): 二期补全断言 + 全量回归"
```

---

## 自检备注

- **规格覆盖：** §3.1→任务1-3；§3.2→任务3；§3.3→任务1；§3.4→任务4/5/8；§3.5→任务7；§3.6→任务2；§5 边界→任务9；§6 测试→各任务步骤1 + 任务9。
- **类型一致性：** `register_flat` 名在任务1定义、任务3/4/5 引用；`active_view_id` 任务3 定义、任务4-8 依赖；`open_in_new_leaf` 任务5 定义、任务7 复用。均先定义后引用。
- **占位符：** 集成测试骨架处标注"实现期校准"——这是有意为之的 spike 决策点（渲染规则 A/B、n 键方向），均有明确取舍指引，非 TODO。
