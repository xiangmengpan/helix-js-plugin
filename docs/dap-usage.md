# DAP 调试使用说明

> 面向**使用者**：怎么开始调、怎么调得顺、出了问题怎么查。
> 参考手册在 `docs/api/dap.md`；实现/设计记录在 `docs/superpowers/specs/2026-10-2{6,7}-dap-*.md`。

---

## 0. 一分钟起步（C 为例）

```bash
hx main.c
```

1. 光标移到想停的那行，按 **`<space>G b`** 打断点（行首出现 `◯`）
2. **`:debug-start`** 回车 —— 自动探测构建方式 → 构建（Output 里流式打印）→ 启动 → 命中
3. 命中后**直接在编辑器里按**（不用进任何"模式"）：

```
n 单步跳过   s 单步进入   o 单步跳出   c 继续   p 暂停   T 结束
```

4. 想改代码？直接 `i` 进插入，改完按 **`r`** 重启（**会先重建**，跑的是新二进制）

---

## 1. 界面长什么样

```
┌──────────────────────────── 编辑器 ────────────────────────────┬──── 右侧：变量树 ────┐
│   3    int s = a + b;                                          │ #1 ⏸ t1  │ a=2 b=3  │
│▶  4    return s;        ← 光标自动跟到停止行                    │ ▾ add  hello.c:4    │
│   5  }                                                        │   ▾ Locals          │
│                                                                │      a = 2 :int     │
├────────────────────────────────────────────────────────────────┤      b = 3 :int     │
│ REPL  ▸Output  Breakpoints   ← 底部三页                        │   ▸ main  hello.c:7 │
│ > █                                                            │   ▸ … 3 个系统帧     │
└────────────────────────────────────────────────────────────────┴─────────────────────┘
 状态栏左侧：`[DEBUG]` ← 输入层徽标        状态栏右侧：断点/位置/编码…
```

- **命中时**：光标跳到停止行并居中、状态栏写 `add — Thread 1 hello.c:4`、树头一行 Locals 摘要、
  提示行弹出 `Debug ▸ r 重启 · T 结束会话 · c 继续 · p 暂停 · n 单步跳过 · s 单步进入 …`
- **右栏永远是"编辑区右边的全高列"，底部永远"横跨整宽"**——无论你关掉再开时焦点在哪
  （早先会跟着焦点在当前活动叶上分裂：焦点在底部时开右栏，右栏只有半屏高）
- **面板只在有会话时出现**；会话结束（不管是程序自己退出还是你按 `T`）都会收起，
  **异常退出会保留 Output**（崩溃信息在那儿）
- 会话**期间打开的源文件**（单步跳进去的、`> 系统帧` 里的）在会话结束后会**关掉**
  —— 只关"本次会话新开、且你没改过"的；你自己原先开着的文件不动
- `:debug-log` 复用**同一个**临时 buffer（敲几次都只有一个）

---

## 2. 键位

### 2.1 暂停时（编辑器里直接按，不用进模式）

| 键 | 动作 | 键 | 动作 |
|---|---|---|---|
| `n` | 单步跳过 | `s` | 单步进入 |
| `o` | 单步跳出 | `c` | 继续 |
| `p` | 暂停 | `T` | 结束会话 |
| `r` | 重启（**先重建**） | `b` | 切换断点 |
| `t` | 切线程 | `f` | 切栈帧 |
| `S` | 切会话 | `v` | 变量**弹层**（浮层列表：`名字 / 值 / 类型` 三列、回车复制值、可 `/` 过滤；寄存器不列）|
| `R` | 跑到光标 | `K` | 光标下标识符求值 |
| `C-c` / `C-l` | 条件断点 / 日志断点 | `e` / `E` | 启用 / 禁用异常断点 |
| `?` | 键位速查（**开一个窗口**显示完整表，`:bc` 关掉） | | |

