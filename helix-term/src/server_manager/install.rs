//! 安装/升级/卸载与任务执行(含只给测试用的本地目录安装)。
//!
//! 从 `server_manager/mod.rs` 抽出(G8b)。模块间不用 glob(`use super::*`),
//! 而是按编译器提示显式 `use super::<mod>::{…}` —— 双向 glob 会构成 glob 环,
//! 解析器会把环当成空集合。

use std::path::{Path, PathBuf};

use super::archive::install_payload;
use super::download::{download_to, mirror_url, sha256_hex, symlink_file, DL_PROGRESS};
use super::langs_toml::{
    apply_server_config, disk_server_manager_table, ignored_local, rewrite_languages_toml,
    set_ignored,
};
use super::registry;
use super::version::expand_template;
use super::Availability;
use anyhow::{anyhow, Context as _, Result};

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
/// 按目录一次性 read_dir 缓存:41+ 配方 × 每 PATH 目录逐个 stat 会让 rows 快照在
/// 含慢速/不可达挂载的 PATH 上卡数秒(arsenal 打开假死主因之一);缓存后每次查询
/// O(目录数)的 HashSet 命中 + 仅对命中项 stat 确认是文件。
pub(crate) fn path_in_path(bin: &str) -> Option<PathBuf> {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex, OnceLock};
    static DIR_CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<HashSet<String>>>>> = OnceLock::new();
    fn dir_names(dir: &Path) -> Arc<HashSet<String>> {
        let cache = DIR_CACHE.get_or_init(Default::default);
        if let Ok(guard) = cache.lock() {
            if let Some(names) = guard.get(dir) {
                return Arc::clone(names);
            }
        }
        let mut set = HashSet::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if let Some(n) = e.file_name().to_str() {
                    set.insert(n.to_string());
                }
            }
        }
        let names = Arc::new(set);
        if let Ok(mut guard) = cache.lock() {
            guard.insert(dir.to_path_buf(), Arc::clone(&names));
        }
        names
    }
    let managed_bin_dir = managed_root().join("bin");
    let path = std::env::var("SM_PATH")
        .or_else(|_| std::env::var("PATH"))
        .ok()?;
    for dir in std::env::split_paths(&path) {
        if dir == managed_bin_dir {
            continue;
        }
        if dir_names(&dir).contains(bin) && dir.join(bin).is_file() {
            return Some(dir.join(bin));
        }
    }
    None
}

/// 安装:按配方类型分发。version 占位(v1:下载源为空 = 报"配方未配置下载源";
/// 测试/自定义源走 *_adhoc 注入)
pub fn install(name: &str, version: &str) -> Result<()> {
    let spec = registry::get(name).ok_or_else(|| anyhow!("unknown server '{name}'"))?;
    install_spec(&spec, version)
}

