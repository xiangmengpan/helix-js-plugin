use helix_view::current_ref;
use helix_view::input::parse_macro;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_statusline_renders() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("st.txt");
    std::fs::write(&file, "data\n")?;
    let plugin_path = dir.path().join("statusline.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.set_statusline((ctx) => "PLUGIN|" + ctx.mode + "|" + ctx.cursor.row);
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
                Some("i"), // 进入 insert 模式，触发重渲染
                Some(&|app| {
                    // 构造 RenderContext 渲染状态栏到独立 surface（仿 bufferline 集成测试）
                    let (view, doc) = current_ref!(app.editor);
                    let area = helix_view::graphics::Rect::new(0, 0, 200, 1);
                    let mut buf = tui::buffer::Buffer::empty(area);
                    let spinners = helix_term::ui::ProgressSpinners::default();
                    let mut rc = helix_term::ui::statusline::RenderContext::new(
                        &app.editor,
                        doc,
                        view,
                        true,
                        &spinners,
                        false,
                        None,
                        "editor",
                        None,
                        None,
                    )
                    .with_state(helix_js::host::state_handle());
                    helix_term::ui::statusline::render(&mut rc, area, &mut buf);
                    let rendered: String = buf.content.iter().map(|c| c.symbol.as_str()).collect();
                    assert!(
                        rendered.contains("PLUGIN|insert|0"),
                        "statusline missing plugin text: {rendered:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

/// replace 模式：整个状态栏由 JS 控制（默认组件不渲染），左右分栏（right 段右对齐）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_statusline_replace_mode() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("sl.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("slr.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.set_statusline((ctx) => [
            { text: "LL", style: "ui.statusline.insert" },
            { text: "CC", zone: "center" },
            { text: "RR", zone: "right" },
        ], { replace: true, zones: [1, 1, 1] });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(":plugin-load {}<ret>", plugin_path.display()))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    // 渲染状态栏行（最后一行）
    let area = helix_view::graphics::Rect::new(0, 0, 80, 1);
    let mut buf = tui::buffer::Buffer::empty(area);
    // 渲染状态栏（RenderContext 构造）
    let spinners = helix_term::ui::ProgressSpinners::default();
    {
        let (view, doc) = current_ref!(app.editor);
        let mut rc = helix_term::ui::statusline::RenderContext::new(
            &app.editor,
            doc,
            view,
            true,
            &spinners,
            false,
            None,
            "editor",
            None,
            None,
        )
        .with_state(helix_js::host::state_handle());
        helix_term::ui::statusline::render(&mut rc, area, &mut buf);
    }
    let row0: String = buf
        .content
        .iter()
        .take(80)
        .map(|c| c.symbol.as_str())
        .collect();
    // replace 模式 + zones 1:1:1（宽 80 → 左 26 / 中 26 / 右 26）：
    // 左区 "LL" 在 x0，中区 "CC" 在 x≈27（居中），右区 "RR" 在右侧
    assert!(row0.starts_with("LL"), "left 区从左渲染: {row0:?}");
    // 左区段间 gap：left 区右边界（x=26）应为空格
    assert_eq!(
        row0.chars().nth(2).unwrap(),
        ' ',
        "left 区段尾 gap: {row0:?}"
    );
    // 中区居中：left_w=26, right_w=26, center_w=80-52=28，内容 "CC" 居中 → x=26+(28-2)/2=39
    assert_eq!(
        row0.chars().nth(39).unwrap(),
        'C',
        "center 区居中: {row0:?}"
    );
    assert_eq!(
        row0.chars().nth(40).unwrap(),
        'C',
        "center 区第二字符: {row0:?}"
    );
    // 右区右对齐（末尾 "RR"）
    assert!(row0.trim_end().ends_with("RR"), "right 区右对齐: {row0:?}");
    assert!(
        !row0.contains("sl.txt"),
        "replace 模式不显示默认文件名组件: {row0:?}"
    );

    Ok(())
}

