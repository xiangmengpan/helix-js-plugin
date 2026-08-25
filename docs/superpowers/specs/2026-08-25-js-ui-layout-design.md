# 设计:JS UI 布局增强(Scroll 虚拟化 + flex + wrap)

日期:2026-08-25
状态:草案(待审核)

## 背景与动机(代码证据)

当前组件树布局(`helix-term/src/ui/comp_layout.rs`)有三个真实缺陷:

1. **Scroll 全量布局**:`comp_layout.rs:121-124`——先 `layout(Col, (宽, u16::MAX))` 无高度限制完整布局整个子树,再取末 h 行,且每帧 `children.clone()`。O(内容) 而非 O(视口)。filetree 因此被迫手动窗口化(`visible_rows()` 全量算 + `slice(start, start+h)` + 20000 行上限截断提示)。
2. **Text 无换行**:`comp_layout.rs:23-39` 超宽直接截断丢字符。
3. **Row/Col 无弹性占比**:全部内容宽度,窗口 resize 列宽不自适应,表格/多列对齐靠 JS 拼空格。

目标:低成本补齐这三个能力,**现有插件零改动,性能不变差**(Scroll 类场景反而提速)。

## 方案与范围取舍

| 方向 | 取舍 |
|---|---|
| scroll `offset` 虚拟化 | **做**(filetree 解药,收益最大) |
| flex 弹性占比 | **做**(两遍布局,树小开销可忽略) |
| text `wrap` | **做**(长文本不丢字符) |
| 边框/背景、焦点管理、绝对定位、组件命中 | **不做**(工作量大,现有插件无需求;YAGNI) |
| JS 侧滚动状态 API(滚动条/动画) | **不做**(offset 由插件自己维护,与 filetree 现状一致) |

## API 设计(全部可选属性,现有插件零改动)

```js
// scroll: offset 指定内容起始行(默认无 = 保持现状"取末 h 行"语义)
helix.el("scroll", rows, { height: 30, offset: 42 });   // 显示第 42..72 行

// flex: 子节点弹性权重(0 = 内容宽度,缺省 0)。Text/Row/Col/Button/Input 均支持。
helix.el("row", [
  helix.el("text", "文件名", { flex: 1 }),     // 占满剩余空间
  helix.el("text", "大小", { width: 8 }),      // 固定 8 格
], { gap: 1 });

// wrap: 超出分配宽度按字符换行成多行(不再截断)
helix.el("text", long_text, { wrap: true, width: 40 });
```

- `scroll` 的 `offset` 是**内容行号**(不是子节点索引):内容第 offset 行开始显示,共 height 行
- `offset` 缺省时保持现状(取末 h 行),which-key 等现有调用零改动
- `flex` 分配算法:容器内 flex=0 子节点先测内容宽;剩余空间(viewport − Σ内容宽 − gap×间隔)按权重比例分给 flex>0 子节点;权重 1:2 → 空间 1:2
- `wrap` 与 `width` 互斥时:wrap 优先按分配宽度换行;width 作为分配宽度依据(无 flex 时)

## 实现

### 1. CompNode 扩展(`helix-js/src/types.rs`)

```rust
Text   { spans, width, id, flex: Option<u16>, wrap: bool }
Row    { children, gap, flex: Option<u16> }
Col    { children, gap, flex: Option<u16> }
Button { label, width, id, flex: Option<u16> }
Input  { value, width, id, flex: Option<u16> }
Scroll { children, height, offset: Option<u16> }
```

### 2. 解析(`helix-js/src/popup.rs` js_el + parse_node)

- `js_el`:text 的 opts 加 `flex`(u16)/`wrap`(bool);row/col/button/input 的 opts 加 `flex`;scroll 的 opts 加 `offset`(u16)
- `parse_node`:对应字段读取,缺省 `flex: None / wrap: false / offset: None`

### 3. 布局算法(`helix-term/src/ui/comp_layout.rs`)

**Row/Col 两遍布局**:
1. 第一遍:对 flex=0 子节点调 `layout` 测内容宽(Col 测内容高)
2. 剩余空间 = viewport − Σ内容宽 − gap×(n−1);按权重 `flex_i / Σflex` 分配
3. 第二遍:flex>0 子节点用分配宽度布局(Text 分配宽度 > 内容宽 → 右补空格;wrap 文本 → 在分配宽度内换行)

