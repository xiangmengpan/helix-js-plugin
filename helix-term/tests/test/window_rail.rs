use super::*;

use helix_term::application::Application;
use helix_term::job::Jobs;
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

// open_panel(left) 插件脚本(模拟 filetree):注册 rail
fn rail_plugin(dir: &std::path::Path, side: &str) -> std::path::PathBuf {
    let p = dir.join(format!("rail_{side}.js"));
    std::fs::write(
        &p,
        format!(
            r#"
            helix.register_command("open-rail", () => {{
                helix.open_panel({{ side: "{side}", size: 24, rail: true, render: () => ["row1", "row2"] }});
            }});
            "#
        ),
    )
    .unwrap();
    p
}

#[tokio::test(flavor = "multi_thread")]
async fn open_panel_left_registers_full_height_rail() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "AAA\n")?;
    let p = rail_plugin(dir.path(), "left");
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", p.display())).await?;
    pump(&mut app, ":open-rail<ret>").await?;
    // 2 叶;rail 标记存在;root = Split(rail | main)
    let dump = app.compositor.layout_tree().dump();
    let rail_leafs: Vec<_> = dump.leafs.iter().filter(|l| l.rail).collect();
    assert_eq!(rail_leafs.len(), 1, "恰一条 rail");
    assert_eq!(dump.leafs.len(), 2, "rail + 编辑器");
    // C-w v(main 分裂)→ rail 不被切,仍在 root first,main 内多一叶
    pump(&mut app, "<C-w>v<esc>").await?;
    let dump = app.compositor.layout_tree().dump();
    let rail_leafs: Vec<_> = dump.leafs.iter().filter(|l| l.rail).collect();
    assert_eq!(rail_leafs.len(), 1, "rail 保持单条");
    assert_eq!(dump.leafs.len(), 3, "main 内新增一叶");
    // root first 仍含 rail
    let first_has_rail = dump
        .tree
        .get("first")
        .and_then(|f| f.get("id"))
        .map(|v| v.as_u64() == Some(rail_leafs[0].id));
    assert_eq!(first_has_rail, Some(true), "rail 仍在 root 边缘");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rail_close_via_x_and_resplit() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "AAA\n")?;
    let p = rail_plugin(dir.path(), "right");
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", p.display())).await?;
    pump(&mut app, ":open-rail<ret>").await?;
    let dump = app.compositor.layout_tree().dump();
    assert_eq!(dump.leafs.len(), 2, "右 rail + 编辑器");
    // main 最右(即编辑器)按 C-w l 应聚焦 rail? (T3) —— 本测试只验 x 关 rail 的免疫入口:
    // 对 rail 的 remove 免疫 → x 应走 take_rail 路径(compositor 层,T3 接线),先跳过
    Ok(())
}

// T3 焦点:main 最左叶 C-w h 聚焦 rail;rail 上 Esc(未消费)/C-w l 回 main;v/s 分裂落 main
#[tokio::test(flavor = "multi_thread")]
async fn rail_focus_enter_leave_and_split_stays_in_main() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "AAA\n")?;
    let p = rail_plugin(dir.path(), "left");
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", p.display())).await?;
    pump(&mut app, ":open-rail<ret>").await?;
    let rid = app.compositor.layout_tree().rail_leaf().unwrap();
    // C-w h:main(leaf0) 向左聚焦 → rail
    pump(&mut app, "<C-w>h<esc>").await?;
    assert_eq!(
        app.compositor.layout_tree().active(),
        rid,
        "C-w h 聚焦 rail"
    );
    // rail 聚焦态按普通键 → 被 rail 吞(不透穿编辑器:文本不变)
    pump(&mut app, "ixxx").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.text().to_string(), "AAA\n", "rail 聚焦时普通键不透穿");
    // Esc(插件 rail 脚本无 onKey → 未消费)→ 回 main
    pump(&mut app, "<esc>").await?;
    assert_ne!(app.compositor.layout_tree().active(), rid, "Esc 回 main");
    // 再进 rail,C-w v → 分裂落在 main(rail 不被切)
    pump(&mut app, "<C-w>h<esc>").await?;
    assert_eq!(app.compositor.layout_tree().active(), rid);
    pump(&mut app, "<C-w>v<esc>").await?;
    let dump = app.compositor.layout_tree().dump();
    let rails: Vec<_> = dump.leafs.iter().filter(|x| x.rail).collect();
    assert_eq!(rails.len(), 1, "rail 仍唯一");
    assert!(app.compositor.layout_tree().is_rail(rails[0].id));
    assert_eq!(dump.leafs.len(), 3, "main 分裂成 2 叶 + rail");
    Ok(())
}
