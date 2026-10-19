use super::*;

use helix_term::job::Jobs;

#[cfg(windows)]
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
    (n[0] as u64, n[1] as u16, n[2] as u16, n[3] as u16, n[4] as u16)
}

async fn pane_area_app() -> anyhow::Result<(tempfile::TempDir, helix_term::application::Application)> {
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
