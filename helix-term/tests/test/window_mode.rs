use super::*;
use helix_term::application::Application;
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

/// 方向键映射:h → 左邻居、l → 右、k → 上、j → 下(布局模式聚焦)
#[tokio::test(flavor = "multi_thread")]
async fn window_mode_directional_focus() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    // 编辑器(id=0)右侧切面板(id=1):H split,second=面板
    app.compositor.split_leaf_with_ratio(
        SplitDir::H,
        false,
        40,
        Box::new(PluginPanel::new(1, PanelSide::Right)),
    );
    let active = app.compositor.layout_tree().active();
    assert_eq!(active, 1, "面板打开后活动叶子=面板");
    pump(&mut app, "<C-w>h<esc>").await?;
    assert_eq!(
        app.compositor.layout_tree().active(),
        0,
        "h → 左邻居(编辑器)"
    );
    pump(&mut app, "<C-w>l<esc>").await?;
    assert_eq!(app.compositor.layout_tree().active(), 1, "l → 右邻居(面板)");
    Ok(())
}

/// 最小化/最大化/关闭往返
#[tokio::test(flavor = "multi_thread")]
async fn window_mode_close_minimize_zoom() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    app.compositor.split_leaf_with_ratio(
        SplitDir::H,
        false,
        40,
        Box::new(PluginPanel::new(1, PanelSide::Right)),
    );
    // 最小化往返
    pump(&mut app, "<C-w>z<esc>").await?;
    assert!(
        app.compositor.layout_tree().minimized().is_some(),
        "z 最小化"
    );
    pump(&mut app, "<C-w>z<esc>").await?;
    assert!(
        app.compositor.layout_tree().minimized().is_none(),
        "再 z 还原"
    );
    // 回面板叶子,最大化往返
    pump(&mut app, "<C-w>l<esc>").await?;
    pump(&mut app, "<C-w>f<esc>").await?;
    assert!(app.compositor.layout_tree().zoomed() == Some(1), "f 最大化");
    pump(&mut app, "<C-w>f<esc>").await?;
    assert!(app.compositor.layout_tree().zoomed().is_none(), "再 f 还原");
    // 关闭
    pump(&mut app, "<C-w>x<esc>").await?;
    assert!(!app.compositor.has_component(panel_type), "x 关闭面板叶子");
    Ok(())
}

/// 交换(H 与左邻居换内容)与 resize(C-l 宽度 +5%)不回归
#[tokio::test(flavor = "multi_thread")]
async fn window_mode_swap_and_resize() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    app.compositor.split_leaf_with_ratio(
        SplitDir::H,
        false,
        40,
        Box::new(PluginPanel::new(1, PanelSide::Right)),
    );
    // 活动=面板(1),H + h 与左邻居(编辑器)交换内容;焦点仍在 id=1
    pump(&mut app, "<C-w>H<esc>").await?;
    let types: Vec<_> = app.compositor.layout_tree().leaf_types();
    assert_eq!(types[0], panel_type, "交换后左叶子为面板");
    assert_eq!(
        types[1], "helix_term::ui::editor::EditorView",
        "交换后右叶子为编辑器"
    );
    // resize:宽度 +5%,不 panic 且布局仍 2 叶
    pump(&mut app, "<C-w>C-l<esc>").await?;
    assert_eq!(
        app.compositor.layout_tree().leaf_types().len(),
        2,
        "resize 后仍 2 叶"
    );
    Ok(())
}

/// 编排层裁定:insert 模式 C-w 不进窗口模式,保留删词语义(delete_word_backward)
#[tokio::test(flavor = "multi_thread")]
async fn window_mode_not_entered_in_insert() -> anyhow::Result<()> {
    // 删词行为与 commands::test_delete_word_backward 同断言
    test((
        "fo#[o|]#ba#(r|)#",
        "a<C-w><esc>",
        "#[
|]#",
    ))
    .await?;
    // 且不进入窗口模式
    let mut app = AppBuilder::new().with_input_text("fo#[o|]#ba(r)").build()?;
    pump(&mut app, "a<C-w>").await?;
    assert!(
        !app.compositor.window_mode_active(),
        "insert 模式 C-w 不进窗口模式"
    );
    Ok(())
}

/// 弹窗/菜单打开时自动退出窗口模式(否则弹窗按键被模式键位吞掉)
#[tokio::test(flavor = "multi_thread")]
async fn window_mode_exits_on_popup_open() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    pump(&mut app, "<C-w>").await?;
    assert!(app.compositor.window_mode_active(), "C-w 进模式");
    // 打开一个弹窗层(仿 filetree prompt:push popup 类 layer)
    let popup = helix_term::ui::Popup::new(
        "plugin-popup",
        helix_term::ui::PluginPopup::new(99, Some((10, 5))),
    )
    .position(Some(helix_core::Position::new(0, 0)));
    app.compositor.push(Box::new(popup));
    assert!(!app.compositor.window_mode_active(), "弹窗打开自动退模式");
    Ok(())
}

