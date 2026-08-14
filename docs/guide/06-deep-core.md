# 06 · 深入 helix-core：函数式编辑核心

> `helix-core` 是编辑器的"大脑"：文本、多光标、变更、撤销、语法树、文本处理算法，
> 全部**不依赖任何 UI**，可独立测试。约 43 个文件、2 万行。
> 模块清单见 `helix-core/src/lib.rs` 的 `pub mod` 列表。

## 核心概念

- **函数式**：多数操作不修改输入，而是返回新值（参考 CodeMirror 6）。例如
  `Transaction` 应用是"构造 → 应用到 Document"，`Selection::map` 返回新 Selection。
- **坐标即 char 索引**：全文用 Rope 的字符索引（`usize`），不用字节索引。
- **一切编辑皆 Transaction**：任何文本变化都打包成事务，统一走"应用 → 映射 → 提交历史"。

## 1. 文本表示：Rope（lib.rs 再导出 ropey）

`helix-core/src/lib.rs`：

```rust
pub use ropey::{self, str_utils, Rope, RopeBuilder, RopeSlice};
pub type Tendril = SmartString<smartstring::LazyCompact>;  // 高效小字符串（插入文本用）
```

- `Rope`：分块字符串（块大小约 1KB），克隆 O(1)、任意位置修改接近 O(log n)；
- `RopeSlice`：借用视图，用于遍历/切分文本；
- `Tendril`：`ChangeSet::insert` 的新文本类型，避免小片段堆分配。

配套工具（`rope_reader.rs` 导出 `RopeReader`）：把 Rope 转成可迭代的字符流，
供 LSP 转换等场景使用。

## 2. 多光标模型：Selection / Range（selection.rs）

```rust
pub struct Range {
    pub anchor: usize,                 // 固定端
    pub head: usize,                   // 移动端
    pub old_visual_position: Option<(u32, u32)>, // 垂直移动时的视觉位置记忆
}
```

核心方法：

| 方法 | 作用 |
|------|------|
| `Range::new(anchor, head)` / `point(head)` | 构造选区 / 单点光标 |
| `from()` / `to()` | 区间两端（min/max） |
| `direction()` / `flip()` | 前后方向判断 / 翻转 |
| `cursor(text)` | 移动端（head），即光标所在位置 |
| `line_range(text)` | 覆盖的行范围 |
| `map(&ChangeSet)` | 用变更集映射到新坐标（见 04 教程） |

```rust
pub struct Selection {
    ranges: SmallVec<[Range; 1]>,  // 单光标时零堆分配
    primary_index: usize,          // 主光标下标
}
```

`Selection` 的方法：`primary()`（主光标）、`push()`（加光标，如 Alt+点击）、`map()`、
`transform()`（批量变换）、`cursors(text)`、`ensure_invariants()`（排序、去重、防止重叠）。

> 关联概念：`helix-core/src/movement.rs` 提供所有移动函数，签名统一为
> `fn(RopeSlice, Range, count) -> Range`，例如 `move_next_word_start`、`move_vertically_visual`
> （视觉行移动，考虑软换行）。`graphemes.rs` 提供字形（grapheme cluster）边界与宽度计算
> （`next_grapheme_boundary` / `grapheme_width`），是"光标不能停在组合字符中间"的保证。

## 3. 变更：Transaction / ChangeSet（transaction.rs）

```rust
pub enum Operation {
    Retain(usize),   // 保留 n 字符
    Delete(usize),   // 删除 n 字符
    Insert(Tendril), // 插入文本
}

pub struct ChangeSet {
    changes: Vec<Operation>,
    len: usize,        // 应用前文档长度（校验）
    len_after: usize,  // 应用后文档长度
}

pub struct Transaction {
    changes: ChangeSet,
    selection: Option<Selection>,  // 显式的新光标（可选）
}
```

### 构造

