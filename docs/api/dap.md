# API:`helix.dap.*`(调试)

> **会用** → [`docs/dap-usage.md`](../dap-usage.md)（上手/键位/任务食谱/排错）
> 总览见 [`docs/plugin-api.md`](../plugin-api.md);设计见
> [`docs/superpowers/specs/2026-10-25-dap-api-p1.md`](../superpowers/specs/2026-10-25-dap-api-p1.md)。

三条入口:**同步快照**(`sessions`/`configs`/`list_breakpoints`,零 IO)、
**Promise**(其余 19 个,失败即 reject)、**事件**(9 个 `dap-*`)。

- `row`/`line` 一律 **0 起始**(与 `helix.lsp.*` / `set_virtual_text` 一致);DAP 协议的 1 起始换算在内核里。
- `id` 省略 = **当前活动会话**;没有会话时**reject**,不静默返回 null。
- `launch`/`attach` 声明了 `process` 能力 ⇒ 插件 manifest 里要写 `permissions: { process: true }`。

```js
// 最小用法:状态栏显示会话 + 命中时列变量
helix.register_statusline_segment("dap", () => {
  const s = (helix.dap.sessions() || [])[0];
  return s ? { text: `#${s.id} ${s.state}` } : null;
});

helix.on("dap-stopped", async (e) => {
  const frames = await helix.dap.stack_frames(e.thread_id);
  const scopes = await helix.dap.scopes(frames[0].id);
  const vars = await helix.dap.variables(scopes[0].variablesReference);
  helix.echo(vars.map((v) => `${v.name}=${v.value}`).join(" "));
});
```

## 成员

| 成员 | 说明 |
|---|---|
| `sessions()` | 会话快照 `[{id,name,state,active,thread?,frame?,filters}]`(同步;`filters` 是 adapter 的异常断点过滤器) |
| `configs()` | 可用调试配置 `[{name,request,language?,source,adapter}]`(P1 只有 `source:"lang"`) |
| `list_breakpoints(path?)` | 断点快照 `[{path,line,condition?,hit?,log?,verified,message?,id?}]`,顺序稳定 |
| `launch(name?, args?, id?)` / `attach(...)` | 起会话;`name` 是 `languages.toml` 的模板名(省略 = 第一个) |
| `terminate(id?)` / `restart(id?)` | 结束 / 重启 |
| `switch_session(id)` | 把某个**已存在**的会话设为 active(纯本地状态,`id` 必填) |
| `continue(id?)` / `pause(thread?, id?)` / `step_in` / `step_out` / `next` | 执行控制(线程省略 = 当前停止线程) |
| `threads(id?)` / `stack_frames(thread,id?)` / `scopes(frame,id?)` / `variables(ref,id?)` | 查询 |
| `eval(expr, frame?, id?)` | 表达式求值 → `{result,type?,variablesReference}` |
| `set_breakpoint(path,row,opts?)` / `remove_breakpoint(path,row)` / `clear_breakpoints(path?)` | 断点(同步改本地断点表,adapter 回填 `verified`)。**不需要活动会话**:无会话时只改本地表并成功兑现,启动后由 `initialized` 事件全量重放 |
| `set_exception_breakpoints(filters)` | 异常断点 |
| `request(name, args?, id?)` | **进阶逃生口**:任意 DAP 请求,响应 body 原样返回 |

## 事件

| 事件 | payload |
|---|---|
| `dap-session` | `{id, kind:"created"\|"removed", name?, state?}` |
| `dap-stopped` | `{id, reason?, thread_id?, text?, all_threads_stopped?}`(**栈帧已拉完**才发) |
| `dap-continued` | `{id, thread_id?}` |
| `dap-exited` / `dap-terminated` | `{id, exit_code?}` / `{id, restart?}` |
| `dap-output` | `{id, category?, output}` |
| `dap-thread` | `{id, reason, thread_id}` |
| `dap-breakpoint` | `{id, path, line, verified, message?, removed?}` |
| `dap-progress` | `{id, kind:"start"\|"update"\|"end", text?, percentage?}` |

## 启动配置从哪来（三级合并）

| 优先级 | 来源 | 说明 |
|---|---|---|
| 高 | `<workspace>/.helix/launch.toml` | 自有格式，可带注释；同名时**逐字段覆盖**下一级 |
| 中 | `<workspace>/.vscode/launch.json` | VS Code 子集（JSONC：允许注释）；`type` 当 adapter 名 |
| 低 | `languages.toml [language.debugger].templates` | 兜底，永远可用（`source: "lang"`） |

```toml
# .helix/launch.toml
[[config]]
name = "run"
language = "rust"            # 省略 = 不按语言过滤
request = "launch"           # launch | attach
program = "${workspaceFolder}/target/debug/app"
cwd = "${workspaceFolder}"
args = ["--flag"]
env = { RUST_BACKTRACE = "1" }
```

支持的变量（**未知变量报错，不静默留字面量**）：
`${workspaceFolder}` `${file}` `${fileBasename}` `${fileDirname}` `${fileBasenameNoExtension}`
`${cwd}` `${env:VAR}`。

**明确不支持**（遇到就报错，不静默忽略）：`preLaunchTask` / `compounds` / `inputs`。
`launch.json` 的 `type`（adapter 名）会被解析成对应语言的 `[language.debugger]`。

`:debug-start`（或 `<space>G l`）的 Picker 里能看到合并结果，`来源` 列即 `toml`/`json`/`lang`。

## 同名 `:` 命令（键位之外的入口）

DAP 动作原先**只有键位**（`<space>G b` 之类），`:` 补全里一个都看不到。现在都接成了 typed 命令：

| 命令 | 别名 | 作用 |
|---|---|---|
| `:debug-start [--release] [模板] [参数…]` | `:dbg` | 起会话。**参数可省略**：能检测到产物就直接用；产物不存在且认出构建方式 ⇒ 先构建再启动；`.helix/launch.toml` / `.vscode/launch.json` 里**写了 `build` 则每次启动前都跑**。`--release` 影响语言模板/项目检测那条路（`cargo build --release`、`target/release/…`）|
| `:debug-build [--release]` | `:dbgb` | 只构建、不起会话（输出进 dapui 的 Output tab） |
| `:debug-toggle-breakpoint` | `:dbgb` | 当前行断点开关 |
| `:debug-continue` / `:debug-next` / `:debug-step-in` / `:debug-step-out` | `:dbgc/:dbgn/:dbgi/:dbgo` | 执行控制 |
| `:debug-eval <expr>` | — | 在当前帧求值（结果显示在状态栏） |
| `:debug-variables` | `:dbgv` | 弹层列作用域/变量（Picker） |
| `:debug-terminate` / `:debug-restart` | `:dbgq` | 结束 / 重启 |
| `:debug-sessions` | — | 多会话切换 |

`helix.dap.*` 与这些命令**共用同一份实现**（内部都走 `DapIntent` → `helix.dap.*` 的派发路径）。

## 状态栏 / 面板

- 状态栏:段名 **`dap`** 由内置插件 `dapui` 注册(`helix.register_statusline_segment`),
  把它写进 `[plugins.statusline] left/center/right` 即显示。
- 面板:内置插件 `dapui` —— **右侧**面板是变量树(`:dapui`,面板内 `t` 切线程 / `f` 切栈帧);
  **底部**面板是 `REPL / Output / Breakpoints` 三 tab(`:dapui-repl` / `:dapui-output` /
  `:dapui-breakpoints`,面板内 `Tab` 循环、`1/2/3` 直达)。
  断点页键位:`d` 删 · `c` 条件 · `l` 日志消息 · `e` 异常断点 · `Enter` 跳转。
  未验证的断点会把 adapter 给的原因一起显示（`◯ !no executable` 这种）。
  变量树键位（**面板聚焦时**）：`jk/l/h` 移动展开 · `t/f` 线程/帧 · `s` 会话 ·
  **`n/i/o` 单步(跳过/进入/跳出)** · `c` 继续 · `p` 暂停 · **`b` 在编辑器光标行切花断点** ·
  **`w` 加监视 · `W` 清空监视** · `R` 重取 · `Esc` 回编辑器 · `q` 关。
- 帧分组：**有源码的**帧平铺（`max_frames` 个），`__libc_start_main` 这类无源码的折成
  「… N 个系统帧（无源码）」一行；当前帧永远排第一。
- 监视：`w` 输入表达式（单行有选中文本时用它预填），每命中/切帧用 `eval(expr, frame)` 重算，
  显示在树顶「⌚ Watches」；失败显示为红字节点。
- **调试键（gdb 式：一套键、无模式）**：会话**暂停**时下面这些键在编辑器里**直接生效**，
  不需要进任何模式、也不看焦点在哪（面板里被吞的键会冒泡到这里）：

  | 键 | 动作 | 键 | 动作 |
  |---|---|---|---|
  | `n` | 单步跳过 | `s` | 单步进入 |
  | `o` | 单步跳出 | `c` | 继续 |
  | `p` | 暂停 | `T` | 结束会话 |
  | `r` | 重启（**会先重建**：会话记住了起它时的构建命令） | `b` | 切换断点 |
  | `t` | 切线程 | `f` | 切栈帧 |
  | `S` | 切会话 | `v` | 变量弹层 |
  | `R` | 跑到光标 | `K` | 光标下求值 |
  | `C-c` / `C-l` | 条件 / 日志断点 | `e` / `E` | 启用 / 禁用异常 |
  | `?` | 键位速查 | | |

  **编辑完全照常**（`i` 还是插入、`d` 还是删除）；运行中这些键不接管（`n` 就是"下一个匹配"）。
  `<space>G` 是同一张表的**菜单形态**（暂停时用不到，但 `l` 启动/断点/会话这类不暂停也能做的动作在里头）。
- **命中后的三件事**（gdb 里"你看得见自己在哪"）：
  1. 状态栏：`add  —  Thread 1  /path/hello.c:4`（函数 + 线程 + 位置）；
  2. 光标与视口**跟到停止行并居中**（只在你当前 buffer 就是它时移动，不抢 buffer）；
  3. 右侧树头部一行 Locals 摘要：`#1 ⏸ t1  │ a=2 b=3 s=5`。
