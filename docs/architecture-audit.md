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

| 状态 | ID | 目标 | 严重度 | 批次 | 性质 | 依赖 |
|---|---|---|---|---|---|---|
| ✅ | **G0** | 建立干净基线:收尾当前 WIP + 清 `.swp` | — | 前置 | 卫生 · 2026-10-17 · `fc0585abe` + `49c4fe1` + 本条 | 无 |
| ✅ | **G1** | `helix-js` 内联测试(4,835 行 / 99 测试)搬出 `lib.rs` | S1 | A | 纯搬家 · 2026-10-17 · 本条 | G0 |
| ✅ | **G2** | 集成测试 harness(`pump`/`render_rows`)收进 `helpers.rs` | S2 | A | 纯减法 · 2026-10-17 · 本条 | G0 |
| ✅ | **G3** | `state.rs` 收窄公共面 + `*_for_test` 收口(实测重定义见 §3) | S3 | A | 纯减法 · 2026-10-17 · 本条 | G0 |
| ✅ | **G4** | 消歧命名:`helix-js/src/layout.rs` → `layout_api.rs` + 模块地图 | S2 | A | 重命名 · 2026-10-17 · 本条 | 无 |
| ✅ | **G5** | `xtask` 一任务一文件 + 单张 `TASKS` 表驱动 help | S2 | A | 纯搬家 · 2026-10-17 · 本条 | 无 |
| ☐ | **G6** | `build_engine`(773 行)按 9 个命名空间切分 | S1 | B | 搬家 | G1, G3 |
| ☐ | **G7** | `apply_ui_requests`(689 行)按请求族切分 | S1 | B | 搬家 | G3 |
| ☐ | **G8** | `server_manager.rs`(2,956 行)拆成 7 个模块并移出 `commands/` | S2 | B | 搬家 | 无 |
| ☐ | **G9** | JS 插件共享层:`wc_*` 等重复助手 → `plugins/lib/` | S3 | C | 真重构 | 无 |
| ☐ | **G10** | `PluginPanel`/`PluginPopup`/`PluginTerminal` 抽 `JsSurface` | S2 | C | 真重构 | G6, G10 spec |
| ☐ | **G11** | 双状态所有权收尾(`state.rs` ↔ `host.rs`,含 `ACTIVE` 裸指针) | S1 | C | 真重构 | G3, G11 spec |
| ☐ | **G12** | `job::dispatch_blocking` 闭包派发 → `UiOp`(85 处) | S3 | C | 真重构 | G7, G12 spec |

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

### ☐ G6 · `build_engine`(773 行)按 9 个命名空间切分

- **症状**:`helix-js/src/lib.rs:85 build_engine` 773 行,手工注册 **134 个成员 / 9 个命名空间**,
  每个成员 6 行样板 `NativeFunction::from_fn_ptr(...)`。宽度而非复杂度。
- **做法**:`helix-js/src/register/{lsp,picker,pane,pane_layout,buffer,doc,fs,term,ui}.rs`,
  统一 `pub(crate) fn register(ctx: &mut Context) -> JsObject`,`build_engine` 只剩 9 次调用 + 拼装。
  第二步(单独提交,可选)把样板压成声明表 `(name, fn_ptr, arity)` 循环展开。
- **完成判据**:最长注册函数 < 120 行;`cargo xtask api-check` 输出**逐字不变**(134 / 9 / 20 / 15)。
- **验证**:`cargo test -p helix-js` + `cargo xtask api-check` + `cargo integration-test`
- **风险**:`helix.d.ts` / `docs/plugin-api.md` / 能力检查点三闸门是这里的安全网,**跑它们**,别只跑测试。

### ☐ G7 · `apply_ui_requests`(689 行)按请求族切分

- **症状**:`helix-term/src/commands/typed.rs:4903` 689 行单函数,45 个 arm,每个 arm 直接做 UI 接线
  (建 Popup、`job::dispatch_blocking`、`compositor.replace_or_push`);`UiRequest` 有 92 个变体。
- **做法**:拆到 `helix-term/src/commands/ui_requests/{pane,term,picker,doc,editor,fs}.rs`,
  每个 arm 变成一个 `fn(...) -> Result<()>`;顶层只剩 6 路分派。
