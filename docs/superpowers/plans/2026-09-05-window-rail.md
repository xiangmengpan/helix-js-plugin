# Window Rail（侧栏轨道）实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** filetree 固定为左/右全高 rail，不被 window mode 分割；`C-w h/l` 聚焦后用 j/k/Enter 浏览，`l`/`Esc` 回 main。

**架构：** LayoutTree 根恒为 `Split(rail叶 | main子树)`；rail 由 `open_panel(side left/right)` 注册；window-mode 操作护栏对 rail 免疫或落 main；焦点进出 rail 有明确键位模型。

**技术栈：** Rust（helix-term layout/compositor/typed）+ 用户侧 filetree.js 零改动目标。

**规格：** `docs/superpowers/specs/2026-09-05-window-rail-design.md`

---

## 文件结构（将新建/修改）

- `helix-term/src/ui/layout.rs`：`LayoutTree` 加 rail 登记/is_rail/结构护栏；split 目标落 main；dump 标记。（修改）
- `helix-term/src/compositor.rs`：window-mode 键位护栏（v/s/n 在 rail 上落 main、x 关 rail、z/f/swap 免疫）；`C-w h/l` 进出 rail；focus 穿越规则；活动叶记忆。（修改）
- `helix-term/src/commands/typed.rs`：`OpenPanel` 处理器改为"rail 注册"路径（side left/right → 注册/替换 rail；bottom 维持叶）。（修改）
- `helix-term/src/ui/plugin_panel.rs`：暴露 `side()`（rail 判定用）；必要时 rail 态未知键 Consumed。（修改）
- 测试：`helix-term/tests/test/window_mode.rs` 或新 `helix-term/tests/test/window_rail.rs`。
- 文档：`README.md`/`docs/plugin-api.md` 窗口段。（修改）

---

## 任务 1：LayoutTree rail 登记与结构护栏

**文件：** `helix-term/src/ui/layout.rs`

- [ ] **步骤 1：写失败单元测试**（layout.rs 测试模块）

```rust
#[test]
fn rail_registration_shapes_root_and_is_rail() {
    // 初始:仅编辑器叶(0)。注册左 rail(叶 1)→ root = Split(1 | main),is_rail(1)。
    // main 内再分裂(叶 2)→ 仍在 main 子树,root 形状不变,rail 叶全高份额。
    // dump 输出 leafs 带 rail:true。
}
```
预期：FAIL（无 rail API）。

- [ ] **步骤 2：实现登记**

`LayoutTree` 加 `rail: Option<u64>`（v1 单 rail，先做 left；右侧作为对称扩展字段 `rail_side` 或按 `plugin_panel.side` 取）。API：
- `register_rail(&mut self, leaf_id: u64, side)` → 若 rail 已存在先移除旧 rail 叶；把 leaf 挂为 root 边缘子（左=first/右=second），main 子树为另一侧；返回旧 rail id（调用方负责 component 清理）。
- `is_rail(&self, id) -> bool`；`rail_leaf() -> Option<u64>`；`unregister_rail(id)`。
- 结构护栏：现有 `split_side*`/`split_leaf` 若活动叶是 rail → 目标改为 main 活动叶（内部先 `focus` 到 main 最左/代表叶再分裂，或直接拒绝并返回——**先实现"拒绝并保持焦点"最简语义，集成期若体验差再改落 main**；spec 预设"操作落到 main"→ 实现落 main：split 前把 active 换到 main 首个叶）。
- `dump()`：LeafInfo 加 `rail: bool`。

- [ ] **步骤 3：跑测试确认通过**
运行：`cargo test -p helix-term layout::test` 相关 + 无回归。

- [ ] **步骤 4：Commit**
```bash
git add helix-term/src/ui/layout.rs
git commit -m "feat(layout): rail 登记/is_rail/root 形状护栏 + dump rail 标记"
```

---

## 任务 2：open_panel left/right → rail 注册；操作政策护栏

**文件：** `helix-term/src/commands/typed.rs`（OpenPanel 处理 ~4829 区）、`helix-term/src/compositor.rs`、`helix-term/src/ui/plugin_panel.rs`

- [ ] **步骤 1：写失败集成测试**

`window_rail.rs`（新文件，持 PLUGIN_TEST_LOCK，pump 模式）：
- `:filetree`（用户插件）或直接 plugin-load 一个 open_panel(left) 的脚本 → 断言 leaf 数 2、`layout()` root 形状 = rail 在左、rail 叶高 == 全高（leaf_rect 或 dump 断言：rail 是 root first、main 是其 second）。
- `C-w v`（在 main 编辑叶）→ 仍 3 叶、rail 依然贴边（dump tree 形状 root = Split(rail | main-split)）。
预期：FAIL（现在 open_panel(left) 是普通分裂叶，root 形状不保证）。

- [ ] **步骤 2：实现**

- typed.rs `UiRequest::OpenPanel`：`side == "left"/"right"` → 构造 PluginPanel 后调 `compositor.register_rail(editor, panel_id, side)` 路径（替换旧 rail；旧 rail 叶若为 BufferLeaf 无关，是 PluginPanel → close_leaf_clean 或 remove_leaf）；`bottom` 维持 `split_leaf_with_ratio`。
- 护栏接入 compositor window-mode 处理器：`window_mode_split/swap/resize/zoom/minimize` 与 `remove_leaf/close` 对 `is_rail(active)` 的处理（swap/resize/min/zoom → 免；x/close_leaf → 关 rail；split → 落 main）。
- `zoom` 语义：main 内 zoom 时 rail 不被隐藏（LayoutTree::zoom/unzoom 渲染路径确认 rail 不在 zoomed 独占范围——若 zoom 渲染只画 zoomed 叶需特判保留 rail）。

