# 设计：树状组件模型（方案二-B）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

插件渲染从"纯文本行"升级为"嵌套组件树 + flex 布局"，同时保持旧行 API 兼容。

## JS API

```js
helix.el("text", "hello", { style: "error", width: 20 })  // 文本节点
helix.el("row", [child, ...], { gap: 1 })                 // 水平排列
helix.el("col", [child, ...], { gap: 0 })                 // 垂直排列
helix.el("scroll", [child, ...], { height: 10 })          // 高度裁剪容器
```

- `helix.el` 返回纯数据节点对象 `{ type, ... }`
- render 返回约定：数组（字符串/样式行）→ 旧行为；**单节点对象（有 type 字段）→ 组件树**
- 组件树可嵌套（col 里 row 里 text）；text 支持 style 与 width（截断）

## 实现

- **helix-js**：`js_el`（构造节点：校验 type 白名单 + 参数）——或 render 解析时直接读节点对象（不强制 el 构造器，允许手写 `{type:"text",...}`，el 只是便捷构造器）；`CompNode` 枚举（Text/Row/Col/Scroll）序列化；`render_popup`/面板 render 返回改 `Content::Lines(Vec<StyledLine>) | Content::Tree(CompNode)`
- **helix-term**：布局引擎 `layout_node(node, viewport) -> Vec<StyledLine>`（新模块 ui/comp_layout.rs 或 plugin_popup.rs 内）：
  - text → 一行（width 截断）
  - col → 子节点纵向堆叠（各取所需高度，viewport 高度内）
  - row → 子节点横向并排（每个子节点的行并排，宽度求和；总宽超 viewport 截断）
  - scroll → 子节点布局后保留最后 height 行（或首行——以实现为准）
  - 样式经 theme.get 解析（同 StyledLine）
- 弹窗/面板渲染：Content::Tree → 布局引擎 → 行 → 既有 Span 渲染

## 测试

- 单测（helix-js）：el 构造校验；render 返回树 → Content::Tree 序列化正确
- 单测（helix-term 布局引擎）：row 并排宽度、col 堆叠、scroll 裁剪、嵌套、text width 截断
- 集成：弹窗 render 返回组件树 → 渲染进 Buffer → 断言布局（如 row 两列文本并排）

## 涉及文件

- `helix-js/src/lib.rs`（js_el/CompNode/Content 枚举/单测）
- `helix-term/src/ui/plugin_popup.rs`（Content 渲染分支）
- `helix-term/src/ui/comp_layout.rs`（新，布局引擎 + 单测）
