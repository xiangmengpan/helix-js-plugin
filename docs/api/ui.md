# API:弹窗、组件树、面板、界面定制

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

## 弹窗

### `helix.open_popup({ render, onKey?, onClose?, width?, height?, position?, layer? })`

JS 渲染的覆盖层弹窗(模态层;同 `layer` 重复 `open_popup` 替换同层,不同 layer 叠层并存)。返回弹窗 id。

```js
helix.open_popup({
  width: 40, height: 10,                     // 可选:尺寸上限(clamp)
  position: { row: 5, col: 10 },             // 可选:屏幕锚点
  render: (focus, ctx) => [                  // focus = 焦点节点 id 或 null(见组件树)
    { text: "Error: ", style: "error" },     // 样式行:style = 主题 scope 名
    "普通行",                                // 或纯字符串
  ],
  onKey: (key, doc) => {                     // key 见下;doc 可编辑
    if (key.name === "Enter") { doc.insert(doc.cursor.row, doc.cursor.col, "x"); return "close"; }
    if (key.name === "Down") return "handled";
    return "ignore";                         // 事件穿透
  },
  onClose: () => { /* 弹窗关闭时 */ },
});
```

- **render 返回两种形式**:字符串/`{text, style}` 数组(行 API),或 `helix.el` 组件树(见下)。
- **尺寸**:`width`/`height` 支持数字(px;锚点模式 = 尺寸上限 clamp)或 `"NN%"` 字符串(0<NN≤100 = 视口百分比,仅 center 模式且须两轴成对)。
- **layer**:可选字符串,缺省 `"plugin-popup"`(旧调用零破坏)。同 layer 再 open 替换同层;不同 layer 叠层并存。
- **position**:缺省 → 锚点式(参照光标/内容自适应);`{ row, col }` → 屏幕锚点(均现状);`"center"` → 居中浮层:无视光标与内容尺寸,按视口算固定居中矩形(百分比 → 视口比例;px → 固定尺寸,超视口钳满屏;无任何尺寸 → 约定 80%×80%)。内容区 = 矩形去边框,溢出由 JS 侧 `el("scroll")` 自理。
- **onKey 返回值**:`"close"`(关闭并触发 onClose) | `"handled"`(消费) | `"ignore"`(穿透)。未识别 → `"handled"`。
- 未提供 `onKey` 时:Esc 关闭,其余穿透。
- `key` 对象:`{ name, shift, ctrl, alt }`;name:字符键/Enter/Esc/Tab/Backspace/Delete/Insert/方向键/Home/End/PageUp/PageDown/F1..F12。
- onKey 的 `doc` 同命令 ctx.doc(编辑 = 一次按键一个事务)。

**优缺点**：优点：模态清晰、onKey 三态返回;同层替换、异层并存(主窗 + 叠层菜单/信息);center 浮层比例尺寸。局限：不传 layer 时同层 open 替换前一个(无多弹窗);百分比仅 center 且须成对;Shift-Tab 不可表示。

## 绘制原语(`helix.draw`)

组件树负责**流式布局**(col/row/flex),但"在 (10,10) 画一个 20×5 的框"这种**绝对坐标**
画法它做不了。`helix.draw.*` 补这层:一次 render 里按顺序发几条 op,最后
`return helix.draw.done()` 交给宿主。

```js
helix.open_popup({
  width: 40, height: 12,
  render: () => {
    helix.draw.rect(0, 0, 20, 5, { border: "round", title: "日志", style: "ui.popup" });
    helix.draw.text(2, 1, "hi 中文");
    helix.draw.fill(2, 3, 16, 1, { style: "ui.selection" });   // 铺底色
    helix.draw.hline(0, 5, 20);                                // 分隔线
    helix.draw.vline(21, 0, 6);                                // 竖分隔
    return helix.draw.done();
  },
});
```

