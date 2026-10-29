# arsenal 改版设计（融合 helix 视觉 / 进度可见 / 操作优化 / 版本可选）

> **状态：P0+P1+P2 已实现（2026-10-30，arsenal `version = "2.0"`）**。
> 已知瑕疵：进程启动后**第一次** `:arsenal` 可能不绘制（再按一次或 resize 即可）；
> 根因是首个 JS 浮层的 `JsSurface` 首帧缓存（`PluginPopup::render` 首次 refresh 得到空行），
> 下一步在 `PluginPopup` 首帧强制 refresh 即可消除。
>
> 用户诉求四条：**① 样式和 helix 融合** · **② 安装要有进度条** · **③ LSP 等操作要顺**
> · **④ 有更新要显眼、并且能自己选版本**。
>
> 现状：`plugins/arsenal/plugin.js`（831 行）是自绘市场窗，功能齐（页签/搜索/批量/信息/版本输入），
> 但**视觉自成一套**（只用 `ui.virtual`/`ui.selection`/`ui.popup` 三个 scope、无边框、全英文）、
> **进度只有一个百分比数字**、**更新要靠手动 `:server update check`**、**版本只能手输**。

---

## 0. 一句话方向

> **arsenal 应该像 helix 的一部分**：借用主题 scope 与 dapui/which-key 的同一套视觉语法；
> **进度用"行内条 + 底部总条 + 状态栏"三处同时表达**；**更新在打开时就告诉你**；
> **装哪个版本由你从列表里挑**（而不是背版本号）。

---

## 1. 视觉融合（A）

### 1.1 口径（与现有 UI 对齐，不发明新颜色）

| 元素 | 用什么 | 依据 |
|---|---|---|
| 浮层边框 | `draw.rect({ border: "round", style: "ui.popup" })` | picker-ui / dapui 的输入框同款（浮层圆角，面板直角）|
| 标题 | 边框上嵌 ` arsenal · 80 recipes ` | picker-ui 的 `picker · <source>` |
| 页签条 | 当前页签 `▸` + `ui.text.focus`，其余 `ui.virtual` | dapui 底部三页同款 |
| 行文字 | 名称 `ui.text`／说明 `ui.virtual`／**状态列右锚** | dapui 变量树 |
| 选中行 | `ui.selection` + 最左 `›` 记号 | dapui（单色终端也看得见）|
| 关键标记 | `✓` 已装 · `▲` 可升级 · `–` 可装 · `⊙` 本机已有 · `✗` 失败 | dapui 断点/系统帧已用同族字形 |
| 页脚键位 | `ui.help`，单行、截断到宽度 | dapui 树/底部页脚 |
| 进度条 | `█`/`░` + `ui.text.focus`，宽 20 列 | 无现成样式，延续 dapui 的字符画风 |
| **不出现** | 硬编码 ANSI 色、emoji（🕐🔧）、全大写标题 | 与主题冲突 |

> 现在只有 3 个 scope、且**没有边框**：这是"看着不像 helix"的主因。

### 1.2 布局（高密度，一屏尽量多）

```
╭ arsenal · 80 recipes · ▲3 更新 ───────────────────────────────  lsp 12/80 ╮
│ all │▸lsp │ dap │ linter │ formatter │ installed │ local                     │
│──────────────────────────────────────────────────────────────────────────────│
│   name                    langs        status                                │
│ › L gopls                 go           ⊙ /usr/bin/gopls                      │
│   L rust-analyzer          rust         ✓ 2025-01-01  ▲ → 2026-10-26         │
│   L pyright               python       ████░░░░░░ 46% downloading            │
│   L clangd                c,cpp        ⊙ /usr/bin/clangd                     │
│──────────────────────────────────────────────────────────────────────────────│
│ 装中 1/3 · 46% ████████░░░░░░░░  gopls                                      │
│ j/k 移动 · / 搜索 · Enter 动作 · d 自检 · L 日志 · u 升级 · U 全部升级 · q 关 │
╰──────────────────────────────────────────────────────────────────────────────╯
```

- 顶部：标题 + 右侧"当前页签/总数"（右锚）
- 标题里带 **`▲N 更新`**（有更新时才出现，见 D）
- 底部：**任务行**（有任务时才占 1 行，见 B）+ 页脚键位
- 行密度：`name(24) langs(12) status(右锚 22)`；窄于 70 列时先砍 langs

