# DAP 零配置启动（自动构建 / 自动推导 program）与「重启」兜底 设计

> 状态：**设计稿**（待评审，不包含实现）
> 关联：`docs/superpowers/specs/2026-10-25-dap-redesign-design.md`（主设计）、
> `docs/api/dap.md`（现有 API/命令）、`helix-term/src/dap_config.rs`（三级配置 + 变量替换）

---

## 0. 结论摘要

1. **不再让用户手填二进制路径**：新增「项目检测」——按语言/构建系统推导 `(build 命令, program 路径)`，
   `:debug-start` 的 Picker 直接给「构建并启动」，模板 Prompt 预填检测到的路径；
   无配置也能跑，`launch.toml` 里的 `build` / program 显式覆盖检测结果。
2. **`r`（重启）不再受 adapter 能力限制**：adapter 支持 DAP `restart` 就用它；不支持（**lldb-dap 就是
   `supportsRestartRequest: false`**）就退化成**客户端重启** = 用本编辑器记住的 launch 参数重新开会话，
   断点表复用、面板不重建。
3. **能力差异显式化**：`?` 帮助里打印当前 adapter 的能力矩阵（✓条件断点 / ✗restartFrame / …），
   而不是按了才知道不支持。

---

## 1. 现状：体验断点在哪（都有代码位置）

| 现象 | 根因 |
|---|---|
| 必须手动填二进制路径 | 语言模板 `completion = [{name:"binary", completion:"filename"}]` ⇒ `debug_parameter_prompt` 弹一个**空白 Prompt**（`commands/dap.rs:487`）。检测/构建能力为零 |
| `program` 不存在时启动报错很晚 | 没有"先构建/先检查产物"的步骤，错误来自 adapter stderr（lldb-dap: `unable to find executable`） |
| `r` 直接失败 | `dap_restart`（`commands/dap.rs:565`）要求 `supports_restart_request == true`；**lldb-dap 报 false** ⇒ `Debugger does not support session restarts`。可重启是调试最高频动作之一 |
| `binary (terminal)` 模板跑不起来 | 它走 DAP 反向请求 `runInTerminal`，实现依赖 `[terminal]` 外部终端配置（`helix-view/src/handlers/dap.rs:586`）；没配就 `No external terminal defined` |
| `s t` / `s f`（切线程/帧）在部分 adapter 下无效 | 这两个是 client 侧实现，不依赖 caps；但**线程表要等到 `thread` 事件**才填充，`pause` 时机不对就是空表（体验上像"不支持"） |

adapter 能力（本机 lldb-dap 实测）：

```
✓ supportsConditionalBreakpoints   ✓ supportsHitConditionalBreakpoints
✓ supportsExceptionOptions         ✓ supportsSetVariable
✓ supportsConfigurationDoneRequest ✓ supportsDelayedStackTraceLoading
✗ supportsRestartRequest           ✗ supportsRestartFrame
✗ supportsModulesRequest           ✗ supportsLoadedSourcesRequest
✗ supportsGotoTargetsRequest       ✗ supportsStepInTargetsRequest
✗ supportsStepBack                 ✗ supportsMemoryReferences
```

---

## 2. 设计原则

1. **机器能推出来的，不让人填**；配置永远是覆盖项，不是前置条件。
2. **构建是显式动作**：`:debug-start` 不会偷偷跑 `cargo build`；"构建并启动"是你**选**的那一行。
3. **能力差异必须显式**（✓/✗ 列出来），不能靠"按了报错"发现。
4. **复用现有通道**：构建输出走 dapui 的 Output tab（已有 `dap-output` 事件），不新开 UI；
   构建完成/失败用状态栏 + Output 里的红色行。

---

## 3. A 部分：自动构建 + program 自动推导

### 3.1 配置面（向后兼容，纯增量）

```toml
# .helix/launch.toml
[[config]]
name = "run"
language = "rust"
build   = "cargo build"                                      # 新增：构建命令（空 = 不构建）
program = "${workspaceFolder}/target/debug/${cargoBin}"      # 新增变量
cwd     = "${workspaceFolder}"
```