| API | 说明 |
|---|---|
| `rect(x, y, w, h, opts?)` | 矩形。`opts.border`: `plain`(缺省)/`round`/`double`/`thick`/`none`;`opts.title`: 画在上边框;`opts.fill`: `true`(用 `style` 填)或 scope 名;`opts.style`: 边框/整块的 scope |
| `fill(x, y, w, h, opts?)` | 无边框填充块。缺省铺带 `opts.style` 的**空格**(TUI 里"上底色"的手段);`opts.ch` 给单字符纹理 |
| `hline(x, y, len, opts?)` / `vline(...)` | 单字符重复线(缺省 `─` / `│`,可用 `opts.ch` 换) |
| `text(x, y, s \| [{text,style}], opts?)` | 按坐标写文字,`opts.maxWidth` 限宽(列) |
| `braille(pts, opts?)` | **半像素点阵**:`pts` = `[[dx,dy], …]`(1 列 = 2 点、1 行 = 4 点),同格的点 OR 成一个盲文字符;适合曲线/散点/细粒度进度 |
| `push_clip(x,y,w,h)` / `pop()` | **显式裁剪区**:区间内产生的 op 只在矩形内可见(坐标仍绝对);忘记 pop 时 `done()` 兜底闭合 |
| `push_scroll(x,y,w,h,{dx,dy})` / `pop()` | **滚动容器**:矩形同时是裁剪区,内部坐标减去 `(dx,dy)` 后显示(**内容坐标系**)⇒ 长列表画一次、滚动只改 `dy` |
| `done()` | **必须 `return`**;它把本帧 op 交给宿主 |

> **完整样板**(两个真实插件,照着抄):
> - `plugins/picker-ui/plugin.js` —— **弹窗**:圆角外框 + 标题、多段样式的查询行、`hline`
>   分隔、`fill` 打选中行底色、`push_scroll` 装列表(滚动只改 `dy`)、`vline` 画滚动条、状态行;
> - `plugins/filetree/plugin.js` —— **侧边面板骨架**:头(根名 + 开关标记)+ 分隔线 +
>   `push_scroll` 装的滚动树(缩进/图标/选中底色/当前文件标记),窗口与滚动位置由两个
>   纯函数 `window_rows` / `scroll_top` 算(node 单测钉住)。

约定(定死,别猜):

- `(0,0)` = 组件(弹窗/面板)可视区左上角;**越界自动裁剪**,不报错。
- 样式 = 主题 scope 名(与 `el`/状态栏同一语义);自定义颜色先 `set_theme({scope: …})` 注册。
- 宽字符(CJK/全角)占 **2 列**;后续写格落在它的第二列时,该字会被清成空格(不会出现半个汉字)。
- **画布不参与交互**:Tab 焦点/点击仍由 `el("button")` / `el("input")` 承担。要"框里有按钮",
  用 `el("box")`(下面)或在画布旁放节点树。
- 同一次 render 里"画了但返回行数组" ⇒ 按**返回的值**渲染,op 丢弃(既有契约不变);
  手写 `{type:"draw"}` 而没调 `done()` ⇒ 明确报错。

### 滚动容器(内容坐标系 + 偏移)

长列表/长文档的常见需求:"内容按自己的坐标画一次,滚动只改偏移"。`push_scroll` 就是这个:

```js
let top = 0;                                  // 已滚过的行数
render: () => {
  helix.draw.push_scroll(0, 0, 30, 6, { dy: top });
    for (let i = 0; i < 200; i++) {           // 200 行只按内容坐标画
      helix.draw.text(2, i, `第 ${i} 行`);      // "屏幕第 0 行" = 内容第 top 行
    }
  helix.draw.pop();                           // 矩形之外的内容自动被裁掉
  return helix.draw.done();
}
```

