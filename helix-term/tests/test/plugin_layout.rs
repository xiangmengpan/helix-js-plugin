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

/// `layout-change` 的**批量模式**(`editor.js-layout-change-batch = true`):
/// 与逐点模式**条数/顺序相同**,但每条事件都在**最终布局**上观察 —— 这正是开关的语义,
/// 也是与上面那条逐点用例互为镜像的断言(同样的探针,只换配置)。
#[tokio::test(flavor = "multi_thread")]
async fn layout_change_batch_mode_sees_final_layout() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("kinds_batch.txt");
    let plugin = dir.path().join("lc_batch_mode.js");
    std::fs::write(
        &plugin,
        format!(
            r#"helix.on("layout-change", (kind) => {{
                 let prev = "";
                 try {{ prev = helix.read_file({log:?}); }} catch (e) {{ prev = ""; }}
                 const panes = (helix.pane.list() || []).length;
                 helix.write_file({log:?}, prev + (kind || "none") + ":" + panes + "\n");
               }});
helix.register_command("lc-batch2", () => {{
  helix.split("right", {{ terminal: {{ cmd: "true" }} }});
  helix.split("bottom", {{ terminal: {{ cmd: "true" }} }});
}});"#,
            log = log.to_string_lossy()
        ),
    )?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;

    let mut cfg = test_config();
    cfg.editor.js_layout_change_batch = true;
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(cfg)
        .build()?;
    // 前置:配置真的进到编辑器了(否则测的是"默认逐点"而不是批量)
    assert!(
        app.editor.config().js_layout_change_batch,
        "前置不成立:js-layout-change-batch 未生效"
    );
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    pump(&mut app, ":lc-batch2<ret>").await?;

    let got = std::fs::read_to_string(&log)?;
    let lines: Vec<&str> = got.lines().collect();
    // 批量模式 = **每个投递窗口合并成一条**:两次 split 各自是一个 job 回调(窗口)
    // ⇒ 2 条(逐点模式是 4 条);`kind` 取窗口内最后一次标注(原语内部那次是 None)。
    assert_eq!(lines.len(), 2, "批量模式应合并同窗事件;实得 {got:?}");
    let kinds: Vec<&str> = lines.iter().map(|l| l.split(':').next().unwrap()).collect();
    assert_eq!(kinds, vec!["split", "split"], "实得 {got:?}");
    // 每条看到的是**该窗口结束时**的布局(2 个窗格 → 3 个窗格)
    let panes: Vec<usize> = lines
        .iter()
        .map(|l| l.rsplit(':').next().unwrap().parse().unwrap())
        .collect();
    assert_eq!(panes, vec![2, 3], "实得 {panes:?}({got:?})");
    Ok(())
}

/// `layout-change` 的**逐点语义**回归钉:一批布局变更 ⇒ **每条意图一个事件**,
/// 且每条事件看到的是**该变更点当时**的布局(不是整批做完后的最终布局)。
///
/// 这条用例的形状是踩过两次坑之后定下来的:
///   · 事件**条数**必须保持(不能被合并)—— 一次 `helix.split` 有**两个** sync 点
///     (① `split_leaf_prealloc` 内部那次、此刻还没有种类 ⇒ `none`;② 请求臂随后
///     `set_layout_change_kind("split")` 再 sync 一次 ⇒ `split`),两次 split ⇒ 4 条;
///   · 每条事件里的**窗格数**必须是**递增**的(逐点语义);若实现改回"攒到帧首再发",
///     四条会都看到最终窗格数(`3,3,3,3`)⇒ 这条断言会红。
#[tokio::test(flavor = "multi_thread")]
async fn layout_change_is_per_change_point() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("kinds.txt");
    let plugin = dir.path().join("lc_batch.js");
    std::fs::write(
        &plugin,
        format!(
            r#"helix.on("layout-change", (kind) => {{
                 let prev = "";
                 try {{ prev = helix.read_file({log:?}); }} catch (e) {{ prev = ""; }}
                 const panes = (helix.pane.list() || []).length;
                 helix.write_file({log:?}, prev + (kind || "none") + ":" + panes + "\n");
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
        lines.len(),
        4,
        "两个 split × 两个 sync 点 ⇒ 4 条事件;实得 {got:?}"
    );
    let kinds: Vec<&str> = lines.iter().map(|l| l.split(':').next().unwrap()).collect();
    assert_eq!(
        kinds,
        vec!["none", "split", "none", "split"],
        "实得 {got:?}"
    );
    // 逐点语义的关键断言:窗格数**递增**(不是四条都等于最终值)
    let panes: Vec<usize> = lines
        .iter()
        .map(|l| l.rsplit(':').next().unwrap().parse().unwrap())
        .collect();
    assert!(
        panes.windows(2).all(|w| w[0] <= w[1]) && panes[0] < panes[3],
        "每条事件应看到**该变更点当时**的窗格数(递增),而不是最终布局;实得 {panes:?}({got:?})"
    );
    Ok(())
}
