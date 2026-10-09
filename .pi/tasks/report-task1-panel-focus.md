# Task 1 报告：open_panel focusable 开关 + PluginPanel 焦点字段落地（未启用零变化）

## 状态：DONE_WITH_CONCERNS

## 实现内容

### 1. 开关传递链（4 文件）
- `helix-js/src/types.rs`：`UiRequest::OpenPanel` 加 `focusable: bool`（注释注明节点焦点路由开关）
- `helix-js/src/popup.rs` `js_open_panel`：解析 focusable——缺省 key（undefined）→ false，否则 `try_js_into::<bool>`，非布尔报错 `"open_panel: 'focusable' must be a boolean"`（brief 中 "undefined 行为需确认" 的兜底分支：代码库既有模式 `is_undefined()` 判缺省，直接采用）；push 带 focusable
- `helix-term/src/commands/typed.rs`：OpenPanel 解构带 focusable → `PluginPanel::new(id, side, focusable)`；Split kind="panel" 路径补 `false`（split 无 focusable 概念）
- `helix-term/src/ui/plugin_panel.rs`：加 `focusable: bool` + `focus: Option<String>` + `focusables: Vec<String>` 字段（仿 plugin_popup.rs:21-23）；`new(id, side, focusable)`；render 改为 focusable 面板才传 `self.focus.as_deref()` + 仿弹窗 refresh 提取 focusables（`focusable_node_ids`），未启用传 None 不提取——零行为变化

### 2. 测试文件适配（`new` 签名变化）
- `helix-term/tests/test/window_mode.rs`（5 处）、`plugin_layout.rs`（1 处）：`PluginPanel::new(1, PanelSide::Right, false)`

## 验证（clean worktree @ bbb090d7e + HEAD 全量）
- `cargo build -p helix-term`：✓
- `cargo test -p helix-js`：80 passed（HEAD 上 82 passed，含并行 read_tree 新增 2 条）
- `cargo test -p helix-term --features integration --test integration plugin_panel`：2 passed（既有面板测试零回归）
- `plugin_popup`：3 passed；`window_mode`：14 passed；`plugin_layout`：3 passed
- clippy：无本任务新增告警（剩余 2 条为既有：helix-core AnnotationSource、completion.rs map_or）
- fmt：本任务 6 文件全部 clean（`rustfmt --check` 逐文件验证；剩余 diff 全在并行 read_tree 代码 shell.rs/types.rs FsTree）

## 提交
- `bbb090d7e` feat(js,term): open_panel focusable 开关 + PluginPanel 焦点字段/focusables 提取(未启用零变化)——内容与本实现一致（并行进程代提交，含其补的 panel_api 测试 `..` 守卫）
- popup.rs fmt 修正因提交竞态折入并行 commit `0387357f0`（消息标 application.rs，内容 = application.rs + popup.rs fmt；HEAD 终态正确且 fmt clean）

## 自检
- 规格覆盖：2.1 focusable 参数 ✓、2.3 数据流（开关→字段→提取）✓；2.2 语义（Tab/Esc/直达）与 2.4 边界属任务 2，未越界
- 未启用零变化：非 focusable 面板 render 传 None、不提取 focusables、handle_event 无焦点分支——与改动前逐行等价
- 无占位符/无 TODO

## 疑虑
1. **并行 agent 同工作区/同分支**：本批次（panel-focus）与 helix.picker（read_tree）并行提交，曾出现提交竞态（我的 amend 折入并行 commit）。当前 HEAD 历史完整：bbb090d7e（本任务）→ 30185c2c1（read_tree）→ 0387357f0（application.rs fix + 我的 popup.rs fmt）。**若并行流程执行 git reset/force，两个批次 commit 都可能受影响**。
2. 遗留 stash@{0}（read_tree WIP 副本 + .pi/tasks json 删除）：内容已提交进 30185c2c1，为冗余，未动。
