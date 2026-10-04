use super::*;
use helix_view::current_ref;

use helix_term::application::Application;
use helix_term::job::Jobs;
use helix_view::input::parse_macro;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

/// 发送键序列并泵事件直到空闲（同 plugin_multipanel::pump）
async fn pump(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(keys)?.into_iter() {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    Ok(())
}

/// 渲染 compositor 到 Buffer，返回所有行的拼接（空格保留）。
fn render_rows(app: &mut Application, area: helix_view::graphics::Rect) -> Vec<String> {
    let mut buf = tui::buffer::Buffer::empty(area);
    let mut jobs = Jobs::new();
    let mut cx = helix_term::compositor::Context {
        editor: &mut app.editor,
        scroll: None,
        jobs: &mut jobs,
    };
    app.compositor.render(area, &mut buf, &mut cx);
    (0..area.height)
        .map(|y| {
            buf.content
                .iter()
                .skip(y as usize * area.width as usize)
                .take(area.width as usize)
                .map(|c| c.symbol.as_str())
                .collect::<String>()
        })
        .collect()
}

/// 弹窗 render 返回组件树 → 布局进 Buffer：
/// - 某行 title（error 样式）
/// - 某行 row 两列文本并排（"left right"，gap 1）
/// - 某行 scroll 保留最后一行（"s2"，无 "s1"）
#[tokio::test(flavor = "multi_thread")]
async fn popup_component_tree_renders_layout() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("ct.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("components.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("tree-popup", () => {
            helix.open_popup({
                render: () => helix.el("col", [
                    helix.el("text", "title", { style: "error" }),
                    helix.el("row", [
                        helix.el("text", "left"),
                        helix.el("text", "right", { width: 5 }),
                    ], { gap: 1 }),
                    helix.el("scroll", [helix.el("text", "s1"), helix.el("text", "s2")], { height: 1 }),
                ]),
            });
        });
        "#,
    )?;

    let area = helix_view::graphics::Rect::new(0, 0, 120, 30);
    let mut app = AppBuilder::new().with_file(file, None).build()?;
    pump(
        &mut app,
        &format!(":plugin-load {}<ret>", plugin_path.display()),
    )
    .await?;
    pump(&mut app, ":tree-popup<ret>").await?;

    // DiffRenderer 有持久状态：app 真实 surface 已渲染过，不能复用其 diff——
    // 从弹窗层读 lines，用全新 DiffRenderer 渲染到测试 buffer。
    let popup = app
        .compositor
        .find::<helix_term::ui::Popup<helix_term::ui::PluginPopup>>()
        .expect("popup layer");
    let lines: Vec<helix_js::StyledLine> = popup.contents().lines().to_vec();
    let mut buf = tui::buffer::Buffer::empty(area);
    let mut diff = helix_term::ui::comp_layout::DiffRenderer::default();
    diff.render(&lines, area, &mut buf, &app.editor.theme);
    let rows: Vec<String> = (0..area.height)
        .map(|y| {
            buf.content
                .iter()
                .skip(y as usize * area.width as usize)
                .take(area.width as usize)
                .map(|c| c.symbol.as_str())
                .collect::<String>()
        })
        .collect();
    let joined = rows.join("\n");
    assert!(
        rows.iter().any(|r| r.contains("title")),
        "title row missing: {joined:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("left right")),
        "row columns side by side missing: {joined:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains('s') && r.contains("s2")),
        "scroll keeps last line s2: {joined:?}"
    );
    assert!(!joined.contains("s1"), "scroll must drop s1: {joined:?}");

    // 退出并关闭
    pump(&mut app, "<esc>:q!<ret>").await?;
    let errs = app.close().await;
    assert!(errs.is_empty(), "close errors: {errs:?}");

    Ok(())
}