- **不做**:不动 `UiRequest` 枚举本身(拆枚举会碰 16 个文件,收益远小于成本 —— 明确列为**不做**)。
- **完成判据**:`apply_ui_requests` < 80 行;最长 handler < 100 行;集成测试数量与基线一致。
- **验证**:`cargo integration-test` + 按族抽样
  (`cargo test -p helix-term --features integration --test integration plugin_panel` 等)

### ☐ G8 · `server_manager.rs`(2,956 行)拆模块并移出 `commands/`

- **症状**:一个文件里塞了 HTTP 下载(`ureq`)、4 套归档解包(`extract_archive`/`extract_bytes`/
  `extract_zip_bytes`/`extract_tar_bytes`)、`sha256_hex`、自写 `strip_path`、TOML 重写
  (`build_section` 118 行、`rewrite_languages_toml` 一带 433 行)、内联 `pub mod registry`(~380 行)、
  PATH 探测、install/update/remove;还包含 `install_from_local_dir_for_test`。
- **做法**:纯 `git mv` + 加 `mod`,**零逻辑改动**:
  `helix-term/src/server_manager/{mod,download,archive,registry,version,install,langs_toml}.rs`;
  `*_for_test` 挂 `#[cfg(test)]`。
- **完成判据**:`commands/` 下不再有 `server_manager.rs`;最长函数 < 150 行;集成测试数量不变。
- **验证**:`cargo test -p helix-term --features integration --test integration server_manager`
  (该族串行,属已知);`cargo clippy --workspace --all-targets -- -D warnings`
- **风险**:`rewrite_languages_toml` 会改用户 `languages.toml` —— 这段**只许移动,不许重排**。

---

## 5. 批次 C —— 真重构(C 批每项:先写 spec → 等确认 → 再动代码)

### ☐ G9 · JS 插件共享层(先做这个,最便宜)

- **症状**:`wc_len / wc_pad / wc_trunc / wc_ell`(CJK 宽度)在插件侧重写(核心已有宽度逻辑 ⇒
  正是 `docs/plugin-vs-core.md` 红线 R3);`render` 出现在 4 个插件、`shq` 3 个、
  `say` / `row_el` / `handle_key` / `guard` 各 2 个。
- **做法**:先只搬 `wc_*` 到 `plugins/lib/`(`docs/plugin-layout.md` 已规定 lib 是共享层),
  调用方改为 `helix.load("lib/…")` 或导出函数;其余函数按需再搬。
- **完成判据**:`plugins/lib/` 存在且被引用;`sh contrib/install-plugins.sh` 后
  `plugins/arsenal/tests/*.test.js` 与相关插件行为不变。
- **验证**:`node plugins/arsenal/tests/arsenal.test.js`(若有)+ `cargo integration-test` 中插件相关模块
- **注意**:改完 `plugins/` **必须**重跑 `contrib/install-plugins.sh`,否则跑的是 `runtime/plugins/` 旧副本。

### ☐ G10 · 三个 JS 组件抽 `JsSurface`

- **症状**:`ui/plugin_panel.rs`(368)↔ `ui/plugin_popup.rs`(400)有 **30 个相同 20 行块**,
  差异仅 276 行;两边各有 234 / 218 行的 `handle_event`、同样的 focusables 提取、同样的 render 骨架;
  `ui/plugin_terminal.rs`(1,558)是第三个变体。历史报告写过"仿 `plugin_popup.rs:21-23`"。
- **做法**:抽 `helix-term/src/ui/js_surface.rs`:`JsSurface { id, content, focus, focusables, dirty }`
  + 四个钩子(`refresh_content` / `extract_focusables` / `dispatch_key` / `render_rows`);
  panel/popup 各留 ~120 行只写几何与放置差异;terminal 第三批再跟。
- **完成判据**:panel+popup 合计 ≤ 500 行(原 768);组件间重复块 0;
  回归网全绿:`plugin_panel`(2)/ `plugin_popup`(3)/ `window_mode`(14)/ `plugin_layout`(3)/ `plugin_input_multiline`。
- **验证**:`cargo integration-test`(全量,别只跑子集 —— 焦点/键路由的回归会跨族出现)

