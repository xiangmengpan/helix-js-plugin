# 设计:装饰/标记 API(set_virtual_text / set_highlight)

日期:2026-08-28
状态:已批准(brainstorming 四轮问答 + 三节设计确认)

关联:docs/superpowers/specs/2026-08-28-js-cross-buffer-design.md(路径匹配/队列模式复用)、docs/superpowers/specs/2026-08-26-js-batch-edit-design.md(txn 门控复用)

## 1. 动机

插件只能改文本/光标/选区,不能**装饰视图**:virtual text(行内注释、类型标注)与区域高亮(搜索结果、diff 视图)都缺失。原始需求(P0 候选 #3):「装饰/标记 API:virtual text + 区域高亮(搜索结果、diff 视图)」。

## 2. 设计

### 2.1 API

```js
// virtual text:在 (row, col) 处插入行内文本(不占用真实字符位置,同 inlay hint)
helix.set_virtual_text(path, row, col, text, style);   // style: 主题 scope 字符串或 null/undefined

// 区域高亮:半开区间 [start, end) 的行列范围
helix.set_highlight(path, sr, sc, er, ec, style);

// 清除该 doc 全部插件装饰(同时清 virtual text 与高亮)= 只传 path,text 省略
helix.set_virtual_text(path);
```

- 注册为 `helix.set_virtual_text`(5 参)/ `helix.set_highlight`(7 参),命令与事件入口可用
- 路径匹配复用批次 2 规则:参数经 `helix_stdx::path::canonicalize`(push 时),与 `Edit.doc` 同款;未打开 → 应用时静默忽略
- 参数校验:path/row/col/text/style 类型错误 → TypeError(与现有 API 惯例)

### 2.2 语义

- **按 doc 整体替换**:命令/事件执行期间多次调用累积;应用时按 doc 整体替换该 doc 的插件装饰集;推空集 = 清除
- 生命周期:下次替换 / doc 关闭 / 插件重载(清空全部 doc)
- 坐标基准:命令开始时快照(与编辑一致);**不随后续事务重映射**——插件监听 `doc-change` 重推(替换语义保证无残留)
- 与 begin_edit 事务解耦?否——**镜像编辑**:装饰请求入独立队列,但 take 点/txn 门控/复位点与 take_edits 完全同构(async 插件用 begin_edit/end_edit 统一 hold 到结束)

### 2.3 数据流

**helix-js(state.rs + commands.rs + lib.rs)**:
- `DECORATION_REQUESTS: RefCell<Vec<DecorationRequest>>`

```rust
pub enum DecorationKind {
    VirtualText { row: usize, col: usize, text: String, style: Option<String> },
    Highlight { sr: usize, sc: usize, er: usize, ec: usize, style: Option<String> },
    /// 清除该 doc 全部插件装饰(set_virtual_text 只传 path 时产生)
    Clear,
}
pub struct DecorationRequest { pub doc: Option<String>, pub kind: DecorationKind }
```

- `js_set_virtual_text` 的 text 参数省略/undefined → push `Clear`;否则 push VirtualText。`js_set_highlight` 全参必填
- 应用时按 doc 分组后**按队列内顺序处理**:Clear → 置空该 doc 集合;VirtualText/Highlight → 追加(累积后整体替换)

- `take_decorations()` 与 take_edits 同构:txn 深度>0 → 返回空(积压);否则 drain。复位点(run_command/emit_event_impl/错误路径)同步清空
- `js_set_virtual_text` / `js_set_highlight` push 请求(path canonicalize 后)

**helix-term(typed.rs + application.rs)**:
- `apply_plugin_decorations(editor, reqs)`:按 doc 分组 → 按 path 查 doc(查不到忽略)→ 该 doc 插件装饰集**整体替换**;在 apply_plugin_edits 三调用点(命令结束 run_plugin_command / 事件结束 emit_plugin_event_impl / 泵循环 application.rs)旁各加一处
- 插件重载路径:清空全部 doc 的 plugin_decorations(与复位 txn 深度同一处)

**helix-view(document.rs)**:

```rust
#[derive(Debug, Clone, Default)]
pub struct PluginDecorations {
    pub virtual_text: Vec<PluginInlineAnnotation>,  // { char_idx: usize, text: Tendril, style: Option<String> }
    pub highlights: Vec<PluginHighlight>,           // { start: usize, end: usize, style: Option<String> }
}
// Document 加字段 pub plugin_decorations: PluginDecorations(坐标 = 应用时 char 索引,pos_to_char 换算)
```

### 2.4 渲染注入

- **virtual text**:`view.text_annotations`(view.rs:458)inlay hints 段后加一段——按 style scope 分组,逐组 `add_inline_annotations`(theme.find_highlight 解析;None → 默认)
- **区域高亮**:`ui/editor.rs` 渲染的 `overlays: Vec<OverlayHighlights>` 内建 overlay 之后 push 插件高亮(按 style 分组)
- 装饰按 doc 存储 → 该 doc 所有 view 共享(与 LSP inlay 的 per-view 不同)

### 2.5 边界

- 坐标:应用时按该 doc 当时文本换算,不随后续事务重映射(2.2)
- scratch buffer:字符串 path 寻址天然排除;插件用 ctx.doc.path 取当前路径
- 样式解析失败 → 回退默认
- 重叠:同位置 virtual text 忽略后续(TextAnnotations 既有语义);overlay 后画者在上
- 与 LSP inlay hints 共存:独立槽位,互不覆盖,LSP idle 重算不受影响
- 性能:集合无硬上限,渲染按可见行裁剪;`ponytail: 大装饰集每帧全量遍历,需要时按行索引`

## 3. 验证

### 3.1 helix-js 单测

- 参数校验(TypeError:path/text/数字类型错误)
- push 后 take_decorations 取到请求(字段正确)
- txn 门控:begin 后取空,end 后取到
- 复位:命令开始清空上一命令残留

### 3.2 integration(新 plugin_decorations.rs,白盒断言 doc.plugin_decorations)

1. 命令里 set_virtual_text/set_highlight 当前 doc → 断言字段内容与 char 坐标
2. 替换语义:两次命令第二次替换第一次(无残留)
3. 推空集 → 清空
4. 未打开 path → 不崩、不写入
5. doc-change 监听重推 → 断言替换后状态
6. async + begin_edit:装饰与编辑同 hold,结束一起应用

## 4. 规模

- helix-js:state.rs(队列)、commands.rs(两函数 + take/reset)、lib.rs(注册 + 测试构造器)
- helix-term:typed.rs(apply_plugin_decorations + 三调用点 + 重载清空)、application.rs(泵循环一处)
- helix-view:document.rs(PluginDecorations 字段)
- 测试:helix-js 单测 + integration(plugin_decorations.rs)
- 约 2 任务:① 队列与 API(helix-js 全链路 + 单测)② 应用/存储/渲染(term + view + integration)
