//! Server Manager（mason 式）——LSP/DAP/linter/formatter 的安装/升级/卸载管理。
//!
//! 设计(见 docs/superpowers/specs/2026-09-05-server-manager-design.md):
//! - 注册表:内置 ServerSpec 常量 + config 扩展(T4)
//! - 受管目录 `~/.local/share/helix/managed/<name>`(T1 默认;T4 可配)
//! - archive 配方:mirror URL 改写 + sha256 校验 + tar.gz/zip 解压 + managed/bin 软链
//! - languages.toml 标记段自动写(T3)
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Context as _, Result};

/// 下载层 mirror 前缀(空 = 直连):经 [server-manager].mirror 配置(T4 读入)
fn mirror_prefix() -> String {
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

static MIRROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// URL 经 mirror 改写:file:// 与本地路径原样;https 前加 mirror 前缀(若配置)。
fn mirror_url(raw: &str) -> String {
    let p = mirror_prefix();
    if p.is_empty() || raw.starts_with("file:") || raw.starts_with('/') {
        raw.to_string()
    } else {
        format!("{p}{raw}")
    }
}

/// sha256 校验文件(小文件;archive 动辄几十 MB,读入内存校验可接受,v1 简化)
fn sha256_hex(path: &Path) -> Result<String> {
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
    static DL_PROGRESS: std::cell::RefCell<Option<Box<dyn FnMut(u64, Option<u64>) + Send>>> =
        const { std::cell::RefCell::new(None) };
}

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

/// 下载 URL 到 dest(file:// 本地复制;https 经 ureq;遵循 mirror)。
/// progress(bytes, total) 节流回调;total 未知为 None;file:// 复制完成后回调一次(总大小)。
fn download_to(url: &str, dest: &Path, mut progress: impl FnMut(u64, Option<u64>)) -> Result<()> {
    if let Some(path) = url.strip_prefix("file://") {
        let size = std::fs::copy(path, dest)
            .with_context(|| format!("copy {path} -> {}", dest.display()))?;
        progress(size, Some(size));
        return Ok(());
    }
    use std::io::{Read as _, Write as _};
    let resp = ureq::get(url)
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

/// 解压到 out_dir(入口;archive 可为 tar/tar.gz/zip 或单文件 gzip):
/// 单文件 gzip 需给 single_name(解码后写入 out/single_name 并加可执行位)。
fn extract_archive(path: &Path, out_dir: &Path, strip: usize) -> Result<()> {
    let raw = std::fs::read(path)?;
    extract_bytes(&raw, out_dir, strip, None)
}

/// install 用载荷落盘:zip/tar 走目录解压;单文件 gzip(rust-analyzer 类 asset)解码为 bin_rel。
fn install_payload(archive: &Path, out_dir: &Path, strip: usize, bin_rel: &str) -> Result<()> {
    let raw = std::fs::read(archive)?;
    extract_bytes(&raw, out_dir, strip, Some(bin_rel))
}

fn extract_bytes(
    raw: &[u8],
    out_dir: &Path,
    strip: usize,
    single_name: Option<&str>,
) -> Result<()> {
    use std::io::Read as _;
    std::fs::create_dir_all(out_dir)?;
    if raw.starts_with(&[0x1f, 0x8b]) {
        // gzip:先整解再判 tar(单文件 gzip 也是合法 gzip)
        let mut dec = Vec::new();
        flate2::read::GzDecoder::new(std::io::Cursor::new(raw))
            .read_to_end(&mut dec)
            .with_context(|| "gzip 解压失败")?;
        if is_tar_bytes(&dec) {
            extract_tar_bytes(&dec, out_dir, strip)
        } else {
            let Some(name) = single_name else {
                return Err(anyhow!("gzip 内容不是 tar,且未给单文件目标名"));
            };
            let dest = out_dir.join(name);
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::write(&dest, &dec)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
            }
            Ok(())
        }
    } else if raw.starts_with(b"PK") {
        extract_zip_bytes(raw, out_dir, strip)
    } else {
        extract_tar_bytes(raw, out_dir, strip)
    }
}

fn is_tar_bytes(b: &[u8]) -> bool {
    b.len() > 265 && &b[257..262] == b"ustar"
}

fn extract_zip_bytes(raw: &[u8], out_dir: &Path, strip: usize) -> Result<()> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(raw))?;
    let names: Vec<(String, bool)> = (0..z.len())
        .map(|i| {
            let f = z.by_index(i).unwrap();
            (f.name().to_string(), f.is_dir())
        })
        .collect();
    for (name, is_dir) in names {
        let rel = strip_path(&name, strip);
        if rel.is_empty() {
            continue;
        }
        let dest = out_dir.join(&rel);
        if is_dir {
            std::fs::create_dir_all(&dest)?;
        } else {
            std::fs::create_dir_all(dest.parent().unwrap())?;
            let mut src = z.by_name(&name)?;
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut src, &mut out)?;
        }
    }
    Ok(())
}

fn extract_tar_bytes(raw: &[u8], out_dir: &Path, strip: usize) -> Result<()> {
    let mut ar = tar::Archive::new(std::io::Cursor::new(raw));
    for entry in ar.entries()? {
        let mut e = entry?;
        let path_in = e.path()?.into_owned();
        let rel = strip_path(&path_in.to_string_lossy(), strip);
        if rel.is_empty() {
            continue;
        }
        let dest = out_dir.join(&rel);
        if e.header().entry_type().is_dir() {
            std::fs::create_dir_all(&dest)?;
        } else {
            std::fs::create_dir_all(dest.parent().unwrap())?;
            e.unpack(&dest)?;
        }
    }
    Ok(())
}

fn strip_path(name: &str, strip: usize) -> String {
    let mut parts: Vec<&str> = name.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= strip {
        return String::new();
    }
    parts.drain(..strip);
    parts.join("/")
}

/// 运行 `<bin> --version` 提取版本串(失败返回 None,视为未安装/不可检测)
pub fn detect_version(bin: &Path) -> Option<String> {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .ok()?;
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
}

/// 默认受管根目录:`~/.local/share/helix/managed`
pub fn managed_root() -> PathBuf {
    if let Ok(dir) = std::env::var("SM_MANAGED_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/share/helix/managed")
}

/// 某工具的受管目录(未创建)
pub fn managed_dir(name: &str) -> PathBuf {
    managed_root().join(name)
}

pub fn managed_bin(name: &str) -> PathBuf {
    managed_root().join("bin").join(name)
}

/// 工具是否已安装(受管目录存在且含配方 bin)
pub fn is_installed(name: &str) -> bool {
    registry::get(name)
        .map(|s| s.bin_path().exists())
        .unwrap_or(false)
}

/// 从磁盘 config.toml 取 [server-manager] 段(与 plugin manifest 同哲学:磁盘为源)。
/// SM_SERVER_CONFIG 可覆写读取路径(与 SM_MANAGED_DIR/SM_LANGS_TOML 同哲学:
/// 集成测试用 env 指向临时 config,生产不设该 env 零影响)。
pub fn disk_server_manager_table() -> Option<toml::Table> {
    let text = std::fs::read_to_string(
        std::env::var("SM_SERVER_CONFIG")
            .ok()
            .map(PathBuf::from)
            .or_else(helix_loader::config_file_opt)?,
    )
    .ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    v.get("server-manager")
        .and_then(toml::Value::as_table)
        .cloned()
}

/// 从 [server-manager] 段读入 mirror + 扩展配方(registry.<name>)。
/// dir 覆写 v1 未支持(默认受管目录;环境 SM_MANAGED_DIR 可测)。
pub fn apply_server_config(cfg: Option<&toml::Table>) -> Result<()> {
    set_mirror("");
    registry::set_ext(Vec::new());
    let Some(cfg) = cfg else {
        return Ok(());
    };
    if let Some(m) = cfg.get("mirror").and_then(toml::Value::as_str) {
        set_mirror(m);
    }
    if let Some(dir) = cfg.get("dir") {
        let _ = dir; // v1 忽略 dir 覆写(默认 ~/.local/share/helix/managed)
    }
    let mut ext = Vec::new();
    if let Some(reg) = cfg.get("registry").and_then(toml::Value::as_table) {
        for (name, v) in reg {
            let tbl = v
                .as_table()
                .ok_or_else(|| anyhow!("[server-manager.registry.{name}] 需是表"))?;
            ext.push(registry::parse_ext_recipe(name, tbl)?);
        }
    }
    registry::set_ext(ext);
    Ok(())
}

// ============================ languages.toml 标记段 ============================
//
// 文本手术:文件由 标记段(# >>> helix-managed … # <<< helix-managed)内的受管内容 +
// 标记外用户手写内容组成。每次 install/update/remove 后剥离旧段 → 按已装清单重建 →
// 写回,标记外内容逐字节保留(幂等)。用户自定义了某语言 → 不生成其 [[language]]
// 条目(避免覆盖用户配置,只给 hint);用户自带同名 [language-server.X] → 不重复写表
// (TOML 禁止同一 doc 重复表头)。

pub const MANAGED_HEADER: &str = "# >>> helix-managed";
pub const MANAGED_FOOTER: &str = "# <<< helix-managed";

/// languages.toml 路径:SM_LANGS_TOML 可覆写(测试隔离;否则 helix 全局配置目录)
pub fn lang_config_file() -> PathBuf {
    if let Ok(p) = std::env::var("SM_LANGS_TOML") {
        return PathBuf::from(p);
    }
    helix_loader::lang_config_file()
}

