# 设计:装饰按行索引 + 粘贴支持(性能/能力优化)

日期:2026-08-31
状态:已批准(brainstorming 两轮问答 + 三节设计确认)

关联:docs/superpowers/specs/2026-08-28-js-decorations-design.md(批次 3,装饰 ponytail 性能边界)、docs/superpowers/specs/2026-08-30-js-input-multiline-design.md(批次 8,multiline input)

## 1. 动机

两个已标注的性能/能力边界:
1. **装饰大集合每帧全量遍历**(批次 3 ponytail 注释):渲染遍历全部装饰,万级时拖慢
2. **输入组件粘贴**:`Event::Paste`(bracketed paste)被插件组件忽略——粘贴进不了插件 input(或逐字符慢)

## 2. 设计

### 2.1 装饰按行索引(二分裁剪)

- `apply_plugin_decorations` 应用时**排序存储**(virtual_text/highlights 各按 char 索引排序;现在渲染时才排)
- `view.text_annotations` 加可见 char 范围参数(anchor char + 可见行数 → char 范围,调用方 editor.rs 计算传入);editor.rs 插件高亮段同理(已有 view_offset.anchor + inner.height)
- 每类装饰**二分取可见范围内子集**,再按 style 分组/合并(既有逻辑不变)——每帧 O(log n + k),k = 可见行内装饰数
- 边界:装饰是快照坐标(不随事务重映射,批次 3 语义),二分按 char 索引排序,可见范围按当前文本算——不新增漂移语义

### 2.2 粘贴支持(批量插入)

- `plugin_popup/plugin_panel` 的 `handle_event` 加 `Event::Paste(contents)` 分支:
  - 焦点在 input → 光标处**一次插入整段 + 一次 onChange**;消费
  - 否则 → Ignored(冒泡给编辑器正文粘贴,现状)
- `input.rs` 加批量插入(光标处插整段,单次状态变更);dispatch 复用现有 onChange/drain
- 单行 input 收到含 \n 的粘贴 → 与逐字符粘贴行为一致(执行时按现有 input_edit 对 \n 的处理确定,保持一致)
- 顺带修复:"粘贴进不了插件 input"现状(bracketed paste 下 Paste 事件被忽略)

## 3. 验证

### 3.1 装饰

- helix-term 单测:`apply_plugin_decorations` 后存储有序;二分辅助函数(可见范围取子集)正确
- integration:装饰功能不回归(既有 plugin_decorations 测试全绿)

### 3.2 粘贴(integration,手动 pump + 构造 Event::Paste 发送)

1. focusable 面板 multiline input 聚焦 → Paste → 值含整段 + 一次 onChange(白盒)
2. 未聚焦 input → Paste 冒泡 → 编辑器正文粘贴(doc 变)
3. 单行 input 粘贴含 \n 行为(与逐字符一致)

## 4. 规模

- helix-term:application.rs(apply_plugin_decorations 排序)、view.rs(text_annotations 可见范围 + 二分)、editor.rs(可见范围计算传入)、ui/plugin_popup.rs + plugin_panel.rs(Paste 分支)
- helix-js:input.rs(批量插入)
- 测试:helix-term 单测 + integration
- 约 2 任务:① 装饰二分(排序存储 + 可见范围 + 单测)② 粘贴支持(批量插入 + Paste 分支 + integration)
