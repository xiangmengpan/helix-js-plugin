# 设计:光标移动事件(cursor-move)

日期:2026-08-15
状态:草案(待审核)

## 1. 动机

插件没有"光标移动/选择变化"这类高频事件(现有事件只有 save/mode-change/buffer-open 等粗粒度)。受益:
- 光标跟随(上下文栏、符号高亮、迷你地图)
- 选择变化联动(选中文本自动操作)
- 状态栏自定义光标信息

## 2. API

```js
// 光标移动(节流合并)
helix.on("cursor-move", (docId, { row, col, mode }) => { ... });

// 选择变化
helix.on("selection-change", (docId, { count, primary: {row, col} }) => { ... });
```

**节流设计**(关键):光标移动是高频事件,不能每键触发 JS。方案:
- **帧级节流**:每渲染帧最多 emit 一次(记录上次光标位置,变化且同帧未发过才发)
- 或**时间节流**:100ms 合并
- 推荐:帧级(Helix 渲染帧 ~30-60fps,天然节流),比时间节流简单且不丢最终位置

## 3. 实现

- **检测点**:render 帧里对比光标位置(编辑器当前 view 的 primary cursor)——不需要钩 keymap,渲染时 diff 即可
- helix-term:render() 里取 `doc.selection(view.id).primary()` 光标 → 与缓存比较 → 变化则 emit(JSON 通道,仿 buffers)
- helix-js:on 白名单加 "cursor-move"/"selection-change";emit 走现有事件机制(带 docId/位置参数)

## 4. 边界

- **节流必做**:无节流会每键触发 JS 调用(性能)。帧级节流上限 = 渲染帧率
- **终端叶子**:终端焦点时无光标事件(终端光标是网格,非编辑器)——仅编辑器模式
- **docId**:当前文档;多 view 二期

## 5. 待决

1. 帧级节流是否足够(打字连续移动,每帧 1 次 = 最高 60/s,JS 侧可接受?)
2. selection-change 是否需要独立事件,还是并入 cursor-move(带 selection 信息)

## 6. 规模

- helix-term 渲染 diff + emit + helix-js 事件(约 1 任务)
- 测试(节流行为、事件参数)(约 0.5 任务)
