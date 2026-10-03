use super::*;
use helix_term::application::Application;
use helix_term::compositor::PaneMode;
use helix_term::job::Jobs;
use helix_term::ui::layout::SplitDir;
use helix_term::ui::plugin_panel::PanelSide;
use helix_term::ui::PluginPanel;
use helix_view::current_ref;
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
    assert_eq!(app.compositor.pane_mode(), PaneMode::Normal, "初始非模式");
    pump(&mut app, "<C-p>").await?;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Pane,
        "C-p 进 Pane 模式"
    );
    pump(&mut app, "<esc>").await?;
    assert_eq!(app.compositor.pane_mode(), PaneMode::Normal, "Esc 退模式");
    pump(&mut app, "<C-p><C-p>").await?;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Normal,
        "模式内同键退出"
    );
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
        Box::new(PluginPanel::new(1, PanelSide::Right, false)),
    );
    let active = app.compositor.layout_tree().active();
    assert_eq!(active, 1, "面板打开后活动叶子=面板");
    pump(&mut app, "<C-p>h<esc>").await?;
    assert_eq!(
        app.compositor.layout_tree().active(),
        0,
        "h → 左邻居(编辑器)"
    );
    pump(&mut app, "<C-p>l<esc>").await?;
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
        Box::new(PluginPanel::new(1, PanelSide::Right, false)),
    );
    // 最小化往返
    pump(&mut app, "<C-p>z<esc>").await?;
    assert!(
        app.compositor.layout_tree().minimized().is_some(),
        "z 最小化"
    );
    pump(&mut app, "<C-p>z<esc>").await?;
    assert!(
        app.compositor.layout_tree().minimized().is_none(),
        "再 z 还原"
    );
    // 回面板叶子,最大化往返
    pump(&mut app, "<C-p>l<esc>").await?;
    pump(&mut app, "<C-p>f<esc>").await?;
    assert!(app.compositor.layout_tree().zoomed() == Some(1), "f 最大化");
    pump(&mut app, "<C-p>f<esc>").await?;
    assert!(app.compositor.layout_tree().zoomed().is_none(), "再 f 还原");
    // 关闭
    pump(&mut app, "<C-p>x<esc>").await?;
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
        Box::new(PluginPanel::new(1, PanelSide::Right, false)),
    );
    // 活动=面板(1),H + h 与左邻居(编辑器)交换内容;焦点仍在 id=1
    pump(&mut app, "<C-p>H<esc>").await?;
    let types: Vec<_> = app.compositor.layout_tree().leaf_types();
    assert_eq!(types[0], panel_type, "交换后左叶子为面板");
    assert_eq!(
        types[1], "helix_term::ui::editor::EditorView",
        "交换后右叶子为编辑器"
    );
    // resize:宽度 +5%,不 panic 且布局仍 2 叶
    pump(&mut app, "<C-n>l<esc>").await?;
    assert_eq!(
        app.compositor.layout_tree().leaf_types().len(),
        2,
        "resize 后仍 2 叶"
    );
    Ok(())
}

/// 拦截规则:insert 里前缀键放行(`C-h` 是删词,不豁免会毁掉退格);
/// 而 `C-w` 是编辑器原有的删词命令,与模式无关。
#[tokio::test(flavor = "multi_thread")]
async fn pane_mode_not_entered_in_insert() -> anyhow::Result<()> {
    // 删词行为与 commands::test_delete_word_backward 同断言
    test((
        "fo#[o|]#ba#(r|)#",
        "a<C-w><esc>",
        "#[
|]#",
    ))
    .await?;
    // 新前缀 C-h 在 insert 里必须放行(不当模式键)
    let mut app = AppBuilder::new().with_input_text("fo#[o|]#ba(r)").build()?;
    pump(&mut app, "a<C-h>").await?;
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Normal,
        "insert 模式 C-h 不进 Move 模式(它是删词)"
    );
    Ok(())
}

/// 弹窗/菜单打开时自动退出窗口模式(否则弹窗按键被模式键位吞掉)
#[tokio::test(flavor = "multi_thread")]
async fn window_mode_exits_on_popup_open() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    pump(&mut app, "<C-p>").await?;
    assert_eq!(app.compositor.pane_mode(), PaneMode::Pane, "C-p 进模式");
    // 打开一个弹窗层(仿 filetree prompt:push popup 类 layer)
    let popup = helix_term::ui::Popup::new(
        "plugin-popup",
        helix_term::ui::PluginPopup::new(99, Some((10, 5))),
    )
    .position(Some(helix_core::Position::new(0, 0)));
    app.compositor.push(Box::new(popup));
    assert_eq!(
        app.compositor.pane_mode(),
        PaneMode::Normal,
        "弹窗打开自动退模式"
    );
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
        Box::new(PluginPanel::new(1, PanelSide::Right, false)),
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
    pump(&mut app, "<C-p>x<esc>").await?;
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
    pump(&mut app, "<C-p>z<esc>").await?;
    assert!(
        app.compositor.layout_tree().minimized().is_some(),
        "z 最小化(未 fixed)"
    );
    app.compositor.set_leaf_fixed(fixed_id, true);
    pump(&mut app, "<C-p>z<esc>").await?;
    assert!(
        app.compositor.layout_tree().minimized().is_none(),
        "fixed×minimized 叶子可还原(z 往返)"
    );
    Ok(())
}