/// 已安装的 registry 配方(managed/bin 在位)
pub fn installed_specs() -> Vec<registry::Spec> {
    let mut v: Vec<_> = registry::all()
        .into_iter()
        .filter(|s| is_installed(&s.name))
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// 受管已装且 detected 版本串不含配方 version → 可升级。
/// detect_version 返回 `<bin> --version` 首行整串,常带工具名前缀
/// (如 "rust-analyzer 2024-09-16"),故用"包含"判定同版;不做 semver。
pub fn is_upgradable(spec: &registry::Spec) -> bool {
    match spec.version_detected() {
        Some(v) => spec
            .version
            .as_deref()
            .is_some_and(|want| !v.contains(want)),
        None => false,
    }
}

/// 安装/更新是否需要用户显式提供版本:archive 下载源 url 含 {version} 占位
/// 且配方未固定 version(config 扩展解析已强制 {version} 配 version,故 true 只可能
/// 来自内置活配方,如 rust-analyzer;Tool 类与无占位 url 恒 false)。
pub fn needs_version(spec: &registry::Spec) -> bool {
    match &spec.install {
        registry::Install::Archive { url_template, .. } => {
            url_template.contains("{version}") && spec.version.is_none()
        }
        registry::Install::Tool { .. } => false,
    }
}

/// 工具可用来源
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// 受管安装(managed/bin 软链;可升级/卸载)
    Managed,
    /// 本地 PATH 已有(非受管;直接使用;不能卸载)
    Local(PathBuf),
    /// 缺失
    Missing,
}

/// 状态判定:受管优先;其次 PATH 中的 bin_name(排除受管 bin 目录自身,
/// 避免 managed/bin 入 PATH 时把受管软链误判为 local)。
pub fn availability(name: &str) -> Availability {
    let Some(spec) = registry::get(name) else {
        return Availability::Missing;
    };
    let managed = spec.bin_path();
    if managed.exists() {
        return Availability::Managed;
    }
    match path_in_path(&spec.bin_name) {
        Some(p) => Availability::Local(p),
        None => Availability::Missing,
    }
}

