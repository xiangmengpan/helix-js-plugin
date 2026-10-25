//! `plugin` 域测试。

use super::*;

/// 多根查找(对齐 Neovim runtimepath 语义):**后加的根覆盖先加的**。
/// 这是"把功能做成随软件分发的内置插件"的地基 —— 内置放前、用户放后即可覆盖。
#[test]
fn plugin_roots_resolve_with_later_overriding_earlier() {
    let bundled = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let mk = |dir: &std::path::Path, rel: &str, tag: &str| {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, tag).unwrap();
    };
    // 两边都有同名:应取 user 那份
    mk(bundled.path(), "features/x/index.js", "bundled");
    mk(user.path(), "features/x/index.js", "user");
    // 只有内置有:应能找到
    mk(bundled.path(), "lib/only_bundled.js", "b");
    let roots = vec![bundled.path().to_path_buf(), user.path().to_path_buf()];

    let got = crate::state::resolve_in(&roots, "features/x/index.js").unwrap();
    assert_eq!(
        std::fs::read_to_string(&got).unwrap(),
        "user",
        "后加的根(user)必须覆盖先加的(bundled)"
    );
    assert!(
        crate::state::resolve_in(&roots, "lib/only_bundled.js").is_some(),
        "只在前面根里的相对名也要能找到"
    );
    // 都不存在 → 回落到**最后一个根**(报错信息贴近用户目录)
    let miss = crate::state::resolve_in(&roots, "nope.js").unwrap();
    assert!(
        miss.starts_with(user.path()),
        "回落应指向最后一个根: {miss:?}"
    );
    // 绝对路径原样透传
    let abs = bundled.path().join("lib/only_bundled.js");
    assert_eq!(
        crate::state::resolve_in(&roots, abs.to_str().unwrap()).unwrap(),
        abs
    );
}

/// **探测**:JS 能否给原生的 `helix.plugin`(**声明函数对象**)挂/改属性?
///
/// 这是 `:plugin` 迁移兼容层的唯一未知点(见 docs/plugin-layout.md §13):
/// Rust 侧在 `helix.plugin` 这个函数对象上挂了 `install`/`update`/`remove`,
/// 兼容层要由 JS 顶替 —— 若原生对象不可写,就得改走"JS 导出模块"(公开 API 变更)。
/// **动手前先在这里问清楚。**
///
/// 读值方式:`helix.echo` + `take_messages()`(与 helix-term 的运行路径同一机制)。
#[test]
fn plugin_object_accepts_new_and_existing_properties() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let _ = take_messages(); // 清空上一次残留

    // ① 挂**新**属性
    load_script(r#"helix.plugin.__probe = "ok"; helix.echo(helix.plugin.__probe);"#).unwrap();
    let m1 = take_messages();
    assert!(
        m1.iter().any(|m| m == "ok"),
        "原生 helix.plugin 应可挂新属性,实得 {m1:?}"
    );

    // ② 覆盖**既有**属性(`install` 由 Rust 挂上)→ 用标记值确认**确实被改掉**
    load_script(r#"helix.plugin.install = () => "shim"; helix.echo(helix.plugin.install());"#)
        .unwrap();
    let m2 = take_messages();
    assert!(
        m2.iter().any(|m| m == "shim"),
        "既有属性 install 应可被 JS 覆盖,否则兼容层方案不成立,实得 {m2:?}"
    );
}

#[test]
fn loaded_scripts_list() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::host::with_state_or_init(|st| {
        load_script_named(st, "a.js", r#"helix.register_command("a", () => {});"#).unwrap()
    });
    crate::host::with_state_or_init(|st| {
        load_script_named(st, "b.js", r#"helix.register_command("b", () => {});"#).unwrap()
    });
    let names = loaded_scripts();
    assert!(names.iter().any(|n| n == "a.js"));
    assert!(names.iter().any(|n| n == "b.js"));
}

#[test]
fn register_and_run_command() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("where", (ctx) => {
        helix.echo("cursor: " + ctx.cursor.row + "," + ctx.cursor.col);
    });
    "#,
    )
    .unwrap();

    let ctx = CommandContext {
        path: Some("/tmp/demo.rs".to_string()),
        text: "hello\nworld".to_string(),
        cursor: (1, 2),
        selection: ((0, 0), (0, 0)),
        docs: vec![],
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "where", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["cursor: 1,2"]);

    // 未注册的命令返回 false
    assert!(!crate::host::with_state_or_init(|st| run_command(st, "nope", &ctx)).unwrap());
    // 非法命令名（含空白）注册时报错
    assert!(load_script(r#"helix.register_command("bad name", () => {});"#).is_err());
}

/// C-1:`helix.plugin` 的新字段被**解析并记录**(deps/schema_version/min_editor/
/// permissions/contributes);`permissions` 不写时是 `None`(= legacy 语义)。
#[test]
fn plugin_manifest_fields_are_parsed_and_recorded() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    set_plugins_dir(d.clone());
    std::fs::write(
        d.join("rich.js"),
        r#"helix.plugin("rich", { schema_version: 1, min_editor: "25.7", deps: [],
             permissions: { process: true, fs_read: false, fs_write: true },
             contributes: ["cmd:rich", "event:save"] });
           helix.register_command("rich", () => {});"#,
    )
    .unwrap();
    std::fs::write(
        d.join("legacy.js"),
        r#"helix.plugin("legacy", { deps: [] });"#,
    )
    .unwrap();
    crate::host::with_state_or_init(|st| {
        load_script_named(
            st,
            "rich",
            &std::fs::read_to_string(d.join("rich.js")).unwrap(),
        )
        .unwrap()
    });
    let st = crate::host::state_handle_or_init();
    load_script_named(
        &st,
        "legacy",
        &std::fs::read_to_string(d.join("legacy.js")).unwrap(),
    )
    .unwrap();

    let manifests = plugin_manifests();
    let rich = manifests
        .iter()
        .find(|m| m.name == "rich")
        .expect("rich 已记录");
    assert_eq!(rich.schema_version, Some(1)); // == API_VERSION(更高会被 plugin_schema_version_is_enforced 拒掉)
    assert_eq!(rich.min_editor.as_deref(), Some("25.7"));
    assert_eq!(rich.contributes, vec!["cmd:rich", "event:save"]);
    let perms = rich.permissions.expect("permissions 已声明");
    assert!(perms.process && perms.fs_write && !perms.fs_read);
    let legacy = manifests
        .iter()
        .find(|m| m.name == "legacy")
        .expect("legacy 已记录");
    assert!(
        legacy.permissions.is_none(),
        "没写 permissions ⇒ None(legacy)"
    );
}

/// C-2:声明了能力 ⇒ 直接放行;声明但没列那项 ⇒ **走询问路径**(拒绝本次 + 记入
/// pending),授权后放行;拒绝后一直拒。
#[test]
fn permission_check_ask_then_decide() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    set_plugins_dir(d.clone());
    std::fs::write(
        d.join("asker.js"),
        r#"helix.plugin("asker", { permissions: { process: false } });
           helix.register_command("ask-run", () => { helix.run("echo ok"); });
           helix.register_command("ask-write", () => { helix.write_plugin_file("t.txt", "x"); });"#,
    )
    .unwrap();
    // 按**源**加载(load_script 收的是源码;按名字走 helix.load 才有"提供根"语义)
    let st = crate::host::state_handle_or_init();
    load_script_named(
        &st,
        "asker",
        &std::fs::read_to_string(d.join("asker.js")).unwrap(),
    )
    .unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    // 未声明 process ⇒ 被拒(并把请求记入待授权)
    let err = crate::host::with_state_or_init(|st| run_command(st, "ask-run", &ctx))
        .unwrap_err()
        .to_string();
    assert!(err.contains("未声明能力"), "{err}");
    assert!(
        pending_permissions()
            .iter()
            .any(|(p, c)| p == "asker" && c == "process"),
        "待授权列表应含 (asker, process):{:?}",
        pending_permissions()
    );
    // 授权后直接放行
    decide_permission("asker", "process", true);
    assert!(crate::host::with_state_or_init(|st| run_command(st, "ask-run", &ctx)).unwrap());
    assert!(pending_permissions().is_empty(), "决定后应移出待授权");
    // 未声明的另一项仍要问;拒绝后一直拒
    decide_permission("asker", "fs_write", false);
    let err = crate::host::with_state_or_init(|st| run_command(st, "ask-write", &ctx))
        .unwrap_err()
        .to_string();
    assert!(err.contains("已被拒绝"), "{err}");
}

