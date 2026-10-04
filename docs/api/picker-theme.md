# API:Picker 与主题

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

## Picker 选择器(helix.picker)

插件定义数据源,调起**原生 Picker**(nucleo fzf 模糊匹配/滚动/预览/键位全核心,与 `:files`/`:grep` 同款 UI)。

```js
helix.picker.define("files", {
  columns: ["name", "path"],                  // 列名(第一列参与过滤)
  items: () => [["main.rs", "src/main.rs"]],  // 行数组(同步或 Promise/async)
  preview: (row) => ({ path: row[1], line: 0 }),  // 可选;文件预览(FileLocation)
  action: (row) => helix.open_file(row[1]),       // 可选;Enter 回调
});
helix.picker.run("files");
```

- **行格式**:数组 `[c1, c2]`(cells = payload)或 `{ cells: [...], payload: [...] }`(cells 展示,payload 传给 preview/action——如 buffers 源藏 buffer id)。
- `items` 支持 Promise:`helix.read_tree(".")` / `helix.run_async("rg ...")` 直接返回。
- 回退语义:源未定义/行宽不匹配 → 状态栏报错;items 空 → 打开空列表;action/preview 抛错 → 忽略;preview null → 无预览。
- 四个内置源(files/grep/buffers/symbols)由**核心**提供;自定义源的模板见 `plugins/examples/picker.js`。

```js
// grep 源(排除 target/.git;无匹配 exit 1 → 空列表)
helix.picker.define("grep", {
  columns: ["file:line", "text"],
  items: () =>
    helix.run_async("rg -n --no-heading -g '!target' -g '!.git' .")
      .then((out) => out.split("\n").filter(Boolean).map((line) => {
        const [p, ln, ...rest] = line.split(":");
        return { cells: [p + ":" + ln, rest.join(":")], payload: [p, String(Number(ln) - 1)] };
      }))
      .catch((e) => { if (e && /^exit 1/.test(e.message)) return []; throw e; }),
  preview: (row) => ({ path: row[0], line: Number(row[1]) }),
  action: (row) => helix.open_file(row[0], { row: Number(row[1]) }),
});
```

**优缺点**
- 优点：性能原生(nucleo/滚动/预览全 Rust);插件可定义任意源;行格式数组/对象分离;空列表/无匹配正常展示。
- 局限：候选一次性返回(非流式,万级目录启动延迟);无 builder 旋钮透传(历史/默认动作核心默认);预览仅文件(无任意文本预览);grep 不做"按输入重跑"(一次性加载 + nucleo 过滤)。

## 主题

### `helix.set_theme({ scope: color | { fg?, bg?, modifiers? } })`

实时覆盖任意主题 scope(整体替换旧覆盖集)。scope 可用 `ui.*` 或任意语法 scope(如 `keyword`、`string`)。

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
