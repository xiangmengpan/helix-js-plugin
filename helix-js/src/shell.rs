
use anyhow::{anyhow, Result};
use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsString, JsValue};

use crate::pty;

use crate::state::{
    with_async_callbacks, with_terms,
    WakeSender,
};

/// worker 读块大小
const TERM_CHUNK_SIZE: usize = 4096;


use crate::types::*;

fn read_stream<R: std::io::Read>(
    mut stream: R,
    id: u64,
    aggregate: bool,
    output: &std::sync::Mutex<Vec<u8>>,
    tx: &WakeSender<TermEvent>,
) {
    let mut buf = [0u8; TERM_CHUNK_SIZE];
    let mut tail: Vec<u8> = Vec::with_capacity(3); // 跨块的不完整 UTF-8 尾部（最长序列 3 字节）
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if aggregate {
            output.lock().unwrap().extend_from_slice(&buf[..n]);
            continue;
        }
        tail.extend_from_slice(&buf[..n]);
        match std::str::from_utf8(&tail) {
            Ok(s) => {
                if tx.send(TermEvent::Chunk(id, s.to_string())).is_err() {
                    return;
                }
                tail.clear();
            }
            Err(e) => {
                let valid = e.valid_up_to();
                if e.error_len().is_some() {
                    // 硬性非法字节（非不完整尾部）：整段 lossy 输出，不留尾部
                    let chunk = String::from_utf8_lossy(&tail).into_owned();
                    if tx.send(TermEvent::Chunk(id, chunk)).is_err() {
                        return;
                    }
                    tail.clear();
                } else if valid > 0 {
                    // 尾部是不完整序列：发出有效前缀，残留字节留到下一块
                    let chunk = String::from_utf8_lossy(&tail[..valid]).into_owned();
                    if tx.send(TermEvent::Chunk(id, chunk)).is_err() {
                        return;
                    }
                    tail.drain(..valid);
                }
            }
        }
    }
    if !aggregate && !tail.is_empty() {
        // EOF：残留的不完整尾部 lossy 输出（无后续字节可拼）
        let chunk = String::from_utf8_lossy(&tail).into_owned();
        let _ = tx.send(TermEvent::Chunk(id, chunk));
    }
}

/// 启动 worker 线程：sh -c 跑子进程，读线程逐块发 Chunk（或聚合进 stdout）；
/// 主线程轮询控制通道（写 stdin / kill）与读线程完成信号，全部读完才收尾发 Exit。
/// 控制通道由 term_write/term_kill 发消息；无人发 Kill 且子进程不退出时 worker 一直存活（终端会话语义）。
fn spawn_worker(
    id: u64,
    cmd: &str,
    aggregate: bool,
    tx: WakeSender<TermEvent>,
    stdin_rx: std::sync::mpsc::Receiver<TermCtrl>,
) {
    let cmd = cmd.to_string();
    std::thread::spawn(move || {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                let _ = tx.send(TermEvent::Exit(id, 127, None));
                return;
            }
        };
        let stream_out = child.stdout.take();
        let stream_err = child.stderr.take();
        let mut stdin = child.stdin.take();

        // 读线程：stdout/stderr 各一个，逐块发 Chunk（非聚合）或拼进共享缓冲（聚合）；完成后发 done 信号
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        // 聚合缓冲存原始字节，EOF 后统一 lossy 解码：跨块多字节字符不损坏
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let mut readers = Vec::new();
        {
            let tx = tx.clone();
            let done_tx = done_tx.clone();
            let output = output.clone();
            readers.push(std::thread::spawn(move || {
                read_stream(stream_out.unwrap(), id, aggregate, &output, &tx);
                let _ = done_tx.send(());
            }));
        }
        {
            let tx = tx.clone();
            let done_tx = done_tx.clone();
            let output = output.clone();
            readers.push(std::thread::spawn(move || {
                read_stream(stream_err.unwrap(), id, aggregate, &output, &tx);
                let _ = done_tx.send(());
            }));
        }
        drop(done_tx);

        // 主循环：处理控制消息；两个读线程都 EOF 后收尾
        loop {
            while let Ok(msg) = stdin_rx.try_recv() {
                match msg {
                    TermCtrl::Write(text) => {
                        if let Some(s) = stdin.as_mut() {
                            let _ = s.write_all(text.as_bytes());
                            let _ = s.flush();
                        }
                    }
                    TermCtrl::Kill => {
                        // pipe worker 未设 process_group：杀直接子进程（sh 单命令会 exec，等于杀目标）
                        let _ = child.kill();
                    }
                }
            }
            let mut finished = 0;
            while done_rx.try_recv().is_ok() {
                finished += 1;
            }
            if finished >= readers.len() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        drop(stdin); // 关 stdin → 仍等输入的子进程读到 EOF 后退出
        for h in readers {
            let _ = h.join();
        }
        let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        let bytes = std::mem::take(&mut *output.lock().unwrap());
        let stdout = if aggregate {
            // 全部原始字节统一 lossy 解码（跨块字符在整流上解码，不产生 U+FFFD）
            Some(String::from_utf8_lossy(&bytes).into_owned())
        } else {
            None
        };
        let _ = tx.send(TermEvent::Exit(id, code, stdout));
    });
}

