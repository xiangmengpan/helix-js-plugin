use super::*;

// :plugin 命令族——list/status 集成验证（不碰真实 plugins 目录：install/remove 有单测覆盖路径校验）。
// config_dir 是真实 ~/.config，只测无副作用路径。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_manager_list_status() -> anyhow::Result<()> {
    // 集成进程**不执行** application.rs 的启动推入 ⇒ `plugin_roots()` 为空 ⇒
    // `helix.plugins_dir()` 返回 null;**Rust 的 :plugin 应答时无妨,迁到 JS 后必需** ✗
    // (`set_plugin_roots` 是 OnceLock:首设生效,重复调用是 no-op ✓)
    let proot = tempfile::tempdir()?;
    helix_js::set_plugin_roots(vec![proot.path().to_path_buf()]);

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("r.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("list-demo.js");
    std::fs::write(
        &plugin_path,
        r#"helix.register_command("list-demo", () => {});"#,
    )?;

    // 迁移期间:`plugin-js`(JS 侧管理器)需**显式加载** —— 它用临时命令名,
    // 不遮挡 Rust 的 `:plugin`(后者仍在本文件的断言下应答)。
    let mgr = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/plugin/plugin.js");
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", mgr.display())), None),
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
                    // ★ §15 防假绿判据:这条**只有当 JS 在应答时**才成立
                    //   (JS 的 list 总会多输出一行 `installed(N): …`;Rust 的 list 只有一行)
                    //   ⇒ 若插件加载失败、回落 Rust,这里立刻变红 ✗
                    assert!(
                        status.as_ref().contains("installed("),
                        "§15 判据:应含 JS 独有的 `installed(`,实得 {status}"
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
    // 集成进程**不执行** application.rs 的启动推入 ⇒ `plugin_roots()` 为空 ⇒
    // `helix.plugins_dir()` 返回 null;**Rust 的 :plugin 应答时无妨,迁到 JS 后必需** ✗
    // (`set_plugin_roots` 是 OnceLock:首设生效,重复调用是 no-op ✓)
    let proot = tempfile::tempdir()?;
    helix_js::set_plugin_roots(vec![proot.path().to_path_buf()]);

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    // 迁移期间:`plugin-js`(JS 侧管理器)需**显式加载** —— 它用临时命令名,
    // 不遮挡 Rust 的 `:plugin`(后者仍在本文件的断言下应答)。
    let mgr = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/plugin/plugin.js");
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", mgr.display())), None),
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
    // 集成进程**不执行** application.rs 的启动推入 ⇒ `plugin_roots()` 为空 ⇒
    // `helix.plugins_dir()` 返回 null;**Rust 的 :plugin 应答时无妨,迁到 JS 后必需** ✗
    // (`set_plugin_roots` 是 OnceLock:首设生效,重复调用是 no-op ✓)
    let proot = tempfile::tempdir()?;
    helix_js::set_plugin_roots(vec![proot.path().to_path_buf()]);

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    // 迁移期间:`plugin-js`(JS 侧管理器)需**显式加载** —— 它用临时命令名,
    // 不遮挡 Rust 的 `:plugin`(后者仍在本文件的断言下应答)。
    let mgr = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/plugin/plugin.js");
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", mgr.display())), None),
            (
                Some(":plugin install plain-name<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("invalid"),
                        "expected invalid arg error, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// pin/unpin 未安装 → 报错(无副作用路径)
#[tokio::test(flavor = "multi_thread")]
async fn plugin_pin_uninstalled_reports() -> anyhow::Result<()> {
    // 集成进程**不执行** application.rs 的启动推入 ⇒ `plugin_roots()` 为空 ⇒
    // `helix.plugins_dir()` 返回 null;**Rust 的 :plugin 应答时无妨,迁到 JS 后必需** ✗
    // (`set_plugin_roots` 是 OnceLock:首设生效,重复调用是 no-op ✓)
    let proot = tempfile::tempdir()?;
    helix_js::set_plugin_roots(vec![proot.path().to_path_buf()]);

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    // 迁移期间:`plugin-js`(JS 侧管理器)需**显式加载** —— 它用临时命令名,
    // 不遮挡 Rust 的 `:plugin`(后者仍在本文件的断言下应答)。
    let mgr = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/plugin/plugin.js");
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", mgr.display())), None),
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

// JS API:helix.plugin.remove(未安装)→ 报错经 set_error(plugin-load 里调用,drain 执行)
#[tokio::test(flavor = "multi_thread")]
async fn plugin_js_api_remove_uninstalled() -> anyhow::Result<()> {
    // 集成进程**不执行** application.rs 的启动推入 ⇒ `plugin_roots()` 为空 ⇒
    // `helix.plugins_dir()` 返回 null;**Rust 的 :plugin 应答时无妨,迁到 JS 后必需** ✗
    // (`set_plugin_roots` 是 OnceLock:首设生效,重复调用是 no-op ✓)
    let proot = tempfile::tempdir()?;
    helix_js::set_plugin_roots(vec![proot.path().to_path_buf()]);

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin = dir.path().join("api.js");
    std::fs::write(&plugin, r#"
        // 测试夹具:fail-closed 之后,脚本必须声明它用的能力(否则被拒)
        helix.plugin("test-fixture", { deps: [], permissions: { process: true, fs_read: true, fs_write: true } });

helix.plugin.remove("nope-js-api");"#)?;
    // 本用例不靠任何全局锁(原 `PLUGIN_TEST_LOCK` 已删,插件用例现在全并行)
    // ⇒ 单独设插件根:集成进程不执行 application.rs 的启动推入,`plugins_dir()` 会是 null,
    //    JS 侧便早退报"插件目录不可用",断言就看不到预期的 `failed to remove` ✗
    let proot = tempfile::tempdir()?;
    helix_js::set_plugin_roots(vec![proot.path().to_path_buf()]);
    // 迁移期间:`plugin-js`(JS 侧管理器)需**显式加载** —— 它用临时命令名,
    // 不遮挡 Rust 的 `:plugin`(后者仍在本文件的断言下应答)。
    let mgr = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/plugin/plugin.js");
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", mgr.display())), None),
            (
                Some(&format!(":plugin-load {}<ret>", plugin.display())),
                Some(&|app| {
                    // PluginOp 请求在 load 的 drain 里 dispatch,事件循环 idle 前执行 → set_error
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("failed to remove"),
                        "expected remove error, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// ── 本地安装（计划任务 3）：单文件 / 递归目录 / 失败不留孤儿 ──

/// 本机 loader 的 mgr 路径（三条用例共用）
fn mgr_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/plugin/plugin.js")
}

/// 自本机 `plugins/plugin/plugin.js` 载入管理器，并执行一条 `:plugin …`。
async fn run_plugin_op(
    file: std::path::PathBuf,
    op: &str,
    assert: &dyn Fn(&helix_term::application::Application),
) -> anyhow::Result<()> {
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", mgr_path().display())),
                None,
            ),
            (Some(&format!(":{op}<ret>")), Some(assert)),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// 单文件安装：`<src>/hello.js` → `<plugroot>/hello/plugin.js`，manifest kind=local。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_install_single_file_local() -> anyhow::Result<()> {
    let proot = tempfile::tempdir()?;
    helix_js::set_plugin_roots(vec![proot.path().to_path_buf()]);
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let src = dir.path().join("hello.js");
    std::fs::write(&src, "helix.echo('hi');\n")?;

    let proot_path = proot.path().to_path_buf();
    run_plugin_op(file, &format!("plugin install {}", src.display()), &|app| {
        let (status, _) = app.editor.get_status().unwrap();
        assert!(
            status.as_ref().contains("installed"),
            "单文件安装应报 installed，got: {status:?}"
        );
        let dest = proot_path.join("hello/plugin.js");
        assert!(dest.exists(), "单文件未落到 {}: {status:?}", dest.display());
    })
    .await?;
    Ok(())
}

/// 递归目录安装：`<src>/sub/deep.js` 也要落盘（此前只拷一层）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_install_dir_recursive() -> anyhow::Result<()> {
    let proot = tempfile::tempdir()?;
    helix_js::set_plugin_roots(vec![proot.path().to_path_buf()]);
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let src = dir.path().join("tree");
    std::fs::create_dir_all(src.join("sub"))?;
    std::fs::write(src.join("plugin.js"), "helix.echo('t');\n")?;
    std::fs::write(src.join("sub/deep.js"), "helix.echo('d');\n")?;

    let proot_path = proot.path().to_path_buf();
    run_plugin_op(file, &format!("plugin install {}", src.display()), &|app| {
        let (status, _) = app.editor.get_status().unwrap();
        assert!(status.as_ref().contains("installed"), "got: {status:?}");
        assert!(
            proot_path.join("tree/plugin.js").exists(),
            "顶层文件未落盘: {status:?}"
        );
        assert!(
            proot_path.join("tree/sub/deep.js").exists(),
            "嵌套目录未递归: {status:?}"
        );
    })
    .await?;
    Ok(())
}

/// 审查重点 #3：中途失败**不留孤儿**（已写入的文件要回滚，manifest 不记）。
///
/// 造法：目录里放 `plugin.js`（会先被拷）与 `zz-bad.bin`（非法 UTF-8 ⇒ read_file 抛错）；
/// 排序后 plugin.js 先写成功，随后失败 ⇒ 若没有回滚，`<plugroot>/<name>/plugin.js` 会残留。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_install_failure_leaves_no_orphan() -> anyhow::Result<()> {
    let proot = tempfile::tempdir()?;
    helix_js::set_plugin_roots(vec![proot.path().to_path_buf()]);
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let src = dir.path().join("bad");
    std::fs::create_dir_all(&src)?;
    std::fs::write(src.join("plugin.js"), "helix.echo('ok');\n")?;
    std::fs::write(src.join("zz-bad.bin"), [0xff, 0xfe, 0x00, 0x01])?;

    let proot_path = proot.path().to_path_buf();
    run_plugin_op(file, &format!("plugin install {}", src.display()), &|app| {
        let (status, _) = app.editor.get_status().unwrap();
        assert!(
            status.as_ref().contains("install"),
            "失败应有 install 相关报错，got: {status:?}"
        );
        assert!(
            !proot_path.join("bad/plugin.js").exists(),
            "失败后残留了孤儿文件: {status:?}"
        );
        let manifest = std::fs::read_to_string(proot_path.join("manifest.json"))
            .unwrap_or_else(|_| String::from("{}"));
        assert!(
            !manifest.contains("\"bad\""),
            "失败后 manifest 不该记录: {manifest}"
        );
    })
    .await?;
    Ok(())
}
