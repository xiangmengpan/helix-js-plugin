# API：进程内模糊匹配（`helix.fuzzy`）

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

nucleo（与核心 picker 同一套）在**进程内**打分。**会话式**：条目只搬一次，之后每次查询只传 `query`。

```js
const views = all.map((it) => String(it));   // items 必须全是字符串
const sid = helix.fuzzy.session(views);      // 条目集变化时才重建

// 每次按键：
const hits = helix.fuzzy.run(sid, query, { limit: 200 });
const shown = hits.map((h) => all[h.index]); // index 指回 session 的下标

helix.fuzzy.drop(sid);                       // 弹窗关闭 / 换数据源
```

| API | 说明 |
|---|---|
| `session(items)` | 建快照，返回 `id`。非字符串项**报错**（不静默跳过） |
| `run(id, query, opts?)` | 返回 `[{index, score}]`，按 score 降序；同分按下标升序（稳定、可复现）。`query` 为空 ⇒ `[]`；未知 `id` ⇒ 抛错 |
| `drop(id)` | 释放快照；未知 id 是 no-op |

`opts.limit`：排序**之后**截断（默认 200，`0` = 不限）。`opts.case`：`"smart"`（默认）/ `"insensitive"` / `"sensitive"`。

## 为什么是会话式（实测，不是设计偏好）

| 形状 | 5000 条 / 次 | 说明 |
|---|---|---|
| 手写 JS 打分（picker-ui 旧路径） | 7.1 ms | 每次按键都跑 |
| `filter(query, items)` 一次性 | **11.0 ms** | 5000 个字符串逐项过边界，**比打分本身还贵** |
| `session` + `run` | **2.0 ms** | 条目只搬一次；其中 nucleo 打分约 1.4ms |

教训：把工作搬进核心时，**跨越边界的数据量**才是主成本。会话句柄把"每次按键搬 5000 条"变成"每次按键搬 1 条 query"。

## 什么时候该用

| 条目数 | 建议 |
|---|---|
| < ~1000 | 手写打分够用（排序手感也更接近插件自己的规则） |
| ≥ ~1000 | 用本 API（`picker-ui` 的 `core_match_min` 默认 1000） |

## 局限

- 排序规则是 **nucleo 的**，与"手写子序列 + 词首奖励"不完全相同（换档时排名会变）。
- 不读文件、不缓存结果：每次 `run` 都从头打分（会话里只缓存**条目**）。
- 不返回匹配位置（高亮要位置的话目前仍需自己算）。
