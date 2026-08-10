use super::*;

use helix_view::current_ref;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_popup_edits_document() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("pe.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("snippet.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("snippet", () => {
            const items = ["AAA", "BBB", "CCC"];
            let idx = 0;
            helix.open_popup({
                render: () => items.map((s, i) => (i === idx ? "> " + s : "  " + s)),
                onKey: (key, doc) => {
                    if (key.name === "Down") { idx = Math.min(items.length - 1, idx + 1); return "handled"; }
                    if (key.name === "Up") { idx = Math.max(0, idx - 1); return "handled"; }
                    if (key.name === "Enter") { doc.insert(doc.cursor.row, doc.cursor.col, items[idx]); return "close"; }
                    if (key.name === "Esc") return "close";
                    return "ignore";
                },
            });
        });
        "#,
    )?;

    let popup_type = std::any::type_name::<helix_term::ui::Popup<helix_term::ui::PluginPopup>>();

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some(":snippet<ret>"),
                Some(&|app| {
                    assert!(app.compositor.has_component(popup_type), "popup open");
                }),
            ),
            // Down×2 选中 CCC，Enter 插入 + 关闭
            (
                Some("<down><down><ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "CCChello\n", "snippet inserted at cursor");
                    assert!(!app.compositor.has_component(popup_type), "popup closed after Enter");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
