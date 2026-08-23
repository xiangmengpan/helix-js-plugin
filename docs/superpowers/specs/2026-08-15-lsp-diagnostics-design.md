# 设计:LSP/诊断详情访问

日期:2026-08-15
状态:草案(待审核)

## 1. 动机

插件只能拿到诊断**计数**(statusline 的 diagnostics_error/warning),拿不到内容。受益:
- 状态栏/面板显示诊断具体消息(错误文本、行号)
- 自定义符号/问题列表
- 基于诊断的过滤/跳转

## 2. API(分期)

### 2.1 一期:诊断详情(数据结构已就绪)

```js
// 当前 buffer 的诊断(按需查询)
helix.diagnostics()
// → [{line, message, severity: "error"|"warning"|"info"|"hint", code, source}]

// 监听诊断更新(debounced)
helix.on("lsp-diagnostics", (docId, diags) => { ... });
```

- 数据源:`Document::diagnostics()`(现成 `&[Diagnostic]`,含 range/line/message/severity/code/source)
- 序列化:与 buffers() 同款 JSON 通道(每帧或事件时更新)
- severity 映射:`"error"|"warning"|"info"|"hint"`(与 set_diagnostic_icons 同枚举)

### 2.2 二期(不做承诺):语法树/语义访问

- tree-sitter 查询(`:tree-sitter-subtree` 已有命令,插件化需暴露查询 API)
- LSP 符号/语义 token(依赖 LSP 事件链路)
- 本期不做,单独设计

## 3. 实现

- helix-term:序列化当前 doc 的 diagnostics(仿 serialize_buffers);`lsp-diagnostics` 事件在诊断更新时 emit(可复用现有事件泵)
- helix-js:diagnostics() 读缓存 JSON.parse;on 白名单加 "lsp-diagnostics"

## 4. 边界

- **只读快照**:诊断数组快照,修改走现有 LSP/诊断机制
- **多文档**:一期只当前 buffer;docId 参数预留(二期遍历)
- **性能**:诊断数组通常 < 100 条,JSON 序列化 µs 级;事件需 debounce(LSP 更新频繁)

## 5. 待决

1. `lsp-diagnostics` 事件是否需要(状态栏可轮询 diagnostics() 代替;事件更省)
2. 语法树访问是否要提前(二期前提是 tree-sitter 查询 API 化,工作量大)

## 6. 规模

- 一期(诊断详情):helix-term 序列化 + 事件 + helix-js 2 API + 测试(约 2 任务)