/// 在 PATH 中查找可执行文件(不含受管 bin 目录)。
/// SM_PATH 为测试/开发覆写(空串 = 禁用本地检测;生产不设)。
fn path_in_path(bin: &str) -> Option<PathBuf> {
    let managed_bin_dir = managed_root().join("bin");
    let path = std::env::var("SM_PATH")
        .or_else(|_| std::env::var("PATH"))
        .ok()?;
    for dir in std::env::split_paths(&path) {
        if dir == managed_bin_dir {
            continue;
        }
        let cand = dir.join(bin);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// 本地可用(非受管、未被忽略)的配方
pub fn local_specs() -> Vec<registry::Spec> {
    let mut v: Vec<_> = registry::all()
        .into_iter()
        .filter(|s| matches!(availability(&s.name), Availability::Local(_)) && !is_ignored(&s.name))
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// 本地停用挂接名单(managed_root/ignored.txt,每行一个配方名)
fn ignored_path() -> PathBuf {
    managed_root().join("ignored.txt")
}

pub fn ignored_local() -> Vec<String> {
    std::fs::read_to_string(ignored_path())
        .map(|t| {
            t.lines()
                .map(str::to_string)
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn is_ignored(name: &str) -> bool {
    ignored_local().iter().any(|n| n == name)
}

/// 设/清本地忽略并返回最新名单
pub fn set_ignored(name: &str, ignored: bool) -> Result<Vec<String>> {
    let mut list = ignored_local();
    let pos = list.iter().position(|n| n == name);
    match (ignored, pos) {
        (true, None) => list.push(name.to_string()),
        (false, Some(i)) => {
            list.remove(i);
        }
        _ => {}
    }
    std::fs::create_dir_all(managed_root())?;
    std::fs::write(
        ignored_path(),
        format!(
            "{}
",
            list.join("\n")
        ),
    )?;
    Ok(list)
}

/// 拆分标记段:返回 (标记外文本, 段内文本)。无标记 → 原文本;段不完整/重复 → Err。
pub fn split_managed(text: &str) -> Result<(String, Option<String>)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let idxs = |m: &str| -> Vec<usize> {
        lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.trim() == m)
            .map(|(i, _)| i)
            .collect()
    };
    let (hs, fs) = (idxs(MANAGED_HEADER), idxs(MANAGED_FOOTER));
    if hs.is_empty() && fs.is_empty() {
        return Ok((text.to_string(), None));
    }
    if hs.len() != 1 || fs.len() != 1 {
        return Err(anyhow!(
            "languages.toml 标记段不完整(需且仅需一对 {} / {}),未改写",
            MANAGED_HEADER,
            MANAGED_FOOTER
        ));
    }
    let (h, f) = (hs[0], fs[0]);
    if h >= f {
        return Err(anyhow!(
            "languages.toml 标记段顺序错误({} 在 {} 后),未改写",
            MANAGED_HEADER,
            MANAGED_FOOTER
        ));
    }
    let mut outside: Vec<&str> = lines[..h].to_vec();
    outside.extend_from_slice(&lines[f + 1..]);
    Ok((outside.join("\n"), Some(lines[h + 1..f].join("\n"))))
}

/// 解析标记外文本里用户定义的语言名与顶层 language-server 表名(供冲突判断)
fn user_defined(text: &str) -> (Vec<String>, Vec<String>) {
    let Ok(v) = toml::from_str::<toml::Value>(text) else {
        return (Vec::new(), Vec::new());
    };
    let langs = v
        .get("language")
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.get("name")?.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let lss = v
        .get("language-server")
        .and_then(|t| t.as_table())
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();
    (langs, lss)
}

fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn abs_bin(bin: &Path) -> PathBuf {
    if bin.is_absolute() {
        bin.to_path_buf()
    } else {
        std::path::absolute(bin).unwrap_or_else(|_| bin.to_path_buf())
    }
}

/// 一次重写的结果(供命令层展示/提示;path/servers 供测试与后续 UI 读)
#[derive(Debug)]
#[allow(dead_code)]
pub struct LangRewrite {
    pub path: PathBuf,
    /// 是否实际写了文件(false = 无内容可写且文件本不存在)
    pub written: bool,
    /// 写入的 [language-server.<n>] 表名(已装 LSP 配方,排除用户自带同名)
    pub servers: Vec<String>,
    /// 提示:语言已被用户自定义,未自动挂接(附手动引用片段)
    pub conflicts: Vec<String>,
}

/// 重建受管段文本(不写盘)。`user_langs`/`user_ls` = 用户已在标记外定义的语言/顶层
/// language-server 表名。返回 (段文本, servers, conflicts)。
pub fn build_section(
    user_langs: &[String],
    user_ls: &[String],
) -> (String, Vec<String>, Vec<String>) {
    let all = registry::all();
    let mut servers = Vec::new();
    let mut conflicts = Vec::new();
    let mut body = String::new();
    // 参与挂接:受管已装 ∪ 本地 PATH 可用(未被忽略);记录 command 来源
    struct Entry<'a> {
        spec: &'a registry::Spec,
        /// None=受管(绝对路径);Some(abs)=本地 PATH 路径
        local: Option<PathBuf>,
    }
    let mut attached: Vec<Entry> = Vec::new();
    for spec in &all {
        let is_attach_kind = matches!(spec.kind, registry::Kind::Lsp | registry::Kind::Dap);
        if !is_attach_kind {
            continue;
        }
        match availability(&spec.name) {
            Availability::Managed => attached.push(Entry { spec, local: None }),
            Availability::Local(p) if !is_ignored(&spec.name) => attached.push(Entry {
                spec,
                local: Some(p),
            }),
            _ => {}
        }
    }
    attached.sort_by(|a, b| a.spec.name.cmp(&b.spec.name));
    // [language-server.<name>] 表(Lsp 且用户未自带同名)
    for e in &attached {
        let s = e.spec;
        if matches!(s.kind, registry::Kind::Lsp) && !user_ls.iter().any(|n| n == &s.name) {
            body.push_str(&format!("[language-server.{}]\n", s.name));
            let cmd = match &e.local {
                Some(p) => abs_bin(p).to_string_lossy().into_owned(),
                None => abs_bin(&s.bin_path()).to_string_lossy().into_owned(),
            };
            body.push_str(&format!("command = {}\n\n", toml_str(&cmd)));
            if e.local.is_none() {
                servers.push(s.name.clone());
            }
        }
    }
    // 按语言聚合 lsp 名与 dap 配置
    let mut by_lang: std::collections::BTreeMap<String, (Vec<String>, Option<&registry::Spec>)> =
        std::collections::BTreeMap::new();
    for e in &attached {
        for lang in &e.spec.languages {
            let slot = by_lang.entry(lang.clone()).or_default();
            match e.spec.kind {
                registry::Kind::Lsp => slot.0.push(e.spec.name.clone()),
                registry::Kind::Dap => slot.1 = Some(e.spec),
                _ => {}
            }
        }
    }
    for (lang, (lsp, dap)) in by_lang {
        let custom = user_langs.iter().any(|u| u == &lang);
        if custom {
            // 用户已自定义该语言 → 不生成条目,提示手动挂接
            for n in &lsp {
                conflicts.push(format!(
                    "语言 '{lang}' 你已自定义:请在它的 [[language]] 条目加 language-servers = [{}]",
                    toml_str(n)
                ));
            }
            if let Some(s) = dap {
                let hint = attached
                    .iter()
                    .find(|e| std::ptr::eq(e.spec, s))
                    .and_then(|e| e.local.clone())
                    .unwrap_or_else(|| s.bin_path());
                conflicts.push(format!(
                    "语言 '{lang}' 你已自定义:请手动为它配置 debugger(command 指向 {})",
                    hint.display()
                ));
            }
            continue;
        }
        if lsp.is_empty() && dap.is_none() {
            continue;
        }
        body.push_str("[[language]]\n");
        body.push_str(&format!("name = {}\n", toml_str(&lang)));
        if !lsp.is_empty() {
            let list = lsp
                .iter()
                .map(|n| toml_str(n))
                .collect::<Vec<_>>()
                .join(", ");
            body.push_str(&format!("language-servers = [{list}]\n"));
        }
        if let Some(s) = dap {
            let cmd = attached
                .iter()
                .find(|e| std::ptr::eq(e.spec, s))
                .and_then(|e| e.local.clone())
                .unwrap_or_else(|| s.bin_path());
            body.push_str(&format!(
                "debugger = {{ name = {}, transport = \"stdio\", command = {}, args = [], templates = [{{ name = \"launch\", request = \"launch\", args = {{ }} }}] }}\n",
                toml_str(&s.name),
                toml_str(&abs_bin(&cmd).to_string_lossy())
            ));
        }
        body.push('\n');
    }
    if body.is_empty() {
        (String::new(), servers, conflicts)
    } else {
        (
            format!("{MANAGED_HEADER}\n{body}{MANAGED_FOOTER}\n"),
            servers,
            conflicts,
        )
    }
}

/// 剥离旧段 → 校验用户部分 TOML 合法性 → 按已装清单重建 → 原子写回。
pub fn rewrite_languages_toml() -> Result<LangRewrite> {
    let path = lang_config_file();
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).context("读 languages.toml"),
    };
    let (outside, _inner) = split_managed(&existing)?;
    if !outside.trim().is_empty() {
        toml::from_str::<toml::Value>(&outside)
            .context("用户 languages.toml 语法错误,未改写(请先修复)")?;
    }
    let (user_langs, user_ls) = user_defined(&outside);
    let (section, servers, conflicts) = build_section(&user_langs, &user_ls);
    let outside_trim = outside.trim_end();
    let mut final_text = String::new();
    if !outside_trim.is_empty() {
        final_text.push_str(outside_trim);
        final_text.push_str("\n\n");
    }
    if !section.is_empty() {
        final_text.push_str(&section);
    }
    if final_text.is_empty() && existing.is_empty() {
        // 无用户内容且无可写条目:文件本不存在就不创建空文件
        return Ok(LangRewrite {
            path,
            written: false,
            servers,
            conflicts,
        });
    }
    // 整份(用户+生成)必须可解析,否则不写
    toml::from_str::<toml::Value>(&final_text).context("生成结果无法解析为 TOML,未改写")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        ".{}.sm.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&tmp, &final_text).with_context(|| format!("写 {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("落盘 {}", path.display()))?;
    Ok(LangRewrite {
        path,
        written: true,
        servers,
        conflicts,
    })
}

// ============================ registry ============================

pub mod registry {
    use super::*;

    /// 工具种类
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Kind {
        Lsp,
        Dap,
        Linter,
        Formatter,
    }

    /// 安装方式(owned;config 扩展配方经 TOML 构造)
    #[derive(Debug, Clone)]
    pub enum Install {
        /// 下载 release 产物(可 tar.gz/zip);url_template 含 {version} 占位
        Archive {
            url_template: String,
            sha256: String,
            strip: usize,
            /// 解压后要链接到 managed/bin 的可执行相对路径(相对解压根)
            bin_rel: String,
        },
        /// 包管理器类(pip --target / cargo --root / npm --prefix):cmd 在受管目录内执行,
        /// args 占位 {version}、{prefix}(= 受管目录);产物须落在 <prefix>/bin/<bin_name>
        Tool { cmd: String, args: Vec<String> },
    }

    /// 配方(内置 + config 扩展同构)
    #[derive(Debug, Clone)]
    pub struct Spec {
        pub name: String,
        pub kind: Kind,
        pub languages: Vec<String>,
        pub bin_name: String,
        pub description: String,
        pub homepage: Option<String>,
        pub install: Install,
        /// 固定安装版本({version} 占位填充);None = install/update 须显式版本
        pub version: Option<String>,
    }

    impl Spec {
        #[allow(clippy::too_many_arguments)]
        fn archive(
            name: &str,
            kind: Kind,
            languages: &[&str],
            bin_name: &str,
            url: &str,
            sha: &str,
            strip: usize,
            bin_rel: &str,
        ) -> Spec {
            Spec {
                name: name.to_string(),
                kind,
                languages: languages.iter().map(|s| s.to_string()).collect(),
                bin_name: bin_name.to_string(),
                description: String::new(),
                homepage: None,
                install: Install::Archive {
                    url_template: url.to_string(),
                    sha256: sha.to_string(),
                    strip,
                    bin_rel: bin_rel.to_string(),
                },
                version: None,
            }
        }
        fn tool(
            name: &str,
            kind: Kind,
            languages: &[&str],
            bin_name: &str,
            cmd: &str,
            args: &[&str],
        ) -> Spec {
            Spec {
                name: name.to_string(),
                kind,
                languages: languages.iter().map(|s| s.to_string()).collect(),
                bin_name: bin_name.to_string(),
                description: String::new(),
                homepage: None,
                install: Install::Tool {
                    cmd: cmd.to_string(),
                    args: args.iter().map(|s| s.to_string()).collect(),
                },
                version: None,
            }
        }

        /// 补展示数据(description 中文一句 / homepage 官方链接);内置配方用
        fn describe(mut self, description: &str, homepage: &str) -> Self {
            self.description = description.to_string();
            self.homepage = Some(homepage.to_string());
            self
        }

        /// 已装工具的 bin 绝对路径(managed/bin/<bin_name> 软链)
        pub fn bin_path(&self) -> PathBuf {
            managed_root().join("bin").join(&self.bin_name)
        }
        #[cfg(test)]
        pub fn installed_dir(&self) -> PathBuf {
            managed_dir(&self.name)
        }
        pub fn version_detected(&self) -> Option<String> {
            detect_version(&self.bin_path())
        }
        /// 有可用下载源(否则 install 报"配方未配置")
        pub fn is_installable(&self) -> bool {
            match &self.install {
                Install::Archive { url_template, .. } => !url_template.is_empty(),
                Install::Tool { cmd, .. } => !cmd.is_empty(),
            }
        }
    }

    /// 内置注册表(v1 子集)。配方为"惰性占位":下载源留空 = install 报"未配置"
    /// (避免无校验/误装);真实 URL/版本/校验是 OS 相关数据,经
    /// [server-manager.registry.<name>] config 扩展(T4)提供可跑源。
    /// M5:内置配方补 description(中文一句)/homepage(官方仓库)——arsenal 市场/信息弹窗展示。
    pub(crate) fn builtin_specs() -> Vec<Spec> {
        vec![
            Spec::archive(
                "rust-analyzer",
                Kind::Lsp,
                &["rust"],
                "rust-analyzer",
                "https://github.com/rust-lang/rust-analyzer/releases/download/{version}/rust-analyzer-{triple}.gz",
                "",
                0,
                "rust-analyzer",
            )
            .describe(
                "Rust 语言服务器(rust-analyzer)",
                "https://github.com/rust-lang/rust-analyzer",
            ),
            Spec::archive("gopls", Kind::Lsp, &["go"], "gopls", "", "", 1, "gopls")
                .describe("Go 官方语言服务器(gopls)", "https://github.com/golang/tools"),
            Spec::archive(
                "pyright",
                Kind::Lsp,
                &["python"],
                "pyright-langserver",
                "",
                "",
                1,
                "pyright-langserver",
            )
            .describe(
                "Python 静态类型检查器/Pylance 内核(pyright)",
                "https://github.com/microsoft/pyright",
            ),
            Spec::archive(
                "clangd",
                Kind::Lsp,
                &["c", "cpp"],
                "clangd",
                "",
                "",
                1,
                "clangd",
            )
            .describe(
                "C/C++ 语言服务器(clangd, LLVM 官方)",
                "https://clangd.llvm.org",
            ),
            Spec::tool("debugpy", Kind::Dap, &["python"], "debugpy", "", &[]).describe(
                "Python 调试适配器(debugpy, VS Code 内核)",
                "https://github.com/microsoft/debugpy",
            ),
            Spec::tool("black", Kind::Formatter, &["python"], "black", "", &[]).describe(
                "Python 代码格式化器(black)",
                "https://github.com/psf/black",
            ),
            Spec::tool(
                "prettier",
                Kind::Formatter,
                &[
                    "javascript",
                    "typescript",
                    "html",
                    "css",
                    "json",
                    "markdown",
                ],
                "prettier",
                "",
                &[],
            )
            .describe(
                "多语言代码格式化器(prettier)",
                "https://github.com/prettier/prettier",
            ),
        ]
    }

    static EXT: OnceLock<Mutex<Vec<Spec>>> = OnceLock::new();
    /// 重读 config 扩展配方(每次 server 操作前从 [server-manager.registry] 解析后调用)
    pub fn set_ext(specs: Vec<Spec>) {
        *EXT.get_or_init(Default::default).lock().unwrap() = specs;
    }

    /// 全部配方:内置 + config 扩展(快照)
    pub fn all() -> Vec<Spec> {
        let mut v = builtin_specs();
        if let Some(m) = EXT.get() {
            v.extend(m.lock().unwrap().clone());
        }
        v
    }

    pub fn get(name: &str) -> Option<Spec> {
        all().into_iter().find(|s| s.name == name)
    }

    /// 解析 [server-manager.registry.<name>] 配方表(v1:archive 类)。
    pub fn parse_ext_recipe(name: &str, tbl: &toml::Table) -> Result<Spec> {
        use toml::Value;
        let kind = match tbl.get("kind").and_then(Value::as_str) {
            None | Some("lsp") => Kind::Lsp,
            Some("dap") => Kind::Dap,
            Some("linter") => Kind::Linter,
            Some("formatter") => Kind::Formatter,
            Some(k) => {
                return Err(anyhow!(
                    "registry '{name}': 未知 kind '{k}'(lsp|dap|linter|formatter)"
                ))
            }
        };
        let need = |k: &str| -> Result<String> {
            tbl.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| anyhow!("registry '{name}': 缺必填 '{k}'"))
        };
        let url = need("url")?;
        let version = tbl
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string);
        if url.contains("{version}") && version.is_none() {
            return Err(anyhow!(
                "registry '{name}': url 含 {{version}} 占位则必填 version(安装版本来源)"
            ));
        }
        let sha256 = tbl
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let strip = tbl
            .get("strip")
            .and_then(Value::as_integer)
            .unwrap_or(1)
            .max(0) as usize;
        let bin_rel = need("bin")?;
        let bin_name = tbl
            .get("bin-name")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string();
        let languages: Vec<String> = match tbl.get("languages") {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        let description = tbl
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let homepage = tbl
            .get("homepage")
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok(Spec {
            name: name.to_string(),
            kind,
            languages,
            bin_name,
            description,
            homepage,
            version,
            install: Install::Archive {
                url_template: url,
                sha256,
                strip,
                bin_rel,
            },
        })
    }
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
fn expand_template(t: &str, version: &str) -> String {
    t.replace("{version}", version)
        .replace("{os}", std::env::consts::OS)
        .replace("{arch}", std::env::consts::ARCH)
        .replace("{triple}", &rust_triple())
}

// ============================ 安装/升级/卸载 ============================

/// 安装:按配方类型分发。version 占位(v1:下载源为空 = 报"配方未配置下载源";
/// 测试/自定义源走 *_adhoc 注入)
pub fn install(name: &str, version: &str) -> Result<()> {
    let spec = registry::get(name).ok_or_else(|| anyhow!("unknown server '{name}'"))?;
    match &spec.install {
        registry::Install::Archive {
            url_template,
            sha256,
            strip,
            bin_rel,
        } => {
            if url_template.is_empty() {
                return Err(anyhow!("server '{name}': 下载源未配置(内置配方待录;或在 [server-manager.registry.{name}] 配 url)"));
            }
            let url = expand_template(url_template, version);
            if url.contains('{') {
                return Err(anyhow!(
                    "server '{name}': url_template 有未支持占位,展开失败 -> {url}(支持 {{version}}/{{os}}/{{arch}}/{{triple}};{{triple}} 需本平台在 rust_triple 表)"
                ));
            }
            let url = mirror_url(&url);
            install_adhoc(name, &spec.bin_name, bin_rel, *strip, &url, sha256)
        }
        registry::Install::Tool { cmd, args } => {
            if cmd.is_empty() {
                return Err(anyhow!("server '{name}': 下载源未配置(内置配方待录)"));
            }
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            install_tool_adhoc(name, &spec.bin_name, cmd, &arg_refs, version)
        }
    }
}

/// 安装内核(不依赖注册表;测试/自定义源走这里)
pub fn install_adhoc(
    name: &str,
    bin_name: &str,
    bin_rel: &str,
    strip: usize,
    url: &str,
    sha256: &str,
) -> Result<()> {
    if url.is_empty() {
        return Err(anyhow!("server '{name}': 下载源为空"));
    }
    let root = managed_root();
    std::fs::create_dir_all(&root)?;
    let tmp_archive = root.join(format!(".{name}.download"));
    let tmp_dir = root.join(format!(".{name}.tmp"));
    // 字节进度:worker 线程经 with_download_progress 挂的钩子在此 take 一次消费;
    // 未挂(主线程 :server 同步路径/测试) → 空闭包,与 M2a 行为一致
    match DL_PROGRESS.with(|c| c.borrow_mut().take()) {
        Some(mut cb) => download_to(url, &tmp_archive, |n, t| cb(n, t))?,
        None => download_to(url, &tmp_archive, |_, _| {})?,
    }
    if !sha256.is_empty() {
        let got = sha256_hex(&tmp_archive)?;
        if got != *sha256 {
            let _ = std::fs::remove_file(&tmp_archive);
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(anyhow!(
                "sha256 mismatch for {name}: got {got}, expected {sha256}"
            ));
        }
    }
    install_payload(&tmp_archive, &tmp_dir, strip, bin_rel)
        .with_context(|| format!("extract {name}"))?;
    let bin_in_tmp = tmp_dir.join(bin_rel);
    if !bin_in_tmp.is_file() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(anyhow!(
            "release 中找不到可执行 '{bin_rel}'({name} 安装失败)"
        ));
    }
    // 清旧目录 → rename(先落位再软链,避免链接指向临时目录)
    let dest = managed_dir(name);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&tmp_dir, &dest).with_context(|| format!("finalize {name}"))?;
    let bin_dir = root.join("bin");
    std::fs::create_dir_all(&bin_dir)?;
    let link = bin_dir.join(bin_name);
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(dest.join(bin_rel), &link).with_context(|| {
        format!(
            "symlink {} -> {}",
            link.display(),
            dest.join(bin_rel).display()
        )
    })?;
    let _ = std::fs::remove_file(&tmp_archive);
    Ok(())
}