/// PTY worker：子进程 stdin/stdout/stderr 接 slave；父线程读 master 发 Chunk（流式），
/// TermCtrl::Write 写 master 当 stdin，Kill 杀子进程。单读线程（pty 无 stderr 区分）。
/// worker 持 master File（Drop 关闭）；slave 经 Stdio::from 交给子进程，spawn 后父侧关闭。
#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub(crate) fn spawn_pty_worker(
    id: u64,
    cmd: &str,
    tx: WakeSender<TermEvent>,
    ctrl_rx: std::sync::mpsc::Receiver<TermCtrl>,
    master: pty::Master,
    slave: std::fs::File,
) {
    let cmd = cmd.to_string();
    std::thread::spawn(move || {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut master = master.into_file();
        // 子进程 stdin/stdout/stderr 都是 slave（dup 三份，spawn 后父侧副本关闭）
        let stdin_slave = match slave.try_clone() {
            Ok(f) => f,
            Err(_) => {
                let _ = tx.send(TermEvent::Exit(id, 127, None));
                return;
            }
        };
        let stdout_slave = match slave.try_clone() {
            Ok(f) => f,
            Err(_) => {
                let _ = tx.send(TermEvent::Exit(id, 127, None));
                return;
            }
        };
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(stdin_slave))
            .stdout(Stdio::from(stdout_slave))
            .stderr(Stdio::from(slave))
            .process_group(0) // 子进程自成进程组：kill 杀整组（sh 未 exec 时的孙进程也杀）
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                let _ = tx.send(TermEvent::Exit(id, 127, None));
                return;
            }
        };
        // 子进程进程组设为 pty 前台组（交互 bash 的 job control 正常；tcsetpgrp 失败静默）
        #[cfg(unix)]
        unsafe {
            libc::tcsetpgrp(std::os::unix::io::AsRawFd::as_raw_fd(&master), child.id() as libc::pid_t);
        }
        // master 读端单独 dup（读写两端并发：写线程主循环 + 读线程）
        let master_reader = match master.try_clone() {
            Ok(f) => f,
            Err(_) => {
                let _ = child.kill();
                let _ = tx.send(TermEvent::Exit(id, 127, None));
                return;
            }
        };

        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        // pty 无聚合模式：output 缓冲不会被 read_stream 使用，占位传引用
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        {
            let tx = tx.clone();
            let done_tx = done_tx.clone();
            let output = output.clone();
            std::thread::spawn(move || {
                read_stream(master_reader, id, false, &output, &tx);
                let _ = done_tx.send(());
            });
        }
        drop(done_tx);

        // 主循环：写 master / kill；读线程 EOF（子进程退出关闭 slave → master 读 EIO）后收尾
        loop {
            while let Ok(msg) = ctrl_rx.try_recv() {
                match msg {
                    TermCtrl::Write(text) => {
                        let _ = master.write_all(text.as_bytes());
                        let _ = master.flush();
                    }
                    TermCtrl::Kill => {
                        #[cfg(unix)]
                        unsafe {
                            // 杀整个进程组（-pgid）：sh 未 exec 时孙进程（如 cat）也一并杀，
                            // 否则孤儿进程持住 slave fd → master 读永不 EIO → worker 挂死
                            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
                        }
                        #[cfg(not(unix))]
                        let _ = child.kill();
                    }
                }
            }
            if done_rx.try_recv().is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        let _ = tx.send(TermEvent::Exit(id, code, None));
        // master File drop → 关闭 fd（TERM_MASTERS 里的裸 fd 变陈旧，resize 报 EBADF，良性）
    });
}

