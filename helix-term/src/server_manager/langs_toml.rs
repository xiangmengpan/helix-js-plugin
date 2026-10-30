//! `languages.toml` 的读写、托管区块与可用性判断。
//!
//! 从 `server_manager/mod.rs` 抽出(G8b)。模块间不用 glob(`use super::*`),
//! 而是按编译器提示显式 `use super::<mod>::{…}` —— 双向 glob 会构成 glob 环,
//! 解析器会把环当成空集合。

use std::path::{Path, PathBuf};

use super::download::set_mirror;
use super::install::{availability, managed_root};
use super::registry;
use super::{Availability, MANAGED_FOOTER, MANAGED_HEADER};
use anyhow::{anyhow, Context as _, Result};

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

/// 独立配方文件(与代码解耦的数据;config_dir/server-manager-recipes.toml,
/// 测试/演示可经 SM_SERVER_RECIPES 覆写)。结构同 config.toml 的
/// `[server-manager.registry.<name>]`(文件顶层即 [server-manager])。
pub fn recipes_file_table() -> Option<toml::Table> {
    let path = std::env::var("SM_SERVER_RECIPES")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| helix_loader::config_dir().join("server-manager-recipes.toml"));
    let text = std::fs::read_to_string(path).ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    v.get("server-manager")
        .and_then(toml::Value::as_table)
        .cloned()
}

/// 解析一个 registry 表(表名 → 配方表)为 (name, Spec) 列表
pub(crate) fn parse_registry_table(reg: &toml::Table) -> Result<Vec<(String, registry::Spec)>> {
    let mut out = Vec::new();
    for (name, v) in reg {
        let tbl = v
            .as_table()
            .ok_or_else(|| anyhow!("[server-manager.registry.{name}] 需是表"))?;
        out.push((name.clone(), registry::parse_ext_recipe(name, tbl)?));
    }
    Ok(out)
}

/// 从 [server-manager] 段读入 mirror + 扩展配方(registry.`<name>`)；随后合并
/// 独立配方文件(server-manager-recipes.toml,后者同名覆盖)。dir 覆写 v1 未支持。
pub fn apply_server_config(cfg: Option<&toml::Table>) -> Result<()> {
    set_mirror("");
    let mut by_name: std::collections::BTreeMap<String, registry::Spec> =
        std::collections::BTreeMap::new();
    if let Some(cfg) = cfg {
        if let Some(m) = cfg.get("mirror").and_then(toml::Value::as_str) {
            set_mirror(m);
        }
        if cfg.get("dir").is_some() {
            // v1 忽略 dir 覆写(默认受管目录;环境 SM_MANAGED_DIR 可测)
        }
        if let Some(reg) = cfg.get("registry").and_then(toml::Value::as_table) {
            for (name, spec) in parse_registry_table(reg)? {
                by_name.insert(name, spec);
            }
        }
    }
    if let Some(rec) = recipes_file_table() {
        if let Some(reg) = rec.get("registry").and_then(toml::Value::as_table) {
            for (name, spec) in parse_registry_table(reg)? {
                by_name.insert(name, spec); // 配方文件同名覆盖 config
            }
        }
    }
    registry::set_ext(by_name.into_values().collect());
    Ok(())
}

// ============================ languages.toml 标记段 ============================
//
// 文本手术:文件由 标记段(# >>> helix-managed … # <<< helix-managed)内的受管内容 +
// 标记外用户手写内容组成。每次 install/update/remove 后剥离旧段 → 按已装清单重建 →
// 写回,标记外内容逐字节保留(幂等)。用户自定义了某语言 → 不生成其 [[language]]
// 条目(避免覆盖用户配置,只给 hint);用户自带同名 [language-server.X] → 不重复写表
// (TOML 禁止同一 doc 重复表头)。

/// languages.toml 路径:SM_LANGS_TOML 可覆写(测试隔离;否则 helix 全局配置目录)
pub fn lang_config_file() -> PathBuf {
    if let Ok(p) = std::env::var("SM_LANGS_TOML") {
        return PathBuf::from(p);
    }
    helix_loader::lang_config_file()
}

