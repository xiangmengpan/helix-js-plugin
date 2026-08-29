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
async fn plugin_popup_size_smoke() -> anyhow::Result<()> {
    // 冒烟：open_popup 的 width/height/position 选项不能使弹窗打开失败
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("sz.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("sizepopup.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("size-popup", () => {
            helix.open_popup({ render: () => ["line1", "line2", "line3"], width: 20, height: 4, position: { row: 2, col: 3 } });
        });
        "#,
    )?;

    let popup_type = std::any::type_name::<ui::Popup<ui::PluginPopup>>();

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":size-popup<ret>"),
                Some(&|app| {
                    assert!(
                        app.compositor.has_component(popup_type),
                        "popup open with size/position options"
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
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
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

#[tokio::test(flavor = "multi_thread")]
async fn plugin_events() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("ev.txt");
    std::fs::write(&file, "data\n")?;
    let file2 = dir.path().join("ev2.txt");
    std::fs::write(&file2, "two\n")?;
    let plugin_path = dir.path().join("events.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.on("save", (doc) => { doc.insert(0, 0, "pre-"); });
        helix.on("mode-change", (mode) => { helix.echo("mode:" + mode); });
        "#,
    )?;
    // buffer-open 处理器放第二个脚本：避开启动时序，用 :open 显式触发
    let open_plugin_path = dir.path().join("open_events.js");
    std::fs::write(
        &open_plugin_path,
        r#"
        helix.on("buffer-open", (doc) => { helix.echo("opened:" + doc.path); });
        "#,
    )?;
    // 抛错的 save 处理器：验证钩子失败不阻断保存
    let bad_plugin_path = dir.path().join("bad_events.js");
    std::fs::write(
        &bad_plugin_path,
        r#"
        helix.on("save", () => { throw new Error("boom"); });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file.clone(), None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            // save 钩子：编辑先应用再保存 → 磁盘与缓冲区都是 "pre-data\n"
            (
                Some(":w<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "pre-data\n");
                    let on_disk = std::fs::read_to_string(&file).unwrap();
                    assert_eq!(on_disk, "pre-data\n");
                }),
            ),
            // mode-change 钩子：进入 insert 模式 → 状态栏 "mode:insert"
            (
                Some("i"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "mode:insert");
                }),
            ),
            // buffer-open 钩子：第二个脚本注册处理器，:open 触发。
            // 注意先 <esc> 退出 insert 模式，否则后续命令会被输入进缓冲区。
            (Some("<esc>"), None),
            (
                Some(&format!(":plugin-load {}<ret>", open_plugin_path.display())),
                None,
            ),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), format!("opened:{}", file2.display()));
                }),
            ),
            // 钩子抛错不阻断保存：save 事件 Err → 编辑队列被丢弃、保存照常完成。
            // （保存成功会覆盖状态栏，故不查 severity；查 doc 无 pre- 插入即可证明
            // emit_event 走了 Err 早退路径，磁盘内容证明保存未被阻断）
            (
                Some(&format!(":plugin-load {}<ret>", bad_plugin_path.display())),
                None,
            ),
            (
                Some(":w<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(
                        doc.text().to_string(),
                        "two\n",
                        "throw handler should abort edit application"
                    );
                    let on_disk = std::fs::read_to_string(&file2).unwrap();
                    assert_eq!(on_disk, "two\n", "save must not be blocked by hook error");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_keymap() -> anyhow::Result<()> {
    // 插件键位注入：helix.map 注册的键在 :plugin-load 后立即可用；
    // 字符串命令走 MappableCommand 回退（execute → run_plugin_command），
    // 回调注册为 __mapped_N 隐藏命令，同一机制。
    // 两行文件 + 光标断言：j 覆盖内置 move_line_down，未覆盖时光标会下移——
    // 单行文件下 move_line_down 是无操作，状态栏又残留上一次的 "hello"，无法区分。
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("km.txt");
    std::fs::write(&file, "hello\nworld\n")?;
    let plugin_path = dir.path().join("keys.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("echo-hello", () => { helix.echo("hello"); });
        helix.map("normal", "X", "echo-hello");
        helix.map("normal", "j", "echo-hello");   // 覆盖内置 j（下移）
        helix.map("normal", "Y", () => { helix.echo("cb"); });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some("X"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "hello");
                }),
            ),
            (
                Some("j"),
                Some(&|app| {
                    let (view, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "hello\nworld\n");
                    let sel = doc.selection(view.id).primary();
                    let pos = sel.cursor(doc.text().slice(..));
                    assert_eq!(pos, 0, "j must be overridden, cursor should not move");
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "hello");
                }),
            ),
            (
                Some("Y"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "cb");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