pub(crate) fn js_run_async(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let cmd: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let cb = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if cb.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.run_async: callback must be a function",
        ))));
    }
    let id = crate::state::next_term_id();
    with_terms(|m| {
        m.insert(id, TermCallbacks { on_chunk: cb.clone(), on_exit: Some(cb), is_run_async: true })
    });
    let (tx, rx) = std::sync::mpsc::channel();
    crate::state::with_term_workers(|m| m.insert(id, tx.clone()));
    spawn_worker(
        id,
        &cmd,
        true,
        crate::state::with_term_events(|t| t.clone().expect("TERM_EVENTS initialized")),
        rx,
    );
    Ok(JsValue::from(id))
}

pub(crate) fn js_spawn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let opts = args.first().unwrap_or(&JsValue::undefined()).as_object().ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.spawn: options object required")))
    })?;
    let cmd: String = opts.get(JsString::from("cmd"), ctx)?.try_js_into(ctx)?;
    let on_chunk = opts.get(JsString::from("onChunk"), ctx)?;
    if on_chunk.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.spawn: onChunk must be a function",
        ))));
    }
    let on_exit = opts.get(JsString::from("onExit"), ctx)?;
    let on_exit = on_exit.as_callable().map(|_| on_exit);
    // pty: bool，缺省 false（管道模式）。非布尔 → 报错
    let pty = {
        let v = opts.get(JsString::from("pty"), ctx)?;
        if v.is_null_or_undefined() {
            false
        } else {
            v.try_js_into::<bool>(ctx).map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from(
                    "helix.spawn: 'pty' must be a boolean",
                )))
            })?
        }
    };
    let id = crate::state::next_term_id();
    with_terms(|m| {
        m.insert(id, TermCallbacks { on_chunk, on_exit, is_run_async: false })
    });
    let (tx, rx) = std::sync::mpsc::channel();
    crate::state::with_term_workers(|m| m.insert(id, tx));
    let term_tx = crate::state::with_term_events(|t| t.clone().expect("TERM_EVENTS initialized"));
    if pty {
        #[cfg(unix)]
        {
            let (master, slave) = pty::open_pty().map_err(|e| {
                JsError::from_opaque(JsValue::from(JsString::from(format!(
                    "helix.spawn: pty: {e}"
                ))))
            })?;
            // 先注册 master fd 再起 worker：spawn 返回后 JS 立即可 term_resize
            crate::state::with_term_masters(|m| m.insert(id, master.fd()));
            spawn_pty_worker(id, &cmd, term_tx, rx, master, slave);
        }
        #[cfg(not(unix))]
        {
            return Err(JsError::from_opaque(JsValue::from(JsString::from(
                "helix.spawn: pty requires a unix platform",
            ))));
        }
    } else {
        spawn_worker(id, &cmd, false, term_tx, rx);
    }
    Ok(JsValue::from(id))
}

pub(crate) fn js_term_write(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    let text: String = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    term_write(id, &text)
        .map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))?;
    Ok(JsValue::undefined())
}

/// 向进程 stdin 写数据（Rust 侧入口，PluginTerminal 按键直通用；js_term_write 转发到这里）。
/// 不调 init()：可能从渲染/事件循环（CONTEXT 未借用）触发，但保持与 term_kill 同款约束。
pub fn term_write(id: u64, text: &str) -> Result<()> {
    let sender = crate::state::with_term_workers(|m| m.get(&id).cloned()).ok_or_else(|| anyhow!("term_write: unknown id"))?;
    sender
        .send(TermCtrl::Write(text.to_string()))
        .map_err(|_| anyhow!("term_write: worker gone"))?;
    Ok(())
}

pub(crate) fn js_term_kill(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context)?;
    term_kill(id).map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))?;
    Ok(JsValue::undefined())
}

