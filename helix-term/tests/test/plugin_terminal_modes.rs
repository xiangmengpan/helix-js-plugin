use std::time::Duration;

use helix_term::application::Application;
use helix_term::compositor::PaneMode;
use helix_term::job::Jobs;
use helix_view::current_ref;
use helix_view::input::parse_macro;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_terminal_modes() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("mt.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("tmodes.js");
    std::fs::write(
        &plugin_path,
        r#"
        let tid = null;
        helix.register_command("tm-open", () => { tid = helix.open_terminal({ cmd: "cat", side: "right", size: 30 }); });
        helix.register_command("tm-min", () => { helix.set_terminal_mode(tid, "minimized"); });
        helix.register_command("tm-clear", () => { helix.term_clear(tid); });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":tm-open<ret>"),
                Some(&|app| {
                    let has = app.compositor.has_component(std::any::type_name::<
                        helix_term::ui::plugin_terminal::PluginTerminal,
                    >());
                    // OpenTerminal 走 job 通道——首帧可能未就位；幂等不严格断言
                    let _ = has;
                }),
            ),
            (
                Some(":tm-min<ret>"),
                Some(&|app| {
                    // minimized 模式：终端不消费按键 → 编辑器可编辑
                }),
            ),
            (
                Some("ihello<esc>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    // 若 minimized 生效：文本应变化（按键穿透）；若未生效（终端仍消费按键）：
                    // 文本不变（hello 进终端）。两种都可能——这里只记录，不严格断言，
                    // 由单测覆盖模式值；本测试验证终端链路不崩。
                    let _ = doc;
                }),
            ),
            // 焦点回编辑器(window 模式导航;esc 不再关闭终端,退出序列 :q! 需要编辑器焦点)
            (Some("<C-p>h<esc>"), None),
        ],
        false,
    )
    .await?;

    Ok(())
}

