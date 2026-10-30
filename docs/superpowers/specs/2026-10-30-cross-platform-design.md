# 跨平台通用化：shell 门面 · PtyBackend 抽象 · 工具链 Rust 化

> **状态：spec，待确认**（按 `docs/architecture-audit.md` §2 铁律 5：C 批先出 spec，写完等人确认再动代码）
> 日期：2026-10-30 · 关联：[`../../vision.md`](../../vision.md)（Cross-platform）、[`../../plugin-vs-core.md`](../../plugin-vs-core.md) 红线 R4
> 目标：Linux / macOS / Windows **行为等价**，不是「能编过」

---

## 0. 一句话

Windows 上现在能开编辑器、能编辑，但**插件系统里凡是碰进程的地方都残**：`helix.run` 硬编码 `sh -c`、三处 `env::var("HOME")`、PTY 整个模块 `#[cfg(unix)]` 拿掉、工具链脚本全是 `#!/bin/sh`。

本 spec 用**三个互不依赖**的改动把「Unix-only」从结构上消掉，任何一件都能独立提交、独立回退，且**不改动 Unix / macOS 的现有行为**：

1. **A** `helix.run` / `run_async` 加 **argv 形态**（不经 shell）+ 字符串形态走平台 shell 门面；
2. **B** 抽 **`PtyBackend`** 接口，Windows 侧先返回**明确错误**（ConPTY 留到下一轮）；
3. **C** `contrib/*.sh`、`plugins/tools/sync_runtime.sh` 收敛成 **`cargo xtask`** 子命令。

---

## 1. 目标 / 非目标

### 1.1 目标（本轮必须达成）

| # | 目标 | 判定标准 |
|---|---|---|
| **G1** | `helix.run` / `helix.run_async` 接受 `string[]`（argv，不经 shell） | Windows 上 `helix.run(["git","--version"])` 成功 |
| **G2** | 字符串形态按平台选 shell（`sh -c` / `cmd /C`） | Unix 现有插件**零改动**仍工作 |
| **G3** | 三处 `env::var("HOME")` 换跨平台取法 | Windows 上不再落到 cwd 下的 `./.local/...` |
| **G4** | PTY 从 `#[cfg(unix)] mod` 变成「接口 + 两平台实现」，Windows 返回明确 `Err` | Windows 上 `cargo test -p helix-js` **编译通过**，pty 用例给出可读错误 |
| **G5** | `cargo xtask install-plugins` / `cargo xtask sync-runtime`（纯 Rust） | 三平台可用；`.sh` 退化为一行 `exec` |

### 1.2 非目标（本轮明确不做）

| # | 不做 | 原因 |
|---|---|---|
| **N1** | ConPTY 真实现 | 唯一「周级」工作，且**必须有 Windows 实机验证**（管道 EOF、Job Object 杀进程树）。本轮只把结构留好 |
| **N2** | 改 5 个内置插件的 JS 调用点（`dapui` 的 `printf %s "$HOME"` 等） | 依赖 G1 先落地；且与并行运行冲突（见 §9） |
| **N3** | `helix.spawn({cmd})` 接受 argv | 它是流式 + pty API，跟 ConPTY 一起做语义才完整；先改会留半套 |
| **N4** | `.ps1` 脚本 | 维护两份必然漂移（仓库已有「靠人记得的步骤一定会被忘」的教训） |
| **N5** | 上游遗留跨平台问题（clipboard / tree-sitter 等） | 上游在维护，碰它只增加未来 merge 成本（沿袭 `architecture-audit.md` D1） |

---

## 2. 现状证据（file:line 实测，非推测）