- **多会话**：树里每个会话一个节点（`▾ #2 ⏸ t1 (当前)` / `▸ #1 ⏸ t5`），当前会话展开、
  其余折叠；选中别的会话按 **`Enter`** 直接切过去（不必记 `S` 弹层），展开它也能看栈帧。
  `[plugins.dapui] show_all_sessions = false` 或只有单会话时保持老样子（帧直接铺在顶层）。
- **dapui 面板**只在**有会话**时出现（`auto_open`），命中即开；正常退出（exit 0）自动全部收起，**异常退出**则保留底部并切到 `Output`（崩溃信息在那儿）；
  面板里只保留"跟选中项有关"的键：`j k h l g G` 导航/展开、`Enter` 跳转、`w W` 监视、
  `d` 删监视、`e` 设值、`y` 复制、`/` 过滤、`q` 关、`Esc` 回编辑器 —— 其余键**一律冒泡**给同一张表。
  （`inline_values` 仍是实验项、默认关：`set_virtual_text` 只能插在列 0，会把代码右移；Locals 走树头部摘要。）
- **真只读**：`:read-only`（当前 buffer）/ `:read-only all`（全部打开的 buffer）切换**强制只读**；
  闸门在 `Document::apply`（命令编辑 / 插件 `doc.insert` / LSP code action 全覆盖）+ `:write`。
  调试联动：`[plugins.dapui] readonly_on_stop = "off"|"doc"|"all"`（继续/结束时自动解除，
  只撤销自己置的那次，不会解开你手动的只读）。JS 侧对应 `helix.set_readonly(on, all?)`。
