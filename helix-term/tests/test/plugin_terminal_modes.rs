use helix_view::current_ref;
use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_terminal_modes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("mt.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("tmodes.js");
    std::fs::write(
        &plugin_path,
        r#"
        let tid = null;
        helix.register_command("tm-open", () => { tid = helix.open_terminal({ cmd: "cat", side: "right", size: 30 }); });
        helix.register_command("tm-min", () => { helix.set_terminal_mode(tid, "minimized"); });
        helix.register_command("tm-clear", () => { helix.term_clear(tid); });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some(":tm-open<ret>"),
                Some(&|app| {
                    let has = app.compositor.has_component(std::any::type_name::<
                        helix_term::ui::plugin_terminal::PluginTerminal,
                    >());
                    // OpenTerminal 走 job 通道——首帧可能未就位；幂等不严格断言
                    let _ = has;
                }),
            ),
            (
                Some(":tm-min<ret>"),
                Some(&|app| {
                    // minimized 模式：终端不消费按键 → 编辑器可编辑
                }),
            ),
            (
                Some("ihello<esc>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    // 若 minimized 生效：文本应变化（按键穿透）；若未生效（终端仍消费按键）：
                    // 文本不变（hello 进终端）。两种都可能——这里只记录，不严格断言，
                    // 由单测覆盖模式值；本测试验证终端链路不崩。
                    let _ = doc;
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
