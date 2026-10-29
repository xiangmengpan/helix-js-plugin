//! 服务器注册表(内置配方 + 配置文件叠加)。
//!
//! 从 `mod.rs` 的内联 `pub mod registry { … }` 抽出(G8b),路径不变:
//! `server_manager::registry::Spec`。

use anyhow::{anyhow, Result};

#[cfg(test)]
use super::install::managed_dir;
use super::install::managed_root;

use super::*;

/// 工具种类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Lsp,
    Dap,
    Linter,
    Formatter,
}

/// 逐平台资产条目(照 Mason 的 `asset[]` 形状)。
/// **非空时优先于 `url_template`** —— 单一模板表达不了平台矩阵(x64-musl / aarch64-gnu / win32-x64)。
#[derive(Clone, Debug)]
pub struct ArchiveAsset {
    /// Mason 的 target 名,如 `linux_x64_musl`
    pub target: String,
    /// 含 `{version}`(可含 `{os}`/`{arch}`/`{triple}`)
    pub url: String,
    /// 该平台的 bin;空 = 沿用外层 `bin_rel`
    pub bin_rel: String,
}

/// 解析 `[[server-manager.registry.<n>.asset]]`(缺省 → 空数组)
fn parse_assets(tbl: &toml::map::Map<String, toml::Value>) -> anyhow::Result<Vec<ArchiveAsset>> {
    let Some(arr) = tbl.get("asset").and_then(toml::Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for a in arr {
        let target = a
            .get("target")
            .and_then(toml::Value::as_str)
            .unwrap_or("")
            .to_string();
        let url = a
            .get("url")
            .and_then(toml::Value::as_str)
            .unwrap_or("")
            .to_string();
        if target.is_empty() || url.is_empty() {
            return Err(anyhow::anyhow!("registry asset 条目需要 target 与 url"));
        }
        out.push(ArchiveAsset {
            target,
            url,
            bin_rel: a
                .get("bin")
                .and_then(toml::Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
    }
    Ok(out)
}

/// 当前平台可能对应的 Mason target 名(**按优先级**,后者更通用)。
/// 不做 libc 探测 → gnu/musl 无法自动区分:把更精确的名字排在前面,
/// 但只有配方**同时**提供两者时才需要区分;两者都缺则回落单模板。
pub(crate) fn platform_targets() -> Vec<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => vec!["linux_x64_musl", "linux_x64_gnu", "linux_x64"],
        ("linux", "aarch64") => vec!["linux_arm64_musl", "linux_arm64_gnu", "linux_arm64"],
        ("linux", "x86") => vec!["linux_x86"],
        ("macos", "x86_64") => vec!["darwin_x64"],
        ("macos", "aarch64") => vec!["darwin_arm64"],
        ("windows", "x86_64") => vec!["win_x64"],
        ("windows", "aarch64") => vec!["win_arm64"],
        _ => Vec::new(),
    }
}

/// 按当前平台从资产表选一条(无匹配 → None,调用方回落单模板)
pub(crate) fn pick_asset(assets: &[ArchiveAsset]) -> Option<&ArchiveAsset> {
    platform_targets()
        .into_iter()
        .find_map(|t| assets.iter().find(|a| a.target == t))
}

/// 动态版本校验:任一 url 模板(单模板或逐平台资产)含 `{version}` 时,
/// 必须给 `version` 或显式 `needs-version = true`。
fn version_gate_ok(
    url: &Option<String>,
    assets: &[ArchiveAsset],
    version: Option<&str>,
    needs_version: bool,
) -> bool {
    if version.is_some() || needs_version {
        return true;
    }
    let single = url.as_deref().unwrap_or("");
    !single.contains("{version}") && !assets.iter().any(|a| a.url.contains("{version}"))
}

/// 安装方式(owned;config 扩展配方经 TOML 构造)
#[derive(Debug, Clone)]
pub enum Install {
    /// 下载 release 产物(可 tar.gz/zip);url_template 含 {version} 占位
    Archive {
        url_template: String,
        /// 逐平台资产(空 = 用 url_template 简写)
        assets: Vec<ArchiveAsset>,
        sha256: String,
        strip: usize,
        /// 解压后要链接到 managed/bin 的可执行相对路径(相对解压根)
        bin_rel: String,
    },
    /// 包管理器类(pip --target / cargo --root / npm --prefix):cmd 在受管目录内执行,
    /// args 占位 {version}、{prefix}(= 受管目录);产物须落在 `<prefix>/bin/<bin_name>`
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
    /// 上游最新版的来源：`github:owner/repo` 或一个返回**纯文本版本号**的 URL。
    /// 空 = 不支持查最新版（`:server update check` 会跳过并说明）。
    pub latest_source: Option<String>,
    /// `kind = "dap"` 时写进 `languages.toml` 的 debugger 启动命令（首元素=command，
    /// 其余=args）。为空 ⇒ 用受管/local 的 bin 路径直接跑。
    /// 典型：debugpy 的 adapter 是 `python3 -m debugpy.adapter`，不是那个 console script。
    pub dap_cmd: Option<Vec<String>>,
    /// `kind = "linter"` 时要执行的命令（**shell 串**，支持 `%{file_path_absolute}` 等
    /// 编辑器变量展开）；`:lint` 跑它、按 gcc 风格解析输出。
    pub lint_cmd: Option<String>,
    /// `kind = "formatter"` 时写进 `languages.toml` 的 `formatter.args`
    /// （如 `ruff format --stdin-filename {}` 这类；空 = 不写 args）
    pub format_args: Vec<String>,
}

impl Spec {
    #[cfg(test)]
    pub fn bin_rel_for_test(&self) -> String {
        match &self.install {
            Install::Archive { bin_rel, .. } => bin_rel.clone(),
            Install::Tool { .. } => String::new(),
        }
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
            Install::Archive {
                url_template,
                assets,
                ..
            } => !url_template.is_empty() || !assets.is_empty(),
            Install::Tool { cmd, .. } => !cmd.is_empty(),
        }
    }
    /// Archive 且 url 含 {version} 占位(安装必须给版本);Tool 与无占位 Archive 不需要
    pub fn install_needs_version(&self) -> bool {
        matches!(
            &self.install,
            Install::Archive {
                url_template,
                assets,
                ..
            } if url_template.contains("{version}")
                || assets.iter().any(|a| a.url.contains("{version}"))
        )
    }
}

/// 内置注册表(v1 子集)。配方为"惰性占位":下载源留空 = install 报"未配置"
/// (避免无校验/误装);真实 URL/版本/校验是 OS 相关数据,经
/// `[server-manager.registry.<name>]` config 扩展(T4)提供可跑源。
/// M5→数据化:内置配方已迁至 `runtime/server-manager-builtin.toml`(改数据不改代码;
/// 可在 config_dir/runtime/ 放同名文件整体覆盖,失败回退嵌入版本)。
pub(crate) fn builtin_specs() -> Vec<Spec> {
    // 优先 runtime_dirs 中的可覆盖数据文件;解析失败则回退编译期嵌入。
    // 单测(cfg(test))只用嵌入副本:避免开发机 config_dir/runtime 的个人覆盖影响断言;
    // 生产/集成仍允许整体覆盖。
    let mut text = BUILTIN_RECIPES_TOML.to_string();
    if !cfg!(test) {
        for dir in helix_loader::runtime_dirs() {
            let path = dir.join("server-manager-builtin.toml");
            if let Ok(t) = std::fs::read_to_string(&path) {
                if parse_builtin_recipes(&t).is_ok() {
                    text = t;
                    break;
                }
                log::warn!("{} 解析失败,回退嵌入内置配方", path.to_string_lossy());
            }
        }
    }
    parse_builtin_recipes(&text).expect("内置配方 TOML 解析失败")
}

/// 内置配方嵌入副本(packaging 兜底:runtime 目录缺失时仍可用)
const BUILTIN_RECIPES_TOML: &str = include_str!("../../../runtime/server-manager-builtin.toml");

/// 解析内置配方文件:按 `order`(缺省向后)排序,保证 arsenal 行序稳定
fn parse_builtin_recipes(text: &str) -> Result<Vec<Spec>> {
    use toml::Value;
    let v: Value = toml::from_str(text)?;
    let reg = v
        .get("server-manager")
        .and_then(|s| s.get("registry"))
        .and_then(Value::as_table)
        .ok_or_else(|| anyhow!("内置配方文件缺 [server-manager.registry.*]"))?;
    let mut out: Vec<(i64, String, Spec)> = Vec::new();
    for (name, tv) in reg {
        let tbl = tv
            .as_table()
            .ok_or_else(|| anyhow!("内置配方 '{name}' 需是表"))?;
        let order = tbl
            .get("order")
            .and_then(Value::as_integer)
            .unwrap_or(i64::MAX);
        out.push((order, name.clone(), parse_ext_recipe(name, tbl)?));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(out.into_iter().map(|(_, _, s)| s).collect())
}

/// 扩展配方的存放点。
///
/// **生产**:进程级 `OnceLock<Mutex<Vec<Spec>>>` —— worker 线程(server_tasks)与主线程
/// 都要看到同一份配置。
///
/// **测试**:thread-local —— 这是 2026-10-17(D8)的实测结论。cargo 默认并行,
/// `EXT` 是进程级可写缓存,任何一条用例(或它派生的 worker 线程)调用
/// `apply_server_config` 都会把并发用例的配方换掉,于是出现
/// `install("asset-ra")` 间歇报 "unknown server"、`builtin_recipes` 间歇看到
/// "可安装的 pyright"。这类"测试之间的共享可变状态"没法靠加锁彻底解决
/// (worker 线程不归测试管),所以测试期把它降级成线程私有;生产的跨线程语义不变。
#[cfg(not(test))]
static EXT: std::sync::OnceLock<std::sync::Mutex<Vec<Spec>>> = std::sync::OnceLock::new();

#[cfg(test)]
thread_local! {
    static EXT_TLS: std::cell::RefCell<Vec<Spec>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// 重读 config 扩展配方(每次 server 操作前从 [server-manager.registry] 解析后调用)
pub fn set_ext(specs: Vec<Spec>) {
    #[cfg(not(test))]
    {
        *EXT.get_or_init(Default::default).lock().unwrap() = specs;
    }
    #[cfg(test)]
    EXT_TLS.with(|c| *c.borrow_mut() = specs);
}

/// 当前扩展配方快照(测试期读 thread-local)。
fn ext_specs() -> Option<Vec<Spec>> {
    #[cfg(not(test))]
    {
        EXT.get().map(|m| m.lock().unwrap().clone())
    }
    #[cfg(test)]
    {
        Some(EXT_TLS.with(|c| c.borrow().clone()))
    }
}

/// 全部配方:内置 + config/配方文件扩展(快照;同名后者覆盖内置,不重复)。
/// 覆盖语义与文档/加载链一致:contrib 的 pyright 应替掉内置惰性占位。
/// 顺序:内置保持文件 order;扩展按名(apply_server_config 的 BTreeMap)追加。
pub fn all() -> Vec<Spec> {
    let mut v = builtin_specs();
    if let Some(ext) = ext_specs() {
        let names: std::collections::HashSet<&str> = ext.iter().map(|s| s.name.as_str()).collect();
        v.retain(|s| !names.contains(s.name.as_str()));
        v.extend(ext);
    }
    v
}

pub fn get(name: &str) -> Option<Spec> {
    all().into_iter().find(|s| s.name == name)
}

/// 解析 `[server-manager.registry.<name>]` 配方表(v1:archive 类)。
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
    let mut url = tbl.get("url").and_then(Value::as_str).map(str::to_string); // 允许显式空串 = 占位
    let assets = parse_assets(tbl)?;
    // 归一化:**只有 asset[] 而没写 url** → 视为"空 url + 有资产"。
    // 这样后面按 url 分流的分支**无需改动**,而 is_installable 会因 assets 非空判为可装。
    if url.is_none() && !assets.is_empty() {
        url = Some(String::new());
    }
    let cmd = tbl
        .get("cmd")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    // cmd 键存在但为空 = 占位 Tool(无下载源,同 url = "" 占位语义)
    let cmd_placeholder = cmd.is_none() && tbl.contains_key("cmd");
    // needs-version = true:允许 url 含 {version} 而 version 留空(动态版本配方,
    // 安装/升级弹版本输入;内置 rust-analyzer 即此形态)
    let needs_version_flag = tbl
        .get("needs-version")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let version = tbl
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string);
    let bin_name = tbl
        .get("bin-name")
        .and_then(Value::as_str)
        .unwrap_or(name)
        .to_string();
    // 上游最新版来源（`github:owner/repo` / URL；空 = 不支持）
    let latest_source = tbl
        .get("latest")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    // DAP 启动命令（数组；仅 dap）
    let dap_cmd: Option<Vec<String>> = match tbl.get("dap-cmd") {
        Some(Value::Array(a)) => Some(
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
        )
        .filter(|v: &Vec<String>| !v.is_empty()),
        _ => None,
    };
    // linter 的执行命令（shell 串；其它 kind 忽略）
    let lint_cmd = tbl
        .get("lint-cmd")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    // formatter 的参数（其它 kind 忽略）
    let format_args: Vec<String> = match tbl.get("format-args") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    // **有下载源** = 单模板 url 非空 **或** 给了逐平台 asset[](Mason 形状)。
    // 早先只看 url ⇒ "只写 asset[]" 的配方会掉进占位分支:strip 被强制成 1、bin 被忽略
    // (用 bin-name),单文件归档被 strip 掉 ⇒ 报"找不到可执行"(实测踩到)。
    let has_url = url.as_deref().is_some_and(|u| !u.is_empty());
    let has_source = has_url || !assets.is_empty();
    // {version} 校验要**同时**看单模板与逐平台资产:否则只有 asset[] 的动态版本配方
    // 会绕过 needs-version 检查,装的时候才报"未支持占位"。
    if !version_gate_ok(&url, &assets, version.as_deref(), needs_version_flag) {
        return Err(anyhow!(
            "registry '{name}': url 含 {{version}} 占位则必填 version(或设 needs-version = true)"
        ));
    }
    let install = match (cmd, has_source) {
        (Some(_), true) => return Err(anyhow!("registry '{name}': url 与 cmd 只能二选一")),
        (Some(cmd), false) => {
            let args: Vec<String> = match tbl.get("args") {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
                _ => Vec::new(),
            };
            Install::Tool { cmd, args }
        }
        (None, true) => {
            let url = url.unwrap_or_default();
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
            Install::Archive {
                url_template: url.to_string(),
                assets,
                sha256,
                strip,
                bin_rel: need("bin")?,
            }
        }
        (None, false) => {
            // 显式 url = "" → 占位(列表 no source;description 写安装引导)
            if url.as_deref() == Some("") {
                Install::Archive {
                    url_template: String::new(),
                    assets,
                    sha256: String::new(),
                    strip: 1,
                    bin_rel: bin_name.clone(),
                }
            } else if cmd_placeholder {
                // cmd = "" → 占位 Tool(无下载源/包管理器,description 引导)
                let args: Vec<String> = match tbl.get("args") {
                    Some(Value::Array(a)) => a
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect(),
                    _ => Vec::new(),
                };
                Install::Tool {
                    cmd: String::new(),
                    args,
                }
            } else {
                return Err(anyhow!(
                    "registry '{name}': 缺安装方式(url、cmd 或 url = \"\" 占位)"
                ));
            }
        }
    };
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
        latest_source,
        dap_cmd,
        lint_cmd,
        format_args,
        install,
    })
}
