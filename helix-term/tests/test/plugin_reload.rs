use super::*;

use helix_term::ui;

// 验证 :plugin-reload 后插件命令仍可用（重跑注册）。
// 本文件保留；integration.rs 的 mod 声明由控制器合并时统一添加。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_reload_command() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("r.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("reload.js");
    std::fs::write(
        &plugin_path,
        r#"helix.register_command("reload-demo", () => { helix.echo("reloaded-ok"); });
helix.register_command("show-panel", () => { helix.open_panel({ side: "right", size: 30, render: () => ["p1"] }); });"#,
    )?;

    let panel_type = std::any::type_name::<ui::PluginPanel>();

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":show-panel<ret>"),
                Some(&|app| {
                    assert!(app.compositor.has_component(panel_type), "panel layer open");
                }),
            ),
            (
                Some(":plugin-reload<ret>"),
                Some(&|app| {
                    assert!(
                        !app.compositor.has_component(panel_type),
                        "panel layer removed after reload (no zombie layer)"
                    );
                }),
            ),
            (
                Some(":panel-close<ret>"),
                Some(&|app| {
                    // LAST_PANEL_ID 已清空 → 不误报有面板，也不产生残留层
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert!(
                        matches!(severity, helix_core::diagnostic::Severity::Error),
                        "panel-close reports no panel"
                    );
                    assert!(status.as_ref().contains("no panel open"));
                    assert!(!app.compositor.has_component(panel_type));
                }),
            ),
            (
                Some(":reload-demo<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "reloaded-ok");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

// ── 计划任务 4：`:plugin reload` 从桩变成真接管 ──

fn mgr_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/plugin/plugin.js")
}

/// `:plugin reload` 真的重载：**改盘上的脚本**后 reload，新内容生效（v1 → v2）。
/// 这是唯一能证明"重跑脚本"而不只是"echo 一行话"的断言。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_op_reload_reloads_script() -> anyhow::Result<()> {
    // `:plugin` 的 JS 侧入口要先能拿到插件根（集成进程不跑启动推入 ⇒ 必须显式设）
    let proot = tempfile::tempdir()?;
    helix_js::set_plugin_roots(vec![proot.path().to_path_buf()]);
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;
    let script = dir.path().join("r.js");
    std::fs::write(
        &script,
        "helix.register_command('ver', () => helix.echo('v1'));\n",
    )?;

    let script_for_edit = script.clone();
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", mgr_path().display())),
                None,
            ),
            (
                Some(&format!(":plugin-load {}<ret>", script.display())),
                None,
            ),
            // 断言 v1 之后顺手把盘上内容改成 v2（**不能**用空按键步：没有按键事件时
            // `event_loop_until_idle` 会一直等下去 —— 实测挂了 15 分钟）
            (
                Some(":ver<ret>"),
                Some(&|app| {
                    let (s, _) = app.editor.get_status().unwrap();
                    assert_eq!(s.as_ref(), "v1", "前置:应为 v1");
                    std::fs::write(
                        &script_for_edit,
                        "helix.register_command('ver', () => helix.echo('v2'));\n",
                    )
                    .expect("改写脚本");
                }),
            ),
            (
                Some(":plugin reload<ret>"),
                Some(&|app| {
                    let st = app.editor.get_status().map(|(s, _)| s.to_string());
                    assert!(
                        !st.as_deref().unwrap_or("").contains("请用 :plugin-reload"),
                        "reload 仍是桩: {st:?}"
                    );
                }),
            ),
            (
                Some(":ver<ret>"),
                Some(&|app| {
                    let (s, _) = app.editor.get_status().unwrap();
                    assert_eq!(s.as_ref(), "v2", "reload 后应跑新脚本内容");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// 审查重点 #4：在**命令里**调 `helix.reload_plugins()` 不得 panic、不得重入引擎。
/// 重载必须排到命令边界之后（job 队列），而不是在当前 JS 栈里同步跑。
#[tokio::test(flavor = "multi_thread")]
async fn reload_inside_command_does_not_panic() -> anyhow::Result<()> {
    let proot = tempfile::tempdir()?;
    helix_js::set_plugin_roots(vec![proot.path().to_path_buf()]);
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;
    let script = dir.path().join("self.js");
    std::fs::write(
        &script,
        r#"helix.register_command("ver", () => helix.echo("v1"));
helix.register_command("self-reload", () => helix.reload_plugins());"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", script.display())),
                None,
            ),
            (
                Some(":self-reload<ret>"),
                Some(&|app| {
                    // 重载本身**不产生状态栏消息**（没有错误就没消息）⇒ 不能 unwrap
                    let st = app
                        .editor
                        .get_status()
                        .map(|(s, _)| s.to_string())
                        .unwrap_or_default();
                    assert!(
                        !st.contains("not a function") && !st.contains("failed"),
                        "helix.reload_plugins 未注册或重载失败: {st:?}"
                    );
                }),
            ),
            (
                Some(":ver<ret>"),
                Some(&|app| {
                    let (s, _) = app.editor.get_status().unwrap();
                    assert_eq!(s.as_ref(), "v1", "重载后脚本重新 eval，命令仍可用");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
