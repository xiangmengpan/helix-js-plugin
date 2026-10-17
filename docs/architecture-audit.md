# 架构体检 → 重构执行目标(逐项打勾)

> **本文是什么**:2026-10 架构体检的结论 + 一份**可逐项打勾执行的清单**。
> **本文不是什么**:不是当前行为依据(行为看 `docs/plugin-api.md` / `docs/architecture.md`),
> 也不是设计规格(C 批的每一项在动手前**另写** spec,放 `docs/superpowers/specs/`)。
>
> **体检方法**(可复现,详见 §1):
> `cargo clippy -W clippy::too_many_lines` + 花括号配平量函数长度 + 归一化 20 行窗口查跨文件重复
> + 与 upstream merge-base(`079a789e8`)对比区分「fork 新增」/「上游遗留」。

---

## 0. 状态看板(唯一进度真相源)

标记规则:**一次只做一项**;门禁全绿才允许把 ☐ 改成 ☑。完成时同步更新本表与 §8 记录。

图例:✅ 完成 · ☐ 未开始 · 🟡 **spec 已出、等人确认**(C 批按铁律 5 必须先过这一步)

| 状态 | ID | 目标 | 严重度 | 批次 | 性质 | 依赖 |
|---|---|---|---|---|---|---|
| ✅ | **G0** | 建立干净基线:收尾当前 WIP + 清 `.swp` | — | 前置 | 卫生 · 2026-10-17 · `fc0585abe` + `49c4fe1` + 本条 | 无 |
| ✅ | **G1** | `helix-js` 内联测试(4,835 行 / 99 测试)搬出 `lib.rs` | S1 | A | 纯搬家 · 2026-10-17 · 本条 | G0 |
| ✅ | **G2** | 集成测试 harness(`pump`/`render_rows`)收进 `helpers.rs` | S2 | A | 纯减法 · 2026-10-17 · 本条 | G0 |
| ✅ | **G3** | `state.rs` 收窄公共面 + `*_for_test` 收口(实测重定义见 §3) | S3 | A | 纯减法 · 2026-10-17 · 本条 | G0 |
| ✅ | **G4** | 消歧命名:`helix-js/src/layout.rs` → `layout_api.rs` + 模块地图 | S2 | A | 重命名 · 2026-10-17 · 本条 | 无 |
| ✅ | **G5** | `xtask` 一任务一文件 + 单张 `TASKS` 表驱动 help | S2 | A | 纯搬家 · 2026-10-17 · 本条 | 无 |
| ✅ | **G6** | `build_engine`(773 行)按命名空间/能力切分到 `register/` | S1 | B | 搬家 · 2026-10-17 · 本条 | G1, G3 |
| ✅ | **G7** | `apply_ui_requests`(689 行)按请求族切分 | S1 | B | 搬家 · 2026-10-17 · 本条 | G3 |
| ✅ | **G8a** | `server_manager` 移出 `commands/`(内容零改动) | S2 | B | 搬家 · 2026-10-17 · 本条 | 无 |
| ✅ | **G8b** | `server_manager/mod.rs`(2,956 行)按职责拆 7 个模块 | S3 | B | 搬家 · 2026-10-17 · 本条 | G8a |
| ✅ | **G9** | JS 插件共享层 → **已评估:收益不足,不抽**(实测只有 4 组/35 行逐字重复) | S3 | C | 评估结案 · 2026-10-17 · 本条 | 无 |
| ✅ | **G10** | `PluginPanel`/`PluginPopup` 抽 `JsSurface`(terminal 另议) | S2 | C | 真重构 · 2026-10-17 · 本条 | G6 ✅ |
| ✅ | **G11** | 双状态所有权收尾 → **所有权地图 + ACTIVE 单一读点**(迁移判为不该做) | S1 | C | 评估+收口 · 2026-10-17 · 本条 | G3 ✅ |
| ✅ | **G12** | `job::dispatch_blocking` 闭包派发 → `UiOp`(pane 族 24 处) | S3 | C | 真重构 · 2026-10-17 · 本条 | G7 ✅ |

范围默认:**只治 fork 新增债务**(16 个 >100 行函数 / 1,486 行超额)。上游遗留的 54 个长函数
(`dap.rs` 439 行 `handle_debugger_message`、`keymap/default.rs` 383 行等)**不动** —— 见 §6 决策 D1。

---

## 1. 基线与复现命令(改完用同一组命令对比)

```bash
# ① 长函数(权威口径)
cargo clippy --workspace --all-targets -- -W clippy::too_many_lines
#    基线:70 个 >100 行函数(fork 新增文件 16 个 / 上游文件 54 个)

# ② 单个 crate 的测试与注册面(等价性证据)
cargo test -p helix-js                                    # 基线:176 个 #[test](G1 后位于 src/tests/ 七个域文件)
cargo xtask api-check
#    基线必须不变:134 个成员 / 20 个事件 / 15 个能力检查点 · 豁免 1
#    ⚠ 已知小漂移:两个脚本对**命名空间**计数不一致 —— helix.d.ts 那份报 10、文档那份报 9,
#      成员数两者一致(134)⇒ 门禁绿但计数口径没统一。属文档/脚本层面的小 bug,不阻塞任何目标。

# ③ 集成测试(全量基线:377 passed / 0 failed / ~220s;文档旧数 367 已过期,G2 实测更正)
cargo integration-test

# ④ 统计脚本(Python,不入库;需要时重建)
#    - 逐 fn 花括号配平量长度 → 与 clippy 交叉验证
#    - 归一化(去注释/压空白)后 20 行窗口哈希 → 跨文件重复块
```

**关键基线数字**(任何一项变化都要能解释):

| 指标 | 基线 |
|---|---|
| `helix-js/src/lib.rs` | 5,693 行 = 858 生产 + **4,835 测试**(`#[cfg(test)]` 在第 859 行) |
| `helix-js` 公共面 | **190** 个 `pub fn`(145 个 `js_*` 前缀) |
| `helix-js/src/state.rs` | 1,230 行 / **106** 个 pub 访问器 / 6 个进程级 `OnceLock` + 12 个 `thread_local!` |
| `UiRequest` | **92** 个变体,16 个文件 match,消费方单函数 **689** 行 |
| `plugin_panel` ↔ `plugin_popup` | 368 / 400 行,**30 个相同 20 行块**,差异仅 276 行 |
| 集成测试重复 | 12 个文件各自定义 `pump`/`render_rows`,两两共享 13–15 个 20 行块 |
| `job::dispatch_*` 调用点 | 85 处(helix-term) |
| **基线 HEAD** | `fc0585abe`(工作区干净;2026-10-17 实测 fmt / clippy -D warnings / `test -p helix-js` / `api-check` 全绿) |

---

## 2. 铁律与「完成」的定义

**铁律**(违反任一条即回退,不许"改着看")

1. **一项一个提交**。批次内不合并;失败就 `git revert`,不带病往下。
2. **不顺手改逻辑**。A/B 批只允许"移动 / 收窄 / 重命名",出现 `if` 条件变化就是做错了。
3. **等价性要有证据**:注册成员数、`#[test]` 数量、集成测试数量三者必须与基线一致(除非该项明确要改)。
4. **先量规模再承诺**。开工前先数要改的点(文件数 / 调用点),写进该项的"预估"里;实际超过 2 倍即停下来报告。
5. **C 批先写 spec**。spec 落 `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`,写完**等人确认**再改代码。
6. 不碰上游遗留长函数;不碰 `CHANGELOG.md`;不改 `runtime/` 下已安装副本(改 `plugins/` 后跑 `contrib/install-plugins.sh`)。

**每项「完成」必须同时满足(DoD)**

```
[ ] cargo fmt --all --check                         绿
[ ] cargo clippy --workspace --all-targets -- -D warnings   零新增
[ ] cargo test --workspace --lib                    零失败(D8 起纳入;此前 --lib 一直是红的)
[ ] 该项 §验证 里列出的命令全绿
[ ] 等价性数字与 §1 基线一致(或差异已在完成记录里解释)
[ ] 单个 commit,消息形如 refactor(js): 把内联测试搬出 lib.rs
[ ] 本文档:该项 ☐ → ☑,§0 表格填 ✅ + 日期 + commit sha,§8 追加一行
```

---

## 3. 批次 A —— 零行为变化(前置 G0 已完成)

### ☑ G0 · 建立干净基线(2026-10-17 完成)

