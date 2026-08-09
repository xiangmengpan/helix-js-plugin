# 设计：状态栏元素（并行特性 B）

日期：2026-08-09
状态：已批准

## 目标

让 JS 插件在状态栏右侧渲染自定义文本：`helix.set_statusline(fn)`。

## 新增 JS API

```js
helix.set_statusline((ctx) => {
  return ctx.mode + " " + (ctx.cursor.row + 1) + ":" + (ctx.cursor.col + 1);
});
helix.set_statusline(null); // 清除钩子
```

- `fn(ctx) -> string | null`：返回字符串 → 渲染在状态栏右侧（既有元素之后）；返回 null / 抛错 → 不显示
- `ctx = { path: string | null, mode: "normal"|"insert"|"select", cursor: { row, col } }`——**轻量，不含 doc.text**（状态栏每帧渲染，避免每帧克隆全文）
- 单例：重复调用覆盖；`set_statusline(null)` 或 `set_statusline()` 清除
- 校验：参数必须是函数或 null/undefined，否则 JS 报错

## 实现

### helix-js

- thread_local 单例：`STATUSLINE_HOOK: RefCell<Option<JsValue>>`
- 原生函数 `js_set_statusline`（挂 helix 对象）：null/undefined → 清除；函数 → 存储；其他 → 报错
- 公共函数：`pub fn statusline_text(ctx: &StatuslineCtx) -> Option<String>`
  - `pub struct StatuslineCtx { pub path: Option<String>, pub mode: String, pub cursor: (usize, usize) }`（纯数据）
  - 无钩子 → None；调 hook（ctx 对象）→ 返回 string → Some；null/非字符串/抛错 → None（错误吞掉，状态栏渲染不容失败）

### helix-term（ui/statusline.rs）

- `render` 的右侧渲染后追加：`if let Some(text) = helix_js::statusline_text(&ctx) { set_string(..., text, style) }`
- ctx 构造：从 RenderContext 的 editor/view/doc/mode 取 path、mode、primary cursor
- 样式：`theme.get("ui.statusline")` 或随右侧元素样式

## 测试

- **helix-js 单测**：注册 → statusline_text 返回文本；返回 null → None；抛错 → None；未注册 → None；set_statusline(null) 清除
- **集成测试**（`tests/test/plugin_statusline.rs`，新文件）：插件 set_statusline → 渲染 statusline 到独立 surface（仿 bufferline 集成测试模式）→ 断言渲染文本含插件字符串。RenderContext 若无法在测试构造，退化为单测 + 手动冒烟（计划中注明实测结论）

## 非目标

- 状态栏元素的多段注册/顺序控制（单钩子，固定右侧）
- 每帧克隆文档文本的富 ctx
- 与既有 statusline 配置元素的交互（插件文本追加在最后）

## 涉及文件

- `helix-js/src/lib.rs`（StatuslineCtx、js_set_statusline、statusline_text、单测）
- `helix-term/src/ui/statusline.rs`（渲染追加）
- `helix-term/tests/integration.rs`（mod 声明，合并时由控制器统一加）
- `helix-term/tests/test/plugin_statusline.rs`（新，集成测试）
