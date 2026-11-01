//! Windows 侧 pty：**ConPTY**（Win10 1809+）。
//!
//! ConPTY 与 Unix pty 不是同一套语义，几处差异都是**行为级**的，不是换个 API 那么简单：
//!
//! | | Unix pty | ConPTY |
//! |---|---|---|
//! | 宿主端句柄 | 裸 fd（`/dev/ptmx`） | `HPCON`（`CreatePseudoConsole`） |
//! | 改尺寸 | `ioctl(TIOCSWINSZ)` | `ResizePseudoConsole` |
//! | 数据形态 | 内核 line discipline 加工的字节 | conhost 里那个仿真器吐出的 **VT 序列** |
//! | 子进程接入 | slave 当 stdin/stdout/stderr | **属性表** `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` |
//! | 关闭 | `close(fd)` 即可 | **必须最后调** `ClosePseudoConsole`，否则读端永不 EOF |
//! | 杀进程树 | `kill(-pgid)` | Job Object（`TerminateProcess` 只杀一个进程） |
//!
//! ## 为什么子进程要用 `CreateProcessW` 而不是 `std::process::Command`
//!
//! 伪终端要靠属性表塞进 `STARTUPINFOEXW`，`Command` 不暴露属性表 —— 所以这里自己拼
//! 命令行（含 Windows 引用规则）、环境块，再 `CreateProcessW`。这也是 [`PtyChild`] 这层
//! trait 存在的原因：拿回来的不是 `std::process::Child`。

use std::fs::File;
use std::io::Write;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};
use windows_sys::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, ResumeThread, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
    EXTENDED_STARTUPINFO_PRESENT, INFINITE, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTUPINFOEXW,
};

use super::{PtyChild, PtyChildStdio, PtyMaster, PtyResize, TermSize};

/// ConPTY 会话句柄的可见部分：`spawn_child` 要把它塞进属性表。
///
/// `HPCON` 在 windows-sys 里是 `isize`（不透明句柄），所以自动就是 `Send`。
pub(crate) struct ConPty {
    pub(crate) hpc: HPCON,
}

/// 宿主端：ConPTY 会话 + 两条管道。
pub(crate) struct Master {
    hpc: HPCON,
    /// 写它 = 往终端喂输入（conhost 从另一头读）
    input: Option<OwnedHandle>,
    /// 读它 = 终端输出（conhost 往另一头写）
    output: Option<OwnedHandle>,
    /// `ClosePseudoConsole` 只能调一次
    closed: bool,
}

impl Master {
    /// 关会话（幂等）。管道先放，再关 —— 反过来的话 `ClosePseudoConsole` 会等 conhost，
    /// 而 conhost 可能正阻塞在写一条没人读的管道上。
    fn close_console(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.input.take();
        self.output.take();
        unsafe { ClosePseudoConsole(self.hpc) };
    }
}

impl PtyMaster for Master {
    fn reader(&self) -> Result<Box<dyn std::io::Read + Send>> {
        let handle = self
            .output
            .as_ref()
            .ok_or_else(|| anyhow!("pty: ConPTY 已关闭"))?;
        // 复制一个句柄给读线程：读线程与写侧（`write_all`）可以同时用
        Ok(Box::new(File::from(handle.try_clone()?)))
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        let handle = self
            .input
            .as_ref()
            .ok_or_else(|| anyhow!("pty: ConPTY 已关闭"))?;
        // ManuallyDrop：句柄所有权仍归 `self.input`（Drop 时由 OwnedHandle 关闭）
        let mut file =
            std::mem::ManuallyDrop::new(unsafe { File::from_raw_handle(handle.as_raw_handle()) });
        file.write_all(buf)?;
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        let handle = self
            .input
            .as_ref()
            .ok_or_else(|| anyhow!("pty: ConPTY 已关闭"))?;
        let mut file =
            std::mem::ManuallyDrop::new(unsafe { File::from_raw_handle(handle.as_raw_handle()) });
        file.flush()?;
        Ok(())
    }

    fn resize_handle(&self) -> Arc<dyn PtyResize> {
        Arc::new(WinResize { hpc: self.hpc })
    }

