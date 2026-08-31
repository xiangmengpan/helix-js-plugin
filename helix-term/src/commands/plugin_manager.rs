//! 插件管理器纯函数层:manifest 读写 + install 复制/回滚 + remove 删除。
//! 命令接线见任务 2(typed.rs)。

use anyhow::anyhow;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestEntry {
    pub source: String,
    pub kind: String, // "local" / "git"
    pub installed_at: String,
    #[serde(default)]
    pub commit: Option<String>, // git 源:clone/update 后的 HEAD hash;local 源 None
    #[serde(default)]
    pub pinned: bool, // true = update 跳过(批次 2 用)
    pub files: Vec<String>, // 相对 plugins/ 的复制目标
}

pub type Manifest = HashMap<String, ManifestEntry>;

pub fn manifest_path() -> PathBuf {
    helix_loader::config_dir()
        .join("plugins")
        .join("manifest.json")
}

/// 读 manifest;缺文件/坏 JSON → 空(不崩);IO 错误 warn + 空
pub fn read_manifest(path: &Path) -> anyhow::Result<Manifest> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Ok(Manifest::new());
    };
    match serde_json::from_str(&raw) {
        Ok(m) => Ok(m),
        Err(e) => {
            log::warn!("manifest parse failed: {e}");
            Ok(Manifest::new())
        }
    }
}

pub fn write_manifest(path: &Path, m: &Manifest) -> anyhow::Result<()> {
    let raw = serde_json::to_string_pretty(m)?;
    fs::write(path, raw).map_err(|e| anyhow!("write manifest: {e}"))
}

/// install 目标路径:文件 → features/<name>;目录 → features/<name>/
pub fn install_target(path: &Path) -> anyhow::Result<(String, PathBuf)> {
    let plugins = helix_loader::config_dir().join("plugins").join("features");
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("invalid path: '{}'", path.display()))?;
    let target = plugins.join(name);
    Ok((name.to_string(), target))
}

/// 复制安装(文件或目录递归);中途失败 → 回滚已复制的部分
pub fn install_copy(src: &Path, dst: &Path) -> anyhow::Result<Vec<String>> {
    let mut copied = Vec::new();
    let result = (|| -> anyhow::Result<()> {
        if src.is_dir() {
            for entry in walkdir(src)? {
                let rel = entry.strip_prefix(src)?;
                let to = dst.join(rel);
                if entry.is_dir() {
                    fs::create_dir_all(&to)?;
                } else {
                    if let Some(p) = to.parent() {
                        fs::create_dir_all(p)?;
                    }
                    fs::copy(&entry, &to)?;
                }
                copied.push(format!(
                    "features/{}/{}",
                    dst.file_name().unwrap().to_string_lossy(),
                    rel.to_string_lossy()
                ));
            }
        } else {
            // 全新配置首次安装:父目录可能不存在
            fs::create_dir_all(dst.parent().unwrap())?;
            fs::copy(src, dst)?;
            copied.push(format!(
                "features/{}",
                dst.file_name().unwrap().to_string_lossy()
            ));
        }
        Ok(())
    })();
    if result.is_err() {
        // 回滚:dst 是独占安装目标,整个删掉(不依赖 config_dir,保持纯函数契约)
        // ponytail: remove_dir_all 对单文件会 ENOTDIR,故按类型分派
        let _ = if dst.is_dir() {
            fs::remove_dir_all(dst)
        } else {
            fs::remove_file(dst)
        };
    }
    result?;
    Ok(copied)
}

/// 删除安装的文件/目录(逐个,忽略不存在)
pub fn remove_files(files: &[String]) {
    remove_files_from(&helix_loader::config_dir().join("plugins"), files);
}