- [ ] **步骤 3：跑测试 + 全量相关模块回归**
运行：`window_rail` + `window_mode`/`window_split`/`plugin`(filetree 相关)。

- [ ] **步骤 4：Commit**
```bash
git add helix-term/src/commands/typed.rs helix-term/src/compositor.rs helix-term/src/ui/plugin_panel.rs helix-term/tests/test/window_rail.rs helix-term/tests/integration.rs
git commit -m "feat(ui): open_panel left/right → rail 注册;window-mode 政策护栏(rail 免疫/落 main/x 关 rail)"
```

---

## 任务 3：焦点模型——C-w h/l 进出 rail、l/Esc 回 main、浏览键消费

**文件：** `helix-term/src/compositor.rs`、`helix-term/src/ui/plugin_panel.rs`、`helix-term/src/ui/layout.rs`

- [ ] **步骤 1：写失败集成测试**

- `C-w h`（main 最左叶）→ rail 聚焦（active == rail id；focus border 在 rail）。
- rail 聚焦态：`j`/`k` 让 filetree 选中移动（渲染选中行变化断言或 JS 状态）；`Esc` → active 回 main（离开前叶）；`C-w h` 再进、`l` → 回 main。
- rail 聚焦态按 `C-w v` → main 出现新叶、rail 未被切。
预期：FAIL（现状 C-w h 到 rail 前需要 rail 已是邻居叶——rail 化后 main 最左叶的邻居即 rail；但 active=rail 时 j/k 等是否被 filetree 消费需护栏）。

- [ ] **步骤 2：实现**

- compositor window-mode 方向聚焦：`h` 且活动叶是 main 最左叶且存在左 rail → focus rail；`l` 且活动叶是 rail → 回 main（记忆离开前叶）；rail 在右时对称。
- rail 聚焦态记录 `pre_rail_focus: Option<u64>`（进入 rail 前的 main 活动叶）；`Esc`/`l` 离开时若该叶仍存在则聚焦它，否则 main 首个叶。
- PluginPanel（rail）：浏览态未知键 `Consumed`（不透穿编辑器）；Esc 分支实测 filetree onKey 返回值——filetree 消费则尊重，未消费则离开 rail。实现时读 plugin_panel.rs 现有 `popup_key` 返回语义接线。
- `C-w v/s/n` 在 rail 聚焦态：先把 active 落到 main（pre_rail_focus 或 main 首叶）再执行分裂（沿用任务 2 护栏）。

- [ ] **步骤 3：跑测试 + 回归**（含用户 filetree 真实插件 `reload_users_real_plugins_dir` 类场景）

- [ ] **步骤 4：Commit**
```bash
git commit -m "feat(ui): rail 焦点模型——C-w h/l 进出、Esc/l 回 main、浏览键消费、rail 上分裂落 main"
```

---

## 任务 4：Enter 打开文件 → main 活动窗打开并聚焦

**文件：** filetree 侧确认（`~/.config/helix/plugins/features/filetree/index.js` 是用户侧——若 open_selected 已 editor.open 打开到 main，仅需聚焦语义确认）；必要时 compositor 层：Enter 在 rail 上由 filetree onKey 返回"打开"后聚焦 main（filetree 打开文件用 editor.open(Replace)？需验证打开落在 main 活动叶的 view/doc 且焦点=main）。

- [ ] **步骤 1：写集成测试**

filetree(用户插件或等价脚本)聚焦态：j/k 选文件 → Enter → 断言：新 doc 打开（documents 增/当前 doc==目标）、active 回 main、filetree 保持打开、rail 仍在。

- [ ] **步骤 2：实现/接线**（以实测 filetree 打开语义为准；若 open 落在 leaf0 而非"上次 main 活动叶"，评估：活动 main 叶即打开目标——若 filetree open 走 editor.open(Replace) 天然落当前 view，而"当前"由 active main 叶决定，则天然正确，仅需聚焦回 main 的接线）

- [ ] **步骤 3：跑测试 + Commit**

---

## 任务 5：文档 + 全量回归

- [ ] **步骤 1：文档**（README/plugin-api 窗口段：rail 语义、C-w h/l 进出、键位表加 rail 行）
- [ ] **步骤 2：全量回归**：`cargo test -p helix-view`、`helix-js`、`helix-term --test integration`（全套，PLUGIN_TEST_LOCK 串行）、clippy、fmt。
- [ ] **步骤 3：Commit**
```bash
git commit -m "docs(ui): rail 文档 + 全量回归"
```

---

## 自检备注

- 规格覆盖：§3.1→任务1；§3.2→任务2；§3.3→任务3；§3.4 dump/zoom→任务1/2；§3.5 API→任务2；§5 测试→各任务+任务5；§6 里程碑→任务1-5。
- 类型一致性：`register_rail/is_rail/rail_leaf/unregister_rail` 任务1 定义、任务2/3 引用；`pre_rail_focus` 任务3 定义。
- 占位符：任务 4 的打开语义标注"以实测为准"——是有意的验证点（filetree 用户侧脚本），非 TODO。
