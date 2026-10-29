//! 调试启动配置：`launch.toml` / `.vscode/launch.json` / `languages.toml` 模板的合并层。
//!
//! 两条边界（设计见 `docs/superpowers/specs/2026-10-25-dap-redesign-design.md` §6）：
//!
//! - **adapter 定义**（起哪个进程、transport、port_arg）**只**来自
//!   `languages.toml [language.debugger]`（用户手写或 server-manager 写入）；
//! - **启动配置**（起什么程序、什么参数）来自本模块的三级合并：
//!   `.helix/launch.toml`（高） → `.vscode/launch.json`（中） → `[language.debugger].templates`（兜底）。
//!
//! 原则：**不认识的东西一律报错，不静默忽略**。VS Code 的 `launch.json` 只支持子集
//! （`type` `request` `name` `program` `args` `cwd` `env` `processId` `stopOnEntry`…），
//! 遇到 `preLaunchTask` / `compounds` / `inputs` 直接拒绝 —— "看起来支持、其实静默无效"
//! 比"不支持"糟得多。

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// 配置来源（`configs()` 快照里给 UI 标注）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Toml,
    Json,
    /// `languages.toml` 的 `[language.debugger].templates`（兜底，永远可用）
    Lang,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Toml => "toml",
            Source::Json => "json",
            Source::Lang => "lang",
        }
    }
}

/// 一条可启动的配置（已做过变量替换）。
#[derive(Debug, Clone)]
pub struct LaunchConfig {
    pub name: String,
    /// `"launch"` | `"attach"`
    pub request: String,
    /// 目标语言（`launch.toml` 的 `language`，或从文件所在语言推断）
    pub language: Option<String>,
    /// `launch.json` 的 `type`（= adapter 名，如 `lldb-dap`）
    pub adapter: Option<String>,
    /// 请求参数（`program`/`args`/`cwd`/`env`/…，变量已替换）
    pub args: Value,
    /// 启动前要跑的构建命令（`None` = 不构建）。变量已替换，`${program}` 可用。
    pub build: Option<String>,
    pub source: Source,
}

/// 变量替换的上下文。
#[derive(Debug, Clone)]
pub struct VarCtx {
    /// 工作区根（`${workspaceFolder}`）
    pub workspace: PathBuf,
    /// 当前文件绝对路径（`${file}` 等）
    pub file: Option<PathBuf>,
    /// 当前工作目录（`${cwd}`）
    pub cwd: PathBuf,
    /// Cargo 目标名（`${cargoBin}`）；非 Rust 项目为 None
    pub cargo_bin: Option<String>,
    /// `debug` | `release`（`${cargoProfile}`）
    pub cargo_profile: String,
    /// 已解析的 program（**只在展开 `build` 时**可用：`${program}`）
    pub program: Option<String>,
}

impl Default for VarCtx {
    fn default() -> Self {
        Self {
            workspace: PathBuf::new(),
            file: None,
            cwd: PathBuf::new(),
            cargo_bin: None,
            cargo_profile: "debug".to_string(),
            program: None,
        }
    }
}

#[cfg(test)]
impl VarCtx {
    /// 测试便利：设置路径字段（其余保持已给定的值）。
    fn with_paths(mut self, workspace: impl AsRef<Path>, file: Option<impl AsRef<Path>>) -> Self {
        self.workspace = workspace.as_ref().to_path_buf();
        self.cwd = workspace.as_ref().to_path_buf();
        self.file = file.map(|f| f.as_ref().to_path_buf());
        self
    }
}

impl VarCtx {
    fn file_str(&self) -> Option<String> {
        self.file.as_ref().map(|p| p.to_string_lossy().into_owned())
    }
}

/// 支持的 VS Code 变量（最小集）。`${env:VAR}` 单独处理。
const VARS: [&str; 9] = [
    "workspaceFolder",
    "file",
    "fileBasename",
    "fileDirname",
    "fileBasenameNoExtension",
    "cwd",
    "cargoBin",
    "cargoProfile",
    "program",
];

/// VS Code 里有、我们**明确不支持**的字段（遇到就报错，不静默）。
const UNSUPPORTED: [&str; 3] = ["preLaunchTask", "compounds", "inputs"];

