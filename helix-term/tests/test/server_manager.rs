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

// :server 面板插件冒烟:加载插件 → 开关面板(行数据回传渲染不崩)。
#[tokio::test(flavor = "multi_thread")]
async fn server_manager_panel_toggle() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("c.txt");
    std::fs::write(&file, "x\n")?;
    std::env::set_var("SM_MANAGED_DIR", dir.path().join("managed"));
    std::env::set_var("SM_LANGS_TOML", dir.path().join("languages.toml"));
    let plugin = format!(
        "{}/plugins/features/server-manager/index.js",
        std::env::var("CARGO_MANIFEST_DIR")
            .unwrap()
            .rsplitn(2, '/')
            .nth(1)
            .unwrap_or(".")
    );

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {plugin}<ret>")),
                Some(&|app| {
                    if app.editor.is_err() {
                        let msg = app
                            .editor
                            .get_status()
                            .map(|(s, _)| s.to_string())
                            .unwrap_or_default();
                        panic!("插件加载不应报错, got: {msg}");
                    }
                }),
            ),
            (
                Some(":server-manager<ret>"),
                Some(&|app| {
                    // 面板打开后行数据请求/渲染在后续帧;此处仅确认无错误状态
                    assert!(!app.editor.is_err(), "面板打开不应报错");
                }),
            ),
            // 面板内 Enter 触发 install(惰性/活配方 → 错误或成功提示,面板不崩;异步 op 在后续帧)
            (Some("<ret>"), None),
            // q 关闭面板后编辑器仍响应(下一命令正常执行覆盖错误状态)
            (
                Some("q"),
                None,
            ),
            (
                Some(":server status<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        !app.editor.is_err() && status.as_ref().contains("已装/"),
                        "关闭后 :server status 正常, got: {status}"
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
                Some(":server install clangd<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        app.editor.is_err(),
                        "惰性配方 install 应报错, got: {status}"
                    );
                    assert!(
                        status.as_ref().contains("下载源未配置"),
                        "提示配置源, got: {status}"
                    );
                }),
            ),
            (
                Some(":server install rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        app.editor.is_err(),
                        "活配方缺 version 应报错, got: {status}"
                    );
                    assert!(
                        status.as_ref().contains("version"),
                        "提示补 version(或 config version), got: {status}"
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
