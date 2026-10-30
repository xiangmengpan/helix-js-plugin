//! 项目检测：从"当前文件 + 工作区"推出 `(build 命令, program 路径)`。
//!
//! 为什么要有它:语言模板的 `completion = filename` 让用户手填 `target/debug/<bin>` ——
//! 而这些信息**完全可以推出来**（`Cargo.toml` / `CMakeLists.txt` / `Makefile` / 单文件 C…）。
//! 设计见 `docs/superpowers/specs/2026-10-26-dap-autobuild-and-restart-design.md` §3。
//!
//! **纯函数**（只读文件/元数据，不执行任何命令），所以能单测；
//! 歧义（多个候选产物）一律**列候选**，不静默猜。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 检测结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detected {
    /// 构建命令（`None` = 这个语言不需要构建，如 python/node）
    pub build: Option<String>,
    /// 产出的可执行文件（可能是候选之一）
    pub program: Option<PathBuf>,
    /// 候选（歧义时 >1；`program` 是其中最新的那个）
    pub candidates: Vec<PathBuf>,
    /// 人读的来源说明，展示在 Picker/提示里
    pub reason: String,
}

impl Detected {
    fn none(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            ..Default::default()
        }
    }
}

/// 检测入口。`file` = 当前缓冲（可为空，如纯目录场景）。
pub fn detect(file: Option<&Path>, workspace: &Path, release: bool) -> Detected {
    if let Some(d) = detect_cargo(file, workspace, release) {
        return d;
    }
    if let Some(d) = detect_cmake(workspace) {
        return d;
    }
    if let Some(d) = detect_meson(workspace) {
        return d;
    }
    if let Some(d) = detect_make(workspace) {
        return d;
    }
    if let Some(d) = detect_source(file, workspace) {
        return d;
    }
    if let Some(d) = detect_go(file, workspace) {
        return d;
    }
    if let Some(d) = detect_script(file) {
        return d;
    }
    Detected::none("没认出项目类型（可用 launch.toml 的 build/program 显式指定）")
}

// ============================ Rust / Cargo ============================

fn detect_cargo(file: Option<&Path>, workspace: &Path, release: bool) -> Option<Detected> {
    // 从当前文件往上找最近的 Cargo.toml（但不越过 workspace）
    let crate_root = file
        .and_then(|f| nearest_manifest(f, workspace, "Cargo.toml"))
        .or_else(|| is_file(&workspace.join("Cargo.toml")).then(|| workspace.to_path_buf()))?;
    let manifest = crate_root.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest).ok()?;
    let bin = cargo_bin_name(&text)?;
    // cargo 的 target 目录在**实际调用 cargo 的目录**（工作区根；单 crate 时就是 crate 根）。
    // 两种情况取值相同 ⇒ 不需要分支（早先是 if/else 两个一样的块，clippy 报 identical blocks）。
    let target_root = workspace.join("target");
    let profile = if release { "release" } else { "debug" };
    let program = target_root.join(profile).join(&bin);
    let mut build = String::from("cargo build");
    if release {
        build.push_str(" --release");
    }
    Some(Detected {
        build: Some(build),
        program: Some(program.clone()),
        candidates: vec![program],
        reason: format!(
            "Cargo.toml · crate {} · bin {}",
            crate_root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "?".into()),
            bin
        ),
    })
}

