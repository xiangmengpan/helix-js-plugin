# 设计:input 组件升级 + 补全联动(B4)

日期:2026-08-26
状态:已批准(分节讨论确认)

## 背景与目标

input 组件目前是"静态展示 + onKey 裸按键"的半成品:可聚焦、字符/Backspace/Delete 路由到 onKey,但 value 由 JS 手动拼串维护,无编辑状态、无 onChange 回调、方向键/Enter 不路由。本设计把 input 升级为**引擎权威的真输入框**(引擎维护 value + 光标,onChange 自动回调,set_input_value 显式改值),使插件能用几行代码实现补全联动(配合已交付的 `helix.lsp.completion()`)。

**范围(用户选定 A+B)**:引擎侧仅弹窗(plugin_popup.rs)按键路由 + input 编辑状态机;不做面板节点路由、不做候选组件。

## input 编辑模型(引擎权威 + 完整光标)

```
渲染:  JS render 返回 input 节点 → 显示值 = 引擎 InputStates[(popup_id, node_id)] 的 value
       (JS 传的 value 仅当引擎无该节点状态时作初始化)
按键(焦点在 input):
  单字符   → 插入光标处, onChange(newValue)
  Backspace→ 删光标前字符, onChange(newValue)
  Delete   → 删光标后字符, onChange(newValue)
  Left/Right → 移动光标(编辑光标,不触发 onChange)
  Home/End → 光标到首/尾
  Up/Down  → onKey("Up"/"Down")   ← 候选导航
  Enter    → onKey("Enter")       ← 选择候选/提交
```

- **InputStates** 存 `(popup_id, node_id) → { value, cursor }`(node_id 即 input 的 id,弹窗树内唯一)
- **onChange(value)** 是 input 节点新属性(与 onKey 并列):值变化时回调,插件在此更新状态/调 completion
- **新 API `helix.set_input_value(node_id, value)`**:JS 强制改值(候选回填/清空/外部注入)——渲染时引擎值优先,必须显式 API
- Up/Down/Enter 走 onKey(命令语义),字符/删除走 onChange(编辑语义),互不冲突

## 分层与数据流

```
helix-js(引擎状态)                    helix-term(按键路由)
─────────────────                    ─────────────────
InputStates[(popup_id, node_id)]       plugin_popup.rs handle_event:
  = { value, cursor }                    焦点在 input 时:
                                         字符/Backspace/Delete/Left/Right/Home/End
parse_node: input 节点渲染值             → input_edit(id, node, key)
  从 InputStates 取(JS 传值仅初始化)      → 新值? → onChange(newValue)
                                         Up/Down/Enter → dispatch_node_event(onKey)
input_edit(state, key) 纯函数
  → Option<String>(新值,无变化 None)    弹窗关闭 → 清理 (popup_id, *) 状态
onChange 回调:NodeHandlers 加 on_change
set_input_value(node_id, value) 新 API
```

- **input_edit 纯函数**(value+cursor+key → 新值/新光标)放 helix-js 可单测
- **onChange 存 NodeHandlers**(与 on_key 并列),专用 `dispatch_input_key` 入口:编辑成功才回调
- **渲染**:parse_node 构建 CompNode::Input 时用引擎状态值(JS 传的 value 仅在状态不存在时初始化);光标渲染为 `|` 插入符(渲染层小改)
- **生命周期**:close_popup 时清理该 popup 的全部 InputStates(防重开同 id 弹窗带旧值)
- **helix-term 改动面**:仅 plugin_popup.rs 按键路由分支;面板不碰(P0 弹窗范围)

## 测试策略

- **helix-js 单测**(input_edit 纯函数 + 状态):
  - 字符插入(中间/末尾)、Backspace(中间/空值/光标在首)、Delete、Left/Right 光标越界 clamp、Home/End
  - 非编辑键(Up/Down/Enter)→ None(不触发 onChange)
  - 渲染值来源:JS 传 value 初始化 → 引擎状态优先
  - set_input_value:更新状态 + 后续渲染反映
  - onChange 触发条件:编辑键且值变化才回调
- **集成测试**(plugin_popup 路径):
  - 焦点 input → 按字符 → onChange 收到新值(JS echo → 断言)
  - Up/Down/Enter 路由到 onKey(断言收到 "Up"/"Down"/"Enter")
  - set_input_value 回填 → 渲染显示新值
- **demo 插件**:`plugins/features/input-completion/index.js`——弹窗:input + 候选区;输入时(防抖)调 `helix.lsp.completion()`,候选渲染为行(选中高亮),↑↓ 导航、Enter 插入文档, Esc 关闭
- **真 LSP 验证**:同上一计划(临时集成测试,不进自动化)

## 文档

`docs/plugin-api.md` input 组件章节更新:onChange 属性、set_input_value API、按键语义表(编辑键/导航键/Enter)。

## 范围外(明确不做)

- 面板侧节点路由(弹窗够用)
- 多行输入 / IME 合成输入(单行优先)
- 候选虚拟化(插件自管)
- 防抖/节流(插件侧 setTimeout)
