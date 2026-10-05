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
        // 入口既可能用 helix.load(,也可能用 init.js 里的 safe_load( 包装
        // (safe_load = 失败只报错并继续,不让一个插件拖垮整个 init.js)
        let Some(rest) = t
            .strip_prefix("helix.load(\"")
            .or_else(|| t.strip_prefix("safe_load(\""))
        else {
            continue;
        };
        let Some(rel) = rest.split('"').next() else {
            continue;
        };
        // **按新布局约定解析**(与加载器的 `state::entry_keys` 同一规则):
        // 裸名 → `<name>/plugin.js`;带 `.js` 或含 `/` → 文件路径(后门)。
        // 这条曾经按"目录直接拼文件名"解析,于是在 init.js 改用裸名点名后**立刻挂了**
        // —— 护栏测试的价值就在这里:它逼着解析规则与加载器保持一致。
        let target = if rel.ends_with(".js") || rel.contains('/') {
            dir.join(rel)
        } else {
            dir.join(rel).join("plugin.js")
        };
        assert!(
            target.is_file(),
            "plugins/init.js 引用了不存在的{}:{}(移动/删除插件时漏改 init.js?)",
            if rel.ends_with(".js") || rel.contains('/') {
                "文件"
            } else {
                "插件入口"
            },
            target.display()
        );
        checked += 1;
    }
    // 防空过:解析逻辑若失效(比如 heli.load 写法变了),checked 会是 0,测试就失去意义
    assert!(
        checked >= 3,
        "只检查到 {checked} 个引用 —— 解析逻辑可能失效,这个测试已失去意义"
    );
}

/// **布局约定的端到端验收**:`helix.load("<name>")` 真的加载 `<name>/plugin.js`。
///
/// 为什么放在集成测试而不是 helix-js 单测:它必须设置**进程级**插件根
/// (`set_plugin_roots`,OnceLock 不可重置)。放在共享的单测进程里会污染其他测试
/// —— 实测连带弄挂 5 个无关测试。集成测试各自独立进程,全局互不干扰。
#[test]
fn bare_plugin_name_loads_plugin_entry_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("myplug")).unwrap();
    std::fs::write(
        dir.path().join("myplug").join("plugin.js"),
        r#"
        helix.plugin("myplug", { deps: [] });
        helix.register_command("myplug-cmd", () => helix.echo("from-plugin-entry"));
        helix.export({ ok: true });
        "#,
    )
    .unwrap();
    helix_js::set_plugin_roots(vec![dir.path().to_path_buf()]);

    // 裸名 → <name>/plugin.js,加载成功
    helix_js::load_script(r#"helix.load("myplug");"#).expect("裸名应解析到 myplug/plugin.js");

    // 反证:不存在的名字必须报错(否则上面的成功可能只是"什么都没做")
    assert!(
        helix_js::load_script(r#"helix.load("no-such-plugin");"#).is_err(),
        "不存在的插件名应报错"
    );

    // 旧式 `<name>.js` 仍然可加载(**向后兼容**:约定是新增第二条路,不是替换)
    std::fs::write(dir.path().join("legacy.js"), r#"helix.echo("legacy-ok");"#).unwrap();
    helix_js::set_plugin_roots(vec![dir.path().to_path_buf()]); // 幂等(已设则忽略)
    helix_js::load_script(r#"helix.load("legacy");"#).expect("旧式 <name>.js 必须仍可加载");

    // ── §4-3:deps 可以写**插件名**(而不只是文件 key)──────────────────
    // 依赖走的是同一个 load_script_checked → 同一个 entry_keys 解析规则,
    // 所以 `deps: ["mydep"]` 应解析到 `mydep/plugin.js`。这里同时钉住**顺序**:
    // 依赖必须先于声明它的脚本被加载。
    // 自证式:依赖设全局标记;**主脚本在校验失败时直接抛错** ——
    // 于是"加载成功"本身就证明了依赖已先加载,不依赖消息管道是否有货
    // (实测:load 期的 helix.echo 不会进 take_messages(),用消息断言会假失败)。
    std::fs::create_dir_all(dir.path().join("mydep")).unwrap();
    std::fs::write(
        dir.path().join("mydep").join("plugin.js"),
        r#"helix.plugin("mydep", { deps: [] }); globalThis.__depLoaded = true;"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("myplug")).unwrap();
    std::fs::write(
        dir.path().join("myplug").join("plugin.js"),
        r#"
        helix.plugin("myplug", { deps: ["mydep"] });
        if (!globalThis.__depLoaded) { throw new Error("依赖 mydep 未先加载"); }
        "#,
    )
    .unwrap();
    helix_js::load_script(r#"helix.load("myplug");"#)
        .expect("deps 用插件名应能加载,且依赖必须先于主脚本");
}
