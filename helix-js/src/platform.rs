//! 平台门面：**子进程命令构造的唯一出口**。
//!
//! 只有两个差异点，收在一处免得散落在各调用点：
//!
//! 1. **字符串形态**必须过平台 shell —— Unix `sh -c <str>`、Windows `cmd /C <str>`；
//! 2. **argv 形态不过 shell** —— 第一个元素是可执行名，其余原样交给子进程。
//!
//! 为什么 argv 才是跨平台的正确抽象：字符串形态的**语法随平台变**（`$VAR` vs `%VAR%`、
//! 单引号 vs 双引号、管道与重定向的转义规则全不同）。插件侧用 `shq()` 拼单引号转义只在
//! Unix 成立；搬到 `cmd` 上不是报错，而是**静默做错事**（参数被拆开、`%` 被展开成空）。
//! 所以给插件的是「别经过 shell」这条路，而不是「把转义写得更聪明」。
//!
//! 见 `docs/superpowers/specs/2026-10-30-cross-platform-design.md` §3。

use std::process::Command;

/// 平台 shell 与它「执行一段命令」的开关：`("sh", "-c")` / `("cmd", "/C")`。
///
/// 与 `helix-term/src/commands/dap.rs` 的 `shell_command()` 同一套取值
/// （那边返回 `(String, Vec<String>)` 便于拼参数，这里直接给 `Command`）。
pub(crate) fn shell_program() -> (&'static str, &'static str) {
    if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    }
}

/// 字符串形态 → 过平台 shell。**语法随平台变**，跨平台插件别用它做转义/管道/重定向。
pub(crate) fn shell_command(cmd: &str) -> Command {
    let (prog, flag) = shell_program();
    let mut command = Command::new(prog);
    command.arg(flag).arg(cmd);
    command
}

/// argv 形态 → **不经 shell**。
///
/// 空数组必须报错：静默成功会让插件以为命令跑过了（比失败更难查）。
pub(crate) fn argv_command(argv: &[String]) -> anyhow::Result<Command> {
    let (prog, rest) = argv
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty argv"))?;
    let mut command = Command::new(prog);
    command.args(rest);
    Ok(command)
}

/// 一条待执行的命令：两种形态。
///
/// 为什么要这个枚举：`helix.run` / `run_async` / `spawn` / pty worker 都要「起一个子进程」，
/// 而两种形态（过 shell / argv）的构造逻辑只能有一处 —— 否则迟早出现"某个入口忘了走门面"。
#[derive(Clone, Debug)]
pub(crate) enum Cmd {
    /// 过平台 shell（`sh -c` / `cmd /C`）：语法随平台变
    Shell(String),
    /// argv，**不经 shell**：跨平台插件用它
    Argv(Vec<String>),
}

impl Cmd {
    /// 造 `Command`。空 argv 也算错（上层通常已在 JS 边界拦过，这里兜底）。
    pub(crate) fn to_command(&self) -> anyhow::Result<Command> {
        match self {
            Cmd::Shell(cmd) => Ok(shell_command(cmd)),
            Cmd::Argv(argv) => argv_command(argv),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// 核心承诺：argv 形态**不经 shell** ⇒ 带空格的参数必须原样到达。
    /// （若偷偷走 `sh -c`，`a b` 会被拆成两个参数，这里立刻红。）
    #[test]
    fn argv_is_not_shell_split() {
        let command = argv_command(&["echo".to_string(), "a b".to_string()]).unwrap();
        assert_eq!(command.get_program().to_string_lossy(), "echo");
        assert_eq!(args_of(&command), ["a b"]);
    }

    #[test]
    fn empty_argv_is_an_error() {
        assert!(argv_command(&[]).is_err(), "空数组必须报错，不能静默成功");
    }

    /// 字符串形态必须真的过平台 shell（否则 `$VAR`/管道这类语法没意义）。
    #[test]
    fn string_form_goes_through_platform_shell() {
        let command = shell_command("echo hi");
        let (prog, flag) = shell_program();
        assert_eq!(command.get_program().to_string_lossy(), prog);
        assert_eq!(args_of(&command), [flag, "echo hi"]);
    }
}
