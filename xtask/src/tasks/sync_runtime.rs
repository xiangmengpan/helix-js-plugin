//! `cargo xtask sync-runtime` —— 把**源**（仓库 `plugins/`）镜像到运行时副本。
//!
//! ## 由来（旧脚本的头注释原话）
//!
//! `<runtime>/plugins` 与 `<config>/plugins` 都是**手工拷贝**的副本 ⇒ 改了源却忘同步，
//! 插件行为"没变化"。后来固化成 `plugins/tools/sync_runtime.sh`。现在把它搬进 Rust：
//! 脚本本身是 POSIX，Windows 用户走不了 —— 而这条恰恰是 Windows 最需要的（内置插件里
//! `helix.d.ts` 只影响补全，改了不同步**看不出来**）。
//!
//! ## 语义（与旧脚本逐条对齐，有意保守）
//!
//! 1. **只更新已存在的目标文件，不新建**（同步会跑在用户已配置好的目录上，不该往里塞新东西）；
//! 2. 例外：**已存在**的插件目录下可以新增文件（典型 `<plugin>/tests/*.js`）——
//!    因为 `cargo xtask api-check` 会逐文件比对这些副本；
//! 3. 覆盖范围 = 分发面：根级 `*.js|*.json|*.md|*.d.ts`、`<plugin>/*.js|*.json|*.md`、
//!    `<plugin>/tests/*.js`；跳过 `tools/`、`examples/`（开发用，不分发）；
//! 4. 逐文件比对（旧脚本 `cmp -s`）后才写，输出每条更新 + 计数。
//!
//! 首次安装请用 `install-plugins`（它建目录、建新文件）。

use std::path::{Path, PathBuf};

use crate::tasks::plugins_fs;
use crate::DynError;

pub fn run(args: Vec<String>) -> Result<(), DynError> {
    let opts = plugins_fs::parse(args)?;
    let src = crate::path::plugins_source();
    if !src.is_dir() {
        return Err(format!("找不到源目录: {}", src.display()).into());
    }

    // 两个根：内置层（runtime）在前、用户层（config）殿后 —— 加载顺序即覆盖顺序。
    let dests = [
        plugins_fs::runtime_root(&opts).join("plugins"),
        plugins_fs::config_root(&opts).join("plugins"),
    ];
    let files = distribution_files(&src)?;

    for dst in dests {
        if !dst.is_dir() {
            println!("  跳过(不存在): {}", dst.display());
            continue;
        }
        let mut n = 0usize;
        for rel in &files {
            let to = dst.join(rel);
            // 顶层（插件目录名或根级文件名）：用来判「这个插件在目标里已经装过了吗」
            let Some(top) = rel.iter().next() else {
                continue;
            };
            let exists = to.exists();
            if !exists && !dst.join(top).exists() {
                continue; // 目标里没有这个插件 ⇒ 不是同步的活（首次安装交给 install-plugins）
            }
            if exists && plugins_fs::same_contents(&src.join(rel), &to) {
                continue;
            }
            plugins_fs::install_file(&src.join(rel), &to, opts.dry_run)?;
            n += 1;
            let verb = if exists { "更新" } else { "新增" };
            let dry = if opts.dry_run {
                "（dry-run，未写入）"
            } else {
                ""
            };
            println!("  {verb} {} → {}{dry}", rel.display(), dst.display());
        }
        let dry = if opts.dry_run { "（dry-run）" } else { "" };
        println!("  {}: 更新 {n} 个文件{dry}", dst.display());
    }
    Ok(())
}

/// 枚举「分发面」的相对路径（与旧 `.sh` 的 glob 集合逐条对应）。
///
/// 刻意**不**递归整棵树：`plugins/` 里还有 `lib/`（将来若有）、`<plugin>/lib/` 等私有模块，
/// 是否分发由 `install-plugins` 决定（它拷整棵树）；同步只维护已知的分发面，
/// 免得把一个正在开发的私有模块推送到用户的运行时里。
fn distribution_files(src: &Path) -> Result<Vec<PathBuf>, DynError> {
    let mut out = Vec::new();
    let root_exts = ["js", "json", "md"];

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name == "tools" || name == "examples" {
                continue; // 开发用，不随软件分发
            }
            push_with_ext(&mut out, &path, src, &["js", "json", "md"])?;
            // `<plugin>/tests/*.js`（api-check 会比对它们）
            let tests = path.join("tests");
            if tests.is_dir() {
                push_with_ext(&mut out, &tests, src, &["js"])?;
            }
        } else if root_exts.iter().any(|e| name.ends_with(&format!(".{e}")))
            || name.ends_with(".d.ts")
        {
            out.push(PathBuf::from(&name));
        }
    }
    out.sort();
    Ok(out)
}

/// 把 `dir` 下扩展名属于 `exts` 的**文件**（相对 `root`）推入 `out`。
fn push_with_ext(
    out: &mut Vec<PathBuf>,
    dir: &Path,
    root: &Path,
    exts: &[&str],
) -> Result<(), DynError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if exts.contains(&ext) {
            out.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}
