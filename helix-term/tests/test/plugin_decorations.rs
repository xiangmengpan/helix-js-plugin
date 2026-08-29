use super::*;

use helix_view::current_ref;

// 命令里 set_virtual_text/set_highlight 当前 doc → 白盒断言 plugin_decorations 字段与 char 坐标
#[tokio::test(flavor = "multi_thread")]
async fn plugin_decorations_applied_and_replaced() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "one\ntwo\n")?; // char 索引:line2 起点 = 4
    let plugin_path = dir.path().join("dec.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"
            helix.register_command("dec1", () => {{
                helix.set_virtual_text("{f}", 1, 1, "X", "ui.help");
                helix.set_highlight("{f}", 0, 0, 1, 3, "ui.selection");
            }});
            helix.register_command("dec2", () => {{
                helix.set_virtual_text("{f}", 0, 0, "Y");
            }});
            helix.register_command("dec-clear", () => {{
                helix.set_virtual_text("{f}");
            }});
            "#,
            f = file.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":dec1<ret>"), None),
            (
                Some(":dec1<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    let pd = &doc.plugin_decorations;
                    assert_eq!(pd.virtual_text.len(), 1);
                    assert_eq!(pd.virtual_text[0].char_idx, 5); // (1,1) → line2 起点4 + 1
                    assert_eq!(pd.virtual_text[0].text.to_string(), "X");
                    assert_eq!(pd.virtual_text[0].style.as_deref(), Some("ui.help"));
                    assert_eq!(pd.highlights.len(), 1);
                    assert_eq!((pd.highlights[0].start, pd.highlights[0].end), (0, 7)); // (0,0)-(1,3):line1 全 + line2 前3("one\ntwo")
                    assert_eq!(pd.highlights[0].style.as_deref(), Some("ui.selection"));
                }),
            ),
            // 替换语义:dec2 只推一个 virtual text → 高亮消失、旧 virtual text 被替换
            (Some(":dec2<ret>"), None),
            (
                Some(":dec2<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.plugin_decorations.virtual_text.len(), 1);
                    assert_eq!(doc.plugin_decorations.virtual_text[0].char_idx, 0);
                    assert_eq!(doc.plugin_decorations.virtual_text[0].text.to_string(), "Y");
                    assert!(doc.plugin_decorations.highlights.is_empty(), "整体替换:旧高亮不残留");
                }),
            ),
            // Clear:只传 path → 全部清空
            (Some(":dec-clear<ret>"), None),
            (
                Some(":dec-clear<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert!(doc.plugin_decorations.virtual_text.is_empty());
                    assert!(doc.plugin_decorations.highlights.is_empty());
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 未打开 path → 不崩、不写入任何 doc
#[tokio::test(flavor = "multi_thread")]
async fn plugin_decorations_unknown_path_ignored() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("dec-unknown.js");
    std::fs::write(
        &plugin_path,
        r#"helix.register_command("dec-unknown", () => {
            helix.set_virtual_text("/nonexistent-helix-xyz.rs", 0, 0, "X");
        });"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":dec-unknown<ret>"), None),
            (
                Some(":dec-unknown<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert!(doc.plugin_decorations.virtual_text.is_empty(), "未打开 path 静默忽略");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// doc-change 监听重推:替换语义保证无残留(插件在 doc-change 里重推)
#[tokio::test(flavor = "multi_thread")]
async fn plugin_decorations_repush_on_doc_change() -> anyhow::Result<()> {
    let _guard = DOC_CHANGE_TEST_LOCK.lock().unwrap();    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("dec-docchange.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"
            helix.on("doc-change", (doc) => {{
                // 每次变更重推:位置跟随变更后的文本
                helix.set_virtual_text("{f}", 0, doc.text.length, "!", null);
            }});
            helix.register_command("dec-init", () => {{
                helix.set_virtual_text("{f}", 0, 0, "Z");
            }});
            "#,
            f = file.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":dec-init<ret>"), None),
            (
                Some(":dec-init<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.plugin_decorations.virtual_text.len(), 1);
                    assert_eq!(doc.plugin_decorations.virtual_text[0].char_idx, 0);
                }),
            ),
            // 编辑触发 doc-change → handler 重推(替换):只剩新位置
            // 断言绑定到 iX<esc> 同一步(plugin_docchange 惯例):doc-change 在 idle 触发,断言紧随
            (
                Some("iX<esc>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.plugin_decorations.virtual_text.len(), 1);
                    // "Xone\n" 长度 5 → 重推位置 (0,5) → char_idx 5;旧的 (0,0) 被替换
                    assert_eq!(doc.plugin_decorations.virtual_text[0].char_idx, 5);
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// async + begin_edit:装饰与编辑同 hold,结束一起应用
#[tokio::test(flavor = "multi_thread")]
async fn plugin_decorations_async_begin_edit() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("dec-async.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"
            helix.register_command("dec-async", () => {{
                helix.begin_edit();
                helix.set_virtual_text("{f}", 0, 0, "A");
                helix.run_async("echo 1").then(() => {{
                    helix.set_highlight("{f}", 0, 0, 1, 0, "ui.selection");
                    helix.end_edit();
                }});
            }});
            "#,
            f = file.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":dec-async<ret>"), None),
            // 后续键驱动 pump,等 async 回调 settle;断言最终状态
            (Some(":dec-async<ret>"), None),
            (
                Some(":dec-async<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.plugin_decorations.virtual_text.len(), 1);
                    assert_eq!(doc.plugin_decorations.virtual_text[0].text.to_string(), "A");
                    assert_eq!(doc.plugin_decorations.highlights.len(), 1);
                    assert_eq!(doc.plugin_decorations.highlights[0].start, 0);
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 同 style 乱序 virtual text:渲染路径按 char_idx 排序(TextAnnotations 要求有序,
// Layer::consume debug_assert)——无排序时 debug 构建渲染即 panic,本测试驱动渲染兜底
#[tokio::test(flavor = "multi_thread")]
async fn plugin_decorations_unsorted_virtual_text_renders() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "one\ntwo\n")?;
    let plugin_path = dir.path().join("dec-unsorted.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"helix.register_command("dec-unsorted", () => {{
                helix.set_virtual_text("{f}", 1, 1, "B", "ui.help");  // char_idx 5
                helix.set_virtual_text("{f}", 0, 0, "A", "ui.help");  // char_idx 0(乱序)
            }});"#,
            f = file.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            // 重复运行:每次渲染都会走排序路径,乱序输入下无 sort 则 debug_assert panic
            (
                Some(":dec-unsorted<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.plugin_decorations.virtual_text.len(), 2);
                    // 字段保持 push 顺序;排序发生在渲染层
                    assert_eq!(doc.plugin_decorations.virtual_text[0].char_idx, 5);
                    assert_eq!(doc.plugin_decorations.virtual_text[1].char_idx, 0);
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 反向高亮区间(sr,sc)>(er,ec):应用时 swap 归一化,产出 start<=end
#[tokio::test(flavor = "multi_thread")]
async fn plugin_decorations_reversed_highlight_normalized() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "one\ntwo\n")?;
    let plugin_path = dir.path().join("dec-rev.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"helix.register_command("dec-rev", () => {{
                helix.set_highlight("{f}", 1, 0, 0, 0, "ui.selection");  // 反向
            }});"#,
            f = file.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some(":dec-rev<ret>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.plugin_decorations.highlights.len(), 1);
                    // (1,0) char 4,(0,0) char 0 → swap 后 start 0, end 4
                    assert_eq!(
                        (doc.plugin_decorations.highlights[0].start, doc.plugin_decorations.highlights[0].end),
                        (0, 4),
                        "反向区间应归一化为 start<=end"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
