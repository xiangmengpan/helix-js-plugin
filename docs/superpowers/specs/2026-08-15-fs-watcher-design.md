# 设计:文件系统 Watcher

日期:2026-08-15
状态:草案(待审核)
依赖:notify crate(workspace 暂无,需新增;上游 helix 的 git 集成用过 notify,版本兼容性需验证)

## 1. 动机

插件无法感知磁盘文件/目录变化。直接受益:
- filetree 自动刷新(外部 `mkdir`/`touch`/删除后树自动更新,不用手动 R)
- 外部修改自动重载(git checkout/脚本生成后 buffer 重载)
- git 状态实时(状态栏分支/改动数)

## 2. API

```js
// 监听路径(目录递归或单文件);返回 watcher id
const wid = helix.watch("/path/to/dir", (events) => {
  // events: [{kind:"create"|"modify"|"delete"|"rename", path}]
  filetree.refresh();
});

// 停止监听
helix.unwatch(wid);

// 监听当前文件(外部修改重载惯用法)
helix.watch(current_path, (events) => {
  if (events.some((e) => e.kind === "modify")) helix.reload_buffer();
});
```

## 3. 实现

- **Rust**:notify crate(debounced watcher);watch 请求入队(UiRequest::Watch { path, id });helix-term 持有 watcher 集合并注册回调通道(仿 term worker:变更 → 事件通道 → 主线程泵 → JS 回调)
- **事件桥接**:复用 async/term 事件泵模式(worker 线程发事件,WakeSender 唤醒,主线程 resolve 调 JS 回调)
- **JS 侧**:watch/unwatch 注册(返回 id;unwatch 按 id 移除)

## 4. 边界

- **回调节流**:notify debounce(如 500ms 合并)避免高频事件(批量文件操作)
- **路径**:回调携带绝对路径;JS 侧自行过滤(如忽略 .git)
- **生命周期**:watcher 随插件 reload 清理(watch 注册表在 reload 时重置)
- **性能**:目录大时递归监听开销——按需监听(插件决定),不全局监听

## 5. 待决

1. 是否做 `reload_buffer` 自动重载(Helix 原生无自动重载;需新增 API)还是只给事件、插件自己处理
2. debounce 时间默认值(500ms?)

## 6. 规模

- notify 依赖 + Rust watcher 管理 + 事件桥(约 1-2 任务)
- JS watch/unwatch + 测试(约 1 任务)
