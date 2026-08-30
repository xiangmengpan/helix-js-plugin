//! 插件管理器纯函数层:manifest 读写 + install 复制/回滚 + remove 删除。
//! 命令接线见任务 2(typed.rs)。

use anyhow::anyhow;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestEntry {
    pub source: String,
    pub kind: String, // "local"(二期 "git")
    pub installed_at: String,
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
}