- **做了什么**
  1. 原工作区的 12 个未提交文件(能力检查点收口 + 新增 `plugins/tools/check_capability_surface.mjs`)
     已被**并行进程**提交为 `fc0585abe`(`feat(js): 能力检查点机械对账 … + 补掉两个真缺口`)。
     本次只做核对,未重写该提交。
  2. `.gitignore` 增加 `*.swp`(带注释)→ `49c4fe1`。**只忽略、不代删** —— 仓库根的 `.swp`
     可能含别人的恢复数据,文件保留在原处。
  3. 本文档 + `docs/README.md` 索引 → 本条提交。
- **基线门禁(实测)**
  | 门禁 | 结果 |
  |---|---|
  | `cargo fmt --all --check` | 绿 |
  | `cargo clippy --workspace --all-targets -- -D warnings` | 绿,0 warning / 0 error |
  | `cargo test -p helix-js` | **176** 测试:170 passed / 0 failed / 6 ignored(+1 doc test) |
  | `cargo xtask api-check` | 绿:134 成员 / 20 事件 / 15 能力检查点 · 豁免 1 |
- **遗留观察(不阻塞)**
  - 命名空间计数在两份脚本里是 10 vs 9(见 §1 注)。
  - 仓库有 **2 个已存在的 stash**(`user-wip-application` 与基于 `bbb090d7e` 的 WIP),**未动**。
  - ⚠ 本工作区存在**并行进程共同提交**(`fc0585abe` 即由其代提交)。后续每项提交都用
    `git commit <paths>` 限定路径,避免把别人的改动卷进我的提交。

### ☑ G1 · 把 `helix-js` 的内联测试搬出 `lib.rs`(2026-10-17 完成)

- **症状**:`lib.rs` 5,693 行里 4,835 行是 `#[cfg(test)] mod tests`(99 个测试)。生产代码只占 858 行,却和测试混在一起 —— 阅读 9 个命名空间的注册表要翻过 4.8k 行测试。
- **做法(实际执行)**
  1. `lib.rs` 末尾只留 `#[cfg(test)] pub(crate) mod tests;` —— 变成**文件化模块**,`crate::tests` 路径与可见性都不变。
  2. 按**测试名归属**把 99 个测试逐字搬进 7 个域文件(每条按原行区间整块搬迁,不改断言语义):
     `plugin`(29)· `popup_panel`(26)· `term_pty`(11)· `doc_edit`(10)· `pane_stack`(9)· `fs_async`(8)· `events_lsp`(6)。
  3. 共享夹具(`TEST_LOCK` + `describe_term_event` / `wait_for_term_event` / `wait_for_async` + 3 条 `use`)
     全部留在 `mod.rs`。子模块只写 `use super::*;` —— **父模块的私有 `use` 对子模块可见**,
     所以 `use crate::*;` 是多余的(先试过加它,编译器报了 7 条 `unused import` 后删掉)。
- **偏离计划的地方**:原计划的文件名(`api/popup/layout/fs/term/lsp`)与真实测试名对不上(测试里
  `plugin`/`reload`/`popup`/`pane` 四族占 74/99),所以改用按真实族名命名。文件数 7 不变。
- **完成判据(实测)**
  | 指标 | 基线 | 现在 |
  |---|---|---|
  | `lib.rs` 行数 | 5,693 | **860**(生产 858 + 2 行模块声明) |
  | `cargo test -p helix-js` | 176 / 170 passed / 6 ignored | **完全相同** |
  | 99 个测试名 | — | 逐个核对:每个恰好出现 1 次(0 丢失 / 0 新增 / 0 重复) |
  | `cargo fmt --all --check` | 绿 | 绿 |
  | `cargo clippy --all-targets -- -D warnings` | 0 warning | **0 warning** |
  | `cargo xtask api-check` | 134 成员 / 15 检查点 | **完全相同** |
- **顺手发现的真问题(不属本项,已记 D5)**:6 个 `#[ignore]` 测试里有 **2 个本来就是坏的** ——
  `reload_real_statusline_render_ok` / `reload_all_real_plugins_keeps_commands` 去拷
  `plugins/features/statusline.js`,而该路径早已迁成 `plugins/statusline/plugin.js` ⇒
  `Os { code: 2, NotFound }`。因为 `#[ignore]`,CI 从不跑它们,所以烂了没人发现。
**风险**:无(纯搬家;回退 = `git revert`,不跨 crate)。

### ☑ G2 · 集成测试 harness 收进 `helpers.rs`(2026-10-17 完成)

- **症状**:12 个测试文件各自重造 `async fn pump(&mut Application, &str)` 与 `fn render_rows(...)`,
  两两共享 13–15 个 20 行块;而 `tests/test/helpers.rs` 已经存在却没用上。
- **先量规模(铁律 4)**:把 19 个定义逐个抽出来归一化哈希,结果不是 19 份相同副本:
  | 函数 | 副本数 | 版本数 | 差异 |
  |---|---|---|---|
  | `pump` | 12 | 3 | 仅字面差异:`parse_macro(keys)?` vs `.into_iter()`(Vec 迭代等价)、`UnboundedReceiverStream` 全路径 vs 导入 —— **语义相同** |
  | `render_rows` | 7 | 3 | `plugin_components` 那份**少了 `reset_plugin_diffs()`**,其余 6 份仅差一行注释 |
  关键发现:`plugin_components::render_rows` **从未被调用**(死代码,且因 `mod test` 在
  `#[cfg(feature = "integration")]` 下,不带 feature 的 clippy 看不到它)。
- **做法**
  1. `helpers.rs` 追加 `pub async fn pump` 与 `pub fn render_rows`(带 `reset_plugin_diffs()`,
     并注释为什么必须 reset:DiffRenderer 有状态,不 reset 时第二次渲染只画变化单元)。
  2. 删掉 19 个本地定义及其上方注释行;`plugin_theme.rs` 补 `use super::*;`(它原先没挂
     父模块 glob,靠自己那份 `pump`)。
  3. 用编译器的 `unused import` 警告驱动清理(48 条 / 12 文件),`#[cfg(windows)]` /
     `#[cfg(not(windows))]` 的 `Event`/`KeyEvent` 成对删除 —— 否则 Windows CI 才会红。
- **偏离计划的地方**:计划说「各文件改 `use super::helpers::*;`」,实际无需新增任何 import ——
  `integration.rs` 的 `mod test` 已有 `use self::helpers::*;`,而**父模块的私有 use / glob
  对子模块可见**(同 G1 的发现)。只有 `plugin_theme.rs` 例外(它连 `use super::*;` 都没有)。
- **完成判据(实测)**
  | 指标 | 基线 | 现在 |
  |---|---|---|
  | `cargo integration-test` | **377 passed / 0 failed**(223.5s;本次实测修正了文档旧数 367) | **377 passed / 0 failed**(218.5s) |
  | 12 个文件里的 `fn pump(` / `fn render_rows(` | 19 个定义 | **0**(只剩 `helpers.rs` 2 个 + 2 个名字相近但语义不同的 `pump_mouse` / `render_rows_probe`) |
  | 行数 | — | **+47 / −352(净 −305)** |
  | `cargo fmt --all --check` / `clippy -D warnings` | 绿 | 绿(0 warning) |
- **顺手发现的真问题(不属本项,已记 D6)**:`cargo clippy --features integration -- -D warnings`
  **在改动前就是红的** —— `application.rs:2387 plugin_roots_for` 只在
  `#[cfg(not(feature = "integration"))]` 下被调用,开 feature 编 lib 时成 dead code。
  现有门禁不带 feature 所以没暴露(与 G2 无关,单行可修)。
- **风险**:`server_manager` 一族共享进程级 `SM_*` 环境变量(串行)—— 未动其串行化。

### ☑ G3 · `state.rs` 收窄公共面 + `*_for_test` 收口(2026-10-17 完成)

- **先量规模(铁律 4)——三条体检结论被实测推翻**
  | 体检原话 | 实测 | 结论 |
  |---|---|---|
  | `state.rs` 有 **106 个 pub 访问器** | **31 个 `pub fn` + 75 个 `pub(crate) fn`** = 106 个访问器 | 数字对、性质错:不是 106 个公共 API |
  | `*_for_test` 以 `pub` 泄漏 | `permissions::reset_for_test` / `host::clear_host_for_test` **本来就是 `pub(crate)`** | 无需收口,只补文档 |
  | 公共 `pub fn` 190 → ≤60 | 顶层 `pub fn` **141**;其中 **111 个被 helix-term 按名字使用**,单元测试还额外用掉一批 | **≤60 算术上不可能**,除非删掉在用 API |
