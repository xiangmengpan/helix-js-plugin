# 设计：样式 API（v9-②）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

弹窗渲染支持主题样式——`render` 可返回带样式的行，解锁彩色输出。

## 新增 API

```js
helix.open_popup({
  render: () => [
    { text: "Error: ", style: "error" },   // 样式名 = helix 主题 scope
    { text: "file not found" },
    "plain line",                          // 字符串 = 无样式
  ],
  ...
});
```

- `render` 返回数组，元素为字符串或 `{ text: string, style?: string }`；style 为 helix 主题 scope 名（"error"/"warning"/"info"/"ui.popup"/"ui.virtual.indent-guide" 等任意 theme key）
- 未知 style 名 → 默认样式（不报错）
- 每行可有自己的样式；行内多段样式暂不支持（一行一个样式）

## 实现

- **helix-js**：`render_popup` 返回类型改 `Vec<StyledLine>`，`pub struct StyledLine { pub text: String, pub style: Option<String> }`——JS 数组元素解析：字符串 → (text, None)；对象 → 读 text（必填字符串）+ style（可选字符串）；非法 → Err
- **helix-term**：`PluginPopup::render` 把 StyledLine 转 tui Span（`theme.get(style_name)` 解析 Style；`theme.get` 对未知 key 返回默认——以 helix Theme API 为准）；`required_size` 按文本宽度（样式不影响）
- 状态栏钩子保持字符串（YAGNI——样式先覆盖弹窗）

## 测试

- 单测：render 混合数组 → StyledLine 解析（字符串/带样式对象/缺 text 报错）
- 集成：弹窗渲染带 style 的行 → 渲染进 Buffer → 断言该行 cell 的 Style 与 theme 的 error scope 一致（仿 bufferline 渲染测试）

## 涉及文件

- `helix-js/src/lib.rs`（StyledLine、render_popup 解析）
- `helix-term/src/ui/plugin_popup.rs`（渲染 Span 化）