新增变量（仅在启动配置/模板里解析，`dap_config.rs` 的同一张替换表）：

| 变量 | 取值 |
|---|---|
| `${cargoBin}` | `Cargo.toml` 的 `[[bin]].name`；没有就 `package.name`（`toml` 已是依赖） |
| `${cargoProfile}` | `debug` \| `release`（来自 `:debug-start --release` 或配置里的 `profile`） |
| `${program}` | 检测结果的 program（只在 `build` 命令里可用，如 `chmod +x ${program}`） |

`build` 的语义：**一条 shell 命令**，cwd = `${workspaceFolder}`，非 0 退出 ⇒ 不启动会话。
（不做 VS Code 的 `preLaunchTask` 任务 DSL —— 复杂需求请写脚本，`build = "./scripts/dev-build.sh"`。）

### 3.2 自动检测（`helix-term/src/dap_build.rs`，纯函数）

输入：`(doc_path, workspace_root)`；输出：

```rust
pub struct Detected {
    pub build: Option<String>,   // 构建命令
    pub program: Option<PathBuf>,// 产出的可执行文件
    pub reason: String,          // 展示用："Cargo.toml · [[bin]] app"
}
```

检测优先级（**只看文件，不执行任何命令**，便于单测）：

| 信号（按优先级） | build | program |
|---|---|---|
| `Cargo.toml` | `cargo build` / `cargo build --release` | `target/{debug,release}/<cargoBin>` |
| `CMakeLists.txt` + `cmake-build/` | `cmake --build <build dir>` | `CMakeCache.txt` 里 `*_OUTPUT_DIRECTORY` + 目标名；拿不到就列 `build/**` 下可执行文件候选（Picker） |
| `Makefile` / `makefile` | `make` | 扫 `*`（可执行位 + 最近 mtime）候选 |
| 单文件 `*.c` / `*.cpp`（无构建系统） | `cc -g -O0 -o <stem> <file>` | `./<stem>` |
| `go.mod` | `go build -o <dir>/<dir> .` | `./<dir>` |
| `pyproject.toml` / 直接 `.py`（debugpy） | — | `${file}`（不需要二进制） |
| `package.json`（js/ts adapter） | — | `${file}` |

`--release` 开关：`:debug-start --release` 或配置 `profile = "release"` 时，`cargo build` 加 `--release`、
路径用 `target/release/`。

**歧义处理**：多个候选可执行文件时不猜 —— Picker 里列成多行（每行显示路径 + mtime），
选中即用；`:debug-start binary` 的 Prompt 里预填**第一个**候选并把光标放末尾。

### 3.3 三个使用路径

1. **`:debug-start` Picker（主路径）**
   - 现有三级来源（toml/json/lang）照旧；
   - 选中项若 `program` **不存在**，展开两行：
     `▶ 用检测到的产物启动   target/debug/app` 和 `▶ 构建并启动   cargo build`
     （`program` 不存在且没有 build 命令 ⇒ 一行提示 + 「打开 launch.toml」）；
   - `program` 存在 ⇒ 直接启动（行为不变）。
2. **`:debug-build`（新命令）**：只构建、不启动。输出进 dapui 的 Output tab
   （新增事件 `dap-build`，`{phase:"start"|"line"|"done", text, exit_code}`），
   状态栏：`构建中… / 构建完成 (2.3s) / 构建失败 (exit 101)`。
3. **模板 Prompt 预填**：`debug_parameter_prompt` 的初值取检测结果（`filename` 类 completion 才预填），
   光标在末尾 ⇒ 回车就启动，再也不用背 `target/debug/...`。

### 3.4 构建执行器

- 用现成 `Jobs` + `Command::new(shell).arg("-lc").arg(build)`（Windows: `cmd /C`），cwd = workspace；
- 行级输出 → `dap-build` 事件 → dapui Output（stderr 用 `error` 样式，与现有 `dap-output` 一致）；
- 同一 workspace 同时只允许一个构建（第二次直接拒绝：`已有构建在跑`）；
- 取消：`:debug-build-cancel`（kill 进程组）；
- **缓存**：`program` 的 mtime ≥ 所有源文件 mtime ⇒ 跳过构建并提示 `产物是最新的（跳过构建）`；
  配置 `build.always = true` 可关掉这个优化（默认开）。

