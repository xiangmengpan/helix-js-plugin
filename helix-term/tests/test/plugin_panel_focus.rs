use super::*;

/// focusable 面板白盒状态脚本：
/// - lastFocus 由 render 的 focus 参数回写（断言焦点路由生效）
/// - onKeyCount 统计面板 onKey 调用次数（断言焦点分支不经过 onKey）
/// - 面板 onKey 返回 "ignore"（与既有面板穿透语义一致）：未聚焦时按键穿透，
///   保证 `:` 命令提示符仍可打开（焦点在节点时按键被焦点分支消费，报告走节点 onKey）
const FOCUS_JS: &str = r#"
let lastFocus = null;
let onKeyCount = 0;
let showExtra = true;
let inputVal = "";
const report = () => helix.echo("focus:" + (lastFocus ?? "none") + " count:" + onKeyCount);
helix.register_command("fp-open", () => {
    helix.open_panel({
        side: "right", size: 30, focusable: true,
        render: (focus) => {
            lastFocus = focus;
            const nodes = [
                helix.el("button", "one", { id: "b1", onPress: () => helix.echo("PRESSED"), onKey: () => report() }),
                { type: "input", id: "i1", value: inputVal, onChange: (v) => { inputVal = v; helix.echo("chg:" + v); } },
            ];
            if (showExtra) nodes.push(helix.el("button", "two", { id: "b2", onKey: (k) => {
                if (k === "h") { showExtra = false; helix.echo("hidden"); } else report();
            } }));
            return helix.el("col", nodes);
        },
        onKey: (key) => { onKeyCount++; helix.echo("key:" + key.name); return "ignore"; },
    });
});
helix.register_command("fp-report", () => report());
"#;

async fn focus_panel_app(plugin: &str) -> anyhow::Result<(tempfile::TempDir, std::path::PathBuf)> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("focus.js");
    std::fs::write(&plugin_path, plugin)?;
    Ok((dir, plugin_path))
}

