use super::*;

use std::time::Duration;

use helix_term::application::Application;
use helix_term::compositor::Component;
use helix_term::job::Jobs;
use helix_view::current_ref;
use helix_view::input::parse_macro;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

/// 发送键序列并泵事件直到空闲
async fn pump(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(keys)?.into_iter() {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    Ok(())
}

/// 渲染 compositor 到 Buffer，返回所有行
fn render_rows(app: &mut Application, area: helix_view::graphics::Rect) -> Vec<String> {
    let mut buf = tui::buffer::Buffer::empty(area);
    // DiffRenderer 持久状态：reset 让本 buffer 完整重绘
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

/// 左 32 列（面板区）内容拼接
fn panel_text(app: &mut Application) -> String {
    render_rows(app, helix_view::graphics::Rect::new(0, 0, 120, 30))
        .iter()
        .map(|r| r.chars().take(32).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 建临时目录结构 + 加载真实插件（读取 ~/.config/helix/plugins/features/filetree/index.js）
async fn setup() -> anyhow::Result<(tempfile::TempDir, Application)> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("src"))?;
    std::fs::write(dir.path().join("src/main.js"), "console.log(1)\n")?;
    std::fs::write(dir.path().join("readme.md"), "hi\n")?;
    std::fs::write(dir.path().join(".secret.txt"), "s\n")?;
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME unset"))?;
    let src = std::fs::read_to_string(format!(
        "{home}/.config/helix/plugins/features/filetree/index.js"
    ))?;
    let plugin = dir.path().join("filetree.js");
    std::fs::write(&plugin, src)?;
    let mut app = AppBuilder::new().build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    Ok((dir, app))
}

/// 核心路径：reveal 定位 → 展开目录 → 打开文件（buffer 切换）
#[tokio::test(flavor = "multi_thread")]
async fn filetree_reveal_expand_open() -> anyhow::Result<()> {
    let _pl = PLUGIN_TEST_LOCK.lock().await;
    let (dir, mut app) = setup().await?;
    let main_js = dir.path().join("src/main.js");
    let readme = dir.path().join("readme.md");

    // 当前文件在树 root（cwd=helix-term）外 → reveal 应重设 root 到 readme 所在目录
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    let text = panel_text(&mut app);
    assert!(
        text.contains("readme.md"),
        "reveal 后面板渲染含 readme.md: {text:?}"
    );
    assert!(text.contains("src"), "root 重设后显示 src 目录: {text:?}");

    // reveal 定位 readme.md（索引 2，排序 [src, filetree.js, readme.md]）→
    // Up×2 到 src（目录优先）→ Enter 展开 → Down 到 main.js → Enter 打开
    pump(&mut app, "<up><up>").await?;
    pump(&mut app, "<ret>").await?;
    pump(&mut app, "<down>").await?;
    pump(&mut app, "<ret>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.path()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
        main_js.to_str(),
        "Enter 应打开 src/main.js"
    );
    Ok(())
}

/// 隐藏文件切换：默认隐藏 . 文件，H 后显示
#[tokio::test(flavor = "multi_thread")]
async fn filetree_hidden_toggle() -> anyhow::Result<()> {
    let _pl = PLUGIN_TEST_LOCK.lock().await;
    let (dir, mut app) = setup().await?;
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    assert!(
        !panel_text(&mut app).contains(".secret.txt"),
        "默认隐藏 dotfile"
    );
    pump(&mut app, "H").await?;
    assert!(
        panel_text(&mut app).contains(".secret.txt"),
        "H 切换后显示 dotfile"
    );
    Ok(())
}

/// 新建文件：a → 输入弹窗 → Tab 聚焦 → 输入名字 → Enter → 异步 touch 生效
#[tokio::test(flavor = "multi_thread")]
async fn filetree_new_file_prompt() -> anyhow::Result<()> {
    let _pl = PLUGIN_TEST_LOCK.lock().await;
    let (dir, mut app) = setup().await?;
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;

    let popup_type = std::any::type_name::<helix_term::ui::Popup<helix_term::ui::PluginPopup>>();
    pump(&mut app, "a").await?;
    assert!(app.compositor.has_component(popup_type), "a → 输入弹窗打开");

    pump(&mut app, "note.txt<ret>").await?;
    assert!(
        !app.compositor.has_component(popup_type),
        "Enter 提交后弹窗关闭"
    );
    for _ in 0..50 {
        if dir.path().join("note.txt").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dir.path().join("note.txt").exists(), "note.txt 已创建");
    Ok(())
}

/// 关闭面板：q 键
#[tokio::test(flavor = "multi_thread")]
async fn filetree_close_panel() -> anyhow::Result<()> {
    let _pl = PLUGIN_TEST_LOCK.lock().await;
    let (_, mut app) = setup().await?;
    pump(&mut app, ":filetree<ret>").await?;
    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    assert!(app.compositor.has_component(panel_type), "面板打开");
    pump(&mut app, "q").await?;
    assert!(!app.compositor.has_component(panel_type), "q 关闭面板");
    Ok(())
}
