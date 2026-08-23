# 设计:Buffer 遍历 API(buffers / focus_buffer)

日期:2026-08-15
状态:草案(待审核,未执行)
关联:docs/superpowers/specs/2026-08-15-js-ui-rendering-design.md(JS 视图层)

## 1. 背景与动机

当前插件只能操作**当前 buffer**(命令 ctx 的 doc;`set_cursor`/`doc_*` 单文档)。缺:
- 列出所有打开的 buffer(路径/状态)
- 按 id 跳转/聚焦任意 buffer
- 批量操作基础(遍历)

直接受益:buffer 标签条(布局标签条只能显示叶子,不能显示多 buffer)、自定义 buffer 选择器、批量统计。

## 2. 设计

### 2.1 API

```js
// 所有打开的 buffer 快照(只读,调用时实时生成)
helix.buffers()
// → [{ id, path, name, dirty, modified, language }]
//    id: 文档 id(数值,本会话内有效;跳转/后续操作使用)

// 聚焦指定 buffer(当前 view 切换到该文档;id 失效 → 报错)
helix.focus_buffer(id)

// 当前 buffer id(快捷)
helix.current_buffer()
// → id
```

### 2.2 数据字段

| 字段 | 类型 | 来源 |
|---|---|---|
| `id` | 数值 | 文档内部 id(稳定,会话内;关闭后失效) |
| `path` | string/null | scratch 无路径 → null |
| `name` | string | 文件名或 "[scratch]" |
| `dirty` | bool | 未保存修改 |
| `modified` | bool | 磁盘 mtime 变更(外部修改) |
| `language` | string | 语言名(可为空) |

### 2.3 实现

- **buffers()**:helix-term 遍历 `editor.documents`(HashMap),serde 序列化为 JSON 数组(仿 `cache_layout` 的 JSON 通道,无 boa 依赖);helix-js `js_buffers` JSON.parse 返回。**实时生成,无缓存**(文档数通常 < 50,序列化 µs 级)。
- **focus_buffer(id)**:入队 `UiRequest::FocusBuffer { id }`(仿 open_file/apply_plugin_edits 通道);helix-term 校验 id 存在 → 当前 view 切换文档(`Action::Switch` 或等价);不存在 → error 提示。
- **current_buffer()**:helix-term 序列化当前文档 id(复用同一 JSON 通道,单值)。

### 2.4 事件联动(已有,无需新增)

- `buffer-open` / `buffer-close` 已存在 → 标签条/选择器可监听刷新
- 文档变化(`doc-change`)已有 → dirty 刷新

## 3. 边界与安全

- **只读快照**:buffers() 返回快照数组,不含可变引用;写入/切换仍走现有 API(focus_buffer / open_file / doc_*)
- **id 生命周期**:文档关闭后 id 失效;focus_buffer(失效 id)返回错误(不 panic);插件应监听 buffer-close 清理
- **并发/时序**:buffers() 调用时同步快照,与事件循环一致(无异步竞态;同一 tick 内 buffers() 与 focus_buffer 顺序调用语义明确)
- **性能**:文档数 × 小 JSON,调用时生成;无每帧调用场景(插件按需调用)

## 4. 与现有系统的关系

- 复用:JSON 传递通道(cache_layout/get_component_state 同款)、UiRequest 队列(focus_buffer 入队)、buffer-open/close 事件
- 不冲突:get_layout(布局叶子)与 buffers(文档)正交——叶子对应 view/buffer,多 buffer 可在一个叶子切换
- 新增面:helix-js 2 个函数(buffers/focus_buffer)+ current_buffer;helix-term 序列化 + FocusBuffer UiRequest 处理

## 5. 用例

```js
// buffer 标签条(顶部槽位,显示所有打开文档)
helix.set_component_render(TABBAR_ID, () => {
  const bs = helix.buffers();
  return { type: "row", children: bs.map((b) => ({
    type: "text",
    text: (b.dirty ? "● " : "  ") + b.name + " ",
    style: b.id === helix.current_buffer() ? "ui.selection" : "ui.statusline.inactive",
  })), gap: 1 };
});

// 自定义 buffer 选择器
helix.buffers().filter((b) => b.path && b.path.includes("src"));
helix.focus_buffer(target.id);
```

## 6. 待决问题

1. `buffers()` 是否含**不可见/scratch** 缓冲(建议:全部,插件自行过滤)
2. `focus_buffer` 语义:当前 view 切换(建议)/ 或跳转目标叶子(多 view 场景二期再说)
3. 是否需要 `on("buffer-change")` 批量事件(缓冲增删节流)?本期不做,插件用 buffer-open/close + 按需轮询
4. id 类型:数值(建议)vs 字符串

## 7. 规模评估

- helix-js:2-3 个 API + 单测(约 1 个任务)
- helix-term:序列化 + FocusBuffer 处理 + 集成测试(约 1 个任务)
- 总计 2 个任务,可独立交付
