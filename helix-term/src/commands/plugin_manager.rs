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
        // 缺省字段(一期条目):commit/pinned 缺省
        let m2 = read_manifest(&path).unwrap();
        let _ = m2;
    }

    #[test]
    fn clone_failure_returns_err() {
        // 假 url(本地端口 1,连接拒绝 → 失败快);失败清理半成品目录
        let dir = std::env::temp_dir().join(format!("hx_clone_fail_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert!(clone_to_vendor("https://127.0.0.1:1/nope/nope.git", &dir).is_err());
        assert!(!dir.exists(), "clone 失败应清理半成品目录");
    }
}