### ☐ G11 · 双状态所有权收尾(`state.rs` ↔ `host.rs`)

- **症状**:进程级 `OnceLock`/`thread_local!`(6+12)与按宿主 `JsHost` 字段**同时存在**,注释里
  "仍是进程级(Phase B)"与"现在按宿主隔离"并存;同一操作有"自由函数 + 方法"两个入口
  (`emit_event` / `render_popup` / `dispatch_input_key`),自由函数按**环境宿主**解析;
  环境宿主是 `ACTIVE: Cell<Option<NonNull<JsHost>>>` 裸指针 + "析构前必须 `clear_active()`"的人工契约
  (违规 = UB,仅 1 个测试守护)。症状证据:8 个测试文件各自持有 `TEST_LOCK`。
- **做法**(按 `docs/superpowers/plans/2026-10-11-js-host-phaseB.md` 已有顺序,一次一个队列)
  1. 先立边界(依赖 G3)。
  2. 每个队列搬完**顺手删掉自由函数别名** —— 否则就是他们自己红线 R1"留两套实现"。
  3. 中期把 `ACTIVE` 裸指针换成 `Rc`/`Weak` 或让自由函数收 `&mut JsHost`(先量有多少调用点需要参数化)。
- **完成判据**:`state.rs` 里 `OnceLock` 数量减少且每次减少都有对应删除的自由函数;
  `cargo test -p helix-js` 可**并行**跑(不设 `--test-threads`)连续 3 次不 flaky。
- **验证**:`cargo test -p helix-js` ×3 + `cargo integration-test`

### ☐ G12 · `job::dispatch_blocking` 闭包派发 → `UiOp`

- **症状**:UI 变更表达为"闭包丢进全局 job 队列,在里面拿 compositor",85 个调用点 ⇒
  请求处理不可单测、执行顺序不可静态推断。
- **做法**:**只做一个子集**(建议 pane/terminal 族):handler 返回 `UiOp`(描述式枚举),
  由一处执行;其余保持原样,不追求全量。
- **完成判据**:选定子集的 handler 有直接单测(不经 Application);调用点数量下降且已解释;
  集成测试数量不变。
- **验证**:`cargo integration-test` + 新增单测
- **注意**:这是收益最大也最危险的一项 —— 没有 spec 与回归网**不要开工**。

---

## 6. 待决策(阻塞部分目标,需先确认)

| ID | 问题 | 默认建议 | 阻塞 |
|---|---|---|---|
| **D1** | 范围:只治 fork 新增债务(16 个长函数 / 1,486 行超额),还是连上游遗留(54 个长函数)一起? | **只治 fork**;上游不动,降低未来 merge 成本 | 全部 |
| **D2** | 是否允许收窄 `helix-js` 公共面(G3,会连带改 helix-term 约 15 个文件)? | **允许**,但先量规模;超 2 倍预估就拆 G3a/G3b | G3, G6, G7, G11 |
| **D3** | 每项都跑全量 `cargo integration-test`(367 tests / ~180s)? | **A 批只跑相关子集,B/C 批全量** | 全部 |
| **D4** | C 批 spec 是否要我写完等你确认后才动代码? | **是**(本文 §2 铁律 5) | G9–G12 |
| **D5** | 新发现:2 个 `#[ignore]` 测试已在坏死(拷 `plugins/features/*.js`,路径已废弃),是否纳入清理? | **纳入**为独立小项(修路径或删测试),别混进 G1 | — |
| **D6** | 新发现:`cargo clippy --features integration -- -D warnings` 改动前就红(`plugin_roots_for` 在开 feature 时是 dead code)。是否修? | **修**:给该 fn 加 `#[cfg(any(not(feature = "integration"), test))]`,并把 feature 纳入 CI 门禁 | — |
| **D7** | G3 量出 **9 个零调用方的 public 函数**(`emit_hook`/`emit_term_close`/`layout_{save,load,list,delete}`/`picker_has_on_key`/`decide`/`emit_lsp_diagnostics`),其中 `layout_*` 疑似与 JS 侧 `:layout` 构成 R3「两套实现」。删除还是接线? | **先查 `:layout` 走哪套**,死的那套删掉;这项单独做,不混进收窄 | — |

---

## 7. 验证命令速查

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
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
