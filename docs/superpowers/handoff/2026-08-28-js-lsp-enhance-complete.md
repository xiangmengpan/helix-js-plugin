# Handoff — JS 插件引擎批次 4 完成(LSP 增强,2026-08-28)

## 会话完成的工作

批次 4(LSP 增强:rename/format/code_actions)完成:SDD 全流程(头脑风暴 → 规格 → 计划 → 执行 → 任务审查 ×2 → 终审 + 修复轮 → 复审)。5 个 commit:

1. `51b657599` — 请求侧(helix-js):`LspMethod` 扩展 4 variant、`LspRequest.params`(rename 的 newName / execute 的 action JSON)、`LspResult.apply` + `LspApply` enum(JSON 载荷)、4 个 JS 函数注册、入队前校验;term 侧 4 方法占位 resolve null
2. `67efbd778` — 应用侧(term):`handle_lsp_request` 映射 4 方法、段 A 先应用再 resolve、`lsp_text_edits_to_transaction` 纯函数(可单测)、复用 `code_actions_for_range`/`apply_workspace_edit`/`Action::lsp().execute()`、integration null 路径
3. `e032c0672` — 文档修正(rename 经 apply_workspace_edit 校验版本)
4. `b60fdb44e` — 终审修复:Format 应用用请求侧捕获的 offset_encoding(防 server 集变化错位)+ files 计数单测
5. `828100e81` — 文档失败语义精确化(协议错误 resolve null 区别于查询类 reject)

规格:`docs/superpowers/specs/2026-08-28-js-lsp-enhance-design.md`;计划:`docs/superpowers/plans/2026-08-28-js-lsp-enhance.md`

## 验证状态(最终)

- helix-js 79 passed;helix-term lsp 单测 2 passed;clippy 零警告;fmt clean
- integration plugin_lsp 3 passed(含新 enhance null 路径);全量套件未跑(本批次改动集中在 LSP 路径,无 server 环境不触发;既有 flake 情况与批次 3 相同)
- SDD 审查:任务 1/2 通过(实现者发现 async 命令错误走 promise 拒绝、简报测试需同步命令);终审"可以合并(是)"→ 修复轮 → 复审全部 ADDRESSED

## 工作区状态

- 工作树:`docs/superpowers/handoff/2026-08-28-js-lsp-enhance-complete.md`(本文件)未 commit;`.pi/tasks/*` 临时文件**不要 commit**;`.superpowers/sdd/2026-08-28-js-lsp-enhance/` SDD 工作区(gitignored)
- 5 个 commit 未推送(remote: js_plugin)

## 已记录边界(不阻塞,留给后续)

- **多 server 简化(brief 明定)**:code_actions 列表聚合全部 server,execute 固定取当前 doc 第一个 CodeAction server——多 server 项目 action 可能发到错误 server;修法:列表项带 server 标识
- **rename 的 offset_encoding 应用侧推导**(走内置 apply_workspace_edit 协商路径,与内置 rename 同款;与 Format 修复不对称,留后续)
- **测试盲区**:无 mock LSP server 基础设施——format 实际应用/rename 跨 doc/version 过期→null 无 integration 覆盖(纯函数单测覆盖转换逻辑);后续批次优先补
- **execute command 乐观 resolve**:`{applied:true}` 在 command 异步响应前兑现(与内置菜单一致)
- **block_on(resolve_code_action) 冻结主线程**(内置同款);**无超时**(LSP 请求可能悬挂,既有边界)
- **files 计数**:WorkspaceEdit 同文件多次编辑重复计入(cosmetic)
- 协议错误 → resolve null(与查询类的 reject 不同,文档已注明)

## 下一步候选(批次 4 之后)

从 p0 已知边界剩余:
- LSP 请求超时(悬挂风险)
- mock LSP server 测试基础设施(解锁 format/rename 全链路测试)
- input 多行/IME;面板节点焦点路由;LSP 装饰完善(按行索引性能)

## 新会话从这里继续

1. 流程惯例不变:brainstorming → writing-plans → subagent-driven-development
2. **子代理执行纪律**(批次 3-4 教训):提示词禁环境探索/禁 pi_fold_context;长 cargo 命令(integration 编译+运行 >10min)由控制者跑;集成测试断言绑定触发键同一步 + DOC_CHANGE_TEST_LOCK
3. 测试:`cargo test -p helix-js`;`cargo test -p helix-term lsp_text_edits_to_transaction`(纯函数);`cargo test -p helix-term --features integration --test integration plugin_lsp`;收尾 `cargo fmt --all --check` + clippy
4. 批次 4 关键实现位置:helix-js/src/lsp.rs(LspMethod/enqueue/LspApply)、helix-term/src/application.rs(handle_lsp_request 四分支/handle_lsp_format|rename|code_actions|execute、apply_lsp_edits、lsp_text_edits_to_transaction、workspace_edit_file_count、tests mod)、commands/lsp.rs(code_actions_for_range 复用)

## 关键架构事实(复述,新会话必读)

- **泵循环 LSP 段**:段 A(application.rs:384)resolve LSP 响应(先 apply 再 resolve);段 B(385)发 LSP 请求;两者都在 pump_jobs 之前
- **LSP Promise 管道**:enqueue_lsp_request 入队 + promise;tokio 任务发请求 → LSP_RESULTS 通道 → 段 A resolve_lsp;**编辑应用载荷(LspApply)走同一通道回主线程,JSON 纯数据**
- **LspResult { id, result, apply }**:result = JSON 字符串/null/错误;apply = Some(载荷)时段 A 先应用(成功覆写 result 为摘要,失败 log + null)
- **native fn 无 ctx 参数**:state.rs 全局路由;promise 经 with_lsp_promises map + WakeSender
- **async 命令错误走 promise 拒绝**(非同步 throw)——单测校验用同步命令
- **LSP 编辑应用**:Format 用请求侧捕获的 offset_encoding(u8 判别值 0=Utf8/1=Utf32/2=Utf16)+ get_synced_view_id;Rename 走 apply_workspace_edit(校验 version);Execute 走 Action::lsp().execute()
- **测试环境默认禁 LSP**(helpers.rs lsp.enable: false)——无 server 路径 resolve null;无 mock server 基础设施
- **既有 flake**:buffer_traversal_and_focus 等 terminal hooks/modes 测试全量下随机挂(基线同挂,单跑 PASS)