/// `helix.open_terminal` 会**起 pty 跑命令** ⇒ 同属 `process` 能力。
/// 此前它漏在检查点之外(声明了 `permissions` 却没要 process 的插件仍能借此起 shell)。
#[test]
fn open_terminal_needs_process_capability() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    set_plugins_dir(d.clone());
    // 声明了 fs_read 但**没有** process ⇒ open_terminal 必须被拒(且在建 pty 之前拒)
    load_script(
        r#"
        helix.plugin("term-cap", { permissions: { fs_read: true } });
        helix.register_command("term-cap-go", () => {
            helix.open_terminal({ cmd: "true", side: "bottom", size: 10 });
        });
        "#,
    )
    .unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    let err = crate::host::with_state_or_init(|st| run_command(st, "term-cap-go", &ctx))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("未声明能力") && err.contains("process"),
        "{err}"
    );
    assert!(
        pending_permissions()
            .iter()
            .any(|(p, c)| p == "term-cap" && c == "process"),
        "应记入待授权:{:?}",
        pending_permissions()
    );
}

/// C-2:内置插件(来自**用户插件目录以外**的根)永不询问 —— 17 个内置插件
/// grandfathered 的根据。内置根与用户根同时存在时,从内置根加载的那个放行,
/// 从用户根加载的同名插件照旧要问。
/// B1(2026-10):`helix.server.*` 会下载/安装/卸载并改写 `languages.toml` ⇒ 受 `process` 检查。
///
/// 这条测试守两件事:①声明了 `permissions` 却缺 `process` ⇒ **被拒**(在入队/落盘之前);
/// ②用户授权后同一调用放行(能力模型的自愈路径)。
#[test]
fn server_apis_require_process_capability() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    set_plugins_dir(d.clone());
    // 只声明 fs_read(不声明 process)⇒ server.* 应被拒
    load_script(
        r#"
        helix.plugin("srv-cap", { permissions: { fs_read: true } });
        helix.register_command("srv-cap-go", () => { helix.server.list(); });
        "#,
    )
    .unwrap();
    crate::state::take_ui_requests(); // 清掉加载期残留
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    let err = crate::host::with_state_or_init(|st| run_command(st, "srv-cap-go", &ctx))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("process"),
        "server.list 未声明 process 时必须被拒,且报错要指名能力;err={err}"
    );
    assert!(
        crate::state::take_ui_requests().is_empty(),
        "被拒时不得留下任何 ServerOp 请求(必须在入队之前拦住)"
    );

    // 用户允许后放行(不再询问)
    crate::permissions::decide("srv-cap", "process", true);
    crate::host::with_state_or_init(|st| run_command(st, "srv-cap-go", &ctx)).unwrap();
    assert!(
        !crate::state::take_ui_requests().is_empty(),
        "授权后应正常入队 ServerOp"
    );
}

#[test]
fn builtin_plugin_is_grandfathered() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    let builtin = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let (b, u) = (builtin.path().to_path_buf(), user.path().to_path_buf());
    std::mem::forget(builtin);
    std::mem::forget(user);
    // 自带 runtime 在前、用户目录在后(顺序即语义:末位=用户层)
    crate::host::with_state_or_init(|st| {
        crate::state::set_plugin_roots(st, vec![b.clone(), u.clone()])
    });
    let src = r#"helix.plugin("core", { permissions: {} });
           helix.register_command("core-run", () => { helix.run("echo ok"); });"#;
    std::fs::write(b.join("core.js"), src).unwrap();
    std::fs::write(u.join("core.js"), src).unwrap();
    // 同名文件两处都有 ⇒ 用户层覆盖(解析从后往前);先只放内置那份
    std::fs::remove_file(u.join("core.js")).unwrap();
    // 按**名字**加载 ⇒ 才走"哪个根提供的"这条路径(内置判据依赖它)
    load_script(r#"helix.load("core.js");"#).unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(
        crate::host::with_state_or_init(|st| run_command(st, "core-run", &ctx)).unwrap(),
        "内置插件不该被能力检查拦住"
    );
    assert!(
        pending_permissions().is_empty(),
        "内置插件不该产生待授权:{:?}",
        pending_permissions()
    );
}

/// C-1:`schema_version` **要比一比** —— 高于本编辑器支持的声明面版本 ⇒ 加载即失败
/// (按不认识的语义跑比"加载失败"更糟)。
#[test]
fn plugin_schema_version_is_enforced() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 与编辑器同版 ⇒ 放行
    load_script(r#"helix.plugin("ok", { schema_version: 1, deps: [] });"#).unwrap();
    // 高于编辑器 ⇒ 抛错,且消息里有版本号与修法
    let err = load_script(r#"helix.plugin("future", { schema_version: 2, deps: [] });"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("schema_version 2"), "{err}");
    assert!(err.contains("高于本编辑器支持"), "{err}");
}

/// **回归**(用户报的 bug):启动时**打开了有路径的文件** ⇒ 内建 dashboard **不该**自动弹
/// (它会和文件选择器/编辑区叠在一起)。
///
/// 这条用例跑的是**真实的** `plugins/dashboard/plugin.js`(不是复刻的逻辑),判据来自
/// `startup` payload 里的 `doc.path` —— 早先那个 payload 是写死的空 ctx,处理器**看不到**
/// 到底有没有打开文件,所以只能无条件弹。
#[test]
fn dashboard_skips_startup_when_a_file_was_opened() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    // 把插件根指到仓库里的 plugins/(加载**真实**插件)
    let plugins = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../plugins"));
    set_plugins_dir(plugins);
    load_script(r#"helix.load("dashboard");"#).unwrap();
    take_ui_requests(); // 清掉加载期可能入队的请求

    let has_popup = |reqs: &[UiRequest]| {
        reqs.iter()
            .any(|r| matches!(r, UiRequest::OpenPopup(OpenPopupReq { .. })))
    };

    // ① 启动时打开了**有路径**的文件 ⇒ 不弹
    let opened = CommandContext {
        docs: vec![],
        path: Some("/tmp/opened.rs".into()),
        text: "fn main() {}\n".into(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    emit_startup(&opened, 0).unwrap();
    let reqs = take_ui_requests();
    assert!(
        !has_popup(&reqs),
        "带路径启动时 dashboard 不该弹(实测 bug:会和文件选择器叠在一起);reqs={reqs:?}"
    );

    // ② 无名空 buffer(启动时没打开文件)⇒ 弹
    let blank = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    emit_startup(&blank, 0).unwrap();
    let reqs = take_ui_requests();
    assert!(
        has_popup(&reqs),
        "无文件启动时 dashboard 应该弹(否则这个插件等于失效);reqs={reqs:?}"
    );

    // ③ 当前 buffer **无路径**,但命令行指定了东西(`hx <目录>` → 选择器浮层 /
    //    `--tutor` / 管道 stdin)⇒ 不弹。这是第二个实测 bug:只判 doc.path 会让
    //    dashboard 弹在文件选择器上面。
    emit_startup(&blank, 1).unwrap();
    let reqs = take_ui_requests();
    assert!(
        !has_popup(&reqs),
        "命令行指定了目录/tutor/stdin(open_paths>0)时 dashboard 不该弹;reqs={reqs:?}"
    );
}

/// `helix.remove_plugin_file` 是**写**操作(与 `write_plugin_file` 对称)⇒ 受 `fs_write` 检查。
/// 此前它被当成"限域所以安全"而漏掉检查点 —— 文档的能力表已经写了、代码却没做
/// (由 `check_capability_surface.mjs` 机械对账抓出来)。
#[test]
fn remove_plugin_file_needs_fs_write() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    set_plugins_dir(d.clone());
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    // ① 声明了 process/fs_read 但**没有 fs_write** ⇒ 被拒
    load_script(
        r#"
        helix.plugin("rm-probe", { permissions: { process: true, fs_read: true } });
        helix.register_command("rm-go", () => { helix.remove_plugin_file("x.js"); });
        "#,
    )
    .unwrap();
    let err = crate::host::with_state_or_init(|st| run_command(st, "rm-go", &ctx))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("未声明能力") && err.contains("fs_write"),
        "{err}"
    );
    let _ = take_messages(); // 清掉①那条"怎么授权"的提示
                             // ② 声明 fs_write ⇒ 放行(文件本来不存在 ⇒ false,幂等)
    load_script(
        r#"
        helix.plugin("rm-probe", { permissions: { fs_write: true } });
        helix.register_command("rm-go", () => {
            helix.echo("rm:" + helix.remove_plugin_file("x.js"));
        });
        "#,
    )
    .unwrap();
    assert!(crate::host::with_state_or_init(|st| run_command(st, "rm-go", &ctx)).unwrap_or(false));
    assert_eq!(take_messages(), vec!["rm:false"], "幂等删除应返回 false");
}

/// `helix.permissions()`(JS API):插件据此降级 —— 语义是"**不问就放行吗**"。
#[test]
fn js_permissions_query() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    // 声明了 process ⇒ process=true;没声明的 fs_read/fs_write ⇒ false(会被问一次)
    load_script(
        r#"
        helix.plugin("caps", { permissions: { process: true } });
        helix.register_command("caps-report", () => helix.echo(JSON.stringify(helix.permissions())));
        "#,
    )
    .unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    crate::host::with_state_or_init(|st| run_command(st, "caps-report", &ctx)).unwrap();
    let msg = take_messages();
    let v: serde_json::Value = serde_json::from_str(&msg[0]).expect("permissions() 应返回对象");
    assert_eq!(v["process"], serde_json::json!(true), "已声明 ⇒ 放行");
    assert_eq!(v["fs_read"], serde_json::json!(false), "未声明 ⇒ 会被问");
    assert_eq!(v["fs_write"], serde_json::json!(false));
    // 记住一个"允许"之后 ⇒ 查询跟着变(true)
    crate::permissions::decide("caps", "fs_read", true);
    crate::host::with_state_or_init(|st| run_command(st, "caps-report", &ctx)).unwrap();
    let msg = take_messages();
    let v: serde_json::Value = serde_json::from_str(&msg[0]).unwrap();
    assert_eq!(v["fs_read"], serde_json::json!(true), "决定后查询应为 true");

    // **没声明 permissions** 的插件 = 空能力集(fail-closed):三项都 false
    // (调用会被问一次,而不是静默放行 —— legacy 放行已废除)
    load_script(
        r#"
        helix.plugin("legacy-caps", { deps: [] });
        helix.register_command("legacy-report", () => helix.echo(JSON.stringify(helix.permissions())));
        "#,
    )
    .unwrap();
    crate::host::with_state_or_init(|st| run_command(st, "legacy-report", &ctx)).unwrap();
    let msg = take_messages();
    let v: serde_json::Value = serde_json::from_str(&msg[0]).unwrap();
    assert_eq!(
        (
            v["process"].clone(),
            v["fs_read"].clone(),
            v["fs_write"].clone()
        ),
        (
            serde_json::json!(false),
            serde_json::json!(false),
            serde_json::json!(false)
        ),
        "未声明 permissions = 空能力集（fail-closed）"
    );
}

