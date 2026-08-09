# 设计：插件 doc 元数据（特性 D，串行第一）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

插件命令可以带说明文字，输入 `:命令` 时在命令行提示区显示（helix 原生 doc UX——没有独立 `:doc` 命令，说明展示在命令输入提示区，`command_line_doc`）。

## 新增 JS API

```js
helix.register_command("show-where", (ctx) => { ... }, "显示光标位置"); // 第三参可选
helix.register_command("plain", () => {});  // 无 doc
```

- 第三参数可选：字符串 → 存储；省略/undefined → doc 为空；其他类型 → JS 报错

## 实现

### helix-js

- thread_local：`COMMAND_DOCS: RefCell<HashMap<String, String>>`
- `js_register_command` 读取可选第三参（存在时校验字符串）→ 存入
- 公共函数：`pub fn command_doc(name: &str) -> Option<String>`

### helix-term（typed.rs）

- `command_line_doc`（L4408）：静态表 miss 时回退 `helix_js::command_doc(command)` → `Some(Cow::Owned(doc))`

## 测试

- **helix-js 单测**：register 带 doc → command_doc 返回；无 doc → None；非法 doc 类型（42）→ 报错
- **集成测试**（tests/test/plugin_doc.rs）：`:plugin-load` 注册带 doc 命令 → 输入 `:cmd名` 触发 doc 显示——prompt doc 的断言方式：输入命令名后检查 prompt 组件 doc_fn 输出（若测试不可行，退化为单测 + 手动冒烟，报告中注明）

## 非目标

- palette/补全列表显示 doc（只做命令行提示区）
- doc 的本地化/多行富文本（纯字符串）

## 涉及文件

- `helix-js/src/lib.rs`（COMMAND_DOCS、register_command 第三参、command_doc、单测）
- `helix-term/src/commands/typed.rs`（command_line_doc 回退）
- `helix-term/tests/test/plugin_doc.rs`（新）