| # | 位置 | Windows 上的实际表现 | 归属 |
|---|---|---|---|
| A1 | `helix-js/src/commands.rs:2043` | `Command::new("sh").arg("-c")` **无条件** ⇒ `helix.run` 必失败；源码自带 `// ponytail: …仅 Unix；未来要 Windows 支持需改 cmd.exe /C` | A |
| A2 | `helix-js/src/shell.rs:186` | `spawn_worker`（`run_async` / `spawn` 非 pty 路径）同样 `sh -c` | A |
| A3 | `helix-js/src/shell.rs:341` | `spawn_pty_worker` 同样 `sh -c` | A |
| B1 | `helix-js/src/commands.rs:1890` `term_state_path()` | `env::var("HOME")` 失败 → 回退 `"."` ⇒ 终端状态写进 **cwd** 下的 `./.local/state/helix/` | B（P1） |
| B2 | `helix-term/src/commands/typed/ui_requests/term.rs:116` | `:term-save` 默认路径拼成 `/.cache/helix/term-N.log` | B（P1） |
| B3 | `helix-term/src/server_manager/install.rs:25` `managed_root()` | 受管根变 `/.local/share/helix/managed` ⇒ **arsenal / server 管理整体失效** | B（P1） |
| C1 | `helix-js/src/pty.rs`（123 行纯 POSIX）+ `lib.rs:16` `#[cfg(unix)] mod pty;` | 模块不存在 ⇒ 编译期就少一块 | B（P4） |
| C2 | `helix-js/src/state.rs:934` | `TERM_MASTERS: HashMap<u64, RawFd>`，整函数 `#[cfg(unix)]` ⇒ resize 链是 POSIX 形状 | B（P4） |
| C3 | `helix-js/src/popup.rs:500-560`、`shell.rs:495-515` | 各有一套 `#[cfg(unix)]` / `#[cfg(not(unix))]` 两半重复 | B（P4） |
| D1 | `contrib/install-plugins.sh`、`plugins/tools/sync_runtime.sh` | 纯 `#!/bin/sh` + `cp -R` + `find -printf`，Windows 需 Git Bash / WSL | C |

**关键判断（决定 B 的抽象层次）**：泄漏点不只是 `pty.rs`，更是 `TERM_MASTERS` **存了 `RawFd`**。只给 `pty.rs` 加 cfg 不够——`term_resize` 那条链仍是 POSIX 语义。所以抽象必须落在**句柄**这一层。

另有两处**已做对**、本轮不碰的对照：`helix-term/src/commands/dap.rs:908-910` 已有 `cmd /C` / `sh -c` 双分支；`helix-term/src/server_manager/archive.rs` 的解包是纯 Rust（zip/tar/gzip crate），天然跨平台。

---

## 3. 方案 A：argv 形态 + shell 门面

### 3.1 API 形态

```ts
run(cmd: string | string[]): string;
run_async(cmd: string | string[]): Promise<string>;
```

`api.rs` 表里 `arity` **已经是 1**（`helix-js/src/api.rs:934`），`caps` 已是 `CAP_PROCESS` ⇒ **表行不用改**，派发点（`api.rs:1671-1685`）能力检查自动继承。`check_api_surface.mjs` 只对**名字**，`describe()` 的 arity 不变 ⇒ **三件套里只有 d.ts 与文档需要改**。

### 3.2 语义（必须逐条写进文档）

| 输入 | 行为 |
|---|---|
| `string` | Unix `sh -c <str>`；Windows `cmd /C <str>`。**语法随平台变** ⇒ 文档必须写明：跨平台插件**禁止**用字符串形态做管道/重定向/引号转义 |
| `string[]` | `Command::new(argv[0]).args(&argv[1..])`，**不经 shell**。`argv[0]` 走 `PATH`（Windows 自动 PATHEXT 展开，无需 `.exe`） |
| `[]`（空数组） | `Err("helix.run: empty argv")` —— 不静默成功 |
| 非 0 退出 | `Err`，消息含 code。Unix 被信号杀死时 `code()` 为 `None` ⇒ 现有 `"signal"` 文案**保留**（Windows 恒有 code） |
| 输出 | 只取 stdout；截断 `RUN_OUTPUT_LIMIT = 65536` 后追加 `(truncated)`。注意该上限是 **char 数**（`chars().take()`，`commands.rs:2029`），不是字节 |
| 能力 | `process`（不变） |
| 阻塞 | 同步阻塞主线程（不变） |

### 3.3 实现切分

新增 `helix-js/src/platform.rs`（≈40 行，无状态）：

