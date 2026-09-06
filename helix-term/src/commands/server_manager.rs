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
pub fn installed_specs() -> Vec<&'static registry::ServerSpec> {
    let mut v: Vec<_> = registry::all()
        .into_iter()
        .filter(|s| is_installed(s.name))
        .collect();
    v.sort_by_key(|s| s.name);
    v
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

/// 一次重写的结果(供命令层展示/提示)
#[derive(Debug)]
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
    let specs = installed_specs();
    let mut servers = Vec::new();
    let mut conflicts = Vec::new();
    let mut body = String::new();
    let attached: Vec<&registry::ServerSpec> = specs
        .iter()
        .copied()
        .filter(|s| matches!(s.kind, registry::Kind::Lsp | registry::Kind::Dap))
        .collect();
    // [language-server.<name>] 表(Lsp 且用户未自带同名)
    for s in &attached {
        if matches!(s.kind, registry::Kind::Lsp) && !user_ls.iter().any(|n| n == s.name) {
            body.push_str(&format!("[language-server.{}]\n", s.name));
            let cmd = abs_bin(&s.bin_path()).to_string_lossy().into_owned();
            body.push_str(&format!("command = {}\n\n", toml_str(&cmd)));
            servers.push(s.name.to_string());
        }
    }
    // 按语言聚合 lsp 名与 dap 配置
    let mut by_lang: std::collections::BTreeMap<&str, (Vec<&str>, Option<&registry::ServerSpec>)> =
        std::collections::BTreeMap::new();
    for s in &attached {
        for lang in s.languages {
            let e = by_lang.entry(lang).or_default();
            match s.kind {
                registry::Kind::Lsp => e.0.push(s.name),
                registry::Kind::Dap => e.1 = Some(s),
                _ => {}
            }
        }
    }
    for (lang, (lsp, dap)) in by_lang {
        let custom = user_langs.iter().any(|u| u == lang);
        if custom {
            // 用户已自定义该语言 → 不生成条目,提示手动挂接
            for n in &lsp {
                conflicts.push(format!(
                    "语言 '{lang}' 你已自定义:请在它的 [[language]] 条目加 language-servers = [{}]",
                    toml_str(n)
                ));
            }
            if let Some(s) = dap {
                conflicts.push(format!(
                    "语言 '{lang}' 你已自定义:请手动为它配置 debugger(command 指向 {})",
                    s.bin_path().display()
                ));
            }
            continue;
        }
        if lsp.is_empty() && dap.is_none() {
            continue;
        }
        body.push_str("[[language]]\n");
        body.push_str(&format!("name = {}\n", toml_str(lang)));
        if !lsp.is_empty() {
            let list = lsp
                .iter()
                .map(|n| toml_str(n))
                .collect::<Vec<_>>()
                .join(", ");
            body.push_str(&format!("language-servers = [{list}]\n"));
        }
        if let Some(s) = dap {
            let cmd = abs_bin(&s.bin_path());
            body.push_str(&format!(
                "debugger = {{ name = {}, transport = \"stdio\", command = {}, args = [], templates = [{{ name = \"launch\", request = \"launch\", args = {{ }} }}] }}\n",
                toml_str(s.name),
                toml_str(&cmd.to_string_lossy())
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
        /// 包管理器类(pip --target / cargo --root / npm --prefix):cmd 在受管目录内执行,
        /// args 占位 {version}、{prefix}(= 受管目录,staging);产物须落在 <prefix>/bin/<bin_name>,
        /// 框架软链到 managed/bin。
        Tool {
            cmd: &'static str,
            args: &'static [&'static str],
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

    /// 内置注册表(v1 子集)。配方为"惰性占位":下载源留空 = install 报"未配置"
    /// (避免无校验/误装);真实 URL/版本/校验由后续数据录入或 config 扩展补充。
    pub fn all() -> Vec<&'static ServerSpec> {
        vec![
            &ServerSpec {
                name: "rust-analyzer",
                kind: Kind::Lsp,
                languages: &["rust"],
                install: Install::Archive {
                    // 真实发布资产名随平台不同;v1 先留架构,真实 template/sha 待录
                    url_template: "",
                    sha256: "",
                    strip: 1,
                    bin_rel: "rust-analyzer",
                },
                bin_name: "rust-analyzer",
            },
            &ServerSpec {
                name: "gopls",
                kind: Kind::Lsp,
                languages: &["go"],
                install: Install::Archive {
                    url_template: "",
                    sha256: "",
                    strip: 1,
                    bin_rel: "gopls",
                },
                bin_name: "gopls",
            },
            &ServerSpec {
                name: "pyright",
                kind: Kind::Lsp,
                languages: &["python"],
                install: Install::Archive {
                    url_template: "",
                    sha256: "",
                    strip: 1,
                    bin_rel: "pyright-langserver",
                },
                bin_name: "pyright-langserver",
            },
            &ServerSpec {
                name: "clangd",
                kind: Kind::Lsp,
                languages: &["c", "cpp"],
                install: Install::Archive {
                    url_template: "",
                    sha256: "",
                    strip: 1,
                    bin_rel: "clangd",
                },
                bin_name: "clangd",
            },
            &ServerSpec {
                name: "debugpy",
                kind: Kind::Dap,
                languages: &["python"],
                install: Install::Tool { cmd: "", args: &[] },
                bin_name: "debugpy",
            },
            &ServerSpec {
                name: "black",
                kind: Kind::Formatter,
                languages: &["python"],
                install: Install::Tool { cmd: "", args: &[] },
                bin_name: "black",
            },
            &ServerSpec {
                name: "prettier",
                kind: Kind::Formatter,
                languages: &[
                    "javascript",
                    "typescript",
                    "html",
                    "css",
                    "json",
                    "markdown",
                ],
                install: Install::Tool { cmd: "", args: &[] },
                bin_name: "prettier",
            },
        ]
    }

    pub fn get(name: &str) -> Option<&'static ServerSpec> {
        all().into_iter().find(|s| s.name == name)
    }
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
                return Err(anyhow!("server '{name}': 下载源未配置(v1 配方待补)"));
            }
            let url = mirror_url(&url_template.replace("{version}", version));
            install_adhoc(name, spec.bin_name, bin_rel, *strip, &url, sha256)
        }
        registry::Install::Tool { cmd, args } => {
            if cmd.is_empty() {
                return Err(anyhow!("server '{name}': 下载源未配置(v1 配方待补)"));
            }
            install_tool_adhoc(name, spec.bin_name, cmd, args, version)
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
    remove_adhoc(name, spec.bin_name)?;
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
    fn sm_env(root: &Path) -> (PathBuf, EnvGuard, EnvGuard) {
        let langs = root.join("languages.toml");
        let ls = langs.to_string_lossy().into_owned();
        let m = root.to_string_lossy().into_owned();
        (
            langs,
            EnvGuard::new("SM_MANAGED_DIR", m),
            EnvGuard::new("SM_LANGS_TOML", ls),
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
        let (_langs, _a, _b) = sm_env(&root);
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
        let (langs, _ga, _gb) = sm_env(&root);
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
        let (langs, _ga, _gb) = sm_env(&root);
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
        let (_langs, _a, _b) = sm_env(&root);
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
        let (langs, _ga, _gb) = sm_env(&root);
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
        let (_langs, _a, _b) = sm_env(&root);
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
        let (langs, _ga, _gb) = sm_env(&root);
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
        let (_langs, _a, _b) = sm_env(&root);
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
}