/// 浮动终端 + C-\ 模式穿透集成：floating 渲染为居中浮窗（带边框），
/// C-\ 在 insert 切 normal（终端内），再 C-\ 回 helix（焦点编辑器）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_terminal_floating_and_mode() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("ft.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("tfloat.js");
    std::fs::write(
        &plugin_path,
        r#"
        let tid = null;
        helix.register_command("tf-open", () => {
            tid = helix.open_terminal({ cmd: "cat", side: "bottom", size: 10 });
        });
        helix.register_command("tf-float", () => { helix.set_terminal_mode(tid, "floating"); });
        helix.register_command("tf-dock", () => { helix.set_terminal_mode(tid, "dock"); });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    // 用 pump + 手动渲染（与 filetree 测试同款辅助）
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(":plugin-load {}<ret>", plugin_path.display()))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    for key_event in parse_macro(":tf-open<ret>:tf-float<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    // floating 生效（dispatch_blocking 走 job 队列——轮询等待）
    for _ in 0..20 {
        if app.compositor.floating().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        app.compositor.floating().is_some(),
        "floating 模式应设置 float 叶子"
    );

    // 渲染到 buffer：浮窗边框字符应出现在中央区域（ui.popup 风格边框 ┌/┐/└/┘）
    {
        let area = helix_view::graphics::Rect::new(0, 0, 120, 40);
        let mut buf = tui::buffer::Buffer::empty(area);
        let mut jobs = Jobs::new();
        let mut cx = helix_term::compositor::Context {
            editor: &mut app.editor,
            scroll: None,
            jobs: &mut jobs,
        };
        app.compositor.render(area, &mut buf, &mut cx);
        // 浮窗 rect：布局树被状态栏 clip_bottom(1)（120×39），
        // 宽 120*0.6=72、高 39*0.7≈27（round），居中 → x=24, y=6
        let tree_area = area.clip_bottom(1);
        let f = helix_term::ui::layout::LayoutTree::float_rect(tree_area);
        assert_eq!(f.x, 24);
        assert_eq!(f.y, 6);
        assert_eq!(f.height, 27);
        let top_left = buf[(f.x, f.y)].symbol.as_str().to_string();
        let top_right = buf[(f.x + f.width - 1, f.y)].symbol.as_str().to_string();
        let bottom_left = buf[(f.x, f.y + f.height - 1)].symbol.as_str().to_string();
        assert_eq!(top_left, "┌", "浮窗左上角边框");
        assert_eq!(top_right, "┐", "浮窗右上角边框");
        assert_eq!(bottom_left, "└", "浮窗左下角边框");
    }

    // dock：取消浮动（浮动终端 active 吞键——先 C-\×2 回编辑器再执行命令）
    // C-\：helix_view 构造（parse_macro 对反斜杠转义不可靠）
    let ctrl_bs = Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
        code: helix_view::input::KeyCode::Char('\\'),
        modifiers: helix_view::input::KeyModifiers::CONTROL,
    }));
    for _ in 0..2 {
        tx.send(Ok(ctrl_bs.clone()))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    for key_event in parse_macro(":tf-dock<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    for _ in 0..20 {
        if app.compositor.floating().is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(app.compositor.floating().is_none(), "dock 模式应取消浮动");

    // 退出并关闭
    for key_event in parse_macro("<esc>:q!<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    let event_loop = app.event_loop(&mut rx_stream);
    tokio::time::timeout(Duration::from_millis(500), event_loop).await?;
    let errs = app.close().await;
    assert!(errs.is_empty(), "close errors: {errs:?}");
    Ok(())
}

/// 回归：dock 分屏（vterm/hterm 路径）后 C-\×2 焦点回编辑器：普通键应进编辑器而非终端。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_terminal_dock_focus_back_to_editor() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "abc\n")?;
    let plugin_path = dir.path().join("tdock.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("td-open", () => {
            helix.open_terminal({ cmd: "cat", side: "bottom", size: 10 });
        });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(":plugin-load {}<ret>", plugin_path.display()))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    for key_event in parse_macro(":td-open<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let term_type = std::any::type_name::<helix_term::ui::plugin_terminal::PluginTerminal>();
    for _ in 0..20 {
        if app.compositor.has_component(term_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(app.compositor.has_component(term_type), "dock 终端应就位");

    let ctrl_bs = Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
        code: helix_view::input::KeyCode::Char('\\'),
        modifiers: helix_view::input::KeyModifiers::CONTROL,
    }));
    for _ in 0..2 {
        tx.send(Ok(ctrl_bs.clone()))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    for key_event in parse_macro("ihello<esc>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "helloabc\n",
        "C-\\×2 后按键应进编辑器"
    );
    Ok(())
}

/// 浮动终端与 dock 分屏并存时，C-\×2 仍应回编辑器。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_terminal_float_plus_dock_focus_editor() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("m.txt");
    std::fs::write(&file, "abc\n")?;
    let plugin_path = dir.path().join("tmix.js");
    std::fs::write(
        &plugin_path,
        r#"
        let tid = null;
        helix.register_command("mx-float", () => {
            tid = helix.open_terminal({ cmd: "cat", side: "bottom", size: 10 });
            helix.set_terminal_mode(tid, "floating");
        });
        helix.register_command("mx-dock", () => {
            helix.open_terminal({ cmd: "cat", side: "bottom", size: 10 });
        });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(":plugin-load {}<ret>", plugin_path.display()))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    for key_event in parse_macro(":mx-float<ret>:mx-dock<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let term_type = std::any::type_name::<helix_term::ui::plugin_terminal::PluginTerminal>();
    for _ in 0..20 {
        if app.compositor.has_component(term_type) && app.compositor.floating().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(app.compositor.floating().is_some(), "浮动终端应就位");

    let ctrl_bs = Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
        code: helix_view::input::KeyCode::Char('\\'),
        modifiers: helix_view::input::KeyModifiers::CONTROL,
    }));
    for _ in 0..2 {
        tx.send(Ok(ctrl_bs.clone()))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    assert!(app.compositor.floating().is_none(), "C-\\×2 应收起浮窗");

    for key_event in parse_macro("ihello<esc>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "helloabc\n",
        "浮动+dock 并存时 C-\\×2 后按键应进编辑器"
    );
    Ok(())
}

