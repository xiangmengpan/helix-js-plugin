//! 阶段①:zellij 式平级模式(`C-g` Locked / `C-p` Pane / `C-n` Resize / `C-h` Move /
//! `C-y` Scroll)的集成测试。旧的全局 `C-w` 单模式已在 ①-3 删除,
//! `C-w` 现仅作 insert 模式的删词命令存在。

use super::*;
use helix_term::application::Application;
use helix_term::compositor::PaneMode;
use helix_term::ui::layout::SplitDir;
use helix_term::ui::plugin_panel::PanelSide;
use helix_term::ui::PluginPanel;

// Windows 上这些用例的 `Event`/`KeyEvent` 用法已随 pty/流式路径 gate 掉 ⇒
// 导入留着但允许未使用（否则整仓 Windows gate 会因 warning 变红）。
#[cfg(windows)]
#[allow(unused_imports)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::Event;

use tokio_stream::wrappers::UnboundedReceiverStream;

/// 发一次鼠标左键按下（点击）并跑一轮事件循环
async fn pump_mouse_down(app: &mut Application, col: u16, row: u16) -> anyhow::Result<()> {
    #[cfg(not(windows))]
    let mouse = termina::event::MouseEvent {
        kind: termina::event::MouseEventKind::Down(termina::event::MouseButton::Left),
        column: col,
        row,
        modifiers: termina::event::Modifiers::NONE,
    };
    #[cfg(windows)]
    let mouse = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: col,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tx.send(Ok(Event::Mouse(mouse)))?;
    app.event_loop_until_idle(&mut UnboundedReceiverStream::new(rx))
        .await;
    Ok(())
}

/// 鼠标点击换布局焦点（规格 §4.1：换焦点 + 保持当前模式）。
/// 回归：此前 Down 事件只派发 JS 组件事件，布局焦点从不跟随 ⇒ 点击落在非活动叶子时
/// 事件被编辑器叶子兜底消费：**光标跑到点击处、聚焦边框留在原叶子**。
#[tokio::test(flavor = "multi_thread")]
async fn mouse_click_moves_layout_focus() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    split_right_panel(&mut app); // active = 1（右侧面板）
    let area = app.compositor.area();
    render_rows(&mut app, area); // leaf_rects 由渲染填：没画过就没几何
    let left = app
        .compositor
        .layout_tree()
        .leaf_rect(0)
        .expect("左叶有几何");
    let right = app
        .compositor
        .layout_tree()
        .leaf_rect(1)
        .expect("右叶有几何");
    let (lx, ly) = (left.x + left.width / 2, left.y + left.height / 2);
    let (rx, ry) = (right.x + right.width / 2, right.y + right.height / 2);

    // 点编辑器区域 → 焦点回 0（此前会停在 1）
    pump_mouse_down(&mut app, lx, ly).await?;
    assert_eq!(
        app.compositor.layout_tree().active(),
        0,
        "点编辑器区域，焦点必须跟过来"
    );

    // 点面板区域 → 焦点到 1
    pump_mouse_down(&mut app, rx, ry).await?;
    assert_eq!(
        app.compositor.layout_tree().active(),
        1,
        "点面板区域，焦点必须跟过来"
    );

    // 模式保持不变（zellij 语义：换焦点不等于换模式）
    pump(&mut app, "<C-p>").await?;
    assert_eq!(app.compositor.pane_mode(), PaneMode::Pane, "已进 Pane 模式");
    pump_mouse_down(&mut app, lx, ly).await?;
    assert_eq!(app.compositor.layout_tree().active(), 0, "点击换焦点");
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Pane,
        "鼠标点击不退出当前模式"
    );
    Ok(())
}