    fn set_foreground(&self, _child_pid: u32) {
        // Windows 没有"前台进程组"概念（job control 由 conhost 管），空实现。
    }

    fn close(&mut self) {
        // ⚠ 这一步是 ConPTY 与 Unix 的**行为差异**，不是清理细节：
        // conhost 不会因为客户端进程退出而关闭输出管道（实测：子进程退完 20s 仍无 EOF）。
        // 只有 `ClosePseudoConsole` 才会让 conhost 退出 ⇒ 读线程拿到 EOF ⇒ worker 可收尾。
        self.close_console();
    }
}

impl Drop for Master {
    fn drop(&mut self) {
        // 顺序有讲究：**先**放掉本进程持有的两条管道，**再**关会话。
        // `ClosePseudoConsole` 会等 conhost 退出；此时若还握着管道，可能出现
        // "conhost 阻塞在写一条没人读的管道 / 等我们关闭" 的互等。
        self.close_console();
    }
}

/// `TERM_MASTERS` 里存的 resize 句柄。
struct WinResize {
    hpc: HPCON,
}

impl PtyResize for WinResize {
    fn resize(&self, size: TermSize) -> Result<()> {
        let coord = coord(size);
        let hr = unsafe { ResizePseudoConsole(self.hpc, coord) };
        if hr < 0 {
            return Err(anyhow!("ResizePseudoConsole: HRESULT 0x{:08x}", hr as u32));
        }
        Ok(())
    }
}

/// `TermSize`（行列）→ `COORD`（X=列，Y=行）。注意顺序是**反**的。
fn coord(size: TermSize) -> COORD {
    COORD {
        X: size.cols as i16,
        Y: size.rows as i16,
    }
}

/// 建一条匿名管道，返回 (读端, 写端)，两者都**不可继承**。
fn pipe() -> Result<(OwnedHandle, OwnedHandle)> {
    let mut read: HANDLE = std::ptr::null_mut();
    let mut write: HANDLE = std::ptr::null_mut();
    if unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) } == 0 {
        return Err(anyhow!("CreatePipe: {}", std::io::Error::last_os_error()));
    }
    // 显式去掉继承位：pty 的宿主端管道不许被子进程继承（ConPTY 在**本进程**里用它们）。
    // CreatePipe 传 NULL 属性本来就不继承，这里写死是为了不依赖那条隐含规则。
    unsafe {
        SetHandleInformation(read, HANDLE_FLAG_INHERIT, 0);
        SetHandleInformation(write, HANDLE_FLAG_INHERIT, 0);
    }
    Ok((
        unsafe { OwnedHandle::from_raw_handle(read as RawHandle) },
        unsafe { OwnedHandle::from_raw_handle(write as RawHandle) },
    ))
}

/// 平台入口：建 ConPTY 会话，返回 (宿主端, 子进程侧要的东西)。
pub(crate) fn open(size: TermSize) -> Result<(Box<dyn PtyMaster>, PtyChildStdio)> {
    // 两条管道的方向（很容易记反，按 conhost 的视角写）：
    //   input  : conhost 从 read 端**读**我们的按键 → 我们写 write 端
    //   output : conhost 往 write 端**写**屏幕 → 我们从 read 端读
    let (input_read, input_write) = pipe()?;
    let (output_read, output_write) = pipe()?;

    let mut hpc: HPCON = 0;
    let hr = unsafe {
        CreatePseudoConsole(
            coord(size),
            input_read.as_raw_handle() as HANDLE,
            output_write.as_raw_handle() as HANDLE,
            0,
            &mut hpc,
        )
    };
    if hr < 0 {
        return Err(anyhow!(
            "CreatePseudoConsole: HRESULT 0x{:08x}（ConPTY 需要 Win10 1809+）",
            hr as u32
        ));
    }
    // 这两端已经交给 ConPTY 了：本进程必须立刻放掉，否则输出管道永远不会 EOF。
    drop(input_read);
    drop(output_write);

    Ok((
        Box::new(Master {
            hpc,
            input: Some(input_write),
            output: Some(output_read),
            closed: false,
        }),
        PtyChildStdio::ConPty(ConPty { hpc }),
    ))
}

