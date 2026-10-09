# 报告:任务 2 编辑路由与集成测试(term 侧)

计划:`docs/superpowers/plans/2026-08-28-js-cross-buffer.md` 任务 2
Commit:`6b66e000d`

## 实现内容

1. **`helix-term/src/commands/typed.rs` — 重写 `apply_plugin_edits`**
   - 按 `Edit.doc` 分组(保持组内顺序),`None` = 当前 buffer(现状),`Some(path)` = 其它 buffer
   - 每组对目标 doc 一个 `Transaction` → 每个被改 buffer 独立一次撤销
   - 其它 buffer 应用事务用命令开始时捕获的 `current_view_id`(先复制释放借用)
   - 目标 buffer 已被关闭 → `anyhow!` 报错(走既有 "plugin edit failed" 错误路径,不崩)
   - 复用既有 `pos_to_char`、`Rope`/`Transaction` 导入(均在 `commands.rs` 顶层导入,无需新增)
   - 修复一处:计划代码 `current!(editor).0` 实际是 `&mut View`,改为 `.0.id` 才是 `ViewId`

2. **`helix-term/tests/integration.rs`** — `mod plugin_cross_buffer;`(放在 `mod plugin_docchange;` 后)

3. **`helix-term/tests/test/plugin_cross_buffer.rs`(新,5 个测试)**
   - `plugin_by_path_reads_other_buffer` — by_path 读另一已打开 buffer 文本
   - `plugin_by_path_missing_returns_null` — 未打开路径 → null
   - `plugin_edit_other_buffer_undo_per_doc` — 改另一 buffer + 一次 undo 回退该 buffer
   - `plugin_mixed_edits_undo_independent` — 一个命令混合改当前+其它,各自独立撤销
   - `plugin_async_begin_edit_cross_buffer` — async 命令 begin_edit 跨 await 改其它 buffer 一次事务

## TDD 证据

- **RED**:仅加测试跑旧 `apply_plugin_edits`(忽略 `edit.doc`)——
  `plugin_mixed_edits_undo_independent`、`plugin_async_begin_edit_cross_buffer` FAILED(编辑落到当前 buffer,另一 buffer 未变),3 个读/巧合路径测试通过
- **GREEN**:重写 `apply_plugin_edits` 后 5 个测试全 PASS

## 与计划的偏差(均为计划代码 bug,按规格语义修正)

1. 测试 4/5 的命令执行前未 `:open file2` —— `by_path` 按规格只查**已打开** buffer,未打开 → null,编辑永远不生效。测试 4 增加 `:open file2` + 切回 `:open file1`(保持"当前 buffer ≠ 编辑目标"的混合语义);测试 5 增加 `:open file2`。
2. 测试 3 `file1` move 进 `with_file` 后又被 `:open file1` 借用 → 编译错误,改为 `with_file(file1.clone(), ...)`。
3. 测试文件需显式 `use helix_view::current_ref;`(filetree.rs 同款——`current_ref!` 宏不随 `use super::*` 双层 glob 传递)。
4. `current!(editor).0` → `.0.id`(类型不匹配)。

## 验证

- `cargo test -p helix-js` → 74 passed(旧测试全绿)
- `cargo test -p helix-term --features integration --test integration plugin_cross_buffer` → 5 passed
- 回归:`plugin_doc`+`plugin_docchange` 9 passed、`plugin_async` 2 passed、`plugin_popup_edit` 1 passed、`plugin_run` 1 passed(apply_plugin_edits 是共享路径)
- clippy:`cargo clippy -p helix-term --features integration --tests` 53 条 warning,与 HEAD 基线(stash 后同样 53 条)一致,**零新增**;新文件与 typed.rs 改动处无任何 clippy 警告

## 自检

- 完整性:计划任务 2 步骤 1-6 全部完成(规格覆盖 2.3 编辑路由、2.4 边界、3.2 五条集成测试)
- 质量:改动限定在计划文件清单内,未动无关代码;错误路径(关闭报错)按计划实现,未加未请求的测试
- 遗留:`.pi/tasks/tasks-01a048c7-*.json` 中任务 8 状态仍为 pending,由父代理更新;工作区有一个**先前会话遗留**的 `.pi/tasks/tasks-01a042aa-*.json` 删除(非本次改动,未提交)
