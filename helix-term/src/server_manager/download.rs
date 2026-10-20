//! HTTP 下载、镜像前缀、sha256、下载进度钩子。
//!
//! 从 `server_manager/mod.rs` 抽出(G8b)。模块间不用 glob(`use super::*`),
//! 而是按编译器提示显式 `use super::<mod>::{…}` —— 双向 glob 会构成 glob 环,
//! 解析器会把环当成空集合。

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context as _, Result};

/// 建软链:`std::os::unix::fs::symlink` 在非 unix 不存在 ⇒ 各平台走各自的门面。
/// Windows 建链需开发者模式/管理员权限,失败照常冒泡成错误(不静默)。
pub(crate) fn symlink_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)
    }
    #[cfg(not(unix))]
    {
        std::os::windows::fs::symlink_file(src, dst)
    }
}

/// 下载层 mirror 前缀(空 = 直连):经 [server-manager].mirror 配置(T4 读入)
pub(crate) fn mirror_prefix() -> String {
    MIRROR
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|g| g.clone())
        .unwrap_or_default()
}

pub fn set_mirror(prefix: &str) {
    MIRROR
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .replace(prefix.to_string());
}

pub(crate) static MIRROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// 写互斥：managed/ + languages.toml 的写流(后台 worker 批次 / :server 同步子命令 /
/// :server panel Enter)不得交错——两个流同时改写受管目录/标记段会互相踩。
/// 锁范围 = "整 op 含最终 rewrite_languages_toml"（run_batch 整批、server_op 写臂、
/// panel Enter 各自在首写前拿锁、尾写后释放）。持锁含下载(分钟级)：后台任务进行中，
/// 主线程同步 :server 写命令会阻塞到其结束——锁只防坏不防等(文档已注明)。
/// 单一层级：锁内代码不会再触发 submit/run_batch/同步写(无重入路径)。
pub(crate) static OP_LOCK: Mutex<()> = Mutex::new(());

/// 取写锁（跨模块：commands/typed.rs、commands/server_tasks.rs 共用）
pub(crate) fn op_lock() -> std::sync::MutexGuard<'static, ()> {
    OP_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// URL 经 mirror 改写:file:// 与本地路径原样;https 前加 mirror 前缀(若配置)。
pub(crate) fn mirror_url(raw: &str) -> String {
    let p = mirror_prefix();
    if p.is_empty() || raw.starts_with("file:") || raw.starts_with('/') {
        raw.to_string()
    } else {
        format!("{p}{raw}")
    }
}

/// sha256 校验文件(小文件;archive 动辄几十 MB,读入内存校验可接受,v1 简化)
pub(crate) fn sha256_hex(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// 下载进度聚合:按字节阈值或时间间隔回调;finish 必 flush。
/// add(bytes) 为**本次增量**(内部累计);回调收到累计已下载字节;finish(bytes) 为最终累计。
pub struct ProgressAgg<'a> {
    throttle_bytes: u64,
    throttle_dur: std::time::Duration,
    cb: &'a mut dyn FnMut(u64, Option<u64>),
    done: u64,
    last_bytes: u64,
    last_at: std::time::Instant,
}

impl<'a> ProgressAgg<'a> {
    pub fn new(
        throttle_bytes: u64,
        throttle_dur: std::time::Duration,
        cb: &'a mut dyn FnMut(u64, Option<u64>),
    ) -> Self {
        ProgressAgg {
            throttle_bytes,
            throttle_dur,
            cb,
            done: 0,
            last_bytes: 0,
            last_at: std::time::Instant::now(),
        }
    }
    pub fn add(&mut self, bytes: u64, total: Option<u64>) {
        self.done += bytes;
        let now = std::time::Instant::now();
        if self.done >= self.last_bytes + self.throttle_bytes
            || now.duration_since(self.last_at) >= self.throttle_dur
        {
            self.last_bytes = self.done;
            self.last_at = now;
            (self.cb)(self.done, total);
        }
    }
    pub fn finish(&mut self, bytes: u64, total: Option<u64>) {
        self.done = self.done.max(bytes);
        self.last_bytes = self.done;
        (self.cb)(self.done, total);
    }
}

// 下载字节进度钩子(线程局部):单 worker 队列线程在跑 install/update 前经
// [with_download_progress] 挂载;install_adhoc 的 download_to 消费一次(take)。
// 挂载方每批(含内联回退)结束须调 [clear_download_progress] 清除——install_adhoc
// 的 take 只发生在真实下载处,未达下载点的批次会把残留钩子留在当前线程,
// 主线程若带残留跑同步 :server install 会消费到死 task_id/seq 的幽灵钩子。
// 主线程 :server 同步路径本身不挂 → 空钩子,与 M2a 行为一致。
thread_local! {
    pub(crate) static DL_PROGRESS: DlProgressSlot =
        const { std::cell::RefCell::new(None) };
}

