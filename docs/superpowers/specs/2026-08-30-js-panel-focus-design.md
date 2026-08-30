# 设计:面板节点焦点路由(open_panel focusable)

日期:2026-08-30
状态:已批准(brainstorming 两轮问答 + 三节设计确认)

关联:docs/superpowers/specs/2026-08-09-js-panel-input-design.md(弹窗节点焦点路由方案乙——本设计推广到面板)

## 1. 动机

P0 已知边界:「面板无节点焦点路由(仅弹窗)」。弹窗已有完整机制(Tab 移动焦点、焦点节点按键直达、button/input 节点事件),面板是整面板 onKey 回调、无节点级焦点。本设计把弹窗机制推广到面板,以**显式开关**保证向后兼容。

## 2. 设计

### 2.1 API

```js
// open_panel 加 focusable 参数(默认 false)
helix.open_panel({ side: "right", size: 30, render: () => [...], focusable: true });
```

### 2.2 语义(focusable = true 时启用焦点路由,与弹窗方案乙同机制)

- Tab 在可聚焦节点间循环移动焦点(初始聚焦第一个)
- 焦点在节点时按键直达:Enter → button onPress / input onKey("Enter");字符 → input 插入;方向键 → input 光标移动/候选导航
- **Esc 取消焦点**(回无焦点态,按键恢复走面板 onKey)——面板非模态,不关闭
- 无焦点时按键走 onKey(现状)
- 焦点失效(render 后节点列表变化)→ 重置焦点到无
- 未启用(默认):完全现状,现有面板插件零影响

### 2.3 数据流

- **JS 侧**:`js_open_panel` 解析 `focusable`(bool,默认 false)→ `UiRequest::OpenPanel` 加字段 → term 侧 `PluginPanel::new(id, side, focusable)` 存开关
- **term 侧 plugin_panel.rs**(仿 plugin_popup.rs):
  - 加 `focusables: Vec<String>` + `focus: Option<String>` 字段
  - render 时提取:`focusable_node_ids(node, &mut self.focusables)`(复用现有 JS 函数);render_popup 传 `self.focus.as_deref()`
  - handle_event 加焦点路由分支(仅 focusable):Tab 移动 / Esc 取消 / 焦点节点按键直达(dispatch_node_event / dispatch_input_key)
  - ctx 构建/drain 与弹窗重复(既有 ponytail 注释)——焦点分支实现时顺带评估抽公共辅助(第三次复用才值得)

### 2.4 边界

- Esc 取消焦点不关闭面板(弹窗 Esc 关闭是弹窗语义)
- 焦点路由仅 focusable 面板;未启用零变化(现有测试不回归)
- 弹窗不受影响(不引入开关,保持现状)
- Tab 在无 focusables 面板仍走 onKey

## 3. 验证

### 3.1 integration(新 plugin_panel_focus.rs 或并入 plugin_panel.rs)

1. focusable 面板:Tab 聚焦第一个节点 → render 收到 focus(白盒断言)
2. 焦点在 button → Enter → onPress 触发(消息断言)
3. 焦点在 input → 字符插入 → onChange 更新值(白盒断言 value)
4. Esc → 取消焦点 → 按键回 onKey(断言 onKey 收到)
5. 未启用 focusable 面板:Tab 走 onKey(不回归)
6. 焦点失效重置:render 后节点消失 → 焦点清空 → Tab 从头

### 3.2 回归

- 既有 plugin_panel/plugin_popup 测试全绿(未启用面板零行为变化)

## 4. 规模

- helix-js:popup.rs(js_open_panel 解析 focusable)、types.rs(UiRequest::OpenPanel 加字段)
- helix-term:ui/plugin_panel.rs(字段/提取/焦点分支)、commands/typed.rs(OpenPanel 构造传递)
- 测试:integration(新文件或并入)
- 约 2 任务:① 开关与 term 侧焦点字段/提取(含未启用零变化)② 焦点路由分支 + 测试