/// 用**已解析**的配方安装。
///
/// 为什么单独拆出来:注册表是**进程级**的(`EXT` 缓存 + `SM_*` 配置),并行测试里
/// 另一条用例调用 `apply_server_config` 就能在"查到配方"与"用配方安装"之间把它换掉
/// (D8 实测:`install("asset-ra")` 间歇报 unknown server)。拿着 `&Spec` 安装就没有这个窗口。
pub(crate) fn install_spec(spec: &registry::Spec, version: &str) -> Result<()> {
    let name = spec.name.as_str();
    match &spec.install {
        registry::Install::Archive {
            url_template,
            assets,
            sha256,
            strip,
            bin_rel,
        } => {
            // 逐平台资产优先(Mason 形状);无匹配则回落单模板简写
            let picked = registry::pick_asset(assets);
            let url_src = picked
                .map(|a| a.url.as_str())
                .unwrap_or(url_template.as_str());
            let bin_eff = picked
                .map(|a| {
                    if a.bin_rel.is_empty() {
                        bin_rel.as_str()
                    } else {
                        a.bin_rel.as_str()
                    }
                })
                .unwrap_or(bin_rel.as_str());
            if url_src.is_empty() {
                return Err(anyhow!("server '{name}': 下载源未配置(内置配方待录;或在 [server-manager.registry.{name}] 配 url)"));
            }
            let url = expand_template(url_src, version);
            if url.contains('{') {
                return Err(anyhow!(
                    "server '{name}': url_template 有未支持占位,展开失败 -> {url}(支持 {{version}}/{{os}}/{{arch}}/{{triple}};{{triple}} 需本平台在 rust_triple 表)"
                ));
            }
            let url = mirror_url(&url);
            install_adhoc(name, &spec.bin_name, bin_eff, *strip, &url, sha256)
        }
        registry::Install::Tool { cmd, args } => {
            if cmd.is_empty() {
                return Err(anyhow!("server '{name}': 下载源未配置(内置配方待录)"));
            }
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            install_tool_adhoc(name, &spec.bin_name, cmd, &arg_refs, version)
        }
    }?;
    // 记下"我装的是哪个版本"（动态版本配方靠它判断能不能升级）
    record_installed_version(name, version);
    Ok(())
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
        Some(cb) => download_to(url, &tmp_archive, cb)?,
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
    // **统一补执行位**:tar 解包会带上 mode、单文件 gzip 分支已 chmod,但 **zip 解包不带
    // unix mode**(archive.rs 只 `File::create` 写字节)⇒ 之前 zip 类配方(terraform-ls /
    // clojure-lsp / elixir-ls)装出来的 bin 是 0644,软链过去 `--version` 直接 permission denied。
    // 放在这里而不是 archive 里:三路(result 格式)共用一处,漏一路也不会重演。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&bin_in_tmp, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("chmod +x '{bin_rel}'({name})"))?;
    }
    // 清旧目录 → rename(先落位再软链,避免链接指向临时目录)
    let dest = managed_dir(name);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&tmp_dir, &dest).with_context(|| format!("finalize {name}"))?;
    let bin_dir = root.join("bin");
    std::fs::create_dir_all(&bin_dir)?;
    let link = bin_dir.join(bin_name);
    let _ = std::fs::remove_file(&link);
    symlink_file(&dest.join(bin_rel), &link).with_context(|| {
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
/// 产物须生成 `<dir>`/bin/`<bin_name>` 后软链 managed/bin——工具自烘焙路径写的是最终路径。
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
        // 127 = 命令找不到。绝大多数是**工具链没装**（`go install gopls` 里缺 go），
        // 直接说清"装什么"，而不是把 `sh: go: not found` 甩给用户。
        if out.status.code() == Some(127) {
            let tool = required_toolchain(cmd, &expanded);
            let hint = match tool.as_deref() {
                Some(t) => format!(
                    "需要工具链 `{t}`（不在 PATH）。先安装 {t} 再重试；\
                     或用系统包管理器装好该工具后 `:server unmanage {name}` 交给本机版本"
                ),
                None => "命令找不到（PATH 里没有需要的程序）".to_string(),
            };
            return Err(rollback(anyhow!(
                "tool '{name}' 安装失败：{hint}\n  底层输出：{tail}"
            )));
        }
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
    symlink_file(&bin_in, &link)
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
/// 阶段事件文案("downloading"/"writing config" 等;bytes/total 恒 None);install 的真实字节进度不走本参数——
/// 由调用方(commands/server_tasks.rs worker)经 `with_download_progress` 挂线程局部
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
                    None if !spec.install_needs_version() => String::new(),
                    None => {
                        return Err(anyhow!(
                            "'{name}' 配方未给 version;传入 version 或 config 补"
                        ))
                    }
                };
                progress("downloading", None, None);
                install(name, &ver)?;
                progress("writing config", None, None);
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
                    None if !spec.install_needs_version() => String::new(),
                    None => {
                        return Err(anyhow!(
                            "'{name}' 配方未给 version;传入 version 或 config 补"
                        ))
                    }
                };
                progress("downloading", None, None);
                update(name, &ver)?;
                progress("writing config", None, None);
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
    symlink_file(&dest.join(&spec.bin_name), &link)?;
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

// ============================ 失败留存 ============================

/// 记一条"这个 server 上次为什么失败"。
///
/// 为什么需要：安装/升级失败的信息以前只在状态栏闪一下（下一次按键就没了），
/// 用户回头看只能翻日志。这里落一个小 JSON，`:server status`/`doctor` 直接显示。
pub fn record_error(name: &str, msg: &str) {
    let path = managed_root().join("last-error.json");
    let mut map: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    map.insert(
        name.to_string(),
        serde_json::json!({ "msg": msg, "at": at }),
    );
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(&serde_json::Value::Object(map)) {
        let _ = std::fs::write(&path, text);
    }
}

/// 上次失败原因（`msg` + unix 时间戳）；没有就 None。成功后应 `clear_error`。
pub fn last_error(name: &str) -> Option<(String, u64)> {
    let path = managed_root().join("last-error.json");
    let text = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let e = v.get(name)?;
    Some((
        e.get("msg")?.as_str()?.to_string(),
        e.get("at").and_then(serde_json::Value::as_u64).unwrap_or(0),
    ))
}

/// 成功后清掉该 server 的失败记录。
pub fn clear_error(name: &str) {
    let path = managed_root().join("last-error.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    if let Some(map) = v.as_object_mut() {
        map.remove(name);
        if let Ok(t) = serde_json::to_string(&v) {
            let _ = std::fs::write(&path, t);
        }
    }
}

// ============================ 跨进程锁 ============================