> 普通/平级模式下想看完整键位：**`:keys`**（也可 `<space>K`）。注意 `<space>?` 是**命令面板**，不是键位表。

> **编辑照常**：`i` 是插入、`d` 是删除。这些调试键**只在暂停时**接管，运行中完全不抢。
> 想改键：`[keys.debug]`（见 §6.2）。

### 2.2 快速入口：`F9` 断点 · `<space>G` 菜单

| 想做的事 | 最短路径 |
|---|---|
| 打断点/取消 | **`F9`**（或 `[keys.normal]` 里自绑 `"A-b" = ":debug-toggle-breakpoint"`） |
| 启动 | **`:debug-start`**（无参数 ⇒ 配置 picker） |
| 其它调试动作 | 暂停时直接按（§2.1）；**没暂停**时才用 `<space>G …` 菜单 |

`<space>G` 是**一次性**菜单：按完一个动作立刻回到普通模式，**不会**卡住、
也不会吞掉 `j/k` 之类的移动键。它比暂停接管表多一个 **`l` = 启动**（暂停时没有"启动"可做）。

### 2.3 面板里（焦点在树/底部时）

| 键 | 动作 |
|---|---|
| `j k h l g G` | 树内上下移动 / 展开 / 折叠 / 首尾 |
| `Enter` | 展开节点；选中别的会话 = **切过去** |
| `w` / `W` | 加监视（单行有选中文本则预填）/ 清空监视 |
| `d` | 删掉当前这条监视 |
| `e` | 给当前变量**设值**（`setVariable`） |
| `y` | 复制值；在**栈帧**节点上复制 `file:line` |

面板**有焦点**时底部会多一行键位提示（没焦点就不占那一行）。

输入类操作（`w` 监视、`/` 搜索、`e` 设值、断点页的 `c`/`l`）与多选（`e` 异常过滤器）都是
**圆角浮层**：边框 + 标题 + 输入行 + 底部提示，跟 `picker-ui` 的弹窗同一套观感。
`Esc` 取消 / `Enter` 确认；输入框超宽时保留**尾部**（光标永远在最后）。
| `/` | **搜索**变量（名字或值）：命中递归保留祖先、光标**跳到第一个命中**、`n`/`N` 跳下/上一个；空串清除 |
| `q` | 关面板（REPL 页 `q` 是普通字符） |
| `Esc` | 焦点回编辑器（面板保留） |

**其余键全部冒泡**给 §2.1 那张表 —— 面板不是第二套键位：`t` 到哪都是"切线程"。

### 2.4 回面板（`Esc` 出来之后）

| 想去 | 命令 / 键 |
|---|---|
| 右侧变量树 | `:dapui-focus tree` 或 **`A-1`** |
| 底部面板 | `:dapui-focus bottom` 或 **`A-2`** |
| 底部指定页 | `:dapui-repl` / `:dapui-output` / `:dapui-breakpoints` |
| 两个都关 | `:dapui-close`；树开/关 = `:dapui` 或 `A-d` |

---

## 3. 常见任务怎么做

### 3.1 "我只想跑到那一行看看"

光标放目标行 → **`R`**（跑到光标）。实现是打一个临时断点再 `c`，命中后自动摘掉，不脏你的断点表。

### 3.2 "这个变量现在是多少？"

| 想做的事 | 用什么 |
|---|---|
| 光标下那个标识符 | **`K`**（结果进状态栏） |
| 看一片局部变量 | 右侧树（命中自动展开 Locals）；树头也有 `a=2 b=3 s=5` 摘要 |
| 一直盯着几个表达式 | **`w`** 加监视（**会持久化**，重开 hx 还在） |
| 算个复杂表达式 / 调个函数 | 底部 **REPL**（`:dapui-repl`）：`Tab` 补全、`↑/↓` 历史、多行、`C-l` 清屏 |
| 改一个值再继续 | 树里选中变量按 **`e`** |

### 3.3 条件断点 / 日志断点 / 异常断点

