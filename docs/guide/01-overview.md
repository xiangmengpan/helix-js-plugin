# 01 · 整体架构概览

> Helix 是一个 Rust 编写的模态终端编辑器。本文从全局视角介绍代码库的组织方式，
> 帮你建立"这个项目是怎么拼起来的"的地图。阅读时间约 10 分钟。

## 1. Workspace 结构

项目是一个 Cargo workspace，成员定义在根目录 `Cargo.toml` 的 `[workspace]`：

```toml
members = [
  "helix-core", "helix-view", "helix-term", "helix-tui",
  "helix-lsp-types", "helix-lsp", "helix-event", "helix-js",
  "helix-dap-types", "helix-dap", "helix-loader", "helix-vcs",
  "helix-parsec", "helix-stdx", "xtask",
]
```

### 1.1 依赖方向（核心心智模型）

各 crate 之间存在严格的依赖方向，理解它是理解整个代码库的钥匙：

```
                     ┌────────────┐
                     │ helix-term │  终端 UI 层（最上层）
                     └─────┬──────┘
        ┌──────────┬───────┼───────┬───────────┬───────────┐
        │          │       │       │           │           │
   ┌────┴───┐ ┌────┴───┐ ┌─┴────┐ ┌┴──────┐ ┌──┴───┐ ┌────┴───┐
   │helix-tui│ │helix-js│ │helix-│ │helix- │ │helix-│ │helix-  │
   │         │ │        │ │lsp   │ │dap    │ │event │ │vcs     │
   └────┬───┘ └────┬───┘ └─┬────┘ └┬──────┘ └──┬───┘ └────┬───┘
        │          │       │       │           │          │
   ┌────┴──────────┴───────┴───┐   │           │          │
   │        helix-view         │   │           │          │
   │  (Editor/Document/View)   │   │           │          │
   └────┬──────────────────────┘   │           │          │
        │                          │           │          │
   ┌────┴──────────────────────────┴───────┴───┴──────┴───┐
   │                    helix-core                        │
   │        (Rope/Selection/Transaction/Syntax/History)   │
   └───────────────────────┬──────────────────────────────┘
                           │
   ┌───────────────────────┴──────────────────────────────┐
   │  helix-stdx / helix-loader / helix-parsec / helix-lsp-types  │
   │  （基础工具：被上面所有层使用）                        │
   └──────────────────────────────────────────────────────┘
```

> 实际依赖树比上图更复杂（例如 `helix-view` 直接依赖 `helix-lsp`，`helix-term` 依赖几乎所有 crate），
> 上图只画主干。精确依赖请查看各 crate 的 `Cargo.toml`。

### 1.2 各 crate 一句话职责

| crate | 职责 | 规模 |
|-------|------|------|
| `helix-core` | 纯函数式编辑核心：文本、多光标、变更、语法树、undo。**不依赖 UI** | ~2 万行 |
| `helix-view` | 编辑器状态层：`Editor`（全局状态）、`Document`（打开的文件）、`View`（分屏） | ~1.5 万行 |
| `helix-term` | 终端 UI：事件循环、命令、keymap、渲染、异步任务调度 | ~3.2 万行 |
| `helix-tui` | 终端 UI 原语：`Buffer`/`Surface` 渲染模型、widgets（从 tui-rs fork） | ~6 千行 |
| `helix-lsp` | LSP 客户端：语言服务器进程管理、请求/通知收发 | ~4 千行 |
| `helix-lsp-types` | LSP 协议类型定义（从 lsp-types crate 再导出 + 补充） | ~1 万行 |
| `helix-dap` / `helix-dap-types` | Debug Adapter Protocol 客户端与类型 | ~2.3 千行 |
| `helix-event` | 事件原语：全局事件通道、`request_redraw` 等 | ~1 千行 |
| `helix-vcs` | 版本控制集成：git diff/hunk，供编辑器 diff 高亮 | ~1.4 千行 |
| `helix-loader` | 加载外部资源：语言配置、grammar、runtime 文件、日志路径 | ~2 千行 |
| `helix-parsec` | 解析器组合子库（用于解析 keymap 等） | 574 行 |
| `helix-stdx` | 标准库扩展：路径、进程、环境、错误处理等工具函数 | ~2.5 千行 |
| `helix-js` | JS 插件运行时（较新特性，通过 tree-house 的 JS 引擎执行插件） | 475 行 |
| `xtask` | 构建辅助任务（文档生成等），不在运行路径上 | - |

### 1.3 分层原则

- **core 是纯函数式的**：大多数操作不修改数据，而是返回新副本（参考 CodeMirror 6 的设计）。
  `helix-core/src/lib.rs` 顶部注释与 `docs/architecture.md` 都强调了这一点。
- **view 层是有状态的**：`Editor`/`Document`/`View` 是可变的大对象，命令通过 `&mut` 修改它们。
- **term 层是胶水**：把终端事件、异步任务（LSP/DAP/文件 IO）、渲染串起来，是最"命令式"的一层。

## 2. 核心设计理念

### 2.1 文本：Rope

缓冲区文本用 `Rope` 表示（再导出 `ropey` crate，见 `helix-core/src/lib.rs` 中
`pub use ropey::{..., Rope, ...}`）。Rope 是分块字符串结构：