### 1.3 本地化

`[plugins.arsenal] language = "en" | "zh-CN"`（默认跟随 `[plugins.which-key] language`）。
只翻"界面词"（页签、状态、键位提示），**不翻 recipe 的 description**（那是数据）。

---

## 2. 安装进度（B）

现状：`helix.server.task(items, cb)` 已有 `phase/progress/done/error` 事件，arsenal 只把它折成一个
`46%` 文本。目标：**三处同时可见 + 可取消 + 窗口不关。**

| 位置 | 内容 |
|---|---|
| **行内** | 该行状态列变成进度条：`████░░░░ 46% downloading`；排队中的行显示 `· queued` |
| **底部任务行** | `装中 1/3 · 46% ████████░░░░░░  gopls`；失败计数 `✗1` 用 `error` 样式 |
| **状态栏** | 一条摘要 `arsenal: 3 个任务（1 完成 / 1 进行 / 1 排队）`（只在窗口关掉时；窗口开着就够） |

行为约定：

1. **提交后窗口不关**（现在会关）——用户要看着进度；
2. **单任务串行、批量排队**（避免 pip/npm 同时抢同一目录）；
3. `Esc` 在任务中 = **不再取消整个窗口**，而是问"取消队列？(y/N)"；已下载的按"失败可回滚"处理
   （现有 install 已是 tmp + rename，天然安全）；
4. 完成/失败后**行状态原地刷新**（不要求 `r` 重刷），失败的保留 `✗ 原因`（来自 `last-error.json`）；
5. 进度事件缺失（Tool 类安装只报阶段不报字节）时，退化为"阶段文字 + 跑马灯 `▏▎▍`"。

需要的新 API（见 §6）：`helix.server.task(...)` 加 **`cancel(id)`**；事件加 `queue:{idx,total}`。

---

## 3. 操作优化（C）

### 3.1 现状痛点
- `Enter` 的动作菜单只有 install/update/remove/unmanage；**自检（doctor）/日志（log）/复制路径**要退出去打命令
- 装完只 echo 一句"installed"，**不知道接没接到语言上**（这正是"装了不生效"的来源）
- 80 条配方只能靠 `/` 搜名字，**没有"按语言过滤"**
- 批量标记 `t` 之后是"全部 install"，不能"全部升级"

### 3.2 目标键位（NORMAL）

| 键 | 动作 | 说明 |
|---|---|---|
| `j/k` `g/G` | 移动 | |
| `/` | 搜索（保持现状） | 进 SEARCH，Esc 清查询 |
| **`l`** | **按语言过滤**（循环：all → 当前文件的语言 → 各语言） | 80 条的必需品 |
| `Tab/S-Tab` | 页签 | |
| **`Enter`** | **智能动作**：未装→安装；已装→升级（有新版）/ 无动作则打开动作菜单 | 减少思考 |
| **`a`** | 动作菜单（install / update / reinstall / remove / unmanage） | `Enter` 的长按等价 |
| **`d`** | **自检**（`:server doctor <name>` → 结果进 buffer，窗口保留） | 新 |
| **`L`** | **日志**（`:server log <name>`） | 新 |
| `i` | 信息（路径/源/主页/上次失败原因） | 加"上次失败" |
| `y` | 复制：bin 路径 / homepage（弹两项） | 新 |
| `u` | 升级选中 | |
| **`U`** | **升级所有可升级项**（批量 + 进度） | 新 |
| `t` | 标记（批量） | |
| **`v`** | **选版本安装/降级**（见 E） | 新 |
| `x` | 卸载 / 取消本地挂接（现状） | |
| `r` | 刷新 | |
| `q/Esc` | 关窗（任务中→确认取消） | |

### 3.3 装完的"接线反馈"（关键）

任务 `done` 事件里带上 `rewrite` 的结果，arsenal 在列表下面追加三行（或结论气泡）：

```
✓ 已安装 ruff 0.17.3
  → 已接入 python 的 formatter（/…/managed/bin/ruff format -）
  ! 你的 languages.toml 里 rust 是自定义条目，需要手动加 language-servers
```

数据来源：核心 `server_manager::rewrite_languages_toml()` 已经返回 `LangRewrite{written, servers, conflicts}`，
把它塞进 task 的 `done` 事件即可（`helix.server.task` 的事件结构扩两个字段）。

---

## 4. 更新可见（D）