| 类型 | 怎么做 |
|---|---|
| 条件断点 | 光标行 `C-c` → 输入 `i > 100`（Breakpoints 页也有 `c`） |
| 日志断点（不暂停，只打印） | `C-l` → 输入 `i={i}` 之类 |
| 异常断点 | `e` 打开过滤器勾选（`E` 全关）；`◯` 说明未验证，后面会带 adapter 给的原因 |
| 断点是否生效 | 行号左边的 **`●`** = adapter 已验证；**`◯`** = 还没验证（未启动会话 / 模块没加载）|
| 断点页长什么样 | `● multi/main.c:9`（路径取末两段）右跟 `条件: v>0` / `日志: i={i}`；未验证的后面带 adapter 的原因 |

### 3.4 多线程 / 多协程

`t` 切线程（弹层里 `j/k` 选、`Enter` 确定），`f` 切栈帧；树里只列**当前线程**的帧，切线程后自动刷新。

### 3.5 改代码后再调

改完按 **`r`** —— 会**先重建**（`:debug-start` 时记住了构建命令），Output 里能看到构建流，
成功后自动启动新会话。想只构建不启动：**`:debug-build`**（`--release` 可加参数）。

### 3.6 同时调两个进程

`:debug-start` 两次即可。树里每个会话一个节点：

```
▾ #2 ⏸ t123 (当前)     ← 展开的
▸ #1 ⏸ t456 = lldb-dap  ← 折叠的，Enter 切过去
```

`S` 也能切（弹层选择）。关掉某一页用 `[plugins.dapui] show_all_sessions = false` 回到"只画当前会话"。

### 3.7 程序崩了 / 输出在哪

- **每条会话的 Output 是独立的**：新会话一建就把上一轮的输出清掉（不会看到上次残留）
- 正常调试期：底部 **Output** 页（构建输出是**流式**的，第一行就自动切过去；
  多行输出按行拆开、CRLF 归一，最后半行会等下一块拼上或会话结束时刷出）
- 程序**异常退出**：面板会保留 Output 并报 `进程退出码 N`
- 想看调试器**协议层**在干什么：**`:debug-log`**（最近 500 条收发，`:debug-log 50` 只看 50 条，`:debug-log clear` 清空）

### 3.8 调试器本身出问题

```
:debug-log        ← 看 DAP 收发（不用 -vv、不用翻 helix.log）
:debug-sessions   ← 有几条会话、各自状态
~/.cache/helix/helix.log   ← 最后手段
```

---

## 4. 语言准备

| 语言 | 需要做什么 | 说明 |
|---|---|---|
| **C / C++** | 装 `lldb-dap`（或 Xcode 自带） | 零配置：自动 `cc -g -O0` / `make` / `cmake --build` |
| **Rust** | 同上 | 自动 `cargo build`，产物名从 `Cargo.toml` 推 |
| **Go** | `go` + `dlv` | 自动 `go build` |
| **Python** | `pip install debugpy` | 零配置：`python3 -m debugpy.adapter`，`:debug-start` 直接跑当前文件 |
| **JS / TS** | `js-debug`（VS Code 的 debug 扩展或 git clone） | 会在 `~/.vscode{,-server,-insiders}/extensions/ms-vscode.js-debug-*/src/dapDebugServer.js`、`~/vscode-js-debug/…` 里**自动探测**；找不到就报"缺 command" |

### 4.1 自定义启动配置（`.helix/launch.toml`）

```toml
[[config]]
name     = "run"
language = "rust"                       # 或者 adapter = "lldb-dap"
request  = "launch"                     # launch | attach
build    = "cargo build"                # 启动前构建（空/省略 = 不构建）；支持 ${program}
cwd      = "${workspaceFolder}"
[config.args]
program  = "${workspaceFolder}/target/debug/${cargoBin}"
args     = ["--verbose"]
env      = { RUST_BACKTRACE = "1" }
```

