//! 快照**按需序列化**的端到端证据(2:`render()` 从"每帧无条件序列化"改成
//! "有插件读过才序列化")。
//!
//! 门控的风险是**饿死读者**(没人读 → 不序列化 → 读的时候拿到空)。这条用例钉住:
//! 读过一次之后,下一帧起快照就可用了 —— 首次读可能空一帧(兴趣标记当帧才置位),
//! 第二次必须非空。
use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn buffer_snapshot_is_available_after_first_read() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("snap.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("snap.js");
    std::fs::write(
        &plugin,
        r#"
        // 测试夹具:fail-closed 之后,脚本必须声明它用的能力(本用例不用受保护 API)
        helix.plugin("snap-fixture", { deps: [], permissions: {} });
        helix.register_command("snap-report", () => {
            const list = helix.buffer.list();
            helix.echo("count:" + list.length);
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin.display())),
                None,
            ),
            // 第一次读:允许本帧还是空(兴趣标记当帧置位) —— 只要求不崩
            (Some(":snap-report<ret>"), None),
            // 第二次:上一帧已经序列化过 ⇒ 必须看到 1 个打开文档
            (
                Some(":snap-report<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(
                        status.as_ref(),
                        "count:1",
                        "第二次读必须拿到真实快照(门控不能饿死读者)"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
