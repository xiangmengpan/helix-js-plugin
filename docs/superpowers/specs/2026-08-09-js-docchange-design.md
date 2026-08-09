# 设计：doc-change 事件（特性 C，串行第二，需防抖）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

`helix.on("doc-change", (doc) => ...)`——文档文本变化时触发，带防抖。

## 防抖机制（关键设计）

**用 helix 既有的 idle 定时器做防抖，零新增计时器**：
- helix 的 idle 定时器（`Editor::idle_timeout`，默认 250ms）在事件流安静 250ms 后触发 `handle_idle_timeout`
- 变化检测：`doc.history.get().current_revision()`（每次事务应用自增）
- 挂点：`handle_idle_timeout`（application.rs:621）——若当前文档 revision 与上次发射时不同 → emit "doc-change" → 记录 revision
- 连续输入时 idle 不触发（有事件在跑），停止输入 250ms 后 idle 触发一次 → 天然防抖：一次 doc-change = 一次编辑停顿后

## 新增事件

- `EVENT_WHITELIST` 加 `"doc-change"`（helix-js）
- 语义：当前文档文本变化（自上次发射后 revision 前进）；ctx 与其他事件同构（可编辑——doc-change 处理器改文档会再触发下一轮，需在规格注明递归风险与防护：emit 后立即更新 last_revision，处理器编辑导致的再次变化会作为"下一次变化"再触发——接受，不做环形防护，PoC 注明）

## 实现

### helix-js

- `EVENT_WHITELIST` 加 `"doc-change"`（一行）

### helix-term（application.rs）

- `handle_idle_timeout` 开头追加（emit_plugin_event 已在 typed.rs，pub(crate)）：
  - 若 `!helix_js::has_handlers("doc-change")` → 跳过
  - 取当前文档 revision，与 `last_plugin_doc_change_revision: Cell<Option<usize>>`（application 字段或全局）比较
  - 不同 → `emit_plugin_event(&mut self.editor, "doc-change", None)` + 更新 last_revision

## 测试

- **helix-js 单测**：`helix.on("doc-change", ...)` 注册成功（白名单通过）；emit_event("doc-change") 正常触发
- **集成测试**（tests/test/plugin_docchange.rs）：插件注册 doc-change 处理器 echo "changed" → 输入文本（如 `i` 插入字符 `<esc>`）→ 等待 idle → 断言状态栏 "changed"。若 idle 时序在测试 harness 里不稳定，在 :plugin-load 后加一次无害按键事件确保 idle 重排，报告中注明实测行为

## 非目标

- 多文档（非当前文档的变化不触发——当前文档快照语义）
- 递归防护（处理器编辑再次触发——接受并注明）
- 更细粒度的事件载荷（如变化范围/类型）

## 涉及文件

- `helix-js/src/lib.rs`（白名单 + 单测）
- `helix-term/src/application.rs`（handle_idle_timeout 挂点 + last_revision 状态）
- `helix-term/tests/test/plugin_docchange.rs`（新）