/// 读三级配置并合并。返回 `(合并结果, 告警)`：
///
/// - 文件不存在 = 正常情况（不告警）；
/// - 解析失败 / 不支持字段 / 变量未知 = 告警（该条配置被跳过，其余照常）
pub fn load(
    workspace: &Path,
    language: Option<&str>,
    lang_templates: &[LangTemplate],
    ctx: &VarCtx,
) -> (Vec<LaunchConfig>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut configs = Vec::new();

    // ① .helix/launch.toml（最高优先级）
    let toml_path = workspace.join(".helix").join("launch.toml");
    let mut from_toml = Vec::new();
    match std::fs::read_to_string(&toml_path) {
        Ok(text) => match parse_toml(&text) {
            Ok((cfgs, mut warns)) => {
                from_toml = cfgs;
                warnings.append(&mut warns);
            }
            Err(err) => warnings.push(format!("{}: {err}", toml_path.display())),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warnings.push(format!("{}: {e}", toml_path.display())),
    }

    // ② .vscode/launch.json（中）
    let json_path = workspace.join(".vscode").join("launch.json");
    let mut from_json = Vec::new();
    match std::fs::read_to_string(&json_path) {
        Ok(text) => match parse_json(&text) {
            Ok((cfgs, mut warns)) => {
                from_json = cfgs;
                warnings.append(&mut warns);
            }
            Err(err) => warnings.push(format!("{}: {err}", json_path.display())),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warnings.push(format!("{}: {e}", json_path.display())),
    }

    // ③ 合并：同名 `name` 逐字段覆盖（toml 赢）
    let mut merged = from_json;
    for t in from_toml {
        match merged.iter_mut().find(|j| j.name == t.name) {
            Some(slot) => {
                let mut args = slot.args.clone();
                if let (Some(dst), Some(src)) = (args.as_object_mut(), t.args.as_object()) {
                    for (k, v) in src {
                        dst.insert(k.clone(), v.clone());
                    }
                }
                slot.args = args;
                slot.request = t.request;
                slot.language = t.language.or(slot.language.take());
                slot.adapter = t.adapter.or(slot.adapter.take());
                slot.source = Source::Toml;
            }
            None => merged.push(t),
        }
    }

    // ④ 变量替换 + 语言过滤
    for mut cfg in merged {
        if let Some(lang) = &cfg.language {
            if language.is_some_and(|cur| cur != lang) {
                continue;
            }
        }
        match expand(&cfg.args, ctx) {
            Ok(args) => cfg.args = args,
            Err(err) => {
                warnings.push(format!("配置 {:?}: {err}", cfg.name));
                continue;
            }
        }
        // `build` 里的 `${program}` 要等 args 展完才知道 ⇒ 第二遍，带 program 展开
        if let Some(build) = cfg.build.take() {
            let mut ctx2 = ctx.clone();
            ctx2.program = cfg
                .args
                .get("program")
                .and_then(|p| p.as_str())
                .map(str::to_string);
            match expand(&Value::String(build), &ctx2) {
                Ok(Value::String(b)) => cfg.build = Some(b),
                Ok(_) => unreachable!("expand 对 String 只会返回 String"),
                Err(err) => {
                    warnings.push(format!("配置 {:?}: build: {err}", cfg.name));
                    continue;
                }
            }
        }
        configs.push(cfg);
    }

    // ⑤ languages.toml 模板兜底（永远可用；与上面同名的以文件为准）
    for t in lang_templates {
        if configs.iter().any(|c| c.name == t.name) {
            continue;
        }
        let args = match expand(&t.args, ctx) {
            Ok(args) => args,
            Err(err) => {
                warnings.push(format!("模板 {:?}: {err}", t.name));
                continue;
            }
        };
        configs.push(LaunchConfig {
            name: t.name.clone(),
            request: t.request.clone(),
            language: language.map(str::to_string),
            adapter: None,
            args,
            build: None,
            source: Source::Lang,
        });
    }

    (configs, warnings)
}

/// `languages.toml` 的模板（本模块不依赖 helix-core 的配置类型，保持可单测）。
#[derive(Debug, Clone)]
pub struct LangTemplate {
    pub name: String,
    pub request: String,
    pub args: Value,
}

// ============================ launch.toml ============================

/// `[[config]]` 列表。字段：`name`（必填）、`request`（缺省 launch）、`language`、`type`、
/// 其余键原样进请求参数。
pub fn parse_toml(text: &str) -> Result<(Vec<LaunchConfig>, Vec<String>), String> {
    let value: toml::Value = toml::from_str(text).map_err(|e| format!("TOML 解析失败: {e}"))?;
    let mut warnings = Vec::new();
    let mut out = Vec::new();

    // 顶层不支持字段
    if let Some(table) = value.as_table() {
        for key in UNSUPPORTED {
            if table.contains_key(key) {
                return Err(format!(
                    "不支持 `{key}`（VS Code 的 launch.json 特性；本仓只支持最小子集）"
                ));
            }
        }
    }

    let Some(list) = value.get("config").and_then(|v| v.as_array()) else {
        return Ok((
            out,
            vec!["launch.toml: 没有 [[config]] 段（忽略）".to_string()],
        ));
    };

    for (i, item) in list.iter().enumerate() {
        let Some(table) = item.as_table() else {
            warnings.push(format!("launch.toml: 第 {} 个 config 不是表", i + 1));
            continue;
        };
        let Some(name) = table.get("name").and_then(|v| v.as_str()) else {
            warnings.push(format!("launch.toml: 第 {} 个 config 缺 `name`", i + 1));
            continue;
        };
        let request = table
            .get("request")
            .and_then(|v| v.as_str())
            .unwrap_or("launch")
            .to_string();
        if request != "launch" && request != "attach" {
            warnings.push(format!(
                "配置 {name:?}: `request` 只能是 launch/attach（得到 {request:?}）"
            ));
            continue;
        }
        // 不支持字段（逐条检测：命中就跳过该条，不静默）
        if let Some(bad) = UNSUPPORTED.iter().find(|k| table.contains_key(**k)) {
            warnings.push(format!("配置 {name:?}: 不支持 `{bad}`（已跳过该配置）"));
            continue;
        }
        let language = table
            .get("language")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let adapter = table
            .get("type")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let build = table
            .get("build")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if table.contains_key("build") && build.is_none() {
            warnings.push(format!(
                "配置 {name:?}: `build` 必须是字符串（已按未设置处理）"
            ));
        }

        let mut args = Map::new();
        for (key, val) in table {
            if matches!(
                key.as_str(),
                "name" | "request" | "language" | "type" | "build"
            ) {
                continue;
            }
            args.insert(key.clone(), toml_to_json(val));
        }
        out.push(LaunchConfig {
            name: name.to_string(),
            request,
            language,
            adapter,
            args: Value::Object(args),
            build,
            source: Source::Toml,
        });
    }
    Ok((out, warnings))
}

fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
    }
}

