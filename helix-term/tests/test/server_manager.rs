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
    std::env::set_var("SM_PATH", ""); // 禁用宿主 PATH 本地检测(hermetic)

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
                        status.as_ref().contains("受管/")
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
    std::env::set_var("SM_PATH", ""); // 禁用宿主 PATH 本地检测(hermetic)
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
            (Some("q"), None),
            (
                Some(":server status<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        !app.editor.is_err() && status.as_ref().contains("受管/"),
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
    std::env::set_var("SM_PATH", ""); // 禁用宿主 PATH 本地检测(hermetic)

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
// 本地已装识别:install 短路(直接用)、remove 拒绝、unmanage 停挂接(toggle)。
#[tokio::test(flavor = "multi_thread")]
async fn server_local_short_circuit_and_unmanage() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "x\n")?;
    std::env::set_var("SM_MANAGED_DIR", dir.path().join("managed"));
    std::env::set_var("SM_LANGS_TOML", dir.path().join("languages.toml"));
    // PATH 里放一个假 rust-analyzer → 视为本地已装
    let bins = dir.path().join("localbin");
    std::fs::create_dir_all(&bins)?;
    let ra = bins.join("rust-analyzer");
    std::fs::write(&ra, "#!/bin/sh\necho rust-analyzer 9.9\n")?;
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&ra, std::fs::Permissions::from_mode(0o755))?;
    }
    std::env::set_var("SM_PATH", bins.to_string_lossy().to_string());

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(":server install rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        !app.editor.is_err() && status.as_ref().contains("本地已可用"),
                        "本地已装应短路安装, got: {status}"
                    );
                }),
            ),
            (
                Some(":server remove rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        app.editor.is_err() && status.as_ref().contains("不能卸载"),
                        "本地工具 remove 应拒绝, got: {status}"
                    );
                }),
            ),
            (
                Some(":server unmanage rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        !app.editor.is_err() && status.as_ref().contains("已停用"),
                        "unmanage 停用挂接, got: {status}"
                    );
                }),
            ),
            (
                Some(":server unmanage rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        !app.editor.is_err() && status.as_ref().contains("已恢复"),
                        "再 unmanage 恢复, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// 写 file:// 假 release:tar.gz(顶层 demo-bin-1.0.0/ + 可执行 bin demo-bin)到 path
fn write_fake_tar_gz(path: &std::path::Path, bin_name: &str, version: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let file = std::fs::File::create(path)?;
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut ar = tar::Builder::new(enc);
    // 顶层目录 {bin}-{version}/
    let top = format!("{bin_name}-{version}");
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Directory);
    h.set_mode(0o755);
    h.set_size(0);
    ar.append_data(
        &mut h,
        format!("{top}/"),
        std::io::Cursor::new(Vec::<u8>::new()),
    )?;
    // bin 脚本(0755)
    let script = format!("#!/bin/sh\necho '{bin_name} {version}'\n");
    let mut hb = tar::Header::new_gnu();
    hb.set_size(script.len() as u64);
    hb.set_mode(0o755);
    ar.append_data(&mut hb, format!("{top}/{bin_name}"), script.as_bytes())?;
    let enc = ar.into_inner()?;
    let f = enc.finish()?;
    f.sync_all()?;
    drop(f);
    Ok(())
}

// Arsenal M2 集成:file:// 假源经 helix.server.task 后台任务全流程安装。
// 注入:SM_SERVER_CONFIG 指到临时 config.toml([server-manager.registry.demo-bin] 配方),
// SM_MANAGED_DIR/SM_LANGS_TOML 隔离产物,SM_PATH 禁用宿主 PATH。JS 回调把任务事件
// echo 成 "ev:kind:name:bytes",经编辑器状态栏断言 done 到达 + managed/bin 落盘 +
// languages.toml 收敛(受管 [language-server.demo-bin] 指向受管绝对路径)。
#[tokio::test(flavor = "multi_thread")]
async fn server_arsenal_task_install_background_file_source() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("arsenal.txt");
    std::fs::write(&file, "x\n")?;
    let managed = dir.path().join("managed");
    let langs = dir.path().join("languages.toml");
    std::env::set_var("SM_MANAGED_DIR", &managed);
    std::env::set_var("SM_LANGS_TOML", &langs);
    std::env::set_var("SM_PATH", ""); // 禁用宿主 PATH 本地检测(hermetic)

    // 假 release:file://{dir}/rel/1.0.0/demo.tar.gz(顶层 demo-bin-1.0.0/,strip=1 → demo-bin)
    let rel_dir = dir.path().join("rel").join("1.0.0");
    write_fake_tar_gz(&rel_dir.join("demo.tar.gz"), "demo-bin", "1.0.0")?;

    // 配方经 SM_SERVER_CONFIG 注入(与 SM_MANAGED_DIR 同哲学:env 覆写磁盘 config 读取源)
    let cfg = dir.path().join("sm-config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[server-manager]\n\n[server-manager.registry.demo-bin]\nurl = \"file://{}/rel/{{version}}/demo.tar.gz\"\nversion = \"1.0.0\"\nstrip = 1\nbin = \"demo-bin\"\nlanguages = [\"demo\"]\n",
            dir.path().display()
        ),
    )?;
    std::env::set_var("SM_SERVER_CONFIG", &cfg);

    // 驱动 JS:helix.server.task 提交后台 install,回调把事件汇聚进脚本级数组;
    // :arsenal-e2e-dump 一次性 echo 全序列(状态栏每泵会覆写,跨泵断言不靠单步截屏)。
    let plugin_path = dir.path().join("arsenal-e2e.js");
    std::fs::write(
        &plugin_path,
        r#"
        const evs = [];
        helix.register_command("arsenal-e2e", () => {
            helix.server.task(
                [{ op: "install", name: "demo-bin", version: "1.0.0" }],
                (ev) => { evs.push("ev:" + ev.kind + ":" + ev.name + ":" + (ev.bytes || 0)); }
            );
        });
        helix.register_command("arsenal-e2e-dump", () => {
            helix.echo("seq:" + evs.join(" "));
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
            // 提交后台任务:worker 异步执行;file:// 假源毫秒级,事件在后续 idle 泵回投
            (
                Some(":arsenal-e2e<ret>"),
                Some(&|app| {
                    assert!(
                        !app.editor.is_err(),
                        "任务提交不应报错, got: {:?}",
                        app.editor.get_status()
                    );
                }),
            ),
            // 无害键步:每步一个 idle 泵窗口,保证 worker 事件(phase/progress/done)全部 resolve
            (Some("j"), None),
            (Some("j"), None),
            (Some("j"), None),
            // 收敛断言:事件序列(JS 侧累积,不受状态栏覆写影响)
            (
                Some(":arsenal-e2e-dump<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    let seq = status.as_ref();
                    assert!(!app.editor.is_err(), "后台安装不应报错, got: {seq}");
                    assert!(
                        seq.contains("seq:ev:phase:demo-bin")
                            && seq.contains("ev:progress:demo-bin:")
                            && seq.contains("ev:done:demo-bin"),
                        "事件序列应含 phase/progress/done, got: {seq}"
                    );
                    assert!(
                        seq.find("ev:phase:demo-bin").unwrap_or(0)
                            < seq.find("ev:done:demo-bin").unwrap_or(usize::MAX),
                        "phase 应先于 done, got: {seq}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    // 状态收敛:受管 bin 落盘(软链)→ 目标存在;languages.toml 出现受管 [language-server.demo-bin]
    let bin = managed.join("bin").join("demo-bin");
    assert!(
        bin.exists(),
        "managed/bin/demo-bin 应已安装: {}",
        bin.display()
    );
    let langs_text = std::fs::read_to_string(&langs)
        .map_err(|e| anyhow::anyhow!("读 languages.toml: {e}"))?;
    assert!(
        langs_text.contains("[language-server.demo-bin]"),
        "languages.toml 应有受管 language-server 段, got: {langs_text}"
    );
    assert!(
        langs_text.contains(&managed.join("bin").join("demo-bin").display().to_string()),
        "command 应指向受管绝对路径, got: {langs_text}"
    );
    // env 是进程级:清掉 SM_SERVER_CONFIG,避免后续测试(同进程)读到已删临时 config
    std::env::remove_var("SM_SERVER_CONFIG");
    Ok(())
}
