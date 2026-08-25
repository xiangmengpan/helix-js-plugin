# JS UI 布局增强实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** 给组件树布局补齐三样能力——scroll `offset` 虚拟化(只产出视口行)、Row/Col `flex` 弹性占比(两遍布局)、Text `wrap` 换行(不截断)。现有插件零改动(全部新属性可选)。

**架构:** 改动三个 crate 内文件 + 一个插件:`types.rs` 扩展 CompNode 字段(破坏性,需修全部构造点)、`popup.rs` js_el/parse_node 透传新 opts、`comp_layout.rs` 布局算法(measure→allocate 两遍、wrap 切行、scroll 快/慢双路径)、filetree 迁移用 scroll offset。布局器保持无状态纯函数。

**技术栈:** 无新依赖。宽度语义沿用现有 `chars().count()`(CJK 按字符数,与现状截断一致)。boa API 不涉及。

**规格:** `docs/superpowers/specs/2026-08-25-js-ui-layout-design.md`(已 commit)

**验证命令:**
```bash
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-js --lib
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-term --lib
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo clippy -p helix-term -p helix-js --all-targets
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo build --release
```

---

### 任务 1:CompNode 字段扩展 + el/parse_node 解析

**文件:**
- 修改:`helix-js/src/types.rs:217-226`(CompNode 枚举)
- 修改:`helix-js/src/popup.rs`(js_el ~436-530、parse_node ~646-746)
- 修改:`helix-js/src/lib.rs`(测试构造点 + 新增解析单测)
- 修改:`helix-term/src/ui/comp_layout.rs`(测试构造点;主分支用 `..` 的不用改)

- [ ] **步骤 1:扩展 CompNode**

`helix-js/src/types.rs`:

```rust
pub enum CompNode {
    Text { spans: Vec<TextSpan>, width: Option<u16>, id: Option<String>, flex: Option<u16>, wrap: bool },
    Row { children: Vec<CompNode>, gap: u16, flex: Option<u16> },
    Col { children: Vec<CompNode>, gap: u16, flex: Option<u16> },
    Scroll { children: Vec<CompNode>, height: u16, offset: Option<u16> },
    Button { label: Vec<TextSpan>, width: Option<u16>, id: String, flex: Option<u16> },
    Input { value: String, width: Option<u16>, id: String, flex: Option<u16> },
}
```

同步更新枚举上方 doc 注释(补 flex/wrap/offset 一句话说明)。

- [ ] **步骤 2:修全部构造点(编译错误驱动)**

grep `CompNode::Text {` / `Row {` / `Col {` / `Scroll {` / `Button {` / `Input {`,给所有显式构造补新字段:
- `helix-term/src/ui/comp_layout.rs`:211,215,257,258(text helper 与测试)+ Row/Col/Scroll 构造点
- `helix-js/src/lib.rs`:测试断言用 `..` 的不用改,显式构造的补字段
- `helix-js/src/popup.rs`:parse_node(下一步重写)
- `helix-js/src/commands.rs:869` 用 `CompNode::Text { .. }` 模式匹配,不用改

先跑 `HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo build -p helix-js -p helix-term` 让编译器列出全部遗漏点,逐个补。

- [ ] **步骤 3:js_el 透传新 opts**

`popup.rs` js_el:
- `"text"` 分支 opts 段(现有 style/width 之后)加:

```rust
if let Some(flex) = obj_opt_u16(&obj, "flex", ctx, api)? {
    props.push(("flex".into(), JsValue::from(flex)));
}
if let Some(wrap) = obj_opt_bool(&obj, "wrap", ctx, api)? {
    props.push(("wrap".into(), JsValue::from(wrap)));
}
```

(`obj_opt_bool` 若不存在,用 `obj.get` + `try_js_into::<bool>` 包一层,仿 obj_opt_u16;类型错报 `"wrap expects a boolean"`)

- `"row" | "col" | "scroll"` 分支:现有只读一个 key(gap/height),改为同时读 flex(row/col)或 offset(scroll):