- **调试视角**：`[plugins.dapui] perspective = true` —— 命中时记住当前布局（`helix.layout.get()`），
  继续/结束时原样恢复（`restore()`），不覆盖你此刻的布局。
### 自定义调试键：`[keys.debug]`

调试键表（暂停时接管的那一张）现在**可以用配置覆盖**——语法与 `[keys.normal]` 完全一致，
写在引擎内置表**之上**合并（只写你要改/加的键）：

```toml
[keys.debug]
"A-1" = "@:dapui-focus tree<ret>"     # 暂停时一键跳进变量树
"A-2" = "@:dapui-focus bottom<ret>"   # 暂停时一键跳到底部面板
"i"   = "dap_step_in"                 # 例：把 i 也变成单步进入（代价：暂停时按 i 不能进插入）
```

- 生效范围：**只影响"暂停时的接管"**；不暂停时 `[keys.normal]` 那套照旧。
- 覆盖关系：`[keys.debug]` 里的同名键**替换**内置（如 `"s" = "dap_next"`），新键**追加**。
- 提示自动跟随：which-key 的 `Debug ▸ …` 会显示你加的键（提示直接读合并后的表）。
- `:config-reload` 即刻生效（表在 `Config::load` 时重建）。

> 注：`[keys.debug]` 是**唯一**的非 `Mode` 段名。写错其它名字（如 `[keys.nromal]`）会被
> **警告并忽略那一段**，其余配置照常生效（以前会让整份 config 报 `Invalid mode`）。