### 3.5 失败必须可见（三处，文案固定）

| 场景 | 状态栏/Output |
|---|---|
| 构建命令起不来 | `构建命令无法执行: <os error>` |
| 构建非 0 退出 | `构建失败 (exit N)；先看 Output（:dapui-output）` |
| 构建成功但产物不存在 | `构建完成但 <path> 不存在（检测到: …）` |

---

## 4. B 部分：`r` 重启（客户端兜底）+ 能力显式化

### 4.1 现状

`dap_restart` 只在 `supports_restart_request` 时可用；lldb-dap 是 `false` ⇒ `r` 直接失败。

### 4.2 设计：能力驱动 + 客户端重启兜底

```
restart():
  if caps.supports_restart_request && starting_request_args.is_some():
        DAP restart                       # 最快，adapter 进程不换
  else if starting_request_args.is_some():   # ← lldb-dap / debugpy 走这条
        terminate(terminateDebuggee = caps.supportsTerminateDebuggee)
        spawn 同一份 (adapter 配置, request, args, socket)   # 复用 start_session_with
        断点表自动重放（Initialized 已有）· dapui 不重建
  else:
        error: "这条会话不是本编辑器起的（attach），无法重启；用 :debug-start 重开"
```

要点：
- 记住启动参数：`Client.starting_request_args` 已有；再需要 `adapter 配置`（`Client.config` 有）
  ⇒ 客户端重启不需要新增存储。
- 重启后的会话是**新 session id**：dapui 的 `active_session()` 会自动指向新会话（`set_active_client`
  已在新会话 `Initialized` 时执行），变量树 `load_roots` 在 `dap-session` 事件里重拉即可。
- `r` 的文案按能力分：`重启会话` / `重开会话（adapter 不支持 DAP restart，改用重开）`。

### 4.3 能力矩阵显式化

`?`（dapui 帮助）里多一段：

```
adapter: lldb-dap
  断点     ✓ 条件 ✓ 命中条件 ✓ 日志
  异常     ✓ 过滤器 cpp_catch / cpp_throw / …
  执行     ✓ continue/step ✓ pause   ✗ stepBack   ✗ restartFrame
  会话     ✗ DAP restart（r 会重开会话）  ✓ terminate   ✗ modules   ✗ memory
```

数据来源：`sessions()` 快照里加 `caps: {...}`（与 2026-10-25 加的 `filters` 同一处：
`dap_sessions_json` 直接读 `client.caps`），dapui 只做渲染，不新增 API 方法。

### 4.4 `runInTerminal`：别让它"看起来能用"

现状：`binary (terminal)` 模板发 `runInTerminal`，我们让它落到 `editor.terminal` 外部终端
（未配置 ⇒ `No external terminal defined`）。两种收尾（评审选一个）：

- **A（小）**：Picker 里给这条模板加后缀 `(需要外部终端配置 [terminal])`，并在 `?` 的能力段标出来；
- **B（中）**：回落到**内置 terminal 面板**（`:hterm`）跑命令，没有外部终端也能用。

建议先 A（0 风险），有需求再 B。

---

## 5. 交付拆分（每步独立可验证）