```rust
/// 字符串形态 → 平台 shell 命令（Unix: sh -c / Windows: cmd /C）
pub(crate) fn shell_command(cmd: &str) -> std::process::Command;

/// argv 形态 → 不经 shell 的命令。空数组必须报错。
pub(crate) fn argv_command(argv: &[String]) -> anyhow::Result<std::process::Command>;
```

改动**只有 3 处**（全在 runtime 路径，均在 §2 表里列出）：

| 文件 | 改法 |
|---|---|
| `helix-js/src/commands.rs` `js_run` | 判 `args[0].is_array()` → `argv_command`；否则 `shell_command`。删掉 `ponytail:` 注释（它记的正是本 spec 要做的事） |
| `helix-js/src/shell.rs:186` `spawn_worker` | `sh -c` → `platform::shell_command` |
| `helix-js/src/shell.rs:341` `spawn_pty_worker` | 同上（Windows 落不到这里，但不留两套写法） |

**不做**：抽「进程执行抽象层」。3 个调用点不值得一个 trait —— `plugin-vs-core.md` R4 的反面：别为将来某一处需求先造框架。

### 3.4 门禁三件套（缺一即红）

1. `plugins/helix.d.ts:859-860` —— 改 union（`tsc --noEmit -p plugins/jsconfig.json` 必须绿）；
2. `docs/api/process-fs.md:20-45` —— 补 argv 形态 + 「字符串形态语法随平台变」警告 + 能力表不变；
3. `docs/plugin-api.md` 速查索引 —— 若列了签名需同步（`check_doc_surface.mjs` 只对名字，不卡签名，但别留错）。
4. `helix-js/src/api.rs` 该行**加注释**说明支持 union —— 表本身不动，但后来人要知道「arity=1 指的是一个 union 参数」。

---

## 4. 方案 B：PtyBackend 抽象（Windows 先 Err）

### 4.1 接口设计

```rust
// helix-js/src/pty/mod.rs —— 唯一平台无关接口
pub(crate) struct TermSize { pub rows: u16, pub cols: u16 }

/// 子进程侧 stdio（Unix = slave 三份 dup；ConPTY = 管道一端）
pub(crate) struct ChildStdio { pub stdin: Stdio, pub stdout: Stdio, pub stderr: Stdio }

/// 宿主端句柄。Drop 负责关闭。
pub(crate) trait PtyMaster: Send + 'static {
    fn resize(&self, size: TermSize) -> Result<()>;
    /// 读线程独占一份（现有代码用 `try_clone` 达到同样效果）
    fn reader(&self) -> Result<Box<dyn Read + Send>>;
    fn write_all(&mut self, buf: &[u8]) -> Result<()>;
    fn flush(&mut self) -> Result<()>;
    /// 注册进 TERM_MASTERS 的 resize 句柄（避免注册表里出现平台类型）
    fn resize_handle(&self) -> Arc<dyn PtyResize>;
}

pub(crate) trait PtyResize: Send + Sync {
    fn resize(&self, size: TermSize) -> Result<()>;
}

/// 打开一对 pty（尺寸由调用方给，替换现在硬编码在 open_pty 里的 24×80）
pub(crate) fn open(size: TermSize) -> Result<(Box<dyn PtyMaster>, ChildStdio)>;

/// 本构建是否支持 pty —— 用于给 JS 侧人话错误，不假装可用
pub(crate) fn is_supported() -> bool;
```

### 4.2 文件布局（沿用 `register/` 的既有习惯）

```
helix-js/src/pty/mod.rs      // 接口 + open()/is_supported() 的平台分发
helix-js/src/pty/unix.rs     // 现有 123 行原样搬（Master → UnixMaster: PtyMaster）
helix-js/src/pty/windows.rs  // open() => Err("pty: ConPTY 未实现（见本 spec §1.2/N1）")
```

`state.rs`：`TERM_MASTERS: RefCell<HashMap<u64, Arc<dyn PtyResize>>>`，**去掉 `#[cfg(unix)]`**。

### 4.3 行为等价保证（本项验收核心）

