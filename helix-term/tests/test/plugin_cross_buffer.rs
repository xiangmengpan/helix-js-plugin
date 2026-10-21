use super::*;

use helix_core::diagnostic::Severity;
use helix_view::current_ref;

// by_path 读另一 buffer 文本(:open 打开第二个文件后,命令里 by_path 读取)
#[tokio::test(flavor = "multi_thread")]
async fn plugin_by_path_reads_other_buffer() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file1 = dir.path().join("a.txt");
    std::fs::write(&file1, "one\n")?;
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file2, "two\n")?;
    let plugin_path = dir.path().join("read.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"helix.register_command("show-other", () => {{
                const d = helix.by_path("{}");
                helix.echo(d === null ? "null" : d.text.trim());
            }});"#,
            file2.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file1, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (Some(&format!(":open {}<ret>", file2.display())), None),
            (
                Some(":show-other<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(*severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "two");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 未打开路径 → null,命令不崩
#[tokio::test(flavor = "multi_thread")]
async fn plugin_by_path_missing_returns_null() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file1 = dir.path().join("a.txt");
    std::fs::write(&file1, "one\n")?;
    let plugin_path = dir.path().join("null.js");
    std::fs::write(
        &plugin_path,
        r#"helix.register_command("show-null", () => {
            helix.echo(helix.by_path("/nonexistent-helix-xyz.rs") === null ? "null" : "hit");
        });"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file1, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":show-null<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "null");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 改另一 buffer:内容生效,当前 buffer 不受影响;undo 一次回退该 buffer
#[tokio::test(flavor = "multi_thread")]
async fn plugin_edit_other_buffer_undo_per_doc() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file1 = dir.path().join("a.txt");
    std::fs::write(&file1, "one\n")?;
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file2, "two\n")?;
    let plugin_path = dir.path().join("edit.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"helix.register_command("edit-other", () => {{
                const d = helix.by_path("{}");
                if (d !== null) d.insert(0, 0, "X");
            }});"#,
            file2.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file1.clone(), None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (Some(&format!(":open {}<ret>", file2.display())), None),
            (Some(":edit-other<ret>"), None),
            (
                // 当前 doc = file2,已被插入 "X"
                Some(&format!(":open {}<ret>", file1.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(
                        doc.text().to_string(),
                        "one\n",
                        "file1 不被跨 buffer 编辑影响"
                    );
                }),
            ),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "Xtwo\n");
                }),
            ),
            // 一次 undo 回退 file2 的插入(每 doc 一事务)
            (Some("u"), None),
            (Some(&format!(":open {}<ret>", file1.display())), None),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "two\n", "undo 回退跨 buffer 编辑");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 一个命令混合改当前与另一 buffer:各自独立撤销
#[tokio::test(flavor = "multi_thread")]
async fn plugin_mixed_edits_undo_independent() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file1 = dir.path().join("a.txt");
    std::fs::write(&file1, "one\n")?;
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file2, "two\n")?;
    let plugin_path = dir.path().join("mix.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"helix.register_command("mix", (ctx) => {{
                ctx.doc.insert(0, 0, "A");
                const d = helix.by_path("{}");
                if (d !== null) d.insert(0, 0, "B");
            }});"#,
            file2.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file1.clone(), None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            // 先打开 file2 再切回 file1：by_path 只查已打开 buffer
            (Some(&format!(":open {}<ret>", file2.display())), None),
            (Some(&format!(":open {}<ret>", file1.display())), None),
            (Some(":mix<ret>"), None),
            (
                // 当前 doc = file1
                Some("u"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "one\n", "undo 回退当前 buffer 的 A");
                }),
            ),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(
                        doc.text().to_string(),
                        "Btwo\n",
                        "另一 buffer 的 B 未被连带撤销"
                    );
                }),
            ),
            (Some("u"), None),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "two\n", "第二处 undo 独立回退 B");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 两条路由指向**同一个** buffer:`ctx.doc`(当前 buffer)与 `helix.by_path(当前文件)` ——
