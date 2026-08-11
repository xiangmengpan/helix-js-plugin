# 设计：多面板并存（v11-①）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

多个面板同时打开（不同侧/同侧叠加），按 id 独立关闭。

## 变更

- 面板层 id 从静态 `"plugin-panel"` 改为每面板唯一 `"plugin-panel-{id}"`；`ClosePanel{id}` 移除对应层
- compositor 布局：支持 N 个面板——同侧面板从边缘向内叠加（right：先开的最靠右；left：先开的最靠左；bottom：先开的最靠底）；编辑器区域 = 全屏 - 各面板条带之和
- `:panel-close` 语义保持（关 LAST_PANEL_ID）
- `open_panel` 重复调用不再替换——各自成层；`helix.close_panel(id)` 精确关

## 实现

- **compositor.rs**：render 特化从"找一个面板"改为"枚举所有 PluginPanel 层"（按 type_name 收集 side/size 列表）→ 按侧分组排布计算各面板 area + 剩余区 → 分层渲染
- **plugin_panel.rs**：层 id 随构造传（`PluginPanel::new(id, side, size)`）；`split_area` 改为给定"该面板的预留条带"（由 compositor 排布后传入，或保留 split_area 单面板逻辑 + compositor 负责多面板排布）
- **typed.rs**：ClosePanel → 按 id 移除层（compositor 层列表按 id 找——需要按层内 id 匹配，PluginPanel 提供 `id()` 访问器；compositor 需要按 id 移除的辅助或遍历）

## 测试

- 集成：开 right 面板 A（size 20）+ right 面板 B（size 10）→ 渲染断言底部行右侧 30 列被面板占用（A 最右 20 + B 左 10）；close_panel(A) → 只剩 B（右侧 10 列）
- 单测：compositor 排布函数（N 面板分组叠加）

## 涉及文件

- `helix-term/src/compositor.rs`（多面板枚举+排布）
- `helix-term/src/ui/plugin_panel.rs`（id 化）
- `helix-term/src/commands/typed.rs`（ClosePanel by id）
