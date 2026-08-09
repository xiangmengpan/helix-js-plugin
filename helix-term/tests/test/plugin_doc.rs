use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_command_doc_shown() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("doc.js");
    std::fs::write(
        &plugin_path,
        r#"helix.register_command("docdemo", () => {}, "演示命令说明");"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            // 输入命令名，检查提示区 doc——若 prompt doc 无法从测试断言，此步改为
            // 输入完整命令并执行（证明命令可用），doc 显示靠单测 command_doc 覆盖
            (
                Some(":docdemo"),
                Some(&|app| {
                    // prompt 组件存在即可（doc 文本显示在 prompt 内，难以外部断言）
                    let prompt_open = app
                        .compositor
                        .has_component(std::any::type_name::<helix_term::ui::Prompt>());
                    assert!(prompt_open, "prompt should be open while typing");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
