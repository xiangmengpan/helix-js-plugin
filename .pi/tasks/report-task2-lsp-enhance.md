# LSP 增强 任务 2 报告（响应应用侧）

**实现者子代理** · 计划: `docs/superpowers/plans/2026-08-28-js-lsp-enhance.md` · 任务 2 brief: `.superpowers/sdd/2026-08-28-js-lsp-enhance/task-2-brief.md`

## 实现内容

1. **纯函数 `lsp_text_edits_to_transaction`**（application.rs）: TextEdit 列表 → Transaction，`lsp_range_to_range` 换算坐标（无效范围过滤）、排序 + 重叠检查（重叠 → `Err`，不 panic；与 `generate_transaction_from_edits` 的静默丢弃策略不同，spec 3.2 要求报错）。
2. **段 A 应用插入**（pump_term_events）: `LspResult { id, result, apply }` 解构出 apply → `apply_lsp_edits` 先应用编辑再 `resolve_lsp`（promise resolve 时编辑已落地）。应用成功 → 覆写 result 为摘要 JSON；失败 → log + resolve null（不 panic）。
3. **`handle_lsp_request` 4 新方法映射**:
   - `Format` → `LanguageServerFeature::Format` + `text_document_formatting`（FormattingOptions 用 doc 的 tab_width/indent_style，与内置 format 同源）→ 任务内 `to_value(edits)` 进 `LspApply::Format { doc_id: doc.id().as_u64(), edits }`。
   - `Rename` → `RenameSymbol` + `rename_symbol`（实际方法名，brief 写的是 `rename`）；newName 从 `req.params` 取字符串，缺失 → null。
   - `CodeActions` → 复用 `code_actions_for_range`（commands/lsp.rs:674，已 pub(crate)），pos 覆盖转 point Range；await 全部 future → 过滤 disabled（与菜单同规则）→ 序列化数组 resolve（无 apply）；空/全失败 → null。
   - `ExecuteCodeAction` → 无 server 请求：action JSON 直接走 `LspApply::ExecuteAction(params)` 通道；与 code_actions 同 server 选择（无 CodeAction server → resolve null）。
   - 请求失败 → 全部 resolve null（符合文档契约「请求失败 → null」，与查询类方法的 reject 语义不同，文档已注明）。
4. **`apply_lsp_edits`**（application.rs 私有 fn）: Format 按 u64 doc_id 查 doc（doc 已关 → bail 不 panic）→ `get_synced_view_id`（后台 doc 防 panic，批次 2 教训）→ txn 应用 + `append_changes_to_history`；WorkspaceEdit → `apply_workspace_edit` + `workspace_edit_file_count` 摘要 files；ExecuteAction → 反序列化 `CodeActionOrCommand` → 当前 doc 第一个 CodeAction server → `helix_view::action::Action::lsp(server_id, action).execute(editor)`。
5. **integration**: `plugin_lsp_enhance_no_server_resolves_null`（4 命令无 server 均 echo null）。
6. **docs/plugin-api.md**: 新小节（4 方法签名/摘要/语义：自动应用、一次撤销、rename 跨 buffer、code_actions 两阶段无状态、失败 null、无超时、format 仅全文档）；方法列表补 4 行、章节「四个方法」→「八个方法」。

## TDD 证据

- **RED**: 先加 2 个单测 → `cargo test -p helix-term lsp_text_edits_to_transaction` 编译失败（E0425 函数不存在）。
- **GREEN**: 实现后 `cargo test -p helix-term --lib lsp_text_edits` → 2 passed。

## 执行时调整（brief 标注「按实际调整」项）

- `Transaction::apply(&mut Rope)` 签名确认可用（测试按此断言）。
- `DocumentId` 无公开 from-u64 构造器 → Format apply 侧用 `editor.documents.keys().find(|did| did.as_u64() == doc_id)` 查回（typed.rs 同款模式），不引入 unsafe。
- rename client 方法实际名为 `rename_symbol`（brief 写的 `rename`）。
- `helix_core::Range` 无 `start/end` 字段 → 用 `from()/to()`；`IndentStyle` 在 `helix_core::indent`（helix-view 的私有）。
- offset_encoding 不进载荷：apply 侧按「与请求侧同源选择（同 feature 第一个 server）」推导，缺省 Utf16（`ponytail:` 注释标注多 server 天花板）。
- 段 A 失败时统一 resolve null（brief 允许「保留 result 或 Ok(None)」；execute 占位若保留会谎报 applied）。

## 文件变更

| 文件 | 变更 |
|---|---|
| `helix-term/src/application.rs` | +283：段 A 应用、4 方法映射、apply_lsp_edits、纯函数、workspace_edit_file_count、tests 模块 |
| `helix-term/tests/test/plugin_lsp.rs` | +71：无 server null 路径 integration |
| `docs/plugin-api.md` | +19：4 方法文档一节 |
| `helix-js/src/lib.rs`、`lsp.rs` | fmt-only（任务 1 遗留 drift，CI `fmt --check` 强制，顺带清） |

## 测试结果

- `cargo test -p helix-term --lib` → 76 passed（含 2 新单测）
- `cargo test -p helix-term --features integration --test integration plugin_lsp` → 3 passed（含 1 新 integration）
- `cargo test -p helix-js` → 79 passed（任务 1 回归无损）
- `cargo clippy --all-targets` → 零新警告（helix-core text_annotations 警告为存量，非本任务）
- `cargo fmt --all --check` → clean

## 自检

- 规格覆盖：2.1 摘要/null ✓、2.2 自动应用/一次撤销/无状态两阶段 ✓、2.3 数据流 ✓、2.4 边界（无超时/仅全文档/不查 version/跨 doc 打开/command 分发）✓、3.2 纯函数单测 ✓、3.3 integration null 路径 ✓。
- 段 A 单入口不变：CodeActions 也走通道 resolve，无跨线程碰 boa 引擎状态。
- Format/Execute 失败不 panic（doc 关闭/载荷畸形 → bail → null）。
- 重叠编辑 Err 不 panic（ChangeSet::from_changes 下溢风险已拦截）。

## 关注点

- ExecuteCodeAction 与列表的 server 一致性：execute 时取「当前 doc 第一个 CodeAction server」，与 code_actions 列表（`code_actions_for_range` 全 server）在多 server 环境下可能不匹配（列表项来自非首个 server 时）。简化接受，`ponytail:` 注释标注；多 server 场景极罕见。
- WorkspaceEdit 应用用单一 offset_encoding（apply_workspace_edit 既有限制，与内置 rename 同款）。