| # | 内容 | 落点 | 测试 |
|---|---|---|---|
| 1 | `dap_build::detect()` 纯函数（rust / cmake / make / 单文件 C / go / python / node） | `helix-term/src/dap_build.rs` | 单测（fixture 目录树）；无副作用 |
| 2 | 新变量 `${cargoBin}` `${cargoProfile}` `${program}` | `helix-term/src/dap_config.rs` | 变量替换单测（已有同类 13 条） |
| 3 | `:debug-build` + 构建执行器 + `dap-build` 事件 + dapui Output 渲染 | `commands/dap.rs` · `plugins/dapui` | 集成（假 shell 脚本）+ node（事件渲染） |
| 4 | `:debug-start` Picker 的「构建并启动」/预填 Prompt / `--release` | `commands/dap.rs` | 集成（Picker 行断言，参照 `plugin_dap_config.rs`） |
| 5 | `restart()` 客户端兜底 | `commands/dap.rs` | 集成（假 adapter 的 `restart` 拒绝场景） |
| 6 | `caps` 进 `sessions()` + `?` 里的能力矩阵 | `commands/dap.rs` · `plugins/dapui` | node（渲染断言） |
| 7（可选） | `runInTerminal` 回落到内置 terminal 面板 | `helix-view/src/handlers/dap.rs` | 集成（真 adapter 不方便 ⇒ 假反向请求） |

工作量粗估：1+2 半天（纯函数 + 单测），3+4 一天（执行器 + UI），5 半天，6 两小时，7 视选 A/B。

---

## 6. 明确不做 / 风险

- **不做**：VS Code `preLaunchTask` / `compounds` / `inputs`；多配置并行构建；构建产物缓存管理
  （交给 cargo/make）；attach 场景自动猜二进制（attach 本来不需要 program）。
- **不做隐式构建**：`:debug-start` 绝不自动跑 build（除你选了"构建并启动"那一行）。
- 风险 1：`CMake`/`Makefile` 的产物检测天然是启发式 ⇒ 一律给候选列表让人选，不静默猜。
- 风险 2：客户端重启会换 session id ⇒ 依赖 `dap-session` 事件重建面板；
  已有 `set_active_client` + `load_roots` 路径，需要在第 5 步补一条回归测试。
- 风险 3：构建命令是 shell 字符串 ⇒ 注入面等于"用户自己的配置"，与 `:sh` 同级，不额外收紧。

---

## 7. 验收清单

- [ ] 全新 Rust 仓库：`:debug-start` → Picker 显示 `run · launch · rust`，`program` 预填
      `target/debug/<bin>`；选「构建并启动」→ Output 里出现 cargo 输出 → 命中 breakpoint
- [ ] 单文件 C（无构建系统）：不填任何路径，`:debug-start` 直接 `cc -g` + 启动 + 命中
- [ ] `r`：lldb-dap 下可重启；断点仍在；面板不闪
- [ ] `?`：能看到 adapter 能力矩阵；`binary (terminal)` 有明确标注
- [ ] 构建失败：状态栏 + Output 红字，会话**没有**被启动

---

## 8. 落地记录（2026-10-26，已实现）

| 设计项 | 落点 | 验证 |
|---|---|---|
| 项目检测 `detect()` | `helix-term/src/dap_build.rs`（纯函数） | 单测 9 条（cargo/workspace/cmake/make/单文件 C/go/python/未知） |
| 新变量 `${cargoBin}` `${cargoProfile}` `${program}` + `build` 字段 | `dap_config.rs`（三级合并后第二遍展开） | 单测 13 条（含 build 里 `${program}`） |
| 构建执行器（非阻塞 job） | `commands/dap.rs::dap_build_run`；输出走 `dap-output`（`category:"build"`） | 真机：`:debug-build` → 状态栏「构建完成」 |
| 构建完自动启动 | `PendingLaunch` 队列 → `dispatch_dap_intents` 里起会话 | 真机：`:debug-start binary`（不带路径）→ 自动 `cc -g -gdwarf-4` → 命中 breakpoint |
| 模板 Prompt 预填 / 单字段直启 | `dap_start_impl` | 同上（不再出现 `{0}` 字面量） |
| Picker 里「产物缺失 ⇒ 先构建再启动」 | `start_or_build` → `launch_or_build` | 真机（同上路径） |
| `r` 客户端重启 | `dap_restart`（`Client::remember_start` 补记 handle 路径的启动参数） | 真机：`r` → 新会话 `#2 ⏸` 停在同一断点，树重新填满 |
| 能力矩阵 | 会话快照 `caps` + dapui `?`（`caps_summary()`） | node 单测 + 真机 `?` |
| `binary (terminal)` 标注 | Picker 标签追加「⚠ 需要 [terminal] 外部终端」（选了 A 方案） | 代码路径（未配外部终端时显示） |