/// 1. focusable 面板:Tab 聚焦第一个节点 → render 收到 focus="b1"(白盒:lastFocus 回写);
///    Tab 被焦点路由消费,不经过面板 onKey(count 不变)
#[tokio::test(flavor = "multi_thread")]
async fn focusable_panel_tab_focuses_first_node() -> anyhow::Result<()> {
    let _rl = PLUGIN_TEST_LOCK.lock().await;
    let (_dir, plugin_path) = focus_panel_app(FOCUS_JS).await?;

    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(plugin_path.parent().unwrap().join("a.txt"), None)
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (Some(":fp-open<ret>"), None),
            // 打开后未聚焦:render 收到 focus=null;count=1 来自 `:` 键本身(焦点无时按键走 onKey,ignore 穿透给提示符)
            (
                Some(":fp-report<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "focus:none count:1", "status: {status:?}");
                }),
            ),
            // Tab → 焦点路由聚焦 b1(按键被消费,onKey 不调用)
            (Some("<tab>"), None),
            // 'x' 聚焦在 b1 → 直达 b1 onKey → 白盒报告 focus=b1、count 仍 1(仅 `:` 那次)
            (
                Some("x"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(
                        status.as_ref(),
                        "focus:b1 count:1",
                        "Tab must focus first node via routing, not onKey: {status:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

/// 2. 焦点在 button → Enter → onPress 触发(消息断言)
#[tokio::test(flavor = "multi_thread")]
async fn focusable_panel_button_enter_triggers_onpress() -> anyhow::Result<()> {
    let _rl = PLUGIN_TEST_LOCK.lock().await;
    let (_dir, plugin_path) = focus_panel_app(FOCUS_JS).await?;

    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(plugin_path.parent().unwrap().join("a.txt"), None)
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (Some(":fp-open<ret>"), None),
            // Tab 聚焦 b1 → Enter → onPress echo PRESSED(不经面板 onKey)
            (
                Some("<tab><ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(
                        status.as_ref(),
                        "PRESSED",
                        "Enter on focused button must fire onPress: {status:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

/// 3. 焦点在 input → 单字符 → dispatch_input_key → onChange 更新值(白盒:chg: 回显)
#[tokio::test(flavor = "multi_thread")]
async fn focusable_panel_input_insert() -> anyhow::Result<()> {
    let _rl = PLUGIN_TEST_LOCK.lock().await;
    let (_dir, plugin_path) = focus_panel_app(FOCUS_JS).await?;

    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(plugin_path.parent().unwrap().join("a.txt"), None)
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (Some(":fp-open<ret>"), None),
            // Tab(b1) → Tab(i1) → 'a' → input 插入 → onChange("a")
            (
                Some("<tab><tab>a"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(
                        status.as_ref(),
                        "chg:a",
                        "char on focused input must insert via dispatch_input_key: {status:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

/// 4. Esc 取消焦点(不关闭面板):焦点清空后按键回 onKey;Esc 本身不经过 onKey
#[tokio::test(flavor = "multi_thread")]
async fn focusable_panel_esc_cancels_focus() -> anyhow::Result<()> {
    let _rl = PLUGIN_TEST_LOCK.lock().await;
    let (_dir, plugin_path) = focus_panel_app(FOCUS_JS).await?;

    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(plugin_path.parent().unwrap().join("a.txt"), None)
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (Some(":fp-open<ret>"), None),
            // Tab 聚焦 b1(焦点路由消费)
            (Some("<tab>"), None),
            // Esc → 取消焦点:render 收到 focus=null,onKey 不调用(count 仍 1——只来自本步 `:` 键;面板不关闭)
            (
                Some("<esc>:fp-report<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(
                        status.as_ref(),
                        "focus:none count:1",
                        "Esc must cancel focus without touching onKey: {status:?}"
                    );
                }),
            ),
            // 'x' 焦点已清空 → 走面板 onKey;随后 `:` 也走 onKey → count=3(x + `:` + 上一步 `:`)
            // 注意:onKey 返回 ignore 时按键穿透给编辑器,编辑器会清 status(editor.rs),
            // 故不直接断言瞬态 "key:x",改由 typed 命令报告 count 证明 x 确实走了 onKey
            (
                Some("x:fp-report<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(
                        status.as_ref(),
                        "focus:none count:3",
                        "after Esc, keys must route to onKey: {status:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

/// 5. 未启用 focusable 的面板:Tab 走 onKey(零行为变化回归)。
///    断言方式:onKey 记录 lastKey 到 JS 状态,typed 命令读回(onKey 返回 ignore 时
///    按键穿透编辑器会清 status,不能直接断言瞬态 echo)
#[tokio::test(flavor = "multi_thread")]
async fn non_focusable_panel_tab_still_onkey() -> anyhow::Result<()> {
    let _rl = PLUGIN_TEST_LOCK.lock().await;
    let (_dir, plugin_path) = focus_panel_app(
        r#"
        let nfTabCount = 0;
        helix.register_command("nf-open", () => {
            helix.open_panel({
                side: "right", size: 30,
                render: () => helix.el("button", "go", { id: "b1" }),
                onKey: (key) => { if (key.name === "Tab") nfTabCount++; return "ignore"; },
            });
        });
        helix.register_command("nf-report", () => helix.echo("tabs:" + nfTabCount));
        "#,
    )
    .await?;

    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(plugin_path.parent().unwrap().join("a.txt"), None)
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (Some(":nf-open<ret>"), None),
            // 无 focusable:Tab 不经焦点路由,直接到 onKey(nfTabCount 置 1)
            (
                Some("<tab>:nf-report<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(
                        status.as_ref(),
                        "tabs:1",
                        "non-focusable panel must keep Tab on onKey: {status:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

/// 6. 焦点失效重置:render 后焦点节点消失 → 焦点清空(按键回 onKey)→ Tab 从头聚焦第一个节点
#[tokio::test(flavor = "multi_thread")]
async fn focusable_panel_focus_reset_on_node_change() -> anyhow::Result<()> {
    let _rl = PLUGIN_TEST_LOCK.lock().await;
    let (_dir, plugin_path) = focus_panel_app(FOCUS_JS).await?;

    test_key_sequences(
        &mut AppBuilder::new()
            .with_file(plugin_path.parent().unwrap().join("a.txt"), None)
            .build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (Some(":fp-open<ret>"), None),
            // Tab×3 → 焦点在 b2(最后一个节点: b1 → i1 → b2)
            (Some("<tab><tab><tab>"), None),
            // 'h' 聚焦在 b2 → 直达 b2 onKey → 隐藏 b2(白盒:echo hidden)
            (
                Some("h"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "hidden", "status: {status:?}");
                }),
            ),
            // render 后焦点失效 → 清空;Enter 走 onKey(证明焦点已清空,非 stale 分发);
            // 随后 `:` 也走 onKey → count=2;直接断言 count(Enter 穿透编辑器会清 status)
            (
                Some("<ret>:fp-report<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(
                        status.as_ref(),
                        "focus:none count:2",
                        "stale focus must be cleared, Enter routes to onKey: {status:?}"
                    );
                }),
            ),
            // Tab → 从头聚焦 b1(白盒报告)
            (
                Some("<tab>x"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(
                        status.as_ref(),
                        "focus:b1 count:2",
                        "Tab after reset must restart from first node: {status:?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