### `[plugins.dapui]` 全部配置

| 键 | 默认 | 作用 |
|---|---|---|
| `auto_open` | `true` | 命中停止时自动开右侧变量树 |
| `auto_open_bottom` | `"off"` | 同时开底部并切到该页：`off`/`repl`/`output`/`breakpoints` |
| `auto_open_repl` | `false` | （旧）等价于 `auto_open_bottom = "repl"` |
| `focus_on_auto_open` | `false` | 自动开面板时**抢焦点**（默认不抢，你继续按调试键） |
| `panel_size` | `34` | 右侧树宽度（列） |
| `max_frames` | `20` | 树里最多列几个"有源码"的栈帧（无源码折成一行） |
| `max_depth` | `2` | 停止时自动展开到第几层（0=全折叠） |
| `max_nodes` | `2000` | 树节点上限（超出截断） |
| `show_all_sessions` | `true` | 多会话并排（当前展开、其余折叠，`Enter` 切换） |
| `breakpoint_marks` | `true` | 断点行首画 `●`/`◯` |
| `inline_values` | `false` | （实验）Locals 写成行首虚拟文本（会右移） |
| `highlight_current_line` | `true` | 高亮当前执行行 |
| `current_line_scope` | `"dap.current-line"` | 用哪个主题 scope 画当前行 |
| `readonly_on_stop` | `"off"` | 命中时强制只读：`off`/`doc`/`all` |
| `perspective` | `false` | 借走布局、继续/结束时还原 |

- **零配置配方**：C/C++/Rust/Go 走 `dap_build` 自动检测+构建（`:debug-start` 不带参数即可）。
  脚本语言（`.py/.js/.ts`）会被当成 "program = 当前文件"，但**适配器命令要自己配**：
  ```toml
  # ~/.config/helix/languages.toml
  [[language]]
  name = "javascript"
  scope = "source.js"
  file-types = ["js", "mjs", "cjs"]
  [language.debugger]
  name = "js-debug-dap"
  transport = "tcp"
  port-arg = "{} 127.0.0.1"
  command = "node"
  args = ["/path/to/js-debug/src/dapDebugServer.js"]   # 不配 → 报错会直接告诉你缺什么
  [[language.debugger.templates]]
  name = "source"
  request = "launch"
  completion = [ { name = "main", completion = "filename" } ]
  args = { program = "{0}", skipFiles = [ "<node_internals>/**" ] }
  ```
  **js-debug 探测**：`js-debug-dap` 的模板不带 `command`，编辑器会在
  `~/.vscode{,-server,-insiders}/extensions/ms-vscode.js-debug-*/src/dapDebugServer.js`
  与 `~/vscode-js-debug/src/dapDebugServer.js` 里找（多版本取最近修改），找到就自动
  `node <server>`；找不到仍给"怎么配"的报错。
  **python 零配置已可用**：`pip install debugpy` 之后，在 `.py` 上 `:debug-start` 直接跑当前文件
  （内置模板见 `languages.toml` 的 `[language.debugger] name = "debugpy"`）。
  两个坑都修了：① debugpy 的 `launch` 要等 `configurationDone` ⇒ launch 必须在客户端进注册表
  之后再发（`DapIntent::StartRequest`）；② debugpy 会把被调试进程设成 tty 前台进程组、把编辑器
  变成后台组 ⇒ 读 tty 拿 EIO、按键全死 ⇒ spawn adapter 时 `setsid()` 隔离。

