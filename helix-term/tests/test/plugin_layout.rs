use super::*;

use helix_term::job::Jobs;

#[tokio::test(flavor = "multi_thread")]
async fn panel_shrinks_editor() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("ly.txt");
    std::fs::write(&file, "x\n")?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    // 面板作为布局树叶子（split 编辑器叶子，右侧 20 列）
    app.compositor.split_leaf_with_ratio(
        helix_term::ui::layout::SplitDir::H,
        false,
        20,
        Box::new(helix_term::ui::PluginPanel::new(
            1,
            helix_term::ui::PanelSide::Right,
            false,
        )),
    );

    // 渲染 compositor 到 Buffer，断言编辑器区域被面板收缩：
    // - 面板区（右侧 20 列）被面板占据：焦点边框顶边在第 0 行，
    //   内容（render 回调未注册时是错误行）inset 1 格从第 1 行开始
    // - 状态栏已全局化：永远屏幕最底行（第 29 行），跨全宽——面板被
    //   clip_bottom(1) 裁剪，不再覆盖/截断状态栏
    let area = helix_view::graphics::Rect::new(0, 0, 120, 30);
    let mut buf = tui::buffer::Buffer::empty(area);
    let mut jobs = Jobs::new();
    let mut cx = helix_term::compositor::Context {
        editor: &mut app.editor,
        scroll: None,
        jobs: &mut jobs,
    };
    // DiffRenderer 有持久状态：重置全部插件面板 diff，让本 buffer 完整重绘
    app.compositor.reset_plugin_diffs();
    app.compositor.render(area, &mut buf, &mut cx);

    let row = |y: usize| -> String {
        buf.content
            .iter()
            .skip(y * 120)
            .take(120)
            .map(|c| c.symbol.as_str())
            .collect()
    };
    let right20 = |row: &str| -> String { row.chars().skip(100).collect() };

    // 面板内容（render 回调未注册时是错误行）出现在右侧 20 列（inset 1 → 第 1 行起）
    let panel_row = row(1);
    assert!(
        right20(&panel_row).contains("plugin panel erro"),
        "panel content should render in right 20 cols: {panel_row:?}"
    );
    // 焦点边框：顶边在第 0 行（面板 rect 0-19 列 → 边框角/边在右 20 列）
    let top_row = row(0);
    assert!(
        right20(&top_row).contains("┌") && right20(&top_row).contains("┐"),
        "panel focus border top edge should be at row 0: {top_row:?}"
    );
    // 全局状态栏在屏幕最底行（第 29 行）：位置指示 1:1 与文件名，跨全宽
    let status_row = row(29);
    assert!(
        status_row.contains("1:1") && status_row.contains("ly.txt"),
        "statusline should render at screen bottom: {status_row:?}"
    );
    // 面板被 clip_bottom(1) 裁剪，不占用状态栏行——状态栏右端可右对齐到最右列
    let status_right20 = right20(&status_row);
    assert!(
        !status_right20.trim().is_empty() && status_row.trim_end().ends_with("1:1"),
        "statusline should extend full width (panel clipped above it): {status_row:?}"
    );

    Ok(())
}
