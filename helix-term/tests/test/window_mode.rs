use super::*;
use helix_term::application::Application;
use helix_term::compositor::Component;
use helix_term::job::Jobs;
use helix_view::input::parse_macro;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

async fn pump(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(keys)?.into_iter() {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    Ok(())
}

/// 渲染 compositor 到 Buffer,返回所有行(仿 filetree.rs)
fn render_rows(app: &mut Application, area: helix_view::graphics::Rect) -> Vec<String> {
    let mut buf = tui::buffer::Buffer::empty(area);
    app.compositor.reset_plugin_diffs();
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

#[tokio::test(flavor = "multi_thread")]
async fn window_mode_enter_exit_toggles() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    assert!(!app.compositor.window_mode_active(), "初始非模式");
    pump(&mut app, "<C-w>").await?;
    assert!(app.compositor.window_mode_active(), "C-w 进模式");
    pump(&mut app, "<esc>").await?;
    assert!(!app.compositor.window_mode_active(), "Esc 退模式");
    pump(&mut app, "<C-w> <C-w>").await?;
    assert!(!app.compositor.window_mode_active(), "模式内 C-w 退出");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn window_mode_statusline_indicator() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    pump(&mut app, "<C-w>").await?;
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    let status = &rows[29];
    assert!(status.contains("[WINDOW]"), "状态栏含指示: {status:?}");
    pump(&mut app, "<esc>").await?;
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    assert!(!rows[29].contains("[WINDOW]"), "退出后指示消失: {:?}", &rows[29]);
    Ok(())
}