- **语义**:屏幕位置 = 内容坐标 − `(dx, dy)`;`dy = 10` = "已滚过 10 行",内容第 10 行顶到容器上沿。
- **容器位置不平移坐标系**(与 clip 同一条纪律):容器在 `(2,1)` 时内容也要写 `(2,1)` 起。
- 嵌套:`push_scroll` 套 `push_clip`/`push_scroll` 都行,偏移**相加**、裁剪取**交集**;
  `pop()` 只闭**最内层**(所以混用不会错配)。
- 宽字符跨容器边界 ⇒ 整字丢弃(不会出现半个汉字)。
- 与组件树里的 `el("scroll")` 不同:后者是**布局期**"只保留最后 N 行",这里是**绘制期**的
  裁剪 + 平移,能配合绝对坐标与 braille。

### 半像素(braille)与裁剪区

```js
render: (focus, { width, height }) => {
  // 裁剪区:右半边画波形,左半边画文字 —— 两边互不越界
  helix.draw.push_clip(0, 0, 10, 4);
    helix.draw.text(0, 0, "left only, clipped at col 10");
  helix.draw.pop_clip();

  // braille:1 列 = 2 点、1 行 = 4 点 ⇒ 8 倍分辨率
  const pts = [];
  for (let dx = 20; dx < 60; dx++) {
    const dy = 2 * 4 + Math.round(2 * Math.sin(dx / 4));   // 点坐标
    pts.push([dx, dy]);
  }
  helix.draw.braille(pts, { style: "ui.text" });
  return helix.draw.done();
}
```

- 点坐标是**绝对**的(`x` 以**点**为单位:列 `c` 的左边 = `2c`),不是相对裁剪区。
- 同一格的多条 braille op:**后画的覆盖**先画的(格子只能放一个字符 + 一份样式)。
  要叠加多个系列,就在 JS 里先把点合并成一个数组再调一次。

## 显示宽度工具(`helix.text`)

与渲染器**同一张宽度表**,替代各插件自己写的 `dw()`/`cut()`:

```js
helix.text.width("中文")            // 4(CJK 占 2 列)
helix.text.truncate("中文字", 3)     // "中"(不会切出半个宽字符)
helix.text.pad("ab", 6, { align: "right" })   // "    ab";align: left(缺省)|right|center
```

## 组件树(el)

`render` 可返回嵌套组件树(旧行数组仍兼容)。节点由 `helix.el(type, arg, opts)` 构造:

```js
// text:单行文本。arg 为字符串或富文本段数组(一行多色)
helix.el("text", "hello", { style: "error", width: 20 })
helix.el("text", [ { text: "标题 ", style: "error" }, { text: "尾注", style: "comment" } ])

// row / col:水平并排 / 垂直堆叠(gap = 间距)
helix.el("row", [child, child], { gap: 1 })
helix.el("col", [child, child], { gap: 0 })

// box:带边框的容器。框是绘制原语,children 在**框内**按 col 布局;
// width/height 是**外框**尺寸(含边框)。里面的 button/input 照旧可聚焦。
helix.el("box", [child, ...], { border: "round", title: "标题", width: 30, height: 8, style: "ui.popup" })

// scroll:高度裁剪容器(保留最后 height 行)
helix.el("scroll", [child, ...], { height: 10 })

// button:可聚焦按钮,渲染为 [ label ],Enter/Space 触发 onPress
helix.el("button", "run", { id: "btn1", onPress: () => helix.echo("pressed") })

// input:可聚焦输入框。value 由引擎权威维护(JS 传值仅初始化),渲染带光标 "|";
// 编辑键自动改值触发 onChange;导航键走 onKey;multiline: true → 多行输入(Enter 换行/Up-Down 行间)
{ type: "input", id: "q", value: "abc", width: 20, multiline: false,
  onChange: (v) => { }, onKey: (k) => { } }
```

### 焦点系统

