# API:Picker 与主题

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

## Picker 选择器(helix.picker)

插件定义数据源,调起**原生 Picker**(nucleo fzf 模糊匹配/滚动/预览/键位全核心,与 `:files`/`:grep` 同款 UI)。

```js
helix.picker.define("files", {
  columns: ["name", "path"],                  // 列名(第一列参与过滤)
  // cells = 看的,payload = preview/action 收的(数组时二者相同)
  items: () => [
    { cells: ["main.rs", "src/main.rs"], payload: [helix.cwd() + "/src/main.rs"] },
  ],
  preview: (row) => ({ path: row[0], line: 0 }),  // 可选;文件预览(FileLocation)
  action: (row) => helix.open_file(row[0]),       // 可选;Enter 回调
});
helix.picker.run("files");
```

- **行格式**:数组 `[c1, c2]`(cells = payload)或 `{ cells: [...], payload: [...] }`(cells 展示,payload 传给 preview/action——如 buffers 源藏 buffer id)。
- **路径 payload 一律存绝对路径**:`read_tree(".")` / `rg` 给的是相对路径,直接当 payload 存
  就等于把 cwd 钉进条目里 —— `:cd` 之后 `preview`/`action` 会指错文件。用
  `helix.cwd() + "/" + p` 现拼(见 `plugins/examples/picker.js` 的 `toAbs`);
  想显示成短的就把相对形式放 `cells`、绝对路径放 `payload`(见上面 files 源)。
- `items` 支持 Promise:`helix.read_tree(".")` / `helix.run_async("rg ...")` 直接返回。
- 回退语义:源未定义/行宽不匹配 → 状态栏报错;items 空 → 打开空列表;action/preview 抛错 → 忽略;preview null → 无预览。
- 四个常用源(files/grep/buffers/symbols)是**JS 侧**用 `helix.picker.define` 定义的,示例在 `plugins/examples/picker.js`;
  核心(`helix-js/src/picker.rs`)只提供**源注册表 + 分发**(`define`/`run`/`invoke_action`/`invoke_preview`),不含任何数据源。

```js
// grep 源(排除 target/.git;无匹配 exit 1 → 空列表)
helix.picker.define("grep", {
  columns: ["file:line", "text"],
  items: () =>
    helix.run_async("rg -n --no-heading -g '!target' -g '!.git' .")
      .then((out) => out.split("\n").filter(Boolean).map((line) => {
        const [p, ln, ...rest] = line.split(":");
        return {
          cells: [p.replace(/^\.\//, "") + ":" + ln, rest.join(":")],
          payload: [p.startsWith("/") ? p : helix.cwd() + "/" + p.replace(/^\.\//, ""), String(Number(ln) - 1)],
        };
      }))
      .catch((e) => { if (e && /^exit 1/.test(e.message)) return []; throw e; }),
  preview: (row) => ({ path: row[0], line: Number(row[1]) }),
  action: (row) => helix.open_file(row[0], { row: Number(row[1]) }),
});
```

**优缺点**
- 优点：性能原生(nucleo/滚动/预览全 Rust);插件可定义任意源;行格式数组/对象分离;空列表/无匹配正常展示。
- 局限：候选一次性返回(非流式,万级目录启动延迟);无 builder 旋钮透传(历史/默认动作核心默认);预览仅文件(无任意文本预览);grep 不做"按输入重跑"(一次性加载 + nucleo 过滤)。

### 源的可选键位钩子:`onKey`

```js
helix.picker.define("my-src", {
  columns: ["name"],
  items: () => [["one"]],
  onKey: (e) => {                       // 可选:核心不认识的键会先问它
    // e = { key, ctrl, alt, shift, query, selected }
    if (e.ctrl && e.key === "y") { helix.echo("y"); return "handled"; }
    return undefined;                   // 不认领 => 交回核心 / 查询框
  },
});
```

**契约**

- 只有**返回字符串 `"handled"`** 才算消费该键;返回其它 / 抛错 / 不返回 => **未处理**(不上抛、不静默改状态)
- 只收到**事实**(只读):`key`(字符,或键名如 `"Escape"`/`"Up"`)、`ctrl`/`alt`/`shift`、`query`、`selected`
- **可选**:不声明 => 零行为变化

**边界(重要)**

- 只有**核心不认识的键**会问插件。`Esc` / `Enter` / `Tab` / `Shift-Tab` / `Up` / `Down` /
  `C-n` / `C-p` / `C-d` / `C-u` / `PageUp` / `PageDown` / `Home` / `End` / `C-c` / `C-s` /
  `C-v` / `C-t` 都是**核心先处理** => 插件**无法**覆盖它们,也就**无法**破坏
  「Esc 能关 / Enter 能接受 / 翻页」这些不变量
- **纯字符输入永不交给插件**(无 Ctrl/Alt 的字符键)=> 从契约上杜绝「插件吞掉打字」
- 只在**按键那一刻**调用 => **不在每帧路径**上(与渲染委托不同,这个没有每帧代价)

**典型用途**:给 picker 加自有快捷键(复制路径、跳转、切排序、打标记…)

## 主题

### `helix.set_theme({ scope: color | { fg?, bg?, modifiers? } })`

实时覆盖任意主题 scope。scope 可用 `ui.*` 或任意语法 scope(如 `keyword`、`string`)。

**合并语义**(2026-10-25 起):只改动本次给出的 scope,**不清掉**别的插件/别的调用注册的覆盖;
传 **空对象 `{}`** 仍表示"清空所有覆盖"(历史用法)。此前是"整体替换" —— 任何插件调一次
`set_theme` 都会抹掉其它插件的覆盖,具体事故是 `statusline` 的 `corners="powerline"`:
它按模式懒注册 `statusline.cap.<mode>`,切模式时把前一个模式的 cap scope 连同别人的一起抹掉
(表现为端帽失去颜色)。要清空请用 `reset_theme()`,要改单条就只传那一条。

```js
helix.set_theme({ "ui.popup": "#ff79c6", "error": "red" });            // 字符串 = fg
helix.set_theme({
  "ui.selection": { fg: "#111111", bg: "#ff0000", modifiers: ["italic"] },
  "warning": { fg: "#f1fa8c", modifiers: ["bold", "underline"] },
});
```

- 颜色支持 palette 颜色名或 `#rrggbb`(不支持引用另一 scope);非法条目忽略。
- modifiers:`bold` `dim` `italic` `underline` `strikethrough` `reversed`。
- 覆盖即时生效(重建主题并替换);支持继承主题(inherits);语法 scope 覆盖同样生效。

### `helix.reset_theme()`

清空全部覆盖,恢复当前基础主题。

### `helix.get_style(scope) -> { fg, bg, modifiers } | null`

读取当前合并后样式(hex;未定义/未知 scope → null)。状态栏取色、自定义组件配色用。

```js
const s = helix.get_style("ui.selection");
```

### `helix.theme_info() -> { name, themes }` / `helix.set_theme_name(name)` / `helix.on("theme-change", fn)`

当前主题名 + 可用列表(切换器用);切换基础主题(异步应用:加载 + 应用 + 清空覆盖;失败状态栏报错);主题变化事件(set_theme/reset_theme/set_theme_name/`:theme` 后触发)。

**优缺点**
- 优点：实时覆盖即时生效;scope 级(含语法高亮);get_style 读合并后样式;继承主题正确。
- 局限：颜色不支持引用另一 scope;覆盖集整体替换(非增量 merge);set_theme_name 异步(调用后立即 get_style 可能还是旧主题)。