// ============================ launch.json ============================

/// VS Code 子集：顶层 `{ "configurations": [ … ] }`；遇到 `compounds`/`inputs` 直接拒绝。
pub fn parse_json(text: &str) -> Result<(Vec<LaunchConfig>, Vec<String>), String> {
    // VS Code 允许注释与尾逗号；先剥掉行注释再解析（JSONC 的最小支持）
    let stripped = strip_json_comments(text);
    let value: Value =
        serde_json::from_str(&stripped).map_err(|e| format!("JSON 解析失败: {e}"))?;
    let Some(obj) = value.as_object() else {
        return Err("顶层不是对象（期望 { \"configurations\": [...] }）".to_string());
    };
    for key in UNSUPPORTED {
        if obj.contains_key(key) {
            return Err(format!(
                "不支持 `{key}`（VS Code 特性；本仓只支持最小子集）"
            ));
        }
    }
    let Some(list) = obj.get("configurations").and_then(|v| v.as_array()) else {
        return Ok((
            Vec::new(),
            vec!["launch.json: 没有 `configurations` 数组（忽略）".to_string()],
        ));
    };

    let mut warnings = Vec::new();
    let mut out = Vec::new();
    for (i, item) in list.iter().enumerate() {
        let Some(table) = item.as_object() else {
            warnings.push(format!("launch.json: 第 {} 个配置不是对象", i + 1));
            continue;
        };
        let Some(name) = table.get("name").and_then(|v| v.as_str()) else {
            warnings.push(format!("launch.json: 第 {} 个配置缺 `name`", i + 1));
            continue;
        };
        let request = table
            .get("request")
            .and_then(|v| v.as_str())
            .unwrap_or("launch")
            .to_string();
        if request != "launch" && request != "attach" {
            warnings.push(format!(
                "配置 {name:?}: `request` 只能是 launch/attach（得到 {request:?}）"
            ));
            continue;
        }
        if let Some(bad) = UNSUPPORTED.iter().find(|k| table.contains_key(**k)) {
            warnings.push(format!("配置 {name:?}: 不支持 `{bad}`（已跳过该配置）"));
            continue;
        }
        let adapter = table
            .get("type")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let language = table
            .get("language")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        // `build` 是本仓的扩展（VS Code 的 launch.json 没有它，也没有 preLaunchTask 支持）
        let build = table
            .get("build")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let mut args = Map::new();
        for (key, val) in table {
            if matches!(
                key.as_str(),
                "name" | "request" | "type" | "language" | "build"
            ) {
                continue;
            }
            args.insert(key.clone(), val.clone());
        }
        out.push(LaunchConfig {
            name: name.to_string(),
            request,
            language,
            adapter,
            args: Value::Object(args),
            build,
            source: Source::Json,
        });
    }
    Ok((out, warnings))
}

