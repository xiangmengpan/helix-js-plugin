# Handoff — JS 插件引擎批次 2 完成(跨 buffer 访问,2026-08-28)

## 会话完成的工作

批次 2 完成、SDD 全流程(头脑风暴 → 规格 → 计划 → 子代理执行 + 三轮任务审查 + 最终宽范围审查)收尾,5 个 commit:

1. `eeaf4f45d` — **by_path 读取链路**:`CommandContext.docs: Vec<DocSnapshot>`(方案 B:入口显式携带)+ helix-js 内部 DOC_SNAPSHOTS 路由 + `helix.by_path(path)` 返回与 ctx.doc 同构对象(含 `_target` 属性路由编辑目标)+ `Edit.doc: Option<String>` 字段
2. `6b66e000d` — **apply_plugin_edits 按 Edit.doc 分组**:每 buffer 一个 Transaction/一次撤销;未打开目标报错入状态栏不崩
3. `3040d13eb` — **多 view panic 修复**:后台 doc 首次编辑用 `get_synced_view_id` 兜底(selections 索引缺失会崩,reviewer 发现)
4. `14e510e53` — vsplit 双 view 回归测试(判别器实证)+ 相对路径测试 + 热路径/双路由注释 + 文档
5. `613cd8a82` — **测试污染修复**:相对路径测试 `:cd` 后恢复进程 cwd(污染并行测试,filetree_enter 基线 0/2 vs 修复前 3/3,修复后 2/2)

规格:`docs/superpowers/specs/2026-08-28-js-cross-buffer-design.md`;计划:`docs/superpowers/plans/2026-08-28-js-cross-buffer.md`

## 验证状态(最终)

- helix-js 74 passed;clippy 零警告
- integration 全量:275 passed / 1 failed(`buffer_traversal_and_focus`——**既有 flaky**,基线 commit 同挂 2/2、单跑 PASS,批次 1 已记录,与本批次无关)
- 全量套件 flake 排查结论:基线本身 2/run 随机 flake(terminal hooks/modes 共享全局终端状态,plugin_async.rs ponytail 注释已文档化);本批次引入的额外 flake(相对路径测试 cwd 污染)已修复并验证(2 次全量 275/1)
- SDD 审查:任务 1/2 审查通过;3 轮修复(多 view panic / 审查建议包 / 测试污染)均定向复审 ADDRESSED;最终宽范围审查"可以合并(是)"

## 工作区状态

- 工作树:`docs/superpowers/handoff/2026-08-28-js-cross-buffer-complete.md`(本文件)未 commit;`.pi/tasks/*` 是会话临时文件**不要 commit**;`.superpowers/sdd/2026-08-28-js-cross-buffer/` 是 SDD 工作区(gitignored,含账本/简报/审查包,可留作记录或删除)
- 5 个 commit 未推送(remote: js_plugin)

## 已记录的已知边界(不阻塞,留给后续)

- `buffer_traversal_and_focus` 全量下连挂(含基线)——既有 flaky,建议后续排查其时序假设
- 相对路径测试失败路径(`?` 早退)会跳过 cwd 恢复——RAII guard 可闭合(polish,当前测试全绿)
- panel/popup 渲染回调里 by_path 读到最近命令/事件入口的快照(过期但不崩)——设计边界,已注释
- 同一命令内 ctx.doc + by_path(当前路径)双路由同 doc → 两次撤销(罕见,已注释+文档)
- 事件入口每事件全量克隆 buffer 全文 O(总字节)(已 ponytail 注释:升级路径=懒加载 path 列表)
- `:cd` 改变 cwd 后 + 相对路径参数可能失配(快照 path 在 set_path 时已 lexical canonicalize;罕见)

## 下一步候选(批次 2 之后)

用户已授权"自己选择",按 p0 候选清单剩余:**批次 3 = 装饰/标记 API**(virtual text + 区域高亮),最大,留最后。
其余候选:lsp rename/formatting/code_actions、input 多行/IME、面板节点焦点路由、LSP 请求超时。

## 新会话从这里继续

1. 流程惯例:brainstorming(规格 → docs/superpowers/specs/)→ writing-plans(计划 → docs/superpowers/plans/)→ subagent-driven-development 执行(每任务子代理 + 审查,工作区 `.superpowers/sdd/YYYY-MM-DD-<feature>/`;代理:implementer/task-reviewer,deepseek-v4-flash)
2. 测试:`cargo test -p helix-js`;`cargo test -p helix-view`;`cargo test -p helix-term --features integration --test integration`(integration 是 feature 门控;全量套件有既有 flaky,单跑验证)
3. 批次 2 关键实现位置:helix-js/src/commands.rs(js_by_path/build_doc_object/edit_target/DOC_SNAPSHOTS 入口写入)、state.rs(DOC_SNAPSHOTS)、helix-term/src/commands/typed.rs(run_plugin_command 与 emit_plugin_event_impl 收集 docs;apply_plugin_edits 分组 + get_synced_view_id)

## 关键架构事实(复述,新会话必读)

- **泵循环**:helix-term/src/application.rs 340 行附近,顺序:term/async 事件 → LSP 结果 resolve → pump_jobs → take_edits/take_messages
- **Promise 管道**:JS 侧 `JsPromise::new_pending` + `with_*_promises` map;结果经 WakeSender channel 回主线程 resolve;`pump_jobs()` 驱动 .then/await 恢复
- **register_command 不 await async fn**,boa async fn 执行到首个 await 后由 pump_jobs 驱动恢复
- **dispatch_blocking 是排队异步**:async 命令里 open_file 定位下一轮事件循环生效
- **测试环境默认禁 LSP**(helpers.rs test_editor_config `lsp.enable: false`)
- **helix-core 的 `Selection` 本身就是多选区**(ranges: SmallVec + primary_index)
- **begin_edit 事务**:helix-js/src/commands.rs js_begin_edit/js_end_edit 深度计数,take_edits 深度>0 积压;helix-term 三入口复位
- **native fn 无 ctx 参数**:JS 运行时靠 state.rs 全局路由(run_command/emit_event_impl 入口写入,与 with_engine 同款模式)
- **文档路径**:打开/保存/重命名时经 `helix_stdx::path::canonicalize`(纯词法:展开 ~、相对 join cwd、去 ./..//);进程 cwd 由 helix-stdx env.rs CWD 缓存管理,`:cd` 走 editor.set_cwd → set_current_working_dir(真 chdir + 缓存)
- **helix-view Document.selections 是 HashMap<ViewId, Selection>**:view 访问过 doc 才插入条目;首次编辑捕获 old_state 时直接索引,缺条目会 panic → 后台 doc 应用必须用 get_synced_view_id 兜底
