use super::*;

use helix_term::job::Jobs;

// Windows 上这些用例的 `Event`/`KeyEvent` 用法已随 pty/流式路径 gate 掉 ⇒
// 导入留着但允许未使用（否则整仓 Windows gate 会因 warning 变红）。
#[cfg(windows)]
#[allow(unused_imports)]
use crossterm::event::{Event, KeyEvent};

/// 布局树里 split 出的面板：事件循环渲染后再向新 buffer 渲染应完整重画。
/// 回归：插件面板渲染原为跨帧 diff（状态失效导致新 surface 空白）；现为全量渲染，
/// 任何 surface 都完整重画。此测试锁定面板在布局树叶子中也能被渲染出来。
#[tokio::test(flavor = "multi_thread")]
async fn tree_panel_diff_reset() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("mini.js");
    std::fs::write(
        &plugin,
        r#"
        helix.register_command("mini", () => {
            helix.open_panel({ side: "left", size: 32, render: () => ["hello-panel"] });
        });
        "#,
    )?;
    let mut app = AppBuilder::new().build()?;
    let keys = format!(":plugin-load {}<ret>:mini<ret>", plugin.display());
    pump(&mut app, &keys).await?;

    let area = helix_view::graphics::Rect::new(0, 0, 120, 30);
    let mut buf = tui::buffer::Buffer::empty(area);
    let mut jobs = Jobs::new();
    {
        let mut cx = helix_term::compositor::Context {
            editor: &mut app.editor,
            scroll: None,
            jobs: &mut jobs,
            js: None,
        };
        app.compositor.render(area, &mut buf, &mut cx);
    }
    // 重置 diff 后再渲染一次到新 buffer：面板内容必须完整出现（tree 叶子）
    let rows = render_rows(&mut app, area);
    let left: String = rows
        .iter()
        .map(|r| r.chars().take(32).collect::<String>())
        .collect();
    assert!(
        left.contains("hello-panel"),
        "tree 面板 diff 重置后重绘: {left:?}"
    );
    Ok(())
}

/// `helix.pane.list()` 里每个 pane 都要有**绝对单元格几何** `area`（计划任务 1）。
///
/// 背景：此前只有浮窗有 `rect`（0..1 比例），平铺叶什么都不给 ⇒ 插件拿到鼠标
/// `component-event(x,y)` 也无法换算成"面板内第几行"。
const PANE_AREA_JS: &str = r#"
helix.register_command("pane-area", () => {
    const p = helix.pane.list().find((x) => x.place === "tiled");
    if (!p) { helix.echo("area:nopane"); return; }
    if (!p.area) { helix.echo("area:nofield"); return; }
    helix.echo("area:" + p.id + "," + p.area.x + "," + p.area.y + "," + p.area.w + "," + p.area.h);
});
"#;

/// 解析 "area:id,x,y,w,h" → (id, x, y, w, h)
fn parse_area(s: &str) -> (u64, u16, u16, u16, u16) {
    let n: Vec<u32> = s
        .trim_start_matches("area:")
        .split(',')
        .map(|v| v.parse().expect("area 字段必须是整数"))
        .collect();
    assert_eq!(n.len(), 5, "area 字段数不对: {s}");
    (
        n[0] as u64,
        n[1] as u16,
        n[2] as u16,
        n[3] as u16,
        n[4] as u16,
    )
}