可用变量：`${workspaceFolder}` `${file}` `${fileBasename}` `${fileDirname}` `${cargoBin}`
`${cargoProfile}`，以及 **`build` 里**可用的 `${program}`。

`.helix/launch.json`（VS Code 格式，含注释）也支持；两级合并，后者优先。

### 4.2 远程 attach

```vim
:debug-remote 127.0.0.1:4711 gdbserver     " 地址 + 模板名
```
模板来自 `.helix/launch.toml` 的 `request = "attach"` 条目或 `languages.toml`。

---

## 5. 能力边界（提前知道，少踩）

| 能做 | 不能做（adapter 不支持） |
|---|---|
| 单步/继续/暂停/重启、条件与日志断点、异常断点 | **反向调试**（`supportsStepBack=false`） |
| 多线程、多会话、栈帧切换 | **内存视图/反汇编**（`memoryReferences=false`） |
| 变量树（分页、设值）、监视、表达式求值 | 跨会话的断点宏/脚本 |
| 程序输出、构建输出、协议流水 | 图形化时间线/火焰图 |

---

## 6. 配置

### 6.1 `[plugins.dapui]`

| 键 | 默认 | 作用 |
|---|---|---|
| `auto_open` | `true` | 命中时自动开右侧树 |
| `auto_open_bottom` | `"off"` | 同时开底部：`off`/`repl`/`output`/`breakpoints` |
| `focus_on_auto_open` | `false` | 自动开面板时抢焦点（默认不抢，你好继续按调试键） |
| `panel_size` | `40` | 树宽（列） |
| `labels` | `"en"` | 分组名语言：`en`（Watches/Locals/Globals…）或 `"zh-CN"`（监视/局部/全局…） |
| `locals_summary` | `false` | 树顶那行 `a=2 b=3` 摘要（展开 Locals 就有同样信息） |
| `persist_watches` | **`false`** | 默认**不跨重启**（每次开 hx 都是干净的）。开成 `true` 才按工作区落盘 `dapui-watches-<工作区 slug>` |
| `just_my_code` | `false` | 单步时**跳过不在工作区**的源码（std/core/系统库）：落到工作区外就自动 `stepOut`，最多 12 跳；断点/异常停在库里不受影响 |
| `frame_name` | `"short"` | 栈帧名显示：`short`=main/len · `keeper`=Point::len · `full`=rsdbg::add |
| `align_columns` | `false` | 变量名补空格对齐（`a    = 2`）；默认写成一格 `a = 2` |
| `columns_header` | `false` | 变量列表上方画列头（`name = value type`）；多占一行，小屏可不开 |
| `max_frames` | `20` | 树里最多列几个"有源码"的帧 |
| `max_depth` | `2` | 停止时自动展开几层 |
| `max_nodes` | `2000` | 树节点上限 |
| `show_all_sessions` | `true` | 多会话并排 |
| `breakpoint_marks` | `true` | 断点行首 `●`/`◯` |
| `highlight_current_line` | `true` | 高亮当前执行行 |
| `current_line_scope` | `"dap.current-line"` | 当前行用哪个主题 scope |
| `readonly_on_stop` | `"off"` | 命中时强制只读：`off`/`doc`/`all` |
| `perspective` | `false` | 命中时借走布局、继续/结束还原 |
| `inline_values` | `false` | （实验）Locals 写成行首虚拟文本，会右移代码 |

### 6.2 提示不要占屏幕（which-key）

```toml
[plugins.which-key]
mode = "status"          # box=完整浮层 / line=1 行浮层 / status=只写状态栏（不占格子）
min_entries = 2          # 当前层子键少于这个数就不弹（消掉 j/k 那种单键层的噪音）
sort_by_frecency = true  # 按最近/最常按重排候选；截断时先丢不常用的
keys_help_command = ":keys"  # 提示里「还有 N 个（? 看全部）」的实际入口；空串 = 去掉这句
on_debug_hint = "off"    # 调试命中时那行提示：off 不弹 / status 只写状态栏 / full 按 mode 画
max_lines = 10
```

