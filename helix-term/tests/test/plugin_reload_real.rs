use super::*;

// 真实用户环境渲染复现:加载用户 init.js + :filetree 打开面板 + 渲染,检查 render failed。
#[tokio::test(flavor = "multi_thread")]
async fn reload_user_env_filetree_render() -> anyhow::Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let config_dir = std::path::PathBuf::from(&home).join(".config/helix");
    let init_path = config_dir.join("init.js");
    if !init_path.is_file() {
        return Ok(());
    }
    // 手动设置插件目录 + 加载用户 init.js(直接调 helix-js,不经 AppBuilder)
    helix_js::set_plugins_dir(config_dir.join("plugins"));
    let init_src = std::fs::read_to_string(&init_path)?;
    helix_js::load_script_named(&init_path.display().to_string(), &init_src)
        .map_err(|e| anyhow::anyhow!("init load: {e}"))?;
    let _ = helix_js::take_messages();
    let _ = helix_js::take_ui_requests();

    // 先 reload(用户场景:reload 后才坏)
    let reload_err = helix_js::reload_all().err().map(|e| e.to_string());
    let _ = helix_js::take_messages();
    let _ = helix_js::take_ui_requests();
    eprintln!("reload_err: {reload_err:?}");
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.rs");
    std::fs::write(&file, "fn main() {}\n")?;
    // 通过事件循环执行 :filetree(渲染面板)
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(":filetree<ret>"),
                Some(&|app| {
                    // 渲染后检查:面板不应显示 render failed
                    let status = app.editor.get_status();
                    if let Some((s, _)) = &status {
                        eprintln!("status: {s}");
                        assert!(
                            !s.contains("render failed") && !s.contains("call stack"),
                            "面板渲染不应失败, got: {s}"
                        );
                    }
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
