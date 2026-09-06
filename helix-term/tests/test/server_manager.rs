use super::*;

/// env RAII(仿 lib 的 EnvGuard):Drop 恢复旧值/删除。SM_* 是进程级变量,测试 panic
/// 会跳过尾部清理 → 残留指向已删 tempdir 的 env 污染后续串行测试;guard 在 unwind 时
/// 也执行 Drop,panic 安全。
pub struct EnvGuard(&'static str, Option<String>);
impl EnvGuard {
    pub fn new(key: &'static str, val: String) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, &val);
        EnvGuard(key, prev)
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.1 {
            Some(v) => std::env::set_var(self.0, v),
            None => std::env::remove_var(self.0),
        }
    }
}

/// 标准 SM_* 三件套 guard:managed 目录 / languages.toml / 禁用宿主 PATH(hermetic)
fn sm_env(root: &std::path::Path) -> [EnvGuard; 4] {
    [
        EnvGuard::new(
            "SM_MANAGED_DIR",
            root.join("managed").to_string_lossy().into_owned(),
        ),
        EnvGuard::new(
            "SM_LANGS_TOML",
            root.join("languages.toml").to_string_lossy().into_owned(),
        ),
        EnvGuard::new("SM_PATH", String::new()),
        EnvGuard::new("SM_SERVER_RECIPES", String::new()),
    ]
}

// :server 命令族——冒烟验证(list/search/status + 惰性配方的负路径 install)。
// 环境隔离:SM_MANAGED_DIR/SM_LANGS_TOML 指到临时目录,不碰真实 ~/.local/share。
#[tokio::test(flavor = "multi_thread")]
async fn server_list_search_status() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let _sm = sm_env(dir.path());

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(":server list<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(!app.editor.is_err(), "server list 不应是错误: {status}");
                    assert!(
                        status.as_ref().contains("servers[")
                            && status.as_ref().contains("rust-analyzer"),
                        "server list 应含注册表与内置配方, got: {status}"
                    );
                }),
            ),
            (
                Some(":server search rust<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("rust-analyzer"),
                        "search 命中 rust-analyzer, got: {status}"
                    );
                }),
            ),
            (
                Some(":server status<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("受管/")
                            && status.as_ref().contains("managed dir"),
                        "status 应报统计与目录, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// :server 面板插件冒烟:加载插件 → 开关面板(行数据回传渲染不崩)。
#[tokio::test(flavor = "multi_thread")]
async fn server_manager_panel_toggle() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("c.txt");
    std::fs::write(&file, "x\n")?;
    let _sm = sm_env(dir.path());
    let plugin = format!(
        "{}/plugins/features/server-manager/index.js",
        std::env::var("CARGO_MANIFEST_DIR")
            .unwrap()
            .rsplitn(2, '/')
            .nth(1)
            .unwrap_or(".")
    );

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {plugin}<ret>")),
                Some(&|app| {
                    if app.editor.is_err() {
                        let msg = app
                            .editor
                            .get_status()
                            .map(|(s, _)| s.to_string())
                            .unwrap_or_default();
                        panic!("插件加载不应报错, got: {msg}");
                    }
                }),
            ),
            (
                Some(":server-manager<ret>"),
                Some(&|app| {
                    // 面板打开后行数据请求/渲染在后续帧;此处仅确认无错误状态
                    assert!(!app.editor.is_err(), "面板打开不应报错");
                }),
            ),
            // 面板内 Enter 触发 install(惰性/活配方 → 错误或成功提示,面板不崩;异步 op 在后续帧)
            (Some("<ret>"), None),
            // q 关闭面板后编辑器仍响应(下一命令正常执行覆盖错误状态)
            (Some("q"), None),
            (
                Some(":server status<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        !app.editor.is_err() && status.as_ref().contains("受管/"),
                        "关闭后 :server status 正常, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
#[tokio::test(flavor = "multi_thread")]
async fn server_negative_paths() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("b.txt");
    std::fs::write(&file, "x\n")?;
    let _sm = sm_env(dir.path());

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(":server install clangd<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        app.editor.is_err(),
                        "惰性配方 install 应报错, got: {status}"
                    );
                    assert!(
                        status.as_ref().contains("下载源未配置"),
                        "提示配置源, got: {status}"
                    );
                }),
            ),
            (
                Some(":server install rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        app.editor.is_err(),
                        "活配方缺 version 应报错, got: {status}"
                    );
                    assert!(
                        status.as_ref().contains("version"),
                        "提示补 version(或 config version), got: {status}"
                    );
                }),
            ),
            (
                Some(":server remove rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        app.editor.is_err() && status.as_ref().contains("未安装"),
                        "未装则 remove 报错, got: {status}"
                    );
                }),
            ),
            (
                Some(":server install<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        status.as_ref().contains("usage: server"),
                        "缺参给 usage, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
// 本地已装识别:install 短路(直接用)、remove 拒绝、unmanage 停挂接(toggle)。
#[tokio::test(flavor = "multi_thread")]
async fn server_local_short_circuit_and_unmanage() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("d.txt");
    std::fs::write(&file, "x\n")?;
    // PATH 里放一个假 rust-analyzer → 视为本地已装
    let bins = dir.path().join("localbin");
    std::fs::create_dir_all(&bins)?;
    let ra = bins.join("rust-analyzer");
    std::fs::write(&ra, "#!/bin/sh\necho rust-analyzer 9.9\n")?;
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&ra, std::fs::Permissions::from_mode(0o755))?;
    }
    let _sm = [
        EnvGuard::new(
            "SM_MANAGED_DIR",
            dir.path().join("managed").to_string_lossy().into_owned(),
        ),
        EnvGuard::new(
            "SM_LANGS_TOML",
            dir.path()
                .join("languages.toml")
                .to_string_lossy()
                .into_owned(),
        ),
        EnvGuard::new("SM_PATH", bins.to_string_lossy().to_string()),
    ];

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(":server install rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        !app.editor.is_err() && status.as_ref().contains("本地已可用"),
                        "本地已装应短路安装, got: {status}"
                    );
                }),
            ),
            (
                Some(":server remove rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        app.editor.is_err() && status.as_ref().contains("不能卸载"),
                        "本地工具 remove 应拒绝, got: {status}"
                    );
                }),
            ),
            (
                Some(":server unmanage rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        !app.editor.is_err() && status.as_ref().contains("已停用"),
                        "unmanage 停用挂接, got: {status}"
                    );
                }),
            ),
            (
                Some(":server unmanage rust-analyzer<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        !app.editor.is_err() && status.as_ref().contains("已恢复"),
                        "再 unmanage 恢复, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

/// 写 file:// 假 release:tar.gz(顶层 demo-bin-1.0.0/ + 可执行 bin demo-bin)到 path
fn write_fake_tar_gz(path: &std::path::Path, bin_name: &str, version: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let file = std::fs::File::create(path)?;
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut ar = tar::Builder::new(enc);
    // 顶层目录 {bin}-{version}/
    let top = format!("{bin_name}-{version}");
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Directory);
    h.set_mode(0o755);
    h.set_size(0);
    ar.append_data(
        &mut h,
        format!("{top}/"),
        std::io::Cursor::new(Vec::<u8>::new()),
    )?;
    // bin 脚本(0755)
    let script = format!("#!/bin/sh\necho '{bin_name} {version}'\n");
    let mut hb = tar::Header::new_gnu();
    hb.set_size(script.len() as u64);
    hb.set_mode(0o755);
    ar.append_data(&mut hb, format!("{top}/{bin_name}"), script.as_bytes())?;
    let enc = ar.into_inner()?;
    let f = enc.finish()?;
    f.sync_all()?;
    drop(f);
    Ok(())
}

// Arsenal M2 集成:file:// 假源经 helix.server.task 后台任务全流程安装。
// 注入:SM_SERVER_CONFIG 指到临时 config.toml([server-manager.registry.demo-bin] 配方),
// SM_MANAGED_DIR/SM_LANGS_TOML 隔离产物,SM_PATH 禁用宿主 PATH。JS 回调把任务事件
// echo 成 "ev:kind:name:bytes",经编辑器状态栏断言 done 到达 + managed/bin 落盘 +
// languages.toml 收敛(受管 [language-server.demo-bin] 指向受管绝对路径)。
#[tokio::test(flavor = "multi_thread")]
async fn server_arsenal_task_install_background_file_source() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("arsenal.txt");
    std::fs::write(&file, "x\n")?;
    let managed = dir.path().join("managed");
    let langs = dir.path().join("languages.toml");
    let _sm = [
        EnvGuard::new("SM_MANAGED_DIR", managed.to_string_lossy().into_owned()),
        EnvGuard::new("SM_LANGS_TOML", langs.to_string_lossy().into_owned()),
        EnvGuard::new("SM_PATH", String::new()),
        EnvGuard::new("SM_SERVER_RECIPES", String::new()),
    ];

    // 假 release:file://{dir}/rel/1.0.0/demo.tar.gz(顶层 demo-bin-1.0.0/,strip=1 → demo-bin)
    let rel_dir = dir.path().join("rel").join("1.0.0");
    write_fake_tar_gz(&rel_dir.join("demo.tar.gz"), "demo-bin", "1.0.0")?;

    // 配方经 SM_SERVER_CONFIG 注入(与 SM_MANAGED_DIR 同哲学:env 覆写磁盘 config 读取源)
    let cfg = dir.path().join("sm-config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[server-manager]\n\n[server-manager.registry.demo-bin]\nurl = \"file://{}/rel/{{version}}/demo.tar.gz\"\nversion = \"1.0.0\"\nstrip = 1\nbin = \"demo-bin\"\nlanguages = [\"demo\"]\n",
            dir.path().display()
        ),
    )?;
    let _sm_cfg = EnvGuard::new("SM_SERVER_CONFIG", cfg.to_string_lossy().into_owned());

    // 驱动 JS:helix.server.task 提交后台 install,回调把事件汇聚进脚本级数组;
    // :arsenal-e2e-dump 一次性 echo 全序列(状态栏每泵会覆写,跨泵断言不靠单步截屏)。
    let plugin_path = dir.path().join("arsenal-e2e.js");
    std::fs::write(
        &plugin_path,
        r#"
        const evs = [];
        helix.register_command("arsenal-e2e", () => {
            helix.server.task(
                [{ op: "install", name: "demo-bin", version: "1.0.0" }],
                (ev) => { evs.push("ev:" + ev.kind + ":" + ev.name + ":" + (ev.bytes || 0)); }
            );
        });
        helix.register_command("arsenal-e2e-dump", () => {
            helix.echo("seq:" + evs.join(" "));
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            // 提交后台任务:worker 异步执行;file:// 假源毫秒级。提交后不再发任何键——
            // 事件在本 pump 的 idle 窗口内 resolve(I1 起 push 即唤醒;集成下至少单 idle
            // 收敛,真实区别见 helix-js push_event_fires_registered_wake 单测 + 报告局限)
            (
                Some(":arsenal-e2e<ret>"),
                Some(&|app| {
                    assert!(
                        !app.editor.is_err(),
                        "任务提交不应报错, got: {:?}",
                        app.editor.get_status()
                    );
                }),
            ),
            // 收敛断言:事件序列(JS 侧累积,不受状态栏覆写影响)
            (
                Some(":arsenal-e2e-dump<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    let seq = status.as_ref();
                    assert!(!app.editor.is_err(), "后台安装不应报错, got: {seq}");
                    assert!(
                        seq.contains("seq:ev:phase:demo-bin")
                            && seq.contains("ev:progress:demo-bin:")
                            && seq.contains("ev:done:demo-bin"),
                        "事件序列应含 phase/progress/done, got: {seq}"
                    );
                    assert!(
                        seq.find("ev:phase:demo-bin").unwrap_or(0)
                            < seq.find("ev:done:demo-bin").unwrap_or(usize::MAX),
                        "phase 应先于 done, got: {seq}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    // 状态收敛:受管 bin 落盘(软链)→ 目标存在;languages.toml 出现受管 [language-server.demo-bin]
    let bin = managed.join("bin").join("demo-bin");
    assert!(
        bin.exists(),
        "managed/bin/demo-bin 应已安装: {}",
        bin.display()
    );
    let langs_text =
        std::fs::read_to_string(&langs).map_err(|e| anyhow::anyhow!("读 languages.toml: {e}"))?;
    assert!(
        langs_text.contains("[language-server.demo-bin]"),
        "languages.toml 应有受管 language-server 段, got: {langs_text}"
    );
    assert!(
        langs_text.contains(&managed.join("bin").join("demo-bin").display().to_string()),
        "command 应指向受管绝对路径, got: {langs_text}"
    );
    Ok(())
}

// ────────────────────────── arsenal M3 冒烟本地辅助（渲染到 buffer 读面板文本） ──────────────────────────

use helix_term::application::Application;
use helix_term::job::Jobs;
use helix_view::input::parse_macro;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

/// 发送键序列并泵事件直到空闲
async fn pump(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(keys)?.into_iter() {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    Ok(())
}

/// 渲染 compositor 到 Buffer，返回所有行
fn render_rows(app: &mut Application, area: helix_view::graphics::Rect) -> Vec<String> {
    let mut buf = tui::buffer::Buffer::empty(area);
    app.compositor.reset_plugin_diffs();
    let mut jobs = Jobs::new();
    let mut cx = helix_term::compositor::Context {
        editor: &mut app.editor,
        scroll: None,
        jobs: &mut jobs,
    };
    app.compositor.render(area, &mut buf, &mut cx);
    (0..area.height)
        .map(|y| {
            buf.content
                .iter()
                .skip(y as usize * area.width as usize)
                .take(area.width as usize)
                .map(|c| c.symbol.as_str())
                .collect::<String>()
        })
        .collect()
}

// ────────────────────────── arsenal M4 冒烟本地辅助（渲染到 buffer 读浮层文本） ──────────────────────────

/// 全宽拼接当前 compositor 渲染文本(浮层居中,64 列切片不再适用;contains 断言跨全行)
fn arsenal_text(app: &mut Application) -> String {
    render_rows(app, helix_view::graphics::Rect::new(0, 0, 120, 40))
        .iter()
        .map(|r| r.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

// Arsenal M4 浮层冒烟:主窗 popup v2 居中浮层(78%×75%,layer "arsenal")→
// 搜索 → Enter 动作链路 → 版本输入(needs_version,内置 rust-analyzer)Esc 取消 →
// i 信息弹窗(命令路径/下载源占位替换为真实行字段)→ 假源安装收敛(行 ✓)→
// 改 bin 检测版本重开 → ▲ 可升级标记(upgradable 字段)→ 动作菜单(update/remove)开/关 →
// q 关闭 → 编辑器仍响应。
#[tokio::test(flavor = "multi_thread")]
async fn server_arsenal_ui_smoke() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;
    let _sm = sm_env(dir.path());

    // 假 release:file://{dir}/rel/{version}/demo.tar.gz(顶层 demo-bin-1.0.0/,strip=1)
    let rel_dir = dir.path().join("rel").join("1.0.0");
    write_fake_tar_gz(&rel_dir.join("demo.tar.gz"), "demo-bin", "1.0.0")?;
    // 配方注入(SM_SERVER_CONFIG):demo-bin url 含 {version} 但 version 固定 → needs_version false
    let cfg = dir.path().join("sm-config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[server-manager]\n\n[server-manager.registry.demo-bin]\nurl = \"file://{}/rel/{{version}}/demo.tar.gz\"\nversion = \"1.0.0\"\nstrip = 1\nbin = \"demo-bin\"\nlanguages = [\"demo\"]\ndescription = \"Fake demo tool\"\nhomepage = \"https://example.invalid/demo\"\n",
            dir.path().display()
        ),
    )?;
    let _sm_cfg = EnvGuard::new("SM_SERVER_CONFIG", cfg.to_string_lossy().into_owned());

    let plugin = format!(
        "{}/plugins/features/arsenal/index.js",
        std::env::var("CARGO_MANIFEST_DIR")
            .unwrap()
            .rsplitn(2, '/')
            .nth(1)
            .unwrap_or(".")
    );

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let popup_type = std::any::type_name::<helix_term::ui::Popup<helix_term::ui::PluginPopup>>();

    // 1. 加载插件 + 打开市场窗(浮层主窗)
    pump(&mut app, &format!(":plugin-load {plugin}<ret>")).await?;
    assert!(
        !app.editor.is_err(),
        "arsenal 插件加载不应报错, got: {:?}",
        app.editor.get_status()
    );
    pump(&mut app, ":arsenal<ret>").await?;
    assert!(!app.editor.is_err(), ":arsenal 打开不应报错");
    assert!(
        app.compositor.has_component(popup_type),
        "市场窗应以 popup 浮层打开"
    );
    let text = arsenal_text(&mut app);
    assert!(text.contains("arsenal"), "浮层标题应渲染: {text:?}");
    assert!(
        text.contains("rust-analyzer") && text.contains("demo-bin"),
        "内置 + 注入配方行可见: {text:?}"
    );

    // 2. Enter 在 rust-analyzer(sel0;缺失+installable+needs_version)→ 版本输入弹窗(install 单项直达)
    pump(&mut app, "<ret>").await?;
    assert!(!app.editor.is_err(), "Enter 版本输入不应报错");
    let text = arsenal_text(&mut app);
    assert!(
        text.contains("explicit version"),
        "needs_version → 版本输入弹窗应开: {text:?}"
    );
    pump(&mut app, "<esc>").await?; // 取消
    assert!(!app.editor.is_err(), "版本输入 Esc 不应报错");
    let text = arsenal_text(&mut app);
    assert!(
        text.contains("arsenal") && !text.contains("explicit version"),
        "Esc 取消版本输入后回主窗: {text:?}"
    );

    // 3. i 信息弹窗:行字段 bin/source 落地(M3 占位文本替换)
    pump(&mut app, "i").await?;
    let text = arsenal_text(&mut app);
    assert!(
        text.contains("command: (not installed)")
            && text.contains("source: https://github.com/rust-lang"),
        "信息弹窗应显示命令/下载源(行字段): {text:?}"
    );
    pump(&mut app, "<esc>").await?;
    assert!(
        arsenal_text(&mut app).contains("arsenal"),
        "信息弹窗关闭后主窗仍在"
    );

    // 4. / 模态搜索过滤 "demo";SEARCH 内可打印字符进查询 → 仅 demo-bin 行
    pump(&mut app, "/demo").await?;
    let text = arsenal_text(&mut app);
    assert!(
        text.contains("demo-bin") && !text.contains("rust-analyzer"),
        "过滤 'demo' 后仅 demo-bin: {text:?}"
    );

    // 5. Enter → demo-bin install 单项直达(version 固定,needs_version false,直发 task)→ 安装收敛 ✓
    pump(&mut app, "<ret>").await?;
    assert!(!app.editor.is_err(), "install 提交不应报错");
    // 无害键步撑窗:SEARCH 态字母(j/f 等)是查询字符 → 用 <down> 撑窗
    // (v2 模态;worker(file:// 假源)事件 + done 后 fetch_rows 渲染收敛)
    for _ in 0..4 {
        pump(&mut app, "<down>").await?;
    }
    assert!(!app.editor.is_err(), "后台安装不应报错");
    let text = arsenal_text(&mut app);
    assert!(
        text.contains("✓ 1.0.0"),
        "安装收敛后行状态 ✓ 1.0.0(版本串已剥 bin 名前缀): {text:?}"
    );

    // 6. 改 bin 检测版本 → 重开市场 → ▲ 可升级标记(upgradable 字段驱动)
    {
        use std::os::unix::fs::PermissionsExt as _;
        let bin = dir.path().join("managed").join("bin").join("demo-bin");
        std::fs::write(&bin, "#!/bin/sh\necho 'demo-bin 9.9'\n")?;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))?;
    }
    pump(&mut app, "<esc>q").await?; // SEARCH Esc 清查询回 NORMAL,q 关窗(模态)
    assert!(
        !app.compositor.has_component(popup_type),
        "Esc+q 应关闭浮层主窗"
    );
    pump(&mut app, ":arsenal<ret>").await?;
    // 重开后过滤为空、sel=0(rust-analyzer);demo-bin 行在列表内(9 行 < 可视高度)
    let text = arsenal_text(&mut app);
    assert!(
        text.contains("▲ 9.9"),
        "检测版本 9.9 ≠ 配方 1.0.0 → ▲ 9.9(剥前缀): {text:?}"
    );

    // 7. / 过滤 'demo' 选中 demo-bin → Enter → 动作菜单(update/remove);Esc 关菜单后
    //    Esc 清搜索 → Tab 切页签 lsp 仍响应(主窗按键在无过滤下正常)
    pump(&mut app, "/demo").await?;
    pump(&mut app, "<ret>").await?;
    let text = arsenal_text(&mut app);
    assert!(
        text.contains("▸ update") && text.contains("remove  demo-bin"),
        "动作菜单应列 update/remove: {text:?}"
    );
    pump(&mut app, "<esc>").await?; // 子层 Esc 关菜单回主窗(filter 仍 demo)
    assert!(
        arsenal_text(&mut app).contains("arsenal"),
        "菜单 Esc 后主窗仍在"
    );
    pump(&mut app, "<esc>").await?; // 清搜索(I2→v2:SEARCH Esc 退 NORMAL)
    pump(&mut app, "<tab>").await?; // NORMAL Tab:页签 all → lsp
    let text = arsenal_text(&mut app);
    assert!(
        text.contains("rust-analyzer")
            && text.contains("demo-bin")
            && !text.contains("prettier")
            && !text.contains("debugpy"),
        "Tab→lsp 页签过滤生效(非 lsp 行不可见): {text:?}"
    );

    // 8. q 关闭市场窗;编辑器仍响应
    pump(&mut app, "q").await?;
    assert!(
        !app.compositor.has_component(popup_type),
        "q 应关闭浮层主窗"
    );
    pump(&mut app, ":server status<ret>").await?;
    {
        let (status, _) = app.editor.get_status().unwrap();
        assert!(
            !app.editor.is_err() && status.as_ref().contains("受管/"),
            "关闭后 :server status 正常, got: {status}"
        );
    }
    Ok(())
}

// 宽字符回归:注入 CJK 描述(64 显示列,超 desc 列宽)配方 → 行内描述按显示列宽截断
// 并以 … 收尾,描述尾部哨兵不得溢出到渲染(状态列右锚不被推出,无 render error)。
// v2 行模型按列宽截断(旧 slice() 字符截断会让 CJK 把行撑出可视区)。
#[tokio::test(flavor = "multi_thread")]
async fn server_arsenal_wide_desc_truncated_inside_frame() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("w.txt");
    std::fs::write(&file, "x\n")?;
    let _sm = sm_env(dir.path());

    let desc = format!("{}越界哨兵", "甲".repeat(28)); // 64 显示列 > desc 列宽 → 必截断
    let cfg = dir.path().join("sm-config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[server-manager]\n\n[server-manager.registry.wide-desc-tool]\nurl = \"file://{}/rel/{{version}}/wide.tar.gz\"\nversion = \"9.9\"\nstrip = 1\nbin = \"wide-tool\"\nlanguages = [\"cjk\"]\ndescription = \"{desc}\"\n",
            dir.path().display()
        ),
    )?;
    let _sm_cfg = EnvGuard::new("SM_SERVER_CONFIG", cfg.to_string_lossy().into_owned());

    let plugin = format!(
        "{}/plugins/features/arsenal/index.js",
        std::env::var("CARGO_MANIFEST_DIR")
            .unwrap()
            .rsplitn(2, '/')
            .nth(1)
            .unwrap_or(".")
    );
    let mut app = AppBuilder::new().with_file(file, None).build()?;
    pump(&mut app, &format!(":plugin-load {plugin}<ret>")).await?;
    pump(&mut app, ":arsenal<ret>").await?;
    assert!(!app.editor.is_err(), "市场窗打开不应报错");
    let text = arsenal_text(&mut app);
    assert!(
        text.contains("wide-desc-tool"),
        "CJK 描述行应可见: {text:?}"
    );
    assert!(text.contains("…"), "超长描述应以 … 收尾: {text:?}");
    assert!(
        !text.contains("哨兵"),
        "描述尾部不得溢出到渲染(按列截断): {text:?}"
    );
    assert!(!text.contains("render error"), "渲染不应报错: {text:?}");

    // / 模态搜索命中单行(CJK 行亦可被过滤)
    pump(&mut app, "/wide").await?;
    let text = arsenal_text(&mut app);
    assert!(
        text.contains("wide-desc-tool") && !text.contains("rust-analyzer"),
        "/wide 过滤后仅 CJK 行: {text:?}"
    );
    pump(&mut app, "<esc>").await?;
    pump(&mut app, "q").await?;
    assert!(
        !app.compositor.has_component(std::any::type_name::<
            helix_term::ui::Popup<helix_term::ui::PluginPopup>,
        >()),
        "q 应关闭市场窗"
    );
    Ok(())
}

// Arsenal 批量:t 标记两个注入配方(均为未装+可装)→ Enter 直接批量(集合快照)→
// 单 worker 串行安装两件 → 收敛后两行均 ✓;managed/bin 落盘断言。
#[tokio::test(flavor = "multi_thread")]
async fn server_arsenal_batch_two_installs() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("b.txt");
    std::fs::write(&file, "x\n")?;
    let _sm = sm_env(dir.path());

    let rel_dir = dir.path().join("rel").join("1.0.0");
    write_fake_tar_gz(&rel_dir.join("demo.tar.gz"), "demo-bin", "1.0.0")?;
    write_fake_tar_gz(&rel_dir.join("demob.tar.gz"), "demob", "1.0.0")?;
    let cfg = dir.path().join("sm-config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[server-manager]\n\n[server-manager.registry.demo-bin]\nurl = \"file://{}/rel/{{version}}/demo.tar.gz\"\nversion = \"1.0.0\"\nstrip = 1\nbin = \"demo-bin\"\nlanguages = [\"demo\"]\n\n[server-manager.registry.demob]\nurl = \"file://{}/rel/{{version}}/demob.tar.gz\"\nversion = \"1.0.0\"\nstrip = 1\nbin = \"demob\"\nlanguages = [\"demo\"]\n",
            dir.path().display(),
            dir.path().display()
        ),
    )?;
    let _sm_cfg = EnvGuard::new("SM_SERVER_CONFIG", cfg.to_string_lossy().into_owned());

    let plugin = format!(
        "{}/plugins/features/arsenal/index.js",
        std::env::var("CARGO_MANIFEST_DIR")
            .unwrap()
            .rsplitn(2, '/')
            .nth(1)
            .unwrap_or(".")
    );

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    pump(&mut app, &format!(":plugin-load {plugin}<ret>")).await?;
    pump(&mut app, ":arsenal<ret>").await?;
    assert!(!app.editor.is_err(), "市场窗打开不应报错");

    // / 过滤 'demo' 命中 demo-bin + demob(SEARCH 态 t/j 是查询字符,过滤仅用来看)
    pump(&mut app, "/demo").await?;
    let text = arsenal_text(&mut app);
    assert!(
        text.contains("demo-bin") && text.contains("demob"),
        "两注入配方应可见: {text:?}"
    );
    // Esc 清搜索回 NORMAL(sel=0=rust-analyzer);内置 7 配方在前 → j×7 到 demo-bin 再标记两件
    // (标记/导航须在 NORMAL 态:字母是命令)
    pump(&mut app, "<esc>").await?;
    pump(&mut app, "jjjjjjj").await?; // sel → demo-bin(第 8 行)
    pump(&mut app, "t").await?; // 标记 demo-bin
    pump(&mut app, "j").await?; // sel → demob
    pump(&mut app, "t").await?; // 标记 demob
    pump(&mut app, "<ret>").await?; // 批量 Enter(集合快照)
    assert!(!app.editor.is_err(), "批量提交不应报错");
    for _ in 0..4 {
        pump(&mut app, "j").await?;
    }
    assert!(!app.editor.is_err(), "批量安装不应报错");
    let text = arsenal_text(&mut app);
    assert!(
        text.contains("✓ 1.0.0"),
        "批量收敛后两行均 ✓ 1.0.0(版本剥 bin 名前缀): {text:?}"
    );
    let managed_bin = dir.path().join("managed").join("bin");
    assert!(
        managed_bin.join("demo-bin").exists() && managed_bin.join("demob").exists(),
        "批量安装应落盘两个受管 bin"
    );

    pump(&mut app, "q").await?;
    pump(&mut app, ":server status<ret>").await?;
    {
        let (status, _) = app.editor.get_status().unwrap();
        assert!(
            !app.editor.is_err() && status.as_ref().contains("受管/"),
            "批量后 :server status 正常, got: {status}"
        );
    }
    Ok(())
}
// 独立配方文件在编辑器会话内生效:SM_SERVER_RECIPES 指向 contrib 文件 → :server search 可见
#[tokio::test(flavor = "multi_thread")]
async fn server_recipes_file_visible_in_editor() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "x\n")?;
    std::env::set_var("SM_MANAGED_DIR", dir.path().join("managed"));
    std::env::set_var("SM_LANGS_TOML", dir.path().join("languages.toml"));
    std::env::set_var("SM_PATH", "");
    std::env::set_var("SM_SERVER_RECIPES", "");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let repo = manifest
        .rsplitn(2, '/')
        .nth(1)
        .unwrap_or(&manifest)
        .to_string();
    let _rec = EnvGuard::new(
        "SM_SERVER_RECIPES",
        format!("{repo}/contrib/server-manager-recipes.toml"),
    );
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(":server search taplo<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        !app.editor.is_err() && status.as_ref().contains("taplo"),
                        "配方文件条目可搜到, got: {status}"
                    );
                }),
            ),
            (
                Some(":server search marksman<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(
                        !app.editor.is_err() && status.as_ref().contains("marksman"),
                        "占位条目可搜到, got: {status}"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