- **做法(全部编译器/门禁驱动)**
  1. `lib.rs`:`pub use state::*` → **显式 22 项清单**(state 里被 helix-term 用到的全部)。新增导出必须显式加一行。
  2. `state.rs`:8 个仅内部使用的访问器改 `pub(crate)`:`command_args` `get_component_state_json`
     `layout_json` `plugins_dir` `remove_plugin_file` `resolve_plugin_path` `runtime_path` `write_plugin_file`。
  3. 顺带按「有没有调用方」扫其余模块的 22 个零命中 `pub fn`,**净收窄 7 个**(watch / commands / input / host / server_rows / picker / permissions)。
  4. 测试侧不再依赖 crate 根的 glob:`src/tests/mod.rs` 加 `use crate::state::with_terms;`,3 处 `crate::with_engine(..)` 改 `crate::host::with_engine(..)`。
- **两个真陷阱(都写进了 `lib.rs` 的行内注释,别踩第二次)**
  1. **`pub use X::*` 会把 `pub(crate)` 项也带出来**(以 crate 可见性),换显式清单后这些项就从 crate 根消失了 —— 单元测试立刻红(`with_engine` / `with_terms`)。
  2. **`cargo xtask api-check` 按源码文本解析**:①它把 `lib.rs` 从**第一个 `#[cfg(test)]` 处截断**,
     所以绝不能在 `build_engine` 之前写 `#[cfg(test)]` 项(本轮踩过:注册表直接变 0 项,报成「d.ts 多出一堆」);
     ②`helix.icons.*` 的成员来自 `icons.rs` 里的 `pub fn icon_*` 字面量,把 `icon_file` 收窄成 `pub(crate)`
     会让 7 个图标成员从注册表消失(门禁红)。**这两处 `pub` 是文档/门禁契约,不是普通可见性**。
- **完成判据(实测)**
  | 指标 | 基线 | 现在 |
  |---|---|---|
  | 顶层 `pub fn` | 141 | **126**(净收窄 15) |
  | 顶层 `pub(crate) fn` | 254 | 269 |
  | `cargo test -p helix-js` | 176 / 170 passed / 6 ignored | **完全相同** |
  | `cargo integration-test` | 377 passed / 0 failed | **完全相同**(206s) |
  | `cargo xtask api-check` | 134 成员 / 20 事件 / 15 检查点 · 豁免 1 | **完全相同** |
  | `fmt` / `clippy --all-targets -D warnings` | 绿 | 绿(0 warning) |
- **未达成原判据的部分(已在 §6 D7 立项)**:收窄暴露出 **9 个零调用方的 public 函数**
  (`commands::emit_hook` `commands::emit_term_close` `layout::{layout_save,layout_load,layout_list,layout_delete}`
  `picker::picker_has_on_key` `permissions::decide` `diagnostics::emit_lsp_diagnostics`,
  外加只被这四个 `layout_*` 使用的私有 `layout::dir_or_err`)。
  本轮**保持 `pub` 不动** —— 删公共 API 属于另一种改动,且 `layout_*` 疑似与 `:layout` 的 JS 实现构成
  「两套实现」(红线 R3),必须先确认哪套是活的。
- **风险**:公共面变化已由 377 个集成测试 + 三道门禁兜住;回退 = `git revert` 单提交。

### ☑ G4 · 消歧命名 + 模块地图(2026-10-17 完成)

- **症状**:三处 `layout.rs`(`helix-tui` 几何切分 / `helix-js` JS API / `helix-term/ui` 布局树引擎)
  与三处 popup 类文件(`helix-js/popup.rs` / `ui/popup.rs` / `ui/plugin_popup.rs`);任务报告里已出现
  必须靠路径消歧的句子。
- **做法**
  1. `git mv helix-js/src/layout.rs helix-js/src/layout_api.rs`;`mod layout` → `mod layout_api`,
     `crate::layout::` → `crate::layout_api::`(diagnostics.rs 2 处、lsp.rs 1 处),
     lib.rs 里 `layout::js_*` 注册点 38 行同步改。
  2. 文件头加注释说明**为什么**叫 `_api`:另两处 `layout.rs` 分别是纯几何与编辑器布局树。
  3. `docs/architecture.md` 新增「模块地图:名字重了的那几处」6 行对照表(哪一层哪个 layout/popup +
     一条改哪里的规则)。
- **只改了 `helix-js` 一处**:`helix-tui/src/layout.rs` 与 `helix-term/src/ui/layout.rs` 的名字
  **本身是对的**(各自层的唯一职责),歧义来自"三个同名",改名只会把歧义搬走。判据里写的
  "无同名歧义文件"按**可检索性**达成:现在 `helix-js` 侧不再叫 layout。
- **完成判据(实测)**
  | 指标 | 基线 | 现在 |
  |---|---|---|
  | `cargo test -p helix-js` | 176 / 170 passed / 6 ignored | **完全相同** |
  | `cargo xtask api-check` | 3 段全绿(134 成员 / 20 事件 / 15 检查点) | **完全相同** |
  | `cargo check --workspace` / `clippy -D warnings` | 绿 | 绿(0 warning) |
  | 模块地图 | 无 | `docs/architecture.md`(6 行 + 1 条规则) |
- **风险**:纯重命名 + 文档;回退 = `git revert`。

### ☑ G5 · `xtask` 一任务一文件 + 单张 `TASKS` 表(2026-10-17 完成)

- **症状**:6 个任务写在 `main.rs` 的 `tasks` 模块里(`highlightcheck` 254 行、`indentcheck` 196 行),
  dispatch 是 `match t.as_str()`,help 是手写 `println!` —— **已经漂了**:`apicheck` 的注释说"两者",
  实际是 3 个脚本(实测)。
- **做法**
  1. `xtask/src/tasks/{docgen,query,indent,highlight,theme,api}.rs`:函数体**整块搬**,
     只把签名改成统一的 `pub fn run(args: Vec<String>) -> Result<(), DynError>`;
     原参数名是 `languages`/`themes`/`args` 的,在函数首行加一句
     `let <原名> = args.into_iter();` —— 函数体**一个字符没动**。
  2. `tasks/mod.rs`:`pub struct Task { name, run, help }` + `pub const TASKS: &[Task]`,
     `print_help()` 与 `find()` 都从它生成 ⇒ **help 不可能再与 dispatch 漂移**。
  3. `main.rs` 622 → **23 行**:只剩 `mod` 声明 + `main`(查表 + 调 `run`)。
  4. 顺手修正那条已经漂了的注释:api 任务现在明确写"三个门禁"及各自脚本名。
- **完成判据(实测)**
  | 指标 | 改前 | 改后 |
  |---|---|---|
  | `xtask/src/main.rs` | 622 行 | **23 行**(`tasks/*.rs` 合计 696 行) |
  | `cargo xtask`(help) | 手写 6 段 | 表生成,**文本逐行一致**(26 列对齐未变) |
  | `cargo xtask docgen` | OK | OK,但产物**有 1 行预存在漂移**:`:js-permission` 于 10-16 加入,而 `book/src/generated/typable-cmd.md` 停在 10-13 ⇒ 已随本项单独补提交(见 §8 末尾) |
  | `query-check` / `indent-check` / `theme-check` / `highlight-check` | OK | OK(highlight 364 assertions 不变) |
  | `cargo xtask api-check` | 3 段全绿 | 3 段全绿(134 成员 / 20 事件 / 15 检查点) |
  | `cargo xtask bogus` | `Invalid task name` | 相同 |
  | `fmt` / `clippy --all-targets -D warnings` | 绿 | 绿(0 warning) |
- **注意**:`highlight-check --dump <lang> <file>` 分支随函数体原样搬走,未改;它的 e2e 走人工验证。
- **风险**:纯搬家 + 一处 dispatch 形式变化(查表代替 `match`),已被上面 6 条命令逐条钉住。
- **附带发现(与 G5 无关,但由 G5 的验证命令暴露)**:`docgen` 产物**不是幂等的现成状态** ——
  `book/src/generated/typable-cmd.md` 比 `helix-term/src/commands/typed.rs` 少了 `:js-permission` 一行。
  CI 里其实有这道门(`build.yml:140` `cargo xtask docgen` + `git diff --exit-code`),说明 10-16 那次
  提交的产物没被补提交、而 CI 未拦住(可能该提交没走 CI)。**结论:生成物必须由提交者自己跑一次
  docgen 并提交**;这条已作为纪律写进本项。



