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
            js: None,
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
    // **有界等待**:pty 回显经真实进程回环,"泵一轮就断言"隐含"机器安静"这个前提 ——
    // 并发满载时回显可能只到一半(实测见过渲染里仅 `hello from ptyx`,期望 `xyz`)。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut all;
    loop {
        app.event_loop_until_idle(&mut rx_stream).await;
        all = render_all(&mut app, area);
        if all.contains("xyz") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pty echo rendered(30s 内有界等待): {all:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let errs = app.close().await;
    if !errs.is_empty() {
        for err in &errs {
            log::error!("Errors closing app: {err}");
        }
        bail!("Error closing app");
    }
    Ok(())
}

/// 注入一次鼠标事件并泵一轮事件循环（选区状态机跨事件 ⇒ 每段都要真跑）
#[cfg(not(windows))]
async fn pump_mouse_at(
    app: &mut Application,
    kind: termina::event::MouseEventKind,
    column: u16,
    row: u16,
) -> anyhow::Result<()> {
    let mouse = termina::event::MouseEvent {
        kind,
        column,
        row,
        modifiers: termina::event::Modifiers::NONE,
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tx.send(Ok(Event::Mouse(mouse)))?;
    app.event_loop_until_idle(&mut UnboundedReceiverStream::new(rx))
        .await;
    Ok(())
}

/// 终端鼠标拖选 → 松手即复制（本地选区，不经 pty 应用）。
///
/// 以前终端里鼠标**只有**两种归宿：应用开了鼠标上报就编码给 pty，否则只有滚轮；
/// 本地一个选区状态都没有 ⇒ 任何一个开 `?1000h` 的 TUI（vim/htop）都会把鼠标吃干净。
#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread")]
async fn mouse_drag_select_copies_terminal_text() -> anyhow::Result<()> {
    use termina::event::{MouseButton, MouseEventKind};

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("t.txt");
    std::fs::write(&file, "x\n")?;
    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);

    for key_event in parse_macro(":term-native<ret>")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;

    // 用**应用自己的视口**画一帧：leaf_rect / last_grid_area 必须与后续事件泵同一坐标系
    let area = app.compositor.area();
    let render_all = |app: &mut Application| {
        let mut buf = tui::buffer::Buffer::empty(area);
        let mut jobs = Jobs::new();
        let mut cx = helix_term::compositor::Context {
            editor: &mut app.editor,
            scroll: None,
            jobs: &mut jobs,
            js: None,
        };
        app.compositor.reset_plugin_diffs();
        app.compositor.render(area, &mut buf, &mut cx);
        buf
    };
    let buf = render_all(&mut app);
    let all: String = buf.content.iter().map(|c| c.symbol.as_str()).collect();
    assert!(all.contains("hello from pty"), "终端内容已就绪: {all:?}");

    // 点击坐标从**渲染结果**里反查，而不是从 leaf_rect 推算：
    // 活动叶子会画聚焦边框、组件内容整体 inset 1 格，算错了就会复制出 "hell"。
    let cells: Vec<&str> = buf.content.iter().map(|c| c.symbol.as_str()).collect();
    let idx = cells
        .windows(5)
        .position(|w| w == ["h", "e", "l", "l", "o"])
        .expect("渲染里找不到 hello");
    let width = area.width as usize;
    let (x0, y0) = (area.x + (idx % width) as u16, area.y + (idx / width) as u16);

    // Down → Drag → Up：三段都要走完整事件泵（选区状态机跨事件）
    pump_mouse_at(&mut app, MouseEventKind::Down(MouseButton::Left), x0, y0).await?;
    pump_mouse_at(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        x0 + 4,
        y0,
    )
    .await?;
    pump_mouse_at(&mut app, MouseEventKind::Up(MouseButton::Left), x0 + 4, y0).await?;

    let copied: Vec<String> = {
        let reg = app
            .editor
            .registers
            .read('"', &app.editor)
            .expect("选区已写入无名寄存器");
        reg.map(|r| r.to_string()).collect()
    };
    assert_eq!(
        copied,
        vec!["hello".to_string()],
        "拖选 5 格应复制出 hello（实得 {copied:?}，click=({x0},{y0})）"
    );

    let errs = app.close().await;
    if !errs.is_empty() {
        bail!("Error closing app: {errs:?}");
    }
    Ok(())
}
