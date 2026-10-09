# 任务 1 报告:快照与读取(API 侧)— by_path 跨 buffer 读取

**状态:** DONE
**提交:** `eeaf4f45d` feat(js,term): by_path 跨 buffer 读取(CommandContext 携带 docs 快照,doc 对象同构 ctx.doc)
**计划:** docs/superpowers/plans/2026-08-28-js-cross-buffer.md 任务 1(9 步全部完成)

## 实现内容

| 文件 | 改动 |
|---|---|
| `helix-js/src/types.rs` | `Edit.doc: Option<String>`、`CommandContext.docs: Vec<DocSnapshot>`、新 `pub struct DocSnapshot { path, text }` |
| `helix-js/Cargo.toml` | 加 `helix-stdx = { path = "../helix-stdx" }` |
| `helix-js/src/state.rs` | 新 `DOC_SNAPSHOTS: RefCell<Vec<DocSnapshot>>` + `set_doc_snapshots`/`with_doc_snapshots`(仿 CURRENT_EDITS 模式);types import 加 DocSnapshot |
| `helix-js/src/commands.rs` | `doc_to_js` 重构出 `build_doc_object(path, text, cursor, target, engine)`(doc_to_js 变薄壳,target=None);`js_by_path`(canonicalize 匹配 → doc 对象/`_target` 带 path/未找到 null);`edit_target(this)` 从 `_target` 读目标,三个编辑函数(insert/replace/delete)填 `Edit.doc`;`run_command` 与 `emit_event_impl` 在复位 txn 深度后 `set_doc_snapshots(ctx.docs.clone())` |
| `helix-js/src/lib.rs` | builder 链注册 `by_path`(紧跟 js_end_edit 后,1 参);新增 2 个单测;4 处多行测试 ctx 构造器补 `docs: vec![]`(其余 ~30 处单行由 sed 补) |
| `helix-term/src/commands/typed.rs` | `run_plugin_command`(~4339)与 `emit_plugin_event_impl`(~4426)在 `current_ref!` 前遍历 `documents.values()` 收集 `helix_js::DocSnapshot` 填入 ctx.docs;测试 ctx 构造器(sed 补 `docs: vec![]`) |
| `helix-term/src/ui/plugin_panel.rs:106`、`plugin_popup.rs:155` | 渲染 ctx 显式 `docs: vec![]`(设计决策:每帧不序列化全文) |
| `docs/plugin-api.md` | §4 文档编辑与选区 内新增 `helix.by_path(path)` 小节(仿 begin_edit 格式,含示例/编辑路由/cursor 恒 0/快照语义/scratch 不可查) |

## TDD 证据

- **RED:** 先写 `by_path_reads_other_buffer`(helix.by_path 未注册)→ `cargo test -p helix-js by_path` FAIL:`plugin command 'read-other' failed: TypeError: not a callable function`(by_path undefined,line 4:36)。
- **GREEN:** 实现后 `cargo test -p helix-js` = **74 passed, 0 failed**(72 旧 + 2 新:by_path_reads_other_buffer / by_path_edits_target_other_buffer)。
- 编辑路由断言:`edit-other` 的编辑 `Edit.doc == Some("/tmp/other.rs")`;`ctx.doc.insert` 仍 `Edit.doc == None`。

## 验证

- `cargo build -p helix-js` ✅
- `cargo build -p helix-term` ✅(typed.rs 两入口 + 测试 ctx + panel/popup 全部编译)
- `cargo test -p helix-js` ✅ 74 passed
- 全仓 grep 确认无遗漏构造器:所有 `CommandContext {` / `Edit { start:` 均已补字段

## 自审发现

- 无阻塞问题。按计划执行:handler 抛错路径只复位队列/深度不重设快照(函数即中止,符合计划注释);`reset_plugin_state`(reload)不清 DOC_SNAPSHOTS——快照按命令生命周期覆盖,reload 后下一命令入口必然重写,无需处理(YAGNI)。
- `run_command` 未注册命令(`Ok(false)`)时快照也已写入——与计划位置一致(复位后、查 registry 前),无害。

## 关注点

- 任务 2(编辑路由:apply_plugin_edits 按 Edit.doc 分组 + 集成测试)是独立任务,未在本任务范围内;`Edit.doc` 字段目前由 term 侧 `apply_plugin_edits` 消费前静默携带(现有逻辑按当前 buffer 处理,doc=Some 的编辑行为待任务 2 定义)。
- 渲染回调里调 by_path 读到的是最近一次命令/事件入口的快照(过期但不崩)——计划标注的 `ponytail:` 边界,当前无插件在渲染里用。
