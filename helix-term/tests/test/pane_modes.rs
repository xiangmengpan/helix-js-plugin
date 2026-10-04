//! 阶段①:zellij 式平级模式(`C-g` Locked / `C-p` Pane / `C-n` Resize / `C-h` Move /
//! `C-y` Scroll)的集成测试。旧的全局 `C-w` 单模式已在 ①-3 删除,
//! `C-w` 现仅作 insert 模式的删词命令存在。

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

/// Pane 模式:`w`/`e` 切换浮动态(阶段② 接通),`i` 对浮窗 pin
#[tokio::test(flavor = "multi_thread")]
async fn pane_mode_float_toggle_and_pin() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    split_right_panel(&mut app); // active = 1
    assert_eq!(app.compositor.layout_tree().floating(), None, "初始不浮动");

    pump(&mut app, "<C-p>w").await?;
    assert_eq!(
        app.compositor.layout_tree().floating(),
        Some(1),
        "w 把活动叶浮动起来"
    );
    pump(&mut app, "i").await?;
    assert!(app.compositor.layout_tree().float_is_pinned(1), "i 置 pin");
    pump(&mut app, "i").await?;
    assert!(
        !app.compositor.layout_tree().float_is_pinned(1),
        "再 i 取消 pin"
    );

    pump(&mut app, "e").await?;
    assert_eq!(app.compositor.layout_tree().floating(), None, "e 收回平铺");
    // 非浮动时 i 只给提示,不改状态
    pump(&mut app, "i").await?;
    assert!(!app.compositor.layout_tree().float_is_pinned(1));
    Ok(())
}

/// 浮动的活动叶:Resize 调浮窗尺寸、Move 搬位置(而不是调分界/交换邻居)
#[tokio::test(flavor = "multi_thread")]
async fn float_keys_resize_and_move_the_float() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    split_right_panel(&mut app);
    pump(&mut app, "<C-p>w").await?;

    let before = app.compositor.layout_tree().floats();
    let (w0, h0, x0, y0) = (before[0].w, before[0].h, before[0].x, before[0].y);

    pump(&mut app, "<C-n>l").await?; // Resize:宽 +
    let resized = app.compositor.layout_tree().floats();
    assert!(
        resized[0].w > w0,
        "Resize 的 l 加宽浮窗({w0} -> {})",
        resized[0].w
    );
    assert_eq!(resized[0].h, h0, "高度不受 l 影响");

    pump(&mut app, "<C-h>l").await?; // Move:右移
    let moved = app.compositor.layout_tree().floats();
    assert!(
        moved[0].x > x0,
        "Move 的 l 右移浮窗({x0} -> {})",
        moved[0].x
    );
    assert_eq!(moved[0].y, y0, "纵向不受 l 影响");
    Ok(())
}

/// Pane 模式 `s`:与兄弟窗建堆叠;`p`/`P`/`Tab` 在组内轮转(而不是切下一个窗口)
#[tokio::test(flavor = "multi_thread")]
async fn pane_mode_stack_and_rotate() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    split_right_panel(&mut app); // 0 与 1 是兄弟叶子;活动叶 = 1(面板)
    assert_eq!(app.compositor.layout_tree().active(), 1);

    // 锚 = 活动叶(1),所以组是 [1, 0]
    pump(&mut app, "<C-p>s").await?;
    assert!(
        app.compositor.layout_tree().is_stacked(1),
        "s 建组,锚 = 活动叶"
    );
    assert_eq!(app.compositor.layout_tree().stack_members(1), vec![1, 0]);

    // 轮转:锚换成兄弟,活动叶跟着走
    pump(&mut app, "p").await?;
    assert_eq!(
        app.compositor.layout_tree().active(),
        0,
        "轮转后活动叶 = 新锚"
    );
    assert_eq!(app.compositor.layout_tree().stack_members(0), vec![0, 1]);

    pump(&mut app, "P").await?;
    assert_eq!(app.compositor.layout_tree().active(), 1, "反向轮转回 1");
    assert_eq!(app.compositor.layout_tree().stack_members(1), vec![1, 0]);

    // 已在组里 → 幂等
    pump(&mut app, "s").await?;
    assert_eq!(
        app.compositor.layout_tree().stack_members(1),
        vec![1, 0],
        "不重复入组"
    );

    // 关掉锚 → 组解散,剩余成员恢复为普通 pane
    pump(&mut app, "<C-p>x").await?;
    assert!(
        !app.compositor.layout_tree().is_stacked(0),
        "摘掉成员 → 组解散"
    );
    assert_eq!(
        app.compositor.layout_tree().leaf_ids().len(),
        1,
        "只剩一个叶子"
    );
    Ok(())
}

/// Pane 模式:`p` / `P` / `Tab` 按树中序环绕切换焦点(zellij 的切下一个窗口)
#[tokio::test(flavor = "multi_thread")]
async fn pane_mode_cycle_focus() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    split_right_panel(&mut app); // active = 1(右面板)

    pump(&mut app, "<C-p>p").await?;
    assert_eq!(app.compositor.layout_tree().active(), 0, "p 绕回左叶");
    pump(&mut app, "p").await?;
    assert_eq!(app.compositor.layout_tree().active(), 1, "再 p 到右叶");
    pump(&mut app, "P").await?;
    assert_eq!(app.compositor.layout_tree().active(), 0, "P 反向");
    pump(&mut app, "<tab>").await?;
    assert_eq!(app.compositor.layout_tree().active(), 1, "Tab 同 p");
    Ok(())
}

/// Resize 模式:`=`/`-` 也走宽度 ±5%(zellij 的「整体增减」落到宽度)。
/// 布局树没有对外 ratio 读取口,这里只钉「不崩、留在模式内」二事。
#[tokio::test(flavor = "multi_thread")]
async fn resize_mode_equals_and_minus_are_safe() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    split_right_panel(&mut app);

    pump(&mut app, "<C-n>=").await?;
    pump(&mut app, "+").await?;
    pump(&mut app, "-").await?;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Resize,
        "操作后仍留在 Resize 模式"
    );
    assert_eq!(
        app.compositor.layout_tree().leaf_types().len(),
        2,
        "叶子数不变"
    );
    Ok(())
}

/// 新模式必须自带键位提示 —— 这是删掉旧 `C-w` 的前置(否则用户失去指引)。
#[tokio::test(flavor = "multi_thread")]
async fn each_pane_mode_shows_its_keymap_hint() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    assert!(app.editor.autoinfo.is_none(), "初始无提示");

    // 前缀键在模式内也能直接切,所以循环即可逐个到达
    for (keys, title) in [
        ("<C-p>", "C-p"),
        ("<C-n>", "C-n"),
        ("<C-h>", "C-h"),
        ("<C-y>", "C-y"),
        ("<C-g>", "C-g"),
    ] {
        pump(&mut app, keys).await?;
        let info = app
            .editor
            .autoinfo
            .as_ref()
            .unwrap_or_else(|| panic!("{keys} 进入模式后必须有键位提示"));
        assert_eq!(info.title.as_ref(), title, "{keys} 的提示标题");
    }
    pump(&mut app, "<C-g>").await?; // Locked → Normal
    assert!(app.editor.autoinfo.is_none(), "退出模式后提示消失");
    Ok(())
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