/// `min_editor` 的语义是**警告并继续加载**(不是拒载 —— 与 `schema_version` 的区别:
/// 版本不匹配只说明插件可能用到新 API,代码本身仍可能正常;字段语义不同才必须拒)。
/// 编辑器版本未知 ⇒ 跳过检查(不误伤库用法/单测)。
#[test]
fn min_editor_warns_and_loads() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 与本 crate 其它用例共存:版本串用**真实形态**(>= 其它用例声明的 25.7)
    set_editor_version("25.7.1 (c4831272)");
    // 满足要求:加载成功,且**不**产生消息
    load_script(r#"helix.plugin("fine", { min_editor: "25.7", deps: [] });"#).unwrap();
    load_script(r#"helix.plugin("exact", { min_editor: "25.7.1", deps: [] });"#).unwrap();
    assert!(take_messages().is_empty(), "满足要求不该报警");
    // 高于当前版本:**加载仍然成功**,但状态栏拿到一条带原因与修法的提示
    load_script(r#"helix.plugin("future", { min_editor: "25.8", deps: [] });"#).unwrap();
    let msgs = take_messages();
    let m = msgs.join(" ");
    assert!(m.contains("需要编辑器 >= 25.8"), "{m}");
    assert!(m.contains("25.7.1"), "{m}");
    assert!(m.contains("已继续加载"), "{m}");
    // manifest 仍然被记下来(`:js-permission list` 能看到)
    let manifests = plugin_manifests();
    assert!(
        manifests
            .iter()
            .any(|x| x.name == "future" && x.min_editor.as_deref() == Some("25.8")),
        "min_editor 应被记录"
    );
}

/// C-2:能力检查覆盖**同族 API** —— 声明了 `process` 就能 `run_async`/`spawn`;
/// 声明了 `fs_read` 就能 `read_dir`;没声明的仍走"问一次"。
#[test]
fn capability_covers_sibling_apis() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    set_plugins_dir(d.clone());
    let main_src = r#"
        helix.plugin("cap", { permissions: { process: true } });
        helix.register_command("cap-read", () => { helix.read_dir("/"); });
        helix.register_command("cap-spawn", () => {
            helix.spawn({ pty: false, cmd: "true", onChunk: () => {}, onExit: () => {} });
        });
    "#;
    crate::host::with_state_or_init(|st| load_script_named(st, "cap", main_src).unwrap());
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    // process 已声明 ⇒ spawn 放行(不再被拒)
    assert!(crate::host::with_state_or_init(|st| run_command(st, "cap-spawn", &ctx)).unwrap());
    assert!(
        pending_permissions().is_empty(),
        "process 已声明,不该产生待授权"
    );
    // fs_read 未声明 ⇒ read_dir 走询问(同族:read_file/read_file_async/read_dir 同一能力)
    let err = crate::host::with_state_or_init(|st| run_command(st, "cap-read", &ctx))
        .unwrap_err()
        .to_string();
    assert!(err.contains("fs_read"), "{err}");
    assert!(
        pending_permissions()
            .iter()
            .any(|(p, c)| p == "cap" && c == "fs_read"),
        "read_dir 应记入 (cap, fs_read):{:?}",
        pending_permissions()
    );
}

#[test]
fn command_docs() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(
        r#"
    helix.register_command("doc1", () => {}, "first doc");
    helix.register_command("nodoc", () => {});
    helix.register_command("nulldoc", () => {}, undefined);
    "#,
    )
    .unwrap();
    assert_eq!(command_doc("doc1"), Some("first doc".to_string()));
    assert_eq!(command_doc("nodoc"), None);
    assert_eq!(command_doc("nulldoc"), None);
    assert_eq!(command_doc("missing"), None);
    // 非法 doc 类型 → 报错
    assert!(load_script(r#"helix.register_command("bad", () => {}, 42);"#).is_err());
}

#[test]
fn plugin_reload() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let st = crate::host::state_handle_or_init();
    load_script_named(
        &st,
        "a.js",
        r#"helix.register_command("reload-cmd", () => { helix.echo("v1"); });"#,
    )
    .unwrap();
    crate::host::with_state_or_init(|st| {
        load_script_named(st, "b.js", r#"helix.on("save", () => {});"#).unwrap()
    });

    assert!(has_handlers("save"));
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "reload-cmd", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["v1"]);

    // reload：清空状态后重跑全部已记录脚本
    crate::host::with_state_or_init(reload_all).unwrap();
    assert!(has_handlers("save"), "handlers re-registered after reload");
    assert!(crate::host::with_state_or_init(|st| run_command(st, "reload-cmd", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["v1"]);
    let _ = take_ui_requests(); // 清 reload_all 入队的 ClosePanel
}

/// 热重载不继承旧装饰:reload_all 在 reset 后 push 全局 Clear(doc: None),
/// 使 term 侧已应用的旧装饰在下一帧被清空。
#[test]
fn decorations_cleared_on_reload() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(r#"helix.register_command("dec-dummy", () => {});"#).unwrap();
    // load_script 自身会 push 一个全局 Clear(见 decorations_cleared_on_script_load),先取走
    assert_eq!(take_decorations().len(), 1);
    assert!(take_decorations().is_empty());
    crate::host::with_state_or_init(reload_all).unwrap();
    let reqs = take_decorations();
    assert!(
        reqs.iter()
            .any(|r| r.doc.is_none() && matches!(r.kind, crate::types::DecorationKind::Clear)),
        "reload 后应有全局 Clear: {reqs:?}"
    );
}

#[test]
#[ignore = "手动跑(--ignored):会复制仓库 plugins/ 并做一次 reload,较慢;默认套件不跑"]
fn reload_keeps_plugins_with_deps_and_hooks() {
    // 模拟真实 init.js 场景:相对名 + deps 声明 + set_statusline + 命令 + 事件
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir); // 线程池复用,目录泄漏保确定性(同文件其他测试惯例)
    set_plugins_dir(d.clone());
    std::fs::create_dir_all(d.join("lib")).unwrap();
    std::fs::create_dir_all(d.join("features")).unwrap();
    std::fs::write(
        d.join("lib/icons.js"),
        r#"helix.plugin("icons", { deps: [] }); helix.export({ src: "icons" });"#,
    )
    .unwrap();
    std::fs::write(
        d.join("features/statusline.js"),
        r#"helix.plugin("statusline", { deps: ["lib/icons.js"] });
           const IC = helix.load("lib/icons.js");
           helix.set_statusline(() => "SL" + (IC ? "-" + IC.src : ""));"#,
    )
    .unwrap();
    std::fs::write(
        d.join("features/filetree.js"),
        r#"helix.plugin("filetree", { deps: ["lib/icons.js"] });
           helix.register_command("ft", () => helix.echo("ft-ok"));"#,
    )
    .unwrap();
    // 模拟 init.js:绝对路径名加载(application.rs 同款),脚本内依赖也用绝对路径(PLUGINS_DIR 全局共享,不依赖)
    let init_path = d.join("init.js");
    let icons_abs = d.join("lib/icons.js").to_string_lossy().into_owned();
    let init_src = format!(
        r#"helix.load({statusline:?}); helix.load({filetree:?});"#,
        statusline = d.join("features/statusline.js").to_string_lossy(),
        filetree = d.join("features/filetree.js").to_string_lossy(),
    );
    // 脚本内依赖改绝对路径:先重写两个 feature 文件的 load 行
    let sl = std::fs::read_to_string(d.join("features/statusline.js")).unwrap();
    std::fs::write(
        d.join("features/statusline.js"),
        sl.replace("\"lib/icons.js\"", &format!("\"{icons_abs}\"")),
    )
    .unwrap();
    let ft = std::fs::read_to_string(d.join("features/filetree.js")).unwrap();
    std::fs::write(
        d.join("features/filetree.js"),
        ft.replace("\"lib/icons.js\"", &format!("\"{icons_abs}\"")),
    )
    .unwrap();
    crate::host::with_state_or_init(|st| {
        load_script_named(st, &init_path.display().to_string(), &init_src).unwrap()
    });
    // 模拟启动时的相对名记录:load 已把 features/* 记录为相对名
    let ctx = CommandContext {
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: vec![],
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "ft", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["ft-ok"]);
    let _ = take_ui_requests();
    // reload:重跑全部(init.js + features/*)
    crate::host::with_state_or_init(reload_all).unwrap();
    assert!(
        crate::host::with_state_or_init(|st| run_command(st, "ft", &ctx)).unwrap(),
        "filetree 命令 reload 后仍可用"
    );
    assert_eq!(take_messages(), vec!["ft-ok"]);
    let _ = take_ui_requests();
}

#[test]
fn reload_after_init_midway_failure_keeps_dependent_plugins() {
    // 模拟用户场景:init.js 第一个插件(terminal)reload 失败 → init.js eval 中断,
    // 但循环继续单独 eval 后续 features/*——它们依赖 icons(缓存被 reset 清空),
    // 重新 eval icons = 嵌套 eval,可能污染后续闭包。验证命令回调完好。
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    set_plugins_dir(d.clone());
    std::fs::create_dir_all(d.join("lib")).unwrap();
    std::fs::create_dir_all(d.join("features")).unwrap();
    std::fs::write(
        d.join("lib/icons.js"),
        r#"helix.plugin("icons", { deps: [] }); helix.export({ src: "icons" });"#,
    )
    .unwrap();
    std::fs::write(
        d.join("features/bad.js"),
        r#"helix.register_command("bad", () => {});"#,
    )
    .unwrap();
    std::fs::write(
        d.join("features/plugin_a.js"),
        r#"helix.plugin("plugin_a", { deps: ["lib/icons.js"] });
           const IC = helix.load("lib/icons.js");
           helix.register_command("pa", () => helix.echo("pa:" + (IC && IC.src)));"#,
    )
    .unwrap();
    let bad_abs = d.join("features/bad.js").to_string_lossy().into_owned();
    let a_abs = d
        .join("features/plugin_a.js")
        .to_string_lossy()
        .into_owned();
    // PLUGINS_DIR 若已被其他测试占用,plugin_a 内部相对名 load 会失败——改写为绝对路径
    let icons_abs = d.join("lib/icons.js").to_string_lossy().into_owned();
    let a = std::fs::read_to_string(d.join("features/plugin_a.js")).unwrap();
    std::fs::write(
        d.join("features/plugin_a.js"),
        a.replace("\"lib/icons.js\"", &format!("\"{icons_abs}\"")),
    )
    .unwrap();
    let init_path = d.join("init.js");
    let init_src = format!(
        r#"helix.load({bad:?}); helix.load({a:?});"#,
        bad = bad_abs,
        a = a_abs
    );
    crate::host::with_state_or_init(|st| {
        load_script_named(st, &init_path.display().to_string(), &init_src).unwrap()
    });
    let ctx = CommandContext {
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: vec![],
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "pa", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["pa:icons"]);
    let _ = take_ui_requests();
    // 磁盘 bad 改坏版 → reload:init.js 在 bad 处失败,plugin_a 单独 eval
    std::fs::write(
        d.join("features/bad.js"),
        r#"helix.register_command("bad", () => { BAD SYNTAX"#,
    )
    .unwrap();
    let err = crate::host::with_state_or_init(reload_all).unwrap_err();
    assert!(err.to_string().contains("failed"), "应有失败汇总: {err}");
    let _ = take_messages();
    let _ = take_ui_requests();
    assert!(
        crate::host::with_state_or_init(|st| run_command(st, "pa", &ctx)).is_ok(),
        "plugin_a 命令 reload 后应可调用"
    );
    let msgs = take_messages();
    assert_eq!(
        msgs.first().map(|s| s.as_str()),
        Some("pa:icons"),
        "回调结果应正确(未被污染),got {msgs:?}"
    );
}

#[test]
#[ignore = "手动跑(--ignored):会复制仓库 plugins/ 并做一次 reload,较慢;默认套件不跑"]
fn reload_users_real_plugins_dir() {
    // 复制用户真实 ~/.config/helix/plugins 到 tempdir,用用户 init.js 序列 reload。
    // 用户 filetree 是改版(offset scroll)+ 有 test.js——最接近真实环境。
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let home = std::env::var("HOME").unwrap_or_default();
    let user_plugins = std::path::PathBuf::from(&home).join(".config/helix/plugins");
    if !user_plugins.join("features/filetree/index.js").is_file() {
        return; // 无用户环境(CI)跳过
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            let to = dst.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                std::fs::create_dir_all(&to).unwrap();
                copy_dir(&e.path(), &to);
            } else {
                std::fs::copy(e.path(), to).unwrap();
            }
        }
    }
    copy_dir(&user_plugins, &d);
    set_plugins_dir(d.clone());
    // 用户 init.js(无 terminal——用户已禁用)
    let init_path = d.join("init.js");
    let init_src = r#"helix.load("features/statusline.js");
helix.load("features/filetree/index.js");
helix.load("features/which-key.js");
helix.load("features/picker.js")
helix.map("normal", "space-f", () => helix.picker.run("files"));"#;
    let r = crate::host::with_state_or_init(|st| {
        load_script_named(st, &init_path.display().to_string(), init_src)
    });
    if let Err(e) = &r {
        panic!("init load failed: {e}");
    }
    let _ = take_messages();
    let _ = take_ui_requests();
    if let Err(e) = crate::host::with_state_or_init(reload_all) {
        panic!("reload_all failed: {e}");
    }
    let _ = take_messages();
    let _ = take_ui_requests();
    let ctx = CommandContext {
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: vec![],
    };
    assert!(
        crate::host::with_state_or_init(|st| run_command(st, "filetree", &ctx)).is_ok(),
        "用户 filetree 命令 reload 后应可调用"
    );
}