/// 杀进程（Rust 侧入口，helix-term 关闭终端面板时用；js_term_kill 转发到这里）。
/// 未知 id / worker 已退出 → Err。Kill 后 worker 的 Exit 事件照常发（回调幂等）。
/// 不调 init()：可能从命令执行（CONTEXT 已借用）里触发，且 TERM_WORKERS 是普通 thread_local。
pub fn term_kill(id: u64) -> Result<()> {
    crate::state::unregister_term(id);
    let sender = crate::state::with_term_workers(|m| m.remove(&id)).ok_or_else(|| anyhow!("term_kill: unknown id"))?;
    sender.send(TermCtrl::Kill).map_err(|_| anyhow!("term_kill: worker gone"))?;
    Ok(())
}

/// 调整 PTY 窗口尺寸（rows/cols）。直连 master fd 做 TIOCSWINSZ：
/// winsize 是 tty 设备属性，spawn 后立即调用也在子进程启动前生效，无消息时序竞态。
/// 非 pty worker / 未知 id → Err。
#[cfg(unix)]
pub fn term_resize(id: u64, rows: u16, cols: u16) -> Result<()> {
    crate::init();
    let fd = crate::state::with_term_masters(|m| m.get(&id).copied())
        .ok_or_else(|| anyhow!("term_resize: unknown id (not a pty worker)"))?;
    pty::set_winsize(fd, rows, cols)
}

#[cfg(unix)]
pub(crate) fn js_term_resize(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.term_resize: id must be a number")))
    })?;
    let rows: u16 = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.term_resize: rows must be a number")))
    })?;
    let cols: u16 = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.term_resize: cols must be a number")))
    })?;
    let fd = crate::state::with_term_masters(|m| m.get(&id).copied()).ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.term_resize: unknown id")))
    })?;
    pty::set_winsize(fd, rows, cols)
        .map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(format!("helix.term_resize: {e}")))))?;
    Ok(JsValue::undefined())
}

/// 基础 glob：基目录 = 模式中首个元字符（`*`/`?`/`[`）之前的字面前缀递归 walk
/// （如 `dir/**/*.js` → `dir`，裸模式 `*.js` → `.`；无元字符 → 整模式为字面路径取父目录），
/// globset 匹配完整路径字符串。literal_separator(true)：`*`/`?` 不跨目录分隔符
/// （`**` 仍跨目录，含零层，globset 语义）。
/// 前导 `./` 归一化：globset 裸模式不匹配 `./x` 候选（`*` 不跨 `/`），模式与候选统一去掉。
pub(crate) fn glob_matches(pattern: &str) -> std::result::Result<Vec<String>, String> {
    use globset::GlobBuilder;
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    let matcher = GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| format!("invalid glob '{pattern}': {e}"))?
        .compile_matcher();
    let base = match pattern.find(['*', '?', '[']) {
        Some(0) => std::path::PathBuf::from("."),
        Some(i) => std::path::PathBuf::from(&pattern[..i]),
        None => std::path::Path::new(pattern)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from(".")),
    };
    let mut out = Vec::new();
    walk_glob(&base, &matcher, &mut out).map_err(|e| format!("glob_async('{pattern}'): {e}"))?;
    out.sort();
    Ok(out)
}

/// 递归 walk 目录树，匹配完整路径字符串（含目录本身——glob 常规语义）
fn walk_glob(dir: &std::path::Path, matcher: &globset::GlobMatcher, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // 基目录为 `.` 时路径带前导 `./`，globset 裸模式（`*.js`）不匹配 → 统一去掉
        let pstr = path.to_string_lossy();
        let pstr = pstr.strip_prefix("./").unwrap_or(&pstr);
        if matcher.is_match(pstr) {
            out.push(pstr.to_owned());
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            walk_glob(&path, matcher, out)?;
        }
    }
    Ok(())
}

/// 注册异步 fs 回调并 spawn 一次性 worker：操作在线程里执行，
/// 结果（Ok 或 Err 字符串）经 ASYNC_EVENTS 通道送回发起线程。
fn spawn_async_op<T: Send + 'static>(
    id: u64,
    op: impl FnOnce() -> std::result::Result<T, String> + Send + 'static,
    mk: impl FnOnce(u64, std::result::Result<T, String>) -> AsyncEvent + Send + 'static,
) {
    let tx = crate::state::with_async_events(|t| t.clone().expect("ASYNC_EVENTS initialized"));
    std::thread::spawn(move || {
        let _ = tx.send(mk(id, op()));
    });
}