/// Scroll 模式（`C-y`）：**编辑器叶子**也要真滚——只滚视口、不动光标。
/// 回归：`pane_scroll` 曾经只认终端叶子 ⇒ 编辑器里按 `C-y` 后 `j/k/d/u/f/b`
/// 全是**静默空操作**，看上去就像“模式没生效”。
#[tokio::test(flavor = "multi_thread")]
async fn scroll_mode_scrolls_editor_viewport() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    let body: String = (0..300).map(|i| format!("line {i}\n")).collect();
    std::fs::write(file.path(), body)?;
    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;

    let focused_doc = |app: &mut Application| {
        let vid = app.editor.tree.focus;
        let doc_id = app.editor.tree.get(vid).doc;
        (vid, doc_id)
    };
    // 视口顶端在第几行（view_offset 的 anchor 就是“视口第一行”那个字符：
    // 无 soft-wrap 时 `vertical_offset` 始终为 0，滚动体现在 anchor 上）
    let top_line = |app: &mut Application| {
        let (vid, doc_id) = focused_doc(app);
        let doc = &app.editor.documents[&doc_id];
        let len = doc.text().len_chars();
        doc.text()
            .char_to_line(doc.view_offset(vid).anchor.min(len))
    };
    let cursor = |app: &mut Application| {
        let (vid, doc_id) = focused_doc(app);
        let doc = &app.editor.documents[&doc_id];
        doc.selection(vid).primary().cursor(doc.text().slice(..))
    };

    assert_eq!(top_line(&mut app), 0, "初始视口在顶部");
    pump(&mut app, "<C-y>jjj").await?;
    assert_eq!(top_line(&mut app), 9, "j 每次 3 行（3×3）");
    assert_eq!(cursor(&mut app), 0, "滚视口不能动光标");
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Scroll,
        "连续滚动仍在 Scroll 模式"
    );

    pump(&mut app, "k").await?;
    assert_eq!(top_line(&mut app), 6, "k 反向滚回 3 行");
    pump(&mut app, "d").await?;
    assert_eq!(top_line(&mut app), 18, "d = 半页 12 行");
    pump(&mut app, "b").await?;
    assert_eq!(top_line(&mut app), 0, "b = 整页 24 行，碰顶夹在 0");
    assert_eq!(cursor(&mut app), 0, "全程光标未动");
    Ok(())
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
        None,
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

/// `Enter` = 确认并**进入**当前窗格(手感):退出平级模式,焦点留在刚选中的窗格,
/// 按键立刻回到编辑器/面板。
///
/// 回归:此前 `Enter` 落到 `pane_mode_key` 的 `_ => {}` 被静默吞掉 —— 用户 `C-p h`
/// 选好窗格后按 `Enter` 什么也没发生,只能猜着按 `Esc`/`C-p`。
#[tokio::test(flavor = "multi_thread")]
async fn enter_confirms_and_enters_the_focused_pane() -> anyhow::Result<()> {
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
        "选窗格后仍在模式内"
    );

    pump(&mut app, "<ret>").await?;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Normal,
        "Enter 应退出模式(= 进入选中的窗格)"
    );
    assert_eq!(
        app.compositor.layout_tree().active(),
        0,
        "Enter 不得改动焦点"
    );

    // 真的“进去了”:之后的普通键回到编辑器,不再被平级模式吞掉
    pump(&mut app, "i").await?;
    assert_eq!(
        app.editor.mode(),
        helix_view::document::Mode::Insert,
        "Enter 之后 `i` 必须落到编辑器"
    );
    Ok(())
}

/// `Enter` 在其余平级模式里同样是“完成”→ 退出(resize/move/scroll 里 Enter 本来也无绑定位)
#[tokio::test(flavor = "multi_thread")]
async fn enter_exits_every_pane_mode() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    for prefix in ["<C-n>", "<C-h>", "<C-y>"] {
        pump(&mut app, prefix).await?;
        assert_ne!(
            app.compositor.pane_mode(),
            PaneMode::Normal,
            "{prefix} 进模式"
        );
        pump(&mut app, "<ret>").await?;
        assert_eq!(
            app.compositor.pane_mode(),
            PaneMode::Normal,
            "{prefix} 之后 Enter 应退出"
        );
    }
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