| API | 用途 |
|-----|------|
| `Transaction::change(doc, changes)` | 从 `(from, to, Option<Tendril>)` 迭代器构造 |
| `Transaction::change_by_selection(doc, sel, f)` | **每光标一改**（最常用） |
| `Transaction::change_by_and_with_selection` | 同上，且返回新 Selection |
| `Transaction::delete(doc, deletions)` | 批量删除（合并重叠） |
| `Transaction::change_ignore_overlapping` | 跳过重叠修改 |

### 关键操作

```rust
pub fn apply(&self, doc: &mut Rope) -> bool          // 应用到 Rope，长度校验失败返回 false
pub fn invert(&self, original: &Rope) -> Self        // 反向事务（undo 核心）
pub fn compose(self, other: Self) -> Self            // 合并两个事务
pub fn map_pos(&self, pos: usize, assoc: Assoc) -> usize  // 坐标迁移
```

`ChangeSet::map_pos(pos, assoc)` 的 `Assoc`（粘附方向）是坐标映射的精华，
详见 [04-tutorial-edit.md](./04-tutorial-edit.md) 第 4 节。

## 4. 撤销历史：History（history.rs）

```rust
pub struct State {
    pub doc: Rope,            // 文本快照（Rope 克隆廉价）
    pub selection: Selection,
}

pub struct History {
    revisions: Vec<Revision>, // 修订栈（含根修订）
    current: usize,           // 当前修订下标
}
```

- 每个 `Revision` 存：父修订、**正向事务**、**反向事务**（invert 得到，因为 delete 不存被删文本）；
- `commit_revision(&transaction, &original)`：压栈；
- `undo()` / `redo()`：返回要应用的 `Transaction`；
- 支持**按时间导航**（`:earlier Ns` / `:later Ns`），修订带时间戳（见 history.rs 顶部大段注释）；
- `UndoKind`（`history.rs:307`）区分 undo 类型（如合并间隔、跳过选择变更）。

## 5. 语法：Syntax / Loader（syntax.rs）

`Syntax` 封装 tree-sitter（经 `tree_house` crate 集成）：

```rust
pub struct Syntax {
    inner: tree_house::Syntax,   // 实际语法树，支持多 layer（语言注入）
}
```

| 方法 | 作用 |
|------|------|
| `Syntax::new(source, language, loader)` | 全量解析 |
| `Syntax::update(old_source, source, changeset, loader)` | **增量解析**：把 ChangeSet 转成 tree-sitter edits，只重解析受影响区域 |
| `layers_for_byte_range(start, end)` / `layer_for_byte_range` | 查询覆盖某范围的注入语言层（内嵌 HTML/JS、markdown 代码块等） |
| `highlighter()` | 返回迭代器，产出 `(byte_range, Highlight)` 供渲染 |
| `tree()` / `walk()` / `named_descendant_for_byte_range` | 语法树访问（textobject、movement 用） |

`Loader`（`syntax.rs:275`）是语法配置的注册中心：

- `Loader::new(Configuration)`：从 `languages.toml` 构建；
- 语言查找：`language_for_filename`（扩展名）、`language_for_shebang`（`#!/usr/bin/env python`）、
  `language_for_scope`、`language_for_match`；
- 编译查询：`indent_query` / `textobject_query` / `tag_query` / `rainbow_query`（`compile_*` 方法），
  把 `indents.scm`、`textobjects.scm` 等 query 文件预编译；
- `language_server_configs()`：每种语言的 LSP 启动配置（命令、参数、环境）。

`LanguageData`（`syntax.rs:40`）：单一语言的全部数据（配置、语法树实例池、查询）。

## 6. 文本处理工具模块

