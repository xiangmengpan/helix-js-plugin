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
    // 两个临时根,一次 `set_plugin_roots` 覆盖本测试的全部子场景
    // (进程级 OnceLock 只能设一次 → 多场景必须共用一个根集合)
    let bundled = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let mk = |d: &std::path::Path, rel: &str, body: &str| {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    };

    // ── 1. 裸名 → <name>/plugin.js(约定 §3.3)──────────────────────────
    mk(
        user.path(),
        "myplug/plugin.js",
        r#"helix.plugin("myplug", { deps: [] });
           helix.register_command("myplug-cmd", () => helix.echo("from-plugin-entry"));
           helix.export({ ok: true });"#,
    );
    helix_js::set_plugin_roots(vec![
        bundled.path().to_path_buf(),
        user.path().to_path_buf(),
    ]);
    helix_js::load_script(r#"helix.load("myplug");"#).expect("裸名应解析到 myplug/plugin.js");

    // 反证:不存在的名字必须报错(否则上面的成功可能只是"什么都没做")
    assert!(
        helix_js::load_script(r#"helix.load("no-such-plugin");"#).is_err(),
        "不存在的插件名应报错"
    );

    // ── 2. 旧式 `<name>.js` 仍可加载(向后兼容)─────────────────────────
    mk(user.path(), "legacy.js", r#"helix.echo("legacy-ok");"#);
    helix_js::load_script(r#"helix.load("legacy");"#).expect("旧式 <name>.js 必须仍可加载");

    // ── 3. §4-3:deps 可写**插件名**───────────────────────────────────
    // 注意:这里不断言「依赖先于主脚本执行」—— 现场核对代码是
    //   eval_wrapped(脚本) -> take(deps) -> 再递归加载依赖(commands.rs),
    // 与那句旧注释「加载目标前先递归加载依赖」**矛盾**。顺序语义未定之前,
    // 不该把断言建在它上面。本场景只钉 §4-3 真正的主张:**依赖名能被解析并加载**。
    mk(
        user.path(),
        "mydep/plugin.js",
        r#"helix.plugin("mydep", { deps: [] }); helix.export({ ok: true });"#,
    );
    mk(
        user.path(),
        "myplug2/plugin.js",
        r#"helix.plugin("myplug2", { deps: ["mydep"] }); helix.export({ main: true });"#,
    );
    helix_js::load_script(
        r#"
        helix.load("myplug2");
        // 依赖若真的被拉过,这里按名再加载会命中缓存并返回它的 export
        const d = helix.load("mydep");
        if (!d || d.ok !== true) {
            throw new Error("deps by name not resolved: " + JSON.stringify(d));
        }
        "#,
    )
    .expect("deps 用插件名应能解析并加载(其 export 可被后续加载读到)");

    // ── 4. §4-2 目录级覆盖:入口来自**内置层**,而用户层有同名 sibling ──
    // 没有 §4-2 时:入口取内置(用户没有 plugin.js),helper 却按"后加的根优先"
    // 取到**用户层** → 造出"内置的 main + 用户的 helper"这种混合体。
    // 有 §4-2(prefer 提供根)时:helper 与入口同层 → 恒为内置。
    mk(
        bundled.path(),
        "plug/plugin.js",
        r#"helix.plugin("plug", { deps: [] });
           helix.load("plug/helper.js");
           if (globalThis.__from !== "bundled") {
               throw new Error("混合体:helper 来自 " + globalThis.__from);
           }"#,
    );
    mk(
        bundled.path(),
        "plug/helper.js",
        r#"globalThis.__from = "bundled";"#,
    );
    mk(
        user.path(),
        "plug/helper.js",
        r#"globalThis.__from = "user";"#,
    );
    // §4-2(prerefer 父插件的根):入口取内置(用户层没有 plugin.js),helper 也必须是内置的 ——
    // 不能因为"后加的根优先"而取到用户层那份(那会造出"内置 main + 用户 helper"的混合体)。
    //
    // 这条断言曾经**故意**写成 expect_err(钉住未修好的行为),修好后如期翻转为 expect。
    // 修的关键不是"脚本体延迟执行"(那是误判:`eval_wrapped` 是立即执行的 IIFE),
    // 而是 **prefer 的对象取错了**:原先拿"正在加载的文件自己的根"去 prefer,
    // 只会查回同一个文件 —— 必须取**父插件的根**。
    helix_js::load_script(r#"helix.load("plug");"#)
        .expect("§4-2:插件自身的文件必须与入口同层(否则会抛'混合体')");
}