/// 已安装的 registry 配方(managed/bin 在位)
/// 受管已装且 detected 版本串不含配方 version → 可升级。
/// detect_version 返回 `<bin> --version` 首行整串,常带工具名前缀
/// (如 "rust-analyzer 2024-09-16"),故用"包含"判定同版;不做 semver。
/// 这个受管 server 有没有"更新的版本可用"。
///
/// 三个来源按可信度排序：
/// 1. 配方**钉了** version（`spec.version`）⇒ 已装（记录值或 `--version` 探测）与它不同就是可升级；
/// 2. 配方没钉版本（动态版本，如 rust-analyzer）⇒ 看 `:server update check` 拉到的
///    **上游最新版**缓存（24h 有效）；有缓存且 ≠ 已装 ⇒ 可升级；
/// 3. 都没有 ⇒ 不知道（**不**谎报可升级）。
pub fn is_upgradable(spec: &registry::Spec) -> bool {
    let recorded = super::install::installed_version(&spec.name);
    let detected = spec.version_detected();
    let have = recorded.clone().or_else(|| detected.clone());
    match spec.version.as_deref() {
        Some(want) => have.is_some_and(|v| !v.contains(want)),
        None => {
            let Some(latest) = super::install::latest_version(&spec.name) else {
                return false; // 没查过上游 ⇒ 不知道
            };
            match have {
                Some(v) => !v.contains(&latest) && latest != v,
                None => false,
            }
        }
    }
}

/// 安装/更新是否需要用户显式提供版本:archive 下载源 url 含 {version} 占位
/// 且配方未固定 version(config 扩展解析已强制 {version} 配 version,故 true 只可能
/// 来自内置活配方,如 rust-analyzer;Tool 类与无占位 url 恒 false)。
pub fn needs_version(spec: &registry::Spec) -> bool {
    match &spec.install {
        registry::Install::Archive {
            url_template,
            assets,
            ..
        } => {
            (url_template.contains("{version}")
                || assets.iter().any(|a| a.url.contains("{version}")))
                && spec.version.is_none()
        }
        // Tool（pip/npm/cargo/go）：args 里写 `{version}` 才算需要版本
        registry::Install::Tool { args, .. } => {
            args.iter().any(|a| a.contains("{version}")) && spec.version.is_none()
        }
    }
}

