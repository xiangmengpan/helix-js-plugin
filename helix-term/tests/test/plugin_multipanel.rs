use super::*;

use helix_term::application::Application;
use helix_term::job::Jobs;
use helix_view::input::parse_macro;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

/// 发送键序列并泵事件直到空闲（等价于 helpers::test_key_sequences 的单步；
/// 手动 pump 才能在两次输入之间渲染 compositor）。
async fn pump(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(keys)?.into_iter() {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    Ok(())
}

/// 渲染 compositor 到 Buffer，返回底部行（y = height-1）字符串。
fn render_bottom(app: &mut Application, area: helix_view::graphics::Rect) -> String {
    let mut buf = tui::buffer::Buffer::empty(area);
    let mut jobs = Jobs::new();
    let mut cx = helix_term::compositor::Context {
        editor: &mut app.editor,
        scroll: None,
        jobs: &mut jobs,
    };
    app.compositor.render(area, &mut buf, &mut cx);
    buf.content
        .iter()
        .skip((area.height - 1) as usize * area.width as usize)
        .take(area.width as usize)
        .map(|c| c.symbol.as_str())
        .collect()
}

/// 多面板并存：right A(20) + right B(10) 同时渲染（A 靠右缘、B 向内），
/// 按 id 关 A 后只剩 B（回靠右缘）。
#[tokio::test(flavor = "multi_thread")]
async fn two_right_panels_coexist_and_close_by_id() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("mp.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("mp.js");
    std::fs::write(
        &plugin_path,
        r#"
        let a, b;
        helix.register_command("open-a", () => {
            a = helix.open_panel({
                side: "right", size: 20,
                render: (ctx) => Array.from({ length: ctx.height }, () => "A".repeat(ctx.width)),
            });
        });
        helix.register_command("open-b", () => {
            b = helix.open_panel({
                side: "right", size: 10,
                render: (ctx) => Array.from({ length: ctx.height }, () => "B".repeat(ctx.width)),
            });
        });
        helix.register_command("close-a", () => { helix.close_panel(a); });
        "#,
    )?;

    let area = helix_view::graphics::Rect::new(0, 0, 120, 30);
    let mut app = AppBuilder::new().with_file(file, None).build()?;

    // 开 A(右20) + B(右10)：两个面板并存
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;
    pump(&mut app, ":open-a<ret>").await?;
    pump(&mut app, ":open-b<ret>").await?;

    // 底部行右侧 30 列被面板占用：B 左 10（90-99）+ A 最右 20（100-119）
    let bottom = render_bottom(&mut app, area);
    assert_eq!(&bottom[90..100], "BBBBBBBBBB", "B inward of A: {bottom:?}");
    assert_eq!(&bottom[100..120], "AAAAAAAAAAAAAAAAAAAA", "A at right edge: {bottom:?}");

    // 按 id 关 A → 只剩 B，回靠右缘（110-119）
    pump(&mut app, ":close-a<ret>").await?;
    let bottom = render_bottom(&mut app, area);
    assert_eq!(&bottom[110..120], "BBBBBBBBBB", "B hugs right edge: {bottom:?}");
    assert!(!bottom[100..110].contains('B'), "cols 100-109 should be editor area: {bottom:?}");

    // 退出并关闭
    pump(&mut app, "<esc>:q!<ret>").await?;
    let errs = app.close().await;
    assert!(errs.is_empty(), "close errors: {errs:?}");

    Ok(())
}
