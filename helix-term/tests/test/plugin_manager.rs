use super::*;

// :plugin 命令族——list/status 集成验证（不碰真实 plugins 目录：install/remove 有单测覆盖路径校验）。
// config_dir 是真实 ~/.config，只测无副作用路径。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_manager_list_status() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("r.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("list-demo.js");
    std::fs::write(
        &plugin_path,
        r#"helix.register_command("list-demo", () => {});"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":plugin list<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains(plugin_path.to_str().unwrap()),
                        ":plugin list should show loaded plugin path, got: {status}"
                    );
                }),
            ),
            (
                Some(":plugin status<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("loaded"),
                        ":plugin status should report loaded count, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

// status 的 manifest 计数后缀（"N installed in manifest"）：无 manifest 时也应显示 0 计数。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_list_and_status() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(":plugin list<ret>"), None),
            (
                Some(":plugin status<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    // status 含 manifest 计数后缀("installed in manifest")
                    assert!(
                        status.as_ref().contains("installed"),
                        ":plugin status should carry manifest count, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// git-url 安装走真实 git clone(需要网络+git,手动验证);无副作用路径:
// 无法识别的参数(非路径非 url)→ 报错不崩。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_install_invalid_arg_reports() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![(
            Some(":plugin install plain-name<ret>"),
            Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert!(
                    status.as_ref().contains("invalid"),
                    "expected invalid arg error, got: {status}"
                );
            }),
        )],
        false,
    )
    .await?;
    Ok(())
}

// pin/unpin 未安装 → 报错(无副作用路径)
#[tokio::test(flavor = "multi_thread")]
async fn plugin_pin_uninstalled_reports() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(":plugin pin nope<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("not installed"),
                        "expected not installed, got: {status}"
                    );
                }),
            ),
            (
                Some(":plugin unpin nope<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("not installed"),
                        "expected not installed, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
