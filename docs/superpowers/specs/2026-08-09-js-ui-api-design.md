# 设计：helix JS 交互式 UI API（v2）

日期：2026-08-09
状态：已批准（设计内容已与用户确认）

## 目标

在既有 JS 插件 PoC 上新增 UI 能力：自定义组件渲染（弹窗）、键盘输入、bufferline 图标钩子。使插件能把内容画到屏幕上并与用户交互。

## 新增 JS API

### 1. `helix.open_popup({ render, onKey, onClose })`

交互式弹窗——JS 渲染内容 + JS 处理按键。

- `render(): string[]` — 每次重绘调用，返回行数组（纯文本，无样式）
- `onKey(key): "close" | "handled" | "ignore"` — 按键回调
  - `key: { name: string, shift: bool, ctrl: bool, alt: bool }`
  - name 取值：`"a".."z"`/`"A".."Z"`/`"0".."9"`（Char）、`"Enter"`、`"Esc"`、`"Backspace"`、`"Tab"`、`"Up"`、`"Down"`、`"Left"`、`"Right"`、`"Home"`、`"End"`、`"PageUp"`、`"PageDown"`、`"Delete"`、`"Insert"`、`"F1".."F12"`
  - `"close"` → 关闭弹窗（触发 onClose）；`"handled"` → 消费事件；`"ignore"` → 事件穿透
- `onClose(): void` — 弹窗关闭时调用（清理用）
- 未提供 onKey 时：Esc 默认关闭，其余穿透
- 多个 open_popup：新弹窗替换旧弹窗（replace 语义）

### 2. `helix.set_buffer_icon(fn)`

bufferline 标签栏图标钩子。

- `fn(path: string | null): string | null` — 每个缓冲区渲染时调用，返回图标字符串或 null（null = 用默认行为）
- 调用时机：bufferline 渲染（每帧），每个可见缓冲区一次
- 单例注册：多次调用覆盖

## 实现

### helix-js（runtime 扩展）

- 新增 `UiRequest` 队列：`pub enum UiRequest { OpenPopup { id: u64 } }`，`take_ui_requests() -> Vec<UiRequest>`（与 MESSAGES 并列的全局 `Mutex<Vec>`）
- 弹窗回调注册表（thread_local，与引擎同线程）：`PopupId(u64) → PopupCallbacks { render: JsValue, on_key: JsValue, on_close: JsValue }`，id 自增计数器
- bufferline hook（thread_local 单例）：`Option<JsValue>`
- 公共函数：
  - `render_popup(id, width: u16, height: u16) -> Result<Vec<String>>` — 调 render，ctx 对象 `{ width, height }`，结果必须是 string[]
  - `popup_key(id, key: &PluginKey) -> Result<PopupKeyResult>` — 调 on_key，解析返回值（缺省：Esc→Close，其他→Ignore）
  - `close_popup(id) -> Result<()>` — 调 on_close + 移除注册表项
  - `bufferline_icon(path: Option<&str>) -> Option<String>` — 调 hook，null/报错→None
  - `PluginKey { name: String, shift: bool, ctrl: bool, alt: bool }`、`enum PopupKeyResult { Close, Handled, Ignored }`（公开类型）
- 原生函数 `js_open_popup`：校验 render 是函数、onKey/onClose 可缺省（默认值），分配 id，入队 UiRequest

### helix-term（UI 接线）

- 新组件 `PluginPopup`（`helix-term/src/ui/plugin_popup.rs`）：
  - 字段：`id: u64`、`lines: Vec<String>`（缓存上次渲染）
  - `required_size`：行数 × 最长行宽（clamp 到 Popup 的 MAX 常量）
  - `render`：调 `helix_js::render_popup(id, w, h)`，把行画到 surface（复用 Text 渲染风格，用 `ui.bufferline` 或普通样式）
  - `handle_event`：`Event::Key` → 构造 PluginKey → `popup_key` → Close：`EventResult::Consumed(Some(Callback::EditorCompositor(Box::new(move |_, comp| { helix_js::close_popup(id); comp.pop(); }))))`；Handled → `Consumed(None)`；Ignored → `Ignored(None)`。JS 错误 → `Ignored(None)` + 记录错误
- 分发钩子（typed.rs，既有 `run_command` 成功分支）：drain `take_ui_requests()`，对每个 OpenPopup 构造 `Popup::new("plugin-popup", PluginPopup::new(id)).auto_close(true)` 推层
- bufferline（editor.rs `render_bufferline` ~L685）：fname 计算后插入——`helix_js::bufferline_icon(doc.path()... )` 返回 Some(icon) 时标签变为 ` {icon} {fname}`

### 依赖

无新依赖。

## 测试

- **helix-js 单测**（`cargo test -p helix-js`）：
  - open_popup 入队 + id 递增
  - render_popup 往返（render 返回行 → 断言行内容；render 返回非数组 → Err）
  - popup_key 三分支（close/handled/ignore）+ 缺省行为（Esc→Close，其他→Ignore）
  - close_popup 触发 onClose 且再调 render_popup 报错（已移除）
  - bufferline_icon：注册→调用→返回 Some；返回 null → None；未注册 → None
- **集成测试**（`cargo test -p helix-term --features integration --test integration plugin`）：
  - 插件命令 open_popup → `compositor.find::<ui::PluginPopup>()` 为 Some
  - 发送 Esc → 层消失（`find` 为 None）

## 非目标（YAGNI）

- 行内样式/风格名、鼠标事件、弹窗内滚动、多弹窗并存
- render 期间发起 open_popup（只在命令边界 drain）
- bufferline 标签完全自定义（只做图标前缀）
- 侧边面板/底部面板等布局槽

## 涉及文件

- `helix-js/src/lib.rs`（runtime 扩展 + 单测）
- `helix-term/src/ui/plugin_popup.rs`（新建）
- `helix-term/src/ui/mod.rs`（导出 PluginPopup）
- `helix-term/src/commands/typed.rs`（drain UiRequest → 推层）
- `helix-term/src/ui/editor.rs`（bufferline 图标钩子）
- `helix-term/tests/test/plugin.rs`（集成测试）
