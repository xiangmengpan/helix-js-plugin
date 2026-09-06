# arsenal(mason 式 LSP/工具管理市场界面)设计规格

日期:2026-09-06
状态:待审查
关联:server manager(`docs/superpowers/specs/2026-09-05-server-manager-design.md` 与实现计划)为能力后端;本规格只做"前台 UI + 喂数据扩展",不重写后端。

## 0. 决策记录(头脑风暴结论)

- **名字**:`arsenal`(军械库/工具库隐喻)。命令 `:arsenal`;插件 `plugins/features/arsenal/index.js`。
- **与现有 server manager 的关系**:三层已定——Rust 后端(`:server` 命令族/registry/languages.toml/本地识别,零重写) → JS 接口层(`helix.server.*`/`rows`,扩展新带回调 op) → UI 层(把现有 `:server-manager` rail 升级为 arsenal 市场窗)。`:server-manager` rail 与 `:server panel` 原生 picker 保留,`:server-manager` 标注 deprecated。
- **容器**:居中浮动大窗(最像 mason),非 rail。
- **进度**:后台线程执行 + 阶段事件 + **真实下载百分比**(下载层字节上报+节流)。
- **多选**:支持 tab 标记集合 + 批量串行队列。
- **描述**:registry 增 `description`/`homepage`,行内+详情都显示。

## 1. 目标

`:arsenal` 打开一个 mason 式"市场"窗口:搜索过滤、分类循环、行内状态(✓受管/⊙本机/▲可升级/–未装)、Enter 动作菜单、i 信息弹窗、tab 多选批量安装、后台执行带阶段与下载百分比进度反馈;安装结果自动写 languages.toml(沿用后端)。能力全部来自 server manager 后端。

## 2. 现状与约束(已核实)

- JS 组件树无鼠标点击命中(helix 限制,filetree 同款注释)→ **纯键盘 UI**。
- 现有 `helix.server.install/remove/update/unmanage` 是单向 `UiRequest::ServerOp`(fire-and-forget,结果写编辑器状态栏);`ServerListRows`/`rows` 提供行数据(无进度通道)。
- 后端 op 目前在主线程 job 同步执行,下载期间 UI 冻结 → 进度反馈与流畅 UI 必须**后台化**。
- `registry::Spec`(owned)无描述/主页字段;内置配方部分为"惰性占位"(无下载源);内置 rust-analyzer 为活配方(url 含 `{version}` 占位,无固定 version)。
- 上传中的 watch/server_rows 全局 mpsc + 回调注册表模式可复用为进度事件通道。

## 3. 架构

### 3.1 定位与入口

- 插件 `plugins/features/arsenal/index.js`,`helix.plugin("arsenal",{deps:[]})`;`init.js` 里 `helix.load("features/arsenal/index.js")`。
- 注册命令 `:arsenal`(toggle 市场窗)。旧 `:server-manager` 保留注册、doc 标注 deprecated(建议 arsenal),转发同能力;`:server panel` 与 `:server` 命令族不动。

### 3.2 窗口布局与键位

居中 popup,默认约终端宽 78% × 高 75%,`+/-` 调尺寸;主题用 `ui.popup/ui.selection/ui.virtual` 等现有 style。

```
┌ arsenal ───────────────────────────────────────────────┐
│ 搜索: [ra]                 LSP 2 · 已装 1 · 全部 7       │ 顶栏:标题|过滤输入|计数(随过滤)
│  L rust-analyzer  Rust 语言服务器    ✓ 2024-09-16       │ 行:kind图标 名字 描述(截断) 状态右对齐
│  L clangd         C/C++ 语言服务器   ⊙ 本机              │
│  L pyright        Python 语言服务器  ▲ 可升级            │
│  批量: 2 选中 ▮▮▮▯ 45% 安装中(1/2) rust-analyzer         │ 底栏:标记计数/队列进度条+阶段文案
│ j/k ↑↓移动 · 字符即搜 · Enter 操作 · i 详情 · f 分类     │
│ t 标记 · x 卸载 · u 升级 · +− 尺寸 · q/Esc 关闭          │
└────────────────────────────────────────────────────────┘
```

- 行状态:`✓`受管 `⊙`本机已有 `▲`可升级(受管&&recipe.version 有值&&!=检测版本)`–`未装;kind 图标 L/D/F/!。
- 搜索:整窗即搜(可打印字符进过滤,匹配名称/语言/描述),Backspace 编辑,Esc 先清空再关窗。
- 分类循环 `f`:`全部→LSP→DAP→Formatter→已装→本机→可升级`。
- Enter:无标记→当前行动作菜单;有标记→直接批量(集合快照,队列串行,底栏逐包汇总)。
- 动作菜单项:安装 / 升级(受管且有 version)/ 卸载(受管)/ 停用挂接(本机已有)/ 信息。
- i:信息弹窗(叠于市场):描述/主页/命令路径/语言/下载源/config version/当前状态。
- 版本输入:配方 url 含 `{version}` 且无默认 version(如内置 rust-analyzer)时,安装动作先弹版本输入框(如 `2024-09-16`),填完再执行。
- x 无二次确认(与 `:server remove` 一致,结果可见于底栏);批量不取消(v1)。
- **快捷键与菜单关系**:Enter 是主入口(弹动作菜单),`x/u/i/f` 为直达快捷键(跳过菜单直卸/直升/详情/分类循环);菜单与直达并存不冲突。

### 3.3 数据层扩展

