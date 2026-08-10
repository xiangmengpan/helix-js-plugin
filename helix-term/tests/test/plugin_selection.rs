use super::*;
use helix_view::current_ref;

// 验证 ctx.selection 读取 + set_cursor/set_selection 写入（:upper 用选区，:jumpend 用光标）。
// 本文件保留；integration.rs 的 mod 声明由控制器合并时统一添加。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_selection_upper() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("sel.txt");
    std::fs::write(&file, "hello world\n")?;
    let plugin_path = dir.path().join("sel.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("upper", (ctx) => {
            const a = ctx.selection.anchor, h = ctx.selection.head;
            if (a.row !== h.row) { helix.echo("single-line only"); return; }
            const line = ctx.doc.text.split("\n")[a.row];
            const lo = Math.min(a.col, h.col), hi = Math.max(a.col, h.col);
            const upper = line.slice(lo, hi).toUpperCase();
            ctx.doc.replace(a.row, lo, a.row, hi, upper);
        });
        helix.register_command("jumpend", (ctx) => { helix.set_cursor(0, 11); });
        "#,
    )?;

    // 初始选中 "hello"（#[hello|]# 标记）
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).with_input_text("#[hello|]# world\n").build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some(":upper<ret>"),
                Some(&|app| {
                    let (view, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "HELLO world\n");
                    let sel = doc.selection(view.id).primary();
                    assert_eq!(sel.anchor, 0, "anchor after upper");
                    assert_eq!(sel.head, 5, "head after upper");
                }),
            ),
            (
                Some(":jumpend<ret>"),
                Some(&|app| {
                    let (view, doc) = current_ref!(app.editor);
                    let pos = doc.selection(view.id).primary().cursor(doc.text().slice(..));
                    assert_eq!(pos, 11, "cursor after jumpend");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