```rust
let is_scroll = type_ == "scroll";
if let Some(v) = obj_opt_u16(&obj, if is_scroll { "height" } else { "gap" }, ctx, api)? {
    let key: &str = if is_scroll { "height" } else { "gap" };
    props.push((key.into(), JsValue::from(v)));
}
if !is_scroll {
    if let Some(flex) = obj_opt_u16(&obj, "flex", ctx, api)? {
        props.push(("flex".into(), JsValue::from(flex)));
    }
} else if let Some(offset) = obj_opt_u16(&obj, "offset", ctx, api)? {
    props.push(("offset".into(), JsValue::from(offset)));
}
```

- `"button"` 分支:opts 遍历数组 `["onPress", "onKey", "style", "width"]` 改为 `["onPress", "onKey", "style", "width", "flex"]`
- `"input"` 分支:同样加 `"flex"` 到遍历数组(先看现有实现,若无遍历数组则照 text 分支的样式加)

- [ ] **步骤 4:parse_node 读新字段**

`popup.rs` parse_node(~646):
- `"text"` 分支(现有读取 width 处)加:

```rust
let flex = obj_opt_u16(&obj, "flex", ctx, api)?;
let wrap = obj_opt_bool(&obj, "wrap", ctx, api)?.unwrap_or(false);
Ok(CompNode::Text { spans, width, id: node_id, flex, wrap })
```

- `"row" | "col"` 分支:读 flex,补进 `CompNode::Row { children, gap, flex }`
- `"scroll"` 分支:读 offset,补进 `CompNode::Scroll { children, height, offset }`
- `"button"` / `"input"` 分支:读 flex

- [ ] **步骤 5:写解析单测 + 跑绿**

`helix-js/src/lib.rs` tests 模块新增(用现有 el → parse_node 路径或直接构造对象走 parse;看现有测试怎么调 parse_node——`popup.rs` 有测试则加那里):

```rust
#[test]
fn el_new_opts_parse() {
    // el("scroll", [...], {height, offset}) → Scroll { offset: Some(n) }
    // el("text", "x", {flex: 2, wrap: true}) → Text { flex: Some(2), wrap: true }
    // 缺省: Text { flex: None, wrap: false } / Scroll { offset: None }
    // 类型错误: flex: "x" → Err
}
```

跑:`HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-js --lib`,再跑 `cargo test -p helix-term --lib`(comp_layout 现有测试是构造点修复的回归网)。

- [ ] **步骤 6:Commit**

```bash
git add helix-js/src/types.rs helix-js/src/popup.rs helix-js/src/lib.rs helix-term/src/ui/comp_layout.rs
git commit -m "feat: CompNode 增加 flex/wrap/offset 字段与 el 解析"
```

---

### 任务 2:flex 两遍布局 + text wrap(comp_layout.rs)

**文件:**
- 修改:`helix-term/src/ui/comp_layout.rs`(layout 主函数 + 辅助函数 + 测试)

- [ ] **步骤 1:写失败测试**

`comp_layout.rs` tests 模块新增:

```rust
#[test]
fn row_flex_allocates_ratio() {
    // viewport (100, 10); flex:1 + flex:2 → 两列按 1:2 分(减 1 个 gap)
    let node = CompNode::Row {
        children: vec![
            CompNode::Text { spans: vec![TextSpan { text: "a".into(), style: None }], width: None, id: None, flex: Some(1), wrap: false },
            CompNode::Text { spans: vec![TextSpan { text: "b".into(), style: None }], width: None, id: None, flex: Some(2), wrap: false },
        ],
        gap: 1,
        flex: None,
    };
    let lines = layout(&node, (100, 10));
    let text = lines[0].spans.iter().map(|s| s.text.as_str()).collect::<String>();
    // 内容 "a" + 空格 + "b" + 空格填充: 总宽 100, a 列 33, b 列 66(减 gap 1)
    assert_eq!(text.chars().count(), 100);
    let a = text.find('a').unwrap();
    let b = text.find('b').unwrap();
    assert!(b - a > 30 && b - a < 36, "1:2 比例: b 距 a {}(期望 ~33)", b - a);
}

#[test]
fn row_flex_zero_keeps_content_width() {
    // flex:0(缺省) + 固定 width:8 → 总宽 = 内容 + 8 + gap
    let node = CompNode::Row {
        children: vec![
            CompNode::Text { spans: vec![TextSpan { text: "hello".into(), style: None }], width: None, id: None, flex: None, wrap: false },
            CompNode::Text { spans: vec![TextSpan { text: "size".into(), style: None }], width: Some(8), id: None, flex: None, wrap: false },
        ],
        gap: 1,
        flex: None,
    };
    let lines = layout(&node, (100, 10));
    let text = lines[0].spans.iter().map(|s| s.text.as_str()).collect::<String>();
    assert_eq!(text.chars().count(), 5 + 1 + 8);
}

#[test]
fn text_wrap_multiline() {
    // wrap=true, width=5: "abcdefghij" → 两行 "abcde" / "fghij"
    let node = CompNode::Text { spans: vec![TextSpan { text: "abcdefghij".into(), style: None }], width: Some(5), id: None, flex: None, wrap: true };
    let lines = layout(&node, (5, 10));
    assert_eq!(lines.len(), 2);
    let joined = lines.iter().map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>()).collect::<Vec<_>>();
    assert_eq!(joined, vec!["abcde", "fghij"]);
}

#[test]
fn text_wrap_cjk_keeps_chars() {
    // 中文 6 字符 width=4 → 两行,不切坏字符
    let node = CompNode::Text { spans: vec![TextSpan { text: "中文测试文本".into(), style: None }], width: Some(4), id: None, flex: None, wrap: true };
    let lines = layout(&node, (4, 10));
    let joined: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.text.as_str())).collect();
    assert_eq!(joined, "中文测试文本");
    assert!(lines.iter().all(|l| l.width() <= 4));
}

#[test]
fn text_wrap_false_truncates() {
    // 回归: 无 wrap → 现状截断
    let node = CompNode::Text { spans: vec![TextSpan { text: "hello world".into(), style: None }], width: Some(5), id: None, flex: None, wrap: false };
    assert_eq!(layout(&node, (100, 10)).len(), 1);
}
```

- [ ] **步骤 2:运行确认失败**

运行:`HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-term --lib comp_layout`
预期:新测试 FAIL(现有实现无 flex/wrap 行为,`text_wrap_false_truncates` 会过)。

- [ ] **步骤 3:实现**

`comp_layout.rs` 新增辅助:

```rust
fn flex_of(node: &CompNode) -> Option<u16> {
    match node {
        CompNode::Text { flex, .. } | CompNode::Row { flex, .. } | CompNode::Col { flex, .. }
        | CompNode::Button { flex, .. } | CompNode::Input { flex, .. } => *flex,
        CompNode::Scroll { .. } => None,
    }
}

/// 内容宽度(测量用,不做完整布局)。宽度语义与 StyledLine::width 一致(chars 数)。
fn content_width(node: &CompNode) -> usize {
    match node {
        CompNode::Text { spans, .. } => spans.iter().map(|s| s.text.chars().count()).sum(),
        CompNode::Row { children, gap, .. } => {
            let g = children.len().saturating_sub(1) * *gap as usize;
            children.iter().map(content_width).sum::<usize>() + g
        }
        CompNode::Col { children, .. } => children.iter().map(content_width).max().unwrap_or(0),
        CompNode::Button { label, .. } => label.iter().map(|s| s.text.chars().count()).sum::<usize>() + 4,
        CompNode::Input { value, .. } => value.chars().count(),
        CompNode::Scroll { .. } => 0,
    }
}
```

`Text` 分支改造(替换现有截断逻辑;样式随字符切行):

```rust
CompNode::Text { spans, width, wrap, .. } => {
    let limit = width.unwrap_or(viewport.0).min(viewport.0) as usize;
    if *wrap && spans.iter().map(|s| s.text.chars().count()).sum::<usize>() > limit && limit > 0 {
        // (char, style) 流按 limit 切行;每行合并相邻同 style 段
        let stream: Vec<(char, Option<String>)> = spans.iter()
            .flat_map(|s| s.text.chars().map(|c| (c, s.style.clone())))
            .collect();
        let mut lines: Vec<StyledLine> = Vec::new();
        let mut cur: Vec<(char, Option<String>)> = Vec::new();
        for item in stream {
            if cur.len() == limit {
                lines.push(StyledLine { spans: merge_spans(&cur) });
                cur.clear();
            }
            cur.push(item);
        }
        if !cur.is_empty() {
            lines.push(StyledLine { spans: merge_spans(&cur) });
        }
        lines
    } else {
        // 现有截断逻辑保持不动(逐 span 截断到 limit)
    }
}
```