- `registry::Spec` 增 `description: String` 与 `homepage: Option<String>`;内置配方填充见 M5;config `[server-manager.registry.<name>]` 可选解析 `description`/`homepage`(缺省空)。
- `ServerRow`(term→JS)增 `description: String`、`homepage: Option<String>`;现有 name/kind/languages/installed/local/version/installable 不变。
- "可升级":受管已装 && `spec.version.is_some()` && `spec.version != version_detected()`(字符串比较;不做 semver)。
- 行渲染的描述截断按列宽;详情弹窗完整显示。

### 3.4 后台任务与进度事件

- **通道**:复用 watch/server_rows 模式——JS 注册回调获得 taskId → push `UiRequest::ServerTask{batch: Vec<(op,name,version?)>}` → term 建**单 worker 队列**串行执行(避免并发写 managed/languages.toml)→ 线程经全局 mpsc 推 `ServerTaskEvent` → 主线程泵点(与 watch resolve 同处)调 JS 回调。
- **事件协议**:`{ id, kind: phase|progress|done|error, name, seq, msg?, bytes?, total? }`
  - phase:下载中/校验/解压/落盘/执行(tool 类无百分比 → UI spinner/阶段文案)。
  - progress:`bytes`+`total?`(Content-Length 缺失则无百分比,只显示字节);**节流**≥256KB 或 ≥80ms 一条。
  - done:成功(含结果路径);error:失败原因。事件均带 `name`(批量区分)与 `seq`。
- **下载层**:`download_to(url,dest,progress_cb)`——ureq reader 手写读循环计数;file:// 复制整文件单事件(无中间进度)。节流聚合器独立小函数(可单测)。
- 每任务完成即 rewrite languages.toml(幂等;批量中间多次写可接受,v1 求简单)。
- 现有单向 `helix.server.*`/`ServerOp` **保留不改**,arsenal 用新的带回调 API **`helix.server.task(specs, cb)`**(命名定稿;specs = 数组 {op,name,version?} 批量或单个;返回 taskId,事件经回调送达)。

### 3.5 UI 流程(进度/刷新)

- 行进行中:该行显示阶段/百分比覆写(spinner 或 `▮▮▮▯ 45%`)。
- done/error 后:JS 自动重新 `rows()` 刷新列表与计数;错误显示于底栏(可读原因)。
- 底栏批量区:标记计数 + 当前包名 + 进度条 + `(已完成/总数)`;批量结束自动收起。

## 4. 错误与边界

| 场景 | 行为 |
|---|---|
| 下载 404/sha 不符/解压缺 bin | error 事件带原因,行保持原状态,底栏展示;不留半成品(后端已保证) |
| 升级失败 | 保留旧版(后端 .bak 回滚);error 事件;languages.toml 不动 |
| 配方无 version 且需 `{version}` | 版本输入框;取消 = 不执行 |
| 本机已有(local) | install 显示"本地已可用",不下载;x/unmanage 语义同 `:server` |
| 语言已用户自定义 | 沿用后端冲突提示(hint),UI 底栏展示 |
| 多选包含不可装(no source) | 跳过并汇总"跳过 n(无源)",done 事件列表展示 |
| 单个 taskId 已失效(Esc 清空?) | v1 回调保持到完成,事件无处投递则丢弃(与 watch 同) |
| 取消批量 | v1 不支持(文档注明);后续按队列 drain 实现 |

## 5. 测试计划

- 单测(term):下载进度聚合器(读流→计数/节流/单调至 100%);可升级判定矩阵;config `description/homepage` 解析;ServerRow 新字段。
- 单测(js):`task` 回调收到 phase→progress→done 序列;批量 seq 顺序;error 事件文本。
- 集成(integration,file:// 假源 + SM vars 隔离):装→底栏进度行出现(至少一次 progress/phase)→done→行变 ✓ 且 languages.toml 含条目;升级与卸载;版本输入流;tab 多选批量顺序;UI 冒烟(开关/搜索/`f` 分类/`i` 详情打开关闭)。
- 回归:`server_*` 现有 8 项集成不回归;helix-term/helix-js/helix-view lib;fmt/clippy 干净。

## 6. 里程碑

1. M1 数据层:Spec desc/homepage + ServerRow 字段 + config 解析 + 可升级判定(单测)。
2. M2 后台任务/进度通道:worker 队列、事件、下载进度+节流、JS task op(单测+集成)。
3. M3 arsenal 主视图:窗口/列表/即搜/分类循环/导航/tab 标记(集成冒烟)。
4. M4 操作流程:动作菜单/版本输入/信息弹窗/批量进度 UI(集成)。
5. M5 收尾:内置配方描述数据 + 文档 + 旧命令 deprecate + clippy/fmt/全量回归。

每期独立可交付;M2 完成后端能力即具备,UI 可并行依赖 M3 的 rows。

## 7. 文档

- `docs/arsenal.md`:用法/键位/config 字段/架构简述/旧 UI 迁移。
- `docs/server-manager.md` 与 `docs/plugin-api.md` 各加指向行。

## 8. 风险

- **无点击命中**:所有交互纯键盘,mason 用户习惯(mouse/tab 多点)需键位引导;底栏帮助行缓解。
- **版本输入的体验**:远端无版本列表接口,只能手填 tag(如 rust-analyzer 周版);失败给可读错误,建议填 config `version` 后由 `:server update` 全量接管。
- **下载无 Content-Length**(部分 CDN):进度退化为字节/旋转动画;不阻塞功能。
- **批量与 languages.toml 多写**:幂等;若性能问题再聚合到队列尾一次 rewrite(风险低,不预做)。
- **线程与 env/全局**:进度线程只读 env/registry(已 immutable/OnceLock 化);写路径全部串行于单 worker,规避并发写。
