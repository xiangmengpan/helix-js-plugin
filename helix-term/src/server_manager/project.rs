//! 项目级"期望状态"：`.helix/servers.toml`。
//!
//! 为什么需要：`languages.toml` 是**用户级**的，团队无法把"这个项目要哪些 server、
//! 钉哪个版本"放进仓库；换机器/新人入职只能凭记忆装。这里读一份**项目内**的清单，
//! 交给 `:server sync` 去对齐（装/卸/接线）。
//!
//! 安全：只读清单 + 只调用既有的 install/remove（都走 sha256 校验），
//! **不**因为项目里出现配置就执行任意命令（这一点和 `[server-manager.registry]` 的自定义 url
//! 不同 —— 那份需要用户显式确认）。

use anyhow::{Context as _, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// `.helix/servers.toml` 的内容。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Desired {
    /// 要装的 server 名（顺序无关）
    pub servers: Vec<String>,
    /// 钉住的版本（`[version]` 段）
    pub versions: BTreeMap<String, String>,
}

/// 清单文件的路径（`SM_PROJECT_CONFIG` 可覆写，测试/演示用）。
pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("SM_PROJECT_CONFIG") {
        return PathBuf::from(p);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.join(".helix").join("servers.toml")
}

/// 读清单。文件不存在 ⇒ `Ok(None)`（**不是错误**：绝大多数项目没有这份文件）。
pub fn desired() -> Result<Option<Desired>> {
    let path = config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("读 {}", path.display())),
    };
    let v: toml::Value =
        toml::from_str(&text).with_context(|| format!("解析 {}", path.display()))?;
    let mut servers: Vec<String> = v
        .get("server")
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    servers.sort();
    servers.dedup();
    let versions: BTreeMap<String, String> = v
        .get("version")
        .and_then(toml::Value::as_table)
        .map(|t| {
            t.iter()
                .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(Desired { servers, versions }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(toml_text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sm-proj-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("servers.toml");
        std::fs::write(&p, toml_text).unwrap();
        p
    }

    #[test]
    fn parses_list_and_versions() {
        let p = write(
            r#"
            server = ["rust-analyzer", "clangd", "rust-analyzer"]
            [version]
            rust-analyzer = "2025-07-01"
            "#,
        );
        std::env::set_var("SM_PROJECT_CONFIG", &p);
        let d = desired().unwrap().expect("应有清单");
        assert_eq!(d.servers, vec!["clangd", "rust-analyzer"], "去重 + 排序");
        assert_eq!(
            d.versions.get("rust-analyzer").map(String::as_str),
            Some("2025-07-01")
        );
        std::env::remove_var("SM_PROJECT_CONFIG");
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let p = std::env::temp_dir().join("sm-proj-nonexistent/servers.toml");
        std::env::set_var("SM_PROJECT_CONFIG", &p);
        assert!(desired().unwrap().is_none(), "没有清单 = None（不是错误）");
        std::env::remove_var("SM_PROJECT_CONFIG");
    }
}
