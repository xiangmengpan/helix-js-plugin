//! `cargo xtask api-check` 的实现。
//!
//! 从 `main.rs` 的 `tasks` 模块拆出(G5):一个任务一个文件,统一入口
//! `run(args: Vec<String>)`,由 `tasks::TASKS` 表驱动 dispatch 与 help。

use crate::DynError;

/// 一次性跑**三个**表面门禁(顺序即失败优先级):
///
/// 1. `check_api_surface.mjs` —— `plugins/helix.d.ts` ↔ Rust 注册表;
/// 2. `check_doc_surface.mjs` —— `docs/plugin-api.md` ↔ 注册表;
/// 3. `check_capability_surface.mjs` —— 能力检查点 ↔ 注册的 JS API。
///
/// 为什么三件事挂在一个命令:文档此前**没有任何门禁**(api-check 只看 d.ts)⇒ 会静默腐烂
/// (实测:文档里还留着 4 个已删除的 API,另有 17 个已注册 API 完全没记)。
/// 一个命令覆盖三半 ⇒ CI 的 "Check plugin API surface" 步骤天然把文档与能力表也管住了。
///
/// ⚠ G5 修正:这里原先的注释写"两者都由 api-check 跑",实际早就是 3 个脚本 ——
/// 正是"help / 注释 / dispatch 三处各写一遍任务清单"会漂的证据,现在清单只活在
/// `tasks/mod.rs` 的 `TASKS` 与这个数组里。
pub fn run(args: Vec<String>) -> Result<(), DynError> {
    let _ = args; // 该任务不接参数
    for script in [
        crate::path::plugin_api_surface_script(),
        crate::path::plugin_doc_surface_script(),
        crate::path::plugin_capability_surface_script(),
    ] {
        let status = std::process::Command::new("node")
            .arg(&script)
            .status()
            .map_err(|err| {
                format!(
                    "api-check 需要 node 且脚本存在(脚本 {}):{err}",
                    script.display()
                )
            })?;
        if !status.success() {
            return Err(format!("api-check failed ({status}); 脚本:{}", script.display()).into());
        }
    }
    Ok(())
}
