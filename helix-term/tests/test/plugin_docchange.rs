use super::*;

use helix_core::diagnostic::Severity;

// ponytail: 见 plugin_async.rs 注释——harness 依赖 multi_thread（block_in_place），
// current_thread 会 panic；多线程下任务迁移丢事件的已知隐患留待后续任务。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_doc_change_event() -> anyhow::Result<()> {
    let _guard = DOC_CHANGE_TEST_LOCK.lock().unwrap();
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
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
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

/// echo 事件参数里合并后的 changes 范围(0,0-0,0|0,0-0,1 格式:oldRange start-end | newRange start-end)
fn range_echo_script() -> String {
    r#"helix.on("doc-change", (doc) => {
        if (doc.changes.length === 0) { return; }
        const c = doc.changes[0];
        helix.echo(c.oldRange.start.row + "," + c.oldRange.start.col + "-" + c.oldRange.end.row + "," + c.oldRange.end.col + "|" + c.newRange.start.row + "," + c.newRange.start.col + "-" + c.newRange.end.row + "," + c.newRange.end.col);
    });"#
    .to_string()
}

// doc-change 带 changes:单次插入 → oldRange 精确到插入点,newRange 为插入后的文本范围
#[tokio::test(flavor = "multi_thread")]
async fn plugin_doc_change_single_change_range() -> anyhow::Result<()> {
    let _guard = DOC_CHANGE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("dc1.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("docchange1.js");
    std::fs::write(&plugin_path, range_echo_script())?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            // 光标 (0,0) 插 "x":old = (0,0)-(0,0)(插入点),new = (0,0)-(0,1)
            (
                Some("ix<esc>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "0,0-0,0|0,0-0,1");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

// doc-change 窗口内两次插入 → 合并为包围范围(光标 (0,0) 插 "a" 再插 "b")
#[tokio::test(flavor = "multi_thread")]
async fn plugin_doc_change_merged_range() -> anyhow::Result<()> {
    let _guard = DOC_CHANGE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("dc2.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("docchange2.js");
    std::fs::write(&plugin_path, range_echo_script())?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            // 两次插入:a 在 char0,b 在 char1 → 合并 old = (0,0)-(1,1) → (0,0)-(0,1);new = (0,0)-(1,2) → (0,0)-(0,2)
            (
                Some("iab<esc>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "0,0-0,1|0,0-0,2");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

// doc-change 删除场景:光标处 "d" 删 'h' → old = (0,0)-(0,1)(被删文本),new = (0,0)-(0,0)(零宽)
#[tokio::test(flavor = "multi_thread")]
async fn plugin_doc_change_delete_range() -> anyhow::Result<()> {
    let _guard = DOC_CHANGE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("dc3.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("docchange3.js");
    std::fs::write(&plugin_path, range_echo_script())?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            // 删除 char0 'h' → old (0,1),new (0,0) → 当前文本 "ello\n" 换算 (0,0)-(0,1)|(0,0)-(0,0)
            (
                Some("d"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "0,0-0,1|0,0-0,0");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

// doc-change 替换场景:c 删光标字符 + 输入替换 → 合并后 old/new 都是非零宽(文本被替换)
#[tokio::test(flavor = "multi_thread")]
async fn plugin_doc_change_replace_range() -> anyhow::Result<()> {
    let _guard = DOC_CHANGE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("dc4.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("docchange4.js");
    std::fs::write(&plugin_path, range_echo_script())?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            // c 删 char0(→ (0,1)|(0,0)) + 插 "Z"(→ (0,0)|(0,1)) → 合并 (0,0)-(0,1)|(0,0)-(0,1)
            (
                Some("cZ<esc>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "0,0-0,1|0,0-0,1");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