---

## 4. 批次 B —— 搬家(不改语义,但动的是覆盖率较薄的地方)

### ☑ G6 · `build_engine`(773 行)按命名空间/能力切分(2026-10-17 完成)

- **症状**:`build_engine` 773 行,手工注册 **134 个成员 / 9 个命名空间**,每个成员 5–6 行样板;宽度而非复杂度。
- **做法**:`helix-js/src/register/`(13 个文件)
  | 文件 | 内容 | 行数 |
  |---|---|---|
  | `lsp.rs` / `picker.rs` / `server.rs` | 命名空间对象(8 / 2 / 8 成员) | 56 / 26 / 57 |
  | `pane.rs` / `pane_layout.rs` | `helix.pane.*`(16)+ `helix.layout.*`(6)/`pane_mode`(2) | 97 / 64 |
  | `buffer.rs` / `icons.rs` / `plugin.rs` | `buffer`(3)+`stack`(4)/ 图标(动态 7)/ 可调用 `plugin()` | 59 / 15 / 22 |
  | `root_{doc,fs,buffer,term,ui,theme}.rs` | 根对象成员 78 个,按能力分族 | 64/99/74/85/83/68 |
  | `mod.rs` | 模块声明 + 兄弟模块导入(让裸路径 `commands::js_echo` 可用) | 36 |
  `build_engine` 现在 **94 行**:建 10 个命名空间 → 5+1 次根族注册 → 10 次 `builder.property` → 注册全局。
- **关键决策:根对象用"分族函数 + `&mut ObjectInitializer`",而不是各建各的对象再合并**
  —— `ObjectInitializer` 独占 `&mut Context`,同时只能存在一个;分族函数接收同一个 builder、
  各自追加成员,才能既拆开又保持"一个根对象"。
- **为了不让门禁失明,同步改了 3 处工具**(这是本项最容易被忽略的部分)
  1. `check_api_surface.mjs`:注册表解析从"只读 lib.rs"改成"lib.rs + `register/*.rs` 按序拼接",
     并给每个容器记上**所属文件**;归属只在同一文件内生效(否则 `root_*.rs` 的成员会被
     错挂到前面某个命名空间)。
  2. 顺带修它的容器正则:原先 `let (\w+)_obj =` 不分右值,把 `let lsp_obj = register::lsp::register(engine)`
     这种"拼装语句"也当容器(实测把所有成员都记成 root);现在要求右值真的是 `ObjectInitializer::new`。
  3. `check_capability_surface.mjs`:`registrations()` 也扫 `register/*.rs`,否则能力检查会报
     "实现里没有 check_js"(它靠 JS 名 → Rust fn 路径定位实现)。
- **完成判据(实测)**
  | 指标 | 基线 | 现在 |
  |---|---|---|
  | `build_engine` 行数 | 773 | **94** |
  | 最长注册函数 | 773 | **99**(`root_fs.rs`) |
  | `cargo xtask api-check` | 134 成员 / 9·10 命名空间 / 20 事件 / 15 检查点 · 豁免 1 | **逐项相同**(buffer 3 · icons 7 · layout 6 · lsp 8 · pane 16 · pane_mode 2 · picker 2 · server 8 · stack 4 · root 78 · 可调用 1 · 全局量 2) |
  | `cargo test -p helix-js` | 176 / 170 passed / 6 ignored | **完全相同** |
  | `cargo integration-test` | 377 passed / 0 failed | **完全相同**(204.5s) |
  | `fmt` / `clippy -D warnings` | 绿 | 绿(0 warning) |
  | 注册调用总数 | 134 | 127 条静态 `.function(` + 图标动态 7 = **134** |
- **已知取舍**:根对象成员的**属性插入顺序**变了(原先按编写顺序,现在按族分组)。
  `Object.keys(helix)` 的顺序会变,但成员集合不变,且这三道门禁都不依赖顺序;若有插件依赖顺序,
  那是它本来就脆。已在 `lib.rs` 注释里写明"顺序 = 族顺序"。
- **风险**:注册表是 API 面的唯一真相源;本次改动同时改了**生成方与检查方**,已用"逐项计数不变"钉住。

### ☑ G7 · `apply_ui_requests`(689 行)按请求族切分(2026-10-17 完成)

- **症状**:689 行单函数,43 个 arm 各自直接做 UI 接线(建 Popup、`job::dispatch_blocking`、
  `compositor.replace_or_push`);`UiRequest` 有 92 个变体。
- **做法**:handler 落在 `helix-term/src/commands/typed/ui_requests/`
  | 文件 | handler 数 | 内容 |
  |---|---|---|
  | `pane.rs` | 24 | 面板/布局/堆叠/缩放/焦点 |
  | `term.rs` | 7 | 打开/模式/关闭/清屏/保存/尺寸/喂数据 |
  | `doc.rs` | 5 | 文档/缓冲区/退出 |
  | `server.rs` | 3 | 服务器管理(含最大的 `ServerListRows` 85 行) |
  | `editor.rs` | 2 | 主题、键位映射 |
  | `popup.rs` / `picker.rs` / `watch.rs` | 1 / 1 / 1 | 弹窗、取词器、文件监视 |
  顶层 `apply_ui_requests` **59 行**:`for req in reqs { match req { … } }`,每个 arm 一行转调。
- **机械搬法(为什么 diff 看起来大但语义为零)**:arm 的**解构写在 handler 签名里**
  (`fn handle_open_panel(OpenPanelReq { id, side, .. }: OpenPanelReq)`),arm 体**逐行原样**成为
  handler 体,末尾 `Ok(())`。所以 43 个 handler 的每一行都能在旧函数里找到对应行。
- **三处必须手写的边界(编译器直接指出,不是猜的)**
  1. `Watch`/`Unwatch` 原先是**一个组合 arm**(`ref request @ (A(..) | B(..))`),我的 arm 扫描器
     第一版只认出 `A`,导致 match 非穷尽 → 改成 `apply_watch(request: UiRequest)`,两条 arm 各自
     重建变体传入(与原 `ref request @` 等价,已核对 `apply_watch` 的 `&UiRequest` 签名)。
  2. handler 若叫 `server_op` 会**遮蔽**同名的真实函数,导致体内 `server_op(editor, …)` 递归到
     自己(编译报参数个数不符)→ 统一 `handle_` 前缀。
  3. 子模块用 `use super::*;` 拿 `typed.rs` 的私有导入(`bail`/`job`/`ui`/`layer_id`),
     所以 handler 体不用重写任何路径 —— 这也是本项能做到"零语义改动"的原因。
- **完成判据(实测)**
  | 指标 | 基线 | 现在 |
  |---|---|---|
  | `apply_ui_requests` | 689 行 | **59 行** |
  | 最长 handler | 689 | **85 行**(`handle_server_list_rows`) |
  | `cargo test -p helix-js` | 176 / 170 passed / 6 ignored | **完全相同** |
  | `cargo integration-test` | 377 passed / 0 failed | **完全相同**(195s) |
  | `fmt` / `clippy -D warnings` | 绿 | 绿(0 warning) |
- **明确不做**:不动 `UiRequest` 枚举本身(92 变体 / 16 文件 match,收益远小于成本)。
- **风险**:纯搬家;43 个 arm 全部有集成测试覆盖(context 里已确认 377 条含 plugin_* 各族)。


### ☑ G8b · `server_manager` 按职责拆成 7 个模块(2026-10-17 完成)

- **结果**:`helix-term/src/server_manager/`
  | 文件 | 行数 | 内容 |
  |---|---|---|
  | `mod.rs` | 1,339 | 模块头 + `mod` 声明 + **21 项显式再导出** + `Availability` / `MANAGED_*` + `#[cfg(test)] mod tests`(1,283 行原样) |
  | `install.rs` | 466 | 安装/升级/卸载/任务执行(16 项) |
  | `langs_toml.rs` | 447 | `languages.toml` 读写/托管区块/可用性(18 项) |
  | `registry.rs` | 384 | 原内联 `pub mod registry { … }` 的内容(路径 `server_manager::registry::Spec` 不变) |
  | `download.rs` | 197 | 镜像/sha256/下载/进度钩子(15 项) |
  | `archive.rs` | 126 | 归档解包 + `strip_path`(7 项) |
  | `version.rs` | 100 | 版本探测 + 模板展开(4 项) |
  合计 3,059 行(比原 2,956 多 103 行,全是模块头注释与 `use`)。