/// 删除无 manifest 的残留:旧版单文件 plugins/<name> 或 features/<name> 孤儿(文件/目录都删);
/// 返回是否删到了东西(两处都没有 → false,调用方报 not found)。
/// 裸名校验防目录逃逸(features/ 孤儿可能是目录名如 filetree,不强制 .js 后缀)。
pub fn remove_orphan(plugins_dir: &Path, name: &str) -> anyhow::Result<bool> {
    // 裸名校验防目录逃逸 + 拒绝 "." / "features":两者会命中 plugins/ 或 features/ 本身,
    // remove_dir_all 会把全部插件删光——features/ 孤儿可能是目录名如 filetree,不强制 .js 后缀
    if name.is_empty()
        || name.contains('/')
        || name.starts_with("..")
        || name == "."
        || name == "features"
    {
        return Err(anyhow!("invalid plugin name: '{name}'"));
    }
    let mut removed = false;
    for p in [
        plugins_dir.join(name),
        plugins_dir.join("features").join(name),
    ] {
        if p.is_dir() {
            std::fs::remove_dir_all(&p)?;
            removed = true;
        } else if p.exists() {
            std::fs::remove_file(&p)?;
            removed = true;
        }
    }
    Ok(removed)
}

fn remove_files_from(base: &Path, files: &[String]) {
    for f in files {
        let p = base.join(f);
        if p.is_dir() {
            let _ = fs::remove_dir_all(p);
        } else {
            let _ = fs::remove_file(p);
        }
    }
}

fn walkdir(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let e = e?;
            let p = e.path();
            out.push(p.clone());
            if p.is_dir() {
                stack.push(p);
            }
        }
    }
    Ok(out)
}

/// git url 识别:含 :// 或 git@ 前缀或 .git 后缀
pub fn is_git_url(arg: &str) -> bool {
    arg.contains("://") || arg.starts_with("git@") || arg.ends_with(".git")
}

/// url → 插件名:basename 去 .git 后缀
pub fn name_from_url(url: &str) -> String {
    let base = url.rsplit('/').next().unwrap_or(url);
    base.strip_suffix(".git").unwrap_or(base).to_string()
}

/// git clone 到 dst;失败清理半成品目录
pub fn clone_to_vendor(url: &str, dst: &Path) -> anyhow::Result<()> {
    let status = std::process::Command::new("git")
        .args(["clone", url])
        .arg(dst)
        .status()
        .map_err(|e| anyhow!("git clone failed (is git installed?): {e}"))?;
    if !status.success() {
        let _ = fs::remove_dir_all(dst);
        return Err(anyhow!("git clone '{url}' failed with {status}"));
    }
    Ok(())
}