- **每次命中都会推一次 `Debug` 键位表** —— `box` 模式下就是每次停下弹一个框。
  嫌挡视线就 `on_debug_hint = "off"`（想看键位随时按 `?`）或 `"status"`（只在状态栏一行）。
- 前缀菜单（`space`/`g`/`m`…）走 `mode`；`status` 模式下也只在状态栏写一行摘要。
- 层徽标（` DEBUG`）由 `statusline` 插件的 `layers` 段显示；dapui 的 `dap` 段只报会话
  （`#1 ⏸ t1`），不会重复。

### 6.3 自定义调试键：`[keys.debug]`

```toml
[keys.debug]
"A-1" = "@:dapui-focus tree<ret>"     # 暂停时一键进变量树
"A-2" = "@:dapui-focus bottom<ret>"   # 暂停时一键进底部面板
"s"   = "dap_next"                    # 改内置键（例：s 改成跳过）
"i"   = "dap_step_in"                 # 加新键（代价：暂停时 i 不能进插入）
```

- 语法与 `[keys.normal]` 完全一致；在**内置表之上合并**（同名替换、新键追加）
- 只影响"暂停时的接管"；不暂停时 `[keys.normal]` 那套照旧
- which-key 的 `Debug ▸ …` 提示**自动跟随**
- `:config-reload` 即刻生效

### 6.4 只读

```vim
:read-only        " 当前 buffer 切换强制只读（i/a/d/粘贴/插件编辑/LSP 改写全部被拒）
:read-only all    " 所有打开的 buffer
```
调试期自动只读：`[plugins.dapui] readonly_on_stop = "doc" | "all"`（默认 `off`，gdb 式不锁文件）。

---

## 7. 排错

