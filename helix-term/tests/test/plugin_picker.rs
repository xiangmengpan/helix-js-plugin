use super::*;

use helix_term::ui::{js_picker::PickerRow, overlay::Overlay, picker::Picker};

// helix.picker 端到端：define → run 打开原生 Picker → Enter 触发 action(echo 到状态栏)。
// 锁：echo 消息队列(MESSAGES)进程全局，与 doc-change 等 echo 测试串行防互偷（helpers.rs 注释）。
// 时序：:pick 的 OpenPicker 请求经 job 队列推层，须与 Enter 分两次输入
// （event_loop_until_idle 先把 picker 层推上，再发 Enter 才会落到 picker）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_picker_define_run_enter() -> anyhow::Result<()> {
    let _guard = DOC_CHANGE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin = dir.path().join("picker_test.js");
    std::fs::write(
        &plugin,
        r#"
        helix.picker.define("t", {
            columns: ["name"],
            items: () => [["one"], ["two"]],
            action: (row) => { helix.echo("picked:" + row[0]); },
        });
        helix.register_command("pick", () => helix.picker.run("t"));
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin.display())),
                None,
            ),
            (
                Some(":pick<ret>"),
                Some(&|app| {
                    // 原生 Picker 层已推上（OpenPicker 经 job 队列消费）；open_picker 推的是 overlaid(picker)
                    let picker_type = std::any::type_name::<Overlay<Picker<PickerRow, ()>>>();
                    assert!(
                        app.compositor.has_component(picker_type),
                        "picker layer should be open"
                    );
                }),
            ),
            (
                Some("<ret>"),
                Some(&|app| {
                    // Enter 选中第一行(one) → action echo
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "picked:one");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// async items：items 返回 Promise → 事件泵 pump_jobs 兑现续体后推 OpenPicker → 同 Enter 触发 action。
// 端到端覆盖 Promise 路径（JS 单测已覆盖 push 时序，此处验证 term 侧接得住异步请求）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_picker_async_items_enter() -> anyhow::Result<()> {
    let _guard = DOC_CHANGE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin = dir.path().join("picker_async.js");
    std::fs::write(
        &plugin,
        r#"
        helix.picker.define("t", {
            columns: ["name"],
            items: async () => [["async-one"]],
            action: (row) => { helix.echo("picked:" + row[0]); },
        });
        helix.register_command("pick", () => helix.picker.run("t"));
        "#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin.display())),
                None,
            ),
            (Some(":pick<ret>"), None),
            (
                Some("<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "picked:async-one");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 未注册源：脚本顶层 helix.picker.run("nope") 抛错 → :plugin-load 失败 → 状态栏 Error
#[tokio::test(flavor = "multi_thread")]
async fn plugin_picker_run_unknown_source_errors() -> anyhow::Result<()> {
    let _guard = DOC_CHANGE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin = dir.path().join("picker_bad.js");
    std::fs::write(&plugin, r#"helix.picker.run("nope");"#)?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![(
            Some(&format!(":plugin-load {}<ret>", plugin.display())),
            Some(&|app| {
                let (status, severity) = app.editor.get_status().unwrap();
                assert!(
                    matches!(severity, helix_core::diagnostic::Severity::Error),
                    "expected Error severity, got {severity:?}"
                );
                assert!(status.as_ref().contains("not defined"), "status: {status}");
            }),
        )],
        false,
    )
    .await?;
    Ok(())
}

// 内置插件冒烟：加载仓库 plugins/features/picker.js（4 源 define 全部执行，捕获 API 拼写/形状错误），
// 经 helper 命令跑 buffers 源（唯一不依赖 cwd 的源——files/grep 走 read_tree(".")/rg，测 cwd 扫仓库不可控）：
// items → OpenPicker 推层 → Enter → action focus_buffer → 层关闭；全程无 Error 状态。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_picker_builtin_plugin_buffers_source() -> anyhow::Result<()> {
    let _guard = DOC_CHANGE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let helper = dir.path().join("picker_builtin_helper.js");
    std::fs::write(
        &helper,
        r#"helix.register_command("pick-buffers", () => helix.picker.run("buffers"));"#,
    )?;
    let picker_js =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/features/picker.js");
    let picker_type = std::any::type_name::<Overlay<Picker<PickerRow, ()>>>();
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            // 加载内置插件：4 个 define 全部执行（columns 数组 + items 可调用校验）
            (
                Some(&format!(":plugin-load {}<ret>", picker_js.display())),
                Some(&|app| {
                    assert_status_not_error(&app.editor);
                }),
            ),
            (
                Some(&format!(":plugin-load {}<ret>", helper.display())),
                None,
            ),
            // 跑 buffers 源：items 返回行 → OpenPicker 经 job 队列推层
            (
                Some(":pick-buffers<ret>"),
                Some(&|app| {
                    assert!(
                        app.compositor.has_component(picker_type),
                        "picker layer should be open"
                    );
                }),
            ),
            // Enter：action focus_buffer(payload id) → 层关闭
            (
                Some("<ret>"),
                Some(&|app| {
                    assert!(
                        !app.compositor.has_component(picker_type),
                        "picker layer should close on Enter"
                    );
                    assert_status_not_error(&app.editor);
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 审查 I1：items 返回空列表（如 grep 无匹配）→ 仍打开 picker（空列表展示），不静默跳过。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_picker_empty_rows_still_opens() -> anyhow::Result<()> {
    let _guard = DOC_CHANGE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin = dir.path().join("picker_empty.js");
    std::fs::write(
        &plugin,
        r#"
        helix.picker.define("t", {
            columns: ["name"],
            items: () => [],
            action: (row) => { helix.echo("picked:" + row[0]); },
        });
        helix.register_command("pick", () => helix.picker.run("t"));
        "#,
    )?;
    let picker_type = std::any::type_name::<Overlay<Picker<PickerRow, ()>>>();
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin.display())),
                None,
            ),
            (
                Some(":pick<ret>"),
                Some(&|app| {
                    // 空 rows + 非空 columns → 仍建 picker（空列表）
                    assert!(
                        app.compositor.has_component(picker_type),
                        "picker layer should open even with empty rows"
                    );
                    assert_status_not_error(&app.editor);
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 审查 I2：行宽与列数不匹配 → 状态栏报错（push_message），picker 不打开。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_picker_width_mismatch_reports_error() -> anyhow::Result<()> {
    let _guard = DOC_CHANGE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin = dir.path().join("picker_wide.js");
    std::fs::write(
        &plugin,
        r#"
        helix.picker.define("t", {
            columns: ["name"],
            items: () => [["one", "too-wide"]],
            action: (row) => { helix.echo("picked:" + row[0]); },
        });
        helix.register_command("pick", () => helix.picker.run("t"));
        "#,
    )?;
    let picker_type = std::any::type_name::<Overlay<Picker<PickerRow, ()>>>();
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin.display())),
                None,
            ),
            (
                Some(":pick<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("不匹配"),
                        "expected width mismatch status, got {status}"
                    );
                    assert!(
                        !app.compositor.has_component(picker_type),
                        "picker should not open on width mismatch"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
