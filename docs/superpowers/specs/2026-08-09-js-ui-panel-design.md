# 设计：侧边面板（v9-③）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

插件开一个**常驻停靠面板**（右侧/左侧/底部条带），显示 JS 渲染内容，不阻塞编辑器输入。终端控制台、构建输出、git 状态视图的宿主。

## 新增 API

```js
const id = helix.open_panel({
  side: "right",         // "right" | "left" | "bottom"
  size: 30,              // 条带宽度（right/left，字符列）或高度（bottom，行）
  render: () => [...],   // 同弹窗 render（字符串或 {text, style}）
  onClose: () => {...},  // 可选：面板关闭时
});
helix.close_panel(id);   // 关闭指定面板
```

- 面板是**显示专用**：不接收键盘（事件穿透到编辑器）——PoC 边界（交互面板留待后续）
- 常驻：不随 Esc 关闭；`helix.close_panel(id)` 或 `:panel-close`（关当前）关闭
- 渲染复用弹窗回调注册表（render 同签名，含样式行）；内容可被异步回调更新（`onChunk` 里改模块状态 → 面板重绘）

## 实现

- **helix-js**：`UiRequest::OpenPanel { id, side: String, size: u16 }`；`js_open_panel`（校验 side 白名单 + size 数字 + render 函数）；`js_close_panel`（UiRequest::ClosePanel { id }）；复用弹窗回调注册表（新 id 空间或共用——以简洁为准，建议共用 NEXT_POPUP_ID 机制）
- **helix-term**：新组件 `PluginPanel`（ui/plugin_panel.rs）：render 用 `render_popup(id, w, h)` 画内容（按 side/size 计算 Rect：right = 右条带，left = 左条带，bottom = 底部条带）；`handle_event` 恒返回 `Ignored`（事件穿透）；dispatch drain `OpenPanel` → push 层；`ClosePanel` → `compositor.remove`/pop
- 布局：面板层在 EditorView 之上（渲染覆盖其条带区域）；事件穿透让编辑器继续可编辑

## 测试

- 单测：js_open_panel 校验（side 白名单/size/render）+ 入队 OpenPanel；js_close_panel 入队
- 集成：`:panel-demo` 开面板 → 层存在（has_component）→ 编辑器仍可输入（按键后文本变化，证明事件穿透）→ `helix.close_panel(id)` 命令 → 层消失

## 非目标

- 面板交互输入（显示专用）、真正的布局收缩（编辑器区域不缩小，面板覆盖其上——PoC 接受）
- 多面板并存、面板拖拽/尺寸调整

## 涉及文件

- `helix-js/src/lib.rs`（OpenPanel/ClosePanel、js_open_panel/js_close_panel）
- `helix-term/src/ui/plugin_panel.rs`（新）
- `helix-term/src/ui/mod.rs`（导出）
- `helix-term/src/commands/typed.rs`（dispatch drain + :panel-close 命令）
