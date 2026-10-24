use super::*;

use std::time::Duration;

use helix_term::application::Application;
use helix_view::current_ref;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};

/// 左 32 列（面板区）内容拼接
fn panel_text(app: &mut Application) -> String {
    render_rows(app, helix_view::graphics::Rect::new(0, 0, 120, 30))
        .iter()
        .map(|r| r.chars().take(32).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 建临时目录结构 + 加载真实插件(读仓库 `plugins/filetree/plugin.js`)
async fn setup() -> anyhow::Result<(tempfile::TempDir, Application)> {
    // 配置合并缓存是进程级的:插件加载时会读 [plugins.filetree],别的用例写过的覆盖会漏进来
    let _cfg = plugin_config_lock().await;
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("src"))?;
    std::fs::write(dir.path().join("src/main.js"), "console.log(1)\n")?;
    std::fs::write(dir.path().join("readme.md"), "hi\n")?;
    std::fs::write(dir.path().join(".secret.txt"), "s\n")?;
    // 读**仓库**里的插件(不是开发机 `~/.config`:那在 CI/新机器上不存在)
    let src = std::fs::read_to_string(repo_plugin("filetree/plugin.js"))?;
    let plugin = dir.path().join("filetree.js");
    std::fs::write(&plugin, src)?;
    let mut app = AppBuilder::new().build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    Ok((dir, app))
}

/// 核心路径：reveal 定位 → 展开目录 → 打开文件（buffer 切换）
#[tokio::test(flavor = "multi_thread")]
async fn filetree_reveal_expand_open() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let main_js = dir.path().join("src/main.js");
    let readme = dir.path().join("readme.md");

    // 当前文件在树 root（cwd=helix-term）外 → reveal 应重设 root 到 readme 所在目录
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    let text = panel_text(&mut app);
    assert!(
        text.contains("readme.md"),
        "reveal 后面板渲染含 readme.md: {text:?}"
    );
    assert!(text.contains("src"), "root 重设后显示 src 目录: {text:?}");

    // reveal 定位 readme.md（索引 2，排序 [src, filetree.js, readme.md]）→
    // Up×2 到 src（目录优先）→ Enter 展开 → Down 到 main.js → Enter 打开
    pump(&mut app, "<up><up>").await?;
    pump(&mut app, "<ret>").await?;
    pump(&mut app, "<down>").await?;
    pump(&mut app, "<ret>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.path()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
        main_js.to_str(),
        "Enter 应打开 src/main.js"
    );
    Ok(())
}

/// 隐藏文件切换：默认隐藏 . 文件，H 后显示
#[tokio::test(flavor = "multi_thread")]
async fn filetree_hidden_toggle() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    assert!(
        !panel_text(&mut app).contains(".secret.txt"),
        "默认隐藏 dotfile"
    );
    pump(&mut app, "H").await?;
    assert!(
        panel_text(&mut app).contains(".secret.txt"),
        "H 切换后显示 dotfile"
    );
    Ok(())
}

/// 新建文件：a → 输入弹窗 → Tab 聚焦 → 输入名字 → Enter → 异步 touch 生效
#[tokio::test(flavor = "multi_thread")]
async fn filetree_new_file_prompt() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;

    let popup_type = std::any::type_name::<helix_term::ui::Popup<helix_term::ui::PluginPopup>>();
    pump(&mut app, "a").await?;
    assert!(app.compositor.has_component(popup_type), "a → 输入弹窗打开");

    pump(&mut app, "note.txt<ret>").await?;
    assert!(
        !app.compositor.has_component(popup_type),
        "Enter 提交后弹窗关闭"
    );
    for _ in 0..50 {
        if dir.path().join("note.txt").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dir.path().join("note.txt").exists(), "note.txt 已创建");
    Ok(())
}