/// fixed 叶子:set_leaf_fixed 标记生效;模式内 x 不关闭;get_layout/dump 输出 fixed 字段
#[tokio::test(flavor = "multi_thread")]
async fn window_mode_fixed_leaf_immune() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    app.compositor.split_leaf_with_ratio(
        SplitDir::H,
        false,
        40,
        Box::new(PluginPanel::new(1, PanelSide::Right)),
    );
    // 面板(id=1)设为固定
    let dump = app.compositor.layout_tree().dump();
    let fixed_id = dump.leafs.iter().find(|l| l.id == 1).unwrap().id;
    app.compositor.set_leaf_fixed(fixed_id, true);
    assert!(
        app.compositor.layout_tree().is_fixed(fixed_id),
        "fixed 标记生效"
    );
    // 模式内 x 不关闭 fixed 叶子
    pump(&mut app, "<C-w>x<esc>").await?;
    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    assert!(
        app.compositor.has_component(panel_type),
        "fixed 叶子不被 x 关闭"
    );
    // dump 输出 fixed 字段
    let dump = app.compositor.layout_tree().dump();
    let f = dump.leafs.iter().find(|l| l.id == fixed_id).unwrap();
    assert!(f.fixed, "dump 含 fixed 字段");
    // 陷阱回归:先最小化后 fixed 的叶子可还原(还原不受 fixed 拦截;否则无恢复路径)
    app.compositor.set_leaf_fixed(fixed_id, false);
    pump(&mut app, "<C-w>z<esc>").await?;
    assert!(app.compositor.layout_tree().minimized().is_some(), "z 最小化(未 fixed)");
    app.compositor.set_leaf_fixed(fixed_id, true);
    pump(&mut app, "<C-w>z<esc>").await?;
    assert!(
        app.compositor.layout_tree().minimized().is_none(),
        "fixed×minimized 叶子可还原(z 往返)"
    );
    Ok(())
}

/// buffer_open:JS 打开文件为新 BufferLeaf 叶子(split:h → 原编辑器右侧新叶)
#[tokio::test(flavor = "multi_thread")]
async fn buffer_open_creates_leaf_with_content() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "AAA\n")?;
    std::fs::write(&b, "BBB\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    // JS 侧调 buffer_open(b.txt, {split:"h"}) → 新叶子显示 b.txt
    let src = r#"
        helix.register_command("bo", () => {
            helix.buffer_open(ARG_PATH, { split: "h" });
        });
    "#
    .replace("ARG_PATH", &format!("{:?}", b.display()));
    let plugin_path = dir.path().join("bo.js");
    std::fs::write(&plugin_path, src)?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;
    pump(&mut app, ":bo<ret>").await?;
    // 布局:2 叶(原编辑器 0 + 新 buffer 叶)
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 2, "buffer_open 新增叶子: {types:?}");
    assert!(
        types.contains(&"BufferLeaf"),
        "新叶子类型为 BufferLeaf: {types:?}"
    );
    // 渲染断言:新叶子区域含 BBB
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    let joined = rows.join("\n");
    assert!(joined.contains("BBB"), "新叶子渲染 b.txt 内容: {joined:?}");
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
    assert!(
        !rows[29].contains("[WINDOW]"),
        "退出后指示消失: {:?}",
        &rows[29]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn zoom_unzoom_restores_tree_render() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    app.compositor.split_leaf_with_ratio(SplitDir::H, false, 40, Box::new(PluginPanel::new(1, PanelSide::Right)));
    pump(&mut app, "<C-w>f<esc>").await?;
    assert_eq!(app.compositor.layout_tree().zoomed(), Some(1));
    pump(&mut app, "<C-w>f<esc>").await?;
    assert!(app.compositor.layout_tree().zoomed().is_none());
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    let r0 = &rows[0];
    assert!(r0.contains('┐'), "unzoom 后面板边框恢复: {r0:?}");
    Ok(())
}

/// commandline(prompt)活跃时状态栏上移一行、提示符占最底行;关闭后状态栏回最底行(可隐藏)
#[tokio::test(flavor = "multi_thread")]
async fn commandline_yields_bottom_row_to_statusline_when_hidden() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    // 无 prompt:状态栏在最底行(行 29)
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    assert!(!rows[29].trim().is_empty(), "无 prompt 时状态栏在最底行: {:?}", &rows[29]);
    // 打开命令 prompt(':' 键)
    pump(&mut app, ":").await?;
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    assert!(rows[29].contains(":"), "prompt 输入行占最底行: {:?}", &rows[29]);
    assert!(!rows[28].trim().is_empty(), "状态栏上移到倒数第 2 行: {:?}", &rows[28]);
    // 关闭(Esc)→ 状态栏回最底行
    pump(&mut app, "<esc>").await?;
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    assert!(!rows[29].trim().is_empty(), "关闭后状态栏回最底行: {:?}", &rows[29]);
    Ok(())
}
