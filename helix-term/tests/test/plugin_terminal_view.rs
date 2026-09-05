use super::*;

use anyhow::bail;
use helix_term::{application::Application, job::Jobs};
use helix_view::input::parse_macro;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

// 临时验证测试：:term-native 开原生终端 → 层存在；__term_native 内 term_feed 模拟输出 →
// 渲染 compositor 到 Buffer → 断言终端文本可见；再加真实回环（按键直通 pty → cat 回显 → 网格）。
// 跑通后移除（mod 声明由控制器合并时统一加）。
#[tokio::test(flavor = "multi_thread")]
async fn term_native_opens_and_renders_feed() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("t.txt");
    std::fs::write(&file, "x\n")?;

    let terminal_type = std::any::type_name::<helix_term::ui::PluginTerminal>();
    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);

    // 打开原生终端（:term-native 命令路径 → OpenTerminal 请求 → 推层）
    for key_event in parse_macro(":term-native<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    assert!(
        app.compositor.has_component(terminal_type),
        "terminal layer open"
    );

    // 渲染 compositor 到 Buffer：__term_native 内的 term_feed 模拟输出应可见
    let area = helix_view::graphics::Rect::new(0, 0, 120, 30);
    let render_all = |app: &mut Application, area| -> String {
        let mut buf = tui::buffer::Buffer::empty(area);
        let mut jobs = Jobs::new();
        let mut cx = helix_term::compositor::Context {
            editor: &mut app.editor,
            scroll: None,
            jobs: &mut jobs,
        };
        app.compositor.render(area, &mut buf, &mut cx);
        buf.content.iter().map(|c| c.symbol.as_str()).collect()
    };
    let all = render_all(&mut app, area);
    assert!(
        all.contains("hello from pty"),
        "terminal text rendered: {all:?}"
    );

    // 真实回环：按键直通 pty → cat 回显 → chunk → bridge 闭包 → term_feed → 网格
    for key_event in parse_macro("xyz")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    let all = render_all(&mut app, area);
    assert!(all.contains("xyz"), "pty echo rendered: {all:?}");

    let errs = app.close().await;
    if !errs.is_empty() {
        for err in &errs {
            log::error!("Errors closing app: {err}");
        }
        bail!("Error closing app");
    }
    Ok(())
}
