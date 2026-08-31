# API:终端与布局树

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

## 终端

### `helix.open_terminal({ cmd, side, size, onExit? })`

原生 pty 终端面板(布局树叶子)。返回 view_id。

```js
const id = helix.open_terminal({ cmd: "bash", side: "right", size: 40 });
helix.term_write(id, "ls\n");        // 写入
helix.term_kill(id);                 // 杀进程
helix.term_resize(id, 30, 100);      // pty TIOCSWINSZ(Unix)
helix.term_clear(id);                // 清屏
helix.set_terminal_mode(id, "floating");  // dock|fullscreen|floating|minimized
```

### 四种显示模式

| 模式 | 行为 |
|------|------|
| `dock`(默认) | 停靠推挤(布局树叶子) |
| `fullscreen` | 占满整个编辑区 |
| `floating` | **居中悬浮窗(带边框,60%×70% 视口),不占 split 布局,渲染在最上层** |
| `minimized` | 底部 1 行细条;按键穿透编辑器、进程保活 |

### 终端能力与钩子

- vte 解析:光标/SGR 颜色/粗体/清屏/滚回/alt screen(vim、htop 可显示)。
- **C-\ 模式切换**(lazyvim 语义):insert → 终端 normal(滚动缓冲 j/k/gg/G/PageUp/PageDown);normal 再 C-\ → 回 helix;normal 内 i/a/Esc 回 insert、q 关闭。
- 滚动缓冲可查看;宽字符(CJK)支持;按键直通 pty(raw mode)。
- 钩子:`term-key`(返回 normal/pass/consume/minimize/close)、`term-open/exit/close/title/resize/mode-change`、`term_state(ptyId, key, value?)` 跨会话持久化。

**优缺点**
- 优点：原生 pty 面板(非模拟器套壳);四种显示模式;滚动缓冲;状态持久化;键位钩子可编程管理。
- 局限：组合字符/鼠标/选择复制不支持;滚回只存不显示(上限 1000 行);PTY 仅 Unix;spawn 无超时。

## 布局树

弹窗之外,面板/终端/编辑器都挂在**布局树**上:编辑器是 id=0 的叶子,面板和终端通过切分活动叶子挂载。JS 直接操作:

```js
const leafId = helix.split("right", { terminal: { cmd: "htop", size: 40 } });
const leafId2 = helix.split("bottom", {
  panel: { render: () => helix.el("col", [helix.el("text", "hi")]), onKey: (k) => "handled" },
});

helix.close_leaf(id);              // 移除叶子(终端进程被杀、面板回调清掉)
helix.zoom(id);                    // 缩放指定叶子到全屏
helix.unzoom();                    // 恢复
helix.resize_leaf(id, 0.3);        // 调整分界比例(0~1)
helix.focus(id);                   // 聚焦叶子
```

窗口管理(`C-w` 进入,任意叶子焦点可用):方向聚焦/交换、尺寸 ∓5%、关闭/最小化/最大化。`layout_fix(id, true)` 固定叶子(免疫交换/关闭/最小化)。

### 布局序列化(半成品)

```js
const layout = helix.get_layout();     // { tree: {...}, active: id, zoomed: id|null } | null
helix.restore_layout(layout);          // 目前只写入缓存(树重建未接线,见 plugin-api §19)
```

`tree` 为嵌套 JSON:`{ "type": "leaf", "id": N }` 或 `{ "type": "split", "dir": "h"|"v", "ratio": 0.5, "first": ..., "second": ... }`。

**优缺点**
- 优点：zellij 式窗口管理(任意叶子焦点);split 可挂终端/面板;layout_fix 免疫操作;布局可读(get_layout)。
- 局限：restore_layout 未接线(只写缓存);窗口模式键位需插件/内置组合;无拖拽调整。
