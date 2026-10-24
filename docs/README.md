# 文档索引

> **找不到东西时先看这里。**
> 使用手册在 [`api/`](api/) 与 [`guide/`](guide/);**历史设计记录在 [`superpowers/`](superpowers/) —— 不要当作当前行为依据**。

## 一、使用手册(写插件 / 用 API)

| 文档 | 内容 |
|---|---|
| [`plugin-api.md`](plugin-api.md) | **插件 API 总览与索引** —— 从这里进 |
| [`api/`](api/) | 按域详细(签名 / 示例 / 优缺点):`plugin` · `events` · `loading-commands` · `config` · `ui` · `editing` · `lsp` · `fuzzy` · `terminal-layout` · `picker-theme` · `process-fs` |
| [`plugin-layout.md`](plugin-layout.md) | **插件布局与命名约定**:一个插件 = 一个目录 + `plugin.js` · 入口如何点名 · `deps` 用插件名 · **两层覆盖**(内置 / 用户) · 分发(`contrib/install-plugins.sh`) |
| [`config-precedence.md`](config-precedence.md) | **配置优先级**:`config.toml` ↔ JS 谁赢 / `:config-reload` vs `:plugin-reload` 各管什么 / 什么时候写哪儿 |
| [`plugin-vs-core.md`](plugin-vs-core.md) | **该插件化 vs 该留核心** —— 判定清单:三闸门 + 分类判定表 + 四条红线(每条对应一次真实事故)· 附实测数字出处 |
| [`guide/`](guide/) | 教程 01–10:概览 → 启动 / 按键 / 编辑 / 渲染 → 深入(core / view / term / lsp) |
| [`../plugins/helix.d.ts`](../plugins/helix.d.ts) | 类型定义(编辑器补全用)。改动后用 `tsc --noEmit` 验 |

## 二、主题文档

| 文档 | 内容 |
|---|---|
| [`architecture.md`](architecture.md) | 分层与数据流(JS 运行时 / compositor / 布局树) |
| [`architecture-audit.md`](architecture-audit.md) | **架构体检 → 重构执行目标(G0–G12,逐项打勾)** —— 唯一进度真相源;改动前看 §2 铁律 |
| [`vision.md`](vision.md) | 这个 fork 想做成什么 |
| [`explicit-host-inventory.md`](explicit-host-inventory.md) | **隐式宿主重构总结**(3a / 3b / 4a L1–L4 收口):分层、可复跑度量、残余边界、踩过的坑 |
| [`architecture-plugin-engine-bench.md`](architecture-plugin-engine-bench.md) | 插件引擎横评(boa / QuickJS / V8):release 数字 + "现在不换引擎"的结论 |
| [`arsenal.md`](arsenal.md) | 浮层市场窗(`:arsenal`)—— 取代已废弃的 server-manager |
| [`server-manager.md`](server-manager.md) | **已废弃的插件**与其**仍在使用的后端**(注册表/安装/版本求解) |
| [`releases.md`](releases.md) | 发布记录 |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | 贡献指南 |

## 三、历史归档(记录**当时**的决定)

[`superpowers/`](superpowers/) —— 设计规格(`specs/`)、实现计划(`plans/`)、交接记录(`handoff/`),约 148 个文件。

用途是回答**"为什么这么设计"**;**不是**当前 API / 路径 / 行为的依据 ——
其中的名字与路径可能已过时(例如旧扁平 API 已被 `pane.*` / `buffer.*` / `layout.*` 取代,
`plugins/features/xxx.js` 已迁为 `plugins/<name>/plugin.js`)。

## 四、仓库根

- [`../README.md`](../README.md)(English) · [`../README.zh-CN.md`](../README.zh-CN.md)(中文)
- [`../plugins/README.md`](../plugins/README.md) —— 插件目录本身的说明
