# 设计：弹窗尺寸/位置控制（v9-①）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

`open_popup` 支持 width/height/position 选项。

## 新增 API

```js
helix.open_popup({
  render: () => [...],
  onKey: (key, doc) => {...},
  width: 40,              // 可选：弹窗宽度（字符列），clamp 到视口
  height: 10,             // 可选：弹窗高度（行），clamp 到视口
  position: { row: 5, col: 10 },  // 可选：锚点（屏幕坐标），弹窗出现在锚点附近
});
```

- 不传 = 现状（自动按内容尺寸 + 默认定位）
- width/height 是上限约束（内容更小则按内容）；height 受既有 MAX_HEIGHT=26 约束

## 实现

- **helix-js**：`UiRequest::OpenPopup { id, width: Option<u16>, height: Option<u16>, position: Option<(u16, u16)> }`；`js_open_popup` 解析可选字段（数字校验）
- **helix-term**：dispatch 构造 Popup 时应用——`position` → `Popup::position(Some(Position))`；宽高 → `PluginPopup::new(id, SizeHint { width, height })`，`required_size` clamp
- 既有 OpenPopup 模式匹配点（dispatch/apply_ui_requests/测试）适配新字段

## 测试

- 单测：js_open_popup 解析 width/height/position（数字/缺省/非法）
- 集成：`open_popup({width: 30, height: 5})` → 渲染 popup 内容断言行数 ≤ 5（或 required_size 行为断言）

## 涉及文件

- `helix-js/src/lib.rs`
- `helix-term/src/ui/plugin_popup.rs`（SizeHint）
- `helix-term/src/commands/typed.rs`（dispatch 构造）
