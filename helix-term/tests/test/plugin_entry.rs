use super::*;

/// 统一入口：init.js 里 helix.load 导入 + helix.lazy 懒加载。
/// js_load 相对名解析用 PLUGINS_DIR（Application::new 设的是真实 config plugins 目录，
/// 集成测试无法改目录）→ init.js 内容用绝对路径写 mod.js/heavy.js（js_load 支持绝对路径）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_entry_import_and_lazy() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("e.txt");
    std::fs::write(&file, "x\n")?;
    let mod_path = dir.path().join("mod.js");
    let heavy_path = dir.path().join("heavy.js");
    std::fs::write(
        &mod_path,
        r#"helix.register_command("mod-cmd", () => { helix.echo("mod-ok"); });"#,
    )?;
    std::fs::write(
        &heavy_path,
        r#"helix.register_command("heavy-cmd", () => { helix.echo("heavy-ok"); });"#,
    )?;
    let init = dir.path().join("init.js");
    std::fs::write(
        &init,
        format!(
            r#"helix.load("{}"); helix.lazy("{}", "heavy-cmd");"#,
            mod_path.display(),
            heavy_path.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", init.display())), None),
            (
                Some(":mod-cmd<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "mod-ok");
                }),
            ),
            // 懒加载：首次 heavy-cmd 生效
            (
                Some(":heavy-cmd<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "heavy-ok");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
