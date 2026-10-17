//! 版本探测与模板展开(把 registry 里的 `{version}`/`{triple}` 变成真实 URL)。
//!
//! 从 `server_manager/mod.rs` 抽出(G8b)。模块间不用 glob(`use super::*`),
//! 而是按编译器提示显式 `use super::<mod>::{…}` —— 双向 glob 会构成 glob 环,
//! 解析器会把环当成空集合。

use std::path::Path;

/// 探测超时:单次 `<bin> --version` 超过即杀(防 PATH 上的可疑工具无限挂起阻塞 UI)
pub(crate) const DETECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// 运行 `<bin> --version` 提取版本串(失败返回 None,视为未安装/不可检测)。
/// 防挂起:超时强杀子进程(曾现 PATH 工具挂起 → arsenal 打开/刷新主线程卡死);
/// 防重复 spawn:按 (bin 路径, mtime) 缓存结果——rows 快照/可升级判定每次都会
/// 查版本,mtime 未变时直接命中缓存(安装/卸载改写文件 → mtime 变化 → 自动失效)。
/// 仅供 UI 展示用版本;受管/本机判定不走这里(availability 只做 PATH 存在性检查)。
pub fn detect_version(bin: &Path) -> Option<String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    /// (mtime, 版本) 缓存：mtime 未变 ⇒ 直接复用
    type VersionCache =
        Mutex<HashMap<std::path::PathBuf, (Option<std::time::SystemTime>, Option<String>)>>;
    static CACHE: OnceLock<VersionCache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mtime = std::fs::metadata(bin).ok().and_then(|m| m.modified().ok());
    if let Ok(guard) = cache.lock() {
        if let Some((mt, v)) = guard.get(bin) {
            if *mt == mtime {
                return v.clone();
            }
        }
    }
    let probe = (|| -> Option<String> {
        let mut child = std::process::Command::new(bin)
            .arg("--version")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let started = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() <= DETECT_TIMEOUT => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                // 超时:杀子进程(等待其退出防僵尸),视为不可检测
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                Err(_) => return None,
            }
        }
        let out = child.wait_with_output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let s = if s.trim().is_empty() {
            String::from_utf8_lossy(&out.stderr).to_string()
        } else {
            s.to_string()
        };
        let t = s.lines().next().unwrap_or("").trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    })();
    if let Ok(mut guard) = cache.lock() {
        guard.insert(bin.to_path_buf(), (mtime, probe.clone()));
    }
    probe
}

/// rust target triple(常用 release asset 命名用);不支持的平台返回空 → 安装时报错
pub fn rust_triple() -> String {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "arm") => "arm-unknown-linux-gnueabihf",
        ("linux", "riscv64") => "riscv64gc-unknown-linux-gnu",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        _ => "",
    }
    .to_string()
}

/// url_template 占位展开:{version}/{os}/{arch}/{triple}
pub(crate) fn expand_template(t: &str, version: &str) -> String {
    t.replace("{version}", version)
        .replace("{os}", std::env::consts::OS)
        .replace("{arch}", std::env::consts::ARCH)
        .replace("{triple}", &rust_triple())
}

// ============================ 安装/升级/卸载 ============================