/// buffer_open 叶子可编辑:聚焦新叶后输入应命中其自身 doc(而非编辑器叶 doc),
/// 且编辑器叶渲染不被双画(防双画由 claimed 过滤保证)。
#[tokio::test(flavor = "multi_thread")]
async fn buffer_open_leaf_is_editable_targets_own_doc() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "AAA\n")?;
    std::fs::write(&b, "BBB\n")?;
    let mut app = AppBuilder::new().with_file(a.clone(), None).build()?;
    let src = r#"
        helix.register_command("bo", () => {
            helix.buffer_open(ARG_PATH, { split: "h" });
        });
    "#
    .replace("ARG_PATH", &format!("{:?}", b.display()));
    let plugin_path = dir.path().join("bo.js");
    std::fs::write(&plugin_path, src)?;
    pump(
        &mut app,
        &format!(":plugin-load {}<ret>", plugin_path.display()),
    )
    .await?;
    pump(&mut app, ":bo<ret>").await?;
    // 布局断言:2 叶(编辑器 0 + BufferLeaf)
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 2, "buffer_open 新增叶子: {types:?}");
    // 输入 "XXX" → 应命中 b.txt 的 doc(活动叶=B),a.txt 不被改动
    pump(&mut app, "iXXX<esc>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "XXXBBB\n",
        "输入应命中 BufferLeaf 自身 doc(b.txt): {:?}",
        doc.text().to_string()
    );
    // a.txt 的 doc 未被编辑(保持 AAA)\n)
    let a_doc = app
        .editor
        .documents
        .values()
        .find(|d| d.path().is_some_and(|p| p.ends_with("a.txt")))
        .unwrap();
    assert_eq!(a_doc.text().to_string(), "AAA\n", "编辑器叶 doc 不应被改");
    let _ = b;
    Ok(())
}

/// buffer_open:JS 打开文件为新 BufferLeaf 叶子(split:h → 原编辑器右侧新叶)
#[tokio::test(flavor = "multi_thread")]
async fn buffer_open_creates_leaf_with_content() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

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
    pump(
        &mut app,
        &format!(":plugin-load {}<ret>", plugin_path.display()),
    )
    .await?;
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
    pump(&mut app, "<C-p>").await?;
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    let status = &rows[29];
    assert!(
        !status.contains("[WINDOW]"),
        "不显示 [WINDOW] 文本: {status:?}"
    );
    // 窗口模式指示 = 窗口图标(Rust 默认状态栏;replace 模式 statusline.js 从 ICONS 引入同款)
    assert!(status.contains('\u{f108}'), "显示窗口图标: {status:?}");
    pump(&mut app, "<esc>").await?;
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    assert!(
        !rows[29].contains('\u{f108}'),
        "退出后窗口图标消失: {:?}",
        &rows[29]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn zoom_unzoom_restores_tree_render() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    app.compositor.split_leaf_with_ratio(
        SplitDir::H,
        false,
        40,
        Box::new(PluginPanel::new(1, PanelSide::Right, false)),
    );
    pump(&mut app, "<C-p>f<esc>").await?;
    assert_eq!(app.compositor.layout_tree().zoomed(), Some(1));
    pump(&mut app, "<C-p>f<esc>").await?;
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
    assert!(
        !rows[29].trim().is_empty(),
        "无 prompt 时状态栏在最底行: {:?}",
        &rows[29]
    );
    // 打开命令 prompt(':' 键)
    pump(&mut app, ":").await?;
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    assert!(
        rows[28].trim_start().starts_with(":"),
        "prompt 输入行在状态栏上一行(倒数第 2 行): {:?}",
        &rows[28]
    );
    assert!(
        !rows[29].trim().is_empty(),
        "状态栏永远在最底行: {:?}",
        &rows[29]
    );
    // 关闭(Esc)→ prompt 行消失,状态栏仍在最底行
    pump(&mut app, "<esc>").await?;
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    assert!(
        !rows[29].trim().is_empty(),
        "关闭后状态栏仍最底行: {:?}",
        &rows[29]
    );
    Ok(())
}

// window mode 创建键:模式内 v/s 同 doc 分屏、n 新空 buffer
#[tokio::test(flavor = "multi_thread")]
async fn window_mode_creates_splits_v_s_n() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "AAA\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    // v:右分同 doc
    pump(&mut app, "<C-p>r<esc>").await?;
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 2, "C-p r 应 2 叶: {types:?}");
    assert_eq!(app.editor.documents.len(), 1, "同 doc 不新增");
    assert!(types.contains(&"BufferLeaf"));
    // 右叶活动 → 编辑同 doc
    pump(&mut app, "iX<esc>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.text().to_string(), "XAAA\n");
    // s:下分同 doc(3 叶)
    pump(&mut app, "<C-p>d<esc>").await?;
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 3, "C-p d 后应 3 叶: {types:?}");
    // n:新空 buffer(4 叶)
    pump(&mut app, "<C-p>n<esc>").await?;
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 4, "C-p n 后应 4 叶: {types:?}");
    assert_eq!(app.editor.documents.len(), 2, "scratch + a");
    let (_, doc) = current_ref!(app.editor);
    assert!(doc.path().is_none(), "n 创建空 scratch 且活动");
    Ok(())
}