// ─────────────────────────── 子进程 ───────────────────────────

/// `CreateProcessW` 起出来的子进程：进程句柄 + 任务对象（杀整棵树用）。
pub(crate) struct WinChild {
    process: OwnedHandle,
    /// `KILL_ON_JOB_CLOSE`：本进程异常退出时，任务对象一关就把子进程树一起带走（不留孤儿）。
    job: Option<OwnedHandle>,
    id: u32,
}

impl PtyChild for WinChild {
    fn id(&self) -> u32 {
        self.id
    }

    fn kill_tree(&mut self) {
        // 整棵树：`cmd /C ...` 会再起子进程，只杀一个会留孤儿（与 Unix 的 kill(-pgid) 对齐）
        if let Some(job) = &self.job {
            if unsafe { TerminateJobObject(job.as_raw_handle() as HANDLE, 1) } != 0 {
                return;
            }
        }
        unsafe { TerminateProcess(self.process.as_raw_handle() as HANDLE, 1) };
    }

    fn exited(&mut self) -> bool {
        unsafe { WaitForSingleObject(self.process.as_raw_handle() as HANDLE, 0) == 0 }
    }

    fn wait(&mut self) -> Option<i32> {
        unsafe {
            WaitForSingleObject(self.process.as_raw_handle() as HANDLE, INFINITE);
            let mut code: u32 = 0;
            if GetExitCodeProcess(self.process.as_raw_handle() as HANDLE, &mut code) == 0 {
                return None;
            }
            Some(code as i32)
        }
    }
}

/// 起子进程并接上 ConPTY。
///
/// 与 Unix 的区别是**没有 stdio 重定向**：`PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` 直接把会话
/// 塞给子进程，它的 stdin/stdout/stderr 由 conhost 接。
pub(crate) fn spawn_child(
    cmd: &crate::platform::Cmd,
    stdio: PtyChildStdio,
) -> Result<Box<dyn PtyChild>> {
    let hpc = match stdio {
        PtyChildStdio::ConPty(con) => con.hpc,
        #[allow(unreachable_patterns)]
        other => {
            let _ = other;
            return Err(anyhow!("pty: Windows 侧需要 ConPTY 句柄"));
        }
    };

    let command = cmd.to_command()?;
    let mut command_line = command_line(&command)?;
    let environment = environment_block(&command);
    let cwd = command.get_current_dir().map(wide_z).transpose()?;

    // ── 属性表：把 HPCON 交给 CreateProcessW ──
    let mut attr_size: usize = 0;
    unsafe {
        // 第一次必然失败，只为拿所需大小（文档规定的两步走）
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size);
    }
    // `usize` 对齐：属性表要求指针按 `usize` 对齐，`Vec<u8>` 不保证 ⇒ 用 `Vec<usize>` 承载
    let mut attr_buf = vec![0usize; attr_size.div_ceil(std::mem::size_of::<usize>())];
    let attr_list = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    unsafe {
        if InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_size) == 0 {
            return Err(anyhow!(
                "InitializeProcThreadAttributeList: {}",
                std::io::Error::last_os_error()
            ));
        }
        // lpvalue 要的是「HPCON 变量的地址」，不是 HPCON 的值本身
        if UpdateProcThreadAttribute(
            attr_list,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            &hpc as *const HPCON as *const std::ffi::c_void,
            std::mem::size_of::<HPCON>(),
            std::ptr::null_mut(),
            std::ptr::null(),
        ) == 0
        {
            DeleteProcThreadAttributeList(attr_list);
            return Err(anyhow!(
                "UpdateProcThreadAttribute(PSEUDOCONSOLE): {}",
                std::io::Error::last_os_error()
            ));
        }
    }

    // CREATE_SUSPENDED：先挂住，把任务对象关联好再让它跑 ——
    // 否则子进程可能在 `AssignProcessToJobObject` 之前就生了孙进程，杀树时漏掉。
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attr_list;

    let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        CreateProcessW(
            std::ptr::null(), // 程序名交给命令行第一段（我们已按规则引用）
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0, // bInheritHandles = FALSE：pty 管道在属性表里，不走继承
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
            environment.as_ptr() as *const std::ffi::c_void,
            cwd.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
            &startup.StartupInfo,
            &mut process_info,
        )
    };
    unsafe { DeleteProcThreadAttributeList(attr_list) };
    if created == 0 {
        return Err(anyhow!(
            "CreateProcessW: {}（命令行：{}）",
            std::io::Error::last_os_error(),
            String::from_utf16_lossy(&command_line)
        ));
    }

    // 进程/线程句柄改成 RAII 包装，后面任何 `?` 都不会泄漏
    let process = unsafe { OwnedHandle::from_raw_handle(process_info.hProcess as RawHandle) };
    let thread = unsafe { OwnedHandle::from_raw_handle(process_info.hThread as RawHandle) };

    // ── 任务对象：杀整棵树 ──
    // 失败不算致命（退化成只杀一个进程），但要能从 debug 日志看出来
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    let job = if job.is_null() {
        None
    } else {
        let job = unsafe { OwnedHandle::from_raw_handle(job as RawHandle) };
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            SetInformationJobObject(
                job.as_raw_handle() as HANDLE,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            AssignProcessToJobObject(
                job.as_raw_handle() as HANDLE,
                process.as_raw_handle() as HANDLE,
            );
        }
        Some(job)
    };

    unsafe { ResumeThread(thread.as_raw_handle() as HANDLE) };
    drop(thread);

    Ok(Box::new(WinChild {
        process,
        job,
        id: process_info.dwProcessId,
    }))
}

