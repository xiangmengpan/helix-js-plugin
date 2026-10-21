//! `cargo xtask api-describe` —— 把 `helix-js/src/api.rs` 的声明面写成 JSON。
//!
//! 为什么单独一个任务：三个对账脚本（d.ts / 文档 / 能力）此前各自**正则解析 Rust 源码**，
//! 那是"四份真相"的根源。现在它们只读这份 JSON（唯一数据源是 `api::API` 那张表），
//! 解析源码的代码整段删掉。
//!
//! 产物：`target/api-surface.json`（不入库：它是表 + 代码的投影，重跑即可）。

use std::path::PathBuf;

use crate::DynError;

pub const OUT: &str = "target/api-surface.json";

pub fn run(args: Vec<String>) -> Result<(), DynError> {
    let _ = args;
    let root = crate::path::project_root();
    let out = root.join(OUT);

    // 用 example 而不是测试：不依赖测试构建，失败直接体现在退出码上
    let output = std::process::Command::new(env!("CARGO"))
        .current_dir(&root)
        .args(["run", "-q", "-p", "helix-js", "--example", "describe"])
        .output()
        .map_err(|e| format!("api-describe: 跑 describe example 失败: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "api-describe: describe example 退出码 {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    // 只取 stdout 里最后一行 JSON（cargo 的警告走 stderr，不影响）
    let text = String::from_utf8_lossy(&output.stdout);
    let json = text
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .ok_or("api-describe: example 没有输出 JSON")?;
    if let Some(dir) = PathBuf::from(&out).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("api-describe: 建目录失败: {e}"))?;
    }
    std::fs::write(&out, json).map_err(|e| format!("api-describe: 写 {OUT} 失败: {e}"))?;
    Ok(())
}