/// `[[bin]].name` 优先，其次 `[package].name`（`name` 里的 `-` 不会变成 `_`）。
fn cargo_bin_name(text: &str) -> Option<String> {
    let value: toml::Value = toml::from_str(text).ok()?;
    if let Some(bins) = value.get("bin").and_then(|b| b.as_array()) {
        for b in bins {
            if let Some(name) = b.get("name").and_then(|n| n.as_str()) {
                return Some(name.to_string());
            }
        }
    }
    value
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

// ============================ Meson ============================

/// `meson.build` → `meson setup build`（首次）+ `meson compile -C build`；
/// 产物从 `build/` 下探测（meson 默认把可执行放在 build 根）。
fn detect_meson(workspace: &Path) -> Option<Detected> {
    if !is_file(&workspace.join("meson.build")) {
        return None;
    }
    let build_dir = workspace.join("build");
    let configured = build_dir.join("build.ninja").is_file()
        || build_dir.join("build.ninja").exists()
        || build_dir.join("meson-info").is_dir();
    let (build, reason) = if configured {
        (
            "meson compile -C build".to_string(),
            "meson.build · 已有 build/".to_string(),
        )
    } else {
        (
            "meson setup build && meson compile -C build".to_string(),
            "meson.build（首次配置）".to_string(),
        )
    };
    let candidates = if configured {
        executables_under(&build_dir)
    } else {
        Vec::new()
    };
    Some(Detected {
        build: Some(build),
        program: candidates.first().cloned(),
        candidates,
        reason,
    })
}

// ============================ CMake ============================

fn detect_cmake(workspace: &Path) -> Option<Detected> {
    if !is_file(&workspace.join("CMakeLists.txt")) {
        return None;
    }
    let build_dir = ["build", "cmake-build-debug", "cmake-build-release"]
        .iter()
        .map(|d| workspace.join(d))
        .find(|d| d.is_dir());
    let (build, reason) = match &build_dir {
        Some(d) => (
            format!("cmake --build {}", shell_quote(d)),
            format!("CMakeLists.txt · 已有构建目录 {}", d.display()),
        ),
        None => (
            "cmake -S . -B build && cmake --build build".to_string(),
            "CMakeLists.txt（首次配置）".to_string(),
        ),
    };
    let candidates = build_dir
        .as_deref()
        .map(executables_under)
        .unwrap_or_default();
    Some(Detected {
        build: Some(build),
        program: candidates.first().cloned(),
        candidates,
        reason,
    })
}

/// 递归找可执行文件（跳过 CMake 自己的 `CMakeFiles/`、`.git/` 等）。
fn executables_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        if depth > 3 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if matches!(
                    name.as_ref(),
                    "CMakeFiles" | ".git" | "node_modules" | "target"
                ) {
                    continue;
                }
                stack.push((path, depth + 1));
            } else if is_executable(&path) && !name.ends_with(".so") && !name.contains('.') {
                out.push(path);
            }
        }
    }
    out.sort_by_key(|p| std::cmp::Reverse(mtime(p)));
    out
}

// ============================ Make ============================

fn detect_make(workspace: &Path) -> Option<Detected> {
    let has_make = ["Makefile", "makefile", "GNUmakefile"]
        .iter()
        .any(|f| is_file(&workspace.join(f)));
    if !has_make {
        return None;
    }
    let mut candidates = executables_under(workspace);
    candidates.retain(|p| p.parent() == Some(workspace) || p.starts_with(workspace.join("bin")));
    Some(Detected {
        build: Some("make".to_string()),
        program: candidates.first().cloned(),
        candidates,
        reason: "Makefile".to_string(),
    })
}

// ============================ 单文件 C / C++ ============================

fn detect_source(file: Option<&Path>, workspace: &Path) -> Option<Detected> {
    let file = file?;
    let ext = file.extension()?.to_string_lossy().to_lowercase();
    let compiler = match ext.as_str() {
        "c" => "cc",
        "cc" | "cpp" | "cxx" => "c++",
        _ => return None,
    };
    let stem = file.file_stem()?.to_string_lossy().into_owned();
    let dir = file.parent().unwrap_or(workspace);
    let program = dir.join(&stem);
    // `-gdwarf-4`：本机 lldb(10) 读不了 gcc11 默认的 DWARF5（DW_FORM 0x1f）⇒ 断点会
    // 静默"pending"。用 DWARF4 保证"点一下就能断"。需要 DWARF5 就自己写 launch.toml 的 build。
    let build = format!(
        "{compiler} -g -gdwarf-4 -O0 -o {} {}",
        shell_quote(&program),
        shell_quote(file)
    );
    Some(Detected {
        build: Some(build),
        program: Some(program.clone()),
        candidates: vec![program],
        reason: format!("单文件 {}（无构建系统）", file.display()),
    })
}

// ============================ Go ============================