// 必须归到**一个** Transaction ⇒ **一次撤销**回退两处。
// (此前的分组按 Edit.doc 原样做键 ⇒ None 与 Some(同一路径) 分成两组 ⇒ 两次撤销,
//  与 apply_plugin_edits 的"每个被改 buffer 一次撤销"口径不符;仅"两条路由同 buffer"这一格有差异,
//  两个**不同** buffer 的情形本就该独立撤销,见上一条用例 ✓)
#[tokio::test(flavor = "multi_thread")]
async fn plugin_edit_same_doc_two_routes_one_undo() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "one\n")?;
    let plugin_path = dir.path().join("same.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"helix.register_command("same", (ctx) => {{
                ctx.doc.insert(0, 0, "A");
                const d = helix.by_path("{}");
                if (d !== null) d.insert(0, 0, "B");
            }});"#,
            file.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file.clone(), None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":same<ret>"),
                Some(&|app| {
                    // ⚠️ 先钉住"两处编辑真的都生效" —— 否则 by_path 返回 null 时本用例会**空跑**
                    //(只有一处编辑 ⇒ 一次撤销当然通过),这正是第一版踩到的假绿。
                    let (_, doc) = current_ref!(app.editor);
                    let t = doc.text().to_string();
                    assert!(
                        t.contains('A') && t.contains('B'),
                        "两条路由的编辑都应生效(空跑即假绿): {t:?}"
                    );
                }),
            ),
            (
                Some("u"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(
                        doc.text().to_string(),
                        "one\n",
                        "两条路由指同一 buffer ⇒ 一次撤销应回退 A 与 B 两处"
                    );
                }),
            ),
            // 第二次 u:证明上面那一步就是**唯一**的一步(撤销栈不因两条路由而多出一格)
            (
                Some("u"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(
                        doc.text().to_string(),
                        "one\n",
                        "第二次 undo 不该再改变任何东西(说明两处编辑合成了一步)"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// async 命令:begin_edit → by_path 改其它 buffer → await → 再改 → end_edit → 一次撤销
//
// ⚠️ **已知对调度敏感**(2026-10-16 实测:整包并行跑、机器忙时偶发红;单独重跑 3/3 绿)。
// 机制:`.then()` 里的 `end_edit()` 必须在下一条命令**开始前**跑完 —— 否则命令入口会复位
// 事务深度并**清空**积压编辑(`begin_edit` 未配对 ⇒ 整批丢弃),两处编辑一起消失
// (实得 `two/three`,期望 `Xtwo/Zthree`)。
// 注:试过"多插几个 idle 步"当缓冲,**反而让 app 提前退出**(该用例对步数敏感)⇒ 已回退。
// 要根治得改成"等到条件成立"而不是"数步数"(与 `wait_for_term_event` 同款),留作后续。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_async_begin_edit_cross_buffer() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file1 = dir.path().join("a.txt");
    std::fs::write(&file1, "one\n")?;
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file2, "two\nthree\n")?;
    let plugin_path = dir.path().join("async-edit.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"
        // 测试夹具:fail-closed 之后,脚本必须声明它用的能力(否则被拒)
        helix.plugin("test-fixture", {{ deps: [], permissions: {{ process: true, fs_read: true, fs_write: true }} }});


            helix.register_command("async-xedit", () => {{
                helix.begin_edit();
                const d = helix.by_path("{}");
                if (d !== null) d.insert(0, 0, "X");
                helix.run_async("echo 1").then(() => {{
                    const d2 = helix.by_path("{}");
                    if (d2 !== null) d2.insert(1, 0, "Z");
                    helix.end_edit();
                }});
            }});
            "#,
            file2.display(),
            file2.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file1, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            // 先打开 file2：by_path 只查已打开 buffer
            (Some(&format!(":open {}<ret>", file2.display())), None),
            (Some(":async-xedit<ret>"), None),
            (Some(&format!(":open {}<ret>", file2.display())), None),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(
                        doc.text().to_string(),
                        "Xtwo\nZthree\n",
                        "跨 await 两次编辑一次应用"
                    );
                }),
            ),
            // 一次 undo 回退两处(同一事务)
            (Some("u"), None),
            (
                Some(&format!(":open {}<ret>", file2.display())),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "two\nthree\n");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 回归:vsplit 双 view 下,当前 view 从未访问过的后台 doc 被 by_path 编辑不 panic。
// 修复前 apply_plugin_edits 用预取 current_view_id 编辑后台 doc → selections 缺条目 → panic 崩溃。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_edit_other_buffer_vsplit_no_panic() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file1 = dir.path().join("a.txt");
    std::fs::write(&file1, "one\n")?;
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file2, "two\n")?;
    let plugin_path = dir.path().join("edit.js");
    std::fs::write(
        &plugin_path,
        format!(
            r#"helix.register_command("edit-other", () => {{
                const d = helix.by_path("{}");
                if (d !== null) d.insert(0, 0, "X");
            }});"#,
            file2.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file1.clone(), None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            // :vsplit 后焦点在新 view B(两个 view 都在 file1)
            (Some(":vsplit<ret>"), None),
            // view B 打开 file2;view A 仍是 file1 且从未访问 file2
            (Some(&format!(":open {}<ret>", file2.display())), None),
            // 焦点回 view A(当前 doc = file1);view A 在 file2 无 selection 条目
            (
                Some("<space>ww"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "one\n", "焦点应回到 file1(view A)");
                }),
            ),
            // 修复前:此处按 view A 的 id 编辑 file2 → apply_inner 索引 selections 缺条目 → panic
            (Some(":edit-other<ret>"), None),
            // 切到 view B(file2):内容已被改,全程不 panic
            (
                Some("<space>ww"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "Xtwo\n");
                }),
            ),
            // 关掉 view B 回到单 view,否则 harness 收尾的 :q! 只关一个 view 无法退出
            (Some("<space>wq"), None),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 相对路径 by_path:cwd-join 端到端(:cd 后 by_path("b.txt") 命中已打开 buffer)
#[tokio::test(flavor = "multi_thread")]
async fn plugin_by_path_relative_resolves_from_cwd() -> anyhow::Result<()> {
    // cwd 是进程级全局状态:独占它(Drop 时还原),否则并行时会被别的 `:cd` 换走
    let _cwd = CwdScope::pinned().await;
    let dir = tempfile::tempdir()?;
    let file1 = dir.path().join("a.txt");
    std::fs::write(&file1, "one\n")?;
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file2, "two\n")?;
    let plugin_path = dir.path().join("rel.js");
    std::fs::write(
        &plugin_path,
        r#"helix.register_command("show-rel", () => {
            const d = helix.by_path("b.txt");
            helix.echo(d === null ? "null" : d.text.trim());
        });"#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file1, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (Some(&format!(":cd {}<ret>", dir.path().display())), None),
            (Some(":open b.txt<ret>"), None),
            (
                Some(":show-rel<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "two");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