**Text wrap**:
- `wrap=true` 且内容宽 > 分配宽:按字符(注意 CJK/全角宽度,`text.width()` 现有辅助)逐行切分,产出多行
- `wrap=false`:现状截断

**Scroll offset 虚拟化**:
- 遍历子节点布局,累计产出行数;跳过 offset 行之前的全部输出,只收集 [offset, offset+height) 区间
- 子节点改为**引用遍历**(去掉 `children.clone()`)
- 单行子节点列表(filetree 行、菜单项):布局计算 O(视口) —— 跳过的子节点只需累计行数
- 多行子节点(wrap 文本、嵌套 Col):跳过部分仍需布局以数行数,最坏 O(内容) 与现状相同,但产出仍是 O(视口)
- `offset: None` → 现状"完整布局取末 h 行"(不虚拟化,保持既有语义)

### 4. 性能

- 布局器保持**无状态纯函数** `layout(node, viewport) -> Vec<StyledLine>`,不引入缓存/记忆化
- flex 两遍布局:现有树小(<100 节点),常数级增加
- Scroll 虚拟化:filetree 类大列表从"产出 O(内容)"降为"产出 O(视口)"(布局计算单行列表同样 O(视口))
- 顺带消除 Scroll 每帧 `children.clone()`

## 测试

### comp_layout.rs 单测(布局器,纯函数最易测)

- `scroll_offset_shows_window`:30 行长列表 + offset=10,height=5 → 恰产出第 10..15 行
- `scroll_no_offset_keeps_tail_semantics`:无 offset → 末 h 行(现状回归)
- `scroll_single_line_children_skip_is_cheap`:5000 个单行子节点 + offset=2500 → 产出 30 行(此测试只验产出正确;性能靠文件 tree 集成)
- `row_flex_allocates_ratio`:Row [flex:1 text, flex:2 text] → 两列宽 1:2(viewport 100 → 33/66 减 gap)
- `row_flex_zero_keeps_content_width`:flex:0 + 固定 width 混合
- `text_wrap_multiline`:wrap=true 长文本 → 多行且每行 ≤ 分配宽
- `text_wrap_cjk_boundary`:中文长文本 wrap 不切坏字符
- 现有全部测试保持绿(回归)

### helix-js 单测(popup.rs)

- `el_scroll_offset_passthrough` / `el_flex_passthrough` / `el_wrap_passthrough`:opts 解析到 CompNode 字段正确
- 类型错误:flex 非数字 / wrap 非布尔 / offset 非数字 → 报错

### 集成测试(helix-term)

- filetree 迁移到 `scroll { height, offset: start }` 后:滚动到中部 → 状态断言行内容正确(替换现有手动 slice 断言)
- 大目录(>5000 文件)打开面板不超时(现 20000 上限是防御性,迁移后应可放宽或保持)

## 非目标(YAGNI)

- 边框/背景、padding/margin(非 gap)、text-align(center/right)
- 焦点环管理(Button 焦点样式保持 JS 侧控制)、Input 框架级输入状态
- 绝对定位/overlay、组件级命中测试/点击
- 滚动条、惯性滚动、JS 滚动状态 API(`offset` 由插件自己维护)
- 布局缓存/记忆化(保持无状态纯函数)

## 涉及文件

- `helix-js/src/types.rs`(CompNode 字段扩展)
- `helix-js/src/popup.rs`(js_el/parse_node 新 opts)
- `helix-term/src/ui/comp_layout.rs`(两遍布局、wrap、scroll 虚拟化 + 单测)
- `helix-term/tests/`(filetree scroll 集成测试,迁移可选)

## 性能结论(直接回答"改进后性能变化")

- 布局器:flex 两遍布局常数级增加,现有树小无感知;无状态纯函数保持,无累积成本
- Scroll:filetree 类大内容从"每帧全量布局+产出 O(内容)"→"产出 O(视口)",**显著变快**
- 无 wrap 的短文本:行为不变,截断逻辑保留