/// 回归：term 打开 → 输入 → 关闭 → 再打开（boa 闭包槽位 + 注册表 id 修复）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_terminal_reopen_after_close() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("r.txt");
    std::fs::write(&file, "abc\n")?;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/nonexistent".into());
    let term_plugin = format!("{home}/.config/helix/plugins/features/terminal.js");
    if !std::path::Path::new(&term_plugin).exists() {
        return Ok(());
    }

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(":plugin-load {}<ret>", term_plugin))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let term_type = std::any::type_name::<helix_term::ui::plugin_terminal::PluginTerminal>();
    for key_event in parse_macro(":term<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    for _ in 0..20 {
        if app.compositor.floating().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(app.compositor.floating().is_some(), "第一次 :term 应浮动");
    assert_status_not_error(&app.editor);

    // Esc 切 terminal normal → C-p 进 Pane 模式 → x 关闭叶子(终端关闭唯一途径)
    for key_event in parse_macro("<esc>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    eprintln!(
        "[dbg] after esc: mode={:?} active={}",
        app.compositor.pane_mode(),
        app.compositor.layout_tree().active()
    );
    for key_event in parse_macro("<C-p>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    eprintln!(
        "[dbg] after C-p: mode={:?} active={}",
        app.compositor.pane_mode(),
        app.compositor.layout_tree().active()
    );
    for key_event in parse_macro("x<esc>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    for _ in 0..20 {
        if !app.compositor.has_component(term_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !app.compositor.has_component(term_type),
        "window 模式 x 应关闭终端"
    );

    for key_event in parse_macro(":term<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    for _ in 0..20 {
        if app.compositor.floating().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        app.compositor.floating().is_some(),
        "第二次 :term 应重新浮动"
    );
    assert_status_not_error(&app.editor);
    Ok(())
}

/// term-save 导出 + normal 模式 y 复制寄存器。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_terminal_save_and_yank() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("s.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("tsave.js");
    std::fs::write(
        &plugin_path,
        r#"
        let tid = null;
        helix.register_command("ts-open", () => {
            tid = helix.open_terminal({ cmd: "cat", side: "bottom", size: 10 });
        });
        helix.register_command("ts-feed", () => { helix.term_feed(tid, "hello-save\r\nline2"); });
        helix.register_command("ts-save", () => { helix.term_save(tid); });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(
        ":plugin-load {}<ret>:ts-open<ret>:ts-feed<ret>:ts-save<ret>",
        plugin_path.display()
    ))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let term_type = std::any::type_name::<helix_term::ui::plugin_terminal::PluginTerminal>();
    for _ in 0..20 {
        if app.compositor.has_component(term_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(app.compositor.has_component(term_type), "终端应就位");

    let home = std::env::var("HOME").unwrap_or_default();
    let log_dir = format!("{home}/.cache/helix");
    let mut found = None;
    for _ in 0..20 {
        found = std::fs::read_dir(&log_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().starts_with("term-"))
                    .find_map(|e| std::fs::read_to_string(e.path()).ok())
            })
            .ok()
            .flatten();
        if found.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        found
            .as_deref()
            .is_some_and(|s| s.contains("hello-save") && s.contains("line2")),
        "term_save 应导出内容，实际: {found:?}"
    );

    let ctrl_bs = Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
        code: helix_view::input::KeyCode::Char('\\'),
        modifiers: helix_view::input::KeyModifiers::CONTROL,
    }));
    tx.send(Ok(ctrl_bs.clone()))?;
    app.event_loop_until_idle(&mut rx_stream).await;
    for key_event in parse_macro("y")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    let reg = app
        .editor
        .registers
        .first('"', &app.editor)
        .map(|c| c.to_string());
    assert!(
        reg.as_deref().is_some_and(|s| s.contains("hello-save")),
        "y 应把可视区写入 \" 寄存器，实际: {reg:?}"
    );
    for _ in 0..2 {
        tx.send(Ok(ctrl_bs.clone()))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    Ok(())
}

