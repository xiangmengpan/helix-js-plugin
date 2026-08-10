# 设计：面板交互输入（v10-①，并行 wave 1）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

面板可接收键盘输入：`open_panel` 支持 onKey（滚动/翻页/自定义命令），按键"handled"则面板消费、"ignore"则穿透编辑器。兼有壁 ③ 的关闭语义。

## API

```js
helix.open_panel({
  side: "right", size: 30,
  render: () => [...],          // 同弹窗（含 {text,style}）
  onKey: (key) => "handled" | "ignore" | "close",   // 可选：面板按键回调
  onClose: () => {...},         // 可选（既有）
});
```

- 无 onKey：面板完全穿透（既有行为）
- onKey 返回 "handled" → 消费该键；"ignore" → 穿透编辑器；"close" → 关闭面板（触发 onClose）
- Esc 默认：无 onKey 时穿透；有 onKey 时交由回调决定（不自动关——面板常驻）

## 实现

- **helix-js**：`js_open_panel` 增加可选 onKey 解析（函数校验）→ 注册 PopupCallbacks 时填 on_key 字段（面板 id 与弹窗共用注册表，`popup_key(id, key, ctx)` 已支持）
- **helix-term**：`PluginPanel::handle_event` 从恒 `Ignored` 改为——有 on_key 时调 `popup_key`（构建 CommandContext，同 PluginPopup）：Close → 移除层 + close_popup；Handled → Consumed；Ignored → Ignored。无 on_key 时保持 Ignored
- ClosePanel 的 id 处理沿用 v9-③ 静态 id 机制（面板单例）

## 测试

- 单测：js_open_panel 带 onKey 注册成功（回调可被 popup_key 调用）；无 onKey 时 popup_key 缺省（Esc 不自动关——缺省 on_key 的 popup_key 语义是 Esc→Close！需注意：面板缺省应为全 Ignore——实现时面板缺省不走 popup_key，直接 Ignored）
- 集成：面板带 onKey（Down → echo）→ 按 Down → 状态栏出现；按普通键 → 穿透编辑器（文本可编辑）

## 涉及文件

- `helix-js/src/lib.rs`（js_open_panel onKey）
- `helix-term/src/ui/plugin_panel.rs`（handle_event）