/// 跨进程安装锁（`managed/install.lock`，flock）。
///
/// `op_lock()` 只是**进程内** Mutex：两个 hx 同时 `:server install` 会各自往同一个
/// `managed/<name>/` 里写（临时文件 + rename 能挡住"半个文件"，但挡不住"两份半成品
/// 互相覆盖"）。这里再加一把文件锁，拿不到就明确报"另一个 hx 正在装"。
pub struct FileLock {
    #[allow(dead_code)]
    file: std::fs::File,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            // SAFETY: fd 属于 self.file，在 drop 期间仍然有效
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

/// 取安装锁（非阻塞）。拿不到 = 别的进程在写，返回 Err。
pub fn file_lock() -> anyhow::Result<FileLock> {
    let root = managed_root();
    std::fs::create_dir_all(&root)
        .map_err(|e| anyhow::anyhow!("创建 {} 失败: {e}", root.display()))?;
    let path = root.join("install.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| anyhow::anyhow!("打开锁文件 {} 失败: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let ok = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if !ok {
            anyhow::bail!(
                "另一个 hx 正在安装/卸载 server（{} 被占用），稍后再试",
                path.display()
            );
        }
    }
    Ok(FileLock { file })
}

// ============================ 已装版本记录 + 上游最新版 ============================

/// `install` 成功后记下"我装的是哪个版本"。
///
/// 为什么需要：动态版本配方（`needs-version = true`，如 rust-analyzer）的 `spec.version`
/// 是 None ⇒ 旧的 `is_upgradable()` 恒 false，装了旧版也**永远不显示可升级**，
/// 只能靠人记得。记下实际装的版本后，"配方要的 / 上游最新 / 我装的"三者才可比。
pub fn record_installed_version(name: &str, version: &str) {
    if version.is_empty() {
        return;
    }
    let path = managed_root().join("installed.json");
    let mut map = read_json_map(&path);
    map.insert(
        name.to_string(),
        serde_json::json!({
            "version": version,
            "at": now_secs(),
        }),
    );
    write_json_map(&path, map);
}

/// 记下的已装版本（没记过 ⇒ None）。
pub fn installed_version(name: &str) -> Option<String> {
    read_json_map(&managed_root().join("installed.json"))
        .get(name)
        .and_then(|v| v.get("version"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// 上游最新版缓存（`:server update check` 写入，24h 内复用）。
pub fn latest_version(name: &str) -> Option<String> {
    let v = read_json_map(&managed_root().join("latest.json"))
        .get(name)?
        .clone();
    let at = v.get("at").and_then(serde_json::Value::as_u64).unwrap_or(0);
    if now_secs().saturating_sub(at) > 24 * 3600 {
        return None; // 过期：当作不知道（不误导成"有新版本"）
    }
    v.get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

pub fn record_latest_version(name: &str, version: &str) {
    if version.is_empty() {
        return;
    }
    let path = managed_root().join("latest.json");
    let mut map = read_json_map(&path);
    map.insert(
        name.to_string(),
        serde_json::json!({ "version": version, "at": now_secs() }),
    );
    write_json_map(&path, map);
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_json_map(path: &std::path::Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn write_json_map(path: &std::path::Path, map: serde_json::Map<String, serde_json::Value>) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(&serde_json::Value::Object(map)) {
        let _ = std::fs::write(path, text);
    }
}

/// 查上游最新版（`:server update check`）。
///
/// 只支持两种来源（保守）：
/// - `github:owner/repo` → `https://api.github.com/repos/owner/repo/releases/latest` 的 `tag_name`
/// - 其它 URL → 当作**纯文本版本号**（一行）
///
/// 结果写进 `latest.json`（24h 有效）。查询失败只记日志，不影响其它 server。
pub fn fetch_latest(spec: &crate::server_manager::registry::Spec) -> Result<Option<String>> {
    let Some(src) = spec.latest_source.as_deref() else {
        return Ok(None);
    };
    let url = match src.strip_prefix("github:") {
        Some(repo) => format!("https://api.github.com/repos/{repo}/releases/latest"),
        None => src.to_string(),
    };
    let text = fetch_text(&url).with_context(|| format!("查 {} 的最新版失败", spec.name))?;
    let version = if src.starts_with("github:") {
        // 极简取 "tag_name":"…"（不引 serde_json 的路径遍历，够用且不依赖响应结构）
        text.split("\"tag_name\"")
            .nth(1)
            .and_then(|rest| rest.split('"').nth(1))
            .map(str::to_string)
    } else {
        Some(text.lines().next().unwrap_or("").trim().to_string())
    };
    let version = version.filter(|v| !v.is_empty());
    if let Some(v) = &version {
        record_latest_version(&spec.name, v);
    }
    Ok(version)
}

/// 最小 HTTP GET（复用已下载模块的 ureq 依赖路径）。
fn fetch_text(url: &str) -> Result<String> {
    let resp = ureq::get(url)
        .set("User-Agent", "helix-server-manager")
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    resp.into_string()
        .map_err(|e| anyhow::anyhow!("读响应失败: {e}"))
}

/// 从 `cmd + args` 里猜出**真正需要的工具链**（`sh -c "GOBIN=… go install …"` → `go`）。
fn required_toolchain(cmd: &str, args: &[String]) -> Option<String> {
    // 直接就是工具链（pip/npm/cargo/…）：但 `sh` 本身一定存在，不算
    let direct = matches!(
        cmd,
        "pip" | "pip3" | "npm" | "cargo" | "go" | "gem" | "dotnet"
    );
    if let Some(script) = args
        .iter()
        .position(|a| a == "-c")
        .and_then(|i| args.get(i + 1))
    {
        for tok in script.split_whitespace() {
            if tok.contains('=') || tok.starts_with('-') || tok == "env" {
                continue;
            }
            return Some(tok.to_string());
        }
    }
    if direct {
        return Some(cmd.to_string());
    }
    None
}