async fn pane_area_app() -> anyhow::Result<(tempfile::TempDir, helix_term::application::Application)>
{
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("area.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("area.js");
    std::fs::write(&plugin, PANE_AREA_JS)?;
    let mut app = AppBuilder::new().with_file(&file, None).build()?;
    pump(
        &mut app,
        &format!(":plugin-load {}<ret>:pane-area<ret>", plugin.display()),
    )
    .await?;
    Ok((dir, app))
}

#[tokio::test(flavor = "multi_thread")]
async fn pane_list_tiled_has_absolute_area() -> anyhow::Result<()> {
    let (_dir, app) = pane_area_app().await?;
    let (status, _) = app.editor.get_status().unwrap();
    let s = status.as_ref();
    assert!(
        s.starts_with("area:") && s != "area:nofield" && s != "area:nopane",
        "pane.list() 缺 area 字段: {s}"
    );
    let (_id, _x, _y, w, h) = parse_area(s);
    assert!(w > 0 && h > 0, "area 尺寸必须非零: {s}");
    Ok(())
}

/// 审查重点 #1：`area` 必须与**鼠标事件**同一坐标系 ——
/// 用 area 的中心点反查 `leaf_id_at`（鼠标命中用的就是它），必须命中同一个叶子。
/// 若 area 少算/多算了一个状态栏高度，这条会红。
#[tokio::test(flavor = "multi_thread")]
async fn pane_area_shares_mouse_coordinate_space() -> anyhow::Result<()> {
    let (_dir, mut app) = pane_area_app().await?;
    let (status, _) = app.editor.get_status().unwrap();
    let s = status.as_ref().to_string();
    let (id, x, y, w, h) = parse_area(&s);
    let (cx, cy) = (x + w / 2, y + h / 2);
    assert_eq!(
        app.compositor.layout_tree().leaf_id_at(cx, cy),
        Some(id),
        "area={s} 的中心点 ({cx},{cy}) 命中不到自己 ⇒ 与鼠标坐标系不一致"
    );
    Ok(())
}

/// JS 侧的组能力：`helix.split("bottom", {panel, at:"root"})` 出来的是**整宽**的窗口，
/// 并且 `helix.pane.list()` 能看见组（`kind:"group"` + `children`/`leaves`/`area`）。
const GROUP_SPLIT_JS: &str = r#"
helix.register_command("g-split", () => {
    // 先在**当前窗口**上切一刀（普通行为）
    helix.split("right", { panel: { render: () => ["p1"], size: 30 } });
    // 再在**整窗**上切一刀：新窗口横跨整宽，贴在底边
    helix.split("bottom", { panel: { render: () => ["p2"], size: 8 }, at: "root" });
});
helix.register_command("g-diag", () => {
    const panes = helix.pane.list();
    const g = panes.filter((p) => p.kind === "group");
    const bottom = panes.filter((p) => p.place === "tiled" && p.area.x === 0 && p.area.w > 0)
        .sort((a, b) => b.area.y - a.area.y)[0];
    const widest = panes.reduce((m, p) => Math.max(m, p.area.w), 0);
    helix.echo(
        "groups:" + g.length
        + " children:" + (g.length ? g[g.length - 1].children.length : 0)
        + " leaves:" + (g.length ? g[g.length - 1].leaves.length : 0)
        + " bottom_w:" + (bottom ? bottom.area.w : -1)
        + " widest:" + widest
    );
});
"#;

#[tokio::test(flavor = "multi_thread")]
async fn js_split_at_root_is_full_width_and_groups_are_listed() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("groups.js");
    std::fs::write(&plugin, GROUP_SPLIT_JS)?;
    let mut app = AppBuilder::new().build()?;
    pump(
        &mut app,
        &format!(":plugin-load {}<ret>:g-split<ret>", plugin.display()),
    )
    .await?;
    // 画一帧（几何靠渲染带出来），再取 set_status 里的诊断
    let area = app.compositor.area();
    render_rows(&mut app, area);
    pump(&mut app, ":g-diag<ret>").await?;
    let status = status_text(&app);
    assert!(status.contains("groups:"), "诊断没跑起来: {status:?}");
    assert!(!status.contains("groups:0"), "组没进 pane.list: {status:?}");
    assert!(
        status.contains("children:"),
        "组条目缺 children: {status:?}"
    );
    // 底部那个窗口的宽度 == 最宽窗口的宽度（即整宽）
    let field = |key: &str| -> i64 {
        status
            .split(' ')
            .find_map(|kv| kv.strip_prefix(key))
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(-1)
    };
    assert_eq!(
        field("bottom_w:"),
        field("widest:"),
        "at:\"root\" 的窗口必须横跨整宽: {status:?}"
    );
    Ok(())
}

