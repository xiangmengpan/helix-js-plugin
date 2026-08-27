# 设计:doc-change 事件粒度(new/old_range 合并范围)

日期:2026-08-26
状态:草案(待审核)

## 1. 动机

现有 `doc-change` 事件(idle 防抖 + revision 前进触发)只带 doc 快照(path/text/cursor/selection),无变更范围。插件无法得知"哪段文本变了"——自动格式化/补全联动、脏区标记、diff 视图都依赖此信息。目标:事件带防抖窗口内所有变更的**合并范围**(old/new)。

## 2. 事件参数

```js
helix.on("doc-change", (doc) => {
  // doc 新增字段:
  // doc.changes: [{ oldRange: {start:{row,col}, end:{row,col}},
  //                newRange: {start:{row,col}, end:{row,col}} }]
});
```

- 窗口内多次变更合并为一条(见 3.3)
- `oldRange`:变更前文本坐标;`newRange`:变更后文本坐标
- 与 `doc.text`/`doc.cursor` 一致,0-based 行列

## 3. 实现

### 3.1 记录(helix-view/document.rs,统一入口)

`Document::apply_inner`(1671 行,所有编辑都经过它)里,从 `transaction.changes()` 取变更 range,记录:

- 每个 Change(range + text)是一个变更:old_range = change.range(旧文本坐标),new_range = change.range 经 changeset 映射后的新坐标(changeset 自身可 map;或按插入文本长度推导)
- 存储:doc 加字段 `pending_doc_changes: Vec<(Range, Range)>`(apply 时 push;apply_temporary 也记录?——temporary 是预览,不记录,只 `apply` 记录)
- 需要 doc_id 关联:helix-js 事件侧按当前 doc 取

### 3.2 事件触发(helix-term/application.rs 783,现状 idle 防抖)

- 触发条件不变(revision 前进)
- 触发时:取走当前 doc 的 `pending_doc_changes`,序列化进事件参数 `changes`
- 取走后清空(下次窗口重新累积)
- 无变更时 `changes: []`

### 3.3 合并语义

规格决策 Q2A:**合并为包围范围**。窗口内多次变更合并为一条:

- `oldRange` = 各变更 old_range 的包围范围(min start .. max end,变更发生时的旧文本坐标)
- `newRange` = 各变更 new_range 的包围范围(min start .. max end,变更后的新文本坐标)
- 窗口内只有一次变更 → 就是该变更本身的范围
- 无变更 → `changes: []`

包围范围是非严格语义(可能包含未变更的中间文本),简单可预测;插件需要逐条粒度时由引擎侧后续再扩展(本次不做)。

## 4. 边界

- **范围仅当前 buffer**:事件本身是当前 doc 的(emit_plugin_event 用 current doc),变更记录按 doc 存,跨 buffer 变更不串(当前只读当前 doc 的 pending)
- **坐标**:old_range 基于变更发生时旧文本;new_range 基于变更后文本(当前文本)。窗口内多次变更的 range 都是相对各自时刻的文本——插件对窗口语义需自行处理(文档注明:range 是各变更发生时刻的坐标,不是当前文本坐标)
- 性能:每次 apply 记一条 range,量级小(用户编辑每秒数次)

## 5. 验证

- **helix-js 单测**:序列化层——构造 changes 数组,断言 oldRange/newRange 行列格式
- **integration 测试**(plugin_docchange 或扩展现有):打字后触发 doc-change,断言 changes 含预期 old/new range;连续多次编辑断言多条
- 测试环境默认禁 LSP 不影响

## 6. 规模

- helix-view(document.rs 记录)+ helix-term(application.rs 触发序列化)+ helix-js(序列化,若在 term 侧做则免)约 1-2 任务。
