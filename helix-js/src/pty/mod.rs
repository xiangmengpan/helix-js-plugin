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

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::{open_pty, Master};

#[cfg(not(unix))]
mod windows;
// 非 unix 侧暂**没有**调用点（`is_supported()` 会先拦下来，见 popup.rs 的错误分支）；
// 这里保留再导出，是为了 ConPTY 落地时调用点不必改，只改 windows.rs。
#[cfg(not(unix))]
#[allow(unused_imports)]
pub(crate) use windows::open_pty;
