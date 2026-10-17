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
| ☐ | **G1** | `helix-js` 内联测试(4,835 行 / 99 测试)搬出 `lib.rs` | S1 | A | 纯搬家 | G0 |
| ☐ | **G2** | 集成测试 harness(`pump`/`render_rows`)收进 `helpers.rs` | S2 | A | 纯减法 | G0 |
| ☐ | **G3** | `state.rs` 收窄公共面(106 pub → 显式导出)+ `*_for_test` 收口 | S3 | A | 纯减法 | G0 |
| ☐ | **G4** | 消歧命名:`helix-js/src/layout.rs` → `layout_api.rs` + 模块地图 | S2 | A | 重命名 | 无 |
| ☐ | **G5** | `xtask` 一任务一文件 + 单张 `TASKS` 表驱动 help | S2 | A | 纯搬家 | 无 |
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
cargo test -p helix-js                                    # 基线:176 个 #[test](99 个在 lib.rs)
cargo xtask api-check
#    基线必须不变:134 个成员 / 20 个事件 / 15 个能力检查点 · 豁免 1
#    ⚠ 已知小漂移:两个脚本对**命名空间**计数不一致 —— helix.d.ts 那份报 10、文档那份报 9,
#      成员数两者一致(134)⇒ 门禁绿但计数口径没统一。属文档/脚本层面的小 bug,不阻塞任何目标。

# ③ 集成测试(全量基线:367 tests / ~180s)
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

### ☐ G1 · 把 `helix-js` 的内联测试搬出 `lib.rs`

- **症状**:`lib.rs` 5,693 行里 4,835 行是 `#[cfg(test)] mod tests`(99 个测试)。生产代码只占 858 行,却和测试混在一起 —— 阅读 9 个命名空间的注册表要翻过 4.8k 行测试。
- **做法**
  1. 建 `helix-js/src/tests/mod.rs` + 按域切 `api.rs / popup.rs / layout.rs / fs.rs / term.rs / lsp.rs`,
     `mod.rs` 里 `pub(crate) use super::*;` 保留访问私有符号的能力(测试用了大量 `pub(crate)` 内部项)。
  2. `lib.rs` 末尾只留 `#[cfg(test)] mod tests;`。
  3. 切分按**测试名归属**机械归类,不改任何断言语义。
- **预估**:1 个文件变 7 个,`lib.rs` 5693 → ~860;新增文件全部是搬过去的原文。
- **完成判据**:`cargo test -p helix-js` 数量与基线**完全相同**(176);`lib.rs` < 900 行。
- **验证**:`cargo test -p helix-js` + `cargo test -p helix-js -- --ignored` + `cargo xtask api-check`
- **回退**:`git revert <sha>`(单提交,无跨 crate 影响)

### ☐ G2 · 集成测试 harness 收进 `helpers.rs`

- **症状**:12 个测试文件各自重造 `async fn pump(&mut Application, &str)` 与 `fn render_rows(...)`,
  两两共享 13–15 个 20 行块;而 `tests/test/helpers.rs`(556 行)已经存在却没用上。
  12 个文件:`filetree / pane_modes / plugin_components / plugin_layout / plugin_layout2 /
  plugin_terminal_hooks / plugin_theme / server_manager / splits / window_mode / window_rail / window_split`。
- **做法**:把 `pump` / `render_rows` / 相关 `use`(含 `#[cfg(not(windows))]` 分支)搬进 `helpers.rs`,
  各文件改 `use super::helpers::*;`(统一走已有约定),删本地副本。
- **预估**:−400 行左右,12 个测试文件各减 ~40 行。
- **完成判据**:12 个文件里不再出现 `fn pump(` / `fn render_rows(` 定义;集成测试数量与基线一致(367)。
- **验证**:`cargo integration-test`
- **风险**:`server_manager` 一族共享进程级 `SM_*` 环境变量(串行),搬动时不要顺手"优化"它的串行化。

### ☐ G3 · `state.rs` 收窄公共面 + `*_for_test` 收口

- **症状**:`state.rs` 1,230 行 / **106 个 pub 访问器**,平均每个 11.6 行(几乎全是 `with_*` 包装);
  `lib.rs` 用 `pub use state::*` 全量转出 ⇒ crate 公共面 190 个函数平铺,外部无法分辨边界。
  同时 `install_from_local_dir_for_test` / `*_reset_for_test` 之类测试入口以 `pub` 形式 leaked。
- **做法**(全部由编译器驱动,不靠人工审计)
  1. `lib.rs`:`pub use state::*;` → 显式列表,只保留 helix-term 真实用到的(用 `cargo check` 逐个补)。
  2. `state.rs` 内其余项改 `pub(crate)`。
  3. `*_for_test` 移进 `#[cfg(test)]` 或 `pub(crate) mod test_support`。
- **预估**:helix-term 侧约 15 个文件、390 处 `helix_js` 引用中会有一批需要改导入路径 —— **先量再承诺**,数字写进本次完成记录。
- **完成判据**:`helix-js` 公共 `pub fn` 数 190 → ≤60;`grep -rn "_for_test" helix-js/src | grep -v cfg(test)` 只命中 `test_support`。
- **验证**:`cargo test --workspace` + `cargo integration-test` + `cargo xtask api-check`
- **风险**:这是 G6/G7 的前置(边界清楚了好切分),但 diff 面比想象大;若超过 2 倍预估,拆成 G3a(导出面)/ G3b(`_for_test`)。

### ☐ G4 · 消歧命名 + 模块地图

- **症状**:三处 `layout.rs`(`helix-tui` TUI 布局 / `helix-js` JS API / `helix-term/ui` 布局树引擎)
  与三处 popup 类文件(`helix-js/popup.rs` / `ui/popup.rs` / `ui/plugin_popup.rs`);任务报告里已出现
  必须靠路径消歧的句子。
- **做法**:`helix-js/src/layout.rs` → `layout_api.rs`(`mod`/`use` 同步改),其余**不动**;
  在 `docs/architecture.md` 加一张 6 行"哪一层哪个 layout/popup"的模块地图。
- **完成判据**:无同名歧义文件;`docs/architecture.md` 有该地图;`cargo test -p helix-js` 数量不变。
- **验证**:`cargo check --workspace` + `cargo test -p helix-js`

### ☐ G5 · `xtask` 一任务一文件 + 单张 `TASKS` 表

- **症状**:6 个任务写在 `main.rs` 的 `tasks` 模块里(`highlightcheck` 254 行、`indentcheck` 196 行),
  dispatch 是 `match t.as_str()`,help 是手写 `println!` —— **已经漂了**:`apicheck` 注释说"两者",
  实际是 3 个脚本(实测)。
- **做法**:`xtask/src/tasks/{docgen,query,indent,highlight,theme,api}.rs`,每个导出一个
  `fn run(args: &[String]) -> Result<(), DynError>`;`main.rs` 只留
  `const TASKS: &[(&str, fn(&[String]) -> Result<(), DynError>, &str)]`,dispatch 与 help 都从它生成。
- **完成判据**:`cargo xtask` 打印的 help 与真实任务一致;6 个任务各跑一次结果与改前相同。
- **验证**:`cargo xtask` + `cargo xtask docgen && git diff --stat`(docgen 产物应当零变化)
  + `cargo xtask api-check`

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