| 断言 | 证据 |
|---|---|
| Unix 路径逐字节等价 | `pty.rs` 的五步开机（`posix_openpt`/`grantpt`/`unlockpt`/`ptsname`/`open slave`）原样搬；3 条自带测试继续绿：`pty_output_nl_maps_to_crlf`、`pty_default_termios_has_echo_and_canonical`、外加 `kernel` 默认 termios 注释所述行为 |
| 尺寸语义不变 | 默认仍是 **24×80**：`pty.rs:57` 注释记录「内核新建 pty winsize 是 0×0，`stty size` 会读成 `0 0`」，必须在子进程启动前设好 |
| 「陈旧 fd → EBADF → Err 良性」保留 | `UnixResize` 只持 `RawFd`（Copy，不 own），与 `state.rs:930` 注释的语义完全一致 |
| 注册顺序不变 | `shell.rs:505` 「先注册 master fd 再起 worker，spawn 返回后 JS 立即可 term_resize」的顺序与注释必须保留 |
| 清理路径不变 | `shell.rs:967/1004` 的 `TERM_MASTERS.remove(&id)` 不动 |
| 搬家证据 | `git diff` 中 `pty/unix.rs` 逻辑行数应与现在的 `pty.rs` 相同（只改类型名与签名） |

### 4.4 错误口径

`is_supported()` 为 `false` 时，JS 侧消息从今天的 `"open_terminal: requires a unix platform"` 改为可执行的话：

> `终端面板需要 PTY；本平台暂未实现（Windows 需 ConPTY，见 docs/superpowers/specs/2026-10-30-cross-platform-design.md）`

保留「失败要响亮」的既有设计（`popup.rs:556` 就是这么做的），不改成静默降级。

### 4.5 为什么「只返回 Err」也算交付

因为它把**平台假设从 5 个文件收敛到 1 个目录**，并让 Windows 的失败变成**可测试的运行期错误**（今天它是编译期缺模块）。N1 落地时只需新增 `pty/windows.rs` + 让 `is_supported()` 返回 true，**不改任何共有代码**。

---

## 5. 方案 C：`.sh` → `cargo xtask`

### 5.1 语义必须逐条对齐（防漂移）

| 现脚本 | 新命令 | 必须一致的语义 |
|---|---|---|
| `contrib/install-plugins.sh` | `cargo xtask install-plugins [--runtime <dir>] [--dry-run]` | 整棵 `plugins/` → `<runtime>/plugins`（含 `lib/`、`examples/`、`init.js`、`helix.d.ts`）；`--dry-run` 只打印不写；结束打印文件数与插件目录名 |
| `plugins/tools/sync_runtime.sh` | `cargo xtask sync-runtime [--runtime <dir>] [--config <dir>] [--dry-run]` | **只更新已存在的目标文件，不新建**；例外：**已存在**的插件目录下可新建文件（典型 `<plugin>/tests/*.js`，「因为 `api-check` 会比对它们」）；跳过 `tools/`、`examples/`；逐文件 `cmp` 后 `cp`；打印每条更新与计数 |
| env `HELIX_RUNTIME` / `HELIX_CONFIG` / `DRY_RUN` | 同名 env + 同名 flag | 全部保留（文档与肌肉记忆已依赖它们） |

**唯一有意的行为改进**：默认目录从硬编码 `$HOME/.config/helix` 改为 `helix_loader::config_dir()`（etcetera）。

- Linux / macOS：结果**不变**（`~/.config/helix`）；
- Windows：变成 `%APPDATA%\helix` —— **这才对**，也是本次改动唯一一处用户可见变化，必须写进 help 与 commit message。

### 5.2 落点

```
xtask/src/tasks/install_plugins.rs
xtask/src/tasks/sync_runtime.rs
xtask/src/tasks/mod.rs     // 只加 2 行 `mod` + 2 条 TASKS 行（表驱动，help 自动生成）
```

`xtask/Cargo.toml` 已依赖 `helix-loader` 与 `helix-stdx`，**无需新增依赖**。

`.sh` 保留为薄封装（保证既有文档/CI/肌肉记忆不断）：

