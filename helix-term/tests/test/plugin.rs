use super::*;

use helix_core::diagnostic::Severity;
use helix_term::ui;
use helix_view::current_ref;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_load_and_run_command() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("test_plugin.js");
    std::fs::write(
        &path,
        r#"
        helix.register_command("hello", (ctx) => {
            helix.echo("cursor: " + ctx.cursor.row + "," + ctx.cursor.col);
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", path.display())), None),
            (
                Some(":hello<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "cursor: 0,0");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_popup_open_and_close() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("popup_plugin.js");
    std::fs::write(
        &path,
        r#"
        helix.register_command("showbox", () => {
            helix.open_popup({
                render: () => ["╔═ box ═╗", "║  hi   ║", "╚═══════╝"],
                onKey: (key) => key.name === "Esc" ? "close" : "handled",
                onClose: () => helix.echo("box closed"),
            });
        });
        "#,
    )?;

    // 弹窗图层实际类型是 Popup<PluginPopup>，且测试断言回调只能拿到 &Application，
    // 故用 has_component(type_name) 断言存在/消失（find 需要 &mut 且无法降级到内容类型）。
    let popup_type = std::any::type_name::<ui::Popup<ui::PluginPopup>>();

    test_key_sequences(
        &mut AppBuilder::new().build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", path.display())), None),
            (
                Some(":showbox<ret>"),
                Some(&|app| {
                    assert!(
                        app.compositor.has_component(popup_type),
                        "popup layer should be open after :showbox"
                    );
                }),
            ),
            (
                Some("<esc>"),
                Some(&|app| {
                    assert!(
                        !app.compositor.has_component(popup_type),
                        "popup layer should close on Esc"
                    );
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "box closed");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_bufferline_icons() -> anyhow::Result<()> {
    // 验证 bufferline 图标钩子真的渲染进标签栏
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.rs");
    let b = dir.path().join("b.py");
    std::fs::write(&a, "fn main() {}\n")?;
    std::fs::write(&b, "print(1)\n")?;
    let plugin_path = dir.path().join("buf_icons.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.set_buffer_icon((path) => {
            if (!path) return null;
            if (path.endsWith(".rs")) return "🦀";
            if (path.endsWith(".py")) return "🐍";
            return null;
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(a, None)
            .with_file(b, None)
            .build()?,
        vec![
            // 注意：不能单独发 (None, ...) 迭代——无按键时 event_loop_until_idle 会挂起
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                Some(&|app| {
                    let area = helix_view::graphics::Rect::new(0, 0, 120, 1);
                    let mut buf = tui::buffer::Buffer::empty(area);
                    helix_term::ui::EditorView::render_bufferline(&app.editor, area, &mut buf);
                    let rendered: String = buf
                        .content
                        .iter()
                        .map(|cell| cell.symbol.as_str())
                        .collect();
                    assert!(
                        rendered.contains("🦀") && rendered.contains("a.rs"),
                        "bufferline missing rs icon: {rendered:?}"
                    );
                    assert!(
                        rendered.contains("🐍") && rendered.contains("b.py"),
                        "bufferline missing py icon: {rendered:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_edit_document() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("t.txt");
    std::fs::write(&file, "world\n")?;
    let plugin_path = dir.path().join("edit_plugin.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("inshello", (ctx) => {
            ctx.doc.insert(ctx.cursor.row, ctx.cursor.col, "hello ");
        });
        helix.register_command("delfirst", (ctx) => {
            ctx.doc.delete(0, 0, 0, 5);
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some(":inshello<ret>"),
                Some(&|app| {
                    let (view, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "hello world\n");
                    let sel = doc.selection(view.id).primary();
                    let pos = sel.cursor(doc.text().slice(..));
                    assert_eq!(pos, 6, "cursor after insert");
                }),
            ),
            // 整个命令 = 一次撤销
            (
                Some("u"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "world\n");
                }),
            ),
            (
                Some(":delfirst<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    // delete(0,0,0,5) 删 [0,5)="world"（5 字符）→ 余 "\n"
                    assert_eq!(doc.text().to_string(), "\n");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