/// tool 类安装内核(不依赖注册表;测试/自定义源走这里)。
/// 直接在受管目录执行 `cmd args`(args 中 {prefix} → 受管目录、{version} → version),
/// 产物须生成 <dir>/bin/<bin_name> 后软链 managed/bin——工具自烘焙路径写的是最终路径。
/// 升级先 mv 旧目录 → .bak;失败删新目录并回滚 .bak(§4:升级失败保留旧版);
pub fn install_tool_adhoc(
    name: &str,
    bin_name: &str,
    cmd: &str,
    args: &[&str],
    version: &str,
) -> Result<()> {
    let root = managed_root();
    std::fs::create_dir_all(&root)?;
    let dest = managed_dir(name);
    let bak = root.join(format!(".{name}.bak"));
    let _ = std::fs::remove_dir_all(&bak);
    let had_old = dest.exists();
    if had_old {
        std::fs::rename(&dest, &bak)?;
    }
    std::fs::create_dir_all(&dest)?;
    let rollback = |_err: anyhow::Error| -> anyhow::Error {
        let _ = std::fs::remove_dir_all(&dest);
        if had_old {
            let _ = std::fs::rename(&bak, &dest);
        }
        _err
    };
    let expanded: Vec<String> = args
        .iter()
        .map(|a| {
            a.replace("{prefix}", &dest.to_string_lossy())
                .replace("{version}", version)
        })
        .collect();
    let out = std::process::Command::new(cmd)
        .args(&expanded)
        .current_dir(&dest)
        .output()
        .map_err(|e| rollback(anyhow!("run '{cmd}' for {name}: {e}")))?;
    if !out.status.success() {
        let tail = String::from_utf8_lossy(&out.stderr)
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .join("\n");
        return Err(rollback(anyhow!(
            "tool '{name}' 安装失败({}):\n{tail}",
            out.status
        )));
    }
    let bin_in = dest.join("bin").join(bin_name);
    if !bin_in.is_file() {
        return Err(rollback(anyhow!(
            "tool '{name}' 未生成 {} (cmd 退出 0 但无产物)",
            bin_in.display()
        )));
    }
    let _ = std::fs::remove_dir_all(&bak);
    let bin_dir = root.join("bin");
    std::fs::create_dir_all(&bin_dir)?;
    let link = bin_dir.join(bin_name);
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&bin_in, &link)
        .with_context(|| format!("symlink {} -> {}", link.display(), bin_in.display()))?;
    Ok(())
}

/// 升级:先检测当前版本,重装最新(失败不动旧版;v1 无远端版本号接口,由命令层给 version)
pub fn update(name: &str, version: &str) -> Result<()> {
    if !is_installed(name) {
        return Err(anyhow!("server '{name}' 未安装,用 install"));
    }
    install(name, version)
}

/// 卸载:删受管目录 + bin 软链,并重写 languages.toml 标记段移除其条目
/// (languages.toml 失败时卸载仍完成,报错提示)
pub fn remove(name: &str) -> Result<()> {
    let spec = registry::get(name).ok_or_else(|| anyhow!("unknown server '{name}'"))?;
    remove_adhoc(name, &spec.bin_name)?;
    rewrite_languages_toml()
        .map(|_| ())
        .map_err(|e| anyhow!("server '{name}' 已卸载,但 languages.toml 未更新: {e:#}"))
}

/// 卸载内核(测试/adhoc)
pub fn remove_adhoc(name: &str, bin_name: &str) -> Result<()> {
    let _ = std::fs::remove_dir_all(managed_dir(name));
    let _ = std::fs::remove_file(managed_bin(bin_name));
    Ok(())
}

// ============================ 任务执行纯函数(线程可跑) ============================
//
// UI 层(面板/JS API)在独立线程跑 run_task 做 install/update/remove/unmanage,
// 主线程只消费 TaskOutcome + progress 事件——线程内不做任何 UI/editor 操作。

/// 单步任务的纯结果(命令层据此 set_status/set_error/刷新面板)
#[derive(Debug)]
pub struct TaskOutcome {
    pub ok: bool,
    pub msg: String,
}

