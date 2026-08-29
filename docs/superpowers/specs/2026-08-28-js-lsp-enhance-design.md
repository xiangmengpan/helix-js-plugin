# 设计:LSP 增强 API(rename / format / code_actions)

日期:2026-08-28
状态:已批准(brainstorming 三轮问答 + 三节设计确认)

关联:docs/superpowers/specs/2026-08-25-js-promise-design.md(Promise 管道)、docs/superpowers/specs/2026-08-26-js-cross-buffer-design.md(跨 buffer 编辑语义)

## 1. 动机

插件 LSP API 只有 4 个**只读查询**(hover/completion/goto_definition/document_symbols,响应为原始 LSP JSON),缺 rename/formatting/code_actions 三类**会改变文档**的操作。P0 已知边界原文:「LSP 只读 diagnostics + 4 方法,无 rename/formatting/code_actions」。

## 2. 设计

### 2.1 API

```js
// 格式化当前 buffer(全文档):自动应用,一次撤销
const r = await helix.lsp.format();
// → { applied: true } 或 null(无 server/不支持/失败)

// 重命名当前符号:自动应用 workspace edits(可跨 buffer,每 buffer 一次撤销)
const r = await helix.lsp.rename("newName");
// → { applied: true, files: 2 } 或 null

// 列出光标处可用 code actions(pos 可选,复用现有 pos 对象形态 { row, col })
const actions = await helix.lsp.code_actions();
// → [{ title, kind, ...完整 LSP action }] 或 null(无 server / 无 actions)

// 执行选中的 action(把列表里拿到的对象原样传回;无状态两阶段,不跨帧缓存)
const r = await helix.lsp.execute_code_action(action);
// → { applied: true } 或 null
```

### 2.2 语义

- **参数校验**:rename 的 newName 非字符串 → TypeError;execute_code_action 非对象 → TypeError;code_actions/format 可选 pos(现有 enqueue 的 pos 解析复用)
- **自动应用**:format 的 TextEdit → Transaction(`doc.apply` + history,一次撤销,与内置 format 一致);rename 的 WorkspaceEdit 走 `editor.apply_workspace_edit`(内置路径,跨 buffer,每 buffer 一次撤销,与批次 2 语义一致);execute_code_action 走内置执行辅助(edit → apply / command → 分发)
- **无 server / 能力不支持 / 请求失败 → resolve null**(promise 不悬空,与现有 4 方法一致)
- **应用时机**:响应到达时在 resolve promise **之前**主线程应用——promise resolve 时编辑已生效
- format 仅全文档(不做 range;内置 format_selections 已覆盖 range 需求)

### 2.3 数据流

**请求侧(helix-js/src/lsp.rs)**:
- `LspMethod` 扩展:`Rename { new_name: String }` / `Format` / `CodeActions` / `ExecuteCodeAction`
- `enqueue_lsp_request` 按 method 解析参数(rename 校验并携带 new_name;execute_code_action 校验对象并携带 action JSON;code_actions/format 无参,可选 pos)
- promise 管道不变(`with_lsp_promises` + WakeSender 回程)

**响应回程(应用插在主线程)**:
- tokio 任务拿到的响应是纯数据(TextEdit / WorkspaceEdit / action JSON)——可跨线程经 LSP_RESULTS 通道回主线程
- `LspResult` 扩展携带 `apply: Option<LspApply>`(Format 的 doc_id + TextEdit 列表 / Rename 的 WorkspaceEdit / Execute 的 action JSON);泵循环 drain 时:有 apply 数据 → 先主线程应用 → 再 `resolve_lsp` 兑现摘要
- 无 server/不支持/失败 → 现有路径 resolve null

**应用细节**:
- format:提取纯函数 `lsp_text_edits_to_transaction(text, edits, offset_encoding) -> Transaction`(helix-lsp 既有 util 复用)→ `doc.apply` + `append_changes_to_history`
- rename:响应 WorkspaceEdit → `editor.apply_workspace_edit`(内置:跨 buffer、自动打开未打开文件、每 buffer 一次撤销)
- execute_code_action:action JSON 重构 `lsp::CodeActionOrCommand` → 复用从 `CodeActionItem::execute`(commands/lsp.rs)提取的公共执行辅助
- 摘要:`{ applied: true }`(format/execute)、`{ applied: true, files: n }`(rename)

### 2.4 边界

- **无超时**:既有已知边界(LSP 请求无超时,promise 可能悬挂)——不新增,文档注明
- **format 仅全文档**:range 由内置 format_selections 覆盖
- **不检查 doc version**:响应到达即应用(比内置宽松,插件用 await 时序自行控制;文档注明)
- **rename 自动打开未打开文件**:复用 apply_workspace_edit 内置行为
- **execute 的 command 分发**:复用内置路径(与内置 code_action 行为一致)

## 3. 验证

### 3.1 helix-js 单测

- rename newName 非字符串 → TypeError;execute_code_action 非对象 → TypeError
- 四种方法 enqueue 的请求形态(method/params)正确

### 3.2 helix-term 单测(提取的纯函数,无 server 也可测)

1. `lsp_text_edits_to_transaction`:合成 TextEdit JSON(UTF-16 坐标、多行替换、插入)→ 断言 Transaction 的 changes 与文本结果
2. code_actions 列表序列化:合成 CodeAction JSON → 断言输出字段(title/kind)

### 3.3 integration(无 server null 路径)

- 4 个新方法在无 LSP 环境 resolve null(async 续体 echo 断言,plugin_lsp.rs 同款)

## 4. 规模

- helix-js:lsp.rs(LspMethod 扩展/enqueue/4 个 JS 函数)、lib.rs(注册)
- helix-term:application.rs(handle_lsp_request 映射 + LspResult.apply + drain 应用)、commands/lsp.rs(提取 CodeActionItem::execute 公共辅助)、新纯函数 + 单测
- 测试:helix-js 单测 + helix-term 单测 + integration(plugin_lsp 扩展)
- 约 2 任务:① 请求侧(helix-js:方法/参数/注册 + 单测)② 响应应用侧(term:handle 映射/应用/摘要 + 单测 + integration)
