# 历史归档(不要当作当前文档)

这里记录的是**当时**的设计规格(`specs/`)、实现计划(`plans/`)与交接记录(`handoff/`),
合计约 150 个文件。

**用途**:回答"为什么这么设计"。
**不是**当前 API / 路径 / 行为的依据 —— 其中的名字与路径**可能已过时**,例如:

- 扁平 API(`helix.focus` / `helix.get_layout` / `helix.buffers` / `helix.layout_fix` …)
  已在后续被命名空间取代(`helix.pane.*` / `helix.buffer.*` / `helix.layout.*`)
- 插件路径 `plugins/features/<name>.js` 已迁为 `plugins/<name>/plugin.js`
  (见 [`../plugin-layout.md`](../plugin-layout.md))
- `helix.restore_layout` 曾标为"未接线",后来已实现

**当前文档的入口**:[`../README.md`](../README.md)(docs 索引)。