/// Pane 模式的 `D`/`R`：**在整个区域上**切一刀（不是切当前窗口）⇒
/// `ed1|ed2` 之上再开一个窗口时，它横跨整宽（zellij/tmux 里要 `split -f` 才有的形状）。
#[tokio::test(flavor = "multi_thread")]
async fn pane_mode_cap_d_splits_full_width() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let mut app = AppBuilder::new().with_file(&file, None).build()?;

    // ed1 | ed2（两次叶子切分；`C-p` 是开关键，进模式后直接按后续键）
    pump(&mut app, "<C-p>r").await?;
    pump(&mut app, "h").await?; // 焦点回左叶
    pump(&mut app, "r").await?; // 左叶再切一次 → 三个窗口
    assert_eq!(app.compositor.layout_tree().leaf_ids().len(), 3);

    // 现在在**根**上切一刀：新窗口贴在底边、横跨整宽
    pump(&mut app, "D").await?;
    let new_leaf = app.compositor.layout_tree().active();
    pump(&mut app, "<esc>").await?; // 退出 Pane 模式（不影响布局）
                                    // 渲染一帧才知道几何
    let area = app.compositor.area();
    render_rows(&mut app, area);

    let rect = app
        .compositor
        .layout_tree()
        .leaf_rect(new_leaf)
        .expect("新窗口有几何");
    let mut union = helix_view::graphics::Rect::new(u16::MAX, u16::MAX, 0, 0);
    for id in app.compositor.layout_tree().leaf_ids() {
        let r = app.compositor.layout_tree().leaf_rect(id).unwrap();
        union.x = union.x.min(r.x);
        union.y = union.y.min(r.y);
        union.width = union.width.max(r.x + r.width);
        union.height = union.height.max(r.y + r.height);
    }
    assert_eq!(
        rect.width, union.width,
        "D 切出来的窗口必须横跨整个区域（宽 {} vs 总宽 {}）",
        rect.width, union.width
    );
    assert_eq!(rect.y + rect.height, union.y + union.height, "它在底边");
    // 上面那排三个窗口都还在
    assert_eq!(app.compositor.layout_tree().leaf_ids().len(), 4);
    Ok(())
}

/// 组级操作：`pane.list()` 里有组条目；整组最小化后兄弟占满整个区域。
#[tokio::test(flavor = "multi_thread")]
async fn groups_are_listed_and_can_be_minimized() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let mut app = AppBuilder::new().with_file(&file, None).build()?;

    pump(&mut app, "<C-p>r").await?; // ed1 | ed2
    pump(&mut app, "D").await?; // 下面整宽的新窗口
    let bottom = app.compositor.layout_tree().active();
    pump(&mut app, "<esc>").await?;
    let area = app.compositor.area();
    render_rows(&mut app, area);

    // 上面那排是一个组
    let groups = app.compositor.layout_tree().group_ids();
    assert!(!groups.is_empty(), "切分产生了组");
    let top = groups
        .into_iter()
        .find(|g| {
            let leaves = app.compositor.layout_tree().group_leaves(*g);
            leaves.len() == 2 && !leaves.contains(&bottom)
        })
        .expect("上面那排是一个组");
    let top_rect = app
        .compositor
        .layout_tree()
        .group_rect(top)
        .expect("组有包围盒");
    let bottom_rect = app.compositor.layout_tree().leaf_rect(bottom).unwrap();
    assert_eq!(
        top_rect.width, bottom_rect.width,
        "组与底窗同宽（都是整宽）"
    );

    // 整组最小化 → 那个组不占布局，底部窗口吃满整个区域
    app.compositor.minimize_leaf(top, true, None);
    render_rows(&mut app, area);
    let bottom_after = app.compositor.layout_tree().leaf_rect(bottom).unwrap();
    assert_eq!(
        (bottom_after.x, bottom_after.y, bottom_after.width),
        (area.x, area.y, area.width),
        "整组折成一条后，底部窗口占满整个区域"
    );
    assert_eq!(app.compositor.layout_tree().minimized_ids(), &[top]);

    // 展开：上层回到原样
    app.compositor.minimize_leaf(top, false, None);
    render_rows(&mut app, area);
    assert!(app.compositor.layout_tree().minimized_ids().is_empty());
    let top_leaf = app.compositor.layout_tree().group_leaves(top)[0];
    assert!(
        app.compositor.layout_tree().leaf_rect(top_leaf).is_some(),
        "展开后组里的叶子又有几何了"
    );
    Ok(())
}