/// 真实内容 + zones [2,3,1]：右区（位置信息）必须贴屏幕最右
#[tokio::test(flavor = "multi_thread")]
async fn plugin_statusline_right_flush_edge() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("rf.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("slf.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.set_statusline((ctx) => [
            { text: " N ", style: "ui.statusline.normal" },
            { text: "main.rs", style: null },
            { text: "1:1", style: null, zone: "right" },
            { text: "1%", style: "ui.virtual", zone: "right" },
            { text: "126", style: "ui.virtual", zone: "right" },
        ], { replace: true, zones: [2, 3, 1] });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(":plugin-load {}<ret>", plugin_path.display()))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let area = helix_view::graphics::Rect::new(0, 0, 126, 1);
    let mut buf = tui::buffer::Buffer::empty(area);
    let spinners = helix_term::ui::ProgressSpinners::default();
    {
        let (view, doc) = current_ref!(app.editor);
        let mut rc = helix_term::ui::statusline::RenderContext::new(
            &app.editor,
            doc,
            view,
            true,
            &spinners,
            false,
            None,
            "editor",
            None,
            None,
        )
        .with_state(helix_js::host::state_handle());
        helix_term::ui::statusline::render(&mut rc, area, &mut buf);
    }
    let row: String = buf.content.iter().map(|c| c.symbol.as_str()).collect();
    // 右区右对齐贴行尾：最后 4 字符应为 " 126"（右区内容 "1:1 1% 126" 右对齐）
    let trimmed_end = row.trim_end();
    assert!(trimmed_end.ends_with("126"), "右区内容应贴行尾: {row:?}");
    // 右区内容后不应有空白（右对齐到 126 列）
    assert!(row.ends_with("126"), "行尾应为 126，无尾随空白: {row:?}");
    Ok(())
}

/// zones 2:3:1：左区 1/3、中区居中、右区右对齐（宽 90 → 左 30/中 30/右 30）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_statusline_zones_ratio() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("zr.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("slz.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.set_statusline((ctx) => [
            { text: "L", style: "ui.statusline.insert" },
            { text: "M", zone: "center" },
            { text: "R", zone: "right" },
        ], { replace: true, zones: [2, 3, 1] });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(":plugin-load {}<ret>", plugin_path.display()))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    let area = helix_view::graphics::Rect::new(0, 0, 90, 1);
    let mut buf = tui::buffer::Buffer::empty(area);
    let spinners = helix_term::ui::ProgressSpinners::default();
    {
        let (view, doc) = current_ref!(app.editor);
        let mut rc = helix_term::ui::statusline::RenderContext::new(
            &app.editor,
            doc,
            view,
            true,
            &spinners,
            false,
            None,
            "editor",
            None,
            None,
        )
        .with_state(helix_js::host::state_handle());
        helix_term::ui::statusline::render(&mut rc, area, &mut buf);
    }
    let row: String = buf.content.iter().map(|c| c.symbol.as_str()).collect();
    // zones 2:3:1，宽 90 → left_w=30, right_w=15, center_w=45（center_x=30, right_x=75）
    assert_eq!(row.chars().next().unwrap(), 'L', "左区 x0: {row:?}");
    // 中区居中：x = 30 + (45-1)/2 = 52
    assert_eq!(row.chars().nth(52).unwrap(), 'M', "中区居中: {row:?}");
    // 右区右对齐：x = 75 + 15 - 1 = 89
    assert_eq!(row.chars().nth(89).unwrap(), 'R', "右区右对齐: {row:?}");
    Ok(())
}