/// 本地可用(非受管、未被忽略)的配方
/// 本地停用挂接名单(managed_root/ignored.txt,每行一个配方名)
pub(crate) fn ignored_path() -> PathBuf {
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

pub(crate) fn is_ignored(name: &str) -> bool {
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
pub(crate) fn user_defined(text: &str) -> (Vec<String>, Vec<String>) {
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

pub(crate) fn toml_str(s: &str) -> String {
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

pub(crate) fn abs_bin(bin: &Path) -> PathBuf {
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
    /// 写入的 `[language-server.<n>]` 表名(已装 LSP 配方,排除用户自带同名)
    pub servers: Vec<String>,
    /// 提示:语言已被用户自定义,未自动挂接(附手动引用片段)
    pub conflicts: Vec<String>,
}

/// 重建受管段文本(不写盘)。`user_langs`/`user_ls` = 用户已在标记外定义的语言/顶层
/// language-server 表名。返回 (段文本, servers, conflicts)。
/// 运行时 languages.toml 里出现过的语言 id 集合。
///
/// 为什么需要：自动挂接会写 `[[language]] name = "objective-c"` 这种**新条目**，
/// 而 helix 只会把它与**已存在**的同名语言合并；名字不存在时这条就是"半截配置" ⇒
/// 启动直接报 `missing field 'scope'`（实测：objective-c 不在内置 languages.toml 里）。
fn known_language_ids() -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for dir in helix_loader::runtime_dirs() {
        let Ok(text) = std::fs::read_to_string(dir.join("languages.toml")) else {
            continue;
        };
        for line in text.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("name = ") {
                // 只在 [[language]] 段内（粗判：本行 name = "…"，且上一行是 [[language]]）
                if let Some(v) = rest.trim().trim_matches('"').split('"').next() {
                    if !v.is_empty() {
                        out.insert(v.to_string());
                    }
                }
            }
        }
    }
    out
}

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
        let is_attach_kind = matches!(
            spec.kind,
            registry::Kind::Lsp | registry::Kind::Dap | registry::Kind::Formatter
        );
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
    /// 每个语言聚合:（lsp 名列表, dap 配方, formatter 配方）
    type LangSlot<'a> = (
        Vec<String>,
        Option<&'a registry::Spec>,
        Option<&'a registry::Spec>,
    );
    let mut by_lang: std::collections::BTreeMap<String, LangSlot> =
        std::collections::BTreeMap::new();
    for e in &attached {
        for lang in &e.spec.languages {
            let slot = by_lang.entry(lang.clone()).or_default();
            match e.spec.kind {
                registry::Kind::Lsp => slot.0.push(e.spec.name.clone()),
                registry::Kind::Dap => slot.1 = Some(e.spec),
                registry::Kind::Formatter => slot.2 = Some(e.spec),
                registry::Kind::Linter => {}
            }
        }
    }
    let known = known_language_ids();
    for (lang, (lsp, dap, fmt)) in by_lang {
        // 内置 languages.toml 里不存在的语言**不能**写 `[[language]]`：
        // 它不会与任何条目合并 ⇒ 生成"半截配置"⇒ 启动报 missing field `scope`（实测 objective-c）。
        if !known.is_empty() && !known.contains(&lang) {
            for n in lsp.iter().chain(dap.iter().map(|s| &s.name)) {
                conflicts.push(format!(
                    "语言 '{lang}' 不在内置 languages.toml 里（{n} 未自动挂接；如需请手动添加完整条目）"
                ));
            }
            continue;
        }
        let custom = user_langs.iter().any(|u| u == &lang);
        if custom {
            // 用户已自定义该语言 → 不生成条目,提示手动挂接
            for n in &lsp {
                conflicts.push(format!(
                    "语言 '{lang}' 你已自定义:请在它的 [[language]] 条目加 language-servers = [{}]",
                    toml_str(n)
                ));
            }
            if let Some(s) = fmt {
                conflicts.push(format!(
                    "语言 '{lang}' 你已自定义:请在它的 [[language]] 条目加 formatter = {{ command = {}, args = [...] }}",
                    toml_str(&fmt_command(s))
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
        if lsp.is_empty() && dap.is_none() && fmt.is_none() {
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
            // 显式给了 `dap-cmd`（debugpy 那种 `python3 -m debugpy.adapter`）就用它，
            // 否则直接跑受管/本地的可执行文件。
            let (cmd, args) = match &s.dap_cmd {
                Some(parts) if !parts.is_empty() => (
                    parts[0].clone(),
                    parts[1..]
                        .iter()
                        .map(|a| toml_str(a))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                _ => {
                    let path = attached
                        .iter()
                        .find(|e| std::ptr::eq(e.spec, s))
                        .and_then(|e| e.local.clone())
                        .unwrap_or_else(|| s.bin_path());
                    (abs_bin(&path).to_string_lossy().into_owned(), String::new())
                }
            };
            body.push_str(&format!(
                "debugger = {{ name = {}, transport = \"stdio\", command = {}, args = [{}], templates = [{{ name = \"launch\", request = \"launch\", args = {{ }} }}] }}\n",
                toml_str(&s.name),
                toml_str(&cmd),
                args
            ));
        }
        if let Some(s) = fmt {
            let args = if s.format_args.is_empty() {
                "[]".to_string()
            } else {
                format!(
                    "[{}]",
                    s.format_args
                        .iter()
                        .map(|a| toml_str(a))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            body.push_str(&format!(
                "formatter = {{ command = {}, args = {} }}\n",
                toml_str(&fmt_command(s)),
                args
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

/// formatter 的可执行路径：受管 bin 优先，本机已装则用 PATH 里那个。
fn fmt_command(spec: &registry::Spec) -> String {
    let p = match availability(&spec.name) {
        Availability::Local(p) => p,
        _ => spec.bin_path(),
    };
    abs_bin(&p).to_string_lossy().into_owned()
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