#[test]
#[ignore = "手动跑(--ignored):会复制仓库 plugins/ 并做一次 reload,较慢;默认套件不跑"]
fn reload_missing_relative_load_then_dependent_pollution() {
    // 用户场景:init.js 里 load 一个不存在的相对名文件(如"禁用"后仍留 load 行的 terminal.js)
    // → init.js eval 失败 → 循环单独 eval 后续 features/* → 它们相对名 load icons(重新 eval=嵌套)
    // → 后续注册的闭包可能被 boa 嵌套 eval 污染。验证命令回调结果。
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    set_plugins_dir(d.clone());
    std::fs::create_dir_all(d.join("lib")).unwrap();
    std::fs::create_dir_all(d.join("features")).unwrap();
    std::fs::write(
        d.join("lib/icons.js"),
        r#"helix.plugin("icons", { deps: [] }); helix.export({ src: "icons" });"#,
    )
    .unwrap();
    // plugin_a:相对名依赖 icons,注册命令回调读 ICONS
    std::fs::write(
        d.join("features/plugin_a.js"),
        r#"helix.plugin("plugin_a", { deps: ["lib/icons.js"] });
           const IC = helix.load("lib/icons.js");
           helix.register_command("pa", () => helix.echo("pa:" + (IC && IC.src)));"#,
    )
    .unwrap();
    // init.js:load 不存在的 terminal.js(用户"禁用"方式)+ plugin_a
    let init_path = d.join("init.js");
    std::fs::write(
        &init_path,
        r#"helix.load("features/terminal.js"); helix.load("features/plugin_a.js");"#,
    )
    .unwrap();
    // 启动模拟:load_script_named 绝对路径 init.js —— 但 init.js 内部 load 相对名,
    // 依赖 PLUGINS_DIR(独占,有效)
    let init_abs = init_path.display().to_string();
    // 启动时 terminal.js 不存在 → load 失败 → init.js 加载失败(用户启动时 terminal 存在,reload 时删了?)
    // 更贴近:先让 terminal 存在(启动 OK),reload 前删掉 → reload 时 init.js 重跑失败
    std::fs::write(
        d.join("features/terminal.js"),
        r#"helix.register_command("term", () => {});"#,
    )
    .unwrap();
    let init_src = std::fs::read_to_string(&init_path).unwrap();
    crate::host::with_state_or_init(|st| load_script_named(st, &init_abs, &init_src).unwrap());
    let ctx = CommandContext {
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: vec![],
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "pa", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["pa:icons"]);
    let _ = take_ui_requests();
    // reload 前删 terminal.js(用户"禁用"= 删文件但 init.js 留着 load)→ reload 重跑 init.js 失败
    std::fs::remove_file(d.join("features/terminal.js")).unwrap();
    let err = crate::host::with_state_or_init(reload_all).unwrap_err();
    assert!(
        err.to_string().contains("terminal.js"),
        "应报 terminal.js 缺失: {err}"
    );
    let _ = take_messages();
    let _ = take_ui_requests();
    // plugin_a 单独 eval(init.js 中断后)命令回调应完好
    assert!(crate::host::with_state_or_init(|st| run_command(st, "pa", &ctx)).is_ok());
    let msgs = take_messages();
    assert_eq!(
        msgs.first().map(|s| s.as_str()),
        Some("pa:icons"),
        "回调未被污染, got {msgs:?}"
    );
}

