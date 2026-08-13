use std::time::Duration;

use helix_term::application::Application;
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
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
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
                    );
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
    let mut jobs = Jobs::new();
    let mut cx = helix_term::compositor::Context {
        editor: &mut app.editor,
        scroll: None,
        jobs: &mut jobs,
    };
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
        );
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
    assert_eq!(row0.chars().nth(2).unwrap(), ' ', "left 区段尾 gap: {row0:?}");
    // 中区居中：left_w=26, right_w=26, center_w=80-52=28，内容 "CC" 居中 → x=26+(28-2)/2=39
    assert_eq!(row0.chars().nth(39).unwrap(), 'C', "center 区居中: {row0:?}");
    assert_eq!(row0.chars().nth(40).unwrap(), 'C', "center 区第二字符: {row0:?}");
    // 右区右对齐（末尾 "RR"）
    assert!(row0.trim_end().ends_with("RR"), "right 区右对齐: {row0:?}");
    assert!(!row0.contains("sl.txt"), "replace 模式不显示默认文件名组件: {row0:?}");

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
