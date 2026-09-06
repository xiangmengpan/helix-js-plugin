use super::*;

// :server 命令族——冒烟验证(list/search/status + 惰性配方的负路径 install)。
// 环境隔离:SM_MANAGED_DIR/SM_LANGS_TOML 指到临时目录,不碰真实 ~/.local/share。
#[tokio::test(flavor = "multi_thread")]
async fn server_list_search_status() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    std::env::set_var("SM_MANAGED_DIR", dir.path().join("managed"));
    std::env::set_var("SM_LANGS_TOML", dir.path().join("languages.toml"));

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(":server list<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(!app.editor.is_err(), "server list 不应是错误: {status}");
                    assert!(
                        status.as_ref().contains("servers[")
                            && status.as_ref().contains("rust-analyzer"),
                        "server list 应含注册表与内置配方, got: {status}"
                    );
                }),
            ),
            (
                Some(":server search rust<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("rust-analyzer"),
                        "search 命中 rust-analyzer, got: {status}"
                    );
                }),
            ),
            (
                Some(":server status<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("已装/")
                            && status.as_ref().contains("managed dir"),
                        "status 应报统计与目录, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 惰性配方(内置)无下载源:install 报错提示配 config 源;未安装的 remove 报错;缺参给 usage。
#[tokio::test(flavor = "multi_thread")]
async fn server_negative_paths() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("b.txt");
    std::fs::write(&file, "x\n")?;
    std::env::set_var("SM_MANAGED_DIR", dir.path().join("managed"));
    std::env::set_var("SM_LANGS_TOML", dir.path().join("languages.toml"));

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(":server install rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        app.editor.is_err(),
                        "内置惰性配方 install 应报错, got: {status}"
                    );
                    assert!(
                        status.as_ref().contains("下载源未配置"),
                        "提示配置源, got: {status}"
                    );
                }),
            ),
            (
                Some(":server remove rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        app.editor.is_err() && status.as_ref().contains("未安装"),
                        "未装则 remove 报错, got: {status}"
                    );
                }),
            ),
            (
                Some(":server install<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("usage: server"),
                        "缺参给 usage, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