#[test]
fn reload_failed_script_does_not_block_others() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    set_plugins_dir(d.clone());
    // 先写好版并加载;之后磁盘改坏版 → reload 重读磁盘时失败
    std::fs::write(
        d.join("bad.js"),
        r#"helix.register_command("bad", () => {});"#,
    )
    .unwrap();
    std::fs::write(
        d.join("good.js"),
        r#"helix.register_command("good", () => helix.echo("still-here"));"#,
    )
    .unwrap();
    // 绝对路径加载(PLUGINS_DIR 全局共享不可靠)
    let bad_abs = d.join("bad.js").to_string_lossy().into_owned();
    let good_abs = d.join("good.js").to_string_lossy().into_owned();
    let st = crate::host::state_handle_or_init();
    load_script_named(
        &st,
        &bad_abs,
        &std::fs::read_to_string(d.join("bad.js")).unwrap(),
    )
    .unwrap();
    let st = crate::host::state_handle_or_init();
    load_script_named(
        &st,
        &good_abs,
        &std::fs::read_to_string(d.join("good.js")).unwrap(),
    )
    .unwrap();
    let ctx = CommandContext {
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: vec![],
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "good", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["still-here"]);
    // 坏脚本名:bad.js(相对名判断用;磁盘读按绝对路径)
    std::fs::write(
        d.join("bad.js"),
        r#"helix.register_command("bad", () => { THIS IS NOT VALID"#,
    )
    .unwrap();
    // reload:bad 磁盘版失败,不中止 good
    let err = crate::host::with_state_or_init(reload_all).unwrap_err();
    assert!(
        err.to_string().contains("1 plugin(s) failed"),
        "汇总错误: {err}"
    );
    assert!(err.to_string().contains("bad.js"), "错误含脚本名: {err}");
    let _ = take_ui_requests();
    assert!(
        crate::host::with_state_or_init(|st| run_command(st, "good", &ctx)).is_ok(),
        "good 脚本 reload 后仍可用(不被 bad 阻塞)"
    );
}

#[test]
fn circular_runtime_load_errors_not_crash() {
    // 回归:跨脚本运行时 helix.load 循环(A load B,B load A)曾无限嵌套 eval 递归
    // → 栈溢出 SIGABRT。js_load 每次新建空栈导致循环检测失效;现在共享加载栈,
    // 循环应报错退出而非崩溃。用绝对路径避免占用全局 PLUGINS_DIR(与其他测试并行)。
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    std::fs::create_dir_all(d.join("features")).unwrap();
    let a_abs = d.join("features/a.js").to_string_lossy().into_owned();
    let b_abs = d.join("features/b.js").to_string_lossy().into_owned();
    let a_src = format!(r#"const B = helix.load("{b_abs}"); helix.export(() => B);"#);
    let b_src = format!(r#"const A = helix.load("{a_abs}"); helix.export(() => A);"#);
    std::fs::write(d.join("features/a.js"), a_src).unwrap();
    std::fs::write(d.join("features/b.js"), b_src).unwrap();
    let init_path = d.join("init.js");
    let init_src = format!(r#"helix.load("{a_abs}");"#);
    std::fs::write(&init_path, init_src).unwrap();
    let st = crate::host::state_handle_or_init();
    let r = load_script_named(
        &st,
        &init_path.display().to_string(),
        &std::fs::read_to_string(&init_path).unwrap(),
    );
    let msg = format!("{r:?}");
    assert!(r.is_err(), "循环 load 应报错退出而非崩溃, got {msg}");
    assert!(msg.contains("circular"), "错误应说明循环依赖, got {msg}");
}

#[test]
#[ignore = "手动跑(--ignored):会复制仓库 plugins/ 并做一次 reload,较慢;默认套件不跑"]
fn reload_all_real_plugins_keeps_commands() {
    // 加载仓库全部真实插件(init.js 同款序列)+ reload,验证命令/钩子保留
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // PLUGINS_DIR 是全局 OnceLock:被其他测试(如 plugin_deps_auto_load,字母序在前)占住后
    // 本测试的相对名解析失效——跳过(核心 reload 行为由 reload_keeps/reload_failed 覆盖)
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    // 复制仓库 plugins/ 到临时目录
    fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            let to = dst.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                std::fs::create_dir_all(&to).unwrap();
                copy_dir(&e.path(), &to);
            } else {
                std::fs::copy(e.path(), to).unwrap();
            }
        }
    }
    copy_dir(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../plugins")),
        &d,
    );
    set_plugins_dir(d.clone());
    // 模拟 init.js:裸名加载(=`<name>/plugin.js`,与 plugins/init.js 同一套名字)。
    // 2026-10 起插件是目录布局,旧的 features/*.js 扁平布局已不存在。
    let init_src = r#"helix.load("terminal");
helix.load("statusline");
helix.load("filetree");
helix.load("which-key");
helix.load("tabbar");
helix.map("normal", "space-f", () => helix.picker.run("files"));"#;
    let init_path = d.join("init.js");
    let r = crate::host::with_state_or_init(|st| {
        load_script_named(st, &init_path.display().to_string(), init_src)
    });
    if let Err(e) = &r {
        panic!("init load failed: {e}");
    }
    let _ = take_messages();
    let _ = take_ui_requests();
    // reload
    if let Err(e) = crate::host::with_state_or_init(reload_all) {
        panic!("reload_all failed: {e}");
    }
    let _ = take_messages();
    let _ = take_ui_requests();
    // 验证:filetree 命令在(真实插件注册的命令)
    let ctx = CommandContext {
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: vec![],
    };
    assert!(
        crate::host::with_state_or_init(|st| run_command(st, "filetree", &ctx)).is_ok(),
        "filetree 命令 reload 后应可调用"
    );
}