/// 组 zoom（`pane.zoom(组 id)` 或 Pane 模式 `f`）：组内按原分割铺满整个区域，
/// 组外叶子让位；再按 `f` 取消。
#[tokio::test(flavor = "multi_thread")]
async fn zoom_a_group_fills_the_area() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let mut app = AppBuilder::new().with_file(&file, None).build()?;
    pump(&mut app, "<C-p>r").await?; // ed1 | ed2
    pump(&mut app, "D").await?; // 底部整宽新窗口
    let bottom = app.compositor.layout_tree().active();
    pump(&mut app, "<esc>").await?;
    let area = app.compositor.area();
    render_rows(&mut app, area);

    let top = app
        .compositor
        .layout_tree()
        .group_ids()
        .into_iter()
        .find(|g| {
            let leaves = app.compositor.layout_tree().group_leaves(*g);
            leaves.len() == 2 && !leaves.contains(&bottom)
        })
        .expect("上面那排是一个组");

    // 组 zoom
    app.compositor.zoom_leaf(top, None);
    render_rows(&mut app, area);
    let r0 = app
        .compositor
        .layout_tree()
        .leaf_rect(0)
        .expect("组内叶子有几何");
    assert_eq!(r0.height, area.height - 1, "组内叶子铺满整高（除状态栏行）");
    assert!(
        app.compositor.layout_tree().leaf_rect(bottom).is_none(),
        "组外叶子在 zoom 期间不占布局"
    );
    assert_eq!(app.compositor.layout_tree().zoomed(), Some(top));

    // 取消
    app.compositor.unzoom(None);
    render_rows(&mut app, area);
    assert_eq!(app.compositor.layout_tree().zoomed(), None);
    assert!(
        app.compositor.layout_tree().leaf_rect(bottom).is_some(),
        "取消后组外叶子回来"
    );
    Ok(())
}

/// `C-p T`：核心不自己起 pty，而是请 terminal 插件的 `hterm-root` 在整窗底边开终端。
/// 插件没装时不许静默失败 —— 状态栏要给出去哪儿开。
#[tokio::test(flavor = "multi_thread")]
async fn pane_mode_cap_t_opens_full_width_terminal_via_plugin() -> anyhow::Result<()> {
    use helix_term::ui::PluginTerminal;
    let dir = tempfile::tempdir()?;
    // 桩插件：注册 hterm-root（声明 process 能力，fail-closed 之后必须声明）
    let plugin = dir.path().join("term_stub.js");
    std::fs::write(
        &plugin,
        r#"
        helix.plugin("term-stub", { deps: [], permissions: { process: true } });
        helix.register_command("hterm-root", () => {
            helix.open_terminal({ cmd: "cat", side: "bottom", size: 8, at: "root" });
        });
        "#,
    )?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hi\n")?;
    let mut app = AppBuilder::new().with_file(&file, None).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    // 先切左右两个编辑窗口，再 C-p T
    pump(&mut app, "<C-p>r").await?;
    pump(&mut app, "T").await?;
    pump(&mut app, "<esc>").await?;
    let area = app.compositor.area();
    render_rows(&mut app, area);

    let tid = app
        .compositor
        .layout_tree()
        .find_leaf_id::<PluginTerminal>(|_| true)
        .expect("终端叶子已打开");
    let trect = app
        .compositor
        .layout_tree()
        .leaf_rect(tid)
        .expect("终端有几何");
    assert_eq!(
        trect.width, area.width,
        "C-p T 开的终端必须横跨整宽（实得 {trect:?}）"
    );
    let _ = app.close().await;
    Ok(())
}

/// 没装 terminal 插件时按 `C-p T`：不崩，状态栏说明去哪儿开。
#[tokio::test(flavor = "multi_thread")]
async fn pane_mode_cap_t_without_plugin_says_where_to_get_it() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    pump(&mut app, "<C-p>T").await?;
    let st = status_text(&app);
    assert!(
        st.contains("terminal") || st.contains("hterm-root"),
        "应当提示插件/命令（实得 {st:?}）"
    );
    Ok(())
}

