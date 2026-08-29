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
        let name = std::ffi::CStr::from_ptr(name)
            .to_string_lossy()
            .into_owned();
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
                                           // termios 保持内核默认（ICANON|ECHO|ISIG|OPOST|ONLCR），与真实终端模拟器一致：
                                           // 不设 raw——raw 关掉内核 ECHO 后，cat 等依赖内核回显的程序输入无显示；
                                           // bash/readline 会自己切 raw 并回显，主控端无需代劳。
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn pty_output_nl_maps_to_crlf() {
        // termios 保持内核默认（含 OPOST|ONLCR）：子进程输出里的 \n 翻译成 \r\n，
        // 否则终端网格的 linefeed 只下移不归列 → ls 等输出逐行递增缩进错位。
        let (master, _slave) = open_pty().unwrap();
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::tcgetattr(master.fd(), &mut termios) }, 0);
        assert_eq!(
            termios.c_oflag & (libc::OPOST | libc::ONLCR),
            libc::OPOST | libc::ONLCR,
            "PTY 输出必须保持 OPOST|ONLCR（\\n → \\r\\n）"
        );
    }

    #[test]
    fn pty_default_termios_has_echo_and_canonical() {
        // 保持内核默认 termios：cat 等依赖内核 ECHO 回显的程序输入可见；
        // 若设 raw（关 ECHO/ICANON），终端里输入字符无显示。
        let (master, _slave) = open_pty().unwrap();
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::tcgetattr(master.fd(), &mut termios) }, 0);
        assert_ne!(
            termios.c_lflag & libc::ECHO,
            0,
            "内核回显必须开启（cat 等程序输入可见）"
        );
        assert_ne!(
            termios.c_lflag & libc::ICANON,
            0,
            "canonical 行缓冲应开启（程序按需自行关闭）"
        );
    }
}
