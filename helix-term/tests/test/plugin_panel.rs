use super::*;

/// ②-3 前置:把面板当前的**存放位置**与两条**陈旧路径**钉住。
///
/// 现状(2026-09-11 实测):`open_panel` 已经走布局树(rail:true → rail 叶子;
/// 否则 → 普通叶子),`close_panel` 也在树上找。但 `remove_panel` / `set_panel_side`
/// 仍查 `compositor.layers` —— 面板不在那儿,所以**恒失败**(死路径)。
/// 后果:`move_panel` 永遠报 "no panel with id",且无任何测试覆盖。
///
/// 这两个测试就是 ②-3 的验收开关:删掉死路径、修好 move_panel 后,
/// 它们会失败 —— 那时应改成断言新行为,而不是删掉了事。
#[tokio::test(flavor = "multi_thread")]
async fn panel_lives_in_layout_tree_not_layers() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let side = helix_term::ui::plugin_panel::PanelSide::Left;
    let leaf = app
        .compositor
        .register_panel(helix_term::ui::PluginPanel::new(7, side, false), side, 20)
        .expect("register_panel 应返回叶子 id");

    // 在布局树里(rail 叶子),不在 compositor.layers
    assert!(
        app.compositor
            .layout_tree()
            .find_leaf_id::<helix_term::ui::PluginPanel>(|_| true)
            .is_some(),
        "面板应在布局树里"
    );
    assert!(
        app.compositor.layout_tree().is_rail(leaf),
        "rail:true → rail 叶子"
    );
    Ok(())
}

/// ②-3 收尾:`move_panel` 现在**真的能用**。
/// 旧实现(`set_panel_side` 查 `compositor.layers`)恒失败,`move_panel` 永远报
/// "no panel with id";面板其实住在布局树里。现在改走树:rail 换边 = 取出组件 →
/// 按新边重新注册(并保住宽度比例)。
#[tokio::test(flavor = "multi_thread")]
async fn rail_panel_can_move_to_other_side() -> anyhow::Result<()> {
    use helix_term::ui::plugin_panel::PanelSide;
    let mut app = AppBuilder::new().build()?;
    let leaf = app
        .compositor
        .register_panel(
            helix_term::ui::PluginPanel::new(7, PanelSide::Left, false),
            PanelSide::Left,
            20,
        )
        .expect("register_panel 应返回叶子 id");
    assert!(app.compositor.layout_tree().is_rail(leaf));
    let ratio_before = app
        .compositor
        .layout_tree()
        .rail_ratio()
        .expect("rail 有比例");

    assert!(
        app.compositor.set_panel_side(7, PanelSide::Right),
        "rail 面板换边应成功(旧实现在这里恒 false)"
    );

    let dump = app.compositor.layout_tree().dump();
    assert_eq!(
        dump.leafs.iter().filter(|l| l.rail).count(),
        1,
        "仍恰一条 rail"
    );
    assert_eq!(dump.leafs.len(), 2, "rail + 编辑器");
    // 宽度比例保住(经 1.0-ratio 往返,用容差比 f32 精确相等)
    let ratio_after = app
        .compositor
        .layout_tree()
        .rail_ratio()
        .expect("rail 有比例");
    assert!(
        (ratio_after - ratio_before).abs() < 1e-4,
        "换边保住宽度比例({ratio_before} -> {ratio_after})"
    );

    // 非 rail 面板(普通叶分裂)不支持换边 —— 明确拒绝而不是静默失败
    let plain = app.compositor.split_leaf_with_ratio(
        helix_term::ui::layout::SplitDir::V,
        false,
        10,
        Box::new(helix_term::ui::PluginPanel::new(
            9,
            PanelSide::Bottom,
            false,
        )),
    );
    if let Some(plain) = plain {
        assert!(
            !app.compositor.set_panel_side(9, PanelSide::Right),
            "非 rail 面板不支持换边"
        );
        let _ = plain;
    }
    Ok(())
}

use helix_view::current_ref;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_panel_open_edit_close() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("panel.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin_path = dir.path().join("panel.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("panel-demo", () => {
            helix.open_panel({ side: "right", size: 20, render: () => ["== panel ==", "content"] });
        });
        "#,
    )?;

    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":panel-demo<ret>"),
                Some(&|app| {
                    assert!(app.compositor.has_component(panel_type), "panel open");
                }),
            ),
            // 面板不拦截按键：事件穿透 → i 进 insert、x 插入、esc 回 normal，文档应变化
            (
                Some("ix<esc>"),
                Some(&|app| {
                    let (_, doc) = current_ref!(app.editor);
                    assert_eq!(doc.text().to_string(), "xhello\n", "typed while panel open");
                }),
            ),
            (
                Some(":panel-close<ret>"),
                Some(&|app| {
                    assert!(!app.compositor.has_component(panel_type), "panel closed");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
