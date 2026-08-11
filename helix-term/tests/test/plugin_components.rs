use super::*;

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
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;
    pump(&mut app, ":tree-popup<ret>").await?;

    let rows = render_rows(&mut app, area);
    let joined = rows.join("\n");
    assert!(rows.iter().any(|r| r.contains("title")), "title row missing: {joined:?}");
    assert!(
        rows.iter().any(|r| r.contains("left right")),
        "row columns side by side missing: {joined:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains('s') && r.contains("s2")),
        "scroll keeps last line s2: {joined:?}"
    );
    assert!(
        !joined.contains("s1"),
        "scroll must drop s1: {joined:?}"
    );

    // 退出并关闭
    pump(&mut app, "<esc>:q!<ret>").await?;
    let errs = app.close().await;
    assert!(errs.is_empty(), "close errors: {errs:?}");

    Ok(())
}