### 8.1 收尾补的三件

| 项 | 落点 | 说明 |
|---|---|---|
| `:debug-start --release` | `commands/typed.rs::debug_start` → `dap_start_impl(..., release)` | 影响语言模板/检测（`cargo build --release` 与 `target/release/…`）；`launch.toml` 的 `build` 由用户自己写 profile |
| **`pane.focus_panel(id)`**（核心新 API） | `helix-js`（`types.rs` / `layout_api.rs` / `api.rs`）+ `ui_op.rs` + `compositor::focus_panel_by_id` | 插件拿不到自己面板的**叶子 id**（`pane.list()` 给的是 leaf id），而 auto_open 不抢焦点之后"把焦点给我开着的面板"是刚需 ⇒ 按 `open_panel` 返回的实例 id 聚焦 |
| `:dapui-focus` / `open_tree` / `open_bottom` 改用 `focus_panel` | `plugins/dapui` | 不再依赖"方向邻居"（两个面板上下叠时方向不一定落在想要的那个）；已开的面板再打开 = 只聚焦不重建 |
| `?` 帮助收短 + 能力矩阵 | `plugins/dapui::show_help` | 状态消息是**拼接**的 ⇒ 帮助必须短，否则胶囊矩阵被宽度截掉 |

真机验证：`:dapui-focus` 后按键直达树（`t` 打出「只有 1 个线程…」而不是被编辑器吃掉）；
`?` 显示 `lldb-dap: ✓条件断点 … ✗DAP-restart`。

未做（与设计一致）：`runInTerminal` 回落到内置终端面板（B 方案）、`preLaunchTask` DSL。

---

## 9. 面板体验收尾（2026-10-26，已实现）

### 9.1 默认显示哪几个面板

```toml
[plugins.dapui]
auto_open = true              # 右侧变量树
auto_open_bottom = "output"   # 底部：off|repl|output|breakpoints（旧 auto_open_repl 仍兼容）
```
两边都开时是"右树 + 底 Output"，命中即同时出现；焦点仍留在编辑器（`focus_on_auto_open = false`）。

### 9.2 栈帧为什么"名字很怪" —— 已分组

`__libc_start_call_main ?:27` / `__libc_start_main@@GLIBC_2.34` / `_start ?:13` 是 **libc/加载器里的
栈帧**（没有源码信息，`source == null`）。以前一股脑平铺 ⇒ 一半是噪音。现在：

- 有源码的帧平铺（`max_frames`，默认 20）；
- 无源码的折成「… N 个系统帧（无源码）」一行，展开才看；
- **当前帧永远第一**（scopes 用的是它）。

### 9.3 变量怎么看

树的结构 = 栈帧 → 作用域（`Locals` / `Globals` / `Registers`）→ 变量（`name = value :type`）。
`max_depth = 2` 时停止后自动展开到第一层作用域的前几个变量；`l`/`Enter` 展开一层，`h` 折叠，
`R` 重取。多帧时只有"当前帧"是展开的，别的帧按 `l` 会现拉它的 scopes。

### 9.4 单步：面板里也能按

`n` 跳过（next）· `i` 进入（step in）· `o` 跳出（step out）· `c` 继续 · `p` 暂停 —— 都是
`helix.dap.next/step_in/step_out/continue/pause`，**不需要先 Esc 回编辑器**。
（`Esc` 仍是"焦点回编辑器、面板保留"。）

### 9.5 断点：面板里也能加

`b` 用**编辑器光标所在行**切花断点（面板的 `onKey` 第二参就是当前 doc 快照，含 `cursor`）；
`space G b` 等价；断点页里还能 `d` 删 / `c` 条件 / `l` 日志 / `e` 异常。

### 9.6 监听变量（Watches，新）

树里 `w` → 输入表达式（单行有选中文本则预填）→ 顶部出现「⌚ Watches」组；每次命中/切帧/单步
都用 `eval(expr, frame)` 重算（帧 = 当前查看的帧），失败显示红字。`W` 清空。