/// 关闭面板：q 键
#[tokio::test(flavor = "multi_thread")]
async fn filetree_close_panel() -> anyhow::Result<()> {
    let (_, mut app) = setup().await?;
    pump(&mut app, ":filetree<ret>").await?;
    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    assert!(app.compositor.has_component(panel_type), "面板打开");
    pump(&mut app, "q").await?;
    assert!(!app.compositor.has_component(panel_type), "q 关闭面板");
    Ok(())
}

/// 面板聚焦时 `C-*` 必须**穿透**给编辑器 —— 面板是非模态的（`open_panel`，不是 popup）。
///
/// 回归：filetree 的 `onKey` 曾对 ctrl/alt 返回 `"handled"`（注释却写着"穿透"）⇒
/// 用户在 `[keys]` / `helix.map` 里显式绑的 C-* 在面板聚焦时**静默失效**。
/// 这里用 `helix.map` 绑一个 C-e 探针，断言它确实触发。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_panel_passes_ctrl_keys_through() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    // 编辑器侧的 C-e 绑定（走插件命令回退，与用户 `helix.map` 同一条路）
    let probe = dir.path().join("probe.js");
    std::fs::write(
        &probe,
        r#"
helix.register_command("probe-ctrl", () => helix.echo("ctrl-through"));
helix.map("normal", "C-e", "probe-ctrl");
"#,
    )?;
    pump(&mut app, &format!(":plugin-load {}<ret>", probe.display())).await?;
    // 先打开 `dir` 里的文件再 reveal ⇒ 面板 root 由**文件路径**决定，
    // 而不是进程 cwd（cwd 是全局的，并行跑别的 `:cd` 会把 root 换走 —— 见 D3 spec §9.3）。
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    assert_ne!(
        app.compositor.layout_tree().active(),
        0,
        "前置不成立：filetree 打开后焦点应在面板"
    );

    // 面板聚焦时按 C-e：必须穿透到编辑器侧的绑定（改前被面板吞掉 ⇒ 状态栏为空）
    pump(&mut app, "<C-e>").await?;
    let st = app
        .editor
        .get_status()
        .map(|(s, _)| s.to_string())
        .unwrap_or_default();
    assert_eq!(
        st, "ctrl-through",
        "面板聚焦时 C-e 应穿透给编辑器绑定（被吞 ⇒ 空）: {st:?}"
    );

    // C-\ 不受本次改动影响：保留面板、焦点回编辑器
    pump(&mut app, "<C-\\>").await?;
    assert_eq!(
        app.compositor.layout_tree().active(),
        0,
        "C-\\ 应把焦点还给编辑器"
    );
    let text = panel_text(&mut app);
    assert!(
        text.contains("readme.md"),
        "C-\\ 只是移焦点，面板应仍在（且 root 不该被别人换走）: {text:?}"
    );
    Ok(())
}

/// `[plugins.filetree] show_hidden = true` 真的改变行为（配置 → 插件,2026-10-23 接线）
///
/// 之前这个键只声明不生效（`CFG` 取了没用），写进 config.toml 只会通过校验 ⇒ 静默无效。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_config_show_hidden_applies() -> anyhow::Result<()> {
    // 配置合并缓存是进程级的 ⇒ 与其它写 [plugins.*] 的用例串行
    let _cfg = plugin_config_lock().await;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("readme.md"), "hi\n")?;
    std::fs::write(dir.path().join(".secret.txt"), "s\n")?;
    let plugin = dir.path().join("filetree.js");
    std::fs::write(
        &plugin,
        std::fs::read_to_string(repo_plugin("filetree/plugin.js"))?,
    )?;

    let mut cfg = test_config();
    cfg.plugins = Some(toml::from_str("[filetree]\nshow_hidden = true\n")?);
    let mut app = AppBuilder::new().with_config(cfg).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    pump(
        &mut app,
        &format!(":open {}<ret>", dir.path().join("readme.md").display()),
    )
    .await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    assert!(
        panel_text(&mut app).contains(".secret.txt"),
        "show_hidden=true ⇒ 面板开局就显示 dotfile（无需按 H）"
    );
    Ok(())
}
