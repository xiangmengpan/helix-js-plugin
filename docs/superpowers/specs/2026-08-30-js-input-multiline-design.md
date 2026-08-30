# 设计:input 多行(multiline)

日期:2026-08-30
状态:已批准(brainstorming 两轮问答 + 三节设计确认)

关联:docs/superpowers/specs/2026-08-09-js-panel-input-design.md(input 组件单行现状)

## 1. 动机

P0 已知边界:「input 单行」。输入组件 `InputState { value, cursor }` 无换行概念,Enter 是提交语义。本设计加 `multiline` 开关:多行输入(Enter 换行、行感知光标移动、多行渲染),单行行为不变。IME 不做(终端受限,crossterm 键事件为已组合字符,helix 主编辑器亦无 IME 组合逻辑)。

## 2. 设计

### 2.1 API

```js
{ type: "input", id: "msg", multiline: true, width: 40, ... }
```

- `multiline: true`(默认 false)

### 2.2 语义(multiline = true 时)

- **Enter → 光标处插入换行**(在 input 内消费,不触发提交/onKey)
- **Up/Down → 行间移动(保持列,短行 clamp 到行尾)**;Home/End → 行首/行尾;Left/Right → 跨行边界;Backspace/Delete → 跨行合并
- 提交:插件用外部 button + onPress 或 onKey 自定义键(Enter 被消费)
- 单行 input(默认)完全不变

### 2.3 数据流

- `InputState` 加 `multiline: bool`(popup.rs 节点初始化时从 multiline 参数读)
- `input_edit` 扩展(纯函数,行感知):Enter 插 \n;Up/Down 列保持;Home/End 行级;Left/Right 跨行;Backspace/Delete 跨行合并;行边界计算 helper;单行行为逐字节不变
- 渲染(comp_layout.rs Input 分支):multiline 按 \n 分行渲染多行 StyledLine,光标所在行插 |;行内宽度限制保持
- `set_input_value` 不变(值含 \n)

### 2.4 边界

- 布局高度:多行 input 渲染多行(行数 = value 行数,受 viewport 裁剪);布局高度计算按既有布局语义最小处理(实现时确认)
- 单行 input 逐字节不变;IME 不做;提交语义变更仅限 multiline

## 3. 验证

### 3.1 helix-js 单测(input_edit 纯函数)

1. multiline Enter 光标处插 \n(cursor 推进)
2. Up/Down 列保持(短行 clamp 行尾)
3. Home/End 行级;Left/Right 跨行边界
4. Backspace/Delete 跨行合并
5. 单行 input_edit 回归(现有测试全绿)

### 3.2 integration

1. multiline input 聚焦 → Enter → value 含 \n(白盒)
2. Up/Down 光标移动后输入位置正确(白盒)
3. 单行 input Enter 仍提交(回归)

## 4. 规模

- helix-js:input.rs(input_edit 扩展 + 行 helper)、popup.rs(InputState 初始化带 multiline)、lib.rs(单测)
- helix-term:comp_layout.rs(Input 多行渲染)
- 测试:helix-js 单测 + integration
- 约 2 任务:① input_edit 多行扩展 + 单测(纯函数主体)② 渲染 + 节点初始化 + integration