- **REPL（底部第 1 页）**：`Enter` 求值（当前帧上下文）· **`Tab`/`Shift-Tab` 补全并循环** ·
  `↑/↓` 历史 · **`C-l` 清屏** ·
  **未配平括号自动续行**（`foo({` 回车后提示 `... `，补齐再回车才求值）·
  历史持久化到 `~/.cache/helix/dapui-repl-history`（最多 200 条，多行表达式按 `\n` 转义存一行）。
  多行草稿按真实换行画（最多 4 行 / 面板一半高），转录区自动收缩。
  注意 `q` 在 REPL 页是**普通字符**（要能输入），关面板用 `:dapui-close`。
  补全走 DAP 的 `completions` 请求（lldb-dap 支持），候选条画在提示行上一行 `[2/7] …`；
  lldb 会把文档塞进 `label`（`s -- int &`）并塞一个空候选，两者都已处理。
- **断点可见**：断点行首会画标记 —— `●` 已验证 / `◯` 未验证（`[plugins.dapui] breakpoint_marks = false` 可关；
  实现只能插在列 0，所以该行文字会右移约 2 格）。
- **REPL**：`Esc` 在**有输入时先清输入**（gdb 里 `C-c` 的手感），清空后再按才回编辑器。
- **复制帧位置**：树里选中栈帧按 `y` = 复制 `file:line`（贴 issue/聊天用）；变量上 `y` 仍复制值。
- **构建输出是流式的**：`:debug-start` 触发的构建（`dap_build` 检测 + `make`/`cargo`/`cmake`）
  逐行推 `dap-output` 事件 ⇒ 底部面板**第一行到达就自动打开并切到 Output**，`cargo build` 期间
  不再是一片空白。实现是进程级小队列（`helix-term/src/dap_output.rs`，4096 行上限，
  worker 线程 push + `request_redraw` 唤醒，主线程在 `pump_term_events` 里 drain 成事件）。
- **协议流水**：`:debug-log [N|clear]` 把最近 N 条 DAP 收发放进临时 buffer
  （环形缓冲保留 500 条，不需要 `-vv`、不用翻 `helm.log`）。
- 默认显示哪些面板：`[plugins.dapui] auto_open`（右侧树）· `auto_open_bottom = "off"|"repl"|"output"|"breakpoints"`。
  (为什么不是 Picker:核心合同规定纯字符键永不交给 picker 的 `onKey`,那三键在 picker 里收不到。)
  配置见 `[plugins.dapui]`(`panel_size` / `bottom_size` / `auto_open` / `auto_open_repl` /
  `inline_values` / `highlight_current_line` / `current_line_scope` / `max_depth` / `max_nodes`)。
- 编辑器内:命中时**高亮当前执行行**(默认开,scope `dap.current-line`,主题没定义时按
  `ui.cursorline.primary` 的底色派生)+ 行尾内联值(默认关)。

## 不用手填二进制（项目检测）

`:debug-start` / `space G l` 的 Picker 会按项目**自动推导** `(build, program)`，规则：

| 项目信号 | 构建命令 | 产物 |
|---|---|---|
| `Cargo.toml` | `cargo build [--release]` | `target/{debug,release}/<[[bin]] 或 package name>` |
| `CMakeLists.txt` | `cmake --build <build 目录>` | 构建目录下最新的可执行文件 |
| `Makefile` | `make` | 顶层/`bin/` 下最新的可执行文件 |
| 单文件 `*.c` / `*.cpp` | `cc -g -gdwarf-4 -O0 -o <stem> <file>` | 同目录 `<stem>` |
| `go.mod` | `go build -o <dir> .` | 同目录 `<dir>` |
| `*.py` / `*.js` | 不需要构建 | 当前文件 |

覆盖方式：`.helix/launch.toml` 的 `build = "…"` / `program = "…"`（新增变量
`${cargoBin}` `${cargoProfile}` `${program}`）。

产物不存在时**先构建再启动**（输出在 `:dapui-output`）；构建失败/产物缺失都会明确报错且**不启动**会话。

### 显式 `build` 每次启动前都跑（= preLaunchTask）