/// 在独立线程执行单步任务(install/update/remove/unmanage)——纯函数:不做 UI,
/// 只重读 config 后执行并返回结果。progress(阶段, bytes, total):阶段事件为
/// "下载中"/"写入配置"等(bytes/total 恒 None);install 的真实字节进度不走本参数——
/// 由调用方(commands/server_tasks.rs worker)经 [with_download_progress] 挂线程局部
/// 钩子、install_adhoc 下载时消费后单独推事件(kind=progress)。
pub fn run_task(
    op: &str,
    name: &str,
    version: Option<&str>,
    mut progress: impl FnMut(&str, Option<u64>, Option<u64>) + Send,
) -> TaskOutcome {
    // 每个 op 先重读 config(保持与 :server 一致;lib 测试无 CONFIG_FILE → None=清空扩展)
    let _ = apply_server_config(disk_server_manager_table().as_ref());
    let out = (|| -> Result<String> {
        match op {
            "install" => {
                let spec = registry::get(name).ok_or_else(|| anyhow!("unknown server '{name}'"))?;
                match availability(name) {
                    Availability::Managed => return Err(anyhow!("'{name}' 已安装,用 update")),
                    Availability::Local(p) => {
                        return Err(anyhow!("'{name}' 本地已可用({}),无需安装", p.display()))
                    }
                    Availability::Missing => {}
                }
                if !spec.is_installable() {
                    return Err(anyhow!("'{name}' 下载源未配置(registry 缺 url)"));
                }
                let ver = match version.map(str::to_string).or_else(|| spec.version.clone()) {
                    Some(v) => v,
                    None => {
                        return Err(anyhow!(
                            "'{name}' 配方未给 version;传入 version 或 config 补"
                        ))
                    }
                };
                progress("下载中", None, None);
                install(name, &ver)?;
                progress("写入配置", None, None);
                rewrite_languages_toml()?;
                Ok(format!("installed '{name}' {ver}"))
            }
            "update" => {
                // 受管已装才可;版本取参或配方 version;下载→重写;否则引导
                let spec = registry::get(name).ok_or_else(|| anyhow!("unknown server '{name}'"))?;
                match availability(name) {
                    Availability::Managed => {}
                    Availability::Local(p) => {
                        return Err(anyhow!(
                            "'{name}' 由本地提供({}),不走 server manager 升级(受管安装才可 update)",
                            p.display()
                        ))
                    }
                    Availability::Missing => return Err(anyhow!("'{name}' 未安装,用 install")),
                }
                if !spec.is_installable() {
                    return Err(anyhow!("'{name}' 下载源未配置(registry 缺 url)"));
                }
                let ver = match version.map(str::to_string).or_else(|| spec.version.clone()) {
                    Some(v) => v,
                    None => {
                        return Err(anyhow!(
                            "'{name}' 配方未给 version;传入 version 或 config 补"
                        ))
                    }
                };
                progress("下载中", None, None);
                update(name, &ver)?;
                progress("写入配置", None, None);
                rewrite_languages_toml()?;
                Ok(format!("updated '{name}' {ver}"))
            }
            "remove" => {
                // 受管才可 remove(内部含 languages.toml 重写);local/missing 引导
                match availability(name) {
                    Availability::Managed => {
                        remove(name)?;
                        Ok(format!("removed '{name}'"))
                    }
                    Availability::Local(p) => Err(anyhow!(
                        "'{name}' 是本地工具({}),不能卸载;如需停用自动挂接用 unmanage",
                        p.display()
                    )),
                    Availability::Missing => Err(anyhow!("'{name}' 未安装")),
                }
            }
            "unmanage" => {
                // 仅本机已有(local)可停/恢复自动挂接;受管用 remove,缺失报错
                match availability(name) {
                    Availability::Managed => {
                        Err(anyhow!("server '{name}' 是受管安装,用 remove 卸载"))
                    }
                    Availability::Missing => Err(anyhow!("server '{name}' 未安装/不在 PATH")),
                    Availability::Local(_) => {
                        let on = !ignored_local().contains(&name.to_string());
                        set_ignored(name, on)?;
                        rewrite_languages_toml()?;
                        Ok(if on {
                            format!("'{name}' 已停用挂接")
                        } else {
                            format!("'{name}' 已恢复挂接")
                        })
                    }
                }
            }
            other => Err(anyhow!("unknown task op '{other}'")),
        }
    })();
    match out {
        Ok(msg) => TaskOutcome { ok: true, msg },
        Err(e) => TaskOutcome {
            ok: false,
            msg: format!("{e:#}"),
        },
    }
}

/// 供测试/开发:把本地目录作为"已装"(跳过下载),验证 bin 检测/版本
#[cfg(test)]
pub fn install_from_local_dir_for_test(name: &str, dir: &Path) -> Result<()> {
    let spec = registry::get(name).ok_or_else(|| anyhow!("unknown '{name}'"))?;
    let dest = spec.installed_dir();
    let _ = std::fs::remove_dir_all(&dest);
    copy_dir_recursive(dir, &dest)?;
    let bin_dir = managed_root().join("bin");
    std::fs::create_dir_all(&bin_dir)?;
    let link = bin_dir.join(&spec.bin_name);
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(dest.join(&spec.bin_name), &link)?;
    Ok(())
}