- **三个坑的解法(上一轮 G8a 已踩过,这轮按解法做)**
  1. **glob 环**:子模块之间**不用** `use super::*`,一律 `use super::<mod>::{具体名字}`;
     `mod.rs` 也只做**显式清单**再导出(`pub(crate) use install::{availability, install, …};`),
     不再是 `pub(crate) use install::*`。glob↔glob 会被解析器当成空集合,这是上一轮失败的根因。
  2. **`#[cfg(test)]` 项**:`download_progress_hook_set` / `extract_archive` /
     `install_from_local_dir_for_test` / `copy_dir_recursive` 只在测试目标存在,再导出时单独挂
     `#[cfg(test)]`,否则非测试构建报 unresolved import。
  3. **clippy `unused_imports` 会误删再导出**:上一轮被它删掉 36 项(含外部仍在用的)。
     这轮改成"**只再导出模块外真的在用的名字**"——用 `grep server_manager::<name>` 量出 21 项,
     其余内部辅助函数由子模块直接 `use super::<mod>::…`,根本不经过 mod.rs,所以没有可误删的东西。
- **测试夹具的可见性**:`mod tests` 仍是 `tests` 子模块,自身加 `use super::{archive,download,…}::*;`
  (叶子模块 glob 不会成环),原有 1,283 行测试**一行没改**。
- **完成判据(实测)**
  | 指标 | 基线 | 现在 |
  |---|---|---|
  | `commands/` 下有 `server_manager.rs` | 无(G8a 已移出) | 无 |
  | 模块数 | 1 个文件 2,956 行 | **7 个文件**(最大 1,339,且其中 1,283 是测试) |
  | 最长函数 | `build_section` 118 | **118**(`build_section`,未变);`too_many_lines` 命中 3 个:141/118/112,全部 < 150 |
  | `cargo integration-test` | 377 passed | **377 passed / 0 failed** |
  | `cargo test ... integration server_manager` | 8 passed | **8 passed** |
  | `cargo test -p helix-js` / `api-check` | 176 · 三段绿 | **相同** |
  | `fmt` / `clippy`(两种口径) | 绿 | 绿(0 warning) |
- **过程中的真发现(记为 D8)**:`cargo test -p helix-term --lib` **在改动前就是红的**(用 `git stash` 对比确认):
  `server_manager::tests::builtin_recipes_loaded_from_toml_keep_semantics` 稳定失败
  ("pyright 应为占位(no source)"),`archive_assets_install_end_to_end` 间歇失败(3 次里 1 次)。
  根因是测试共享**进程级** `EXT: OnceLock<Mutex<Vec<Spec>>>` 缓存 + `SM_*` 环境变量,
  并行执行时互相踩(该模块注释里本来就记着一次"22 个连锁 PoisonError"的事故)。
  **与本次拆分无关**(HEAD 同样频率),但意味着 `--lib` 目前不能当门禁用。
- **风险**:纯搬家 + 再导出;`registry` 的 `include_str!("../../../runtime/server-manager-builtin.toml")`
  相对路径在新位置深度相同(编译期已验证)。


---

## 5. 批次 C —— 真重构(C 批每项:先写 spec → 等确认 → 再动代码)

### ✅ G9 · JS 插件共享层 —— 评估后结案,不抽(2026-10-17)

> **spec**:[`superpowers/specs/2026-10-17-plugin-shared-lib-design.md`](superpowers/specs/2026-10-17-plugin-shared-lib-design.md)
> **spec 已出**(2026-10-17),需要你拍板:**建议不做原形态**。实测跨插件「逐字相同」的函数只有 **4 个 / 9 份 / 35 行**;
> 其余同名函数(`render` ×3、`handle_key` ×2 110/44 行)是各自面板的交互骨架,合并属行为重构。
> spec 里的方案 A = 结案不做,方案 B = 只抽 4 个 + 确立 `plugins/lib/` 约定。

- **症状**:`wc_len / wc_pad / wc_trunc / wc_ell`(CJK 宽度)在插件侧重写(核心已有宽度逻辑 ⇒
  正是 `docs/plugin-vs-core.md` 红线 R3);`render` 出现在 4 个插件、`shq` 3 个、
  `say` / `row_el` / `handle_key` / `guard` 各 2 个。
- **做法**:先只搬 `wc_*` 到 `plugins/lib/`(`docs/plugin-layout.md` 已规定 lib 是共享层),
  调用方改为 `helix.load("lib/…")` 或导出函数;其余函数按需再搬。
- **完成判据**:`plugins/lib/` 存在且被引用;`sh contrib/install-plugins.sh` 后
  `plugins/arsenal/tests/*.test.js` 与相关插件行为不变。
- **验证**:`node plugins/arsenal/tests/arsenal.test.js`(若有)+ `cargo integration-test` 中插件相关模块
- **注意**:改完 `plugins/` **必须**重跑 `contrib/install-plugins.sh`,否则跑的是 `runtime/plugins/` 旧副本。

- **结案结论(按 spec 方案 A)**:不建 `plugins/lib/`。判据不再是行数,而是**证据**:
  逐字重复 35 行 / 13 个插件 = 0.8%,而代价是给 5 个插件各加一个**用户可目录级覆盖**的加载依赖
  (`plugins/lib/` 一旦被用户覆盖,所有依赖方一起坏)。合成层真正值得做的形态是 **Rust 原生 API + `helix.d.ts`**
  (那样才进三道门禁),而不是 JS 共享文件。
- **复算方式**(任何人可复核):抽每个 `plugins/*/plugin.js` 的顶层 `function` 体 → 去注释/压空白 → 哈希;
  同哈希即逐字重复。脚本见 spec 附录。

### ✅ G10 · 抽 `JsSurface`:panel + popup(2026-10-17 完成)

> spec:[`superpowers/specs/2026-10-17-js-surface-design.md`](superpowers/specs/2026-10-17-js-surface-design.md)

- **结果**:新增 `helix-term/src/ui/js_surface.rs`(**310 行**),把「状态 + 刷新 + Tab 焦点 +
  节点键路由 + paste + onKey 收尾 drain + 文档快照」收成一层;两个容器只留语义。

  | 文件 | 改前 | 改后 |
  |---|---|---|
  | `ui/plugin_panel.rs` | 369 | **168** |
  | `ui/plugin_popup.rs` | 401 | **208** |
  | 两者合计 | **770** | **376**(+ `js_surface.rs` 310) |
  | 两者间 20 行共享窗口 | **73** | **0** |

- **共享 vs 语义的切法(spec 的核心决定)**
  - 收进 `JsSurface`:`id/focus/focusables/lines/diff`、`refresh`(有 `Context`)/`refresh_global`
    (布局期 `required_size` 无 `Context`)、`tab_focus`、`node_key`(Enter/方向键/编辑键/粘贴)、
    `drain_after_onkey`(游标/编辑/装饰/消息/UI 请求)、`command_context`、`render_lines`。
  - 留在容器:**Esc 语义**(面板 = 取消焦点,弹窗 = 走 onKey 关闭)、关闭/自愈动作、
    `required_size`、面板的 `focusable` 开关、弹窗的 layer 语义。
  - 差异一律走 `SurfaceOpts { collect_focusables, reset_missing_focus, esc_releases_focus, label }`,
    **共享层里没有 `if 是面板/是弹窗`**。
- **过程中被测试抓住的两个真回归(值得记)**:
  1. **面板的 Esc 分支丢了**。我从 popup 复制节点键路由时漏了 panel 独有的
     `"Esc" => { self.focus = None }` ⇒ `plugin_panel_focus` 5 条全红。改成 `esc_releases_focus` 开关后复原。
  2. 弹窗单测直接写 `p.lines = …`(字段),字段搬进 `JsSurface` 后编译失败(E0615),改
     `p.surface.lines = …`。**这两处都不是"看起来对就行"的地方,是测试救回来的。**
- **完成判据(实测)**
  | 指标 | 基线 | 现在 |
  |---|---|---|
  | `cargo integration-test` | 377 passed | **377 passed / 0 failed** |
  | `plugin_panel`(含 focus 族) | 11 passed | **11 passed** |
  | `plugin_popup` / `plugin_paste` / `plugin_input_multiline` / `window_mode` | 4 / 2 / 1 / 16 | **全部通过** |
  | popup 单测(`required_size` 3 条) | 3 passed | **3 passed** |
  | 最长 `handle_event` | 234 | **48**(面板)/ **57**(弹窗) |
  | `fmt` / clippy(两种口径) | 绿 | 绿(0 warning) |