/// 剥掉 `//` 行注释与 `/* */` 块注释（VS Code 的 launch.json 是 JSONC）。
/// **字符串里的注释标记不能动** —— 例如 `"args": ["http://x"]`。
fn strip_json_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                while let Some(&n) = chars.peek() {
                    if n == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                while let Some(c2) = chars.next() {
                    if c2 == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

// ============================ 变量替换 ============================

/// 递归替换 `${...}`。未知变量 ⇒ `Err`（**不静默留字面量**）。
pub fn expand(value: &Value, ctx: &VarCtx) -> Result<Value, String> {
    match value {
        Value::String(s) => Ok(Value::String(expand_str(s, ctx)?)),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(expand(item, ctx)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), expand(v, ctx)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

fn expand_str(s: &str, ctx: &VarCtx) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            // 没有闭合：原样保留（不当作变量）
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let name = &after[..end];
        let resolved = resolve(name, ctx)?;
        out.push_str(&resolved);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn resolve(name: &str, ctx: &VarCtx) -> Result<String, String> {
    if let Some(var) = name.strip_prefix("env:") {
        return std::env::var(var).map_err(|_| format!("环境变量 `{var}` 未设置"));
    }
    let ws = ctx.workspace.to_string_lossy().into_owned();
    match name {
        "workspaceFolder" => Ok(ws),
        "file" => ctx
            .file_str()
            .ok_or_else(|| "${file} 需要一个当前文件".to_string()),
        "fileBasename" => ctx
            .file
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| "${fileBasename} 需要一个当前文件".to_string()),
        "fileDirname" => ctx
            .file
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.to_string_lossy().into_owned())
            .ok_or_else(|| "${fileDirname} 需要一个当前文件".to_string()),
        "fileBasenameNoExtension" => ctx
            .file
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| "${fileBasenameNoExtension} 需要一个当前文件".to_string()),
        "cwd" => Ok(ctx.cwd.to_string_lossy().into_owned()),
        "cargoBin" => ctx
            .cargo_bin
            .clone()
            .ok_or_else(|| "${cargoBin} 只在 Rust 项目里可用（没找到 Cargo.toml）".to_string()),
        "cargoProfile" => Ok(ctx.cargo_profile.clone()),
        // 只在 `build` 里展开（先算 program，再带着它展开 build）
        "program" => ctx
            .program
            .clone()
            .ok_or_else(|| "${program} 只能用在上启动的 `build` 命令里".to_string()),
        other => Err(format!(
            "未知变量 ${{{other}}}（支持: {} 与 ${{env:VAR}}）",
            VARS.join(" / ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> VarCtx {
        VarCtx {
            cargo_bin: Some("mycrate".to_string()),
            cargo_profile: "debug".to_string(),
            program: None,
            ..VarCtx::default()
        }
        .with_paths("/proj", Some("/proj/src/main.rs"))
    }

    // ── launch.toml ──

    #[test]
    fn toml_parses_minimal_config() {
        let (cfgs, warns) = parse_toml(
            r#"
            [[config]]
            name = "run"
            program = "${workspaceFolder}/target/debug/app"
            args = ["--flag", "x"]
            env = { RUST_BACKTRACE = "1" }
            "#,
        )
        .unwrap();
        assert_eq!(warns.len(), 0);
        assert_eq!(cfgs.len(), 1);
        let c = &cfgs[0];
        assert_eq!(c.name, "run");
        assert_eq!(c.request, "launch", "request 缺省 launch");
        assert_eq!(c.source, Source::Toml);
        assert_eq!(c.args["program"], "${workspaceFolder}/target/debug/app");
        assert_eq!(c.args["args"][1], "x");
        assert_eq!(c.args["env"]["RUST_BACKTRACE"], "1");
        assert!(c.args.get("name").is_none(), "name 不进请求参数");
    }

    #[test]
    fn toml_rejects_unknown_request_and_unsupported_keys() {
        let (cfgs, warns) = parse_toml(
            r#"
            [[config]]
            name = "bad-request"
            request = "wat"

            [[config]]
            name = "tasky"
            preLaunchTask = "cargo build"
            "#,
        )
        .unwrap();
        assert!(cfgs.is_empty(), "两条都该被拒: {cfgs:?}");
        assert_eq!(warns.len(), 2, "每种拒绝都要有告警: {warns:?}");
        assert!(warns[0].contains("launch/attach"));
        assert!(warns[1].contains("preLaunchTask"));
    }

    #[test]
    fn toml_top_level_compounds_is_hard_error() {
        let err = parse_toml("compounds = []\n").unwrap_err();
        assert!(err.contains("compounds"), "{err}");
    }

    // ── launch.json ──

    #[test]
    fn json_parses_vscode_subset_with_comments() {
        let (cfgs, warns) = parse_json(
            r#"
            {
              // VS Code 允许注释
              "version": "0.2.0",
              "configurations": [
                {
                  "type": "lldb-dap",
                  "request": "launch",
                  "name": "run crate",
                  "program": "${workspaceFolder}/target/debug/app",
                  "args": ["--flag"],
                  "cwd": "${workspaceFolder}" // 工作目录
                }
              ]
            }
            "#,
        )
        .unwrap();
        assert_eq!(warns.len(), 0);
        assert_eq!(cfgs.len(), 1);
        let c = &cfgs[0];
        assert_eq!(c.adapter.as_deref(), Some("lldb-dap"));
        assert_eq!(c.source, Source::Json);
        assert_eq!(c.args["args"][0], "--flag");
        assert!(c.args.get("type").is_none(), "type 不进请求参数");
        assert!(c.args.get("request").is_none());
    }

    #[test]
    fn json_url_inside_string_is_not_a_comment() {
        let (cfgs, _) = parse_json(
            r#"{"configurations":[{"name":"a","request":"launch","program":"http://x/y"}]}"#,
        )
        .unwrap();
        assert_eq!(cfgs[0].args["program"], "http://x/y");
    }

    #[test]
    fn json_compounds_is_hard_error() {
        let err = parse_json(r#"{"compounds":[],"configurations":[]}"#).unwrap_err();
        assert!(err.contains("compounds"), "{err}");
    }

    #[test]
    fn json_broken_syntax_is_error_not_panic() {
        assert!(parse_json("{ oops").is_err());
        assert!(parse_json("[]").is_err(), "顶层必须是对象");
    }

    // ── 变量替换 ──

    #[test]
    fn expand_supports_documented_vars() {
        let v = json!({
            "program": "${workspaceFolder}/target/debug/app",
            "args": ["--file", "${file}", "${fileBasename}", "${fileDirname}",
                     "${fileBasenameNoExtension}", "${cwd}"],
        });
        let out = expand(&v, &ctx()).unwrap();
        assert_eq!(out["program"], "/proj/target/debug/app");
        assert_eq!(out["args"][1], "/proj/src/main.rs");
        assert_eq!(out["args"][2], "main.rs");
        assert_eq!(out["args"][3], "/proj/src");
        assert_eq!(out["args"][4], "main");
        assert_eq!(out["args"][5], "/proj");
    }

    #[test]
    fn expand_env_and_unknown_var() {
        std::env::set_var("DAP_CFG_TEST", "yes");
        assert_eq!(
            expand(&json!("${env:DAP_CFG_TEST}"), &ctx()).unwrap(),
            "yes"
        );
        let err = expand(&json!("${nope}"), &ctx()).unwrap_err();
        assert!(err.contains("未知变量") && err.contains("nope"), "{err}");
        // 需要文件但没文件 ⇒ 报错（不静默留字面量）
        let no_file = VarCtx::default().with_paths("/proj", None::<&std::path::Path>);
        assert!(expand(&json!("${file}"), &no_file).is_err());
        // 没闭合的 ${ 原样保留（不算变量）
        assert_eq!(expand(&json!("a${b"), &ctx()).unwrap(), "a${b");
    }

    // ── 合并 ──

    #[test]
    fn toml_overrides_json_field_by_field() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join(".helix")).unwrap();
        std::fs::create_dir_all(ws.join(".vscode")).unwrap();
        std::fs::write(
            ws.join(".vscode/launch.json"),
            r#"{"configurations":[
                {"type":"lldb-dap","name":"run","request":"launch","program":"/old","args":["--a"],"cwd":"/old-cwd"},
                {"type":"lldb-dap","name":"json-only","request":"launch","program":"/j"}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            ws.join(".helix/launch.toml"),
            r#"
            [[config]]
            name = "run"
            program = "/new"
            "#,
        )
        .unwrap();

        let (cfgs, warns) = load(ws, Some("rust"), &[], &ctx());
        assert_eq!(warns, Vec::<String>::new());
        assert_eq!(cfgs.len(), 2);
        let run = cfgs.iter().find(|c| c.name == "run").unwrap();
        assert_eq!(run.source, Source::Toml, "同名 ⇒ 记成 toml");
        assert_eq!(run.args["program"], "/new", "toml 覆盖 program");
        assert_eq!(run.args["args"][0], "--a", "未覆盖字段沿用 json");
        assert_eq!(run.args["cwd"], "/old-cwd");
        assert!(cfgs.iter().any(|c| c.name == "json-only"));
    }

    #[test]
    fn lang_templates_are_the_fallback_and_never_override_files() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join(".helix")).unwrap();
        std::fs::write(
            ws.join(".helix/launch.toml"),
            "[[config]]\nname = \"binary\"\nprogram = \"/from-toml\"\n",
        )
        .unwrap();
        let templates = vec![
            LangTemplate {
                name: "binary".into(),
                request: "launch".into(),
                args: json!({"program": "/from-lang"}),
            },
            LangTemplate {
                name: "attach".into(),
                request: "attach".into(),
                args: json!({"pid": "{0}"}),
            },
        ];
        let (cfgs, _) = load(ws, Some("rust"), &templates, &ctx());
        let binary = cfgs.iter().find(|c| c.name == "binary").unwrap();
        assert_eq!(binary.args["program"], "/from-toml", "文件优先于语言模板");
        let attach = cfgs.iter().find(|c| c.name == "attach").unwrap();
        assert_eq!(attach.source, Source::Lang);
        assert_eq!(attach.request, "attach");
    }

    #[test]
    fn language_filter_skips_other_languages() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join(".helix")).unwrap();
        std::fs::write(
            ws.join(".helix/launch.toml"),
            r#"
            [[config]]
            name = "py"
            language = "python"
            program = "/py"
            [[config]]
            name = "rs"
            language = "rust"
            program = "/rs"
            "#,
        )
        .unwrap();
        let (cfgs, _) = load(ws, Some("rust"), &[], &ctx());
        assert_eq!(cfgs.len(), 1, "只留当前语言: {cfgs:?}");
        assert_eq!(cfgs[0].name, "rs");
    }

    #[test]
    fn bad_variable_skips_only_that_config() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(ws.join(".helix")).unwrap();
        std::fs::write(
            ws.join(".helix/launch.toml"),
            r#"
            [[config]]
            name = "broken"
            program = "${nope}"
            [[config]]
            name = "ok"
            program = "/ok"
            "#,
        )
        .unwrap();
        let (cfgs, warns) = load(ws, None, &[], &ctx());
        assert_eq!(cfgs.len(), 1, "坏变量只跳过那一条: {cfgs:?}");
        assert_eq!(cfgs[0].name, "ok");
        assert!(warns
            .iter()
            .any(|w| w.contains("broken") && w.contains("nope")));
    }
}