#[cfg(test)]
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir_recursive(&e.path(), &to)?;
        } else {
            std::fs::copy(e.path(), to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// 写一个含 bin 的 tar.gz 到 path(供 file:// 假源)
    fn make_tar_gz(path: &Path, bin_name: &str) -> Result<String> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let file = std::fs::File::create(path)?;
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut ar = tar::Builder::new(enc);
        // 顶层目录 v1.0.0/
        let top = format!("{bin_name}-1.0.0");
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Directory);
        h.set_mode(0o755);
        h.set_size(0);
        ar.append_data(
            &mut h,
            format!("{top}/"),
            std::io::Cursor::new(Vec::<u8>::new()),
        )?;
        // bin 脚本
        let script = format!("#!/bin/sh\necho '{bin_name} 1.0.0'\n");
        let mut hb = tar::Header::new_gnu();
        hb.set_size(script.len() as u64);
        hb.set_mode(0o755);
        ar.append_data(&mut hb, format!("{top}/{bin_name}"), script.as_bytes())?;
        let enc = ar.into_inner()?;
        let f = enc.finish()?;
        f.sync_all()?;
        drop(f);
        // sha256
        let bytes = std::fs::read(path)?;
        use sha2::{Digest, Sha256};
        let mut hh = Sha256::new();
        hh.update(&bytes);
        Ok(hh.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    /// EnvGuard 类测试改进程级环境变量,cargo 并行会互踩 → 串行化
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn tmp_root() -> PathBuf {
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("sm-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn mirror_url_rewrite_and_file_passthrough() {
        let _m = ENV_LOCK.lock().unwrap();
        apply_server_config(None).unwrap(); // 清掉可能残留的 mirror/ext
        assert_eq!(mirror_url("https://x/y"), "https://x/y", "无 mirror 原样");
        assert_eq!(
            mirror_url("file:///tmp/a.tar.gz"),
            "file:///tmp/a.tar.gz",
            "file:// 不受 mirror 影响"
        );
    }

    #[test]
    fn sha256_and_extract_roundtrip() {
        let root = tmp_root();
        let arc = root.join("rel.tar.gz");
        let sha = make_tar_gz(&arc, "demo-bin").unwrap();
        assert_eq!(sha.len(), 64);
        let out = root.join("x");
        extract_archive(&arc, &out, 1).unwrap();
        assert!(out.join("demo-bin").is_file(), "strip 顶层后 bin 在位");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn archive_install_update_remove_lifecycle() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let arc = root.join("src/rel.tar.gz");
        std::fs::create_dir_all(arc.parent().unwrap()).unwrap();
        let sha = make_tar_gz(&arc, "demo-bin").unwrap();
        let src = format!("file://{}", arc.display());
        // adhoc 安装(demo-bin:tar.gz 顶层目录,strip=1,bin_rel=demo-bin)
        install_adhoc("demo-bin", "demo-bin", "demo-bin", 1, &src, &sha).unwrap();
        assert!(managed_bin("demo-bin").exists(), "bin 软链已建");
        assert_eq!(
            detect_version(&managed_bin("demo-bin")).unwrap(),
            "demo-bin 1.0.0"
        );
        // 升级(换版本文件)→ 重装成功
        let arc2 = root.join("src/rel2.tar.gz");
        let sha2 = make_tar_gz(&arc2, "demo-bin").unwrap();
        install_adhoc(
            "demo-bin",
            "demo-bin",
            "demo-bin",
            1,
            &format!("file://{}", arc2.display()),
            &sha2,
        )
        .unwrap();
        assert_eq!(
            detect_version(&managed_bin("demo-bin")).unwrap(),
            "demo-bin 1.0.0"
        );
        // 校验失败 → 中止且不留半成品、旧版仍在
        let bad = "0".repeat(64);
        let r = install_adhoc("demo-bin", "demo-bin", "demo-bin", 1, &src, &bad);
        assert!(r.is_err(), "sha 不符应失败");
        assert!(managed_bin("demo-bin").exists(), "失败不影响旧版");
        // 卸载
        remove_adhoc("demo-bin", "demo-bin").unwrap();
        assert!(!managed_bin("demo-bin").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 写可执行的假脚本
    fn write_sh(root: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let p = root.join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// 假"工具"脚本(模拟 pip --target / cargo --root):$1=前缀目录,$2=版本;
    /// 在 $1/bin/ 生成 bin_name 可执行,内容里带上前缀与版本以验证占位替换。
    fn make_fake_tool(root: &Path, bin_name: &str) -> PathBuf {
        let script = format!(
            "#!/bin/sh\nset -e\nmkdir -p \"$1/bin\"\n\
             printf '#!/bin/sh\\necho {bin_name} 2.0.0 via %s %s\\n' \"$1\" \"$2\" > \"$1/bin/{bin_name}\"\n\
             chmod +x \"$1/bin/{bin_name}\"\n"
        );
        write_sh(root, "fake-tool.sh", &script)
    }

    #[test]
    fn tool_install_expands_placeholders_and_links_bin() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let fake = make_fake_tool(&root, "demo-tool");
        // args 含 {prefix}/{version} 占位——应被替换为受管目录与版本
        install_tool_adhoc(
            "demo-tool",
            "demo-tool",
            fake.to_string_lossy().as_ref(),
            &["{prefix}", "{version}"],
            "2.0.0",
        )
        .unwrap();
        let link = managed_bin("demo-tool");
        assert!(link.exists(), "bin 软链已建");
        let prefix = managed_dir("demo-tool");
        let content = std::fs::read_to_string(prefix.join("bin/demo-tool")).unwrap();
        assert!(
            content.contains(prefix.to_string_lossy().as_ref()),
            "{{prefix}} 已替换为受管目录"
        );
        assert!(content.contains("2.0.0"), "{{version}} 已替换");
        let v = detect_version(&link).unwrap();
        assert!(v.contains("2.0.0"), "detect_version 走软链读到新版本: {v}");
        // 幂等:重装(同 update 语义)后仍一致
        install_tool_adhoc(
            "demo-tool",
            "demo-tool",
            fake.to_string_lossy().as_ref(),
            &["{prefix}", "{version}"],
            "2.0.0",
        )
        .unwrap();
        assert!(managed_bin("demo-tool").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tool_failure_cleans_up_and_keeps_old_on_upgrade() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let ok_tool = make_fake_tool(&root, "demo-tool");
        // 先装成功
        install_tool_adhoc(
            "demo-tool",
            "demo-tool",
            ok_tool.to_string_lossy().as_ref(),
            &["{prefix}", "{version}"],
            "2.0.0",
        )
        .unwrap();
        // 升级失败 → 旧版保留(§4),staging 不残留
        let fail = write_sh(&root, "fail-tool.sh", "#!/bin/sh\necho boom >&2\nexit 3\n");
        let r = install_tool_adhoc(
            "demo-tool",
            "demo-tool",
            fail.to_string_lossy().as_ref(),
            &["{prefix}"],
            "9.9",
        );
        assert!(r.is_err(), "非零退出应失败");
        assert!(
            managed_dir("demo-tool").exists(),
            "升级失败保留旧版受管目录"
        );
        let v = detect_version(&managed_bin("demo-tool")).unwrap();
        assert!(
            v.contains("2.0.0") && !v.contains("9.9"),
            "旧版本仍可检测: {v}"
        );
        assert!(
            !managed_root().join(".demo-tool.bak").exists(),
            "成功后 .bak 不残留"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tool_success_without_bin_fails_cleanly() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let fake = write_sh(&root, "noop-tool.sh", "#!/bin/sh\nmkdir -p \"$1\"\n");
        let r = install_tool_adhoc(
            "demo-tool",
            "demo-tool",
            fake.to_string_lossy().as_ref(),
            &["{prefix}"],
            "1.0",
        );
        let msg = format!("{:?}", r);
        assert!(r.is_err() && msg.contains("未生成"), "缺 bin 应报错: {msg}");
        assert!(!managed_dir("demo-tool").exists(), "缺 bin 也清理");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- languages.toml 标记段 ----

    /// 双 env guard:SM_MANAGED_DIR + SM_LANGS_TOML → tmp root;返回 languages 路径
    fn sm_env(root: &Path) -> (PathBuf, EnvGuard, EnvGuard, EnvGuard) {
        let langs = root.join("languages.toml");
        let ls = langs.to_string_lossy().into_owned();
        let m = root.to_string_lossy().into_owned();
        (
            langs,
            EnvGuard::new("SM_MANAGED_DIR", m),
            EnvGuard::new("SM_LANGS_TOML", ls),
            EnvGuard::new("SM_PATH", String::new()), // 禁用宿主 PATH 本地检测(hermetic)
        )
    }

    /// 假装某 registry 配方已安装(受管目录 + bin 软链)
    fn install_fake(name: &str) {
        let root = PathBuf::from(std::env::var("SM_MANAGED_DIR").unwrap());
        let src = root.join("src").join(name);
        std::fs::create_dir_all(&src).unwrap();
        write_sh(&src, name, &format!("#!/bin/sh\necho {name} 1.0\n"));
        install_from_local_dir_for_test(name, &src).unwrap();
    }

    fn count(text: &str, sub: &str) -> usize {
        text.matches(sub).count()
    }

    #[test]
    fn langs_rewrite_from_empty_builds_section_idempotent() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let (_langs, _a, _b, _gp) = sm_env(&root);
        install_fake("rust-analyzer");
        let rw = rewrite_languages_toml().unwrap();
        assert!(rw.written);
        assert_eq!(rw.servers, vec!["rust-analyzer"]);
        assert!(rw.conflicts.is_empty());
        let text = std::fs::read_to_string(&rw.path).unwrap();
        assert_eq!(count(&text, "# >>> helix-managed"), 1);
        assert_eq!(count(&text, "# <<< helix-managed"), 1);
        assert!(text.contains("[language-server.rust-analyzer]"));
        assert!(text.contains("command = \"/"));
        assert!(
            text.contains("[[language]]\nname = \"rust\"\nlanguage-servers = [\"rust-analyzer\"]")
        );
        toml::from_str::<toml::Value>(&text).expect("生成文本可解析");
        // 幂等:再次重写文本一致
        let rw2 = rewrite_languages_toml().unwrap();
        assert_eq!(text, std::fs::read_to_string(&rw2.path).unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn langs_user_content_preserved_and_section_replaced() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let (langs, _ga, _gb, _gp) = sm_env(&root);
        let existing = "# 用户注释\n[[language]]\nname = \"haskell\"\nscope = \"source.haskell\"\n\n\
            # >>> helix-managed\n[language-server.rust-analyzer]\ncommand = \"/old/stale/path\"\n\n\
            [[language]]\nname = \"rust\"\nlanguage-servers = [\"rust-analyzer\"]\n# <<< helix-managed\n\n\
            [[language]]\nname = \"toml\"\ncomment-token = \"#\"\n";
        std::fs::write(&langs, existing).unwrap();
        install_fake("rust-analyzer");
        let rw = rewrite_languages_toml().unwrap();
        let text = std::fs::read_to_string(&rw.path).unwrap();
        // 用户标记外内容逐字节保留
        assert!(text.contains("# 用户注释"));
        assert!(text.contains("scope = \"source.haskell\""));
        assert!(text.contains("comment-token = \"#\""));
        // 旧段被整体替换,无残留
        assert!(!text.contains("/old/stale/path"));
        assert_eq!(count(&text, "# >>> helix-managed"), 1);
        assert_eq!(count(&text, "# <<< helix-managed"), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn langs_conflict_user_defined_language_skipped_with_hint() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let (langs, _ga, _gb, _gp) = sm_env(&root);
        let existing = "[[language]]\nname = \"rust\"\nroots = [\"my-own-root\"]\n";
        std::fs::write(&langs, existing).unwrap();
        install_fake("rust-analyzer");
        let rw = rewrite_languages_toml().unwrap();
        let text = std::fs::read_to_string(&rw.path).unwrap();
        assert_eq!(
            count(&text, "name = \"rust\""),
            1,
            "用户已自定义 rust → 不再生成同名条目"
        );
        assert!(
            text.contains("[language-server.rust-analyzer]"),
            "server 表仍生成供手动引用"
        );
        assert_eq!(rw.conflicts.len(), 1);
        assert!(rw.conflicts[0].contains("rust-analyzer"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn langs_dap_debugpy_emits_debugger_block() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let (_langs, _a, _b, _gp) = sm_env(&root);
        install_fake("debugpy");
        let rw = rewrite_languages_toml().unwrap();
        assert!(rw.servers.is_empty(), "Dap 不进 language-server 表");
        let text = std::fs::read_to_string(&rw.path).unwrap();
        assert!(text.contains("[[language]]\nname = \"python\""));
        assert!(text.contains("debugger = { name = \"debugpy\""));
        assert!(text.contains("transport = \"stdio\""));
        assert!(text.contains("templates = [{ name = \"launch\""));
        toml::from_str::<toml::Value>(&text).expect("生成文本可解析");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn langs_bad_user_toml_and_unbalanced_marker_abort() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let (langs, _ga, _gb, _gp) = sm_env(&root);
        let p = &langs;
        // 语法坏(无标记)→ 报错不改写
        std::fs::write(p, "this is not toml {{{\n[[language]\n").unwrap();
        let err = rewrite_languages_toml().unwrap_err().to_string();
        assert!(err.contains("语法错误"), "{err}");
        assert_eq!(
            std::fs::read_to_string(p).unwrap(),
            "this is not toml {{{\n[[language]\n"
        );
        // 只有单边标记 → 报错不改写
        std::fs::write(p, "# >>> helix-managed\nx = 1\n").unwrap();
        let err = rewrite_languages_toml().unwrap_err().to_string();
        assert!(err.contains("不完整"), "{err}");
        assert_eq!(
            std::fs::read_to_string(p).unwrap(),
            "# >>> helix-managed\nx = 1\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn langs_remove_clears_entry() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let (_langs, _a, _b, _gp) = sm_env(&root);
        install_fake("rust-analyzer");
        rewrite_languages_toml().unwrap();
        remove("rust-analyzer").unwrap();
        assert!(!managed_dir("rust-analyzer").exists());
        let text = std::fs::read_to_string(root.join("languages.toml")).unwrap();
        assert!(!text.contains("rust-analyzer"), "条目已清:{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn langs_user_own_server_table_not_duplicated() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let (langs, _ga, _gb, _gp) = sm_env(&root);
        let existing = "[language-server.rust-analyzer]\ncommand = \"/mine/custom/ra\"\n";
        std::fs::write(&langs, existing).unwrap();
        install_fake("rust-analyzer");
        let rw = rewrite_languages_toml().unwrap();
        let text = std::fs::read_to_string(&rw.path).unwrap();
        assert_eq!(
            count(&text, "[language-server.rust-analyzer]"),
            1,
            "不重复表头"
        );
        assert!(text.contains("/mine/custom/ra"), "用户 command 保留");
        assert!(rw.servers.is_empty(), "用户自带同名 → 不再生成表");
        assert!(text.contains("language-servers = [\"rust-analyzer\"]"));
        toml::from_str::<toml::Value>(&text).expect("可解析");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 关键合并语义验证:生成的 languages.toml 经 helix-loader 真实合并逻辑
    /// (merge_toml_values depth 3 → Configuration)后,rust 挂上 rust-analyzer 且
    /// command 指向受管绝对路径;python 获得 debugpy debugger。
    #[test]
    fn langs_loader_merge_resolves_managed_servers() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let (_langs, _a, _b, _gp) = sm_env(&root);
        install_fake("rust-analyzer");
        install_fake("debugpy");
        rewrite_languages_toml().unwrap();
        let file_text = std::fs::read_to_string(root.join("languages.toml")).unwrap();
        let user: toml::Value = toml::from_str(&file_text).unwrap();
        let merged =
            helix_loader::merge_toml_values(helix_loader::config::default_lang_config(), user, 3);
        let conf: helix_core::syntax::config::Configuration = merged.try_into().unwrap();
        let ra = conf
            .language_server
            .get("rust-analyzer")
            .expect("rust-analyzer 表存在");
        assert_eq!(
            ra.command,
            managed_bin("rust-analyzer").to_string_lossy(),
            "command 被覆写为受管绝对路径"
        );
        let rust = conf
            .language
            .iter()
            .find(|l| l.language_id == "rust")
            .expect("builtin rust 条目与生成条目合并");
        assert!(rust
            .language_servers
            .iter()
            .any(|f| f.name == "rust-analyzer"));
        let py = conf
            .language
            .iter()
            .find(|l| l.language_id == "python")
            .expect("python 语言存在");
        let dap = py.debugger.as_ref().expect("python 获得 debugger");
        assert_eq!(dap.name, "debugpy");
        assert_eq!(
            dap.command,
            managed_bin("debugpy").to_string_lossy(),
            "debugger command 为受管绝对路径"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- T4:config 段(mirror + registry.<name> 扩展配方) ----

    #[test]
    fn config_mirror_and_ext_recipe_install_loop() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let (langs, _ga, _gb, _gp) = sm_env(&root);
        // file:// 假 release(tar.gz 顶层 my-ls-1.0.0/,strip=1)
        let arc = root.join("rel.tar.gz");
        let sha = make_tar_gz(&arc, "my-ls").unwrap();
        let cfg = format!(
            "[server-manager]\nmirror = \"https://mirror.example/\"\n\
             [server-manager.registry.my-ls]\n\
             url = \"{src}\"\nsha256 = \"{sha}\"\nstrip = 1\nbin = \"my-ls\"\n\
             languages = [\"rust\"]\nkind = \"lsp\"\nversion = \"1.0.0\"\n",
            src = format!("file://{}", arc.display())
        );
        let tbl: toml::Table = toml::from_str(&cfg).unwrap();
        let sm = tbl
            .get("server-manager")
            .unwrap()
            .as_table()
            .unwrap()
            .clone();
        apply_server_config(Some(&sm)).unwrap();
        // mirror:https 前缀改写,file:// 原样
        assert_eq!(
            mirror_url("https://x/y"),
            "https://mirror.example/https://x/y"
        );
        assert_eq!(mirror_url("file:///a"), "file:///a");
        // ext 配方进注册表;内置不受影响
        let spec = registry::get("my-ls").expect("ext 配方可查");
        assert!(spec.is_installable() && spec.version.as_deref() == Some("1.0.0"));
        assert!(registry::get("rust-analyzer").is_some(), "内置仍在");
        assert!(registry::get("nope").is_none());
        // 经 ext 源真实安装(版本默认取配方 version)
        install("my-ls", spec.version.as_deref().unwrap()).unwrap();
        assert!(managed_bin("my-ls").exists(), "bin 软链已建");
        // languages.toml 重建含 my-ls 条目
        let rw = rewrite_languages_toml().unwrap();
        assert!(rw.servers.iter().any(|n| n == "my-ls"));
        let text = std::fs::read_to_string(&langs).unwrap();
        assert!(text.contains("[language-server.my-ls]"));
        assert!(text.contains("language-servers = [\"my-ls\"]"));
        // registry 路径 remove:清目录 + 重写标记段
        remove("my-ls").unwrap();
        assert!(!managed_bin("my-ls").exists());
        let text = std::fs::read_to_string(&langs).unwrap();
        assert!(!text.contains("my-ls"), "条目已清:{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn config_ext_recipe_validation_errors() {
        let _m = ENV_LOCK.lock().unwrap();
        // url 含 {version} 但缺 version 字段
        let bad: toml::Table =
            toml::from_str("url = \"https://x/{version}/y.tar.gz\"\nbin = \"x\"\n").unwrap();
        let e = registry::parse_ext_recipe("demo", &bad)
            .unwrap_err()
            .to_string();
        assert!(e.contains("version"), "{e}");
        // 缺 bin
        let bad2: toml::Table = toml::from_str("url = \"https://x/v1/y.tar.gz\"\n").unwrap();
        let e = registry::parse_ext_recipe("demo", &bad2)
            .unwrap_err()
            .to_string();
        assert!(e.contains("'bin'"), "{e}");
        // 未知 kind
        let bad3: toml::Table =
            toml::from_str("url = \"https://x/{version}/y.tar.gz\"\nbin = \"y\"\nkind = \"wat\"\n")
                .unwrap();
        let e = registry::parse_ext_recipe("demo", &bad3)
            .unwrap_err()
            .to_string();
        assert!(e.contains("wat"), "{e}");
        // 合法:默认值(strip=1/kind=lsp/bin-name=name)
        let ok: toml::Table = toml::from_str(
            "url = \"https://x/{version}/y.tar.gz\"\nbin = \"z\"\nversion = \"1.0\"\n",
        )
        .unwrap();
        let spec = registry::parse_ext_recipe("demo", &ok).unwrap();
        assert!(matches!(spec.kind, registry::Kind::Lsp));
        assert_eq!(spec.bin_name, "demo");
        assert_eq!(spec.languages.len(), 0);
        assert_eq!(spec.version.as_deref(), Some("1.0"));
        let _m2 = _m;
    }

    // ---- 单文件 gzip release(rust-analyzer 类)+ token 展开 ----

    /// 写单文件 gzip 假 release:内容 = bin 脚本;返回 sha256
    fn make_plain_gz(path: &Path) -> Result<String> {
        use sha2::{Digest, Sha256};
        std::fs::create_dir_all(path.parent().unwrap())?;
        let script = b"#!/bin/sh\necho plain-gz 3.2.1\n";
        let enc = flate2::write::GzEncoder::new(
            std::fs::File::create(path)?,
            flate2::Compression::default(),
        );
        let mut w = std::io::BufWriter::new(enc);
        w.write_all(script)?;
        let enc = w.into_inner().unwrap().finish()?;
        enc.sync_all()?;
        let mut h = Sha256::new();
        h.update(&std::fs::read(path)?);
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    #[test]
    fn plain_gz_single_binary_installs_and_runs() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let arc = root.join("ra.gz");
        let sha = make_plain_gz(&arc).unwrap();
        install_adhoc(
            "plain-gz-demo",
            "plain-gz-demo",
            "plain-gz-demo",
            0,
            &format!("file://{}", arc.display()),
            &sha,
        )
        .unwrap();
        let link = managed_bin("plain-gz-demo");
        assert!(link.exists(), "单文件 gz 落位并软链");
        let v = detect_version(&link).unwrap();
        assert!(v.contains("3.2.1"), "解码产物可执行: {v}");
        remove_adhoc("plain-gz-demo", "plain-gz-demo").unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn template_tokens_expand_os_arch_triple() {
        let v = expand_template("{version}/{os}/{arch}/{triple}", "2024-09-16");
        assert!(!v.contains('{'), "占位全展开: {v}");
        assert!(v.contains("2024-09-16"));
        assert!(v.contains(std::env::consts::OS));
        assert!(v.contains(std::env::consts::ARCH));
        assert!(!rust_triple().is_empty(), "本机平台应有 triple");
        assert!(v.contains(&rust_triple()));
        // mirror_url 仍走 file 原样 / https 前缀
    }

    // ---- 本地 PATH 已装识别(local)+ 忽略名单 ----

    /// 在 tmp bin 目录放一个可执行假工具(用于 SM_PATH 注入)
    fn fake_local_bin(root: &Path, name: &str) -> PathBuf {
        let dir = root.join("localbin");
        std::fs::create_dir_all(&dir).unwrap();
        write_sh(&dir, name, &format!("#!/bin/sh\necho {name} 9.9\n"));
        dir
    }

    #[test]
    fn availability_prefers_managed_over_local() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let (_langs, _a, _b, _p) = sm_env(&root);
        let bin_dir = fake_local_bin(&root, "rust-analyzer");
        let _bin = EnvGuard::new("SM_PATH", bin_dir.to_string_lossy().into_owned());
        // 仅本地
        assert_eq!(
            availability("rust-analyzer"),
            Availability::Local(bin_dir.join("rust-analyzer"))
        );
        // 受管优先
        install_fake("rust-analyzer");
        assert_eq!(availability("rust-analyzer"), Availability::Managed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn local_server_writes_path_command_and_ignore_removes_it() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let (langs, _a, _b, _p) = sm_env(&root);
        let bin_dir = fake_local_bin(&root, "rust-analyzer");
        let _bin = EnvGuard::new("SM_PATH", bin_dir.to_string_lossy().into_owned());
        // languages.toml 对本地 server 写绝对 PATH command 并挂接 rust
        let rw = rewrite_languages_toml().unwrap();
        let text = std::fs::read_to_string(&langs).unwrap();
        assert!(text.contains("[language-server.rust-analyzer]"), "{text}");
        let cmd = bin_dir.join("rust-analyzer").to_string_lossy().into_owned();
        assert!(
            text.contains(&format!("command = \"{cmd}\"")),
            "本地 server command=PATH 绝对路径:\n{text}"
        );
        assert!(rw.servers.is_empty(), "local 不计入受管报告");
        // unmanage(忽略)→ 重建后无该 server;恢复后回来
        set_ignored("rust-analyzer", true).unwrap();
        assert!(ignored_local().contains(&"rust-analyzer".to_string()));
        rewrite_languages_toml().unwrap();
        let text = std::fs::read_to_string(&langs).unwrap();
        assert!(!text.contains("rust-analyzer"), "忽略后不再挂接:\n{text}");
        set_ignored("rust-analyzer", false).unwrap();
        rewrite_languages_toml().unwrap();
        let text = std::fs::read_to_string(&langs).unwrap();
        assert!(text.contains("[language-server.rust-analyzer]"), "恢复挂接");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn spec_description_homepage_and_upgradable() {
        // parse_ext_recipe 支持 description/homepage
        let ok: toml::Table = toml::from_str(
            "url = \"file:///v{version}/x.tar.gz\"\nbin = \"z\"\nversion = \"1.0\"\n\
             description = \"假工具\"\nhomepage = \"https://example.com\"\n",
        )
        .unwrap();
        let spec = registry::parse_ext_recipe("demo", &ok).unwrap();
        assert_eq!(spec.description, "假工具");
        assert_eq!(spec.homepage.as_deref(), Some("https://example.com"));
        // 可升级判定:受管已装 && version 有值 && != 检测版本 → true
        let _ = spec;
    }

    #[test]
    fn builtin_specs_all_have_description_and_homepage() {
        // M5:内置 7 配方补展示数据(arsenal 行/信息弹窗显示)——缺描述/主页=遗漏
        let specs = registry::builtin_specs();
        assert_eq!(specs.len(), 7, "内置配方数变更需同步测试");
        for s in &specs {
            assert!(
                !s.description.is_empty(),
                "{} 缺 description(M5 应补中文一句)",
                s.name
            );
            assert!(
                s.homepage
                    .as_deref()
                    .is_some_and(|h| h.starts_with("https://")),
                "{} 缺 homepage 官方链接",
                s.name
            );
        }
    }

    #[test]
    fn is_upgradable_contains_semantics() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        // 配方 version=1.0.0;受管装 bin 输出 "my-ls 1.0.0"(detected 含 recipe 版本 → 同版)
        let ok: toml::Table = toml::from_str(
            "url = \"https://x/v{version}/my-ls.tar.gz\"\nbin = \"my-ls\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let spec = registry::parse_ext_recipe("my-ls", &ok).unwrap();
        let arc = root.join("my-ls.tar.gz");
        let sha = make_tar_gz(&arc, "my-ls").unwrap();
        install_adhoc(
            "my-ls",
            "my-ls",
            "my-ls",
            1,
            &format!("file://{}", arc.display()),
            &sha,
        )
        .unwrap();
        assert_eq!(
            detect_version(&managed_bin("my-ls")).unwrap(),
            "my-ls 1.0.0"
        );
        assert!(
            !is_upgradable(&spec),
            "detected 含 recipe version → 同版不可升级"
        );
        // 换装:bin 检测输出变为 "my-ls 9.9"(detected 不含 recipe 版本 → 可升级)
        let bin = managed_bin("my-ls");
        std::fs::write(&bin, "#!/bin/sh\necho my-ls 9.9\n").unwrap();
        assert_eq!(detect_version(&bin).unwrap(), "my-ls 9.9");
        assert!(
            is_upgradable(&spec),
            "detected 不含 recipe version → 可升级"
        );
        // version=None → 恒 false
        let nover: toml::Table =
            toml::from_str("url = \"https://x/v1/my-ls.tar.gz\"\nbin = \"my-ls\"\n").unwrap();
        let spec2 = registry::parse_ext_recipe("my-ls", &nover).unwrap();
        assert!(!is_upgradable(&spec2), "recipe 无 version → 不可升级");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn needs_version_archive_template_semantics() {
        // 内置活配方 rust-analyzer:url 含 {version} 且 recipe 无固定 version → 需显式版本
        let ra = registry::all()
            .into_iter()
            .find(|s| s.name == "rust-analyzer")
            .unwrap();
        assert!(
            needs_version(&ra),
            "内置 {{version}} 占位 + 无 version → needs_version"
        );
        // 惰性配方(url 空)与 Tool 类(debugpy/black) → false
        let gopls = registry::all()
            .into_iter()
            .find(|s| s.name == "gopls")
            .unwrap();
        assert!(!needs_version(&gopls), "url 空(惰性)→ false");
        let debugpy = registry::all()
            .into_iter()
            .find(|s| s.name == "debugpy")
            .unwrap();
        assert!(!needs_version(&debugpy), "Tool 安装 → false");
        // 手构 Spec 补边界:url 含 {version} + version 固定 → false;url 无 {version} → false
        let mk = |url: &str, version: Option<String>| registry::Spec {
            name: "t".into(),
            kind: registry::Kind::Lsp,
            languages: vec![],
            bin_name: "t".into(),
            description: String::new(),
            homepage: None,
            install: registry::Install::Archive {
                url_template: url.into(),
                sha256: String::new(),
                strip: 1,
                bin_rel: "t".into(),
            },
            version,
        };
        assert!(
            !needs_version(&mk("https://x/v{version}/t.tar.gz", Some("1.0".into()))),
            "version 固定 → false"
        );
        assert!(
            !needs_version(&mk("https://x/v1.0/t.tar.gz", None)),
            "url 无 {{version}} 占位 → false"
        );
    }

    // ---- M2a:进度聚合器 / download_to 进度 / run_task 纯函数 ----

    #[test]
    fn progress_aggregator_monotonic_throttled() {
        // 聚合器:回调每 ≥throttle_bytes 或 ≥interval 触发;结束 flush 到 100%
        let mut hits = Vec::new();
        let mut cb = |n, total| {
            hits.push((n, total));
        };
        let mut agg = ProgressAgg::new(8, std::time::Duration::from_millis(100), &mut cb);
        for _ in 0..50u64 {
            agg.add(1, Some(100)); // 共 50 字节,节流 8 → 约 6 次
        }
        agg.finish(50, Some(100));
        assert!(hits.len() >= 2, "至少节流若干次+末尾 flush: {hits:?}");
        let last = hits.last().unwrap();
        assert_eq!(last.0, 50, "末尾 flush 到总量");
        // 序列单调
        let ns: Vec<u64> = hits.iter().map(|(n, _)| *n).collect();
        assert!(ns.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn download_to_file_reports_progress_once_with_size() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let src = root.join("payload.bin");
        let body = vec![0xabu8; 1024 * 5]; // 5KiB
        std::fs::write(&src, &body).unwrap();
        let dest = root.join("out.bin");
        let mut hits = Vec::new();
        download_to(&format!("file://{}", src.display()), &dest, |n, total| {
            hits.push((n, total));
        })
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body, "内容已复制");
        assert_eq!(
            hits,
            vec![(body.len() as u64, Some(body.len() as u64))],
            "file:// 下载完成回调一次(总大小)"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// run_task 分支冒烟(hermetic)。lib 测试无 CONFIG_FILE(config_file_opt=None),
    /// run_task 开头重读磁盘 config 会把 registry 扩展清空——install 走 file:// 假源的
    /// ok:true 全程(含阶段事件)需磁盘 config 注入配方,留给任务 5 编辑器集成;
    /// 此处覆盖可离线判定的 ok/err 分支(均不触网)。
    #[test]
    fn run_task_branches_ok_and_err_hermetic() {
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let (_langs, _a, _b, _p) = sm_env(&root);
        // 未知 op / 未知 server / unmanage 缺失 → ok:false 文案
        let out = run_task("bogus", "x", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("unknown task op"), "{out:?}");
        let out = run_task("install", "ghost", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("unknown server"), "{out:?}");
        let out = run_task("unmanage", "ghost", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("未安装/不在 PATH"), "{out:?}");
        // install 命中受管 → 引导 update
        install_fake("rust-analyzer");
        let out = run_task("install", "rust-analyzer", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("已安装,用 update"), "{out:?}");
        // update 受管但配方无 version → 引导显式 version(不触网)
        let out = run_task("update", "rust-analyzer", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("配方未给 version"), "{out:?}");
        // remove 受管 → ok:true 且清理
        let out = run_task("remove", "rust-analyzer", None, |_, _, _| {});
        assert!(out.ok && out.msg.contains("removed"), "{out:?}");
        assert!(!managed_bin("rust-analyzer").exists());
        // update 缺失 → 引导 install
        let out = run_task("update", "rust-analyzer", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("未安装"), "{out:?}");
        // unmanage 本地 PATH 工具 → 停用挂接,再跑恢复
        let bin_dir = fake_local_bin(&root, "rust-analyzer");
        let _bin = EnvGuard::new("SM_PATH", bin_dir.to_string_lossy().into_owned());
        let out = run_task("unmanage", "rust-analyzer", None, |_, _, _| {});
        assert!(out.ok && out.msg.contains("已停用挂接"), "{out:?}");
        assert!(ignored_local().contains(&"rust-analyzer".to_string()));
        let out = run_task("unmanage", "rust-analyzer", None, |_, _, _| {});
        assert!(out.ok && out.msg.contains("已恢复挂接"), "{out:?}");
        // remove 本地工具 → 指引 unmanage
        let out = run_task("remove", "rust-analyzer", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("unmanage"), "{out:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    struct EnvGuard(&'static str, String);
    impl EnvGuard {
        fn new(key: &'static str, val: String) -> Self {
            std::env::set_var(key, &val);
            EnvGuard(key, val)
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }

    /// clear API:未达下载点的批次(remove/unmanage/下载前报错)经 clear 后当前线程
    /// 钩子必清——主线程后续同步 :server install 不会消费到残留幽灵钩子。
    #[test]
    fn clear_download_progress_drops_hook() {
        with_download_progress(Box::new(|_, _| {}));
        assert!(download_progress_hook_set(), "钩子已挂");
        clear_download_progress();
        assert!(
            !download_progress_hook_set(),
            "clear 后当前线程钩子已清(无残留)"
        );
    }

    #[test]
    fn download_progress_hook_consumed_by_install_adhoc() {
        // DL_PROGRESS 线程局部钩子:install_adhoc 下载前挂载,take 一次消费并收到字节回调
        let _m = ENV_LOCK.lock().unwrap();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let arc = root.join("src/rel.tar.gz");
        std::fs::create_dir_all(arc.parent().unwrap()).unwrap();
        let sha = make_tar_gz(&arc, "demo-bin").unwrap();
        let src = format!("file://{}", arc.display());
        let size = std::fs::metadata(&arc).unwrap().len();
        let hits: std::sync::Arc<std::sync::Mutex<Vec<(u64, Option<u64>)>>> =
            std::sync::Arc::default();
        let hits2 = hits.clone();
        with_download_progress(Box::new(move |n, t| hits2.lock().unwrap().push((n, t))));
        install_adhoc("demo-bin", "demo-bin", "demo-bin", 1, &src, &sha).unwrap();
        assert!(managed_bin("demo-bin").exists());
        assert_eq!(
            hits.lock().unwrap().as_slice(),
            &[(size, Some(size))],
            "file:// 复制完成回调一次(总大小),钩子已消费"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
