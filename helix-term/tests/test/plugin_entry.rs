use super::*;

/// 统一入口：init.js 里 helix.load 导入 + helix.lazy 懒加载。
/// js_load 相对名解析用 PLUGINS_DIR（Application::new 设的是真实 config plugins 目录，
/// 集成测试无法改目录）→ init.js 内容用绝对路径写 mod.js/heavy.js（js_load 支持绝对路径）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_entry_import_and_lazy() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("e.txt");
    std::fs::write(&file, "x\n")?;
    let mod_path = dir.path().join("mod.js");
    let heavy_path = dir.path().join("heavy.js");
    std::fs::write(
        &mod_path,
        r#"helix.register_command("mod-cmd", () => { helix.echo("mod-ok"); });"#,
    )?;
    std::fs::write(
        &heavy_path,
        r#"helix.register_command("heavy-cmd", () => { helix.echo("heavy-ok"); });"#,
    )?;
    let init = dir.path().join("init.js");
    std::fs::write(
        &init,
        format!(
            r#"helix.load("{}"); helix.lazy("{}", "heavy-cmd");"#,
            mod_path.display(),
            heavy_path.display()
        ),
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", init.display())), None),
            (
                Some(":mod-cmd<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "mod-ok");
                }),
            ),
            // 懒加载：首次 heavy-cmd 生效
            (
                Some(":heavy-cmd<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert_eq!(status.as_ref(), "heavy-ok");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// `plugins/init.js`(模板)里每个 `helix.load("...")` 的相对路径都必须**解析得到**。
///
/// 为什么需要(实测发生过的坑):我把 `features/picker.js` 移到 `examples/picker.js` 时,
/// 只检查了**仓库模板**(那里 picker 那行是注释掉的)和测试引用(没有),据此判断"可自由移动"。
/// 但**用户 live 的 `~/.config/helix/init.js` 引用了它** —— 结果那条 load 在启动时失败。
/// 移动/删除插件文件时最容易漏的就是 `init.js` 里的引用。
///
/// 覆盖边界:这个测试只能守**仓库模板**(用户 live 配置不在仓库里,CI 也读不到)。
/// 但模板漂移正是同类事故的一半,而且这一半是可以自动挡住的。
#[test]
fn plugins_init_template_loads_resolve() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins");
    let src = std::fs::read_to_string(dir.join("init.js"))
        .unwrap_or_else(|e| panic!("读不到 {}: {e}", dir.join("init.js").display()));
    let mut checked = 0;
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with("//") {
            continue; // 注释行不算(模板里有大量被注释掉的可选项)
        }
        let Some(rest) = t.strip_prefix("helix.load(\"") else {
            continue;
        };
        let Some(rel) = rest.split('"').next() else {
            continue;
        };
        assert!(
            dir.join(rel).is_file(),
            "plugins/init.js 引用了不存在的文件:{rel}(移动/删除插件时漏改 init.js?)"
        );
        checked += 1;
    }
    // 防空过:解析逻辑若失效(比如 heli.load 写法变了),checked 会是 0,测试就失去意义
    assert!(
        checked >= 3,
        "只检查到 {checked} 个引用 —— 解析逻辑可能失效,这个测试已失去意义"
    );
}
