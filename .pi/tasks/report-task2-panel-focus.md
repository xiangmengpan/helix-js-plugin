# Task 2 报告：面板节点焦点路由分支 + 集成测试

## 状态：DONE_WITH_CONCERNS

## 实现内容（4 文件，commit b31685f57，分支 `panel-focus-task2`）

### 1. handle_event 焦点路由分支（plugin_panel.rs，仿 plugin_popup.rs:74-130）
- 结构：`key_to_plugin_key` 提前到函数头 → focusable && focusables 非空时走焦点分支 → 否则走既有 onKey 路径（未启用零变化，非 focusable 面板行为逐键等价）
- Tab：焦点在列表循环移动（初始第一个）；按键被消费，不经过 onKey
- 焦点在节点时按键直达（复用弹窗语义）：Enter → input 有状态走 `onKey("Enter")` 否则 button `onPress`；Left/Right/Home/End → input 光标移动；Up/Down → input 候选导航（onKey 形态）；单字符/Backspace/Delete → input 插入（dispatch_input_key）否则 button 分发
- **Esc → 取消焦点回 onKey（Consumed，不关闭面板）**——与弹窗差异点（弹窗 Esc 走 onKey 关闭），注释写明
- drain 复用弹窗形态：消息 + UI 请求即时应用（编辑/光标与弹窗焦点分支一致不应用——弹窗为权威参照）
- 焦点分支独立于 onKey：focusable 面板无 onKey 也能用焦点路由（分支在 onKey 检查之前）

### 2. 焦点失效重置（render 内，规格 2.2）
- render 提取 focusables 后检查：焦点节点不在新列表 → 清空（render 先传旧 focus 供 JS 样式，提取后重置）

### 3. 集成测试（新 plugin_panel_focus.rs，6 条）+ integration.rs mod + 文档
- `focusable_panel_tab_focuses_first_node`：白盒 lastFocus 回写（render 的 focus 参数）断言 Tab 聚焦 b1 且 onKey 不调用
- `focusable_panel_button_enter_triggers_onpress`：Tab+Enter → onPress echo "PRESSED"
- `focusable_panel_input_insert`：Tab×2 到 input → 'a' → onChange "chg:a"
- `focusable_panel_esc_cancels_focus`：Esc 后 render 收到 focus=null、onKey 计数不变（count 证明）、后续键回 onKey（count 递增）
- `non_focusable_panel_tab_still_onkey`：未启用 focusable → Tab 走 onKey（Tab 计数断言，零变化回归）
- `focusable_panel_focus_reset_on_node_change`：焦点在 b2 → 隐藏 b2 → Enter 走 onKey（证明焦点清空非 stale 分发）→ Tab 从头聚焦 b1
- `docs/plugin-api.md`：open_panel 加 focusable 参数 + 语义说明（Tab/Esc/节点直达/失效重置/默认 false）

## TDD 证据（worktree 隔离环境）
- **RED**：`git stash` 掉 plugin_panel.rs 实现后跑 6 条 → 全部 FAILED（unwrap-None，无焦点路由）
- **GREEN**：恢复实现 → `cargo test -p helix-term --features integration --test integration plugin_panel_focus` → **6 passed**（连跑 3 次稳定）

## 调试中发现的既有行为（测试设计依据，非本任务改动）
1. **面板是布局树叶子**：open_panel 后 `split_leaf_with_ratio` 把新叶子设为 active，面板按键路由需面板为活动叶子
2. **EditorView::handle_event 每次按键清 status_msg**（editor.rs:1527）：onKey 返回 "ignore" 的按键穿透给编辑器后，面板 drain 设置的瞬态 status 会被清掉——故测试改为 typed 命令报告 JS 状态（count/lastFocus）断言，不断言瞬态 echo
3. NODE_HANDLERS 为 thread_local（state.rs:51）：tokio 多线程下跨线程 render/dispatch 会丢 handler——既有弹窗测试同机制且稳定（3/3 复跑通过），本批次测试同 PANEL_TEST_LOCK 串行 + 复跑 3 次稳定，未触发

## 验证
- `plugin_panel`（含本批次 6 条）：8 passed；`plugin_popup`：3 passed；`plugin_input`：1 passed；`plugin_components`：5 passed；`window_mode`：14 passed；`plugin_layout`：3 passed；`plugin_multipanel`：1 passed
- `cargo test -p helix-js`：89 passed
- clippy：无本任务新增告警（剩余 2 条为既有：helix-core AnnotationSource、completion.rs map_or）
- fmt：本任务 3 个 Rust 文件 clean（剩余 diff 全在并行 read_tree 代码 shell.rs/types.rs）

## 提交与分支
- **commit `b31685f57` 在分支 `panel-focus-task2`（worktree `.worktrees/panel-focus`），master 未动**
- 背景：共享主工作区被并行 picker 批次反复 clobber（任务 1 报告已预警）——两次清掉我的未提交改动（plugin_panel.rs 焦点分支、docs、integration.rs mod），故改用隔离 worktree 完成
- master 仍在 d017aab92（我的 base）→ 合并为 **fast-forward**：`git merge panel-focus-task2` 即可
- 唯一合并注意点：主工作区 picker 批次的未提交 `integration.rs` 修改会阻止 checkout——需在其提交后合并并手动保留 `mod plugin_panel_focus;`

## 疑虑
1. 并行 picker 批次已 3 次清掉共享工作区未提交改动（git restore 行为），controller 侧需确认合并时机
2. 全量 integration 套件未跑（耗时 5-10 分钟，与并行 agent 共享 target 有锁竞争）；相关面板/弹窗/组件套件已全绿，建议 controller 合并后跑全量
3. 测试用 status 断言依赖"typed 命令报告"模式（因 editor 清 status 的既有行为），与既有 plugin_input 测试的瞬态断言风格不同——见上"调试发现"

## 报告文件
`/home/muyang/code/rs/helix/.pi/tasks/report-task2-panel-focus.md`
