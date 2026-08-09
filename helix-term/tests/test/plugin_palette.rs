use super::*;

use helix_core::diagnostic::Severity;

/// 命令面板数据源包含插件命令：加载插件 → `space ?` 打开命令面板
/// （默认键位：normal 模式 space 前缀树里 "?" => command_palette）→
/// 输入命令名过滤 → 回车执行选中项 → 状态栏出现插件 echo 消息。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_command_in_palette() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("pal.txt");
    std::fs::write(&file, "data\n")?;
    let plugin_path = dir.path().join("pal.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("pal-hello", () => { helix.echo("from-palette"); });
        "#,
    )?;

    // 面板层实际类型是 Overlay<Picker<MappableCommand>>（push 时包了 overlaid），
    // 故用 has_component(type_name) 断言存在（find 无法 downcast 过 Overlay）。
    let palette_layer = std::any::type_name::<
        helix_term::ui::overlay::Overlay<
            helix_term::ui::picker::Picker<
                helix_term::commands::MappableCommand,
                helix_term::keymap::ReverseKeymap,
            >,
        >,
    >();

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some("<space>?"),
                Some(&|app| {
                    assert!(
                        app.compositor.has_component(palette_layer),
                        "command palette should open after `<space>?`"
                    );
                }),
            ),
            (
                Some("pal-hello<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "from-palette");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