| 时机 | 行为 |
|---|---|
| 打开 arsenal | **后台**跑一次 `update check`（复用 24h 缓存，有缓存则 0 网络）；标题先显示 `… 检查更新`，回来后变 `▲3 更新` |
| 行内 | `✓ 2025-01-01  ▲ → 2026-10-26`（`▲` 用 `hint` 色） |
| 顶部 | `▲3 更新` 高亮；`U` 一键全升级 |
| 无网/无 `latest` 源 | 静默不显示（**不**谎报），底部页脚临时提示 `· 3 个配方不支持查更新`（按 `i` 看原因） |

实现要点：`helix.server.checkUpdates()`（新 API，后台任务，完成发 `server-checked` 事件），
arsenal 收到后重绘。**不阻塞**打开窗口（现在如果同步查，80 条会卡几秒）。

---

## 5. 版本选择（E）

### 5.1 交互

`v`（或"装 needs_version 的配方"时）弹**版本列表**（复用 arsenal 自己的子层，而不是自由文本）：

```
╭ 选择版本 · ruff ─────────────────────────────────────╮
│ › latest        0.17.3   2026-10-20                   │
│   0.17.2        3 天前                                 │
│   0.17.1        9 天前                                 │
│   0.16.0        1 个月前                                │
│   ……（最近 20 个）                                     │
│ 手动输入…                                              │
╰ j/k 选 · Enter 装 · m 手动输入 · Esc 取消 ────────────╯
```

- 默认选 **latest**；列表项显示 tag + 发布日期（有的话）
- 最后一项 **"手动输入…"** 回退到现在的文本输入（离线/私有源/预发布版）
- 已装版本在列表里标 `(已装)`

### 5.2 版本从哪来

| 来源 | 用法 |
|---|---|
| 配方 `latest = "github:owner/repo"` | `GET /repos/{owner}/{repo}/releases?per_page=30` → 取 `tag_name` + `published_at`（**不需要 token**，速率 60/h 够用）|
| 配方 `latest = "<url>"` | 该 URL 返回纯文本版本号 → 只有"最新"一个，列表退化为两项（最新 / 手动）|
| 网络失败 | 直接进"手动输入"，并提示 `查不到版本列表（网络/未配 latest）` |

**缓存**：`managed/versions.json`（每配方最多 30 项，TTL 6h），避免每次开窗都打 GitHub。

### 5.3 与"更新"的关系

升级 `u` 也走版本列表（默认选中"比当前新的最新一个"），这样"降级"顺便就有了。

---

## 6. 需要的 API / 核心改动（都很小）

| 需要 | 现状 | 改动 |
|---|---|---|
| `helix.server.versions(name)` | ✗ | 新增：核心查 GitHub releases（复用 `fetch_latest` 的 HTTP 栈）+ 6h 缓存，返回 `[{tag, date}]` |
| `helix.server.checkUpdates()` | ✗ | 新增：后台跑 `update check`，完成发 `server-checked` 事件 |
| `helix.server.task(..., {cancel: id})` | 只有提交 | 新增 `helix.server.cancel(id)`；事件加 `canceled` |
| task 事件带 `queue:{idx,total}` + `done.wiring` | 部分 | 扩字段（`wiring: {servers, conflicts}`）|
| `install(name, version)` 接受 `""` = latest | 已有（本轮修） | 不变 |
| doctor/log 结果进 buffer | 已有 `:server doctor` / `:server log` | arsenal 里按钮调 `helix.run_command("server", ["doctor", name])` 即可（无需新 API）|

> 全部是"加字段/加函数"，不改既有语义 ⇒ 现有 `:server` 命令行与脚本不受影响。

---

## 7. 实施计划（分三档，可分批交付）

| 档 | 内容 | 工作量 |
|---|---|---|
| **P0 视觉融合** | 圆角框+标题、页签 `▸`、`›` 选中记号、状态列右锚、页脚键位、本地化开关、去掉自成一套的色 | 0.5–1 天 |
| **P1 进度 + 操作** | 行内进度条、底部任务行、窗口不关、排队串行、取消确认；`d`/`L`/`y`/`a` 动作菜单、装完接线反馈、`l` 按语言过滤 | 1–1.5 天 |
| **P2 更新 + 版本** | 打开后台 check + `▲N 更新` 标题 + `U` 全升级；`v` 版本列表 + `versions.json` 缓存 + `server.versions`/`checkUpdates`/`cancel` 三个 API | 1.5–2 天 |