### 9.7 顺带修的两个调度问题（都是"看着像没反应"的根因）

| 现象 | 根因 | 修法 |
|---|---|---|
| 监视值停在 `…`，按一下键才变成 `5 :int` | `helix.dap.*` 的 Promise 兑现后只 `request_redraw`，而 boa 的 `.then` 是入队 job → 没有输入事件就不泵 | 兑现后立刻 `pump_jobs()`（跟随 `cx.js` 宿主）|
| idle 时不泵 job | `should_render \|\| … \|\| self.pump_term_events()` 短路 ⇒ 要渲染就不泵 | 先无条件 `pump_term_events()` 再判断渲染 |
| 弹层关闭后按键落到编辑器（"n 没反应"） | 弹层 pop 后焦点回编辑器 | `ask_text`/`ask_choice` 提交后 `refocus_panel()` |

### 9.8 从编辑器回到面板（`:dapui-focus` 的三种形态）

| 形态 | 行为 |
|---|---|
| `:dapui-focus tree` | 聚焦右侧树（没开就开） |
| `:dapui-focus bottom` | 聚焦底部面板（没开就按上次的 tab 打开） |
| `:dapui-focus`（无参） | 两个都开着 ⇒ **轮换**（树 ↔ 底）；只开一个 ⇒ 聚焦它；都没开 ⇒ 开树 |

`last_panel` 会跟着实际聚焦更新（弹层里提交后 `refocus_panel()` 也更新），所以轮换方向与"你上次在哪儿"一致。

## 10. 过期二进制：显式 `build` 每次跑 + 首次构建落盘 launch.toml（2026-10-28，已实现）

**症状**（真实 case）：Rust 小项目，断点打在 `a = add(1, 2);`，按 `s` 单步却停在别处。
根因不是 step-in，是**跑的是旧二进制**：语言模板没有 `build`，而 §3.3 第 1 条的判定是
"`program` 不存在才构建"，于是改了源码再按 `l` 还是旧产物 —— 旧 DWARF 行号表让断点/单步
落在旧行号上，看起来像"跳错文件"。（按 `r` 才有构建，见 §4。）

**改动一：显式 `build` ⇒ 每次启动前都跑**（`BuildPolicy::Always`）

- `start_with_launch_config`（`.helix/launch.toml` / `.vscode/launch.json` 那条路）不再直接
  `start_session_with`，改走 `launch_or_build(…, BuildPolicy::Always)`；
- 语义就是 preLaunchTask：**写在配置里的构建命令是用户意图**，每次都跑（cargo 增量通常 <0.3s），
  代价远小于"调试一个过期产物"；
- 检测/模板猜出来的 `build` 仍是 `BuildPolicy::IfMissing`（`make all` / `cmake --build` 可能很慢，
  不能因为按一下 `l` 就无谓地跑）。**故意不做 mtime 判定**：多一次全项目 stat 走查，还没解决
  "生成物 / 代码生成 / 环境变量变了" 这些 mtime 看不出来的情况。

**改动二：首次自动构建落盘 `.helix/launch.toml`**

- 位置：`dap_build_run` 里（`helix-term/src/dap_build.rs::launch_toml_document` 生成文本、
  `write_launch_toml_if_absent` 落盘），能同时拿到 `program` 和 `build` 才写；
- 规则：**已存在就不动**（`create_new`，竞态也不覆盖）；工作区有 `.vscode/launch.json` 也**不写**
  —— `launch.toml` 优先级更高，写了会把用户现有配置遮蔽掉；
- 写了就提示一次：`已生成 .helix/launch.toml（build 将随每次启动执行）；构建中: cargo build`；
- 生成的文本必须能被 `dap_config::parse_toml` 原样读回（单测 `launch_toml_document_roundtrips_through_parser`
  钉住，含引号/反斜杠转义）。

**改动三：`PendingLaunch` 带上 `build`**

构建完成起会话时把构建命令交给 `start_session_with` 记住（原来传 `None`），否则"构建完 → 按 `r`
重启"又拿不到构建命令，回到旧二进制。