/// 从 `std::process::Command` 拼出 `CreateProcessW` 要的 UTF-16 命令行。
///
/// 引用规则按 MSVC CRT 的 `argv` 解析（`\"` 转义、反斜杠成对加倍）。以 `"` 开头时
/// 有特殊语义，所以不能简单包一层引号。
fn command_line(command: &std::process::Command) -> Result<Vec<u16>> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut line = quote_arg(&program);
    for arg in command.get_args() {
        line.push(' ');
        line.push_str(&quote_arg(&arg.to_string_lossy()));
    }
    Ok(line.encode_utf16().chain(std::iter::once(0)).collect())
}

/// 按 Windows 的规则给一个参数加引号（不含引号字符时原样返回）。
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        if ch == '\\' {
            backslashes += 1;
            continue;
        }
        // 反斜杠在引号前要加倍（原文里的反斜杠必须原样到达，不能当转义符吃掉）
        out.extend(std::iter::repeat_n('\\', backslashes));
        backslashes = 0;
        if ch == '"' {
            out.push('\\');
        }
        out.push(ch);
    }
    // 结尾的反斜杠也要加倍，否则会把收尾的引号转义掉
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// 环境块：`CreateProcessW` 要 `KEY=VALUE\0...\0\0`，且必须按名排序（不区分大小写）。
///
/// 基于本进程环境 + 命令自己的 env 增删 —— 效果与 `Command::env`/`env_remove` 一致。
fn environment_block(command: &std::process::Command) -> Vec<u16> {
    use std::collections::BTreeMap;
    // 键统一大写做排序键（Windows 环境变量不区分大小写），值保留原样
    let mut map: BTreeMap<String, (String, String)> = std::env::vars()
        .map(|(k, v)| (k.to_lowercase(), (k, v)))
        .collect();
    for (key, value) in command.get_envs() {
        let key = key.to_string_lossy().into_owned();
        match value {
            Some(v) => {
                map.insert(key.to_lowercase(), (key, v.to_string_lossy().into_owned()));
            }
            None => {
                map.remove(&key.to_lowercase());
            }
        }
    }
    let mut block: Vec<u16> = Vec::new();
    for (_, (key, value)) in map {
        block.extend(format!("{key}={value}").encode_utf16());
        block.push(0);
    }
    block.push(0); // 双 NUL 结尾
    if block.len() == 1 {
        block.push(0); // 空环境也要有结尾的第二个 NUL
    }
    block
}

