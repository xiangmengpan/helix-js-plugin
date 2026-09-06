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

## 组件树(el)

`render` 可返回嵌套组件树(旧行数组仍兼容)。节点由 `helix.el(type, arg, opts)` 构造:

```js
// text:单行文本。arg 为字符串或富文本段数组(一行多色)
helix.el("text", "hello", { style: "error", width: 20 })
helix.el("text", [ { text: "标题 ", style: "error" }, { text: "尾注", style: "comment" } ])

// row / col:水平并排 / 垂直堆叠(gap = 间距)
helix.el("row", [child, child], { gap: 1 })
helix.el("col", [child, child], { gap: 0 })

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
- 面板 render 固定收到 `focus = null`(面板无节点焦点,见 plugin-api §19)。

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
- `focusable: true` → 面板可参与焦点路由(Tab/Esc/节点按键直达,批次 7 能力)。

**优缺点**：优点：真实布局收缩(非覆盖),可换边;focusable 焦点路由。局限：render 无节点焦点(focus=null);内容需自维护命中。

## 界面定制

### `helix.set_statusline(fn)`

```js
helix.set_statusline(({ mode, path }) => " N " + path);
// 返回 null → 隐藏状态栏;未注册 → 内置状态栏
```

**优缺点**：优点：整行替换,可返回任意字符串;null 隐藏。局限：纯字符串(无富文本段)。

### `helix.set_buffer_icon(fn)`

`fn(path) -> string | null`:bufferline/文件树图标(建议从 `lib/icons.js` 取)。**优缺点**：优点：load 即注册;null 无图标。局限：只返回字符,无样式。

### `helix.set_completion_icon(fn)` / `helix.set_completion_render(fn)`

kind 列图标钩子 + 候选行渲染钩子(详见 plugin-api §15):

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

组件外观 JS 绘制 + 组件状态读取 + which-key 提示:

```js
helix.set_component_render(tid, (ctx) => [{ type: "text", text: " " + (helix.get_component_state(tid).title || "term") }]);
helix.set_keymap_hint((ctx) => ({ text: ctx.entries.map(e => e.keys + " " + e.doc).join("\n"), position: "bottom-right" }));
```

**优缺点**：优点：组件外观全定制(标题条/标签条);提示位置可配;null 隐藏。局限：网格渲染留 Rust(JS 只能画行级内容);提示回调每帧调用。
