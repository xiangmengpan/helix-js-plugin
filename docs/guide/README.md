# Helix 代码库指南

> 本文档集合帮助中文读者理解 [Helix 编辑器](https://github.com/helix-editor/helix) 的 Rust 代码库。
> 所有论断均标注真实代码路径，可对照源码验证。

## 三层结构

```
docs/guide/
├── README.md                ← 你在这里
│
│  ── A 层 · 快速概览 ──
├── 01-overview.md           整体架构：15 个 crate 的职责、依赖关系、核心设计理念
│
│  ── C 层 · 入门教程（跟着一条主线走完编辑器）──
├── 02-tutorial-startup.md   从 main() 到编辑器就绪
├── 03-tutorial-keypress.md  一个按键的旅程：终端事件 → keymap → 命令
├── 04-tutorial-edit.md      一次编辑的生命周期：命令 → Transaction → Rope → undo
├── 05-tutorial-render.md    渲染流程：View → Surface → 终端绘制
│
│  ── B 层 · 深入理解（按 crate 拆解）──
├── 06-deep-core.md          helix-core：Selection / Transaction / Syntax / 文本处理
├── 07-deep-view.md          helix-view：Editor / Document / View / History
├── 08-deep-term.md          helix-term：事件循环 / Compositor / keymap / 命令体系
├── 09-deep-lsp.md           helix-lsp + helix-lsp-types：LSP 客户端
└── 10-deep-others.md        其余 crate 速览（tui / event / vcs / dap / loader / parsec / stdx / js）
```

## 推荐阅读顺序

| 目的 | 顺序 |
|------|------|
| 只看架构 | 01 → README 其他部分按需 |
| 完整入门 | 01 → 02 → 03 → 04 → 05 |
| 准备改代码 | 01 → 02~05 → 对应 crate 的深挖篇（06~10） |

## 关键速查

| 想找什么 | 去哪个文件 |
|----------|-----------|
| 程序入口 | `helix-term/src/main.rs` |
| 事件循环 | `helix-term/src/application.rs`（`event_loop_until_idle`） |
| UI 组件栈 | `helix-term/src/compositor.rs`（`Component` / `Compositor`） |
| 按键映射 | `helix-term/src/keymap.rs`（`Keymaps::get`） |
| 命令实现 | `helix-term/src/commands.rs`（`MappableCommand`）+ `commands/typed.rs`（`:命令`） |
| 文本编辑模型 | `helix-core/src/transaction.rs`（`Transaction` / `ChangeSet`） |
| 多光标 | `helix-core/src/selection.rs`（`Selection` / `Range`） |
| 语法高亮 | `helix-core/src/syntax.rs`（`Syntax`，tree-sitter 封装） |
| 撤销历史 | `helix-core/src/history.rs`（`History` / `State`） |
| 打开的文件 | `helix-view/src/document.rs`（`Document`） |
| 全局状态 | `helix-view/src/editor.rs`（`Editor`） |
| 分屏树 | `helix-view/src/tree.rs`（`Tree` / `Container`） |
| 文本缓冲区原语 | `helix-tui/src/buffer.rs`（`Buffer` / `Cell`） |
| LSP 客户端 | `helix-lsp/src/client.rs`（`Client`） |