/// 带结尾 NUL 的 UTF-16 字符串。
fn wide_z(s: &std::path::Path) -> Result<Vec<u16>> {
    let s = s.to_string_lossy();
    if s.contains('\0') {
        return Err(anyhow!("路径里含 NUL 字符"));
    }
    Ok(s.encode_utf16().chain(std::iter::once(0)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::time::Duration;

    /// 引用规则是「手写命令行」最容易错的地方：这几条与 CRT 的行为一一对应。
    #[test]
    fn quoting_matches_crt_rules() {
        assert_eq!(quote_arg("a"), "a");
        assert_eq!(quote_arg("a b"), "\"a b\"");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg("a\"b"), "\"a\\\"b\"");
        // 结尾反斜杠在引号前必须加倍，否则会把收尾引号转义掉
        assert_eq!(quote_arg("a b\\"), "\"a b\\\\\"");
        assert_eq!(quote_arg("a b\\\\"), "\"a b\\\\\\\\\"");
        // 不在引号里的反斜杠原样保留
        assert_eq!(quote_arg("C:\\x"), "C:\\x");
    }
    /// ConPTY 端到端：输出经 pty 回来，退出码拿得到，**`close()` 之后读端能 EOF**。
    ///
    /// 这条测试盯的是最脏的那个行为点：conhost **不会**随客户端进程退出而关闭输出管道
    /// （首版就是在这里红的：子进程退完 20s 仍无 EOF）。所以协议是
    /// 「发现子进程退了 → `PtyMaster::close()` → 读端 EOF → `wait()`」，worker 照此执行；
    /// 这里把同一套顺序手写一遍，把这个假定钉死在测试里。
    ///
    /// **为什么 `#[ignore]`**：本仓的 Windows 验证跑在 WSL2 的 interop 里，而 ConPTY 在那里
    /// 起不来 —— 子进程一律以 `0xC0000142`（`STATUS_DLL_INIT_FAILED`）退出、pty 里零字节。
    /// 这不是本实现的锅：**按 MS 示例逐行写的 C 程序同样如此**（`CreatePseudoConsole` 成功、
    /// conhost 也起来了、`ResizePseudoConsole` 也成功，就是子进程初始化失败）。换 cmd.exe /
    /// whoami.exe、显式 cwd、`bInheritHandles=TRUE`、`PSEUDOCONSOLE_INHERIT_CURSOR` 全部复现。
    /// 所以在 **Windows 实机**上跑：
    ///   cargo test -p helix-js --lib pty::windows -- --ignored --nocapture
    #[ignore = "ConPTY 在 WSL2 interop 下不可用（子进程 0xC0000142，MS 示例 C 程序同样复现）⇒ 需 Windows 实机"]
    #[test]
    fn conpty_echo_roundtrip() {
        use std::sync::{Arc, Mutex};
        use std::time::Instant;

        let (mut master, stdio) =
            open(TermSize { rows: 24, cols: 80 }).expect("CreatePseudoConsole");
        let mut child = spawn_child(
            &crate::platform::Cmd::Shell("echo conpty-hello".into()),
            stdio,
        )
        .expect("CreateProcessW");
        let mut reader = master.reader().expect("reader");

        // 读线程只负责把字节攒起来：**不靠 EOF 才知道「收到了什么」**
        let got = Arc::new(Mutex::new(Vec::<u8>::new()));
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        {
            let got = got.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.lock().unwrap().extend_from_slice(&buf[..n]),
                    }
                }
                let _ = done_tx.send(());
            });
        }

        // ① 输出内容：`echo` 的回显必须经 pty 回来
        let want = "conpty-hello";
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline
            && !String::from_utf8_lossy(&got.lock().unwrap()).contains(want)
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        let text = String::from_utf8_lossy(&got.lock().unwrap()).into_owned();
        assert!(text.contains(want), "pty 输出里没有回显: {text:?}");

        // ② 进程退出（worker 主循环就是靠这个发现“该收尾了”）
        let deadline = Instant::now() + Duration::from_secs(15);
        while !child.exited() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(child.exited(), "子进程 15s 内没退出");

        // ③ 关会话 ⇒ 读端必须 EOF（不关的话这里会超时，正是首版的红灯）
        master.close();
        done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("close() 之后读端仍无 EOF ⇒ worker 会挂死");
        assert_eq!(child.wait(), Some(0), "退出码应是 0");
    }
}