| 症状 | 原因 / 处理 |
|---|---|
| 按 `n` 没反应 | 会话没在**暂停**（运行中不接管键）；或焦点在命令行/插入模式 |
| 右侧栏选中行看不出选的是哪一行 | 选中行最前面有 **`›`** 记号；面板**没焦点**时不铺底色。**全栏不再有其它底色**（早先 `ui.text.focus` 带 bg，帧/作用域每行一条色带）|
| Rust 栈帧名是 `_RNv…` 这种符号 | 已修：核心用 `rustc-demangle` 解 v0/legacy（并去掉 lldb 的 `[inlined]`、` at x.rs:12` 尾巴和 `@@GLIBC` 版本号）→ 显示 `main` / `add`；`frame_name` 可切 `keeper`（`Point::len`）/`full`（`rsdbg::add`）|
| 栈帧里一堆 `call_once` / `lang_start` | 已修：源码**不在当前工作区**的帧（std/core 的库源码）折叠进 `… +N system frames` |
| 变量名和 `=` 之间空格太多 | 缩进每级 1 格、名字列只按**变量名**算（帧不参与）、上限 14 ⇒ `a   = 2  int` |
| 监视表达式显示成红色 `?` | 求值失败（换了程序 / 变量不在当前帧）：右侧暗色文字是原因；`d` 删掉、`W` 清空 |
| `Globals`/`Registers` 展开是空的 | 会标 `(empty)`；adapter 不报数量时它仍会列出来（无法事先判断） |
| `<space>G` 之后按 `j` 不动 | 已修：菜单改成**不 sticky**（按完一个动作就退出，不会一直吞键）|
| 配置里写 `":cmd<ret>"` 启动就报 `Bad config` | 核心命令**不带** `<ret>`（`<ret>` 只在 `@...` 按键序列里用）：`"A-b" = ":debug-toggle-breakpoint"`。报错信息现在会直接提示这一点 |
| 暂停时 `i` 是插入，不是单步 | 设计如此：单步进入是 **`s`**（gdb 约定）。想改：`[keys.debug]` 里写 `"i" = "dap_step_in"` |
| 断点打上了但不生效 | 行首是 `◯`（未验证）：① 会话没启动 ② 那个模块没加载（`dlopen` 后才验证）③ 代码改了没重建 → 按 `r` |
| 单步跳的行号不对 | 跑的是**旧二进制**（DWARF 行号表过期）→ `r`（会重建）；或编译没开 `-g` |
| `:debug-start` 报 `Command not provided` | 该语言模板没有 adapter 命令（node 没探测到 js-debug）→ 按提示在 `languages.toml` 配 `command/args` |
| `构建完成但产物不存在` | 没探测到 `program` → `:debug-start <模板> <可执行文件>`，或在 `launch.toml` 写 `program` |
| 面板没出现 | `auto_open` 关了；或还没命中（`:dapui` 手动开） |
| 结束调试后面板/buffer 一堆没关 | 已修：`T` 现在与"程序正常退出"走同一条收尾（收面板、清只读/视角、关掉本次会话新开的未修改文件）。手动开的（`A-1`/`A-2`）仍保留 |
| 敲了几次 `:debug-log` 就多了几个无名 buffer | 已修：复用同一个临时 buffer（`?` 的速查页同理）|
| 断点行首出现**两个**标记（行号前后各一个）| 已修：核心 gutter 画 `●/◯`，dapui 的 `breakpoint_marks` 默认改成 **关** |
| `<space>G l` 的配置列表没对齐 | 已修：改用 picker 的**真列**（配置/请求/来源/语言），不再手写 `{:<12}` 拼一行 |
| 树里一半是 `__libc_start_main` 之类 | 「… N 个系统帧（无源码）」是折叠组，展开才看（正常） |
| Output 里行粘在一起 / 看着乱 | 已修：按 `\n` 拆行 + `\r\n` 归一（adapter 常把多行塞进一个 `output` 事件）|
| REPL 里打不出 `1`/`2`/`3` | 已修：REPL 页里数字是**输入**；切页用 `Tab`（输入为空时）或 `:dapui-*` |
| 状态栏那个 `⏸`/`▶` 图标不搭 | 已修：改用 nerd-font（pause/play/clock/stop），层徽标改成 `\uf188 DEBUG` 暗色（不再是 `[DEBUG]` 胶囊）|
| 变量只显示 200 个 | 分页：下面一行 `… 更多（start=200，按 l 加载）`，按 `l` 继续 |
| `… （adapter 不支持分页…）` | 该 adapter 忽略 `start`，只能取到这一页 |
| 状态栏消息一闪就没了 | 面板/插件也会写状态栏；关键信息看 `:debug-log` 或 Output |
| 配置里写错 `[keys.xxx]` | 只会**警告并忽略那一段**（其余配置照常） |

---

## 8. 一页纸速查

```
起步        F9 打断点（或 <space>G b）· :debug-start（无参=选配置）
暂停时      n 跳过 · s 进入 · o 跳出 · c 继续 · p 暂停 · T 结束 · r 重启(重建)
            b 断点 · C-c 条件 · C-l 日志 · t 线程 · f 帧 · S 会话
            R 跑到光标 · K 光标求值 · ? 速查
树/面板     jk hl gG 导航 · Enter(切会话/展开) · w 监视 · W 清空 · e 设值 · y 复制 · / 过滤
            q 关 · Esc 回编辑器
回面板      A-1 树 · A-2 底部 · :dapui-focus tree|bottom
底部页      REPL(补全/历史/多行/C-l) · Output(流式构建/程序输出) · Breakpoints(d/c/l/e/Enter)
诊断        :debug-log [N|clear] · :debug-sessions
只读        :read-only [all] · [plugins.dapui] readonly_on_stop = "off"|"doc"|"all"
改键        [keys.debug]（合并到内置表；提示自动跟随）
```