推荐顺序：**P0 → P1 → P2**（P0 立刻改善观感，P1 解决"装的时候没反馈"，P2 是新增能力）。

---

## 8. 风险与兼容

| 风险 | 处理 |
|---|---|
| 改键位造成肌肉记忆冲突 | `Enter` 保持"安装/升级"，其余是**新增**键（`d/L/y/a/v/U/l`）；旧键 `i/t/x/u/r/q` 全保留 |
| 全英文用户 | `language = "en"` 保持现状文案 |
| GitHub API 限流（60/h 未认证） | 6h 缓存 + 只在打开窗口/显式 `U`/`v` 时查；失败静默退化 |
| 任务中断（关窗/退出 hx） | install 本身是 tmp+rename，最多丢临时文件；下次 `:server status` 显示"上次失败" |
| 80 条列表性能 | 现有已按显示列宽算，行数固定可视区；过滤/搜索在内存里，无需优化 |

---

## 9. 验收清单

- [ ] 打开 arsenal：**圆角边框 + 标题 + 页签 `▸` + 页脚键位**，与 picker-ui/dapui 同一观感
- [ ] 深色/浅色两套主题下都不出现"硬编码色"（只用 `ui.*` scope）
- [ ] `:server install ruff`：**行内进度条**动，底部 `装中 1/1 · 46%`，窗口**不关**
- [ ] 装完出现**接线反馈**（"已接入 python 的 formatter"）
- [ ] 打开窗口 1s 内看到 `▲N 更新`（缓存命中）或"检查中…"（首查），**不卡 UI**
- [ ] `v` 弹出**版本列表**（含 latest/日期/已装标记/手动输入），装指定版本成功
- [ ] `d` 直接看到自检结论（可执行 / 可执行位 / 被哪个语言引用）
- [ ] 80 条配方下 `l` 按语言过滤能把列表收敛到当前语言
- [ ] 任务中按 `Esc` 弹确认；确认后队列取消，路由/目录无残留

---

## 10. 不做（明确边界）

- 不内置"配方编辑器"（改配方请编辑 `[server-manager.registry]` / 项目级 `recipes.toml`）
- 不做"自动更新"（后台静默升级）：用户须显式 `U`/`u`
- 不引入包管理器以外的下载通道（仍只有 pip/npm/cargo/go/官方 release）
- 不在 arsenal 里做 lint/format 的执行（那是 `:lint` / `:format`；arsenal 只负责"装/升/卸/自检"）


---

## 11. 落地记录（2026-10-30）

| 项 | 实现 |
|---|---|
| 版本 | `helix.plugin("arsenal", { …, version: "2.0" })` |
| ① 行内进度条 | `status_text()` 用 `████░░░░ 46%`（8 列），无字节进度时退化为阶段文字（剥掉核心的 `[i/N]` 前缀） |
| ① 取消 / 接线反馈 | `on_task_event` 新增 `canceled`（推进队列、汇总提示）与 `wiring`（解析核心的 JSON，echo `languages.toml updated · wired N · M 处需手动接线`） |
| ② 更新可见 | 打开时 `silent_update_check()` 对**已受管**项入队 `{op:"check"}`（核心侧 24h 缓存 ⇒ 通常瞬时）；标题变 `▲N 更新 · …`；`U` 一键升级全部可升级项 |
| ③ 操作 | 新键 `l` 语言过滤（循环，标题显示当前语言）、`d` → `:server doctor`、`L` → `:server log`、`y` 复制 bin 路径、`U` 全升级、`v` 版本列表；页脚同步更新 |
| ④ 版本可选 | `helix.server.versions(name)`（核心查 GitHub releases + 6h 缓存）→ 版本弹层：`› latest`、最近 20 个 tag + 日期、`manual input…`；选中即 `S.vinputs[name]=tag` 后走 install/update |

**随本轮一起进核心的支撑改动**（前面几轮踩坑的产物，都已单独提交）：

- `helix.server.versions(name)` / `helix.server.cancel(id)` / `task` 的 `check` op / `wiring`+`canceled` 事件
- `Editor::clear_status()` 的 2s 最短展示期（否则 LSP 进度每秒抹掉提示）
- 撤销 `helix.load` 里的二次宿主借用（曾导致**所有插件停摆**）