#[test]
#[ignore = "手动跑(--ignored):会复制仓库 plugins/ 并做一次 reload,较慢;默认套件不跑"]
fn reload_real_statusline_render_ok() {
    // 用户场景:statusline.js 依赖 icons,reload 后 render(状态栏渲染)不应抛错/返回空
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    set_plugins_dir(d.clone());
    std::fs::create_dir_all(d.join("statusline")).unwrap();
    // 复制仓库 statusline 插件(2026-10 起的布局:plugins/statusline/plugin.js)。
    // 图标已收进核心(helix-js/src/icons.rs),statusline 的 deps 是空的 ⇒ 不再拷 icons。
    let repo = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../plugins"));
    std::fs::copy(
        repo.join("statusline/plugin.js"),
        d.join("statusline/plugin.js"),
    )
    .unwrap();
    let src = std::fs::read_to_string(d.join("statusline/plugin.js")).unwrap();
    let r =
        crate::host::with_state_or_init(|st| load_script_named(st, "statusline/plugin.js", &src));
    eprintln!("load statusline result: {r:?}");
    r.unwrap();
    let _ = take_messages();
    let hook_set = crate::state::with_statusline_hook_current(|h| h.is_some());
    eprintln!("statusline hook registered: {hook_set}");
    crate::state::with_engine(|_e| {
        let callable = crate::state::with_statusline_hook_current(|h| {
            h.as_ref().map(|v| v.as_callable().is_some())
        });
        eprintln!("hook callable: {callable:?}");
    });
    let ctx = statusline_ctx();
    // reload 前 render 正常
    let before = crate::host::with_state_or_init(|st| statusline_parts(st, &ctx));
    eprintln!("reload 前 statusline_parts: {before:?}");
    assert!(
        before.is_some() && !before.unwrap().is_empty(),
        "reload 前 statusline 应有内容"
    );
    // reload
    crate::host::with_state_or_init(reload_all).unwrap();
    let _ = take_messages();
    let _ = take_ui_requests();
    // reload 后 render 应正常(非空,不抛错)
    let after = crate::host::with_state_or_init(|st| statusline_parts(st, &ctx));
    assert!(after.is_some(), "reload 后 statusline render 不应抛错");
    assert!(!after.unwrap().is_empty(), "reload 后 statusline 应有内容");
}

#[test]
fn reload_closes_open_panel_layer() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 面板由命令打开（不在加载时），保证 reload 重跑脚本不会自动重开面板
    load_script(
        r#"helix.register_command("open-panel", () => { helix.open_panel({ side: "right", size: 30, render: () => ["p1"] }); });"#,
    )
    .unwrap();
    let ctx = CommandContext {
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: vec![],
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "open-panel", &ctx)).unwrap());
    assert!(matches!(
        take_ui_requests()[0],
        UiRequest::OpenPanel(OpenPanelReq { .. })
    ));

    // reload：面板层还挂在 compositor 上 → 必须入队 ClosePanel 供 apply_ui_requests 移除
    crate::host::with_state_or_init(reload_all).unwrap();
    let reqs = take_ui_requests();
    assert!(
        matches!(&reqs[0], UiRequest::ClosePanel(ClosePanelReq { .. })),
        "layer removed after reload"
    );
    // LAST_PANEL_ID 已清空 → :panel-close 不再误报有面板
    assert!(close_last_panel().is_err());
}

#[test]
fn plugin_reload_top_level_const() {
    // 顶层 const/let 脚本 reload 必须成功：IIFE 包裹下重跑获得全新词法作用域
    // （无 IIFE 时全局词法环境重复声明会抛 SyntaxError: duplicate lexical declaration）
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let st = crate::host::state_handle_or_init();
    load_script_named(
        &st,
        "c.js",
        r#"const X = 1; helix.register_command("ccmd", () => helix.echo("ok"));"#,
    )
    .unwrap();

    crate::host::with_state_or_init(reload_all).unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "ccmd", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["ok"]);
}

