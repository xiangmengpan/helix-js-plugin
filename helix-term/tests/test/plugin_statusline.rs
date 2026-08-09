use super::*;

use helix_view::current_ref;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_statusline_renders() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("st.txt");
    std::fs::write(&file, "data\n")?;
    let plugin_path = dir.path().join("statusline.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.set_statusline((ctx) => "PLUGIN|" + ctx.mode + "|" + ctx.cursor.row);
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some("i"), // 进入 insert 模式，触发重渲染
                Some(&|app| {
                    // 构造 RenderContext 渲染状态栏到独立 surface（仿 bufferline 集成测试）
                    let (view, doc) = current_ref!(app.editor);
                    let area = helix_view::graphics::Rect::new(0, 0, 200, 1);
                    let mut buf = tui::buffer::Buffer::empty(area);
                    let spinners = helix_term::ui::ProgressSpinners::default();
                    let mut rc = helix_term::ui::statusline::RenderContext::new(
                        &app.editor,
                        doc,
                        view,
                        true,
                        &spinners,
                    );
                    helix_term::ui::statusline::render(&mut rc, area, &mut buf);
                    let rendered: String = buf.content.iter().map(|c| c.symbol.as_str()).collect();
                    assert!(
                        rendered.contains("PLUGIN|insert|0"),
                        "statusline missing plugin text: {rendered:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
