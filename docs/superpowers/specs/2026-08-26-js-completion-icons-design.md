# 设计:completion 弹窗候选图标(纯 JS)

日期:2026-08-26
状态:草案(待审核)

## 1. 动机

`:ic` 补全弹窗候选行目前只有文本 label,无类型图标。LSP `completionItem.kind`(数字 1-25)已随 `helix.lsp.completion()` 返回,但未呈现。目标:候选行按 kind 显示 nerd font 图标,与 filetree/statusline 的既有图标机制保持一致。

## 2. 决策记录

- **纯 JS 层,零 Rust 改动**:现有生态已确立"图标 = JS 层拼字符 + icons.js 共享表"惯例(filetree/statusline/which-key/tabbar 4 消费者)。completion 照此办理,不引入引擎 kind 语义(B1 方案否决:引擎认识 kind 与现有"图标=纯文本"模型冲突,表与 icons.js 分家)。
- **映射表进 icons.js**:不另起炉灶;icons.js 是唯一图标源,completion 成为第 5 个消费者。
- **耦合度接受现状**:路径字符串硬编码、各插件自写回退逻辑是已知坏点,但规模小(5 消费者、单层表),不值得为此引入接口/DI 层。回退重构待真正痛时再做。

## 3. 改动

### 3.1 `plugins/lib/icons.js`

新增 `ICONS.completion` 段:LSP CompletionItemKind(数字 1-25)→ nerd font 字符。

```js
completion: {
  1:  "\uf031",   // Text
  2:  "...",      // Method
  3:  "...",      // Function
  4:  "...",      // Constructor
  5:  "...",      // Field
  6:  "...",      // Variable
  7:  "...",      // Class
  8:  "...",      // Interface
  9:  "...",      // Module
  10: "...",      // Property
  11: "...",      // Unit
  12: "...",      // Value
  13: "...",      // Enum
  14: "...",      // Keyword
  15: "...",      // Snippet
  16: "...",      // Color
  17: "...",      // File
  18: "...",      // Reference
  19: "...",      // Folder
  20: "...",      // EnumMember
  21: "...",      // Constant
  22: "...",      // Struct
  23: "...",      // Event
  24: "...",      // Operator
  25: "...",      // TypeParameter
},
```

- 码位实现时按 nerd font 语义核对(coc.nvim/telescope 同款映射风格)。
- 新增查表函数并 export:

```js
/// LSP kind 数字 → 图标;未知/缺省 → ""(无图标)
function getCompletionIcon(kind) {
  return ICONS.completion[kind] ?? "";
}
```

### 3.2 `plugins/features/input-completion/index.js`

- `helix.plugin("input-completion", { deps: ["lib/icons.js"] })`(新增依赖,与 filetree 同款)。
- 候选行渲染拼图标:

```js
text: (i === sel ? "> " : "  ") + (ICONS ? ICONS.getCompletionIcon(it.kind) : "") + (ICONS ? " " : "") + label(it),
```

- `it.kind` 可能缺失(非 LSP 来源)→ `getCompletionIcon` 回退空字符串,无图标。
- ICONS 获取方式与 filetree 一致:`ICONS = helix.load("lib/icons.js") || null;`,null 时回退无图标。

## 4. 边界

- **宽度计算**:`StyledLine::width()` / `comp_layout.rs:246` 用 `chars().count()`,图标(终端 2 列)算 1 列。截断边界差 1 列;弹窗宽 44 足够,仅超长 label 截断时误差 1 列,接受,不修。
- **无 icons.js**:回退无图标,行对齐不受影响。
- **kind 缺失**:回退空,不报错。

## 5. 验证

- **icons.js node 自检**(module.exports 模式):`getCompletionIcon(7)` 非空;`getCompletionIcon(999)` 返回 `""`;completion 段键为数字 1-25。
- **手动**:ts server 下 `:ic` 补全候选显示图标,对齐正常。

## 6. 规模

- 单文件 icons.js 加一段 + 一函数;demo 加 deps + 改一行渲染。约 1 任务,无 Rust 改动。