- `button`/`input`(有 `id`)自动成为可聚焦节点;`render(focus, ctx)` 的 `focus` = 当前焦点节点 id。
- **Tab** 在可聚焦节点间循环(无 Shift-Tab);button 上 Enter/Space → onPress。
- input 上:字符/Backspace/Delete 编辑触发 onChange;Left/Right/Home/End 移光标不触发;Up/Down/Enter 走 onKey(单行)/行间移动(多行)。
- 强制改值:`helix.set_input_value(popup_id, node_id, value)`;弹窗关闭自动清理 input 状态。
- 面板节点焦点**需显式开启**：`open_panel({focusable: true})` 后面板 render 才收到 `focus`，并启用 Tab 循环 / Esc 取消 / 节点按键直达；未开启时面板 render 收到 `focus = null`（见 plugin-api §4 已知限制）。

### 布局与性能

- `text.width` 是截断上限;row/col 拼接堆叠;scroll O(内容) 而非 O(视口)。
- 多 span 行不坍缩(一行多色);渲染走脏格 diff(只写变化的格)。

**优缺点**：优点：声明式树 + 引擎权威 input(onChange 自动)+ 焦点系统 + 脏格 diff。局限：scroll 全量布局(大列表卡);无树内点击命中(JS 自维护行号);面板无节点焦点;Shift-Tab 无。

## 侧边面板

### `helix.open_panel({ side, size, render, onKey?, onClose?, focusable? })`

返回面板 id。面板是**布局树叶子**:真实收缩编辑器布局(切分活动叶子,推挤而非覆盖)。

```js
const id = helix.open_panel({
  side: "right", size: 30,
  render: (focus) => [ "面板内容" ],
  onKey: (key, doc) => { /* 同 open_popup 语义 */ },
  onClose: () => { /* 面板关闭时 */ },
});
```

- 关闭:`helix.close_panel(id)` 或 onKey 返回 "close";`move_panel(id, side)` 换边。
- `focusable: true` → **面板内** `button`/`input` 节点的 Tab 焦点路由(批次 7 能力)。
  ⚠️ 它**不是**"面板能不能接收按键"的开关:面板打开后本身就是布局的活动叶子,
  `onKey` 直接收键(`tests/test/window_rail.rs` 有断言)。
- `rail: true` → **侧栏轨道**:贴 root 边缘、固定全高、不参与窗口分割,注册即聚焦;
  rail 上 `Esc`/裸 `l` 回主区。**全局只能有一条 rail** —— 注册新的会取掉旧的,
  所以两个侧栏插件都用 rail 会互相驱逐(需要并存就别用 rail)。

**优缺点**：优点：真实布局收缩(非覆盖),可换边;focusable 焦点路由。局限：render 无节点焦点(focus=null);内容需自维护命中。

## 界面定制

### `helix.set_statusline(fn, opts?)`

```js
// ① 纯文本（与内置状态栏共存）
helix.set_statusline(({ mode, path }) => " N " + path);

// ② 富文本：每段自带主题色 + 分区，整行接管
helix.set_statusline(
  () => [
    { text: " N ", style: "ui.statusline.insert" },
    { text: "main.rs" },
    { text: " 2 errors ", style: "error", zone: "right" },
  ],
  { replace: true, zones: [1, 0, 1] },
);

// 返回 null → 隐藏状态栏；未注册 → 内置状态栏
```

`fn(ctx)` 的 `ctx` 是完整的 `StatuslineCtx`（`path`/`mode`/`cursor`/`total_lines`/`modified`/`diagnostics_*`/`window_mode`/`active_leaf_type`/`read_only`/`encoding`/`line_ending`/`register`/`file_type`/`cwd`/`workspace_diagnostics_*`/`spinner` …，字段与 `helix-js/src/types.rs::StatuslineCtx` 一一对应；完整签名见 `plugins/helix.d.ts`）。

- 返回数组 = **富文本**：`{ text, style?, zone? }`，`style` 是主题 scope 名（解析失败/省略 = 跟随状态栏基样式）。
- 段间**自动插 1 空格** gap（首段前不插）；`zone` 缺省 `"left"`，`"center"`/`"right"` 受 `zones` 比例约束（`replace: true` 时才分区）。
- `replace: true` 时内置组件（模式/文件名/位置…）不渲染，整行归插件。