```sh
#!/bin/sh
exec cargo xtask install-plugins "$@"
```

### 5.3 与红线 R4 的关系

`plugin-vs-core.md` R4 = 「搬走但没接线 / 未同步运行副本」。现在「同步」有了跨平台入口，且能被 `cargo xtask` 的 help 发现 —— 比藏在 `plugins/tools/` 下更不容易被忘。

---

## 6. 提交切分（铁律 1：一项一个提交）

| 提交 | 内容 | DoD（每条都要跑过） |
|---|---|---|
| **P1** | 三处 `HOME` → `home_dir()` | `cargo test -p helix-js`；`cargo integration-test`（`server_manager` 族） |
| **P2** | `platform.rs` + `js_run` union + 2 处 `sh` → 门面 | `cargo test -p helix-js`；`cargo fmt --all --check` |
| **P3** | `d.ts` union + `docs/api/process-fs.md` + `api.rs` 注释 | `cargo xtask api-check`（零 diff）；`tsc --noEmit -p plugins/jsconfig.json` |
| **P4** | `pty/{mod,unix,windows}.rs`；`state.rs` 去 fd；`shell.rs`/`popup.rs` 收敛 cfg | `cargo test -p helix-js`（3 条 termios 测试必须绿）；`git diff` 显示 `pty/unix.rs` 逻辑行数不变 |
| **P5** | `xtask install-plugins` | `cargo xtask install-plugins --dry-run` 与 `DRY_RUN=1 sh contrib/install-plugins.sh` **逐行比对** |
| **P6** | `xtask sync-runtime` | 同上；另加实测「改一个插件文件 → 只镜像那一个」 |
| **P7** | Windows 交叉 check 进门禁（见 §7） | 该命令退出码 0 |

**P4 单独一提交的理由**：它是唯一可能影响行为等价性的改动，必须能单独 `git revert`。

**P1/P2 顺序不可换**：`js_run` 的 argv 形态与 `HOME` 无关，但 P1 是纯 bug 修复、风险最低，先落地可让 P2 的失败更好定位。

---

## 7. 门禁与验收

### 7.1 已验证（有据可查）

- `helix-js` 可交叉 check Windows：`cargo check -p helix-js --all-targets --target x86_64-pc-windows-msvc` → 0 error / 0 warning（`e9b78c87e` 的验收记录，12 个错清零）。

### 7.2 待实测（写在这里是为了不做成假承诺）

| 项 | 状态 | 处理 |
|---|---|---|
| `helix-term` 能否同法交叉 check | **未验证** —— 其 `build.rs` + tree-sitter 编译可能拦路 | P7 之前先跑一次；跑不通就退化为「只守 `helix-js` + `helix-stdx`」，并在 spec 与 CI 注释里写明覆盖边界 |
| Windows **运行时**验证（真跑 `cargo test`） | **未验证** | 本轮所有承诺按「交叉 check + Linux 全量测试」验收，**不宣称 Windows 测试已绿**。真绿必须有一次 Windows runner 或 VM 实跑 |

### 7.3 新增门禁（P7）

```bash
cargo check -p helix-js -p helix-stdx --all-targets --target x86_64-pc-windows-msvc
```

理由：今天 Windows 专有编译错要等 Windows runner 才暴露（`e9b78c87e` 的 12 个错误就是这么来的）。这条在 ubuntu runner 上约 10s 就能拦住，是本次改造的**止损线**。

### 7.4 回归门禁（沿用现有 8 道，一条不减）

`cargo fmt --all --check` · `clippy`（dev `--features integration` + release 两种口径） · `cargo test --workspace --lib` · `cargo test -p helix-js` · `cargo integration-test` · `cargo xtask api-check`（三道） · `cargo xtask docgen` 零 diff · `tsc --noEmit -p plugins/jsconfig.json`。

**基线数字一律以改动前的命令输出为准**（`architecture-audit.md` 里的 154 成员 / 377 测试等数字已漂移过，不复用旧值）。

---

## 8. 风险与回退

