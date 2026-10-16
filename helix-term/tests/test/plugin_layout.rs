use helix_term::application::Application;
use helix_term::job::Jobs;
use helix_view::input::parse_macro;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

use super::*;

/// 驱动若干按键直到空闲(与本目录其它插件用例同款;helpers 里没有这个包装)
async fn pump(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(keys)? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn panel_shrinks_editor() -> anyhow::Result<()> {
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
        js: None,
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

/// D-2 收尾(意图队列 + 帧首 flush)的**回归钉**:一批布局变更 ⇒ **每条意图一个事件**,
/// 不被合并成一条,且顺序保持 —— 这是"把事件从变更点挪到边界处"的最大回归风险
/// (天真实现会把同一批合成一条)。
///
/// 为什么期望是 4 条而不是 2 条(实测记录,不是猜的):一次 `helix.split` 有**两个**
/// sync 点 —— ① `split_leaf_prealloc` 内部那次(此刻还没有种类 ⇒ `None`),
/// ② 请求臂随后 `set_layout_change_kind("split")` 再 sync 一次(带种类)。
/// 两个 split ⇒ 序列 `none, split, none, split`。这条断言把**当前**投递语义写死;
/// 若以后去掉冗余的那个 sync 点(会少一半事件),这里是必须一起改的地方。
#[tokio::test(flavor = "multi_thread")]
async fn layout_change_batch_keeps_one_event_per_intent() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("kinds.txt");
    let plugin = dir.path().join("lc_batch.js");
    std::fs::write(
        &plugin,
        format!(
            r#"helix.on("layout-change", (kind) => {{
                 let prev = "";
                 try {{ prev = helix.read_file({log:?}); }} catch (e) {{ prev = ""; }}
                 helix.write_file({log:?}, prev + (kind || "none") + "\n");
               }});
helix.register_command("lc-batch", () => {{
  helix.split("right", {{ terminal: {{ cmd: "true" }} }});
  helix.split("bottom", {{ terminal: {{ cmd: "true" }} }});
}});"#,
            log = log.to_string_lossy()
        ),
    )?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    pump(&mut app, ":lc-batch<ret>").await?;

    let got = std::fs::read_to_string(&log)?;
    let lines: Vec<&str> = got.lines().collect();
    assert_eq!(
        lines,
        vec!["none", "split", "none", "split"],
        "一次变更两个 sync 点(内部 none + 臂 split)、两次 split ⇒ 4 条;实得 {got:?}"
    );
    Ok(())
}