| 模块 | 内容 | 代表 API |
|------|------|---------|
| `graphemes.rs` | 字形簇与宽度 | `next_grapheme_boundary`、`grapheme_width`、`Grapheme` 迭代器 |
| `chars.rs` | 字符分类 | `char_is_word`、`char_is_whitespace` |
| `movement.rs` | 光标移动 | `move_horizontally`、`move_vertically_visual`、`move_next_word_start` 等 |
| `search.rs` | 搜索 | `find_nth_char`、`find_next`（配合 regex 模块） |
| `textobject.rs` | 文本对象（`iw`/`ap` 等） | `textobject_word`、`textobject_pair_surround`、`textobject_treesitter` |
| `object.rs` | 对象选择辅助 | 与 textobject 配合 |
| `comment.rs` | 注释切换 | `toggle_line_comments`、`toggle_block_comments`（返回 Transaction） |
| `surround.rs` | 包围符（`ms`/`ds`/`cs`） | `find_nth_pairs_pos`、`get_surround_pos` |
| `auto_pairs.rs` | 自动配对 | `AutoPairs`、`hook_insert`（插入时自动补右括号） |
| `snippets.rs` | 代码片段（`Tabstop` 解析） | `TabstopIdx`、片段模板解析 |
| `indent.rs` | 缩进 | `auto_detect_indent_style`、`IndentQuery`（tree-sitter 缩进查询求值） |
| `diagnostic.rs` | 诊断（LSP 来源） | `Diagnostic`（range/severity/provider） |
| `diff.rs` | 文本 diff | `compare_ropes`（返回表示差异的 Transaction） |
| `doc_formatter.rs` | 软换行格式化 | `DocumentFormatter`、`TextFormat`（渲染层逐视觉行遍历文本） |
| `text_annotations.rs` | 行内标注 | `Overlay`、`TextAnnotations`（虚拟文本、inlay hints 的基础） |
| `line_ending.rs` | 行尾 | `LineEnding`、`get_line_ending_of_str` |
| `case_conversion.rs` | 大小写 | 供 `~`、`Alt+c` 等命令 |
| `increment/` | 数字/日期递增（`Ctrl+a/x`） | `increment::integer`、`increment::date_time` |
| `match_brackets.rs` | 括号匹配 | 高亮/跳转括号对 |
| `wrap.rs` | 文本环绕 | `hardwrap` 等 |
| `position.rs` | 坐标换算 | `pos_at_coords`、`char_idx_at_visual_offset`（行/列 ↔ char 索引，处理软换行） |
| `uri.rs` | URL 工具 | 文件路径 ↔ URL |
| `command_line.rs` | 命令行解析 | `Args`（`:命令` 参数解析） |
| `editor_config.rs` | 编辑器配置类型 | `Config`（`editor` 部分的强类型定义） |
| `fuzzy.rs` | 模糊匹配 | picker 过滤用 |
| `config.rs` | 语言配置加载 | `user_lang_loader` / `default_lang_loader` |
| `test.rs` | 测试工具 | 供各模块测试使用 |
| `macros.rs` | 宏工具 | 内部宏 |

## 7. 数据流：一次完整编辑在 core 层的样子

```
命令函数（在 helix-term）
 └─ Transaction::change_by_selection(doc, selection, f)   // core：每光标一个 Change
     └─ ChangeSet（Retain/Delete/Insert 序列）
         ├─ Transaction::apply(&mut Rope)                 // core：改文本
         ├─ Selection::map / ChangeSet::map_pos           // core：光标迁移
         ├─ Syntax::update                                // core：增量解析
         └─ History::commit_revision(transaction, old_state) // core：入撤销栈
```

**core 层对外接口是纯函数式的**：输入 Rope/Selection，输出 Transaction；状态的累积
（`Document`/`Editor`）在 view 层。

## 8. 代码位置指引

| 主题 | 位置 |
|------|------|
| 模块总览 | `helix-core/src/lib.rs` |
| 选区/光标 | `helix-core/src/selection.rs` |
| 事务/变更 | `helix-core/src/transaction.rs` |
| 撤销历史 | `helix-core/src/history.rs` |
| 语法/高亮 | `helix-core/src/syntax.rs` |
| 移动 | `helix-core/src/movement.rs` |
| 文本对象 | `helix-core/src/textobject.rs` |
| 软换行格式化 | `helix-core/src/doc_formatter.rs` |
| 语言配置加载 | `helix-core/src/config.rs` |

测试：core 是纯函数层，测试最密集——各模块内联 `#[cfg(test)]` 单元测试是理解
函数行为的最佳资料（如 `selection.rs` 末尾、`transaction.rs` 末尾的测试）。
