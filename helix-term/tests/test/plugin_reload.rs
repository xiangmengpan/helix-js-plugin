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
    helix_js::set_plugin_roots_current(vec![proot.path().to_path_buf()]);
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
    helix_js::set_plugin_roots_current(vec![proot.path().to_path_buf()]);
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

// ── 插件配置:load / reload 后必须重建合并缓存 ──
//
// `define_config` 的 schema 只在脚本**执行时**才注册,而 `CONFIGS` 原先只在启动与
// `:config-reload` 构建 ⇒ 运行时 `:plugin-load` 的插件 `get_config()` 拿到 null(默认值全丢),
// `:plugin-reload` 后新增的 schema 键也没有默认值。两个入口都必须在脚本跑完后重建。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_config_rebuilt_after_load_and_reload() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("cfg.txt");
    std::fs::write(&file, "x\n")?;
    let script = dir.path().join("cfg-demo.js");

    let v1 = r#"helix.define_config("cfg-demo", { flag: { type: "boolean", default: false } });
helix.register_command("cfg-demo-dump", () => { helix.echo(JSON.stringify(helix.get_config("cfg-demo"))); });"#;
    // reload 会按名重读磁盘 ⇒ v2 多一个 schema 键(旧缓存里没有它)
    let v2 = r#"helix.define_config("cfg-demo", {
  flag:  { type: "boolean", default: false },
  limit: { type: "number",  default: 7 },
});
helix.register_command("cfg-demo-dump", () => { helix.echo(JSON.stringify(helix.get_config("cfg-demo"))); });"#;
    std::fs::write(&script, v1)?;

    let script_for_edit = script.clone();
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", script.display())),
                None,
            ),
            // 断言 load 后即可读配置,顺手把盘上内容改成 v2(空按键步会挂住事件循环)
            (
                Some(":cfg-demo-dump<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    let v: serde_json::Value = serde_json::from_str(status.as_ref())
                        .unwrap_or_else(|e| panic!("get_config 不是 JSON({e}): {status}"));
                    assert_eq!(
                        v["flag"], false,
                        "plugin-load 后 get_config 应立即可用(含默认值)"
                    );
                    std::fs::write(&script_for_edit, v2).expect("改写脚本");
                }),
            ),
            (Some(":plugin-reload<ret>"), None),
            (
                Some(":cfg-demo-dump<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    let v: serde_json::Value = serde_json::from_str(status.as_ref())
                        .unwrap_or_else(|e| panic!("get_config 不是 JSON({e}): {status}"));
                    assert_eq!(v["flag"], false, "reload 后旧键仍在");
                    assert_eq!(v["limit"], 7, "reload 后新增的 schema 键应带默认值");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// ── 插件配置:运行时懒加载(`helix.load`)后也要重建 ──
//
// `helix.load` 走 helix-js 内部的 load_script_checked(不经过 helix-term 的 :plugin-load),
// 所以重建必须发生在 helix-js 侧,否则懒加载插件的 get_config() 一直是 null。
// 用**绝对路径**加载:不依赖插件根(`set_plugin_roots` 是首设生效,并行测试下不可靠)。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_config_rebuilt_after_lazy_load() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("lazy.txt");
    std::fs::write(&file, "x\n")?;
    let lazy = dir.path().join("lazy-cfg.js");
    std::fs::write(
        &lazy,
        r#"helix.define_config("lazy-cfg", { flag: { type: "boolean", default: true } });
helix.register_command("lazy-cfg-dump", () => { helix.echo(JSON.stringify(helix.get_config("lazy-cfg"))); });"#,
    )?;
    let loader = dir.path().join("loader.js");
    std::fs::write(
        &loader,
        format!(
            "helix.register_command('load-lazy', () => {{ helix.load({:?}); }});",
            lazy.display().to_string()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", loader.display())),
                None,
            ),
            // 懒加载:命令回调里才 helix.load
            (Some(":load-lazy<ret>"), None),
            (
                Some(":lazy-cfg-dump<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    let v: serde_json::Value = serde_json::from_str(status.as_ref())
                        .unwrap_or_else(|e| panic!("get_config 不是 JSON({e}): {status}"));
                    assert_eq!(v["flag"], true, "helix.load 懒加载后 get_config 应立即可用");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
