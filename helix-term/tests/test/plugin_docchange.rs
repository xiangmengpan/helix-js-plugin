use super::*;

use helix_core::diagnostic::Severity;

// ponytail: 见 plugin_async.rs 注释——harness 依赖 multi_thread（block_in_place），
// current_thread 会 panic；多线程下任务迁移丢事件的已知隐患留待后续任务。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_doc_change_event() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("dc.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("docchange.js");
    std::fs::write(
        &plugin_path,
        r#"helix.on("doc-change", (doc) => { helix.echo("changed"); });"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            // 修改文本：i + 输入 + esc → idle 触发 doc-change → 状态栏 "changed"
            (
                Some("ix<esc>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "changed");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