/// 校验回调参数（第二个/第三个参数必须是可调用函数）
fn async_cb(args: &[JsValue], pos: usize, api: &str) -> boa_engine::JsResult<JsValue> {
    let cb = args.get(pos).cloned().unwrap_or(JsValue::undefined());
    if cb.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
            "helix.{api}: callback must be a function"
        )))));
    }
    Ok(cb)
}

pub(crate) fn js_read_file_async(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.read_file_async: path must be a string")))
    })?;
    let cb = async_cb(args, 1, "read_file_async")?;
    let id = crate::state::next_async_id();
    with_async_callbacks(|m| { m.insert(id, cb); });
    spawn_async_op(id, move || {
        std::fs::read_to_string(&path).map_err(|e| format!("read_file_async('{path}'): {e}"))
    }, AsyncEvent::FsRead);
    Ok(JsValue::from(id))
}

pub(crate) fn js_write_file_async(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.write_file_async: path must be a string")))
    })?;
    let content: String = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.write_file_async: content must be a string")))
    })?;
    let cb = async_cb(args, 2, "write_file_async")?;
    let id = crate::state::next_async_id();
    with_async_callbacks(|m| { m.insert(id, cb); });
    spawn_async_op(id, move || {
        std::fs::write(&path, &content).map_err(|e| format!("write_file_async('{path}'): {e}"))
    }, AsyncEvent::FsWrite);
    Ok(JsValue::from(id))
}

pub(crate) fn js_stat_async(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let path: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.stat_async: path must be a string")))
    })?;
    let cb = async_cb(args, 1, "stat_async")?;
    let id = crate::state::next_async_id();
    with_async_callbacks(|m| { m.insert(id, cb); });
    spawn_async_op(id, move || {
        std::fs::metadata(&path)
            .map(|m| FsStat {
                size: m.len(),
                is_dir: m.is_dir(),
                mtime: m.modified()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).map_err(std::io::Error::other))
                    .unwrap_or(0),
            })
            .map_err(|e| format!("stat_async('{path}'): {e}"))
    }, AsyncEvent::FsStat);
    Ok(JsValue::from(id))
}

pub(crate) fn js_glob_async(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let pattern: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.glob_async: pattern must be a string")))
    })?;
    let cb = async_cb(args, 1, "glob_async")?;
    let id = crate::state::next_async_id();
    with_async_callbacks(|m| { m.insert(id, cb); });
    spawn_async_op(id, move || glob_matches(&pattern), AsyncEvent::FsGlob);
    Ok(JsValue::from(id))
}

/// 取走全部待处理进程事件（主线程轮询用）
pub fn drain_term_events() -> Vec<TermEvent> {
    crate::init();
    let mut events = Vec::new();
    crate::state::with_term_events_rx(|r| {
        if let Some(rx) = r.as_mut() {
            while let Ok(e) = rx.try_recv() {
                events.push(e);
            }
        }
    });
    events
}

/// 把一条进程事件投递到对应 id 的 JS 回调。
/// Chunk → onChunk(chunk)；Exit → run_async 调 onExit(null, stdout)、spawn 调 onExit(code)；
/// 进程结束（Exit）后清理回调与 worker 注册，防止重复回调。
pub fn resolve_term_event(id: u64, event: TermEvent) -> Result<()> {
    crate::init();
    crate::state::with_engine(|engine| {
        let callbacks = with_terms(|m| {
            m.get(&id).map(|c| TermCallbacks {
                on_chunk: c.on_chunk.clone(),
                on_exit: c.on_exit.clone(),
                is_run_async: c.is_run_async,
            })
        });
        let Some(callbacks) = callbacks else { return Ok(()) };
        let undefined = JsValue::undefined();
        match event {
            TermEvent::Chunk(_, chunk) => {
                let func = callbacks.on_chunk.as_callable().and_then(JsFunction::from_object)
                    .ok_or_else(|| anyhow!("term {id} onChunk not callable"))?;
                let _: JsValue = func.call(&undefined, &[JsValue::from(JsString::from(chunk))], engine)
                    .map_err(|e| anyhow!("term {id} onChunk failed: {e}"))?;
            }
            TermEvent::Exit(_, code, stdout) => {
                if let Some(on_exit) = callbacks.on_exit {
                    let func = on_exit.as_callable().and_then(JsFunction::from_object)
                        .ok_or_else(|| anyhow!("term {id} onExit not callable"))?;
                    let args = if callbacks.is_run_async {
                        vec![JsValue::null(), JsValue::from(JsString::from(stdout.unwrap_or_default()))]
                    } else {
                        vec![JsValue::from(code)]
                    };
                    let _: JsValue = func.call(&undefined, &args, engine)
                        .map_err(|e| anyhow!("term {id} onExit failed: {e}"))?;
                }
                with_terms(|m| m.remove(&id));
                crate::state::with_term_workers(|m| m.remove(&id));
                #[cfg(unix)]
                crate::state::with_term_masters(|m| m.remove(&id));
            }
        }
        Ok(())
    })
}

