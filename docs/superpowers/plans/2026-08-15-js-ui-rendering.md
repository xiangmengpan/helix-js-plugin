# 计划:JS 视图层阶段 A(通用渲染通道)

日期:2026-08-15
来源:docs/superpowers/specs/2026-08-15-js-ui-rendering-design.md(阶段 A)

## 现状确认

- 组件树(CompNode:Text/Row/Col/Scroll/Button/Input + 富文本 style)+ comp_layout 布局引擎已存在
- panel/popup 已走"Rust 组件调用 JS render 回调 → 组件树 → 绘制"(render_popup 复用同一注册表)
- **缺口**:①API 命名 popup 专属(render_popup)②无组件状态只读通道(get_component_state)③非 popup/panel 组件未接入

## 任务(每任务 commit)

1. **A1 通用渲染 API**:`helix_js::render_component`(render_popup 泛化别名,语义通用);单测
2. **A2 组件状态通道**:注册表(组件注册状态提供者)+ `helix_js::get_component_state(id) -> JsValue`;单测
3. **A3 终端接入**:PluginTerminal 注册状态提供者(模式/标题/最小化/滚动偏移),get_component_state 返回;集成测试
4. 收尾:全量回归 + 文档更新(阶段 A 标记完成) — **完成(fbccb0e49)**

## 阶段 A 完成(fbccb0e49)

- A1 `render_component`(14666e21b)
- A2 `get_component_state`(df951e5bb)
- A3 终端状态接入(fbccb0e49)
- 全量集成 245/246(1 失败 = 已知 reload 并行 flake,单跑通过);helix-js 47、helix-term lib 55、clippy 0

## 后续(阶段 B,另立计划)

终端视图 JS 化(标题/最小化条由 JS 画,网格留 Rust)