`merge_spans` 辅助:相邻相同 style 的 (char, style) 合并成 TextSpan。

`Row` 分支改造为两遍(现有逻辑保留为 total_flex==0 的快路径):

```rust
CompNode::Row { children, gap, .. } => {
    let total_flex: u16 = children.iter().filter_map(flex_of).sum();
    if total_flex == 0 {
        // ==== 现有逻辑(不动) ====
    } else {
        // 第一遍: flex=0 子节点正常布局并记录; flex>0 子节点测内容宽
        let mut parts: Vec<Vec<StyledLine>> = Vec::with_capacity(children.len());
        let mut fixed_w = 0usize;
        let mut flex_w: Vec<usize> = Vec::with_capacity(children.len());
        for child in children {
            match flex_of(child) {
                None => {
                    let lines = layout(child, viewport);
                    fixed_w += lines.iter().map(|l| l.width()).max().unwrap_or(0);
                    parts.push(lines);
                    flex_w.push(0);
                }
                Some(_) => {
                    flex_w.push(content_width(child));
                    parts.push(Vec::new());
                }
            }
        }
        // 剩余空间 = viewport - 固定宽 - gap 总宽;按 flex 权重分配(阶梯法,无浮点)
        let gaps = children.len().saturating_sub(1) * *gap as usize;
        let remaining = (viewport.0 as usize).saturating_sub(fixed_w).saturating_sub(gaps);
        let mut alloc: Vec<usize> = Vec::with_capacity(children.len());
        let mut acc = 0usize;
        for child in children {
            match flex_of(child) {
                Some(f) => {
                    let lo = acc * remaining / total_flex as usize;
                    acc += f as usize;
                    let hi = acc * remaining / total_flex as usize;
                    alloc.push(hi - lo);
                }
                None => alloc.push(0),
            }
        }
        // 第二遍: flex>0 子节点用分配宽布局(Text 补空格到分配宽)
        for (i, child) in children.iter().enumerate() {
            if flex_of(child).is_some() {
                let mut lines = layout(child, (alloc[i] as u16, viewport.1));
                for line in &mut lines {
                    let pad = alloc[i].saturating_sub(line.width());
                    if pad > 0 {
                        line.spans.push(TextSpan { text: " ".repeat(pad), style: None });
                    }
                }
                parts[i] = lines;
            }
        }
        // 拼接(复用现有逐行拼 span 逻辑,把它抽成与 parts 同形的循环)
        // —— 现有 60-95 行的拼 span 循环保持不变,只把 parts 的来源换成上面的两遍结果
    }
}
```

`Col` 分支同样两遍(垂直方向:flex=0 内容高、剩余高度按权重分配;分配高度传给 layout 的 viewport.1)。现有 Col 逻辑保留为 total_flex==0 快路径。

注意:flex 子节点 layout 的 viewport 宽度要限制,否则 Text wrap 不触发(wrap 用 width 或 viewport 的最小值——Text 分支已取 `width.unwrap_or(viewport.0).min(viewport.0)`,flex 分配宽通过 width 传入?**决策:flex 分配的宽度不写进 Text.width,而是通过 viewport.0 传入**——即 flex 子节点 layout(child, (alloc_w, h))。Text 分支的 limit = width.unwrap_or(viewport.0).min(viewport.0) 自动取到 alloc_w。Row/Col 嵌套同理。)

- [ ] **步骤 4:运行确认通过**

