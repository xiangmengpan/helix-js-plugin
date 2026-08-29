# 设计:completion 增强(snippet 源 + 匹配高亮 + 行渲染钩子)

日期:2026-08-29
状态:已批准(分节讨论确认)

## 背景与目标

原生 completion 已有 nucleo fzf 式模糊匹配、`set_completion_icon` kind 图标钩子(2026-08-28+)。与 nvim-cmp 相比剩余差距:

1. **无自定义 snippet 源**——只有 LSP 返回的 snippet 候选,没有 friendly-snippets 式本地片段库
2. **无匹配高亮**——nucleo 算出评分但匹配区间未呈现到 UI
3. **候选行结构固定**——`Row([label, kind_cell])` 两列,无 source 标签等定制

本设计三块独立落地,顺序 1 → 2 → 3(数据 → 渲染 → 定制)。

## 节 1:snippet 源(CompletionProvider::Snippet)

### 1.1 数据来源

- 补全触发时扫描 `~/.config/helix/snippets/<lang>.json`(lang = 当前文档 language id)
- 找不到 `<lang>.json` 则试 `all.json`(friendly-snippets 惯例)
- 格式(friendly-snippets 兼容):

```json
{
  "fn": {
    "prefix": "fn",
    "body": ["function ${1:name}(${2:params}) {", "\t${0}", "}"],
    "description": "Function declaration"
  }
}
```

- `prefix` 可为字符串或字符串数组(多前缀),`body` 为字符串数组(逐行,渲染时 join "\n"),`description` 可选
- 反序列化:serde_json,容错——单个 snippet 解析失败跳过并 log,不崩整个文件

### 1.2 候选构建

- `helix-core/src/completion.rs`:
  - `CompletionProvider` 加 `Snippet` variant
  - `CompletionItem` 加 `Snippet { label, body, description, provider }` variant(label = 首 prefix,body = join 后的原始文本)
- 补全 handler(helix-term):触发时读文件 → 构建 `CompletionItem::Snippet` 列表 → 与 LSP/Word 候选合并进 `active_completions`
- **触发语义**:与现有补全入口一致(insert 模式自动触发 + 手动 `:completion`);触发条件与 Word 源相同——文档有 language id 且存在对应 snippet 文件即触发,不依赖 LSP server 是否启用
- 匹配:label 走 nucleo 模糊匹配(与 LSP 同款 `filter_text()`)

### 1.3 accept 展开

- `CompletionItem::Snippet` 的 accept 路径复用现有 LSP snippet 展开逻辑:
  `generate_transaction_from_snippet`(completion.rs 纯函数)→ (transaction, Some(snippet)) → `ActiveSnippet::new(snippet)`,占位符 tab 跳转与 LSP snippet 完全同款
- 展开前先删掉已输入的 prefix 字符(触发时光标前的单词),与 LSP 行为一致

### 1.4 排序

- `provider_priority()` 加分支:Snippet 排在 Lsp 之后、Word 之前(默认值,可调)

### 1.5 边界

- 无 LSP server 时 snippet 独立工作
- 无 snippet 文件 → 无候选,零开销
- 大文件(几千 snippet):读取+解析在补全触发时一次性,不进热路径

## 节 2:匹配高亮

- `score()` 中用 `pattern.indices(...)`(picker.rs:774 上游先例)获取匹配区间,随 match 项存储(与 score 同生命周期)
- 渲染 label 时按区间分段,匹配段加样式
- 新 theme key:`ui.completion.match`(默认 `underline`;主题可覆盖,缺省时回退无样式)
- 纯 Rust,LSP/Word/Snippet 全候选受益;不注册任何 JS

### 边界

- nucleo 返回的区间是 char index,渲染按 char 分段(现有 StyledLine 模型一致)
- 匹配区间在增量过滤时随 score 一起更新

## 节 3:L2 行渲染钩子(set_completion_render)

### 3.1 API

```js
// fn(ctx) => [{ type: "text", text, style }] | null
helix.set_completion_render((ctx) => {
  // ctx: { label, kind, kindNum, provider, detail, deprecated, matchIndices }
  return [
    { type: "text", text: ctx.label, style: "ui.completion" },
    { type: "text", text: " " + ctx.provider, style: "ui.statusline.inactive" },
  ];
});
// 返回 null 或未注册 → 原生两列回退
```

- ctx 字段:label(字符串)、kind(文本,如 "method")、kindNum(1-25,0=非 LSP)、provider("lsp"|"word"|"snippet")、detail(LSP detail,可选)、deprecated(bool)、matchIndices(匹配区间数组)
- 存储/调用机制照抄 `set_component_render`(helix-js popup.rs 现有 hook 模式:注册存闭包、Rust 侧调用)
- 调用频率:仅可见行(menu 视口内,~20 行),每帧渲染时

### 3.2 benchmark(本批次内)

- 临时钩子:JS 返回 3 列 cells,数一帧渲染耗时(可见行 × JS 调用)
- 数据阈值:若明显低于 60fps 预算(>8ms/帧),行钩子保留但文档注明性能上限;若不可接受,降级为"仅 Rust 三列(provider 列)"不做 JS 定制

### 3.3 边界

- JS 返回空数组 → 空行
- JS 抛错 → 回退原生两列(与 set_completion_icon 抛错回退语义一致)
- 钩子注册/热重载:与 set_component_render 同生命周期

## 测试策略

- **helix-js 单测**:set_completion_render 注册/读取/抛错回退(照 set_component_render 测试模式)
- **helix-core/term 单测**:
  - snippet JSON 反序列化(合法/坏文件/多 prefix/缺 description)
  - `CompletionItem::Snippet` accept 生成 transaction + ActiveSnippet(复用现有 snippet 单测模式)
  - provider_priority 排序顺序
  - 高亮:indices 分段渲染输出
- **integration**:补全触发含 snippet 候选(参考现有 plugin_lsp integration 模式)

## 不做的(明确排除)

- path 源(cmp-path)——场景窄,用户选定不做
- JS 注入 API(add_provider 通用通道)——只有 snippet 一个消费者,YAGNI;未来有动态源再加
- 行为层定制(确认键/触发阈值)——Rust 键位路由,性价比最低
- 多行输入/IME——独立主题

## 关键实现位置

- helix-core/src/completion.rs(CompletionProvider::Snippet、CompletionItem::Snippet)
- helix-core/src/snippets/(已存在,复用)
- helix-term/src/handlers/completion/(读取+合并 handler)
- helix-term/src/commands.rs(completion 触发入口)
- helix-term/src/ui/completion.rs(accept 展开、高亮渲染、行渲染钩子调用)
- helix-js/src/popup.rs(set_completion_render 注册/读取,照 set_component_render)
- helix-js/src/lib.rs(native fn 注册表)
- runtime/themes/(ui.completion.match 主题 key)
