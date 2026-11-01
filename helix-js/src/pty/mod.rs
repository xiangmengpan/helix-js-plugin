//! 伪终端（PTY）的**平台门面**。
//!
//! - **Unix**：POSIX 实现（`posix_openpt` / `grantpt` / ioctl），见 [`unix`]；
//! - **Windows**：尚未实现 —— 需要 **ConPTY**（Win10 1809+）。在那之前**只返回明确错误**，
//!   不给"能编译但跑不对"的实现（见 `windows.rs` 的说明）。
//!
//! ## 为什么把 resize 做成 trait
//!
//! `state.rs` 的 `TERM_MASTERS` 是一张「进程 id → 谁能改窗口尺寸」的注册表。它此前直接存
//! `std::os::fd::RawFd`（Unix 才有的类型），于是**注册表本身**在 Windows 上不成立。
//! 平台差异应当落在**实现**，不该落在注册表 —— 所以这里只暴露
//! [`PtyResize`] 这个与平台无关的能力，具体是裸 fd 还是 `HPCON` 由各平台自己决定。
//!
//! 见 `docs/superpowers/specs/2026-10-30-cross-platform-design.md` §4。

/// pty 窗口尺寸（单元格）。
///
/// 内核新建的 pty 是 **0×0**（`stty size` 会读成 `0 0`），必须在子进程启动前设好；
/// 之后由 `term_resize` 按 UI 尺寸再调。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TermSize {
    pub rows: u16,
    pub cols: u16,
}

/// 「能改这个 pty 的窗口尺寸」的句柄 —— 注册表里只存它。
pub(crate) trait PtyResize: Send + Sync {
    fn resize(&self, size: TermSize) -> anyhow::Result<()>;
}

/// 本构建是否支持 pty。
///
/// 调用方用它给出**人话错误**（而不是让用户看到"unknown id"这种内部术语，也不是假装可用）。
pub(crate) fn is_supported() -> bool {
    cfg!(unix)
}

/// 宿主端句柄：`spawn_pty_worker` 只通过它读写/改尺寸/设前台组，不碰平台类型。
///
/// 这是 ConPTY 落地时的**唯一接缝**：新增一个 Windows 实现即可，worker 一行不用改。
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) trait PtyMaster: Send + 'static {
    /// 读端独占一份（Unix 是 dup 出的第二个 File）。
    fn reader(&self) -> anyhow::Result<Box<dyn std::io::Read + Send>>;
    fn write_all(&mut self, buf: &[u8]) -> anyhow::Result<()>;
    fn flush(&mut self) -> anyhow::Result<()>;
    /// 注册进 `TERM_MASTERS` 的 resize 句柄（`term_resize` 走它）。
    fn resize_handle(&self) -> std::sync::Arc<dyn PtyResize>;
    /// 把子进程设为该 pty 的**前台进程组**（交互 shell 的 job control 需要）；
    /// Windows 无此概念 ⇒ 空实现。
    fn set_foreground(&self, child_pid: u32);
}

/// 打开一对 pty：宿主端句柄 + **子进程侧的 slave `File`**。
///
/// ⚠ 刻意**不**在这里把 slave 拆成三份 `Stdio`：上一版那样做之后 Linux 的 pty 用例
/// （`pty_spawn`/`pty_resize`）拿不到真 tty（`tty` 输出 8 字节后不再产出、等不到 Exit）。
/// 拆分的时机/位置显然有影响，所以这一步只换"宿主端句柄"这一个变量，slave 仍原样交给 worker。
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn open(size: TermSize) -> anyhow::Result<(Box<dyn PtyMaster>, std::fs::File)> {
    #[cfg(unix)]
    {
        unix::open(size)
    }
    #[cfg(not(unix))]
    {
        let _ = size;
        Err(windows::not_supported())
    }
}

#[cfg(unix)]
mod unix;

#[cfg(not(unix))]
mod windows;
// 非 unix 侧暂**没有**调用点（`is_supported()` 会先拦下来，见 popup.rs 的错误分支）；
// 这里保留再导出，是为了 ConPTY 落地时调用点不必改，只改 windows.rs。
#[cfg(not(unix))]
#[allow(unused_imports)]
pub(crate) use windows::open_pty;
