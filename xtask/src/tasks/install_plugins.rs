//! `cargo xtask install-plugins` —— 把仓库 `plugins/` 装进 helix 的 **runtime 目录**。
//!
//! 这是"内置插件"的**分发**步骤：随软件分发的那一层由本命令写入，用户层
//! （`<config>/plugins/`）里的同名目录会**整体覆盖**它（见 `docs/plugin-layout.md` §3.5）。
//!
//! ## 为什么从 `.sh` 搬过来
//!
//! 旧实现是 `contrib/install-plugins.sh`（纯 POSIX + `cp -R`），Windows 上要 Git Bash/WSL；
//! 而"改完源要重装/同步"是**必经步骤**（`plugin-vs-core.md` 红线 R4：改源忘了同步运行副本，
//! 插件行为"没变化"）。必经步骤不能只有 Unix 能走。
//!
//! ## 语义（与旧脚本逐条对齐，除下面这一条）
//!
//! - **整棵树**拷过去（含 `lib/`、`examples/`、`tools/`、`init.js`、`helix.d.ts`）——
//!   和 `cp -R "$src"/. "$dest"/` 一致。「只同步分发面」是 `sync-runtime` 的活。
//! - 计数口径照旧：数的是**目标目录里现有文件数**（可能含历史残留），不是本次写入数。
//! - ⚠ 唯一有意差异：默认目录从硬编码 `$HOME/.config/helix/runtime` 换成
//!   `helix_loader::config_dir()`（跨平台；Windows 上是 `%APPDATA%\helix`）。

use crate::tasks::plugins_fs;
use crate::DynError;

pub fn run(args: Vec<String>) -> Result<(), DynError> {
    let opts = plugins_fs::parse(args)?;
    let src = crate::path::plugins_source();
    let dest = plugins_fs::runtime_root(&opts).join("plugins");

    if !src.is_dir() {
        return Err(format!("找不到源目录: {}", src.display()).into());
    }
    println!("源   : {}", src.display());
    println!("目标 : {}", dest.display());

    let files = plugins_fs::walk_files(&src)?;
    if opts.dry_run {
        println!("(DRY_RUN:不做任何写入)");
        for rel in &files {
            println!("  将安装: {}", rel.display());
        }
        return Ok(());
    }

    std::fs::create_dir_all(&dest)?;
    for rel in &files {
        plugins_fs::install_file(&src.join(rel), &dest.join(rel), false)?;
    }

    let installed = plugins_fs::walk_files(&dest).map_or(0, |v| v.len());
    println!("已安装文件数: {installed}");

    let mut dirs: Vec<String> = std::fs::read_dir(&dest)?
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    dirs.sort();
    println!("插件目录: {}", dirs.join(" "));
    println!();
    println!("提示:用户层 <config>/plugins/ 里的同名目录会**整体覆盖**这里的内置版本。");
    Ok(())
}