fn detect_go(file: Option<&Path>, workspace: &Path) -> Option<Detected> {
    if !is_file(&workspace.join("go.mod")) {
        return None;
    }
    let dir_name = file
        .and_then(|f| f.parent())
        .filter(|d| d.starts_with(workspace))
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .or_else(|| {
            workspace
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "app".into());
    let program = workspace.join(&dir_name);
    Some(Detected {
        build: Some(format!("go build -o {} .", shell_quote(&program))),
        program: Some(program.clone()),
        candidates: vec![program],
        reason: "go.mod".to_string(),
    })
}

// ============================ 脚本语言（无需二进制） ============================

fn detect_script(file: Option<&Path>) -> Option<Detected> {
    let file = file?;
    let ext = file.extension()?.to_string_lossy().to_lowercase();
    if matches!(ext.as_str(), "py" | "js" | "mjs" | "ts") {
        return Some(Detected {
            build: None,
            program: Some(file.to_path_buf()),
            candidates: vec![file.to_path_buf()],
            reason: "脚本语言（直接跑当前文件）".to_string(),
        });
    }
    None
}

// ============================ 小工具 ============================

fn is_file(p: &Path) -> bool {
    p.is_file()
}

fn nearest_manifest(start: &Path, stop_at: &Path, name: &str) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        if is_file(&dir.join(name)) {
            return Some(dir);
        }
        if dir == stop_at || !dir.starts_with(stop_at) {
            return None;
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn mtime(p: &Path) -> SystemTime {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
        && matches!(
            p.extension().and_then(|e| e.to_str()),
            Some("exe") | Some("cmd") | Some("bat")
        )
}

/// 单引号包裹（POSIX shell）。Windows 上由执行器换成 `cmd /C` 能接受的写法。
fn shell_quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        format!("\"{s}\"")
    } else if s.contains(|c: char| c.is_whitespace() || "'\"$`&|;()<>*?[]~#".contains(c)) {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.into_owned()
    }
}

// ============================ 首次构建落盘 launch.toml ============================

/// 生成 `.helix/launch.toml` 的内容。
///
/// 为什么要有它：语言模板里没有 `build`，而 `l`（`launch_or_build`）只做到
/// "产物缺失才构建" ⇒ 改了源码再按 `l`，跑的还是旧二进制（DWARF 行号表过期，
/// 断点/单步会落在旧行号上）。首次构建时把检测结果落成**显式配置**，
/// 下次 `l` 就走「显式 `build` ⇒ 每次启动前都构建」那条路（cargo 增量，通常很快）。
///
/// 纯函数（只拼字符串）：返回的文本必须能被 `dap_config::parse_toml` 原样读回，
/// 单测钉住这一条 —— 写了读不回来的配置，等于在用户项目里埋一份坏文件。
pub fn launch_toml_document(
    name: &str,
    language: Option<&str>,
    program: &Path,
    cwd: &Path,
    build: &str,
) -> String {
    let mut out = String::new();
    out.push_str("# 由 helix 在首次调试构建时自动生成（helix-term/src/dap_build.rs）。\n");
    out.push_str("# `build` 会在**每次**启动前执行（cargo 是增量的，通常很快）；\n");
    out.push_str("# 想换构建方式/产物路径就改这里：它比 .vscode/launch.json 和语言模板优先。\n");
    out.push_str("[[config]]\n");
    out.push_str(&format!("name = {}\n", toml_string(name)));
    out.push_str("request = \"launch\"\n");
    if let Some(language) = language {
        out.push_str(&format!("language = {}\n", toml_string(language)));
    }
    out.push_str(&format!(
        "program = {}\n",
        toml_string(&program.to_string_lossy())
    ));
    out.push_str(&format!("cwd = {}\n", toml_string(&cwd.to_string_lossy())));
    out.push_str(&format!("build = {}\n", toml_string(build)));
    out
}

/// TOML 基本字符串字面量（转义 `\`、`"` 与控制字符；其余原样，中文不用管）。
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `.helix/launch.toml` 的路径。
pub fn launch_toml_path(workspace: &Path) -> PathBuf {
    workspace.join(".helix").join("launch.toml")
}

/// 工作区里已有 `launch.toml`，或有优先级**更低**的 `.vscode/launch.json`。
///
/// 后者也得算"别动"：`launch.toml` 优先于 `launch.json`（见 `dap_config::load`），
/// 自动生成一份就会把用户现有的 VS Code 配置**遮蔽**掉。
fn existing_config(workspace: &Path) -> bool {
    launch_toml_path(workspace).is_file() || workspace.join(".vscode/launch.json").is_file()
}