/// window_mode 透传给 JS 状态栏钩子（replace 钩子可据此渲染 [WINDOW]）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_statusline_window_mode_field() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let plugin_path = dir.path().join("sl.js");
    std::fs::write(
        &plugin_path,
        r#"helix.set_statusline((ctx) => ctx.window_mode ? "MODE_ON" : "MODE_OFF");"#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new().build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some("C-p"),
                Some(&|app| {
                    let (view, doc) = current_ref!(app.editor);
                    let area = helix_view::graphics::Rect::new(0, 0, 120, 1);
                    let mut buf = tui::buffer::Buffer::empty(area);
                    let spinners = helix_term::ui::ProgressSpinners::default();
                    let mut rc = helix_term::ui::statusline::RenderContext::new(
                        &app.editor,
                        doc,
                        view,
                        true,
                        &spinners,
                        true,
                        None,
                        "editor",
                        None, // window_mode=true
                        None,
                    )
                    .with_state(helix_js::host::state_handle());
                    helix_term::ui::statusline::render(&mut rc, area, &mut buf);
                    let rendered: String = buf.content.iter().map(|c| c.symbol.as_str()).collect();
                    assert!(
                        rendered.contains("MODE_ON"),
                        "window_mode 透传给 JS: {rendered:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// 窗口图标链路:active_leaf_type/active_leaf_path 透传给 replace 模式 JS 钩子
#[tokio::test(flavor = "multi_thread")]
async fn plugin_statusline_window_leaf_fields() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let plugin_path = dir.path().join("sl.js");
    std::fs::write(
        &plugin_path,
        r#"helix.set_statusline((ctx) => ctx.active_leaf_type + "|" + (ctx.active_leaf_path || "null"));"#,
    )?;
    let mut app = AppBuilder::new().build()?;
    // 泵入插件加载(仿 filetree 测试的 pump;plugin_statusline 模块无 pump helper,内联)
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(":plugin-load {}<ret>", plugin_path.display()))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    // 手动渲染:window_mode + active_leaf=editor(/tmp/a.rs)
    let (view, doc) = current_ref!(app.editor);
    let area = helix_view::graphics::Rect::new(0, 0, 120, 1);
    let mut buf = tui::buffer::Buffer::empty(area);
    let spinners = helix_term::ui::ProgressSpinners::default();
    let mut rc = helix_term::ui::statusline::RenderContext::new(
        &app.editor,
        doc,
        view,
        true,
        &spinners,
        true, // window_mode
        None, // pane_mode
        "editor",
        Some("/tmp/a.rs".to_string()),
        None,
    )
    .with_state(helix_js::host::state_handle());
    helix_term::ui::statusline::render(&mut rc, area, &mut buf);
    let rendered: String = buf.content.iter().map(|c| c.symbol.as_str()).collect();
    assert!(
        rendered.contains("editor|/tmp/a.rs"),
        "active_leaf 透传: {rendered:?}"
    );
    Ok(())
}

/// replace 模式下每段可带自己的主题色：两段不同 scope ⇒ 单元格 fg 各自等于 theme.get(scope)。
///
/// 回归锁（计划任务 5 / 审查重点）：运行时早已支持 `{text, style, zone}`，但
/// `plugins/helix.d.ts` 一度只声明 `string`。这条测试钉住"每段样式真的生效"，
/// 防止文档升级后被当成装饰性字段。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_statusline_parts_have_per_part_style() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("slp.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("slp.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.set_statusline((ctx) => [
            { text: "L", style: "ui.statusline" },
            { text: "R", style: "error", zone: "right" },
        ], { replace: true, zones: [1, 0, 1] });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(":plugin-load {}<ret>", plugin_path.display()))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    // zones 1:0:1，宽 90 ⇒ L 在 x=0，R 右对齐在 x=89
    let area = helix_view::graphics::Rect::new(0, 0, 90, 1);
    let mut buf = tui::buffer::Buffer::empty(area);
    let spinners = helix_term::ui::ProgressSpinners::default();
    {
        let (view, doc) = current_ref!(app.editor);
        let mut rc = helix_term::ui::statusline::RenderContext::new(
            &app.editor,
            doc,
            view,
            true,
            &spinners,
            false,
            None,
            "editor",
            None,
            None,
        )
        .with_state(helix_js::host::state_handle());
        helix_term::ui::statusline::render(&mut rc, area, &mut buf);
    }

    let expect_l = app.editor.theme.get("ui.statusline").fg;
    let expect_r = app.editor.theme.get("error").fg;
    assert_ne!(
        expect_l, expect_r,
        "测试主题里两个 scope 的 fg 相同，无法区分（换 scope 而不是删断言）"
    );
    // 单元格是 helix_tui::Color，主题是 Option<Color>：用 Some(..) 对齐
    assert_eq!(Some(buf.content[0].fg), expect_l, "左段样式未生效");
    assert_eq!(Some(buf.content[89].fg), expect_r, "右段样式未生效");
    Ok(())
}