/// `helix.pane.move_into`：把已经切在半宽里的 pane 搬到整窗底边（事后修形）。
const MOVE_INTO_JS: &str = r#"
let narrow = 0;
helix.register_command("m-setup", () => {
    // ed1 | (ed2 / x)：x 只有右半宽的一半
    helix.split("right", { panel: { render: () => ["p1"], size: 30 } });
    narrow = helix.split("bottom", { panel: { render: () => ["p2"], size: 6 } });
});
helix.register_command("m-diag", () => {
    const panes = helix.pane.list().filter((p) => p.kind !== "group");
    const self = panes.find((p) => p.id === narrow);
    const widest = panes.reduce((m, p) => Math.max(m, p.area.w), 0);
    helix.echo("narrow:" + (self ? self.area.w : -1) + "," + widest);
});
helix.register_command("m-lift", () => {
    helix.pane.move_into(narrow, { at: "root", dir: "bottom", size: 8 });
});
"#;

#[tokio::test(flavor = "multi_thread")]
async fn js_pane_move_into_lifts_nested_pane_to_full_width() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("move_into.js");
    std::fs::write(&plugin, MOVE_INTO_JS)?;
    let mut app = AppBuilder::new().build()?;
    pump(
        &mut app,
        &format!(":plugin-load {}<ret>:m-setup<ret>", plugin.display()),
    )
    .await?;
    let area = app.compositor.area();
    render_rows(&mut app, area);
    pump(&mut app, ":m-diag<ret>").await?;
    let before = status_text(&app);
    let (narrow_before, widest) = parse_pair(&before);
    assert!(
        narrow_before < widest,
        "前置：它本来是窄的（{narrow_before} < {widest}）: {before:?}"
    );

    pump(&mut app, ":m-lift<ret>").await?;
    render_rows(&mut app, area);
    pump(&mut app, ":m-diag<ret>").await?;
    let after = status_text(&app);
    let (narrow_after, widest_after) = parse_pair(&after);
    assert_eq!(
        narrow_after, widest_after,
        "搬到整窗底边后必须横跨整宽: {after:?}"
    );
    Ok(())
}

/// 解析 "narrow:12,120" → (12, 120)
fn parse_pair(s: &str) -> (u16, u16) {
    let nums: Vec<u16> = s
        .trim_start_matches("narrow:")
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    assert_eq!(nums.len(), 2, "诊断输出格式不对: {s:?}");
    (nums[0], nums[1])
}

/// `helix.open_panel({side:"bottom", rail:false, at:"root"})`：面板也认 `at`，
/// 跨满整宽（而不是在焦点那个窗口里再裂一刀）。
#[tokio::test(flavor = "multi_thread")]
async fn js_open_panel_at_root_spans_full_width() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("panel_root.js");
    std::fs::write(
        &plugin,
        r#"
helix.register_command("p-root", () => {
    helix.split("right", { panel: { render: () => ["ed2"], size: 40 } });
    helix.open_panel({ side: "bottom", size: 6, at: "root", render: () => ["bottom-panel"] });
});
helix.register_command("p-diag", () => {
    const panes = helix.pane.list().filter((p) => p.kind !== "group");
    const panel = panes.find((p) => p.kind === "PluginPanel");
    const widest = panes.reduce((m, p) => Math.max(m, p.area.w), 0);
    helix.echo("panel:" + (panel ? panel.area.w : -1) + "," + widest);
});
"#,
    )?;
    let mut app = AppBuilder::new().build()?;
    pump(
        &mut app,
        &format!(":plugin-load {}<ret>:p-root<ret>", plugin.display()),
    )
    .await?;
    let area = app.compositor.area();
    render_rows(&mut app, area);
    pump(&mut app, ":p-diag<ret>").await?;
    let s = status_text(&app);
    let nums: Vec<u16> = s
        .trim_start_matches("panel:")
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    assert_eq!(nums.len(), 2, "诊断输出格式不对: {s:?}");
    assert_eq!(nums[0], nums[1], "at:\"root\" 的面板必须横跨整宽: {s:?}");
    Ok(())
}