- 克隆是 O(1)（引用计数），可以廉价地保存任意数量的文本快照；
- 任意位置插入/删除接近 O(log n)；
- 索引单位是**字符（char）**，不是字节。全代码库的坐标都是 char 索引。

### 2.2 多光标：Selection / Range

编辑模型的核心是多光标（multi-cursor）。`helix-core/src/selection.rs`：

- `Range { anchor, head }`：一个选区，`anchor` 固定端，`head` 移动端。单个光标就是
  `anchor == head` 的 Range。
- `Selection` 是 `Vec<Range>` 的封装（用 `SmallVec` 优化单光标情况，见 `Selection::new`），
  带一个 `primary_index` 标记主光标。所有编辑操作都作用于全部 Range。

### 2.3 变更：Transaction（OT 风格）

所有文本修改都通过 `Transaction` 进行（`helix-core/src/transaction.rs`）：

- `Transaction` 内部是 `ChangeSet`，由 `Operation`（`Retain`/`Delete`/`Insert`）序列组成，
  类似 OT 的变更表示，与 CodeMirror 6 的 changes 结构一致；
- `Transaction::change_by_selection(doc, selection, f)` 是主要编辑入口：对每个光标
  调用闭包 `f`，收集所有 Range 的修改，合并成一个事务；
- **可逆性**：`Transaction::invert(&original_rope)` 生成反向事务，undo 就是这么实现的；
- **可映射**：`ChangeSet::map_pos` 能把一个旧文档位置翻译成新文档位置，Selection 的
  `Range::map` / `Selection::map` 依赖它——这是"编辑后光标不跑偏"的机制。

### 2.4 撤销：History（undo 树）

`helix-core/src/history.rs` 维护撤销历史：

- `State` 是文档的一个快照（文本 + 选择）；
- `History::commit_revision` 把"上一状态 + 事务"压栈；
- `undo()` / `redo()` 通过 `Transaction::invert` 回放实现。

### 2.5 语法：tree-sitter 封装

`helix-core/src/syntax.rs` 的 `Syntax` 封装了 tree-sitter（通过 `tree_house` 包）：

- 支持**增量解析**：`Syntax::update(old_source, source, changeset, loader)` 复用
  编辑前的语法树，只重解析受影响的区域；
- 支持**语言注入**（layers）：内嵌的 HTML/JS/CSS、markdown 代码块等都作为独立 layer 解析，
  `layers_for_byte_range` 可查询覆盖某个范围的嵌套语言。

### 2.6 界面：Cursive 风格组件栈

`helix-term/src/compositor.rs` 的 `Compositor` 是一个组件层栈（借鉴 Cursive）：

- `Component` trait：`handle_event`（输入）、`render`（绘制）、`cursor`（光标位置）；
- 事件从**最顶层**开始向下"冒泡"，第一个 `Consumed` 的组件停止传播；
- 弹出层（补全菜单、命令面板、picker）都是 `push` 进栈的新层，自动盖在编辑器之上。

### 2.7 异步：Job 队列

LSP 请求、文件保存等异步操作不阻塞主循环（`helix-term/src/job.rs`）：

- 异步任务在后台执行，完成后通过 `callback` channel 把闭包送回主循环；
- 主循环 `tokio::select!` 同时监听：信号、终端事件、job 回调、编辑器事件（见
  `application.rs` 的 `event_loop_until_idle`）；
- `helix-event` crate 提供跨线程的 `request_redraw` / 事件通道。

## 3. 一次按键的完整链路（预告）

这是理解整个代码库最重要的主线，教程篇会展开，这里先给骨架：

```
终端事件 (termina/crossterm)
  → application.rs::handle_terminal_events
  → compositor::handle_event（事件冒泡到最顶层组件）
  → ui/editor.rs::Editor::handle_event（Event::Key）
  → keymap.rs::Keymaps::get(mode, key)  →  KeymapResult
  → MappableCommand::execute(&mut Context)
  → 具体命令函数（如 move_char_left / insert_mode / :write）
  → 修改 Editor / Document（Transaction）
  → request_redraw → render()  →  terminal.draw()
```

## 4. 入口文件速查

| 入口 | 位置 | 说明 |
|------|------|------|
| 二进制入口 | `helix-term/src/main.rs` | 参数解析、配置加载、启动 Application |
| 事件循环 | `helix-term/src/application.rs::Application::run` | `event_loop` → `event_loop_until_idle` |
| 渲染 | 同上 `Application::render` | compositor 渲染 → `terminal.draw` |
| 编辑器主组件 | `helix-term/src/ui/editor.rs` | 最大的 Component，处理按键与绘制 |
| 命令注册 | `helix-term/src/commands.rs` | `static_commands!` 宏 + `TYPABLE_COMMAND_MAP` |
| 默认 keymap | `helix-term/src/keymap/default.rs` | 各模式按键绑定 |

## 5. 设计参考与文档

- `docs/architecture.md`：官方高层架构说明（英文，较简略）；
- `docs/vision.md`：项目愿景；
- 核心模型借鉴 [CodeMirror 6](https://codemirror.net/6/docs/)（Rope、changes、Selection 映射），
  组件栈借鉴 [Cursive](https://github.com/gyscos/cursive)。