- **没做的**:`PluginTerminal`(1,558 行,内部还有 `TerminalCell`/`TerminalGrid`)—— 它的共享面是
  网格/光标/PTY 回调,与这两个不同构;等有第二个终端类组件时再评估。

### ✅ G11 · 双状态所有权收尾(2026-10-17 完成:G11a 收口,G11b 判定不做)

> spec:[`superpowers/specs/2026-10-17-host-ownership-design.md`](superpowers/specs/2026-10-17-host-ownership-design.md)

- **做了什么(G11a)**
  1. **`ACTIVE` 读点 5 → 1**:新增 `fn active_ptr()`,5 处 `ACTIVE.with(|a| a.get())` 全部改走它
     (剩余 4 处 `ACTIVE.with` 都是**写点**:set/clear/pump 的两处)。
  2. **所有权地图写进代码**:`host.rs` 模块头新增一张表(按宿主 / 进程级+分桶 / thread_local 三类,
     每行写「为什么」),并给出一条判断规则:**新增状态先问「它是跨线程的吗?」**
  3. **删掉 4 处自相矛盾的 Phase B 说法**(模块头、结构体注释、两条测试注释),
     改成指向本 spec 的结论。
  4. **补 2 条不变量测试**:`host_key_is_stable_per_host_and_falls_back`、
     `queries_do_not_create_host`(后者钉住「查询类入口不得顺手建引擎」,且在**干净线程**上验,
     不动测试线程的默认宿主)。
- **G11b 为什么判为「不该做」**(本项最有价值的结论)
  | 状态 | 类型 | 为什么不能进宿主 |
  |---|---|---|
  | `TERM_EVENTS` / `ASYNC_EVENTS` / `LSP_RESULTS`(含 `_RX`) | thread_local | worker → 主线程通道,**必须 `Send`**;宿主持有 boa `Context`,是 `!Send` |
  | `TERM_WORKERS` / `TERM_MASTERS` | thread_local | pty 的进程/fd 表,与「哪个 JS 宿主」无关 |
  | `PICKER_STATE` / `VIEWPORT` / `LAST_LAYOUT` / `CONFIGS` / `DIAGNOSTICS` | `OnceLock` | 消费端是 helix-term 帧路径(每帧读),签名冻结 |
  | `UI_REQUESTS` | `OnceLock`(按宿主分桶) | 本体必须 `Send`(worker 也 push);消费端 16 个冻结调用点 |
  硬搬只能靠 `unsafe impl Send` 或再套一层全局表 —— 前者是 UB 温床,后者等于没搬。
  真需要「两个完全隔离的宿主」时,正确形态是**两个进程**,而不是一进程两引擎。
- **`ACTIVE` 保留裸指针的理由**:换成 `Rc<RefCell<JsHost>>` 会让 Phase D 的成果**退化** ——
  现在 `pump()` 拿 `&mut JsHost`,「JS 回调里再调一次 render」是**编译不过**;
  `RefCell` 会把它变成**运行期 panic**。裸指针的真实风险(析构前没 `clear_active`)
  由 `host_cleared_before_drop` + `ActiveGuard` 守着,本项再把读点收成一个。
- **完成判据(实测)**
  | 指标 | 基线 | 现在 |
  |---|---|---|
  | `ACTIVE` 读点 | 5 | **1** |
  | 「Phase B / 仍是进程级」矛盾说法 | 4 | **0** |
  | 不变量测试 | 2 | **4** |
  | `cargo test -p helix-js` | 176 | **178**(172 passed / 6 ignored) |
  | `-- --ignored` / api-check / 377 集成 | 绿 | **绿(6/6 · 三段 · 377)** |
  | `fmt` / clippy(两种口径) | 绿 | 绿(0 warning) |

### ✅ G12 · `job::dispatch_blocking` 闭包 → `UiOp`(pane 族,2026-10-17 完成)

> spec:[`superpowers/specs/2026-10-17-uiop-dispatch-design.md`](superpowers/specs/2026-10-17-uiop-dispatch-design.md)

- **结果**:新增 `ui_requests/ui_op.rs`(365 行,其中 5 条单测),`UiOp` 24 个变体 +
  `apply`(唯一执行点)+ `dispatch`(保持「一请求一次 dispatch」)。

  | 指标 | 改前 | 改后 |
  |---|---|---|
  | `pane.rs` 里的 `job::dispatch_blocking` | **24** | **0** |
  | `pane.rs` 行数 | 330(含 190 行闭包体) | 207(纯映射) |
  | 全仓 `dispatch*` 调用点 | 75 | **54**(−24 个闭包 +1 个唯一执行点 + 注释里的字样) |
  | pane 族 handler 直接单测 | 0 | **5 条**(映射/白名单/JSON 透传) |
  | `apply_ui_requests` | 59 行 | 59 行(pane arm 仍是 1 行:`dispatch(pane::handle_x(req)?)`) |
- **怎么搬的**:闭包体**整段**搬进 `UiOp::apply` 的对应分支,`bail!` 与参数白名单留在 handler;
  组件构造(`PluginTerminal`/`PluginPanel`)从闭包搬进 `apply`,所以 op 只装数据
  (`kind: String`),能 `derive(Debug, Clone, PartialEq)` 被单测直接断言。
- **两条刻意的不变(写在 `ui_op.rs` 文件头)**
  1. **一个请求一次 dispatch** —— 不合并成批,否则与其他 UI 工作的相对时序会变;
  2. **`apply` 不补齐缺失的 `set_layout_change_kind`** —— 哪些操作发 `layout-change` 是既有行为
     (例如 `open_panel`/`zoom` 本来就不发),顺手补上属行为变化(铁律 2)。
- **完成判据(实测)**
  | 指标 | 基线 | 现在 |
  |---|---|---|
  | `cargo integration-test` | 377 passed | **377 passed / 0 failed** |
  | `window_mode` / `plugin_layout` / `plugin_panel` | 16 / 5 / 11 | **16 / 5 / 11 全绿** |
  | `ui_op` 单测 | — | **5 passed** |
  | `fmt` / clippy(两种口径) | 绿 | 绿(0 warning) |
- **没做的**:`term.rs`(8)/`doc.rs`(5)/`editor.rs`(2)等 22 处 dispatch —— 它们的闭包里有
  异步/进程/编辑器语义,需要各自设计;`UiRequest` 枚举仍不动(沿用 G7 的决定)。


---

## 6. 待决策(阻塞部分目标,需先确认)

| ID | 问题 | 默认建议 | 阻塞 |
|---|---|---|---|
| **D1** | 范围:只治 fork 新增债务(16 个长函数 / 1,486 行超额),还是连上游遗留(54 个长函数)一起? | **只治 fork**;上游不动,降低未来 merge 成本 | 全部 |
| **D2** | 是否允许收窄 `helix-js` 公共面(G3,会连带改 helix-term 约 15 个文件)? | **允许**,但先量规模;超 2 倍预估就拆 G3a/G3b | G3, G6, G7, G11 |
| **D3** | 每项都跑全量 `cargo integration-test`(367 tests / ~180s)? | **A 批只跑相关子集,B/C 批全量** | 全部 |
| **D4** | C 批 spec 是否要我写完等你确认后才动代码? | **是**(本文 §2 铁律 5) | G9–G12 |
| **D5** | ~~2 个 `#[ignore]` 测试已在坏死(拷 `plugins/features/*.js`)~~ | **已修**(2026-10-17):路径改到 `plugins/<name>/plugin.js`;`--ignored` 由 4/6 变 **6/6 通过** | — |
| **D6** | ~~`clippy --features integration` 改动前就红~~ | **已修**(2026-10-17):`plugin_roots_for` 加 `#[cfg(any(not(feature="integration"), test))]`;CI 的 clippy 步骤加 `--features integration`;顺带清掉 25 条该 feature 下的既有 lint | — |
| **D7** | ~~9 个零调用方的 public 函数~~ | **已处理**(2026-10-17):`layout_{save,load,list,delete}` + `dir_or_err` **确认死亡并删除**(`:layout` 实际由 `plugins/layout/plugin.js` 实现,JS 侧走 `js_layout_*`,测试 5+16 条全绿);其余 4 个(`emit_hook`/`emit_term_close`/`picker_has_on_key`/`emit_lsp_diagnostics`)**不是死代码** —— 被单元测试调用,`decide` 更是被 `typed.rs:5114` 用(以 `decide_permission` 别名,G3 的名字扫描漏了别名) | — |
| **D8** | ~~`cargo test -p helix-term --lib` 改动前就红~~ | **已修**(2026-10-17):扩展配方缓存在测试期改 thread-local;`builtin_recipes` 断言内置集合;`install_spec` 消除「查表→用表」窗口;38 个 SM 用例统一 `sm_lock`;CI 新增 `cargo test --workspace --lib` | — |

