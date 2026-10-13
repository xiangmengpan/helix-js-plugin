use super::*;

use std::time::Duration;

use helix_term::application::Application;
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

/// 布局树里 split 出的面板：事件循环渲染后再向新 buffer 渲染应完整重画。
/// 回归：插件面板渲染原为跨帧 diff（状态失效导致新 surface 空白）；现为全量渲染，
/// 任何 surface 都完整重画。此测试锁定面板在布局树叶子中也能被渲染出来。
#[tokio::test(flavor = "multi_thread")]
async fn tree_panel_diff_reset() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("mini.js");
    std::fs::write(
        &plugin,
        r#"
        helix.register_command("mini", () => {
            helix.open_panel({ side: "left", size: 32, render: () => ["hello-panel"] });
        });
        "#,
    )?;
    let mut app = AppBuilder::new().build()?;
    let keys = format!(":plugin-load {}<ret>:mini<ret>", plugin.display());
    pump(&mut app, &keys).await?;

    let area = helix_view::graphics::Rect::new(0, 0, 120, 30);
    let mut buf = tui::buffer::Buffer::empty(area);
    let mut jobs = Jobs::new();
    {
        let mut cx = helix_term::compositor::Context {
            editor: &mut app.editor,
            scroll: None,
            jobs: &mut jobs,
        };
        app.compositor.render(area, &mut buf, &mut cx);
    }
    // 重置 diff 后再渲染一次到新 buffer：面板内容必须完整出现（tree 叶子）
    let rows = render_rows(&mut app, area);
    let left: String = rows
        .iter()
        .map(|r| r.chars().take(32).collect::<String>())
        .collect();
    assert!(
        left.contains("hello-panel"),
        "tree 面板 diff 重置后重绘: {left:?}"
    );
    Ok(())
}