`.helix/launch.toml` / `.vscode/launch.json` 里写了 `build` ⇒ **每次** `l`/`:debug-start`
前都执行它（cargo 增量，通常 <0.3s）。检测/模板猜出来的 `build` 仍只在"产物不存在"时跑。

为什么：检测路只有"产物缺失才构建"，于是改了源码再按 `l` 跑的还是旧二进制，而旧 DWARF
行号表会让断点/单步落在旧行号上（看起来像"跳错文件/跳错行"）。写在配置里的 `build` 是
用户自己的意图，按 preLaunchTask 语义每次都跑才是对的。

### 首次构建自动生成 `.helix/launch.toml`

第一次自动构建（能检测出 `program` + `build` 时）会往项目里写一份 `.helix/launch.toml`：

```toml
[[config]]
name = "temp"
request = "launch"
language = "rust"
program = "/abs/path/target/debug/temp"
cwd = "/abs/path"
build = "cargo build"
```

规则：**文件已存在就一个字都不动**（那是用户写的）；写入成功时状态栏提示一次
（`已生成 .helix/launch.toml（build 将随每次启动执行）`）。

## `r`（重启）在 lldb-dap 上也能用

- adapter 支持 DAP `restart` ⇒ 直接发（快，进程不换）；
- 不支持（**lldb-dap 实测 `supportsRestartRequest: false`**）⇒ **客户端重启**：用记住的
  `(adapter, launch/attach, 参数, socket)` 开新会话，旧会话随后断开。断点表自动重放，
  dapui 面板不重建；`dapui` 的事件已按会话 id 过滤，旧会话的收尾事件不会清掉新会话的装饰。

能力矩阵在哪看：dapui 面板里按 `?`（`lldb-dap: ✓条件断点 · ✓命中条件 · ✓日志断点 · ✓异常过滤器(6) ·
✓改变量 · ✓terminate · ✗DAP-restart · ✗restartFrame · ✗stepBack · ✗modules · ✗memory`）。
**回到面板**（`Esc` 出来之后）：

| 想去 | 命令 | 说明 |
|---|---|---|
| 右侧变量树 | `:dapui-focus tree` | 没开就开，已开只聚焦 |
| 底部面板 | `:dapui-focus bottom` | 没开就按上次的页打开（REPL/Output/Breakpoints） |
| 底部指定页 | `:dapui-breakpoints` / `:dapui-repl` / `:dapui-output` | 已开 = 切页 + 聚焦 |
| 两个之间来回 | `:dapui-focus`（不带参数） | 两个都开着时**轮换**：树 ↔ 底 |
| 任意叶子 | `C-p` + `h/j/k/l` | 窗口模式（与其它 pane 一致） |

也可以自己绑一个键，例如 `config.toml`：

```toml
[keys.normal]
"A-1" = ":dapui-focus tree<ret>"
"A-2" = ":dapui-focus bottom<ret>"
```

## 调试不通时先看这两条

1. **断点没拦住**:先看 adapter 的 `setBreakpoints` 回包。用 `hx -vv` 起编辑器,
   `~/.cache/helix/helix.log` 里会看到
   `dap -> setBreakpoints (...) {"breakpoints":[{"line":N}]}` 与 `dap <- setBreakpoints ok {"breakpoints":[]}`。
   回**空数组** = adapter 认不出这行,不是 helix 没发。
   最常见原因:**adapter 的 lldb 太老,读不了编译器的新 DWARF**。例如
   `lldb 10` + `gcc 11`(默认 DWARF 5)会在 lldb 里报
   `unsupported DW_FORM value: 0x1f` / `Breakpoint: no locations (pending)`,
   用 `-gdwarf-4` 重编就能命中:
   ```sh
   cc -g -gdwarf-4 -O0 -o app main.c
   ```
   验证一条命令:`lldb --batch -o "break set -f main.c -l 2" -o run -- ./app`,
   打出 `Breakpoint 1: where = ...` 才算 adapter 能解析。
2. **`:dapui-*` 没反应** / 报 `面板 N 状态丢失`:插件没加载成功,先 `:plugin-reload dapui`
   看状态栏;装的是旧二进制时 `dapui` 会因 `helix.on('dap-stopped')` 未知而整插件加载失败
   (见 `plugin-reload` 提示),重新 `cargo build --release --bin hx` 安装即可。
