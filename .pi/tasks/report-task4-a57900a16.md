# 任务 4 实现报告：open_file 扩展行列定位

计划：`docs/superpowers/plans/2026-08-26-js-lsp-request.md` 任务 4
Commit：`a57900a16 feat(js): open_file 支持行列定位`

## 实现内容（TDD：RED → GREEN）

1. **集成测试** `helix-term/tests/test/plugin_lsp.rs`（追加 `plugin_open_file_with_position`）：
   10 行文件（line0..line9），插件命令 `:jump10` 调 `helix.open_file(路径, { row: 9, col: 4 })`，
   断言光标落到第 9 行。补 `use helix_view::current_ref;` 导入。

2. **helix-js/src/types.rs**：`UiRequest::OpenFile` 加 `row: Option<u16>` / `col: Option<u16>`。

3. **helix-js/src/popup.rs**：`js_open_file` 解析可选第二参数对象 `{ row, col }`
   （复用 `opt_u16`，null/undefined → None；非对象第二参数 → 与旧行为一致，向后兼容）。

4. **helix-term/src/commands/typed.rs**：OpenFile 分支在 `editor.open` 成功后，
   `(Some(row), Some(col))` → `pos_to_char`（既有 clamp helper，行/列越界防 ropey panic，
   JS 输入不可信）→ `set_selection` → `align_view(Center)`（与 `:open path:row:col` 同款定位语义）。

5. **helix-js/src/lib.rs**：`sidecar_apis` 单测 `matches!` 解构补 `row: None, col: None`。

## TDD 证据

- RED：`cargo test -p helix-term --features integration --test integration plugin_open_file_with_position`
  → FAILED（`left: 0, right: 9`：open_file 忽略第二参数，光标停在原处）。
  期间修两个实现前编译问题：`current_ref` 宏未导入；`opt_u16` 已返回 `Option<u16>`，
  误包 `Some()` 成 `Option<Option<u16>>`（均属测试/实现自身的编译修正，非设计变更）。
- GREEN：同一命令 → `ok`。
- 回归：`cargo test -p helix-js` → 64 passed；`plugin_` 集成组 → 71 passed（含
  先前偶发 flaky 的 `plugin_reload_command`，本次全绿）。

## 验证

- `cargo clippy -p helix-js` / `-p helix-term` → 无新 warning/error。
- fmt：本任务新增/修改行全部 fmt-clean（popup.rs:361 链式调用、lib.rs:367 断言、
  typed.rs pos 行、plugin_lsp.rs 全文——该文件全部为任务 3/4 代码，直接整体 rustfmt）。
  其余 drift 为既有全 crate 问题（任务 2/3 已记录，非本次引入）。

## 自审

- **YAGNI**：定位复用既有 `pos_to_char`（clamp）与 `align_view`，无新抽象、无新依赖。
- **边界**：行列越界 clamp 防 panic；`editor.open` 失败走原有 set_error，不定位；
  不传第二参数行为与旧版完全一致（向后兼容）。
- **偏差记录**：`pos_to_char` 的 col clamp 是 `(line_start + col).min(len_chars)`——
  超行尾的 col 会落到下一行而非行尾（与计划里 `line_to_char+col` 直加同语义），
  与 `:open` 的 `pos_at_coords` 行尾截断略有差异；角落场景，计划与实现一致，接受。
- **ponytail 标注**：无故意简化留下的已知上限（row/col 用 u16 与 open_popup 位置约定一致）。

## 交接

任务 5（demo 插件 + 文档）可直接开始：`helix.open_file(path, {row, col})` 已在真链路
可用（本任务集成测试验证）；`helix.lsp.*` 四方法亦在任务 3 就绪。
