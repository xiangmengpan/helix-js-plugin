//! Server Manager（mason 式）——LSP/DAP/linter/formatter 的安装/升级/卸载管理。
//!
//! 设计(见 docs/superpowers/specs/2026-09-05-server-manager-design.md):
//! - 注册表:内置 ServerSpec 常量 + config 扩展(T4)
//! - 受管目录 `~/.local/share/helix/managed/<name>`(T1 默认;T4 可配)
//! - archive 配方:mirror URL 改写 + sha256 校验 + tar.gz/zip 解压 + managed/bin 软链
//! - languages.toml 标记段自动写(T3)
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context as _, Result};

/// 下载层 mirror 前缀(空 = 直连)。T1 无 config,模块级可设(T4 接 config)。
fn mirror_prefix() -> &'static str {
    ""
}

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

/// 下载 URL 到 dest(file:// 本地复制;https 经 ureq;遵循 mirror)
fn download_to(url: &str, dest: &Path) -> Result<()> {
    if let Some(path) = url.strip_prefix("file://") {
        std::fs::copy(path, dest).with_context(|| format!("copy {path} -> {}", dest.display()))?;
        return Ok(());
    }
    let resp = ureq::get(url)
        .call()
        .with_context(|| format!("download {url}"))?;
    let mut reader = resp.into_reader();
    let mut out = std::fs::File::create(dest)?;
    std::io::copy(&mut reader, &mut out)?;
    Ok(())
}

/// 解压 tar.gz / zip 到 out_dir(可选 strip 顶层目录)
fn extract_archive(path: &Path, out_dir: &Path, strip: usize) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let f = std::fs::File::open(path)?;
    if path.extension().map(|e| e == "zip").unwrap_or(false) {
        let mut z = zip::ZipArchive::new(f)?;
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
    } else {
        // tar(.gz)——gzip 由 magic 判定
        let r = std::io::BufReader::new(f);
        macro_rules! unpack_tar {
            ($ar:expr) => {{
                for entry in $ar.entries()? {
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
            }};
        }
        if looks_gzip(path) {
            let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(r));
            unpack_tar!(ar);
        } else {
            let mut ar = tar::Archive::new(r);
            unpack_tar!(ar);
        }
    }
    Ok(())
}

fn looks_gzip(path: &Path) -> bool {
    std::fs::read(path)
        .ok()
        .map(|b| b.len() > 2 && b[0] == 0x1f && b[1] == 0x8b)
        .unwrap_or(false)
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

    /// 安装方式
    #[derive(Debug, Clone)]
    pub enum Install {
        /// 下载 release 产物(可 tar.gz/zip);url_template 含 {version} 占位
        Archive {
            url_template: &'static str,
            sha256: &'static str,
            strip: usize,
            /// 解压后要链接到 managed/bin 的可执行相对路径(相对解压根)
            bin_rel: &'static str,
        },
    }

    /// 配方
    pub struct ServerSpec {
        pub name: &'static str,
        pub kind: Kind,
        pub languages: &'static [&'static str],
        pub install: Install,
        /// 版本检测别名(默认 name)
        pub bin_name: &'static str,
    }

    impl ServerSpec {
        /// 已装工具的 bin 绝对路径
        pub fn bin_path(&self) -> PathBuf {
            managed_root().join("bin").join(self.bin_name)
        }
        pub fn installed_dir(&self) -> PathBuf {
            managed_dir(self.name)
        }
        pub fn version(&self) -> Option<String> {
            detect_version(&self.bin_path())
        }
    }

    /// 内置注册表(v1 子集;T1 仅 archive 类)
    pub fn all() -> Vec<&'static ServerSpec> {
        // sha256 占位为空的配方在 install 时若配了 sha256 才校验(空=跳过校验,v1 内置已填真实值前先跳过)
        vec![
            &ServerSpec {
                name: "rust-analyzer",
                kind: Kind::Lsp,
                languages: &["rust"],
                install: Install::Archive {
                    // 真实发布资产名随平台不同;v1 配方先留 template+bin_rel 架构,
                    // sha256 为空 = 跳过校验(下载仍可用)——真实值由 T4/后续录入时补
                    url_template: "",
                    sha256: "",
                    strip: 1,
                    bin_rel: "rust-analyzer",
                },
                bin_name: "rust-analyzer",
            },
            // gopls/black/prettier 等后续批次;T1 架构验证用注入式配方(测试)
        ]
    }

    pub fn get(name: &str) -> Option<&'static ServerSpec> {
        all().into_iter().find(|s| s.name == name)
    }
}

// ============================ 安装/升级/卸载 ============================

/// 安装(archive):下载 → sha256 → 解压到临时 → 软链 bin → rename。
/// version 占位(v1:url_template 空 = 未接下载源 → 报"配方未配置下载源",测试用注入 override)
pub fn install(name: &str, version: &str) -> Result<()> {
    let spec = registry::get(name).ok_or_else(|| anyhow!("unknown server '{name}'"))?;
    let registry::Install::Archive {
        url_template,
        sha256,
        strip,
        bin_rel,
    } = &spec.install;
    if url_template.is_empty() {
        return Err(anyhow!("server '{name}': 下载源未配置(v1 配方待补)"));
    }
    let url = mirror_url(&url_template.replace("{version}", version));
    install_adhoc(name, spec.bin_name, bin_rel, *strip, &url, sha256)
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
    download_to(&url, &tmp_archive)?;
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
    extract_archive(&tmp_archive, &tmp_dir, strip).with_context(|| format!("extract {name}"))?;
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

/// 升级:先检测当前版本,重装最新(失败不动旧版;v1 无远端版本号接口,由命令层给 version)
pub fn update(name: &str, version: &str) -> Result<()> {
    if !is_installed(name) {
        return Err(anyhow!("server '{name}' 未安装,用 install"));
    }
    install(name, version)
}

/// 卸载:删受管目录 + bin 软链(不动 languages.toml,T3 处理配置)
pub fn remove(name: &str) -> Result<()> {
    let spec = registry::get(name).ok_or_else(|| anyhow!("unknown server '{name}'"))?;
    remove_adhoc(name, spec.bin_name)
}

/// 卸载内核(测试/adhoc)
pub fn remove_adhoc(name: &str, bin_name: &str) -> Result<()> {
    let _ = std::fs::remove_dir_all(managed_dir(name));
    let _ = std::fs::remove_file(managed_bin(bin_name));
    Ok(())
}

/// 供测试/开发:把本地目录作为"已装"(跳过下载),验证 bin 检测/版本
#[doc(hidden)]
pub fn install_from_local_dir_for_test(name: &str, dir: &Path) -> Result<()> {
    let spec = registry::get(name).ok_or_else(|| anyhow!("unknown '{name}'"))?;
    let dest = spec.installed_dir();
    let _ = std::fs::remove_dir_all(&dest);
    copy_dir_recursive(dir, &dest)?;
    let bin_dir = managed_root().join("bin");
    std::fs::create_dir_all(&bin_dir)?;
    let link = bin_dir.join(spec.bin_name);
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(dest.join(spec.bin_name), &link)?;
    Ok(())
}

#[doc(hidden)]
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
    fn tmp_root() -> PathBuf {
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("sm-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn mirror_url_rewrite_and_file_passthrough() {
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
        let root = tmp_root();
        let _guard = EnvGuard("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
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

    struct EnvGuard(&'static str, String);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }
}