/// 取走全部待处理异步 fs 事件（主线程轮询用）
pub fn drain_async_events() -> Vec<AsyncEvent> {
    crate::init();
    let mut events = Vec::new();
    crate::state::with_async_events_rx(|r| {
        if let Some(rx) = r.as_mut() {
            while let Ok(e) = rx.try_recv() {
                events.push(e);
            }
        }
    });
    events
}

/// stat 快照 → JS 对象 { size, is_dir, mtime }
fn stat_to_js(st: &FsStat, ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    Ok(JsValue::from(
        ObjectInitializer::new(ctx)
            .property(JsString::from("size"), JsValue::from(st.size as f64), Attribute::all())
            .property(JsString::from("is_dir"), JsValue::from(st.is_dir), Attribute::all())
            .property(JsString::from("mtime"), JsValue::from(st.mtime as f64), Attribute::all())
            .build(),
    ))
}

/// 把一条异步 fs 事件投递到对应 id 的 JS 回调。
/// 回调签名：read → (err, content)；write → (err)；stat → (err, {size,is_dir,mtime})；glob → (err, paths[])。
/// err 成功为 null、失败为错误字符串。一次性语义：resolve 后从注册表移除（回调失败也移除）。
pub fn resolve_async_event(id: u64, event: AsyncEvent) -> Result<()> {
    crate::init();
    crate::state::with_engine(|engine| {
        let cb = with_async_callbacks(|m| m.remove(&id));
        let Some(cb) = cb else { return Ok(()) }; // 已 resolve / 未知 id → no-op（幂等）
        let func = cb.as_callable().and_then(JsFunction::from_object)
            .ok_or_else(|| anyhow!("async fs {id} callback not callable"))?;
        let (err, arg): (JsValue, Option<JsValue>) = match event {
            AsyncEvent::FsRead(_, Ok(content)) => (JsValue::null(), Some(JsValue::from(JsString::from(content)))),
            AsyncEvent::FsRead(_, Err(e)) => (JsValue::from(JsString::from(e)), None),
            AsyncEvent::FsWrite(_, Ok(())) => (JsValue::null(), None),
            AsyncEvent::FsWrite(_, Err(e)) => (JsValue::from(JsString::from(e)), None),
            AsyncEvent::FsStat(_, Ok(st)) => (
                JsValue::null(),
                Some(stat_to_js(&st, engine).map_err(|e| anyhow!("async fs {id} stat result failed: {e}"))?),
            ),
            AsyncEvent::FsStat(_, Err(e)) => (JsValue::from(JsString::from(e)), None),
            AsyncEvent::FsGlob(_, Ok(paths)) => {
                let arr = JsArray::new(engine);
                for p in &paths {
                    arr.push(JsValue::from(JsString::from(p.clone())), engine)
                        .map_err(|e| anyhow!("async fs {id} glob result failed: {e}"))?;
                }
                (JsValue::null(), Some(JsValue::from(arr)))
            }
            AsyncEvent::FsGlob(_, Err(e)) => (JsValue::from(JsString::from(e)), None),
        };
        let undefined = JsValue::undefined();
        let mut call_args = vec![err];
        if let Some(arg) = arg {
            call_args.push(arg);
        }
        let _: JsValue = func.call(&undefined, &call_args, engine)
            .map_err(|e| anyhow!("async fs {id} callback failed: {e}"))?;
        Ok(())
    })
}