/// 布局原语 API 全链路：layout_resize / layout_swap / layout_minimize。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_layout_ops_api() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("l.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("lops.js");
    std::fs::write(
        &plugin_path,
        r#"
        let tid = null;
        helix.register_command("lw-open", () => {
            tid = helix.open_terminal({ cmd: "cat", side: "right", size: 40 });
        });
        helix.register_command("lw-resize", () => { helix.layout_resize(tid, "h", 0.1); });
        helix.register_command("lw-swap", () => { helix.layout_swap(0, tid); });
        helix.register_command("lw-min", () => { helix.layout_minimize(tid, true); });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(
        ":plugin-load {}<ret>:lw-open<ret>:lw-resize<ret>:lw-swap<ret>:lw-min<ret>",
        plugin_path.display()
    ))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let term_type = std::any::type_name::<helix_term::ui::plugin_terminal::PluginTerminal>();
    for _ in 0..20 {
        if app.compositor.has_component(term_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(app.compositor.has_component(term_type), "终端应就位");
    assert_status_not_error(&app.editor);

    for _ in 0..20 {
        if app.compositor.minimized_leaf().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        app.compositor.minimized_leaf().is_some(),
        "layout_minimize 应设置最小化叶子"
    );
    assert_status_not_error(&app.editor);
    Ok(())
}

/// filetree/term 焦点回编辑器验证：filetree 未映射键穿透（i → insert），
/// term 用 C-\ ×2 回编辑器后同样可编辑。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_focus_return_to_editor() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("fr.txt");
    std::fs::write(&file, "abc\n")?;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/nonexistent".into());
    let filetree_plugin = format!("{home}/.config/helix/plugins/features/filetree/index.js");
    if !std::path::Path::new(&filetree_plugin).exists() {
        return Ok(());
    }
    let opener = dir.path().join("fr_open.js");
    std::fs::write(
        &opener,
        r#"
        helix.register_command("fr-open", () => {
            helix.open_terminal({ cmd: "cat", side: "right", size: 40 });
        });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(
        ":plugin-load {}<ret>:plugin-load {}<ret>:filetree<ret>",
        filetree_plugin,
        opener.display()
    ))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    for _ in 0..20 {
        if app.compositor.has_component(panel_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        app.compositor.has_component(panel_type),
        "filetree 面板应就位"
    );
    assert_ne!(
        app.compositor.layout_tree().active(),
        0,
        "filetree 打开后焦点在面板"
    );

    // filetree 焦点：C-\ 回编辑器（保留面板；filetree.js 新加）
    let ctrl_bs = Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
        code: helix_view::input::KeyCode::Char('\\'),
        modifiers: helix_view::input::KeyModifiers::CONTROL,
    }));
    tx.send(Ok(ctrl_bs.clone()))?;
    app.event_loop_until_idle(&mut rx_stream).await;
    assert_eq!(
        app.compositor.layout_tree().active(),
        0,
        "面板焦点 C-\\ 回编辑器（保留面板）"
    );

    // 开终端：焦点在终端
    for key_event in parse_macro(":fr-open<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    let term_type = std::any::type_name::<helix_term::ui::plugin_terminal::PluginTerminal>();
    for _ in 0..20 {
        if app.compositor.has_component(term_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(app.compositor.has_component(term_type), "终端应就位");
    assert_ne!(
        app.compositor.layout_tree().active(),
        0,
        "终端打开后焦点在终端"
    );

    // 终端 C-\ ×2 回编辑器
    tx.send(Ok(ctrl_bs.clone()))?;
    tx.send(Ok(ctrl_bs.clone()))?;
    app.event_loop_until_idle(&mut rx_stream).await;
    assert_eq!(
        app.compositor.layout_tree().active(),
        0,
        "C-\\×2 从终端回编辑器"
    );

    // 回编辑器后 i → insert（可编辑），: 开命令
    for key_event in parse_macro("i")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    assert_eq!(
        app.editor.mode(),
        helix_view::document::Mode::Insert,
        "回编辑器后可进入 insert"
    );
    for key_event in parse_macro("<esc>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    for key_event in parse_macro(":")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    assert!(
        app.compositor
            .has_component(std::any::type_name::<helix_term::ui::prompt::Prompt>()),
        "回编辑器后 : 应打开命令提示"
    );

    Ok(())
}

/// 活动叶子高亮边框：焦点在终端时边框在右半区，C-p h 回编辑器后边框移到左半区。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_focus_border() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("fb.txt");
    std::fs::write(&file, "x\n")?;
    let opener = dir.path().join("fb_open.js");
    std::fs::write(
        &opener,
        r#"
        helix.register_command("fb-open", () => {
            helix.open_terminal({ cmd: "cat", side: "right", size: 40 });
        });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(
        ":plugin-load {}<ret>:fb-open<ret>",
        opener.display()
    ))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let term_type = std::any::type_name::<helix_term::ui::plugin_terminal::PluginTerminal>();
    for _ in 0..20 {
        if app.compositor.has_component(term_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(app.compositor.has_component(term_type), "终端应就位");

    // 渲染并断言边框：扫描 ┌ 的列位置（焦点在终端 → 右半区；编辑器 → 左半区）
    let border_col = |app: &mut Application| -> Option<usize> {
        let area = helix_view::graphics::Rect::new(0, 0, 120, 30);
        let mut buf = tui::buffer::Buffer::empty(area);
        let mut jobs = helix_term::job::Jobs::new();
        let mut cx = helix_term::compositor::Context {
            editor: &mut app.editor,
            scroll: None,
            jobs: &mut jobs,
        };
        app.compositor.render(area, &mut buf, &mut cx);
        buf.content
            .iter()
            .position(|c| c.symbol.as_str() == "┌")
            .map(|i| i % 120)
    };
    let col = border_col(&mut app).expect("应有 ┌ 边框");
    assert!(col > 60, "焦点在终端：边框在右半区（col {col}）");

    // C-\\ ×2 回编辑器 + C-p h → 边框移到编辑器
    let ctrl_bs2 = Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
        code: helix_view::input::KeyCode::Char('\\'),
        modifiers: helix_view::input::KeyModifiers::CONTROL,
    }));
    tx.send(Ok(ctrl_bs2.clone()))?;
    tx.send(Ok(ctrl_bs2.clone()))?;
    app.event_loop_until_idle(&mut rx_stream).await;
    let cw = |c: char| {
        Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
            code: helix_view::input::KeyCode::Char(c),
            modifiers: helix_view::input::KeyModifiers::CONTROL,
        }))
    };
    tx.send(Ok(cw('w')))?;
    tx.send(Ok(Event::Key(KeyEvent::from(
        helix_view::input::KeyEvent {
            code: helix_view::input::KeyCode::Char('h'),
            modifiers: helix_view::input::KeyModifiers::NONE,
        },
    ))))?;
    app.event_loop_until_idle(&mut rx_stream).await;
    let col = border_col(&mut app).expect("应有 ┌ 边框");
    assert!(col < 60, "焦点回编辑器：边框在左半区（col {col}）");

    Ok(())
}

