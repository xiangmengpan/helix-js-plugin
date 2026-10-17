//! `cargo xtask docgen` 的实现。
//!
//! 从 `main.rs` 的 `tasks` 模块拆出(G5):一个任务一个文件,统一入口
//! `run(args: Vec<String>)`,由 `tasks::TASKS` 表驱动 dispatch 与 help。

use crate::DynError;

pub fn run(args: Vec<String>) -> Result<(), DynError> {
    let _ = args; // 该任务不接参数
    use crate::docgen::*;
    write(TYPABLE_COMMANDS_MD_OUTPUT, &typable_commands()?);
    write(STATIC_COMMANDS_MD_OUTPUT, &static_commands()?);
    write(LANG_SUPPORT_MD_OUTPUT, &lang_features()?);
    Ok(())
}
