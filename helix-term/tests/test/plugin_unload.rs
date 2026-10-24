use super::*;

// `:plugin-unload <name>` —— 只回收该插件的注册,其它插件不受影响。
// 实现:`helix_js::unload_plugin`(按属主回收);`reload_all` 复用同一份清理逻辑。

/// 一个插件被卸掉:它的命令消失、**另一个插件的命令仍然可用**、`onUnload` 跑过、
/// 它绑的键位也解绑(绑定回落到编辑器原生)。
#[tokio::test(flavor = "multi_thread")]
async fn unload_one_plugin_leaves_the_other_working() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("u.txt");
    std::fs::write(&file, "x\n")?;
    let a = dir.path().join("ub-a.js");
    std::fs::write(
        &a,
        r#"helix.plugin("ub-a", { onUnload: () => { helix.echo("bye-a"); } });
helix.register_command("ub-a-cmd", () => { helix.echo("A"); });
helix.map("normal", "C-b", "ub-a-cmd");
"#,
    )?;
    let b = dir.path().join("ub-b.js");
    std::fs::write(
        &b,
        r#"helix.plugin("ub-b", {});
helix.register_command("ub-b-cmd", () => { helix.echo("B"); });
"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", a.display())), None),
            (Some(&format!(":plugin-load {}<ret>", b.display())), None),
            // 前置:A 的命令与键位都通
            (
                Some(":ub-a-cmd<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "A", "前置:A 的命令可用");
                }),
            ),
            (
                Some(":plugin-unload ub-a<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("bye-a"),
                        "onUnload 应在回收前跑: {status}"
                    );
                    assert!(
                        status.as_ref().contains("unloaded ub-a"),
                        "应报告回收计数: {status}"
                    );
                }),
            ),
            // A 的命令没了(状态栏是错误级的 "no such command")
            (
                Some(":ub-a-cmd<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert!(
                        matches!(severity, helix_core::diagnostic::Severity::Error),
                        "已卸载插件的命令应报未知命令: {status}"
                    );
                    assert!(status.as_ref().contains("ub-a-cmd"), "{status}");
                }),
            ),
            // B 完全不受影响
            (
                Some(":ub-b-cmd<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "B", "另一插件必须仍然可用");
                }),
            ),
            // 键位:解绑后 C-b 不再 echo "A"(绑定已摘,按键不再落到已卸插件)
            (
                Some("<C-b>"),
                Some(&|app| {
                    // 按键没有产生任何 echo 才算解绑成功(否则状态栏会留下 "A")
                    let status = app
                        .editor
                        .get_status()
                        .map(|(s, _)| s.to_string())
                        .unwrap_or_default();
                    assert_ne!(status, "A", "已卸载插件的键位应解绑,实得 {status:?}");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
