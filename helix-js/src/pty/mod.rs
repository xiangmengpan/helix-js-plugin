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
///
/// Windows 侧的 ConPTY 实现已经写完了（见 `pty::windows::open` / `pty::windows::spawn_child`；
/// 这两个符号只在 `cfg(not(unix))` 下存在 ⇒ 这里**不能**写成 intra-doc 链接：Linux 上跑
/// `cargo doc -D warnings` 会因"unresolved link"直接红），
/// 但**还没在任何一台 Windows 上验过** —— 开发机（Windows 11 26200）的 ConPTY 链路本身是坏的：
/// 本实现、MS 示例式 C 程序、`portable-pty` 0.9 三个独立实现全都起不来子进程（`0xC0000142`
/// 或永远不退出、pty 零字节），且绕开 WSL interop（计划任务）后依旧复现 ⇒ 与代码无关。按本仓的规矩：
/// **没验过的能力不报 true**（宁可给明确错误，也不给一个开了就空、还杀不掉的面板）。
/// 在实机上跑通 `pty::windows::conpty_echo_roundtrip` 后，把下面这行改成 `cfg!(any(unix, windows))`
/// 即可打开，其余代码一行不用动。
pub(crate) fn is_supported() -> bool {
    cfg!(unix)
}

/// 子进程侧接 pty 所需的东西：Unix 拿 slave `File`，Windows 拿 ConPTY 会话句柄。
///
/// ⚠ Unix 的**转换必须发生在 worker 线程里**（`into_stdio()` 被 `spawn_pty_worker` 调用，而不是在
/// `open()` 里）。上一版把三份 dup 提前到 caller 就导致 Linux 的 pty 拿不到真 tty
/// （`pty_spawn`/`pty_resize` 挂）—— 这个时机是**行为的一部分**，不是实现细节。
///
/// Windows 侧**根本没有**这一步：ConPTY 不走 stdio 重定向，子进程是靠属性表接上会话的。
pub(crate) enum PtyChildStdio {
    /// Unix：pty 的 slave 端（三份 dup 由 `into_stdio` 在 worker 里做）
    #[cfg(unix)]
    Slave(std::fs::File),
    /// Windows：ConPTY 会话句柄（`spawn_child` 拿它填 `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`）
    #[cfg(windows)]
    ConPty(windows::ConPty),
}

impl PtyChildStdio {
    /// 在 **worker 线程**里把 slave 转成子进程的三份 stdio。
    ///
    /// 仅 Unix：ConPTY 不用 stdio 重定向。
    #[cfg(unix)]
    pub(crate) fn into_stdio(
        self,
    ) -> anyhow::Result<(
        std::process::Stdio,
        std::process::Stdio,
        std::process::Stdio,
    )> {
        match self {
            PtyChildStdio::Slave(slave) => {
                let stdin = slave.try_clone()?;
                let stdout = slave.try_clone()?;
                Ok((
                    std::process::Stdio::from(stdin),
                    std::process::Stdio::from(stdout),
                    std::process::Stdio::from(slave),
                ))
            }
        }
    }
}

/// 子进程句柄 —— worker 只通过这 4 个动作使用子进程。
///
/// ## 为什么需要这层 trait
///
/// Unix 侧是 `std::process::Child`，但 **ConPTY 必须自己调 `CreateProcessW`**
/// （`std::process::Command` 带不上 `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`），
/// 拿到的是 `PROCESS_INFORMATION` 里的 `HANDLE`，**根本不是** `std::process::Child`。
/// worker 的其余部分（读线程 / 主循环 / Exit 事件）因此只依赖下面这几个动作。
pub(crate) trait PtyChild: Send {
    /// 进程 id（Unix 用作 pgid 去杀整组；Windows 是任务管理器里的那个 id）。
    fn id(&self) -> u32;
    /// 杀**整棵进程树**。
    ///
    /// Unix：`kill(-pgid, SIGKILL)` —— sh 未 exec 时孙进程（如 `cat`）也一并杀，
    /// 否则孤儿进程持住 slave fd → master 读永不 EIO → worker 挂死。
    /// Windows：Job Object 整棵树终止（`TerminateProcess` 只杀一个进程）。
    fn kill_tree(&mut self);
    /// 不阻塞地问一句：进程退了吗？
    ///
    /// **为什么 worker 必须轮询这个**：ConPTY 下 conhost 不会随客户端退出而关闭输出管道
    /// （实测：`cmd /C echo` 退完后 20s 依然无 EOF）⇒ 必须由 worker 发现“子进程没了”并
    /// 主动 [`PtyMaster::close`]，读端才会 EOF。Unix 侧它是无害的（并顺手把退出码缓存下来）。
    fn exited(&mut self) -> bool;
    /// 等退出码（读端 EOF 之后调用，不会挂很久）；拿不到返回 `None`（worker 记 -1）。
    /// 若 [`Self::exited`] 已把它收割掉，这里返回缓存值。
    fn wait(&mut self) -> Option<i32>;
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
    /// 子进程已退出 ⇒ 主动收尾，让读端能 EOF。
    ///
    /// - **Windows（ConPTY）**：`ClosePseudoConsole` 才会让 conhost 退出、输出管道关闭。
    ///   不调它，读线程就永远阻塞在 `ReadFile` 上（实测过，不是理论）。
    /// - **Unix**：空操作 —— slave 全关后 master 自然 EIO，提前关反而会让读端的 dup 读不到尾巴。
    ///
    /// 幂等：重复调用安全。
    fn close(&mut self);
}

/// 打开一对 pty：宿主端句柄 + **子进程侧接 pty 所需的东西**。
///
/// ⚠ Unix 上刻意**不**在这里把 slave 拆成三份 `Stdio`：上一版那样做之后 Linux 的 pty 用例
/// （`pty_spawn`/`pty_resize`）拿不到真 tty（`tty` 输出 8 字节后不再产出、等不到 Exit）。
/// 拆分的时机/位置显然有影响，所以这一步只换"宿主端句柄"这一个变量，slave 仍原样交给 worker。
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn open(size: TermSize) -> anyhow::Result<(Box<dyn PtyMaster>, PtyChildStdio)> {
    #[cfg(unix)]
    {
        unix::open(size)
    }
    #[cfg(not(unix))]
    {
        windows::open(size)
    }
}

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::UnixChild;

#[cfg(not(unix))]
mod windows;
#[cfg(not(unix))]
pub(crate) use windows::spawn_child as spawn_windows_child;
