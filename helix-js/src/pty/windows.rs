//! Windows 侧 pty：**尚未实现**，只给出明确错误。
//!
//! 为什么不是"先糊一个能编译的实现"：ConPTY 与 Unix pty 不是同一套语义 ——
//!
//! | | Unix pty | ConPTY |
//! |---|---|---|
//! | 宿主端句柄 | 裸 fd（`/dev/ptmx`） | `HPCON`（`CreatePseudoConsole`） |
//! | 改尺寸 | `ioctl(TIOCSWINSZ)` | `ResizePseudoConsole` |
//! | 数据形态 | 内核 line discipline 加工的字节 | conhost 里那个仿真器吐出的 **VT 序列** |
//! | 输入侧 | 原始按键字节 | 必须编码成 VT（鼠标上报等） |
//! | 关闭 | `close(fd)` 即可 | **必须最后调** `ClosePseudoConsole`，否则读端永不 EOF |
//! | 杀进程树 | `kill(-pgid)` | 需要 Job Object |
//!
//! 这些都要在 **Windows 实机**上验（EOF 时序、进程树回收），否则就是给用户一个
//! 「能开面板、但一按 Ctrl-C 就挂住」的东西 —— 比明确报错糟得多。
//!
//! 落地时的入口已经在位：本文件实现 `open_pty()`，`is_supported()` 返回 true，
//! 再把 `shell.rs` 里 `#[cfg(unix)]` 的 `spawn_pty_worker` 改成接 trait 对象即可。
#![allow(dead_code)] // 非 unix 侧暂无调用点（`is_supported()` 先拦），保留形状给 ConPTY 实现

use anyhow::{anyhow, Result};

/// 本平台还开不了 pty。
pub(crate) fn open_pty() -> Result<()> {
    Err(anyhow!(
        "pty: 本平台尚未实现（Windows 需要 ConPTY，Win10 1809+）；\
         同步命令请用 helix.run([...])"
    ))
}