/// **不存在才写**：已有 `launch.toml` 或 `.vscode/launch.json` 就返回 `Ok(None)`，用户手里的
/// 配置一个字都不动。
///
/// 用 `create_new` 而不是 `exists()` + `write()`：并发/竞态下也不会抹掉别人刚写的内容
/// （构建是异步的，两次 `l` 挨得很近时确实会撞）。
pub fn write_launch_toml_if_absent(
    workspace: &Path,
    doc: &str,
) -> std::io::Result<Option<PathBuf>> {
    use std::io::Write;
    if existing_config(workspace) {
        return Ok(None);
    }
    let path = launch_toml_path(workspace);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
        Err(err) => return Err(err),
    };
    file.write_all(doc.as_bytes())?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn launch_toml_document_roundtrips_through_parser() {
        let doc = launch_toml_document(
            "temp",
            Some("rust"),
            Path::new("/home/u/p/target/debug/temp"),
            Path::new("/home/u/p"),
            "cargo build",
        );
        let (configs, warnings) = crate::dap_config::parse_toml(&doc).expect("parse");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(configs.len(), 1);
        let cfg = &configs[0];
        assert_eq!(cfg.name, "temp");
        assert_eq!(cfg.request, "launch");
        assert_eq!(cfg.language.as_deref(), Some("rust"));
        assert_eq!(cfg.build.as_deref(), Some("cargo build"));
        assert_eq!(cfg.args["program"], "/home/u/p/target/debug/temp");
        assert_eq!(cfg.args["cwd"], "/home/u/p");
    }

    #[test]
    fn launch_toml_document_escapes_quotes_and_backslashes() {
        let doc = launch_toml_document(
            "a\"b",
            None,
            Path::new(r"C:\tmp\x y"),
            Path::new(r"C:\tmp"),
            "echo \"hi\"",
        );
        let (configs, warnings) = crate::dap_config::parse_toml(&doc).expect("parse");
        assert!(warnings.is_empty(), "{warnings:?}");
        let cfg = &configs[0];
        assert_eq!(cfg.name, "a\"b");
        assert_eq!(cfg.args["program"], r"C:\tmp\x y");
        assert_eq!(cfg.build.as_deref(), Some("echo \"hi\""));
        assert!(cfg.language.is_none());
    }

    #[test]
    fn write_launch_toml_never_overwrites() {
        let d = tmp();
        let first = write_launch_toml_if_absent(d.path(), "[[config]]\nname = \"mine\"\n").unwrap();
        assert_eq!(first, Some(launch_toml_path(d.path())));
        let second =
            write_launch_toml_if_absent(d.path(), "[[config]]\nname = \"other\"\n").unwrap();
        assert_eq!(second, None, "已存在的配置不能被覆盖");
        let text = std::fs::read_to_string(launch_toml_path(d.path())).unwrap();
        assert!(text.contains("mine"), "{text}");
        assert!(!text.contains("other"), "{text}");
    }

    #[test]
    fn write_launch_toml_skips_workspace_with_vscode_config() {
        let d = tmp();
        std::fs::create_dir_all(d.path().join(".vscode")).unwrap();
        std::fs::write(d.path().join(".vscode/launch.json"), "{}").unwrap();
        let got = write_launch_toml_if_absent(d.path(), "[[config]]\nname = \"x\"\n").unwrap();
        assert_eq!(got, None, "不能生成一份遮蔽 launch.json 的配置");
        assert!(!launch_toml_path(d.path()).exists());
    }

    #[test]
    fn cargo_workspace_bin_name_from_bin_section() {
        let d = tmp();
        std::fs::write(
            d.path().join("Cargo.toml"),
            "[package]\nname = \"mypkg\"\nversion = \"0.1.0\"\n\n[[bin]]\nname = \"mybin\"\npath = \"src/main.rs\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/main.rs"), "fn main() {}").unwrap();

        let got = detect(Some(&d.path().join("src/main.rs")), d.path(), false);
        assert_eq!(got.build.as_deref(), Some("cargo build"));
        assert_eq!(got.program, Some(d.path().join("target/debug/mybin")));
        assert!(got.reason.contains("mybin"), "{}", got.reason);
    }

    #[test]
    fn cargo_falls_back_to_package_name_and_release_profile() {
        let d = tmp();
        std::fs::write(
            d.path().join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let got = detect(None, d.path(), true);
        assert_eq!(got.build.as_deref(), Some("cargo build --release"));
        assert_eq!(got.program, Some(d.path().join("target/release/app")));
    }

    #[test]
    fn cargo_uses_nearest_manifest_in_workspace() {
        let d = tmp();
        std::fs::write(
            d.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\"]\n",
        )
        .unwrap();
        let a = d.path().join("a");
        std::fs::create_dir_all(a.join("src")).unwrap();
        std::fs::write(
            a.join("Cargo.toml"),
            "[package]\nname = \"a\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let f = a.join("src/main.rs");
        std::fs::write(&f, "fn main() {}").unwrap();
        let got = detect(Some(&f), d.path(), false);
        assert_eq!(got.program, Some(d.path().join("target/debug/a")));
        assert!(got.reason.contains("crate a"), "{}", got.reason);
    }

    #[test]
    fn single_c_file_derives_cc_with_dwarf4() {
        let d = tmp();
        let f = d.path().join("hello.c");
        std::fs::write(&f, "int main(){return 0;}").unwrap();
        let got = detect(Some(&f), d.path(), false);
        let build = got.build.expect("应推导出构建命令");
        assert!(build.starts_with("cc -g -gdwarf-4 -O0 -o "), "{build}");
        assert!(build.ends_with("hello.c"), "{build}");
        assert_eq!(got.program, Some(d.path().join("hello")));
    }

    #[test]
    fn python_needs_no_build_and_program_is_the_file() {
        let d = tmp();
        let f = d.path().join("app.py");
        std::fs::write(&f, "print(1)").unwrap();
        let got = detect(Some(&f), d.path(), false);
        assert!(got.build.is_none(), "脚本语言不需要构建");
        assert_eq!(got.program, Some(f));
    }

    #[test]
    fn go_mod_builds_into_workspace_named_binary() {
        let d = tmp();
        std::fs::write(d.path().join("go.mod"), "module x\n").unwrap();
        let f = d.path().join("main.go");
        std::fs::write(&f, "package main").unwrap();
        let got = detect(Some(&f), d.path(), false);
        assert!(got.build.as_deref().unwrap().starts_with("go build -o "));
        assert!(got.program.is_some());
    }

    #[test]
    fn cmake_without_build_dir_configures_first() {
        let d = tmp();
        std::fs::write(
            d.path().join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.10)",
        )
        .unwrap();
        let got = detect(None, d.path(), false);
        assert_eq!(
            got.build.as_deref(),
            Some("cmake -S . -B build && cmake --build build")
        );
        assert!(got.program.is_none(), "还没构建 ⇒ 没有候选");
    }

    #[test]
    fn cmake_with_build_dir_finds_executables() {
        let d = tmp();
        std::fs::write(d.path().join("CMakeLists.txt"), "x").unwrap();
        let b = d.path().join("build");
        std::fs::create_dir_all(&b).unwrap();
        let exe = b.join("app");
        std::fs::write(&exe, "bin").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let got = detect(None, d.path(), false);
        assert!(got.build.as_deref().unwrap().contains("cmake --build"));
        assert_eq!(got.program, Some(exe));
    }

    #[test]
    fn unknown_project_reports_reason_not_panic() {
        let d = tmp();
        let got = detect(None, d.path(), false);
        assert!(got.build.is_none());
        assert!(got.reason.contains("没认出"), "{}", got.reason);
    }
}

#[cfg(test)]
mod meson_tests {
    use super::*;

    #[test]
    fn detects_meson_and_prefers_compile_when_configured() {
        let root = std::env::temp_dir().join(format!("hx-meson-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("meson.build"), "project('demo', 'c')").unwrap();

        let d = detect(None, &root, false);
        assert_eq!(
            d.build.as_deref(),
            Some("meson setup build && meson compile -C build")
        );
        assert!(d.reason.contains("meson.build"));

        // 已有 build/（meson-info 目录是配置完成的标志）⇒ 只 compile
        std::fs::create_dir_all(root.join("build/meson-info")).unwrap();
        let d2 = detect(None, &root, false);
        assert_eq!(d2.build.as_deref(), Some("meson compile -C build"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