/// 弹窗 render 返回 scroll + offset → 全量列表只显示 offset 窗口：
/// 100 行 + offset=42 + height=5 → 面板显示 row42..row46（无 row0/row99）
#[tokio::test(flavor = "multi_thread")]
async fn popup_scroll_offset_shows_window() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("so.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("scroll.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("scroll-popup", () => {
            const rows = [];
            for (let i = 0; i < 100; i++) rows.push(helix.el("text", "row" + i));
            helix.open_popup({ render: () => helix.el("scroll", rows, { height: 5, offset: 42 }) });
        });
        "#,
    )?;

    let area = helix_view::graphics::Rect::new(0, 0, 120, 30);
    let mut app = AppBuilder::new().with_file(file, None).build()?;
    pump(
        &mut app,
        &format!(":plugin-load {}<ret>", plugin_path.display()),
    )
    .await?;
    pump(&mut app, ":scroll-popup<ret>").await?;

    // DiffRenderer 有持久状态：app 真实 surface 已渲染过，不能复用其 diff——
    // 从弹窗层读 lines，用全新 DiffRenderer 渲染到测试 buffer。
    let popup = app
        .compositor
        .find::<helix_term::ui::Popup<helix_term::ui::PluginPopup>>()
        .expect("popup layer");
    let lines: Vec<helix_js::StyledLine> = popup.contents().lines().to_vec();
    assert_eq!(
        lines.len(),
        5,
        "scroll window must be exactly 5 rows, got {}",
        lines.len()
    );
    let mut buf = tui::buffer::Buffer::empty(area);
    let mut diff = helix_term::ui::comp_layout::DiffRenderer::default();
    diff.render(&lines, area, &mut buf, &app.editor.theme);
    let rows: Vec<String> = (0..area.height)
        .map(|y| {
            buf.content
                .iter()
                .skip(y as usize * area.width as usize)
                .take(area.width as usize)
                .map(|c| c.symbol.as_str())
                .collect::<String>()
        })
        .collect();
    let shown: Vec<String> = rows
        .iter()
        .filter(|r| !r.trim().is_empty())
        .map(|r| r.trim().to_string())
        .collect();
    let expect: Vec<String> = (42..47).map(|i| format!("row{i}")).collect();
    assert_eq!(
        shown, expect,
        "offset window must show row42..row46 only, rows: {rows:?}"
    );

    // 退出并关闭
    pump(&mut app, "<esc>:q!<ret>").await?;
    let errs = app.close().await;
    assert!(errs.is_empty(), "close errors: {errs:?}");

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_node_focus_events() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("nf.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("focus.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("focus-popup", () => {
            helix.open_popup({
                render: (focus) => helix.el("col", [
                    helix.el("button", "run", { id: "btn1", onPress: () => helix.echo("PRESSED"), style: focus === "btn1" ? "error" : null }),
                    helix.el("button", "cancel", { id: "btn2", onPress: () => helix.echo("CANCELED") }),
                ]),
            });
        });
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
                Some(":focus-popup<ret>"),
                Some(&|app| {
                    let popup_type =
                        std::any::type_name::<helix_term::ui::Popup<helix_term::ui::PluginPopup>>();
                    assert!(app.compositor.has_component(popup_type), "popup open");
                }),
            ),
            // Tab 聚焦第一个按钮（btn1），Enter 触发 onPress
            (
                Some("<tab><ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(
                        status.as_ref(),
                        "PRESSED",
                        "onPress should fire on Enter after Tab-focus"
                    );
                    assert!(matches!(severity, helix_core::diagnostic::Severity::Info));
                }),
            ),
            // 再 Tab 聚焦 btn2，Enter → CANCELED
            (
                Some("<tab><ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "CANCELED");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_open_file_from_panel_key() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    // 面板 onKey 里调 helix.open_file → 文件必须在 buffer 打开（UI 请求即时应用）
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("pf.txt");
    std::fs::write(&file, "x\n")?;
    let target = dir.path().join("target.txt");
    std::fs::write(&target, "TARGET-CONTENT\n")?;
    let plugin_path = dir.path().join("openfile.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("pf-panel", () => {
            helix.open_panel({
                side: "right", size: 30,
                render: () => ["enter to open"],
                onKey: (key) => {
                    if (key.name === "Enter") { helix.open_file("TARGET"); return "handled"; }
                    return "ignore";
                },
            });
        });
        "#,
        // TARGET 用路径拼接注入
    )?;
    // 注入 TARGET 绝对路径
    let plugin_src = std::fs::read_to_string(&plugin_path)?;
    let plugin_src = plugin_src.replace("TARGET", &target.display().to_string());
    std::fs::write(&plugin_path, plugin_src)?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (Some(":pf-panel<ret>"), None),
            (Some("<ret>"), None),
            // open_file 走 job 通道异步——settle 一轮再断言
            (
                Some("<esc>"),
                Some(&|app| {
                    // 目标文件应已打开为当前文档
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(
                        doc.text().to_string(),
                        "TARGET-CONTENT\n",
                        "open_file from panel onKey should open the file in a buffer"
                    );
                    assert!(
                        doc.path()
                            .map(|p| p.ends_with("target.txt"))
                            .unwrap_or(false),
                        "current doc should be target.txt"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_split_terminal_leaf() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    // 布局树 API：split 终端叶子 → 关闭
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("sl.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("split.js");
    std::fs::write(
        &plugin_path,
        r#"
        let lid = null;
        helix.register_command("sp-open", () => { lid = helix.split("right", { terminal: { cmd: "cat", size: 20 } }); helix.echo("id:" + lid); });
        helix.register_command("sp-panel", () => { lid = helix.split("right", { panel: { render: () => ["P"], size: 20 } }); });
        helix.register_command("sp-close", () => { helix.pane.close(lid); });
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
                Some(":sp-open<ret>"),
                Some(&|app| {
                    let has_term = app.compositor.has_component(std::any::type_name::<
                        helix_term::ui::plugin_terminal::PluginTerminal,
                    >());
                    assert!(has_term, "split should create a terminal leaf");
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(status.as_ref().starts_with("id:"), "split returns leaf id");
                }),
            ),
            // 终端叶子:Esc 切 normal → C-p 进 Pane 模式 → x 关闭(关闭唯一途径)
            (
                Some("<esc><C-p>x<esc>"),
                Some(&|app| {
                    let has_term = app.compositor.has_component(std::any::type_name::<
                        helix_term::ui::plugin_terminal::PluginTerminal,
                    >());
                    assert!(!has_term, "window 模式 x should close the terminal leaf");
                }),
            ),
            // 面板叶子可穿透按键——用 close_leaf 命令关闭
            (
                Some(":sp-panel<ret>"),
                Some(&|app| {
                    let has = app.compositor.has_component(std::any::type_name::<
                        helix_term::ui::plugin_panel::PluginPanel,
                    >());
                    assert!(has, "panel leaf should exist");
                }),
            ),
            (
                Some(":sp-close<ret>"),
                Some(&|app| {
                    let has = app.compositor.has_component(std::any::type_name::<
                        helix_term::ui::plugin_panel::PluginPanel,
                    >());
                    assert!(!has, "close_leaf should remove the panel leaf");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