/// 真插件 + 真 `set_theme`：**多模式 powerline 端帽共存**。
///
/// 回归目标：`set_theme` 曾经是"整体替换"，于是 normal 注册完再切 insert，后一次调用会把
/// `statusline.cap.normal` 抹掉 ⇒ 切回 normal 时端帽掉色（默认样式）。改成合并语义后应共存。
///
/// 断言看的是**渲染出来的格子颜色**（不是内部表）：端帽字符 `` 的 fg 必须等于该模式的底色。
#[tokio::test(flavor = "multi_thread")]
async fn powerline_caps_coexist_across_modes() -> anyhow::Result<()> {
    use helix_term::ui::statusline::RenderContext;
    use tui::style::Color;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("st.txt");
    std::fs::write(&file, "data\n")?;

    // ① 默认主题没有 ui.statusline.normal/insert ⇒ 先造两个模式底色
    let setup = dir.path().join("setup.js");
    std::fs::write(
        &setup,
        r##"
        helix.set_theme({
            "ui.statusline.normal": { fg: "#ffffff", bg: "#0000ff" },
            "ui.statusline.insert": { fg: "#000000", bg: "#00ff00" },
        });
        "##,
    )?;

    // ② `corners = "powerline"` 走 config.toml 注入（插件启动时读）
    let mut plugins = toml::Table::new();
    let mut sl = toml::Table::new();
    sl.insert("corners".into(), toml::Value::String("powerline".into()));
    plugins.insert("statusline".into(), toml::Value::Table(sl));
    let config = helix_term::config::Config {
        plugins: Some(plugins),
        ..Default::default()
    };

    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(config)
        .build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", setup.display())).await?;
    let plugin = format!(
        "{}/../plugins/statusline/plugin.js",
        env!("CARGO_MANIFEST_DIR")
    );
    pump(&mut app, &format!(":plugin-load {plugin}<ret>")).await?;

    // 把状态栏渲染成一行格子，返回左端帽的 fg 颜色
    let cap_fg = |app: &mut helix_term::application::Application| -> Option<Color> {
        let (view, doc) = current_ref!(app.editor);
        let area = helix_view::graphics::Rect::new(0, 0, 200, 1);
        let mut buf = tui::buffer::Buffer::empty(area);
        let spinners = helix_term::ui::ProgressSpinners::default();
        let mut rc = RenderContext::new(
            &app.editor,
            doc,
            view,
            true,
            &spinners,
            false,
            None,
            "editor",
            None,
            None,
        )
        .with_state(helix_js::host::state_handle());
        helix_term::ui::statusline::render(&mut rc, area, &mut buf);
        buf.content
            .iter()
            .find(|c| c.symbol == "\u{e0b6}")
            .map(|c| c.style.fg)
    };

    let normal_fg = cap_fg(&mut app);
    assert_eq!(
        normal_fg,
        Some(Color::Rgb(0, 0, 255)),
        "normal 模式端帽 fg 应等于 mode 底色"
    );

    // 切 insert：注册 cap.insert
    pump(&mut app, "i").await?;
    let insert_fg = cap_fg(&mut app);
    assert_eq!(
        insert_fg,
        Some(Color::Rgb(0, 255, 0)),
        "insert 模式端帽用自己的底色"
    );

    // 切回 normal：**这条是关键** —— 如果 cap.normal 被抹掉了，这里就拿不到蓝色
    pump(&mut app, "<esc>").await?;
    assert_eq!(
        cap_fg(&mut app),
        Some(Color::Rgb(0, 0, 255)),
        "切回 normal 端帽仍应着色（多模式 cap scope 必须共存）"
    );
    Ok(())
}
