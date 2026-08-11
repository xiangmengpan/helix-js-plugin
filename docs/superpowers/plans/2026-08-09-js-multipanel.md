# 多面板并存实现计划（v11-①）

> 自主执行。TDD → 通过 → 提交；控制者自动合并（并行 wave，冲突由控制者解决）。

### 任务 1：N 面板

**文件：** `helix-term/src/compositor.rs`、`helix-term/src/ui/plugin_panel.rs`、`helix-term/src/commands/typed.rs`

- [ ] **步骤 1：失败集成测试**（tests/test/plugin_multipanel.rs，临时 mod）
  - 开 right 面板 A（size 20）+ right 面板 B（size 10）→ 渲染 Buffer → 底部行右侧 30 列被面板占用（A 最右 20 + B 左 10）
  - `helix.close_panel(A)` → 渲染 → 只剩右侧 10 列（B）
- [ ] **步骤 2：运行失败** → 断言失败（当前单面板 replace 语义）。
- [ ] **步骤 3：实现**
  - `PluginPanel`：层 id 化——构造带 id，`PluginPanel::id()` 访问器；`split_area` 保留（单面板用）
  - `compositor.rs` render 特化：枚举所有 PluginPanel 层（遍历 layers 按 type_name 收集 (id, side, size)）→ 排布：right 从右缘向内、left 从左缘向内、bottom 从底缘向上（先开优先靠边）→ 每面板 area + 剩余区（各侧减和）→ 分层渲染（面板按各自 area，其余按剩余区）
  - `typed.rs` ClosePanel：按 id 找对应 PluginPanel 层移除（compositor 加 `remove_panel(id)` 辅助或遍历 layers 匹配 id）
  - `:panel-close` 保持 LAST_PANEL_ID 语义
- [ ] **步骤 4：跑测试**（临时 mod + 排布函数单测——排布逻辑提取为可单测函数）→ PASS
- [ ] **步骤 5：回归**（plugin 组、command_line、cargo check/clippy）
- [ ] **步骤 6：Commit** `feat(term): multiple coexisting panels`

---

## 自检
- 风险：compositor 枚举层的借用（先收集 owned (id,side,size) 再渲染）；排布函数可测性（提取纯函数）；既有单面板测试（plugin_panel/plugin_layout）不回归（单面板 = N=1 特例）。