| 风险 | 概率 | 处理 |
|---|---|---|
| P4 抽接口时把「注册 fd 早于 spawn」的顺序弄反 ⇒ resize 竞态 | 中 | 保留 `shell.rs:505` 的顺序与注释；`plugin_terminal_*` 集成测试覆盖 |
| `TERM_MASTERS` 换 `Arc<dyn>` 后生命周期变化 ⇒ fd 提前关闭 | 中 | `PtyResize` 只持 `RawFd`（不 own），`Drop` 仍归 `PtyMaster`，与今天一致 |
| `js_run` 的 union 判别写错 ⇒ 字符串被当 argv | 低 | 显式判 `is_array()`，其余一律按字符串（向后兼容优先） |
| xtask 版与 `.sh` 版的 `cp -R` 语义差异（符号链接 / 权限） | 中 | dry-run 逐行比对；符号链接语义见 D4 |
| `cmd /C` 的引号/转义语义与 `sh` 不同 ⇒ 插件字符串形态在 Windows 悄悄做错事 | **高**（既有风险） | 文档显著警告；N2 把内置插件全部改 argv；字符串形态在 Windows **不承诺**跨平台语义 |

**回退策略**：每个提交独立，`git revert <P4>` 即可回到「Unix-only PTY + 抽象前」的状态，不影响 P1/P2/P3/P5/P6。

---

## 9. 并行冲突与排期

同窗口另一处运行正在做 **which-key / dap** 相关文件。

| 提交 | 落点 | 冲突评估 |
|---|---|---|
| P1 | `helix-js/src/commands.rs`、`helix-term/src/{commands/typed,server_manager}` | ✅ 不重叠 |
| P2 | `helix-js/src/{platform.rs,commands.rs,shell.rs}` | ✅ 不重叠（新建文件 + 局部改动） |
| P3 | **`plugins/helix.d.ts`**、`docs/api/process-fs.md`、`helix-js/src/api.rs` | ⚠️ **`helix.d.ts` 可能与对方重叠** —— 动手前先看该文件近期是否被改过 |
| P4 | `helix-js/src/{pty/**,state.rs,shell.rs,popup.rs}` | ✅ 不重叠 |
| P5/P6 | `xtask/**`、两个 `.sh`（改为薄封装） | ✅ 不重叠 |
| N2 | `plugins/**/*.js`（5 个插件） | ❌ **本轮不做**，等对方跑完 |

---

## 10. 待确认的决定

| # | 决定 | 建议 |
|---|---|---|
| **D1** | `helix.spawn({cmd})` 本轮是否也接受 argv | **否**（N3）：与 ConPTY 一起做，否则语义半套 |
| **D2** | 三处 `HOME` 落到 `home_dir()/.local/state/...`（保持今天路径形状）还是 `data_dir()` / `cache_dir()`（符合 XDG / Windows 惯例） | **先 `home_dir()` 保等价**；路径规范化单独立项 —— 否则 P1 会把「修 bug」与「改路径」两件事混进一个提交 |
| **D3** | P7 若 `helix-term` 交叉 check 跑不通，是否接受「只守 `helix-js` + `helix-stdx`」 | **接受**，但必须在 spec 与 CI 注释写明覆盖边界 |
| **D4** | `install-plugins` 对**符号链接**：跟随（复制内容）还是保留链接 | **跟随**（与 `cp -R` 默认一致），help 里写一行 |
| **D5** | `.sh` 保留薄封装还是直接删 | **保留** —— `contrib/`、`plugins/tools/` 的路径在多个文档中被引用，删改面大于收益 |

---

## 附：本轮明确不碰的文件（防顺手改）

- `helix-term/src/ui/plugin_terminal.rs` —— 渲染 / vte 消费端**已经就绪**（`alacritty_terminal` 的 `vte::ansi::Processor`），ConPTY 落地时天然复用；
- `plugins/**/*.js`（N2）；
- `runtime/**`（红线：改的是源，同步靠 `cargo xtask sync-runtime`）；
- `CHANGELOG.md`（发布流程填）；
- 上游遗留跨平台代码（`helix-view/src/clipboard.rs`、`helix-loader/src/grammar.rs`、`helix-tui/src/backend/crossterm.rs`）。