**优缺点**：优点：整行接管 + 逐段真主题色（不是 ANSI 拼接），分区比例可调；`null` 隐藏。局限：纯文本形态无富文本段；钩子**每帧**都调（实测 `statusline` 是最热的 JS 入口），里面不要跑重活。

> 回归锁：`plugin_statusline_parts_have_per_part_style`（断言每段 fg 各自等于 `theme.get(scope).fg`）。

### `helix.set_buffer_icon(fn)`

`fn(path) -> string | null`:bufferline/文件树图标(建议从 `lib/icons.js` 取)。**优缺点**：优点：load 即注册;null 无图标。局限：只返回字符,无样式。

### `helix.set_completion_icon(fn)` / `helix.set_completion_render(fn)`

kind 列图标钩子 + 候选行渲染钩子(详见 plugin-api §2「弹窗 / 组件树 / 面板 / 界面定制」):

```js
helix.set_completion_icon((k) => icons.getCompletionKindIcon(k));  // 轻量,每行一个字符
helix.set_completion_render((ctx) => [                             // 整行 JS 画
  { type: "text", text: `${ctx.label}  ${ctx.kind}`, style: "ui.completion" },
]);
```

- 行钩子注册时整行由 JS 画(图标钩子被绕过);行钩子回退原生两列时图标钩子生效。
- `ctx = { label, kind, kindNum, provider, detail, deprecated, matchIndices }`。
- ⚠️ 行钩子每帧每候选行调用——不要跑重逻辑(实测 ~275µs/行,20 行/帧 ≈ 5.5ms)。

**优缺点**：优点：补全 UI 全可定制(图标/列/source 标签);未注册/抛错回退原生。局限：行钩子性能有上限;图标不能带样式;行钩子内容拼进第一列(多列破坏对齐)。

### `helix.set_diagnostic_icons({ error?, warning?, info?, hint? })`

诊断标记列图标(未设置用默认 ●)。**优缺点**：优点：任意字符串(nerd font)。局限：整体替换;非字符串忽略。

### `helix.set_component_render(id, fn)` / `helix.get_component_state(id)` / `helix.set_keymap_hint(fn)`

**`{ pure: true }`:跳过整次 JS 调用。** 声明"我的渲染**只依赖宿主传入的输入**"(focus + ctx 尺寸 +
调用前宿主喂给这个组件的快照),宿主才会按输入签名缓存 —— 命中时**连回调都不调**(每帧渲染的组件
如 picker 受益最大)。声明了却实际读了主题/缓冲区等外部状态 ⇒ UI 不会更新;不声明则永远是安全的
逐帧真渲染。

**脏渲染**:`fn` 返回**显式 `null`** 表示"自上次以来无变化",宿主会**复用上次内容**并跳过
本次解析(省一次转换/分配);何时脏由插件决定,因此不会出现"宿主以为没变其实变了"的陈旧 UI。
注意**首次**调用必须真的返回内容(首次返回 `null` 会报错,因为无内容可复用);而 `undefined`
(例如忘了 `return`)仍按原来的"返回值非法"报错 —— 这是有意的:忘返回是真 bug,不该被静默成陈旧 UI。

组件外观 JS 绘制 + 组件状态读取 + which-key 提示:

```js
helix.set_component_render(tid, (ctx) => [{ type: "text", text: " " + (helix.get_component_state(tid).title || "term") }]);
helix.set_keymap_hint((ctx) => ({ text: ctx.entries.map(e => e.keys + " " + e.doc).join("\n"), position: "bottom-right" }));
```

**优缺点**：优点：组件外观全定制(标题条/标签条);提示位置可配;null 隐藏。局限：网格渲染留 Rust(JS 只能画行级内容);提示回调每帧调用。
