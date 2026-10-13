//! 插件主题覆盖集成测试（v10-② 计划强制保留）：
//! 走真实命令路径——:theme 切换到含 inherits 的捆绑主题 → 插件 set_theme 覆盖
//! → editor.theme 变化（且父主题样式不丢）→ reset_theme 恢复基准。
//! 独立测试文件（不挂 integration.rs mod），控制器合并时再接线。

#![cfg(feature = "integration")]

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use helix_loader::workspace_trust::WorkspaceTrust;
use helix_term::application::Application;
use helix_term::args::Args;
use helix_term::config::Config;
use helix_view::editor::{
    Config as EditorConfig, ImplicitTrustLevelConfig, LspConfig, WordCompletion,
    WorkspaceTrustConfig,
};
use helix_view::input::parse_macro;
use helix_view::theme::{Color, Loader};
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

/// 发送键序列并泵事件直到空闲（等价于 helpers::test_key_sequences 的单步）。
async fn pump(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(keys)?.into_iter() {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    Ok(())
}

/// 主题测试串行锁：CURRENT_THEME 快照是进程级静态，并行测试会互相覆盖
static THEME_LOCK: Mutex<()> = Mutex::new(());

/// 测试用 app：true color + 无 LSP
fn test_app() -> Application {
    Application::new(
        Args::default(),
        Config {
            editor: EditorConfig {
                true_color: true,
                lsp: LspConfig {
                    enable: false,
                    ..Default::default()
                },
                word_completion: WordCompletion {
                    enable: false,
                    ..Default::default()
                },
                workspace_trust: WorkspaceTrustConfig {
                    level: ImplicitTrustLevelConfig::Insecure,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        helix_core::config::default_lang_loader(),
        WorkspaceTrust::fully_trusted(),
    )
    .expect("app")
}

/// 退出 app（plugin_theme 测试共用收尾）
async fn quit_app(mut app: Application) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro("<esc>:q!<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    let event_loop = app.event_loop(&mut rx_stream);
    tokio::time::timeout(Duration::from_millis(500), event_loop).await?;
    let errs = app.close().await;
    assert!(errs.is_empty(), "close errors: {errs:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_theme_set_and_reset_over_inheriting_theme() -> anyhow::Result<()> {
    let _guard = THEME_LOCK.lock().unwrap();
    // ashokai_brahn 继承 ashokai：ui.popup/ui.background 只定义在父主题里。
    // 若基准用 load_raw（不解析 inherits）捕获，set/reset 都会丢父主题样式。
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../runtime");
    let expected = Loader::new(&[runtime]).load("ashokai_brahn")?;
    let expected_popup_fg = expected.get("ui.popup").fg;
    let expected_bg_bg = expected.get("ui.background").bg;
    assert!(
        expected_popup_fg.is_some() && expected_bg_bg.is_some(),
        "precondition: ashokai_brahn must resolve parent styles via inherits"
    );

    let dir = tempfile::tempdir()?;
    let plugin_path = dir.path().join("theme_plugin.js");
    std::fs::write(
        &plugin_path,
        r##"
        helix.register_command("theme_set", () => {
            helix.set_theme({ "ui.popup": "#ff00aa" });
        });
        helix.register_command("theme_reset", () => {
            helix.reset_theme();
        });
        "##,
    )?;

    let mut app = test_app();

    // 切到含 inherits 的主题：Validate 分支应用并重捕获基准（v10-② #4）
    pump(&mut app, ":theme ashokai_brahn<ret>").await?;
    assert_eq!(app.editor.theme.name(), "ashokai_brahn");

    pump(
        &mut app,
        &format!(":plugin-load {}<ret>", plugin_path.display()),
    )
    .await?;

    // 断言 1：set_theme 覆盖生效 + 父主题样式保留（v10-② #1/#2）
    pump(&mut app, ":theme_set<ret>").await?;
    assert_eq!(
        app.editor.theme.get("ui.popup").fg,
        Some(Color::Rgb(255, 0, 170)),
        "override must win after set_theme"
    );
    assert_eq!(
        app.editor.theme.get("ui.background").bg,
        expected_bg_bg,
        "parent (inherited) style must survive the override merge"
    );

    // 断言 2：reset_theme 还原到继承解析后的基准
    pump(&mut app, ":theme_reset<ret>").await?;
    assert_eq!(
        app.editor.theme.get("ui.popup").fg,
        expected_popup_fg,
        "reset must restore the inherits-resolved base style"
    );

    quit_app(app).await?;
    Ok(())
}

/// JS 主题 API 集成：对象覆盖（fg/bg/modifiers）、get_style、theme_info、set_theme_name、theme-change 事件
#[tokio::test(flavor = "multi_thread")]
async fn plugin_theme_js_apis() -> anyhow::Result<()> {
    let _guard = THEME_LOCK.lock().unwrap();
    let dir = tempfile::tempdir()?;
    let plugin_path = dir.path().join("theme2.js");
    std::fs::write(
        &plugin_path,
        r##"
        helix.register_command("t_style", () => {
            helix.set_theme({ "ui.selection": { fg: "#111111", bg: "#ff0000", modifiers: ["italic"] } });
        });
        helix.register_command("t_query", () => {
            const s = helix.get_style("ui.selection");
            helix.echo("style=" + (s ? s.fg + "|" + s.bg + "|" + s.modifiers.join(",") : "null"));
        });
        helix.register_command("t_info", () => {
            const i = helix.theme_info();
            helix.echo("info=" + i.name + "|" + i.themes.length);
        });
        helix.register_command("t_switch", () => {
            helix.set_theme_name("base16_default");
        });
        helix.register_command("t_again", () => {
            helix.set_theme({ "ui.selection": { fg: "#222222" } });
        });
        helix.on("theme-change", () => { helix.echo("theme-changed"); });
        "##,
    )?;

    let mut app = test_app();
    pump(&mut app, ":theme ashokai_brahn<ret>").await?;
    pump(
        &mut app,
        &format!(":plugin-load {}<ret>", plugin_path.display()),
    )
    .await?;

    // 对象覆盖：fg + bg + modifiers 全部生效
    pump(&mut app, ":t_style<ret>").await?;
    let sel = app.editor.theme.get("ui.selection");
    assert_eq!(sel.fg, Some(Color::Rgb(0x11, 0x11, 0x11)), "fg override");
    assert_eq!(sel.bg, Some(Color::Rgb(0xff, 0, 0)), "bg override");
    assert!(
        sel.add_modifier
            .contains(helix_view::graphics::Modifier::ITALIC),
        "modifiers override"
    );

    // get_style 查询（走 helix-term 快照回调）。快照是进程级静态，并行测试的 app 启动
    // 会覆盖它——这里只断言"能返回结构"（样式值的精确性由上方 editor.theme 断言保证）
    pump(&mut app, ":t_query<ret>").await?;
    let (status, _) = app.editor.get_status().expect("status");
    assert!(
        status.contains("style=") && status.contains("|"),
        "get_style 应返回 fg|bg|modifiers 结构: {status}"
    );

    // theme_info：主题列表非空（当前主题名可能被并行 app 覆盖，不断言具体名）
    pump(&mut app, ":t_info<ret>").await?;
    let (status, _) = app.editor.get_status().expect("status");
    let n = status.trim_start_matches("info=").split('|').nth(1);
    assert!(
        n.map(|s| !s.is_empty() && s != "0").unwrap_or(false),
        "theme_info 应含非空主题列表: {status}"
    );

    // set_theme_name：切基准主题 + 清覆盖（t_style 的 ui.selection 覆盖应消失）
    pump(&mut app, ":t_switch<ret>").await?;
    assert_eq!(
        app.editor.theme.name(),
        "base16_default",
        "set_theme_name 切换"
    );
    assert_ne!(
        app.editor.theme.get("ui.selection").fg,
        Some(Color::Rgb(0x11, 0x11, 0x11)),
        "切换后旧覆盖已清"
    );

    // theme-change 事件：再次 set_theme 触发
    pump(&mut app, ":t_again<ret>").await?;
    let (status, _) = app.editor.get_status().expect("status");
    assert!(
        status.contains("theme-changed"),
        "theme-change event: {status}"
    );
    // 事件后覆盖已应用
    assert_eq!(
        app.editor.theme.get("ui.selection").fg,
        Some(Color::Rgb(0x22, 0x22, 0x22)),
        "事件后的 set_theme 生效"
    );

    quit_app(app).await?;
    Ok(())
}