运行:`HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-term --lib comp_layout` + `cargo test -p helix-term --lib` 全量 + `cargo clippy -p helix-term --all-targets`
预期:PASS,零警告。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/ui/comp_layout.rs
git commit -m "feat: Row/Col flex 弹性分配 + Text wrap 换行"
```

---

### 任务 3:scroll offset 虚拟化(comp_layout.rs)

**文件:**
- 修改:`helix-term/src/ui/comp_layout.rs`(Scroll 分支 + is_single_line 辅助 + 测试)

- [ ] **步骤 1:写失败测试**

```rust
#[test]
fn scroll_offset_shows_window() {
    // 10 个单行子节点, offset=3, height=4 → 恰产出第 3..7 行
    let children = (0..10).map(|i| text(&format!("row{i}"))).collect::<Vec<_>>();
    let node = CompNode::Scroll { children, height: 4, offset: Some(3) };
    let lines = layout(&node, (40, 20));
    assert_eq!(lines.len(), 4);
    let joined = lines.iter().map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>()).collect::<Vec<_>>();
    assert_eq!(joined, vec!["row3", "row4", "row5", "row6"]);
}

#[test]
fn scroll_no_offset_keeps_tail() {
    // 回归: 无 offset → 现状"取末 height 行"
    let children = (0..10).map(|i| text(&format!("row{i}"))).collect::<Vec<_>>();
    let node = CompNode::Scroll { children, height: 4, offset: None };
    let lines = layout(&node, (40, 20));
    let joined = lines.iter().map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>()).collect::<Vec<_>>();
    assert_eq!(joined, vec!["row6", "row7", "row8", "row9"]);
}

#[test]
fn scroll_offset_past_end_yields_empty() {
    let children = (0..3).map(|i| text(&format!("row{i}"))).collect::<Vec<_>>();
    let node = CompNode::Scroll { children, height: 4, offset: Some(10) };
    assert!(layout(&node, (40, 20)).is_empty());
}

#[test]
fn scroll_offset_mixed_height_slow_path() {
    // 混合: 单行 + wrap 多行子节点 → 慢路径按行号跳过也正确
    let children = vec![
        text("a"),
        CompNode::Text { spans: vec![TextSpan { text: "0123456789".into(), style: None }], width: Some(5), id: None, flex: None, wrap: true },
        text("c"),
    ];
    // 内容行: a / 01234 / 56789 / c → 4 行; offset=2 → 56789 / c
    let node = CompNode::Scroll { children, height: 10, offset: Some(2) };
    let lines = layout(&node, (40, 20));
    let joined = lines.iter().map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>()).collect::<Vec<_>>();
    assert_eq!(joined, vec!["56789", "c"]);
}
```

- [ ] **步骤 2:运行确认失败**

运行:`HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-term --lib comp_layout`
预期:新测试 FAIL(`offset` 未实现)。

- [ ] **步骤 3:实现**

`Scroll` 分支替换:

```rust
CompNode::Scroll { children, height, offset } => {
    let h = (*height).min(viewport.1) as usize;
    if h == 0 {
        return Vec::new();
    }
    match offset {
        None => {
            // 现状语义(保持): 全量布局取末 h 行
            let col = layout(
                &CompNode::Col { children: children.clone(), gap: 0, flex: None },
                (viewport.0, u16::MAX),
            );
            let skip = col.len().saturating_sub(h);
            col.into_iter().skip(skip).collect()
        }
        Some(off) => {
            let off = *off as usize;
            // 快路径: 全部子节点确定单行 → offset 即子节点索引,按索引切片(产出 O(视口))
            if children.iter().all(is_single_line) {
                let mut out = Vec::new();
                for child in children.iter().skip(off).take(h) {
                    out.extend(layout(child, viewport));
                }
                return out;
            }
            // 慢路径: 布局并跳过 offset 行(混合高度/wrap 场景,最坏 O(内容))
            let mut out = Vec::new();
            let mut produced = 0usize;
            'outer: for child in children {
                for line in layout(child, (viewport.0, u16::MAX)) {
                    if produced < off {
                        produced += 1;
                        continue;
                    }
                    out.push(line);
                    if out.len() >= h {
                        break 'outer;
                    }
                }
            }
            out
        }
    }
}