/// 当前 commit hash;失败 → None
pub fn git_head_commit(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

/// git fetch origin;失败 → Err
pub fn git_fetch(dir: &Path) -> anyhow::Result<()> {
    let status = std::process::Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(["fetch", "origin"])
        .status()
        .map_err(|e| anyhow!("git fetch failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("git fetch failed with {status}"))
    }
}

/// 当前 HEAD(Err 化版本,供 update 用)
pub fn git_head(dir: &Path) -> anyhow::Result<String> {
    git_head_commit(dir).ok_or_else(|| anyhow!("git rev-parse HEAD failed"))
}

/// origin/HEAD;失败 → Err(无远端跟踪等)
pub fn git_origin_head(dir: &Path) -> anyhow::Result<String> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(["rev-parse", "origin/HEAD"])
        .output()
        .map_err(|e| anyhow!("git rev-parse origin/HEAD failed: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(anyhow!("git rev-parse origin/HEAD failed"))
    }
}

/// ff-only pull;失败(冲突/非快进)→ Err,工作树保留旧版
pub fn git_pull_ff(dir: &Path) -> anyhow::Result<()> {
    let status = std::process::Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(["pull", "--ff-only"])
        .status()
        .map_err(|e| anyhow!("git pull failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("git pull --ff-only failed with {status}"))
    }
}

/// 更新一批条目。返回 (updated_names, up_to_date, skipped, failed)。
/// local/pinned 跳过;git 目录缺失 failed;fetch/head 读取失败 failed;
/// head 相同 up_to_date;不同 → ff pull,成功记入 updated_names(调用方更新 manifest commit)。
/// 本函数不改 manifest。
pub fn update_entries(
    entries: &[(String, ManifestEntry)],
    plugins_dir: &Path,
) -> (Vec<String>, usize, usize, usize) {
    let mut updated = Vec::new();
    let mut up = 0;
    let mut skipped = 0;
    let mut failed = 0;
    for (name, e) in entries {
        if e.kind != "git" || e.pinned {
            skipped += 1;
            continue;
        }
        let dir = plugins_dir.join("vendor").join(name);
        if !dir.is_dir() {
            failed += 1;
            continue;
        }
        match (git_fetch(&dir), git_head(&dir), git_origin_head(&dir)) {
            (Ok(()), Ok(local), Ok(origin)) if local == origin => up += 1,
            (Ok(()), Ok(_), Ok(_)) => match git_pull_ff(&dir) {
                Ok(()) => updated.push(name.clone()),
                Err(_) => failed += 1,
            },
            _ => failed += 1,
        }
    }
    (updated, up, skipped, failed)
}

/// 更新 manifest 中已更新条目的 commit(update 成功后调用)
pub fn refresh_commits(manifest: &mut Manifest, plugins_dir: &Path, names: &[String]) {
    for n in names {
        if let Some(e) = manifest.get_mut(n) {
            if let Ok(h) = git_head(&plugins_dir.join("vendor").join(n)) {
                e.commit = Some(h);
            }
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PluginDep {
    pub name: String,
    pub git: String,
}

/// plugin.json 解析:deps 可选;坏 JSON/缺 name/git 字段 → Err;缺文件由调用方处理
pub fn parse_plugin_json(raw: &str) -> anyhow::Result<Vec<PluginDep>> {
    #[derive(serde::Deserialize)]
    struct PluginJson {
        #[serde(default)]
        deps: Vec<PluginDep>,
    }
    let parsed: PluginJson = serde_json::from_str(raw)?;
    Ok(parsed.deps)
}

/// 循环检测:name 已在栈(任意位置)→ 循环
pub fn check_cycle(stack: &[String], name: &str) -> bool {
    stack.iter().any(|s| s == name)
}

/// 深度限制:depth(当前层数,0 = 根)<= max 放行
pub fn depth_ok(depth: usize, max: usize) -> bool {
    depth < max
}

/// 递归安装依赖(含本体)。先装依赖再装本体;已装(manifest/visited)跳过;
/// 循环/超深 → Err;任一依赖失败 → Err(已装的保留,不回滚)。
/// manifest 在本体依赖全部成功后写入。
#[allow(clippy::too_many_arguments)]
pub fn install_with_deps(
    name: &str,
    git_url: &str,
    plugins_dir: &Path,
    manifest: &mut Manifest,
    visited: &mut Vec<String>,
    stack: &mut Vec<String>,
    depth: usize,
) -> anyhow::Result<()> {
    if !depth_ok(depth, 10) {
        return Err(anyhow!("dependency depth exceeds 10 at '{name}'"));
    }
    if check_cycle(stack, name) {
        return Err(anyhow!(
            "circular dependency: {} -> {name}",
            stack.join(" -> ")
        ));
    }
    if manifest.contains_key(name) || visited.contains(&name.to_string()) {
        return Ok(()); // 已装/处理中 → 跳过
    }
    // clone 本体(vendor/<name>)
    let vendor_dir = plugins_dir.join("vendor");
    let target = vendor_dir.join(name);
    if !target.exists() {
        fs::create_dir_all(&vendor_dir)?;
        clone_to_vendor(git_url, &target)?;
    }
    // 读本体的 plugin.json(无文件 = 无依赖)
    let deps = match fs::read_to_string(target.join("plugin.json")) {
        Ok(raw) => parse_plugin_json(&raw)?,
        Err(_) => Vec::new(),
    };
    stack.push(name.to_string());
    for dep in &deps {
        install_with_deps(
            &dep.name,
            &dep.git,
            plugins_dir,
            manifest,
            visited,
            stack,
            depth + 1,
        )?;
    }
    stack.pop();
    // 本体记入 manifest(依赖全部成功后)
    manifest.insert(
        name.to_string(),
        ManifestEntry {
            source: git_url.to_string(),
            kind: "git".into(),
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_default(),
            commit: git_head_commit(&target),
            pinned: false,
            files: vec![format!("vendor/{name}/")],
        },
    );
    visited.push(name.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn manifest_read_write_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        // 缺文件 → 空
        let m = read_manifest(&path).unwrap();
        assert!(m.is_empty());
        // 写读回
        let entry = ManifestEntry {
            source: "./x".into(),
            kind: "local".into(),
            installed_at: "t".into(),
            commit: None,
            pinned: false,
            files: vec!["features/x.js".into()],
        };
        write_manifest(&path, &HashMap::from([("x".to_string(), entry.clone())])).unwrap();
        let m = read_manifest(&path).unwrap();
        assert_eq!(m.get("x").unwrap().source, "./x");
    }

    #[test]
    fn manifest_bad_json_is_empty_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        std::fs::write(&path, "not json {{{").unwrap();
        let m = read_manifest(&path).unwrap(); // 坏 JSON → 空,不崩
        assert!(m.is_empty());
    }

    #[test]
    fn install_copy_dir_then_remove_files() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("plug");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("main.js"), "x").unwrap();
        fs::write(src.join("sub").join("a.js"), "y").unwrap();
        // dst 在临时目录的 plugins 根下:plugins/features/plug
        let plugins = dir.path().join("plugins");
        let dst = plugins.join("features").join("plug");
        let files = install_copy(&src, &dst).unwrap();
        assert!(files.iter().any(|f| f.ends_with("main.js")));
        assert!(dst.join("main.js").exists());
        assert!(dst.join("sub").join("a.js").exists());
        remove_files_from(&plugins, &files);
        assert!(!dst.join("main.js").exists());
        assert!(!dst.join("sub").exists());
    }

    #[test]
    fn install_copy_single_file_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("plug.js");
        fs::write(&src, "x").unwrap();
        // 全新配置首次单文件安装:dst 父目录不存在
        let dst = dir.path().join("plugins").join("features").join("plug.js");
        let files = install_copy(&src, &dst).unwrap();
        assert_eq!(files, vec!["features/plug.js"]);
        assert!(dst.exists());
    }

    #[test]
    fn install_copy_failure_rolls_back_dst() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("plug");
        fs::create_dir_all(src.join("bad")).unwrap();
        fs::write(src.join("main.js"), "x").unwrap();
        fs::write(src.join("bad").join("a.js"), "y").unwrap();
        // 注入失败:dst/bad 预置为文件 → 复制时 create_dir_all(dst/bad) 必失败
        let dst = dir.path().join("plugins").join("features").join("plug");
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("bad"), "poison").unwrap();
        assert!(install_copy(&src, &dst).is_err());
        assert!(!dst.exists());
    }

    #[test]
    fn remove_orphan_deletes_legacy_file_and_features_dir() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join("plugins");
        fs::create_dir_all(&plugins).unwrap();
        // 旧版单文件安装:plugins/<name>
        fs::write(plugins.join("pt.js"), "x").unwrap();
        // features/ 孤儿目录(无 manifest,如 write_manifest 失败残留)
        fs::create_dir_all(plugins.join("features").join("filetree")).unwrap();
        fs::write(
            plugins.join("features").join("filetree").join("index.js"),
            "x",
        )
        .unwrap();

        assert!(remove_orphan(&plugins, "filetree").unwrap());
        assert!(!plugins.join("features").join("filetree").exists());
        assert!(remove_orphan(&plugins, "pt.js").unwrap());
        assert!(!plugins.join("pt.js").exists());
        // 两处都没有 → false(调用方报 not found)
        assert!(!remove_orphan(&plugins, "nope.js").unwrap());
        // 目录逃逸拒绝
        assert!(remove_orphan(&plugins, "../evil.js").is_err());
        assert!(remove_orphan(&plugins, "a/b.js").is_err());
        // 容器目录名拒绝:命中 plugins/ 或 features/ 本身会 remove_dir_all 删光全部插件
        assert!(remove_orphan(&plugins, ".").is_err());
        assert!(plugins.exists(), ". 不应删掉 plugins 目录");
        assert!(remove_orphan(&plugins, "features").is_err());
        assert!(plugins.join("features").exists(), "features 不应被删");
    }

    #[test]
    fn url_identification_and_name() {
        assert!(is_git_url("https://github.com/foo/bar.git"));
        assert!(is_git_url("git@github.com:foo/bar.git"));
        assert!(is_git_url("https://github.com/foo/bar")); // 无 .git 也是 url(含 ://)
        assert!(!is_git_url("./local/plugin.js"));
        assert!(!is_git_url("/abs/path/plugin"));
        assert!(!is_git_url("plain-name")); // 非 url 非路径 → 调用方报错
        assert_eq!(name_from_url("https://github.com/foo/bar.git"), "bar");
        assert_eq!(name_from_url("git@github.com:foo/bar.git"), "bar");
        assert_eq!(name_from_url("https://github.com/foo/baz"), "baz");
    }

    #[test]
    fn manifest_git_fields_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        let entry = ManifestEntry {
            source: "https://github.com/foo/bar.git".into(),
            kind: "git".into(),
            installed_at: "t".into(),
            commit: Some("a1b2c3d".into()),
            pinned: true,
            files: vec!["vendor/bar/".into()],
        };
        write_manifest(&path, &HashMap::from([("bar".to_string(), entry.clone())])).unwrap();
        let m = read_manifest(&path).unwrap();
        let got = m.get("bar").unwrap();
        assert_eq!(got.kind, "git");
        assert_eq!(got.commit.as_deref(), Some("a1b2c3d"));
        assert!(got.pinned);
        // 缺省字段(一期条目无 commit/pinned):反序列化后 None/false
        let legacy = r#"{"x": {"source": "./x", "kind": "local", "installed_at": "t", "files": ["features/x.js"]}}"#;
        let m2: Manifest = serde_json::from_str(legacy).unwrap();
        assert_eq!(m2["x"].commit, None);
        assert!(!m2["x"].pinned);
    }

    #[test]
    fn clone_failure_returns_err() {
        // 假 url(本地端口 1,连接拒绝 → 失败快);失败清理半成品目录
        let dir = std::env::temp_dir().join(format!("hx_clone_fail_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert!(clone_to_vendor("https://127.0.0.1:1/nope/nope.git", &dir).is_err());
        assert!(!dir.exists(), "clone 失败应清理半成品目录");
    }

    #[test]
    fn update_skips_local_and_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let entries = vec![
            (
                "loc".to_string(),
                ManifestEntry {
                    source: "./x".into(),
                    kind: "local".into(),
                    installed_at: "t".into(),
                    commit: None,
                    pinned: false,
                    files: vec!["features/x.js".into()],
                },
            ),
            (
                "pin".to_string(),
                ManifestEntry {
                    source: "https://g/p.git".into(),
                    kind: "git".into(),
                    installed_at: "t".into(),
                    commit: Some("a".into()),
                    pinned: true,
                    files: vec!["vendor/p/".into()],
                },
            ),
        ];
        let (updated, up, skipped, failed) = update_entries(&entries, &dir.path().join("plugins"));
        assert!(updated.is_empty());
        assert_eq!(up, 0);
        assert_eq!(skipped, 2); // local + pinned
        assert_eq!(failed, 0);
    }

    #[test]
    fn update_missing_dir_fails_entry() {
        let dir = tempfile::tempdir().unwrap();
        let entries = vec![(
            "gone".to_string(),
            ManifestEntry {
                source: "https://g/g.git".into(),
                kind: "git".into(),
                installed_at: "t".into(),
                commit: Some("a".into()),
                pinned: false,
                files: vec!["vendor/gone/".into()],
            },
        )];
        let (updated, _up, _skipped, failed) =
            update_entries(&entries, &dir.path().join("plugins"));
        assert!(updated.is_empty());
        assert_eq!(failed, 1);
    }

    #[test]
    fn parse_plugin_json_deps() {
        // 合法:deps 提取
        let deps =
            parse_plugin_json(r#"{"deps": [{"name": "a", "git": "https://g/a.git"}]}"#).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "a");
        // 无 deps 字段 → 空
        assert!(parse_plugin_json(r#"{}"#).unwrap().is_empty());
        // 坏 JSON → Err
        assert!(parse_plugin_json("not json").is_err());
        // 缺 name/git → Err
        assert!(parse_plugin_json(r#"{"deps": [{"name": "a"}]}"#).is_err());
        assert!(parse_plugin_json(r#"{"deps": [{"git": "x"}]}"#).is_err());
    }

    #[test]
    fn cycle_and_depth_checks() {
        // 循环:name 已在栈(任意位置)
        let stack = vec!["A".to_string(), "B".to_string()];
        assert!(check_cycle(&stack, "A"));
        assert!(check_cycle(&stack, "B"));
        assert!(!check_cycle(&stack, "C"));
        // 深度限制
        assert!(depth_ok(0, 10));
        assert!(depth_ok(9, 10));
        assert!(!depth_ok(10, 10));
    }
}
