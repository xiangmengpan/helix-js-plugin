# 设计:内置补全菜单 kind 图标钩子

日期:2026-08-26
状态:草案(待审核)

## 1. 动机

内置补全菜单(C-x / completion 命令)的 kind 目前渲染为文本(`"method"`/`"function"`/`"class"`…),无图标。目标:提供 JS 插件钩子,让插件为 kind 提供 nerd font 图标;未注册钩子或钩子返回空时回退为现有默认文本。

复用 `set_buffer_icon` 的成熟模式(JS 注册回调 → Rust 渲染时调用 → 无返回回退),不引入新架构。

## 2. API

```js
helix.set_completion_icon((kind) => "…");
// kind: LSP CompletionItemKind 数字(1-25,与 icons.js getCompletionKindIcon 键一致)
// 返回:图标字符(非空);""/null/undefined/抛错 → 回退默认文本
```

- 参数必须是函数,否则 JS 报错(`set_completion_icon: expected a function`)
- 覆盖注册:重复调用替换旧 hook
- 传数字而非字符串:与 icons.js 现有数字键表(`getCompletionKindIcon`)直接兼容,插件一行注册:
  ```js
  helix.set_completion_icon((k) => ICONS.getCompletionKindIcon(k));
  ```

## 3. 实现

### 3.1 helix-js(popup.rs,与 buffer_icon hook 并排)

- thread_local hook 存储:`COMPLETION_ICON_HOOK: OnceLock<Mutex<Option<JsValue>>>`(或与 buffer_icon 同款模式)
- `js_set_completion_icon`:校验 callable → 存 hook;注册到 `helix` 对象(lib.rs builder)
- `pub fn completion_kind_icon(kind: u8) -> Option<String>`(仿 `bufferline_icon`):
  - 无 hook → None
  - 调 hook(kind 数字)→ 返回值;空串/非字符串/抛错 → None

### 3.2 helix-term(ui/completion.rs)

format 里现有 match 是 `kind → 文本/■色块`,保持其产出(kind 展示内容),每分支顺带带出数字:

```rust
// match 产出 (Spans, u8):kind 文本(COLOR 分支为 ■ 色块 Spans)+ LSP 数字
fn kind_cell(kind: &lsp::CompletionItemKind) -> (Spans, u8) { … }
```

format 中:

```rust
let (kind_spans, kind_num) = kind_cell(&item.kind);
let kind_cell = match helix_js::completion_kind_icon(kind_num) {
    Some(icon) => menu::Cell::from(Span::raw(icon)),
    None => menu::Cell::from(kind_spans),  // 原逻辑(含 COLOR 的 ■ 色块)
};
menu::Row::new([menu::Cell::from(label), kind_cell])
```

- 钩子返回图标时替换整个 kind cell(含 COLOR 的 ■ 色块);回退时保留原逻辑

## 4. 回退语义

| 场景 | 行为 |
|---|---|
| 未注册 hook | 默认 kind 文本(现状) |
| hook 返回 ""/null/非字符串 | 默认 kind 文本 |
| hook 抛错 | 默认 kind 文本(错误被吞,与 bufferline_icon 同款) |
| 终端不支持 nerd font | Rust 无法自动检测字体;用户不注册钩子即回退默认(JS 侧控制) |

## 5. 边界

- **性能**:钩子每候选行渲染调一次(与 tabbar 的 bufferline_icon 同量级,菜单行数 ~10-50,可接受)
- **kind 数字来源**:completion.rs 的 match 已有全部 kind 分支,数字与 LSP 协议编号一致(1-25),不引入 lsp-types 内部访问
- **非 LSP 候选**(buffer 词补全 `CompletionItem::Other`):kind 是字符串无数字,不调钩子,走原逻辑(不传数字的字符串)

## 6. 验证

- **helix-js 单测**(仿 bufferline_icon 测试):注册后返回图标;未注册 → None;返回空串/非字符串/抛错 → None;缺参/非函数 → JS 报错
- **completion 渲染测试**:构造 `CompletionItem`(LSP + 非 LSP),断言 format 输出含图标 vs 回退文本;测试环境默认禁 LSP 不影响(构造 item 不经 LSP 请求)
- **手动**:ts server 下内置 C-x,注册钩子后候选显示图标;不注册显示默认文本

## 7. 规模

- 2 个文件改动(helix-js popup.rs + lib.rs 注册;helix-term completion.rs)+ 测试,约 2 任务。