/// 下载进度钩子的**线程局部槽**类型 —— 单独起名只为避开 clippy::type_complexity。
pub(crate) type DlProgressSlot =
    std::cell::RefCell<Option<Box<dyn FnMut(u64, Option<u64>) + Send>>>;

/// 挂载下载字节进度回调(每次 install_adhoc 下载消费一次;download_to 层已节流)。
/// 每批结束后须调 [clear_download_progress](run_batch 已在两分支处理)。
pub fn with_download_progress(f: Box<dyn FnMut(u64, Option<u64>) + Send>) {
    DL_PROGRESS.with(|c| *c.borrow_mut() = Some(f));
}

/// 清除当前线程的下载进度钩子(worker/内联路径每批收尾调用;防止未达下载点的
/// 批次把残留钩子留给同线程后续同步下载)。
pub fn clear_download_progress() {
    DL_PROGRESS.with(|c| *c.borrow_mut() = None);
}

/// 当前线程是否挂有下载进度钩子(仅测试断言用)
#[cfg(test)]
pub(crate) fn download_progress_hook_set() -> bool {
    DL_PROGRESS.with(|c| c.borrow().is_some())
}

/// 环境里的 HTTP 代理(按 curl 的惯例:`HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY`,小写优先)。
///
/// 为什么必须自己读:**ureq 2.x 不会读这些环境变量**(curl / reqwest 会)。实测后果:
/// 同一台机器上 `curl` 下 GitHub release 正常,而 `:server install` 直接挂死在直连上
/// —— 主线程同步下载,表现为整个 UI 冻住(文档里"锁防坏不防等"说的就是这个场景)。
///
/// 已知边界:不实现 `NO_PROXY`(ureq 无此能力)。需要的场合请改配 `[server-manager] mirror`
/// 或直连;内网/本机地址也会走代理。
pub(crate) fn proxy_from_env() -> Option<ureq::Proxy> {
    // 小写优先(与 curl 一致:小写覆盖大写)
    // 空串要**跳过并继续找**(不是"找到空串就当没有"):小写设成 "" 而大写有值时,
    // 后者仍应生效(测试 proxy_env_is_read_lowercase_first 钉住这条)
    let raw = [
        "https_proxy",
        "HTTPS_PROXY",
        "http_proxy",
        "HTTP_PROXY",
        "all_proxy",
        "ALL_PROXY",
    ]
    .iter()
    .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))?;
    ureq::Proxy::new(raw.trim()).ok()
}

/// 下载用 agent:带代理(若有)+ **明确的超时**。
///
/// 超时的意义:网络不通时旧实现会一直等内核 TCP 超时(分钟级),期间主线程同步下载
/// ⇒ UI 冻死且没有任何反馈。连接 10s、整体 10 分钟(几十 MB 的大包留足余量)。
fn download_agent() -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(600));
    if let Some(proxy) = proxy_from_env() {
        builder = builder.proxy(proxy);
    }
    builder.build()
}

/// 下载 URL 到 dest(file:// 本地复制;https 经 ureq;遵循 mirror)。
/// progress(bytes, total) 节流回调;total 未知为 None;file:// 复制完成后回调一次(总大小)。
pub(crate) fn download_to(
    url: &str,
    dest: &Path,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<()> {
    if let Some(path) = url.strip_prefix("file://") {
        let size = std::fs::copy(path, dest)
            .with_context(|| format!("copy {path} -> {}", dest.display()))?;
        progress(size, Some(size));
        return Ok(());
    }
    use std::io::{Read as _, Write as _};
    let resp = download_agent()
        .get(url)
        .call()
        .with_context(|| format!("download {url}"))?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok());
    let mut reader = resp.into_reader();
    let mut out = std::fs::File::create(dest)?;
    let mut buf = [0u8; 64 * 1024];
    let mut acc = 0u64;
    let mut agg = ProgressAgg::new(
        256 * 1024,
        std::time::Duration::from_millis(80),
        &mut progress,
    );
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("read {url}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        acc += n as u64;
        agg.add(n as u64, total); // add 收增量(内部累计);循环后 finish(acc) flush 到 100%
    }
    agg.finish(acc, total);
    Ok(())
}