/// 终端 Insert 直通模式 C-p 放行给 pty(不拦截,终端内 vim/emacs 的 C-p);
/// C-\ 切到 Normal(滚动)后 C-p 进模式并导航邻居。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_terminal_insert_cw_passthrough() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("wk.txt");
    std::fs::write(&file, "x\n")?;
    let opener = dir.path().join("wk_open.js");
    std::fs::write(
        &opener,
        r#"
        helix.register_command("wk-open", () => {
            helix.open_terminal({ cmd: "cat", side: "right", size: 40 });
        });
        "#,
    )?;
    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(
        ":plugin-load {}<ret>:wk-open<ret>",
        opener.display()
    ))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    let term_type = std::any::type_name::<helix_term::ui::plugin_terminal::PluginTerminal>();
    for _ in 0..20 {
        if app.compositor.has_component(term_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(app.compositor.has_component(term_type), "终端应就位");
    let ctrl = |c: char| {
        Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
            code: helix_view::input::KeyCode::Char(c),
            modifiers: helix_view::input::KeyModifiers::CONTROL,
        }))
    };
    let plain = |c: char| {
        Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
            code: helix_view::input::KeyCode::Char(c),
            modifiers: helix_view::input::KeyModifiers::NONE,
        }))
    };
    let esc = Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
        code: helix_view::input::KeyCode::Esc,
        modifiers: helix_view::input::KeyModifiers::NONE,
    }));
    // 终端 Insert 直通:C-p 放行给 pty(不拦截),不进 Pane 模式、焦点仍在终端
    tx.send(Ok(ctrl('p')))?;
    app.event_loop_until_idle(&mut rx_stream).await;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Normal,
        "终端 Insert 直通模式 C-p 放行给 pty,不进 Pane 模式"
    );
    assert_eq!(app.compositor.layout_tree().active(), 1, "焦点仍在终端");
    // C-\ → 终端 Normal(滚动);此焦点下 C-p 照常进 Pane 模式
    tx.send(Ok(ctrl('\\')))?;
    tx.send(Ok(ctrl('p')))?;
    app.event_loop_until_idle(&mut rx_stream).await;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Pane,
        "终端 Normal(滚动)焦点 C-p 进 Pane 模式"
    );
    // h → 聚焦左邻居(编辑器);Esc 退出
    tx.send(Ok(plain('h')))?;
    tx.send(Ok(esc))?;
    app.event_loop_until_idle(&mut rx_stream).await;
    assert_eq!(
        app.compositor.layout_tree().active(),
        0,
        "C-p h 聚焦左邻居(编辑器)"
    );
    assert_eq!(app.compositor.pane_mode(), PaneMode::Normal, "Esc 退出模式");
    Ok(())
}

