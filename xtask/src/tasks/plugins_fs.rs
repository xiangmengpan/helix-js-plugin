//! `install-plugins` / `sync-runtime` 共用的小工具：目录决议、`--dry-run`、文件比对。
//!
//! 为什么单独一个文件：两个任务对**同一组环境变量与默认目录**必须得出同一个答案，
//! 复制两份必然漂移（仓库对此有过教训：`helix.d.ts` 镜像从未被同步）。任务本体仍各自
//! 一个文件（`tasks/<name>.rs`，G5 的"一任务一文件"）。

use std::path::{Path, PathBuf};

use crate::DynError;

/// 命令行与环境的合并视图。
pub struct Opts {
    /// 位置参数（`sh contrib/install-plugins.sh /path/to/rt` 的老用法）
    pub positional: Option<PathBuf>,
    pub runtime: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub dry_run: bool,
}

/// 解析 `[路径] [--runtime <dir>] [--config <dir>] [--dry-run]`（等价 env：`DRY_RUN`）。
///
/// 未知参数**报错**而不是忽略：这两个命令会往用户的 helix 目录写文件，静默忽略拼错的
/// `--dryrun` 等于按默认目录动手，那是最糟的失败方式。
pub fn parse(args: Vec<String>) -> Result<Opts, DynError> {
    let mut opts = Opts {
        positional: None,
        runtime: None,
        config: None,
        dry_run: std::env::var_os("DRY_RUN").is_some_and(|v| !v.is_empty()),
    };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dry-run" | "-n" => opts.dry_run = true,
            "--runtime" => {
                opts.runtime = Some(PathBuf::from(it.next().ok_or(
                    "--runtime 需要一个目录参数（例：--runtime ~/.config/helix/runtime）",
                )?))
            }
            "--config" => {
                opts.config = Some(PathBuf::from(
                    it.next()
                        .ok_or("--config 需要一个目录参数（例：--config ~/.config/helix）")?,
                ))
            }
            other if other.starts_with('-') => {
                return Err(format!(
                    "未知参数 `{other}`；可用：--runtime <dir> --config <dir> --dry-run"
                )
                .into())
            }
            other => {
                if opts.positional.is_some() {
                    return Err(format!("多余的路径参数 `{other}`（只接受一个）").into());
                }
                opts.positional = Some(PathBuf::from(other));
            }
        }
    }
    Ok(opts)
}

/// HELIX 配置目录（etcetera，跨平台：Unix `~/.config/helix` / Windows `%APPDATA%\helix`）。
///
/// ⚠ 这是本次改动**唯一**一处用户可见差异：旧 `.sh` 硬编码 `$HOME/.config/helix`，
/// Windows 上那是错的路径。其余语义（env 覆盖、目录形状）全部照旧。
pub fn default_config_dir() -> PathBuf {
    helix_loader::config_dir()
}

/// runtime 根目录：`--runtime` > 位置参数 > `HELIX_RUNTIME` > `<config>/runtime`。
///
/// 与旧 `.sh` 的优先级一致（`.sh` 的位置参数优先于 `HELIX_RUNTIME`）。
pub fn runtime_root(opts: &Opts) -> PathBuf {
    opts.runtime
        .clone()
        .or_else(|| opts.positional.clone())
        .or_else(|| env_path("HELIX_RUNTIME"))
        .unwrap_or_else(|| default_config_dir().join("runtime"))
}

/// 配置根目录：`--config` > `HELIX_CONFIG` > etcetera 的配置目录。
pub fn config_root(opts: &Opts) -> PathBuf {
    opts.config
        .clone()
        .or_else(|| env_path("HELIX_CONFIG"))
        .unwrap_or_else(default_config_dir)
}

fn env_path(var: &str) -> Option<PathBuf> {
    // 不做 `~` 展开：旧 `.sh` 也没有（避免"看起来更聪明"的静默行为差异）
    std::env::var_os(var).map(PathBuf::from)
}

/// 两个文件内容是否相同（旧 `.sh` 用 `cmp -s`）。读不全（权限/IO）按"不同"处理 ⇒ 会重写。
pub fn same_contents(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// 递归列出目录下的**文件**相对路径（排序，保证输出稳定/可比对）。
pub fn walk_files(root: &Path) -> Result<Vec<PathBuf>, DynError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let meta = entry.metadata()?;
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                // 跟随符号链接（`entry.metadata()` 已跟随）；今天 `plugins/` 里没有链接，
                // 这一条只为将来语义明确 —— 与 `cp -R` 的"复制内容"一致。
                out.push(path.strip_prefix(root)?.to_path_buf());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// 建父目录 + 写文件（dry-run 时不落盘）。
pub fn install_file(from: &Path, to: &Path, dry_run: bool) -> Result<(), DynError> {
    if dry_run {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to)?;
    Ok(())
}
