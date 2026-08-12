use super::*;

use helix_term::job::Jobs;

#[tokio::test(flavor = "multi_thread")]
async fn panel_shrinks_editor() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("ly.txt");
    std::fs::write(&file, "x\n")?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    // 直接推面板层（等价于 JS open_panel 的 replace_or_push 路径，绕过事件循环——
    // 测试助手会在序列末尾发 :q! 退出应用、清空 view tree，无法再渲染）
    app.compositor.replace_or_push(
        "plugin-panel",
        helix_term::ui::PluginPanel::new(1, helix_term::ui::PanelSide::Right, 20),
    );

    // 渲染 compositor 到 Buffer，断言编辑器区域被面板收缩：
    // - 面板区（右侧 20 列）被面板内容占据（Paragraph 从面板区顶部渲染）
    // - 状态栏由 EditorView 画（editor_area 为视口 clip 1 行给 commandline，
    //   故状态栏在倒数第 2 行）——收缩后宽度 = 视口 - 20，右对齐内容
    //   （"1 sel  1:1"）不越过第 100 列
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

    // 面板内容（render 回调未注册时是错误行）出现在右侧 20 列
    let top_row = row(0);
    assert!(
        right20(&top_row).contains("<plugin panel error:"),
        "panel should render in right 20 cols: {top_row:?}"
    );
    // 状态栏在倒数第 2 行存在（位置指示 1:1 与文件名），但最右 20 列为空
    let status_row = row(28);
    assert!(
        status_row.contains("1:1") && status_row.contains("ly.txt"),
        "statusline should render: {status_row:?}"
    );
    let status_right20 = right20(&status_row);
    assert!(
        status_right20.trim().is_empty(),
        "statusline should not extend under panel: {status_right20:?}"
    );

    Ok(())
}
