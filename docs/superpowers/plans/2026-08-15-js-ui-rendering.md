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

## 阶段 B 完成(0bf76212a)

- B1 `set_component_render`(f8ba937ca + 断言修正 8acb0dec5)
- B2 终端标题条 JS 化(0bf76212a):顶部 1 行由 JS 画,网格下移;minimized 条 JS 化;无回调 Rust 默认兜底;Drop 注销
- 集成测试 terminal_title_bar_from_js_view:标题条渲染断言
- 全量集成 247/247、helix-js 48、helix-term lib 55、clippy 0

## 阶段 C 完成(ce233189c)

- filetree 迁移:确认已完整组件树化(el text/scroll/col + 图标/样式),零工作
- C2 布局标签条(ce233189c):TABBAR_ID 顶部槽位 + 示例插件 features/tabbar.js(读 get_layout/get_component_state 画叶子标签,活动高亮);集成测试 tabbar_renders_top_row_from_js_view
- 全量集成 248/248、helix-js 48、helix-term lib 55、clippy 0

## 后续(交互与鼠标)

点击标题/最小化条/标签切换依赖鼠标命中(P4),单独规划