/// 子节点是否确定只渲染 1 行(单行列表的 scroll 快路径用)。
fn is_single_line(node: &CompNode) -> bool {
    match node {
        CompNode::Text { wrap, .. } => !*wrap,
        CompNode::Button { .. } | CompNode::Input { .. } => true,
        CompNode::Row { children, .. } => !children.is_empty() && children.iter().all(is_single_line),
        CompNode::Col { .. } | CompNode::Scroll { .. } => false,
    }
}
```

注意:任务 2 已给 Col 加 flex 字段,这里 `CompNode::Col { children, gap: 0, flex: None }` 的构造带 flex。

- [ ] **步骤 4:运行确认通过**

运行:`HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-term --lib` + clippy 零警告
预期:PASS(现有 scroll 测试 + 新 4 个)。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/ui/comp_layout.rs
git commit -m "feat: scroll offset 虚拟化(快/慢双路径)"
```

---

### 任务 4:filetree 迁移 scroll offset + 验证

**文件:**
- 修改:`plugins/features/filetree/index.js:545-556`(render 尾部)
- 同步:`~/.config/helix/plugins/features/filetree/index.js`

- [ ] **步骤 1:迁移 render**

`filetree/index.js` render 尾部,把"手动 slice + 无 offset scroll"改为"slice(避免 JS 全量生成行)+ offset":

```js
function render(focus, ctx) {
  try {
    const rows = visible_rows(S.tree, S.show_hidden, 0, []);
    const h = Math.max(1, (ctx ? ctx.height : 30) - 1);
    // 手动窗口化: 以 cursor 为中心截取 h 行(scroll 虚拟化只需 JS 侧不生成窗口外行)
    const start = Math.max(0, Math.min(S.cursor - Math.floor(h / 2), rows.length - h));
    const window = rows.slice(start, start + h);
    const lines = window.map((r, i) => row_el(r, start + i === S.cursor, r.node.path === S.current));
    if (rows.length > 20000) {
      lines.push(helix.el("text", "... " + (rows.length - start - h) + " 行未显示(内容过多)", { style: "ui.virtual" }));
    }
    return helix.el("scroll", lines, { height: h, offset: start });
  } catch (e) {
    return helix.el("col", [helix.el("text", "filetree 渲染错误: " + (e && e.message || e), { style: "ui.popup" })]);
  }
}
```

变更点:最后一行 `{ height: h }` → `{ height: h, offset: start }`。其余不动(JS 侧仍 slice 窗口,避免 row_el 全量生成)。

- [ ] **步骤 2:语法检查 + 编译 + 同步**

```bash
node --check plugins/features/filetree/index.js
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo build --release
cp plugins/features/filetree/index.js ~/.config/helix/plugins/features/filetree/index.js
```

- [ ] **步骤 3:真机验证**

release 版 helix:打开大目录(>500 项,可临时建)`:filetree` → 上下滚动 → 选中行跟随光标、显示行正确、无错位。`:config-reload` 后正常。环境受限则如实报告跳过原因。

- [ ] **步骤 4:Commit**

```bash
git add plugins/features/filetree/index.js
git commit -m "refactor: filetree 迁移到 scroll offset 虚拟化"
```

---

## 自检记录

- **规格覆盖:** scroll offset(任务 3)、flex(任务 2)、wrap(任务 2)、兼容性零改动(任务 1 全可选字段 + scroll None 保留旧语义)、性能保持(布局器无状态纯函数,scroll 快路径按索引切)、filetree 迁移(任务 4)。非目标(边框/焦点/绝对定位/命中)未引入。
- **占位符:** 无 TODO;所有代码块可直接执行。任务 2 的 Row 拼接复用现有循环(明确标注"现有 60-95 行拼 span 循环保持不变"),flex 分支是其前置测量/分配。
- **类型一致性:** `flex: Option<u16>`、`wrap: bool`、`offset: Option<u16>` 在任务 1-3 中命名统一;`flex_of`/`content_width`/`is_single_line` 定义于使用处同一文件;`text()` helper(comp_layout.rs:210)供测试用,scroll 测试里引用它。
- **依赖顺序:** 任务 1(字段+解析)→ 任务 2(布局用字段)→ 任务 3(Scroll 用 flex 字段构造 Col)→ 任务 4(插件用 offset)。任务 2/3 都动 comp_layout.rs,顺序执行不冲突。
