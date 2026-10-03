//! 阶段①:zellij 式平级模式(`C-g` Locked / `C-p` Pane / `C-n` Resize / `C-h` Move /
//! `C-y` Scroll)的集成测试。
//!
//! 过渡期说明:旧的全局 `C-w` 单模式仍然并存(`window_*.rs` 覆盖它),本文件只覆盖新模式。

use super::*;
use helix_term::application::Application;
use helix_term::compositor::PaneMode;
use helix_term::job::Jobs;
use helix_term::ui::layout::SplitDir;
use helix_term::ui::plugin_panel::PanelSide;
use helix_term::ui::PluginPanel;
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

/// 渲染 compositor 到 Buffer,返回所有行(用于断言状态栏内容)
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

/// 回归:`C-n`/`C-p` 是 helix 的**补全键**,不能在有输入态层(命令行/选择器)时被模式抢走。
/// 曾经漏过:insert 豁免盖不住命令行——那时编辑器不是 `Insert` 模式。
#[tokio::test(flavor = "multi_thread")]
async fn completion_keys_not_stolen_by_prefixes() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    pump(&mut app, ":theme d<C-n>").await?;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Normal,
        "命令行里的 C-n 是补全下一个,不得切到 Resize 模式"
    );
    Ok(())
}

/// 状态栏模式指示:图标 + 模式名(只有图标的话 Pane/Resize/Move/Scroll/Locked 区分不出来)
#[tokio::test(flavor = "multi_thread")]
async fn pane_mode_statusline_label() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let area = helix_view::graphics::Rect::new(0, 0, 120, 30);
    let status = |app: &mut Application| render_rows(app, area).pop().unwrap();

    assert!(!status(&mut app).contains("PANE"), "不在模式里不显示");

    pump(&mut app, "<C-p>").await?;
    assert!(status(&mut app).contains("PANE"), "Pane 模式标注");

    pump(&mut app, "<C-n>").await?;
    assert!(
        status(&mut app).contains("RESIZE"),
        "切到 Resize 后标注跟着变"
    );
    assert!(!status(&mut app).contains("PANE"), "旧标注不残留");

    pump(&mut app, "<C-g>").await?;
    assert!(status(&mut app).contains("LOCKED"), "Locked 标注");

    pump(&mut app, "<C-g>").await?;
    assert!(!status(&mut app).contains("LOCKED"), "退出后标注消失");
    Ok(())
}

/// 开一个右侧面板叶子(id=1)并把焦点交给它
fn split_right_panel(app: &mut Application) {
    app.compositor.split_leaf_with_ratio(
        SplitDir::H,
        false,
        40,
        Box::new(PluginPanel::new(1, PanelSide::Right, false)),
    );
    assert_eq!(
        app.compositor.layout_tree().active(),
        1,
        "面板打开后活动叶子=1"
    );
}

/// Pane 模式:`hjkl` 聚焦(与旧 C-w 语义一致),且操作后仍在模式内
#[tokio::test(flavor = "multi_thread")]
async fn pane_mode_focus_and_stays_in_mode() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    split_right_panel(&mut app);

    pump(&mut app, "<C-p>h").await?;
    assert_eq!(
        app.compositor.layout_tree().active(),
        0,
        "h → 左邻居(编辑器)"
    );
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Pane,
        "一次操作后仍留在 Pane 模式(可连续操作)"
    );

    pump(&mut app, "l").await?;
    assert_eq!(app.compositor.layout_tree().active(), 1, "l → 右邻居(面板)");
    Ok(())
}

/// 前缀键两义:进入 + 同键退出;`Esc` 也退出
#[tokio::test(flavor = "multi_thread")]
async fn pane_mode_prefix_toggle_and_esc_exit() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;

    pump(&mut app, "<C-p>").await?;
    assert_eq!(app.compositor.pane_mode(), PaneMode::Pane, "C-p 进入 Pane");
    pump(&mut app, "<C-p>").await?;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Normal,
        "同一个前缀键再按回 Normal"
    );

    pump(&mut app, "<C-p><esc>").await?;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Normal,
        "Esc 回 Normal"
    );
    Ok(())
}

/// 平级而非压栈:任一模式内按另一个前缀键直接切过去
#[tokio::test(flavor = "multi_thread")]
async fn pane_modes_switch_directly_not_nested() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;

    pump(&mut app, "<C-p><C-n>").await?;
    assert_eq!(app.compositor.pane_mode(), PaneMode::Resize);
    pump(&mut app, "<C-h>").await?;
    assert_eq!(app.compositor.pane_mode(), PaneMode::Move);
    pump(&mut app, "<C-y>").await?;
    assert_eq!(app.compositor.pane_mode(), PaneMode::Scroll);
    // C-g 从任一模式进 Locked
    pump(&mut app, "<C-g>").await?;
    assert_eq!(app.compositor.pane_mode(), PaneMode::Locked);
    pump(&mut app, "<C-g>").await?;
    assert_eq!(app.compositor.pane_mode(), PaneMode::Normal);
    Ok(())
}

/// Locked:除 `C-g` 外的前缀键**不**切换模式(原样交给叶子)。
/// 这条是设计 §4.2 的核心 —— 不这样做 Locked 就退化成"另一个普通模式"。
#[tokio::test(flavor = "multi_thread")]
async fn locked_only_answers_to_ctrl_g() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;

    pump(&mut app, "<C-g>").await?;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Locked,
        "C-g 进 Locked"
    );

    pump(&mut app, "<C-p>").await?;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Locked,
        "Locked 内 C-p 必须放行给叶子,不得切模式"
    );
    pump(&mut app, "<C-n><C-h>").await?;
    assert_eq!(app.compositor.pane_mode(), PaneMode::Locked, "其余前缀同理");

    pump(&mut app, "<C-g>").await?;
    assert_eq!(app.compositor.pane_mode(), PaneMode::Normal, "C-g 逃出");
    Ok(())
}

/// 拦截规则:insert 里前缀键放行(`C-h` 是删词);但 `C-g` 例外 —— 始终拦截。
/// 不豁免 `C-h` 会毁掉退格;不例外 `C-g` 则永远进不去 Locked。
#[tokio::test(flavor = "multi_thread")]
async fn insert_mode_exemption_with_ctrl_g_exception() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;

    pump(&mut app, "iabc<C-h>").await?;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Normal,
        "insert 里 C-h 不切模式(要被当成删词)"
    );

    pump(&mut app, "<C-g>").await?;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Locked,
        "C-g 在 insert 里也必须拦,否则从终端直通进不了 Locked"
    );
    Ok(())
}
