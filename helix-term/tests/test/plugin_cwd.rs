//! `cwd-change` 端到端:`:cd` 真的 chdir 之后,插件**从事件里知道**并**现取**到新目录。
//!
//! 回归背景:改之前 `:cd` 只有 core 侧生效(进程 chdir + `CWD` 缓存),helix-js 侧
//! 既无 getter 也无事件 ⇒ 插件只能启动时 `helix.run("pwd")` 缓存一次,
//! `:cd` 之后面板永远停在启动目录(内建 filetree 就是这么坏的)。
//!
//! ⚠️ cwd 是**进程级**状态,而本 crate 的集成测试并行跑(另有
//! `plugin_cross_buffer::plugin_by_path_relative_resolves_from_cwd` 也会 `:cd`)⇒
//! 这里的断言只依赖**本测试自己 `:cd` 出来的值**(事件里的 `ev.cwd`),不依赖
//! "此刻环境的 cwd 是谁"(那会被并行测试改写)。

use super::*;

use helix_term::application::Application;

/// 左 32 列(filetree 面板区)内容拼接
fn panel_text(app: &mut Application) -> String {
    render_rows(app, helix_view::graphics::Rect::new(0, 0, 120, 30))
        .iter()
        .map(|r| r.chars().take(32).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `:cd` → `cwd-change` 处理器拿到新目录,且同一时刻 `helix.cwd()` 已是新值。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_cwd_change_event() -> anyhow::Result<()> {
    // 会真 chdir ⇒ 与其它 chdir 的用例串行(见 helpers::CWD_LOCK)
    let _cwd_lock = CWD_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let sub = dir.path().join("sub");
    std::fs::create_dir_all(&sub)?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("cwd.js");
    std::fs::write(
        &plugin_path,
        r#"helix.on("cwd-change", (doc, ev) => {
            // 事件里的新值 == 此刻 helix.cwd() 现取到的值(证明"事件在 chdir 之后发")
            helix.echo("ev=" + ev.cwd + " get=" + helix.cwd() + " prev=" + ev.prev_cwd);
        });"#,
    )?;

    let expected_new = helix_stdx::path::canonicalize(&sub)
        .to_string_lossy()
        .to_string();

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(&format!(":cd {}<ret>", sub.display())),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    let status = status.as_ref();
                    // 处理器 echo 的是最后一条消息 ⇒ 会覆盖 `:cd` 自己的状态栏提示
                    assert!(
                        status.contains(&format!("ev={expected_new}")),
                        "事件 cwd 不对: {status}"
                    );
                    assert!(
                        status.contains(&format!("get={expected_new}")),
                        "helix.cwd() 在处理器里必须已是新目录: {status}"
                    );
                    // prev 是 chdir **之前**的值 —— 具体是谁由并行测试决定,只校验
                    // "有值且不等于新值"(证明它确实是变更前取的)
                    assert!(
                        status.contains("prev=")
                            && !status.contains(&format!("prev={expected_new}")),
                        "prev_cwd 应是变更前的目录: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// 真实 filetree 插件跟 `:cd` 换根 —— 用户报的那个场景的端到端回归。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_panel_follows_cd() -> anyhow::Result<()> {
    let _cwd_lock = CWD_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let b = dir.path().join("b");
    std::fs::create_dir_all(&b)?;
    std::fs::write(b.join("only_in_b.txt"), "b\n")?;

    // 读**仓库**里的插件(不是开发机 ~/.config)
    let src = std::fs::read_to_string(repo_plugin("filetree/plugin.js"))?;
    let plugin = dir.path().join("filetree.js");
    std::fs::write(&plugin, src)?;

    let _cwd = CwdGuard::enter(dir.path());
    let mut app = AppBuilder::new().build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    // 面板**先开**(根 = 打开时的 cwd,与本断言无关),再 :cd —— 处理器用 ev.cwd
    // 换根,所以断言只依赖本测试给出的绝对路径 b
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, &format!(":cd {}<ret>", b.display())).await?;

    let text = panel_text(&mut app);
    assert!(
        text.contains("only_in_b.txt"),
        "面板必须跟着 :cd 换根(回归:此前永远停在启动目录): {text:?}"
    );
    Ok(())
}
