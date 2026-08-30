use super::*;

/// multiline input 集成（白盒状态脚本）：
/// - 多行 input（multiline: true）：Enter 光标处插 \n（input 内消费,不触发 onKey/提交）
/// - Up/Down 行间移动保持列（短行 clamp 行尾）
/// - 单行 input Enter 仍走 onKey("Enter")（提交语义回归）
/// - onChange 回显 JSON.stringify(value)（\n 以 \\n 转义序列出现在状态栏）
#[tokio::test(flavor = "multi_thread")]
async fn plugin_input_multiline_enter_newline_and_line_move() -> anyhow::Result<()> {
    let _rl = super::PANEL_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("ml.js");
    std::fs::write(
        &plugin_path,
        r#"
        let mval = "ab\ncd";
        helix.register_command("ml", () => {
            helix.open_popup({
                render: () => helix.el("col", [
                    { type: "input", id: "m", multiline: true, value: mval,
                      onChange: (v) => { mval = v; helix.echo("chg:" + JSON.stringify(v)); },
                      onKey: (k) => helix.echo("mkey:" + k) },
                    { type: "input", id: "s", value: "",
                      onChange: (v) => { helix.echo("schg:" + v); },
                      onKey: (k) => helix.echo("skey:" + k) },
                ]),
            });
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
            (Some(":ml<ret>"), None),
            // Tab 聚焦 m(首个 input);'X' 插到行尾 → "ab\ncdX"(光标初值 = 值末尾)
            (
                Some("<tab>X"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("chg:\"ab\\ncdX\""),
                        "char insert on multiline input: {status:?}"
                    );
                }),
            ),
            // Up:行1(列 3)→ 行0,短行 clamp 列 2(行尾);'Y' → "abY\ncdX"
            (
                Some("<up>Y"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("chg:\"abY\\ncdX\""),
                        "Up must move cursor to line 0 with col clamp: {status:?}"
                    );
                }),
            ),
            // Down:行0(列 3=行尾)→ 行1 列 3(行尾);'Z' → "abY\ncdXZ"
            (
                Some("<down>Z"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("chg:\"abY\\ncdXZ\""),
                        "Down must move cursor into line 1 at same col: {status:?}"
                    );
                }),
            ),
            // Tab → 聚焦 s(单行 input);Enter → 仍走 onKey("Enter")(提交语义回归)
            (
                Some("<tab><ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("skey:Enter"),
                        "single-line input Enter must stay onKey: {status:?}"
                    );
                }),
            ),
            // Tab → 回 m(multiline);Enter → 光标处插 \n(不触发 onKey/提交) → "abY\ncdXZ\n"
            (
                Some("<tab><ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("chg:\"abY\\ncdXZ\\n\""),
                        "multiline Enter must insert newline, not commit: {status:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