/// 堆叠的成员可以是**组**：`0 | (1/2)` 里把 0 与"那一排"叠成一个 tab，
/// `p` 轮转到组时组内两窗铺满同一块区域（零映射），焦点落在组内真叶子。
#[tokio::test(flavor = "multi_thread")]
async fn stack_a_leaf_with_a_group_and_rotate() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hi\n")?;
    let mut app = AppBuilder::new().with_file(&file, None).build()?;
    // 0 | (b / c)：`r` = 右分（左右），`d` = 下分（上下）⇒ 0 的兄弟是一个组
    pump(&mut app, "<C-p>r").await?;
    pump(&mut app, "d").await?;
    pump(&mut app, "h").await?; // 焦点回 0
    assert_eq!(app.compositor.layout_tree().active(), 0);
    let area = app.compositor.area();
    render_rows(&mut app, area);
    let grp = app
        .compositor
        .layout_tree()
        .sibling_of(0)
        .expect("0 的兄弟是组");
    assert_eq!(
        app.compositor.layout_tree().node_kind(grp),
        Some(helix_term::ui::layout::NodeKind::Group)
    );

    pump(&mut app, "s").await?; // 与组建堆叠
    assert!(app.compositor.layout_tree().is_stacked(0));
    assert_eq!(app.compositor.layout_tree().stack_members(0), vec![0, grp]);
    render_rows(&mut app, area);
    assert!(
        app.compositor.layout_tree().leaf_rect(1).is_none(),
        "非锚（整组）让位"
    );

    // 轮转到组：组内两窗回来占满同一块区域，焦点是组内真叶子
    pump(&mut app, "p").await?;
    render_rows(&mut app, area);
    let active = app.compositor.layout_tree().active();
    assert!(
        app.compositor
            .layout_tree()
            .group_leaves(grp)
            .contains(&active),
        "焦点必须在组内真叶子上（实得 {active}）"
    );
    assert!(
        app.compositor.layout_tree().leaf_rect(0).is_none(),
        "原锚让位"
    );
    let b = app
        .compositor
        .layout_tree()
        .leaf_rect(1)
        .expect("组内叶子有几何");
    assert_eq!(b.x, 0, "组拿到同一块区域（零映射）");
    Ok(())
}

/// `[editor] focus-scope = "group"`：方向移动止步于组边界（geometry 会跨出去）。
#[tokio::test(flavor = "multi_thread")]
async fn focus_scope_group_does_not_cross_group_boundary() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hi\n")?;

    let build = |scope: helix_view::editor::FocusScope| -> anyhow::Result<Application> {
        let mut cfg = test_config();
        cfg.editor.focus_scope = scope;
        AppBuilder::new()
            .with_file(file.clone(), None)
            .with_config(cfg)
            .build()
    };

    // 造形状：0 | (b / c)（r = 右分，d = 下分），焦点回到组里的 b
    async fn shape(app: &mut Application) -> anyhow::Result<()> {
        pump(app, "<C-p>r").await?;
        pump(app, "d").await?;
        pump(app, "k").await?; // 组内上移到 b
        Ok(())
    }

    // ① group：b 往左 ⇒ 组边界，不动
    let mut app = build(helix_view::editor::FocusScope::Group)?;
    shape(&mut app).await?;
    let b = app.compositor.layout_tree().active();
    assert_ne!(b, 0, "前置：焦点在组内（不是 0）");
    pump(&mut app, "h").await?;
    assert_eq!(
        app.compositor.layout_tree().active(),
        b,
        "focus-scope=group：不许跨出组（{b} 应留在原地）"
    );
    let st = status_text(&app);
    assert!(
        st.contains("边界"),
        "走到组边界要有提示，不能像按键坏了（实得 {st:?}）"
    );

    // ② geometry（默认）：同样的按键跨到 0
    let mut app2 = build(helix_view::editor::FocusScope::Geometry)?;
    shape(&mut app2).await?;
    assert_eq!(app2.compositor.layout_tree().active(), b);
    pump(&mut app2, "h").await?;
    assert_eq!(
        app2.compositor.layout_tree().active(),
        0,
        "focus-scope=geometry：跨到组外的 0"
    );
    Ok(())
}
