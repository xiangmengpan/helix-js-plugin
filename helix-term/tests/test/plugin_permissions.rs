//! C-2(能力清单/capability)的**端到端**证明:声明了 `permissions` 的插件,
//! 未声明的能力首次使用时被**询问**(那次调用失败 + 记入待授权),用
//! `:js-permission allow <plugin> <cap>` 回答后放行。
//!
//! 为什么值得单独一条集成用例:检查点的单测在 helix-js 内部(直接调 `run_command`),
//! 而真实路径要经过 `:plugin-load`(绝对路径加载 ⇒ 提供根为空 ⇒ **不**算内置)、
//! 命令泵、`run_command` 的 Err → 状态栏 Error、以及 `:js-permission` 这条 TypableCommand。
//! 这条用例把"插件作者看到的行为"整体跑一遍。
use super::*;

/// 未声明 `process` ⇒ 首次被拒(错误可见);`:js-permission allow` 之后放行。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_permission_ask_then_allow() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("p.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("perm_demo.js");
    // 声明了 `permissions`(空对象 = 什么能力都没要)⇒ 进入检查
    std::fs::write(
        &plugin,
        r#"
        helix.plugin("perm-demo", { deps: [], permissions: {} });
        helix.register_command("perm-run", () => {
            helix.echo("ran:" + helix.run("echo perm-ok").trim());
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin.display())),
                None,
            ),
            // 首次:未声明 process ⇒ 命令失败,状态栏 Error 且写明插件名与能力名
            (
                Some(":perm-run<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert!(
                        matches!(severity, helix_core::diagnostic::Severity::Error),
                        "未声明能力应报错(失败可见),got {severity:?}"
                    );
                    assert!(status.as_ref().contains("perm-demo"), "status: {status}");
                    assert!(status.as_ref().contains("process"), "status: {status}");
                    assert!(status.as_ref().contains("未声明能力"), "status: {status}");
                }),
            ),
            // :js-permission list 应能看到"待授权"
            (
                Some(":js-permission list<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("待授权") && status.as_ref().contains("process"),
                        "list 应列出待授权: {status}"
                    );
                }),
            ),
            // 回答"允许"
            (
                Some(":js-permission allow perm-demo process<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert!(status.as_ref().contains("allow"), "status: {status}");
                    assert!(
                        matches!(severity, helix_core::diagnostic::Severity::Info),
                        "授权本身不是错误, got {severity:?}"
                    );
                }),
            ),
            // `list` 要能看到"我上次答了什么"(决定唯一的对外出口)
            (
                Some(":js-permission list<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    let s = status.as_ref();
                    assert!(
                        s.contains("已允许") && s.contains("perm-demo process"),
                        "list 应列出已记住的允许: {s:?}"
                    );
                    assert!(
                        !s.contains("待授权: perm-demo"),
                        "已经回答过的不该还在待授权里: {s:?}"
                    );
                }),
            ),
            // 再跑 ⇒ 放行(真实起了一个 `echo` 子进程)
            (
                Some(":perm-run<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "ran:perm-ok");
                    assert!(
                        matches!(severity, helix_core::diagnostic::Severity::Info),
                        "授权后应放行, got {severity:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

/// 反向:`deny` 之后一直拒(不会被"再来一次"绕过)。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_permission_deny_is_sticky() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("p.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("perm_deny.js");
    std::fs::write(
        &plugin,
        r#"
        helix.plugin("perm-deny-demo", { deps: [], permissions: {} });
        helix.register_command("perm-deny-run", () => {
            helix.echo("ran:" + helix.run("echo should-not-run").trim());
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin.display())),
                None,
            ),
            (
                Some(":js-permission deny perm-deny-demo process<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(status.as_ref().contains("deny"), "status: {status}");
                }),
            ),
            (
                Some(":perm-deny-run<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert!(
                        matches!(severity, helix_core::diagnostic::Severity::Error),
                        "deny 之后应一直拒, got {severity:?}"
                    );
                    assert!(status.as_ref().contains("已被拒绝"), "status: {status}");
                }),
            ),
            // `list` 里能看到"已拒绝"
            (
                Some(":js-permission list<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    let s = status.as_ref();
                    assert!(
                        s.contains("已拒绝") && s.contains("perm-deny-demo process"),
                        "list 应列出已记住的拒绝: {s:?}"
                    );
                }),
            ),
            // 再跑一次仍被拒(决定是记住的,不是一次性的)
            (
                Some(":perm-deny-run<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert!(
                        matches!(severity, helix_core::diagnostic::Severity::Error),
                        "决定应被记住, got {severity:?}"
                    );
                    assert!(status.as_ref().contains("已被拒绝"), "status: {status}");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

/// 没声明 `permissions` 的插件**不受影响**(legacy 语义)—— 存量插件的零影响保证。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_without_permissions_is_unaffected() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("p.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("perm_legacy.js");
    std::fs::write(
        &plugin,
        r#"
        helix.plugin("perm-legacy", { deps: [] });
        helix.register_command("perm-legacy-run", () => {
            helix.echo("ran:" + helix.run("echo legacy-ok").trim());
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin.display())),
                None,
            ),
            (
                Some(":perm-legacy-run<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "ran:legacy-ok");
                    assert!(
                        matches!(severity, helix_core::diagnostic::Severity::Info),
                        "没声明 permissions ⇒ 不拦截, got {severity:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

/// `helix.permissions()` 的**实际用法**(内置插件已按这个形状改:`statusline` 查 git 分支、
/// plugin-manager 查 git 操作前先看 `process`)。
///
/// 语义要点(决定了这段码为什么这么写):**没声明 `permissions` 的插件恒 true**
/// (legacy 放行)⇒ 光"不声明"是**测不到**降级路径的;要触发降级,插件得声明了
/// **别的**能力而没声明 `process`(或用户 `deny` 了它)。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_degrades_without_process_capability() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("p.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("deg.js");
    std::fs::write(
        &plugin,
        r#"
        // 声明了 fs_read 但**没有** process ⇒ permissions().process === false
        helix.plugin("deg", { deps: [], permissions: { fs_read: true } });
        helix.register_command("deg-guarded", () => {
            if (!helix.permissions().process) { helix.echo("degraded"); return; }
            helix.run("git status");
        });
        helix.register_command("deg-unguarded", () => { helix.run("git status"); });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin.display())),
                None,
            ),
            // 带守卫的调用:降级(不 spawn、不报错)
            (
                Some(":deg-guarded<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "degraded");
                    assert!(
                        matches!(severity, helix_core::diagnostic::Severity::Info),
                        "降级不是错误,got {severity:?}"
                    );
                }),
            ),
            // 对照:不带守卫的同一调用 ⇒ 被检查点拒掉(这就是守卫要避免的体验)
            (
                Some(":deg-unguarded<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert!(
                        matches!(severity, helix_core::diagnostic::Severity::Error),
                        "未声明的能力应被拒,got {severity:?}"
                    );
                    assert!(status.as_ref().contains("未声明能力"), "{status}");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// **C-3 归属**:`onPress` 里调 `helix.run` 必须被拒,且错误里是**它自己**的名字。
///
/// 这是本次改动的验收依据:改前 `onPress` 没有属主,判定回退到"加载期最后声明的
/// 插件"(内置 init.js 的 filetree ⇒ builtin ⇒ 直接放行)。
#[tokio::test(flavor = "multi_thread")]
async fn onpress_is_denied_for_declared_plugin() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("op.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("perm_onpress.js");
    std::fs::write(
        &plugin,
        r#"
        helix.plugin("perm-onpress", { deps: [], permissions: {} });
        helix.register_command("perm-onpress-popup", () => {
            helix.open_popup({
                render: () => helix.el("col", [
                    helix.el("button", "run", {
                        id: "b1",
                        onPress: () => { helix.run("echo pwned"); helix.echo("RAN"); },
                    }),
                ]),
            });
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin.display())),
                None,
            ),
            (Some(":perm-onpress-popup<ret>"), None),
            // Tab 聚焦 b1 → Enter 触发 onPress;随后的 Esc 关掉弹窗(否则下面的
            // `:` 命令会被弹窗吃掉,命令根本没执行)
            (Some("<tab><ret><esc>"), None),
            // 被拒的证据在"授权通道"里(状态栏会被随后的重绘清掉,不适合做断言):
            // 待授权列表里出现的必须是 **perm-onpress + process** —— 即判定用的是
            // 调用者自己的 manifest,而不是"加载期最后声明的插件"
            (
                Some(":js-permission list<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    let status = status.as_ref().to_string();
                    assert!(
                        status.contains("待授权") && status.contains("perm-onpress"),
                        "onPress 里的 helix.run 应被拒并记入待授权: {status}"
                    );
                    assert!(status.contains("process"), "应指出缺的能力: {status}");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
