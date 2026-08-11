# 设计：PTY 终端（v11-②）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

`helix.spawn({ pty: true, ... })` 让子进程跑在真实 PTY 下——交互式/ curses 程序（vim、htop、REPL）可用。终端 UI 复用既有弹窗终端插件（terminal.js 加 `pty: true`）。

## 变更

- `helix.spawn` 增加可选 `pty: boolean`；`pty: true` 时子进程 stdin/stdout/stderr 接 PTY slave，`TERM=xterm-256color` 环境
- 新增 `helix.term_resize(id, rows, cols)`——TIOCSWINSZ 更新 PTY 窗口尺寸（默认 24×80）
- `term_write`/`term_kill` 语义不变（写 PTY master / kill）

## 实现（零新依赖——裸 libc openpty）

- **helix-js/Cargo.toml**：加 `libc = { workspace = true }`
- **helix-js**（新模块或 lib.rs 内）：PTY FFI——
  - `openpty()`：`libc::posix_openpt(O_RDWR|O_NOCTTY)` → `grantpt`/`unlockpt` → `ptsname` → 打开 slave fd
  - spawn 时（worker 内）：master/slave fd 传给子进程（slave 作为 stdin/stdout/stderr，dup2），父线程持有 master——读 master 当输出（替代 stdout/stderr 管道），写 master 当 stdin（替代 stdin 管道）
  - `term_resize`：`libc::ioctl(master, TIOCSWINSZ, winsize{rows, cols})`
  - 关闭顺序：子进程退出/EOF 后关 master fd
  - ponytail：Unix-only（与 sh 一致）；无 raw mode 设置（终端 UI 层不设——子进程内 curses 自己管）
- **worker 结构**：pty 模式的 worker 不再用 stdout/stderr 双读线程，改单 master 读线程 + master 写（TermCtrl::Write → write(master)）

## 测试

- 单测：spawn `{pty: true, cmd: "tty"}` → 输出含 `/dev/pts/`（isatty 证明）；`stty size` → "24 80"；term_resize(40, 100) 后 `stty size` → "100 40"（行列对应以实测为准）
- 集成：可省（单测覆盖 FFI；UI 链路与既有 terminal 弹窗相同）——或最小冒烟

## 涉及文件

- `helix-js/Cargo.toml`（libc）
- `helix-js/src/lib.rs`（pty FFI + spawn pty 模式 + term_resize + 单测）
- 演示：`~/.config/helix/plugins/terminal.js` 加 `pty: true`（控制器更新）
