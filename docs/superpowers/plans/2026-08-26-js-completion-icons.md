# completion 弹窗候选图标实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 给 `:ic` 补全弹窗候选行加 LSP kind 图标,纯 JS 层(icons.js 加 completion 映射表 + demo 拼字符),零 Rust 改动。

**架构：** `plugins/lib/icons.js`(共享图标表,现有 4 消费者)新增 `ICONS.completion` 段(LSP CompletionItemKind 数字 1-25 → nerd font 字符)与 `getCompletionKindIcon(kind)` 查表函数;`plugins/features/input-completion/index.js` 声明 `deps: ["lib/icons.js"]` 依赖,候选行渲染时拼图标,无图标/加载失败回退空。

**技术栈：** 纯 JavaScript(插件层),无 Rust。node(自检)。

**规格：** `docs/superpowers/specs/2026-08-26-js-completion-icons-design.md`(已批准)

---

### 任务 1：icons.js — completion 图标表 + 查表函数

**文件：**
- 修改：`plugins/lib/icons.js`(ICONS 表加 `completion` 段;工具函数区加 `getCompletionKindIcon`;export 列表加它)

- [ ] **步骤 1：定义失败测试**(node 断言命令,测试不写进文件——node -e 直接 require 验证)

```bash
node -e "const i=require('./plugins/lib/icons.js'); const a=require('assert'); a.strictEqual(i.getCompletionKindIcon(7) && i.getCompletionKindIcon(7).length>0, true, 'Class kind 应有图标'); a.strictEqual(i.getCompletionKindIcon(999), '', '未知 kind 回退空'); a.strictEqual(i.getCompletionKindIcon(undefined), '', '缺省 kind 回退空'); console.log('icons completion 自检通过')"
```

- [ ] **步骤 2：运行确认失败**

运行：上面的 node -e 命令。预期：FAIL(TypeError: i.getCompletionKindIcon is not a function)。

- [ ] **步骤 3：加 completion 映射表 + 查表函数**(`plugins/lib/icons.js`)

在 `ICONS` 表 git 段之后加(码位为 telescope/nvim-web-devicons 标准 nerd font 映射;若某字符终端显示异常,在任务 2 步骤 4 手动验证时调整该键值):

```js
  // LSP CompletionItemKind(数字)→ 补全候选图标(telescope lsp 图标映射)
  completion: {
    1: "\uf031",   // Text
    2: "\uf6fc",   // Method
    3: "\uf794",   // Function
    4: "\uf6f6",   // Constructor
    5: "\uf6f3",   // Field
    6: "\uf6f4",   // Variable
    7: "\uf6f9",   // Class
    8: "\uf6f8",   // Interface
    9: "\uf6f7",   // Module
    10: "\uf6f5",  // Property
    11: "\uf475",  // Unit
    12: "\uf6f4",  // Value
    13: "\uf6fa",  // Enum
    14: "\uf6fc",  // Keyword
    15: "\uf6f6",  // Snippet
    16: "\uf475",  // Color
    17: "\uf6f7",  // File
    18: "\uf6f5",  // Reference
    19: "\uf6f9",  // Folder
    20: "\uf6fa",  // EnumMember
    21: "\uf6f4",  // Constant
    22: "\uf6f8",  // Struct
    23: "\uf6f9",  // Event
    24: "\uf6f7",  // Operator
    25: "\uf6f5",  // TypeParameter
  },
```

工具函数区(git 段 getGitIcon 之后)加:

```js
/// LSP kind 数字 → 补全图标;未知/缺省 → ""(无图标)
function getCompletionKindIcon(kind) {
  return ICONS.completion[kind] ?? "";
}
```

export 列表追加 `getCompletionKindIcon`。

- [ ] **步骤 4：运行 node 自检确认通过**

运行：上面的 node -e 断言命令。预期：输出图标字符(Class kind)、空、空,打印 "icons completion 自检通过"。

- [ ] **步骤 5：Commit**

```bash
git add plugins/lib/icons.js
git commit -m "feat(plugins): icons.js 加 completion kind 图标表 + getCompletionKindIcon"
```

### 任务 2：input-completion demo — 候选行拼图标

**文件：**
- 修改：`plugins/features/input-completion/index.js`

- [ ] **步骤 1：加插件声明 + 依赖**(文件顶部注释后,`let pid = null;` 之前)

```js
helix.plugin("input-completion", { deps: ["lib/icons.js"] });
let ICONS = null;   // icons.js 的 exports;load 后填充,缺失回退无图标
```

- [ ] **步骤 2：`:ic` 命令入口加载共享表**(`helix.register_command("ic", ...)` 回调第一行)

```js
if (!ICONS) ICONS = helix.load("lib/icons.js") || null;
```

- [ ] **步骤 3：候选行渲染拼图标**(`items.forEach` 里的 text 行)

改前：
```js
text: (i === sel ? "> " : "  ") + label(it),
```
改后：
```js
text: (i === sel ? "> " : "  ") + (ICONS ? ICONS.getCompletionKindIcon(it.kind) + " " : "") + label(it),
```

- [ ] **步骤 4：语法检查 + 手动验证**

运行：`node --check plugins/features/input-completion/index.js`
预期：无语法错误。

手动(需要 helix 运行时 + typescript-language-server)：`:plugin-load plugins/features/input-completion/index.js` → `:ic` → Tab 聚焦输入框 → 输入触发补全 → 候选行显示 kind 图标,`>` 光标行对齐正常;无 icons.js(临时改名验证)时候选行无图标但不错位。若有图标字符终端显示为方块/错位,回任务 1 步骤 3 调整对应码位。

- [ ] **步骤 5：Commit**

```bash
git add plugins/features/input-completion/index.js
git commit -m "feat(plugins): input-completion 候选行按 kind 显示图标"
```