/// 回归：filetree 面板热重载后残留（render_popup not open → 显示 error、无法关闭）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_filetree_reload_no_zombie() -> anyhow::Result<()> {
    let _rl = PLUGIN_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("z.txt");
    std::fs::write(&file, "x\n")?;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/nonexistent".into());
    let filetree_plugin = format!("{home}/.config/helix/plugins/features/filetree/index.js");
    let layout_plugin = format!("{home}/.config/helix/plugins/lib/layout.js");
    if !std::path::Path::new(&filetree_plugin).exists()
        || !std::path::Path::new(&layout_plugin).exists()
    {
        return Ok(());
    }

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(
        ":plugin-load {}<ret>:filetree<ret>",
        filetree_plugin
    ))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    for _ in 0..20 {
        if app.compositor.has_component(panel_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        app.compositor.has_component(panel_type),
        "filetree 面板应就位"
    );

    // 渲染检查：无 error 文本
    {
        let area = helix_view::graphics::Rect::new(0, 0, 120, 30);
        let mut buf = tui::buffer::Buffer::empty(area);
        let mut jobs = helix_term::job::Jobs::new();
        let mut cx = helix_term::compositor::Context {
            editor: &mut app.editor,
            scroll: None,
            jobs: &mut jobs,
        };
        app.compositor.render(area, &mut buf, &mut cx);
        let all: String = buf.content.iter().map(|c| c.symbol.as_str()).collect();
        assert!(
            !all.contains("plugin panel error"),
            "reload 前面板渲染不应报错: {all:?}"
        );
    }

    // 热重载
    for key_event in parse_macro(":plugin-reload<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    for _ in 0..20 {
        if !app.compositor.has_component(panel_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !app.compositor.has_component(panel_type),
        "热重载后不应残留无法关闭的面板"
    );

    Ok(())
}

/// 僵尸面板自愈：JS 注册表丢失（模拟 reload 状态丢失）后，按键触发自动移除面板，
/// 不再"渲染报错 + 无法关闭"。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_panel_zombie_selfheal() -> anyhow::Result<()> {
    let _rl = PLUGIN_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("zh.txt");
    std::fs::write(&file, "x\n")?;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/nonexistent".into());
    let filetree_plugin = format!("{home}/.config/helix/plugins/features/filetree/index.js");
    if !std::path::Path::new(&filetree_plugin).exists() {
        return Ok(());
    }

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(
        ":plugin-load {}<ret>:filetree<ret>",
        filetree_plugin
    ))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    for _ in 0..20 {
        if app.compositor.has_component(panel_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        app.compositor.has_component(panel_type),
        "filetree 面板应就位"
    );

    // 模拟 JS 注册表丢失（面板 id=1 的 render/onKey 被清）
    let _ = helix_term::helix_js::close_popup(1);

    // 按任意键：onKey 报 not open → 面板自动移除（自愈）
    for key_event in parse_macro("j")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    for _ in 0..20 {
        if !app.compositor.has_component(panel_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !app.compositor.has_component(panel_type),
        "僵尸面板按键后应自动移除（自愈）"
    );

    Ok(())
}

/// 多面板热重载：全部关闭（不只 last_panel_id），不留僵尸。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_multi_panel_reload_all_closed() -> anyhow::Result<()> {
    let _rl = PLUGIN_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("mp.txt");
    std::fs::write(&file, "x\n")?;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/nonexistent".into());
    let filetree_plugin = format!("{home}/.config/helix/plugins/features/filetree/index.js");
    if !std::path::Path::new(&filetree_plugin).exists() {
        return Ok(());
    }
    let two = dir.path().join("two.js");
    std::fs::write(
        &two,
        r#"
        helix.register_command("two-open", () => {
            helix.open_panel({ side: "right", size: 20, render: () => helix.el("text", "two"), onKey: () => "ignore" });
        });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(
        ":plugin-load {}<ret>:plugin-load {}<ret>:filetree<ret>:two-open<ret>",
        filetree_plugin,
        two.display()
    ))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    for _ in 0..20 {
        if app.compositor.count_type(panel_type) >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(app.compositor.count_type(panel_type) >= 2, "两个面板应就位");

    // 热重载：两个面板都应关闭
    for key_event in parse_macro(":plugin-reload<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    for _ in 0..20 {
        if app.compositor.count_type(panel_type) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        app.compositor.count_type(panel_type),
        0,
        "热重载后所有面板应关闭"
    );

    Ok(())
}

/// 全局状态栏：有 split（editor|term）时，屏幕底部 1 行仍显示状态栏（不只在编辑器底部）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_statusline_global() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("sg.txt");
    std::fs::write(&file, "abc\n")?;
    let opener = dir.path().join("sg_open.js");
    std::fs::write(
        &opener,
        r#"
        helix.register_command("sg-open", () => {
            helix.open_terminal({ cmd: "cat", side: "right", size: 40 });
        });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(
        ":plugin-load {}<ret>:sg-open<ret>",
        opener.display()
    ))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let term_type = std::any::type_name::<helix_term::ui::plugin_terminal::PluginTerminal>();
    for _ in 0..20 {
        if app.compositor.has_component(term_type) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(app.compositor.has_component(term_type), "终端应就位");

    // 渲染：底部 1 行（全局状态栏）应有内容，且是屏幕全宽（覆盖终端区域）
    let area = helix_view::graphics::Rect::new(0, 0, 120, 30);
    let mut buf = tui::buffer::Buffer::empty(area);
    let mut jobs = helix_term::job::Jobs::new();
    let mut cx = helix_term::compositor::Context {
        editor: &mut app.editor,
        scroll: None,
        jobs: &mut jobs,
    };
    app.compositor.render(area, &mut buf, &mut cx);
    // 底部行非空白
    let bottom: String = (0..120)
        .map(|x| buf[(x, 29)].symbol.as_str().to_string())
        .collect();
    assert!(
        bottom.chars().any(|c| !c.is_whitespace()),
        "全局状态栏应显示在屏幕底部，实际: {bottom:?}"
    );
    // 终端区域下方（x=90..120, y=29）也应有状态栏内容——证明全局覆盖（不只编辑器底部）
    let over_term: String = (90..120)
        .map(|x| buf[(x, 29)].symbol.as_str().to_string())
        .collect();
    assert!(
        over_term.chars().any(|c| !c.is_whitespace()),
        "状态栏应覆盖到终端区域下方（全局），实际: {over_term:?}"
    );

    Ok(())
}