---

## 7. 验证命令速查

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --lib                            # 库里那批只跑在 --lib 的用例(D8)
cargo test -p helix-js                                   # 可加 -- --ignored
cargo integration-test                                   # = cargo test --features integration --profile integration --workspace --test integration
cargo test -p helix-term --features integration --test integration <module>
cargo xtask api-check                                    # 期望:134 成员 / 9 命名空间 / 20 事件 / 15 检查点 · 豁免 1
cargo xtask docgen                                       # 产物应零 diff
cargo xtask query-check | indent-check | highlight-check | theme-check
sh contrib/install-plugins.sh                            # 改了 plugins/ 之后必须跑
```

---

## 8. 变更记录(每完成一项追加一行)

| 日期 | ID | commit | 等价性证据(数字) | 备注 |
|---|---|---|---|---|
| 2026-10-17 | **G0** | `fc0585abe`(WIP)· `49c4fe1`(.gitignore)· 本条(docs) | 176 测试 / 0 failed;api-check 134 成员 · 15 检查点;fmt+clippy 绿 | 基线就位。WIP 由**并行进程**代提交;`.swp` 只忽略不删;仓库 2 个 stash 未动 |
| 2026-10-17 | **G1** | 本条(`refactor(js): … (G1)`) | lib.rs 5693→**860**;176 测试 / 170 passed / 6 ignored **不变**;99 个测试名逐个核对 1 次;clippy 0 warning;api-check 不变 | 7 个域文件 + `mod.rs`(夹具);发现 2 个 `#[ignore]` 测试已坏死 → D5 |
| 2026-10-17 | **G2** | 本条(`refactor(tests): … (G2)`) | integration **377 passed / 0 failed 与基线逐项一致**;19 个本地 harness 定义→0;净 −305 行;fmt+clippy 绿 | 3+3 个变体逐个比对过;`plugin_components::render_rows` 是死代码;发现 feature 门禁漏洞 → D6 |
| 2026-10-17 | **G3** | 本条(`refactor(js): … (G3)`) | 顶层 pub fn **141→126**;176 测试 / 377 集成 / api-check 134 成员 **全部不变**;fmt+clippy 绿 | `state` 显式导出 22 项;踩到 api-check 两个文本解析陷阱(已写进注释);零调用方 API → D7 |
| 2026-10-17 | **G4** | 本条(`refactor(js): … (G4)`) | 176 测试 / api-check 3 段全绿 不变;clippy 0 warning | `layout.rs`→`layout_api.rs`(42 行引用同步);新增模块地图 |
| 2026-10-17 | **G5** | 本条(`refactor(xtask): … (G5)`) | 6 个任务逐个跑 OK;docgen 零 diff;help 逐行一致;main.rs 622→23 | `TASKS` 表驱动 dispatch+help;修掉已漂的 api-check 注释 |
| 2026-10-17 | **G6** | 本条(`refactor(js): … (G6)`) | `build_engine` 773→**94**;api-check **逐项计数不变**(134/20/15);176 测试 / 377 集成不变 | 13 个 register 文件;同步扩展**两个**检查脚本(否则门禁失明);修掉容器正则误判 |
| 2026-10-17 | **G7** | 本条(`refactor(term): … (G7)`) | `apply_ui_requests` 689→**59**;最长 handler 85;176 测试 / 377 集成不变;clippy 0 warning | 43 个 handler 按族分 8 文件;组合 arm / 同名遮蔽两个坑 |
| 2026-10-17 | **G8a** | 本条(`refactor(term): … (G8a)`) | `commands/` 下已无 server_manager;server_manager 集成测试 8 passed;clippy 0 warning;66 处 `commands::server_manager::…` 引用零改动 | 拆分失败原因(glob 环 + cfg(test) 再导出 + clippy 误删)已记入 G8b |
| 2026-10-17 | **D5** | 本条(`test(js): …`) | `cargo test -p helix-js -- --ignored` **6 passed / 0 failed**(改前 4/6);默认套件 176/170/6 不变;fmt+clippy 绿 | 两个测试拷的是 2026-10 前的扁平布局;`ignore` 理由同步改成真实原因 |
| 2026-10-17 | **D6** | 本条(`fix(lint): …`) | `clippy --features integration -D warnings` 由 **红(26 条)**变 0;普通 clippy 仍 0;377 集成测试不受影响 | 顺带修掉 2 处 `MutexGuard` 跨 await(改成 tokio Mutex) |
| 2026-10-17 | **D7** | 本条(`refactor(js): …`) | 删 37 行死实现;176 测试 / `--ignored` 6/6 / api-check 三段 / plugin_layout 5 / window_mode 16 全绿;clippy 0 | 修正 D7 原始清单:4 个是测试专用,`decide` 被 alias 调用 |
| 2026-10-17 | **G8b** | 本条(`refactor(term): … (G8b)`) | 7 个模块(最大 1,339,含 1,283 行测试);377 集成测试不变;两种 clippy 0 warning | 三个坑的解法:显式再导出 / cfg(test) 单挂 / 只导出外部在用的 21 项 |
| 2026-10-17 | **G9/G10/G12** | 本条(`docs: C 批三份 spec`) | 无代码改动;三份 spec 各含**实测数字**(G9: 4 组/35 行重复;G10: 73 个共享窗口;G12: pane 24/全仓 75)与完成判据 | 状态 🟡,等你逐条拍板后再开工 |
| 2026-10-17 | **G9** | 本条(`docs(audit): G9 结案`) | 无代码改动;交付的是**可复算的度量**(4 组/9 份/35 行)与结案理由 | 决定不抽 JS 共享层;要共享就走 Rust 原生 API |
| 2026-10-17 | **G10** | 本条(`refactor(ui): … (G10)`) | panel+popup **770→376**、共享窗口 **73→0**、最长 handle_event 234→57;377 集成测试不变;clippy 两种口径 0 warning | 新增 `js_surface.rs`(310);Esc 语义用开关区分，测试抓到面板 Esc 分支丢失 |
| 2026-10-17 | **G12** | 本条(`refactor(term): … (G12)`) | pane.rs dispatch **24→0**;全仓 75→54;377 集成测试不变;新增 5 条 handler 单测;clippy 两种口径 0 warning | `UiOp` 只装数据;保持一请求一次 dispatch;不补既有缺失的 kind |
| 2026-10-17 | **D8** | `e3713f1df` | `cargo test --workspace --lib` 连跑 8 轮 0 失败(此前 1 稳定红 + 1 间歇红);377 集成测试不变;clippy 两种口径 0 warning | 根因=EXT 进程级可写缓存 × worker 线程;测试期改 thread-local |
| 2026-10-17 | **G11** | `159730b4a` | ACTIVE 读点 5→1;矛盾注释 4→0;测试 176→**178**(+2 不变量);377 集成 / api-check 三段不变;clippy 0 | G11b(搬 6 OnceLock + 12 TLS 进宿主)判定**不该做**:通道必须 Send,而宿主持有 !Send 的 boa Context |
| 2026-10-17 | **S2-a** | 本条(`refactor(term): …`) | `server_op` **269→22**(clippy 口径);最长 handler 53;`typed.rs` 6307→6028;`--workspace --lib` / 377 集成 / api-check 三段 / docgen 全绿 | 照 G7 同法:`typed/server_ops/` 7 个 handler + 分派;分支体逐行搬,判断条件未改 |
| 2026-10-17 | **G10b spec** | 本条(`docs: G10b spec`) | 无代码改动;spec 含实测(handle_event 147 / 7 段 / 23+26+14+1 条测试)与逐条行为对照清单 | 状态 🟡;结论是不接 JsSurface(共享面为零),同文件拆 6 个方法 |
| 2026-10-17 | **G10b** | 本条(`refactor(ui): …`) | `handle_event` 147→**12**;最长方法 49;终端测试 23 + 27 + 14 + 1 全绿;377 集成不变;`--workspace --lib` / api-check / docgen 全绿 | 不接 JsSurface;同文件按事件种类拆 7 个方法,分支体逐行搬(结构行差异已逐条核对) |
| 2026-10-17 | **S3-a** | `7a5c4f485` | 239 处错误构造 → 1 个 `js_err`;净 **−517 行**(419+/936−);popup.rs −179;172 测试 / `--ignored` 6/6 / 377 集成 / api-check 不变 | 根因是样板而非长函数;`js_err` 放 crate 根,`layout_api` 的本地包装删除 |

