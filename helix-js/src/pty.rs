use std::fs::File;
use std::os::fd::{FromRawFd, RawFd};

use anyhow::{anyhow, Result};

/// master fd 包装：Drop 时 close（worker 线程持有时保证 fd 生命周期）
pub(crate) struct Master(RawFd);

impl Master {
pub(crate) fn fd(&self) -> RawFd {
        self.0
    }

    /// 把裸 fd 交给 File，不再 Drop close（防双关）
pub(crate) fn into_file(self) -> File {
        let fd = self.0;
        std::mem::forget(self);
        unsafe { File::from_raw_fd(fd) }
    }
}

impl Drop for Master {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

/// 打开新 PTY（默认 24×80），返回 (master, slave File)
pub(crate) fn open_pty() -> Result<(Master, File)> {
    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if master < 0 {
            return Err(anyhow!("posix_openpt: {}", std::io::Error::last_os_error()));
        }
        let master = Master(master); // 尽早包进 RAII 包装，后续 `?` 错误路径不再泄漏 fd
        if libc::grantpt(master.fd()) != 0 {
            return Err(anyhow!("grantpt: {}", std::io::Error::last_os_error()));
        }
        if libc::unlockpt(master.fd()) != 0 {
            return Err(anyhow!("unlockpt: {}", std::io::Error::last_os_error()));
        }
        let name = libc::ptsname(master.fd());
        if name.is_null() {
            return Err(anyhow!("ptsname: {}", std::io::Error::last_os_error()));
        }
        let name = std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned();
        let slave = libc::open(
            std::ffi::CString::new(name)?.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        );
        if slave < 0 {
            return Err(anyhow!("open slave: {}", std::io::Error::last_os_error()));
        }
        let slave = File::from_raw_fd(slave);
        // 默认尺寸：内核新建 pty 的 winsize 是 0×0，stty size 会读成 "0 0"；
        // 简报期望缺省 "24 80"。在子进程启动前设好（master ioctl 作用于同一 tty）。
        set_winsize(master.fd(), 24, 80)?; // 失败时 master/slave 由 Drop 兜底关闭
        // raw mode：关 canonical 缓冲/回显/信号生成——输入即达子进程（bash/readline
        // 自己处理回显与 Ctrl-C），消除"输入缓冲、字母成批"的观感。
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(master.fd(), &mut termios) == 0 {
            libc::cfmakeraw(&mut termios);
            libc::tcsetattr(master.fd(), libc::TCSANOW, &termios);
        }
        Ok((master, slave))
    }
}

/// TIOCSWINSZ 设置 pty 窗口尺寸（master/slave fd 均可，作用于同一 tty 设备）。
/// 在 spawn 后立即调用也能在子进程启动前生效——winsize 是 tty 设备属性，
/// 不依赖子进程是否存在，因此无消息时序竞态。
pub(crate) fn set_winsize(fd: RawFd, rows: u16, cols: u16) -> Result<()> {
    unsafe {
        let ws = libc::winsize {
            ws_row: rows as libc::c_ushort,
            ws_col: cols as libc::c_ushort,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        if libc::ioctl(fd, libc::TIOCSWINSZ, &ws) != 0 {
            return Err(anyhow!("TIOCSWINSZ: {}", std::io::Error::last_os_error()));
        }
    }
    Ok(())
}