/// 方案 2 依赖清单：helix.plugin deps 自动拓扑加载（先依赖后自身）、
/// 已加载去重、循环依赖报错。不碰 set_plugins_dir（OnceLock 全局，污染后续测试），
/// 全用绝对路径。
#[test]
fn plugin_deps_auto_load() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let dir = tempfile::tempdir().unwrap();
    // ponytail: 泄漏 tempdir（同文件其他测试同款）——线程池复用线程，
    // 目录被删会误伤后续 load；泄漏几个 /tmp 小文件换确定性。
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    std::fs::create_dir_all(d.join("features")).unwrap();
    std::fs::create_dir_all(d.join("lib")).unwrap();
    std::fs::write(
        d.join("lib/icons.js"),
        r#"helix.plugin("icons", { deps: [] }); helix.export({ src: "icons" });"#,
    )
    .unwrap();
    std::fs::write(
        d.join("features/feat.js"),
        format!(
            r#"helix.plugin("feat", {{ deps: [{icons:?}] }}); helix.export({{ src: "feat" }});"#,
            icons = d.join("lib/icons.js").to_string_lossy()
        ),
    )
    .unwrap();
    let icons_abs = d.join("lib/icons.js").to_string_lossy().into_owned();
    let feat_abs = d.join("features/feat.js").to_string_lossy().into_owned();
    let script = format!(
        r#"
    helix.register_command("deps-run", () => {{
        const feat = helix.load({feat:?});
        const icons = helix.load({icons:?});
        helix.echo("feat:" + feat.src + " icons:" + icons.src + " same:" + (feat === helix.load({feat:?})));
    }});
    "#,
        feat = feat_abs,
        icons = icons_abs,
    );
    crate::host::with_state_or_init(|st| load_script_named(st, "driver.js", &script).unwrap());
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "deps-run", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["feat:feat icons:icons same:true"]);

    // 循环依赖：a deps b，b deps a → 报错
    let a_abs = d.join("a.js").to_string_lossy().into_owned();
    let b_abs = d.join("b.js").to_string_lossy().into_owned();
    let a_script = format!(r#"helix.plugin("a", {{ deps: [{b:?}] }});"#, b = b_abs);
    let b_script = format!(r#"helix.plugin("b", {{ deps: [{a:?}] }});"#, a = a_abs);
    std::fs::write(d.join("a.js"), a_script).unwrap();
    std::fs::write(d.join("b.js"), b_script).unwrap();
    let cyc_script = format!(
        r#"
    helix.register_command("cyc-run", () => {{
        try {{
            helix.load({a:?});
            helix.echo("no-cycle");
        }} catch (e) {{
            helix.echo("cycle:" + String(e));
        }}
    }});
    "#,
        a = a_abs,
    );
    crate::host::with_state_or_init(|st| load_script_named(st, "cyc.js", &cyc_script).unwrap());
    assert!(crate::host::with_state_or_init(|st| run_command(st, "cyc-run", &ctx)).unwrap());
    assert!(
        take_messages()[0].contains("circular dependency"),
        "循环依赖应报错"
    );
}

/// 统一入口：load/export 往返 + 缓存、lazy 桩、run_command 带 ctx、未知文件报错。
/// set_plugins_dir 是进程全局——测试用临时目录隔离。
// ponytail: tempdir 不 drop（std::mem::forget）——线程池复用线程，后续 reload 测试
// 会在本线程重跑 LOADED_SCRIPTS（含 init.js→load("exp.js")），目录被删会误伤；
// 泄漏几个 /tmp 小文件换确定性。
#[test]
fn entry_load_export_lazy() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let dir = tempfile::tempdir().unwrap();
    set_plugins_dir(dir.path().to_path_buf());

    // 导出 + 加载往返
    std::fs::write(
        dir.path().join("exp.js"),
        r#"helix.export({ a: 1, b: "x" });"#,
    )
    .unwrap();
    crate::host::with_state_or_init(|st| {
        load_script_named(st, "init.js", r#"helix.load("exp.js");"#).unwrap()
    });
    // init.js 的 load 本身无法断言返回值——直接测 js_load 路径：
    // 用 helix.run_command 间接：注册命令调用 load 并把结果 echo 出来
    let st = crate::host::state_handle_or_init();
    load_script_named(
        &st,
        "driver.js",
        r#"
    helix.register_command("load-exp", () => {
        const mod = helix.load("exp.js");
        helix.echo("a:" + mod.a + " b:" + mod.b);
    });
    helix.register_command("load-cached", () => {
        const m1 = helix.load("exp.js");
        const m2 = helix.load("exp.js");
        helix.echo("same:" + (m1 === m2));
    });
    "#,
    )
    .unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "load-exp", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["a:1 b:x"]);
    assert!(crate::host::with_state_or_init(|st| run_command(st, "load-cached", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["same:true"]);

    // lazy：桩首次调用时加载 + 转执行
    std::fs::write(
        dir.path().join("lazy.js"),
        r#"helix.register_command("lazy-cmd", () => { helix.echo("lazy-ran"); });"#,
    )
    .unwrap();
    crate::host::with_state_or_init(|st| {
        load_script_named(
            st,
            "lazy-driver.js",
            r#"helix.lazy("lazy.js", "lazy-cmd");"#,
        )
        .unwrap()
    });
    assert!(crate::host::with_state_or_init(|st| run_command(st, "lazy-cmd", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["lazy-ran"]);

    // run_command 带 ctx：命令读 ctx.cursor
    let st = crate::host::state_handle_or_init();
    load_script_named(
        &st,
        "rc.js",
        r#"
    helix.register_command("where", (c) => { helix.echo("at:" + c.cursor.row + "," + c.cursor.col); });
    "#,
    )
    .unwrap();
    let st = crate::host::state_handle_or_init();
    load_script_named(
        &st,
        "rc-driver.js",
        r#"helix.register_command("call-where", () => { helix.run_command("where", { cursor: { row: 3, col: 7 } }); });"#,
    )
    .unwrap();
    assert!(crate::host::with_state_or_init(|st| run_command(st, "call-where", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["at:3,7"]);

    // 校验：未知文件 → 抛错
    let st = crate::host::state_handle_or_init();
    load_script_named(
        &st,
        "bad-driver.js",
        r#"helix.register_command("bad-load", () => { helix.load("nope.js"); });"#,
    )
    .unwrap();
    assert!(crate::host::with_state_or_init(|st| run_command(st, "bad-load", &ctx)).is_err());

    std::mem::forget(dir);
}

/// **deps 的库算"引用它的插件的一部分"**:`deps: ["lib/x.js"]` 的顶层代码按**父插件**
/// 判定,而不是"没有属主 ⇒ 拒绝"。
///
/// 为什么值得一条:加载顺序是 eval 本脚本 → 递归加载 deps,属主若在 eval 后就恢复,
/// 库的顶层代码会拿到 None(被拒),而它明明属于那个声明了能力的插件。
#[test]
fn dep_library_inherits_parent_plugin_owner() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::mem::forget(dir);
    set_plugins_dir(d.clone());
    // deps 的 key 相对**插件根**解析(与仓库布局一致:plugins/lib/*.js 与 plugins/<name>/ 平级)
    std::fs::create_dir_all(d.join("lib")).unwrap();
    std::fs::create_dir_all(d.join("dep")).unwrap();
    // 库的**顶层**就调受保护 API(读目录)
    std::fs::write(d.join("lib/x.js"), r#"const seen = helix.read_dir(".");"#).unwrap();
    std::fs::write(
        d.join("dep/plugin.js"),
        r#"helix.plugin("dep-owner", { deps: ["lib/x.js"], permissions: { fs_read: true } });"#,
    )
    .unwrap();

    // 父插件声明了 fs_read ⇒ 库的顶层调用放行
    load_script(r#"helix.load("dep");"#).expect("库应继承父插件的 fs_read 能力");

    // 反过来:父插件没声明 ⇒ 库也被拒(错误里要能看出是哪个能力)。
    // 用另一个库文件:同一个 key 已被上面的成功加载缓存(helix.load 的语义),不会重跑。
    crate::permissions::reset_for_test();
    std::fs::create_dir_all(d.join("dep2")).unwrap();
    std::fs::write(d.join("lib/y.js"), r#"const seen = helix.read_dir(".");"#).unwrap();
    std::fs::write(
        d.join("dep2/plugin.js"),
        r#"helix.plugin("dep-owner-2", { deps: ["lib/y.js"], permissions: {} });"#,
    )
    .unwrap();
    let err = load_script(r#"helix.load("dep2");"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("fs_read"), "错误应指出缺的能力: {err}");
}

/// `:plugin-unload <name>` 的核心语义:**只**回收该插件注册的东西。
///
/// 两个插件同时在场(审查重点 1:属主判据写错就会误删另一个的键位/钩子),
/// 并且 A 的产物覆盖全部六类(命令 / 事件 / 钩子 / 键位 / 主题 / 弹窗)。
#[test]
fn unload_removes_only_that_plugins_registrations() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    crate::host::with_state_or_init(|st| {
        load_script_named(
            st,
            "a.js",
            r##"helix.plugin("a", { onUnload: () => helix.echo("bye-a") });
helix.register_command("a-cmd", () => { helix.echo("a"); });
helix.register_command("a-cmd2", () => {});
helix.on("mode-change", () => { helix.echo("a-event"); });
helix.on("save", () => { helix.echo("a-save"); });
helix.set_statusline(() => "A");
helix.map("normal", "C-b", "a-cmd");
helix.set_theme({ "ui.text": "#ff0000" });
helix.open_popup({ render: () => ["popup"] });"##,
        )
        .unwrap()
    });
    crate::host::with_state_or_init(|st| {
        load_script_named(
            st,
            "b.js",
            r##"helix.plugin("b", {});
helix.register_command("b-cmd", () => {});
helix.on("mode-change", () => { helix.echo("b-event"); });
helix.map("normal", "C-c", "b-cmd");
helix.set_theme({ "ui.text.alt": "#00ff00" });"##,
        )
        .unwrap()
    });
    // popup 的 render 回调属主也绑上了
    let popup_count =
        crate::host::with_state_or_init(|st| crate::state::with_popups(st, |m| m.len()));
    assert_eq!(popup_count, 1, "A 的弹窗注册在案");

    let report = crate::host::with_state_or_init(|st| unload_plugin(st, "a"));

    assert_eq!(report.commands, 2, "A 的两条命令");
    assert_eq!(report.handlers, 2, "A 的两个事件处理器");
    assert_eq!(report.hooks, 1, "A 的状态栏钩子");
    assert_eq!(report.keymaps, 1, "A 的一条键位");
    assert_eq!(report.themes, 1, "A 的一个主题 scope");
    assert_eq!(report.popups, 1, "A 的弹窗");

    // 1) onUnload 先跑(消息队列可见)
    let msgs = take_messages();
    assert!(
        msgs.iter().any(|m| m == "bye-a"),
        "onUnload 应被调用: {msgs:?}"
    );

    // 2) A 的东西没了,B 的都在(审查重点 1)
    let names = command_names();
    assert!(!names.contains(&"a-cmd".to_string()), "A 的命令应消失");
    assert!(!names.contains(&"a-cmd2".to_string()));
    assert!(names.contains(&"b-cmd".to_string()), "B 的命令必须还在");

    // 3) 事件处理器:只剩 B 那个,且属主表与列表**等长**(没按下标删错)
    let (handlers, owners) = crate::host::with_state_or_init(|st| {
        (
            crate::state::with_event_handlers(st, |m| {
                m.get("mode-change").map(|v| v.len()).unwrap_or(0)
            }),
            crate::permissions::handler_owner_len("mode-change"),
        )
    });
    assert_eq!(handlers, 1, "只剩 B 的 mode-change 处理器");
    assert_eq!(owners, 1, "handler_owners 必须与处理器列表等长");
    assert_eq!(
        crate::permissions::handler_owner("mode-change", 0).as_deref(),
        Some("b"),
        "存活的处理器属主应是 B(删错人就会变成 a)"
    );
    assert_eq!(
        crate::host::with_state_or_init(|st| {
            crate::state::with_event_handlers(st, |m| m.get("save").map(|v| v.len()).unwrap_or(0))
        }),
        0,
        "A 独占的 save 事件应清空"
    );

    // 4) 钩子:状态栏槽(A 独占)已清空,属主也摘了
    assert!(
        crate::host::with_state_or_init(|st| {
            crate::state::with_statusline_hook(st, |h| h.is_none())
        }),
        "A 的状态栏钩子应被清空"
    );
    assert_eq!(
        crate::permissions::callback_owner(&crate::permissions::CbKey::hook(
            crate::permissions::CB_HOOK_STATUSLINE
        )),
        None,
        "钩子的属主条目也应摘掉"
    );

    // 5) 键位/主题:只回收 A 的
    assert_eq!(
        crate::host::with_state_or_init(|st| crate::state::with_keymap_owners(st, |m| m.len())),
        1,
        "A 的键位摘掉后只剩 B 的一条"
    );
    let ov = theme_overrides();
    assert!(ov.contains_key("ui.text.alt"), "B 的主题覆盖还在");
    assert!(!ov.contains_key("ui.text"), "A 的主题覆盖已摘");

    // 6) 入队:一条解绑 + 一条关闭弹窗
    let reqs = take_ui_requests();
    assert!(
        reqs.iter()
            .any(|r| matches!(r, UiRequest::MapKey(m) if m.unbind && m.key == "C-b")),
        "A 的键位应入队解绑"
    );
    assert!(
        reqs.iter().any(|r| matches!(r, UiRequest::ClosePopup(_))),
        "A 的弹窗应入队关闭"
    );
}

/// `unload_orphans`:只回收**没有属主**的注册,有属主的插件产物一条不动。
/// `reload_all` 用它接替旧的"整表清空"(那个 fn 已删)。
///
/// 真实的无主来源:共享库文件(`lib/x.js`)里注册命令但没 `helix.plugin`,属主表里没有它。
/// 测试用 `forget_command_owner` 直接造出这个状态(比造一条真实的 lib 加载链更短、
/// 断言更直接)。
#[test]
fn unload_orphans_only_removes_unowned_registrations() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    crate::host::with_state_or_init(|st| {
        load_script_named(
            st,
            "owned.js",
            r#"helix.plugin("owned", {});
helix.register_command("owned-cmd", () => {});
helix.register_command("orphan-cmd", () => {});
helix.on("save", () => {});
helix.set_statusline(() => "owned");"#,
        )
        .unwrap()
    });
    // 把 orphan-cmd 的属主抹掉(事件处理器与钩子保持有主)
    crate::permissions::forget_command_owner("orphan-cmd");

    let removed = crate::host::with_state_or_init(crate::commands::unload_orphans);
    assert_eq!(removed, 1, "只有那一条无主命令该被回收");

    let names = command_names();
    assert!(!names.contains(&"orphan-cmd".to_string()), "无主命令应消失");
    assert!(
        names.contains(&"owned-cmd".to_string()),
        "有属主的命令必须留着"
    );
    // 有主的事件处理器与钩子一条不动(不越界)
    assert_eq!(
        crate::host::with_state_or_init(|st| {
            crate::state::with_event_handlers(st, |m| m.get("save").map(|v| v.len()).unwrap_or(0))
        }),
        1,
        "有主的事件处理器不该被清"
    );
    assert!(
        crate::host::with_state_or_init(|st| {
            crate::state::with_statusline_hook(st, |h| h.is_some())
        }),
        "有主的状态栏钩子不该被清"
    );
}

/// `helix.plugin.unload(name)` —— **只入队**(不能同步回收:`onUnload` 要再进引擎)。
/// 真正的回收在 helix-term 的命令边界走 `helix_js::unload_plugin`(与 `:plugin-unload` 同一实现)。
#[test]
fn js_plugin_unload_only_enqueues_a_request() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    let _ = take_ui_requests(); // 清掉前面用例残留
    load_script(
        r#"helix.plugin("a", {});
helix.register_command("a-cmd", () => {});
helix.plugin.unload("a");"#,
    )
    .unwrap();

    let reqs = take_ui_requests();
    assert!(
        reqs.iter()
            .any(|r| matches!(r, UiRequest::UnloadPlugin(q) if q.name == "a")),
        "应入队 UnloadPlugin(a)"
    );
    // 还没真的回收(命令仍在):回收发生在命令边界
    assert!(command_names().contains(&"a-cmd".to_string()));

    // 参数校验
    assert!(
        load_script(r#"helix.plugin.unload(42);"#).is_err(),
        "非字符串应报错"
    );
    assert!(
        load_script(r#"helix.plugin.unload("");"#).is_err(),
        "空名应报错"
    );
}

/// 真插件（仓库里那份 `plugins/statusline/plugin.js`）的**配置面**回归：
///   ① 默认布局 = 旧观感：两端 `╭`/`╮`、文件名、位置/百分比/行数各归其区
///   ② `corners = "off"` ⇒ 无圆角
///   ③ `left/center/right` 列表决定**内容与所在区**；拼错的段名变成 `!名字`（红字可见，不静默）
///
/// 用**私有宿主**（`JsHost::new()`）而不是默认宿主：配置覆盖与状态栏钩子都是按宿主隔离的，
/// 这样这条测试不会污染 `popup_panel::statusline_hook`（那条假定共享宿主里没有钩子 ——
/// 底下没有清钩子的 API，见 `state::with_statusline_hook_current`）。
#[test]
fn real_statusline_layout_is_config_driven() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../plugins/statusline/plugin.js"
    ))
    .expect("仓库里的真实 statusline 插件应可读");
    // 断言用：把 parts 拼成一条可读串（`|` 分隔，便于失败时看整行）
    let joined = |parts: &[crate::types::StatuslinePart]| -> String {
        parts
            .iter()
            .map(|p| p.text.clone())
            .collect::<Vec<_>>()
            .join("|")
    };
    let ctx = statusline_ctx(); // mode=insert cursor=(3,7) total=100 err=2 warn=1 1 char

    // ① 默认布局：圆角 + 各段内容
    let mut host = crate::host::JsHost::new();
    host.load_script_named("statusline/plugin.js", &src)
        .expect("真插件应能加载");
    let parts = host.statusline_parts(&ctx).expect("默认应渲染出内容");
    let line = joined(&parts);
    assert!(
        line.starts_with("╭"),
        "默认 line 圆角：left 区从 ╭ 起: {line:?}"
    );
    assert!(
        line.ends_with("╮"),
        "默认 line 圆角：right 区以 ╮ 收: {line:?}"
    );
    assert!(line.contains("a.rs"), "文件名段: {line:?}");
    assert!(line.contains("4:8"), "位置段 3,7 ⇒ 4:8: {line:?}");
    assert!(line.contains("4%"), "百分比段 4/100: {line:?}");
    assert!(line.contains("1 char"), "主选区长度段: {line:?}");
    // 诊断在 center 区（红/黄两条）
    let center: Vec<&str> = parts
        .iter()
        .filter(|p| p.zone.as_deref() == Some("center"))
        .map(|p| p.text.as_str())
        .collect();
    assert_eq!(center.len(), 2, "error+warning 两段进 center: {center:?}");
    // 位置/百分比/行数在 right 区
    let right: Vec<&str> = parts
        .iter()
        .filter(|p| p.zone.as_deref() == Some("right"))
        .map(|p| p.text.as_str())
        .collect();
    assert!(right.contains(&"4:8"), "位置在 right: {right:?}");
    assert!(right.contains(&"100"), "总行数在 right: {right:?}");

    // ②③ 配置覆盖：corners=off、左区只要文件名、右区拼错一个段名
    let mut host = crate::host::JsHost::new();
    host.cache_plugin_overrides(
        r#"{"statusline":{"corners":"off","left":["file"],"center":[],"right":["nope","position"]}}"#,
    );
    host.load_script_named("statusline/plugin.js", &src)
        .expect("真插件在覆盖下也应能加载");
    let parts = host.statusline_parts(&ctx).expect("覆盖后仍应有内容");
    let line = joined(&parts);
    assert!(
        !line.contains('╭') && !line.contains('╮'),
        "corners=off 无圆角: {line:?}"
    );
    assert!(
        !line.contains("char"),
        "right 列表里没有 chars ⇒ 不显示: {line:?}"
    );
    assert!(line.contains("a.rs"), "左区只要文件名: {line:?}");
    assert!(
        parts
            .iter()
            .any(|p| p.text == "!nope" && p.style.as_deref() == Some("error")),
        "拼错的段名要红字可见（不静默）: {line:?}"
    );
    assert!(
        !parts.iter().any(|p| p.zone.as_deref() == Some("center")),
        "center 列表清空 ⇒ 没有 center 段: {line:?}"
    );
}

/// 真插件 `plugins/dapui/plugin.js` 能在这个引擎里**加载并跑通基本路径**。
///
/// 为什么必须有这条：内置插件最终跑在 boa 里，node 测试通过 ≠ 引擎通过
/// （本仓历史上栽过："可选链/新语法在 boa 里不支持" ⇒ 插件整个加载失败，而 node 全绿）。
/// 这里用**私有宿主**（不污染共享宿主的钩子表），断言：
/// ① 加载不抛；② 注册了 `dap` 状态栏段；③ 注册了 3 个命令；④ 快照为空时段函数返回 null。
#[test]
fn real_dapui_plugin_loads_in_engine() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../plugins/dapui/plugin.js"
    ))
    .expect("仓库里的真实 dapui 插件应可读");
    let mut host = crate::host::JsHost::new();
    host.load_script_named("dapui/plugin.js", &src)
        .expect("dapui 应能在 boa 里加载（语法/API 都要对）");

    // ① 命令注册了（名字与插件里一致）
    let names =
        crate::state::with_registry(host.state(), |r| r.keys().cloned().collect::<Vec<_>>());
    for want in ["dapui", "dapui-breakpoints", "dapui-repl"] {
        assert!(
            names.contains(&want.to_string()),
            "命令 {want} 应已注册；实际: {names:?}"
        );
    }

    // ② 状态栏段注册了，且没有会话时返回 null（不能画半条）
    let seg = crate::state::with_statusline_segments(host.state(), |m| m.contains_key("dap"));
    assert!(seg, "应注册 statusline 段 'dap'");
    let parts = host.statusline_parts(&statusline_ctx());
    // 没注册状态栏钩子（dapui 不接管整条状态栏）⇒ 这里应为 None
    assert!(parts.is_none(), "dapui 不该接管整条状态栏");
}