---

## 9. 收尾体检(2026-10-17,全部目标收口后重跑同一套口径)

> 口径与 §1 完全一致(clippy 权威长函数 / 归一化 20 行窗口 / 文件行数 / 门禁),所以数字可比。

### 9.1 基线 → 现在

| 指标 | 体检基线 | 现在 | 说明 |
|---|---|---|---|
| `helix-js/src/lib.rs` | 5,693 行 | **202** | G1(测试搬出)+ G6(注册表搬到 `register/`) |
| `build_engine` | 773 行 | **94** | G6 |
| `apply_ui_requests` | 689 行 | **59** | G7 |
| `pane` 族 dispatch 闭包 | 24 处 | **0** | G12(`UiOp`) |
| 全仓 `job::dispatch*` 调用点 | 75 | **54** | −24 闭包 +1 唯一执行点 |
| `server_manager` | 1 文件 2,956 行 | **7 文件**(最大 1,339,含 1,283 行测试) | G8a/G8b |
| `plugin_panel` + `plugin_popup` | 770 行 | **376**(+`js_surface` 310) | G10 |
| 两者 20 行共享窗口 | 73 | **0** | G10 |
| 跨文件重复(20 行窗口) | 47 个块 | **2 对文件 / 12 个窗口** | 见 9.2 |
| 长函数(>100 行,唯一) | 70 | **63** | fork 相关 16 → **12**(`server_op` 269→22、`plugin_terminal::handle_event` 147→12) |
| JS 错误构造样板 | 239 处 | **0**(统一 `crate::js_err`) |
| `ACTIVE` 裸指针读点 | 5 | **1** | G11a |
| `helix-js` 测试 | 176 | **178** | +2 条不变量 |
| 集成测试 | 377 | **377** | 未增未减 |
| api-check | 134 / 20 / 15 | **134 / 20 / 15** | 三道门禁全程未松 |

### 9.2 重复:只剩 2 对

| 文件对 | 窗口数 | 判断 |
|---|---|---|
| `helix-js/src/tests/plugin.rs` ↔ `tests/popup_panel.rs` | 9 | **S3**:G1 拆分时两边都带了同款测试夹具(`make_tar_gz`/`write_sh`/`load_script_named` 一类)。可以收进 `tests/mod.rs`,纯测试改动 |
| `helix-dap/src/transport.rs` ↔ `helix-lsp/src/transport.rs` | 3 | **不做**:上游代码(D1),改它只增加未来 merge 成本 |

### 9.3 fork 相关的长函数(14 个),按「值不值得再动」分三档

| 函数 | 行数 | 判断 |
|---|---|---|
| ~~`typed.rs::server_op`~~ | ~~269~~ → **22** | ✅ **已拆**(2026-10-17):`typed/server_ops/` 7 个 handler + `mod.rs` 分派;最长 handler 53 行;`typed.rs` 6307 → 6028 |
| `popup.rs::statusline_parts` | 223 | S2(可选):纯数据整形,可拆成「取 zones / 组装 parts」两段 |
| `popup.rs::parse_node` | 161 | S3:JSON → 节点的递归下降,拆开反而更难读 |
| ~~`plugin_terminal.rs::handle_event`~~ | ~~147~~ → **12** | ✅ **已拆**(2026-10-17,G10b):`on_mouse` 18 / `on_paste` 8 / `on_key` 35 / `term_key_decision` 28 / `normal_mode_key` 49 / `insert_mode_key` 13 / `copy_visible` 11;不接 `JsSurface`(共享面为零) |
| `ui_op.rs::apply` | 139 | **不做**:24 个 arm 的单 match,每个 arm 5–6 行;拆成 24 个函数只是把 match 搬走 |
| ~~`popup.rs::{js_open_popup, js_open_terminal, js_el}`~~ | 120/136/143 → **103/101/129** | ✅ **已清**(2026-10-17):根因不是「函数太长」,而是**错误构造样板** —— `JsError::from_opaque(JsValue::from(JsString::from(..)))` 全 crate **239 处**,统一成 `crate::js_err(..)` 后净 **−517 行** |
| `typed.rs::server_panel` | 109 | S3 |
| `server_rows.rs::deliver` | 108 | S3 |
| `server_manager::{parse_ext_recipe, build_section, run_task}` | 101–132 | **不改**:G8b 判据是 <150,已达标 |

### 9.4 其它遗留(都记录在案,均不阻塞)

1. **`tsc --noEmit` 不在 CI**:`docs/` 把它列为插件侧检查,但 `build.yml` 里没有这一步(本机也没装 tsc)。类型面目前靠 `helix.d.ts` 人工维护 + api-check 的名字对账。
2. **一次未能复现的偶发红**:收尾时 `cargo test -p helix-js` 有 1 次(9 次里)整模块
   失败且耗时 71s(平时 11.7s)⇒ 是「某个等待型用例超时后被 TEST_LOCK 放大」那一类
   (模块注释 2026-10-14 记过同款事故)。随后 8 轮单跑 + 4 轮 `--workspace --lib` 全绿,
   无法复现;已顺手把两处「不持锁却碰宿主」的测试收进 `TEST_LOCK`(其中 G11 新增的那条
   改成在**干净线程**上验,彻底不碰测试线程的默认宿主)。若将来复现,按 `wait_for_term_event`
   的超时日志定位。
3. **12 处 `#[ignore]`**:fork 侧 6 处(已全部修好并 `--ignored` 6/6 通过,默认不跑是因为慢),其余为上游。
4. **fork 内 13 处 `TODO/FIXME/ponytail`**:`ponytail:` 是**有意的取舍记录**(每条都写了为什么+量化),不是待办;`TODO` 里多数是上游 25.7.1 的原文。
5. **2 个 pre-existing stash**(`user-wip-application`、基于 `bbb090d7e` 的 WIP):不是本次产物,原样保留。
6. **`docs/CONTRIBUTING.md`「367 tests」与 `docs/server-manager.md` 的 `server_manager.rs` 路径**:本次已顺手修正(377 / `server_manager/mod.rs`)。

### 9.5 结论

- A/B/C 三批 **12 个目标 + 4 个决策项全部收口**,没有"未解释"的数字变化(每一项的差异都在 §8 有记录)。
- 剩余事项按上面分档:**S2 两条已全部拆完**(`server_op` 269→22、`plugin_terminal::handle_event` 147→12);
  其余是 S3 / 明确不动。**§9 的遗留清单已清空到只剩 S3。**
- 门禁现状:`fmt` / `clippy`(两种口径)/ `cargo test --workspace --lib` / `cargo test -p helix-js`(+`--ignored`)/ `cargo integration-test`(377)/ `api-check`(三道)/ `docgen` 零 diff —— 全部绿。

---

## 附:体检要点(为什么是这 12 项)

| 类别 | 症状 | 证据强度 |
|---|---|---|
| **长函数** | 70 个 >100 行;fork 新增文件 16 个(超额 1,486 行) | 权威口径(clippy)+ 脚本交叉验证 |
| **重复** | panel↔popup 30 个相同 20 行块;12 个测试文件各自重造 harness;JS 侧 `wc_*` 重写核心能力 | 哈希窗口实测 |
| **命名混乱** | 3×`layout.rs`、3× popup 类文件;190 个 pub fn 中 145 带 `js_` 前缀、45 不带 | 词法统计 |
| **隐藏耦合** | `ACTIVE` 裸指针 + `pump()` 约定;`UiRequest` 92 变体 / 16 文件;`job` 闭包 85 处 | 代码阅读 + 计数 |
| **深模块破坏** | `lib.rs` 85% 是测试;`build_engine` 773 行手工注册 134 成员;`state.rs` 106 个 11 行访问器 | 文件内比例 |
| **正向(勿破坏)** | `too_many_arguments` 0 处、`allow(too_many_lines)` 仅 1 处、三道 API 门禁全绿、文档挂实测数字 | 实测 |
