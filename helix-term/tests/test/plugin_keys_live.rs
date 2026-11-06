//! **实测**：插件作用域键位在真实编辑器里的两处出口 —— `:keys` 与 `?`。
//!
//! 与 `plugin_keys_scope.rs` 的区别：那份用 `test_key_sequences`（它对收尾 `<esc>:q!`
//! 有强假设，驱动打开新 buffer 的 `:keys` 会挂住）；这里用手动事件循环 + 超时，
//! 只验证"真的渲染出来了什么"。

use helix_view::current_ref;
use helix_view::input::parse_macro;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

use super::*;
use helix_term::application::Application;

/// 送一串宏，等事件循环空闲（带超时：挂住就失败，不拖测试套件）
async fn feed(app: &mut Application, macro_str: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut stream = UnboundedReceiverStream::new(rx);
    for ke in parse_macro(macro_str)? {
        tx.send(Ok(Event::Key(KeyEvent::from(ke))))?;
    }
    // 60s：并行跑整套时，boa 引擎/全局宿主有互斥，单个用例的 4–6s 会涨到十几秒
    // （15s 曾偶发超时 —— 这套用例本身不慢，是**争用**）。
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        app.event_loop_until_idle(&mut stream),
    )
    .await
    .map_err(|_| anyhow::anyhow!("事件循环 60s 未空闲（挂住了）"))?;
    Ok(())
}

/// `:keys` 的「插件作用域」段：把声明表原样列出来（名字 + 每条 键/说明）
#[tokio::test(flavor = "multi_thread")]
async fn keys_help_shows_plugin_scope_section() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("kh1.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("ks.js");
    std::fs::write(
        &plugin,
        r#"
        helix.plugin("ks-live", { deps: [], permissions: {} });
        helix.keymap.scope("ks-live", {
          keys: {
            "q": { action: "close", desc: "关掉面板" },
            "C-\\": { action: "leave", desc: "回编辑器" },
            "G": "grep-all",
          },
        });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    feed(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    feed(&mut app, ":keys<ret>").await?;

    let (_view, doc) = current_ref!(app.editor);
    let text = doc.text().to_string();
    let _ = std::fs::write("/tmp/keys_help_screen.txt", &text);
    assert!(text.contains("插件作用域"), "要有分段标题:\n{text}");
    assert!(text.contains("ks-live"), "要列作用域名:\n{text}");
    assert!(text.contains("关掉面板"), "要列 desc:\n{text}");
    assert!(
        text.contains("回编辑器"),
        "`C-\\` 也应该在（对象/字符串同源）:\n{text}"
    );
    assert!(
        text.contains("grep-all"),
        "简写形式（值=动作名）也要在:\n{text}"
    );
    let _ = app.close().await;
    Ok(())
}

/// `?` 在 filetree 面板里弹出的**画面**里就是声明表（不再是一行 echo）
#[tokio::test(flavor = "multi_thread")]
async fn filetree_help_popup_is_rendered_from_scope() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("kh2.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("filetree_live.js");
    // 用仓库里那份真实 filetree（与运行时副本一致）
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../plugins/filetree/plugin.js"
    ))?;
    std::fs::write(&plugin, src)?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    feed(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    feed(&mut app, ":filetree<ret>").await?;
    feed(&mut app, "?").await?;

    // 把整块画面渲染成行 —— 弹窗的文本只有这样才看得到（与 statusline 测试同一路线）
    let area = helix_view::graphics::Rect::new(0, 0, 120, 40);
    let rows = super::helpers::render_rows(&mut app, area);
    let screen = rows.join("\n");
    // 落盘一份完整画面：调试/验收时直接看（`cargo test` 输出里贴不下）
    let _ = std::fs::write("/tmp/filetree_help_screen.txt", &screen);
    // 渲染出来的画面里 CJK 是**双宽**：按 cell 取值会在两字之间插空格
    //（"键 位"），所以断言在"去掉所有空白"之后做。
    let flat: String = screen.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        flat.contains("filetree键位"),
        "帮助弹窗应有标题（这是 `show_help` 写的抬头）:\n{screen}"
    );
    assert!(
        flat.contains("[plugins.filetree.keys]可改"),
        "标题要点明可覆盖:\n{screen}"
    );
    // 正文来自 `helix.keymap.hints("filetree")` ⇒ 必然含声明表里的 desc
    assert!(
        flat.contains("关闭面板") && flat.contains("显示键位"),
        "弹窗内容应来自作用域声明表（q/? 的 desc）:\n{screen}"
    );
    // B 阶段：帮助弹窗绑了作用域（`scope: SCOPE`）⇒ `:` 这类全局前缀**不该**被弹层吃掉
    feed(&mut app, ":echo \"HELP-PT\"<ret>").await?;
    let status = app
        .editor
        .get_status()
        .map(|(m, _)| m.to_string())
        .unwrap_or_default();
    assert!(
        status.contains("HELP-PT"),
        "帮助弹窗开着时 `:` 应穿透到命令行: {status:?}"
    );

    let _ = app.close().await;
    Ok(())
}

/// **B 阶段**：`passthrough` 里声明的键在 popup 里能穿透回编辑器。
///
/// 这条钉的是"popup 是模态层但不再吞掉一切"：不声明 passthrough 时 `:` 会被弹窗吃掉
/// （用户视角＝"莫名其妙没反应"）；声明后 `:` 打开命令行、命令正常执行，弹窗仍在。
#[tokio::test(flavor = "multi_thread")]
async fn popup_passthrough_lets_colon_through() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("pt.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("pt.js");
    std::fs::write(
        &plugin,
        r#"
        helix.plugin("pt-demo", { deps: [], permissions: {} });
        helix.keymap.scope("pt-demo", {
          passthrough: [":"],          // 只在声明里的键才穿透
          keys: { "q": { action: "close", desc: "关闭" } },
        });
        helix.register_command("pt-open", () => {
          helix.open_popup({
            scope: "pt-demo",
            width: 40,
            height: 5,
            render: () => "hello",
            onKey: (k) => { helix.echo("onkey:" + String(k && k.name)); return "handled"; },
          });
        }, "打开演示弹窗");
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    feed(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    feed(&mut app, ":pt-open<ret>").await?;

    // ① `:` 穿透 ⇒ 命令行可用（命令真的执行了）
    feed(&mut app, ":echo \"PT-OK\"<ret>").await?;
    let status = |app: &Application| {
        app.editor
            .get_status()
            .map(|(m, _)| m.to_string())
            .unwrap_or_default()
    };
    assert!(
        status(&app).contains("PT-OK"),
        "`:` 应穿透到命令行并执行: {:?}",
        status(&app)
    );

    // ② 没声明的键仍然交给 JS onKey
    feed(&mut app, "x").await?;
    assert!(
        status(&app).contains("onkey:x"),
        "未声明的键仍应交给 onKey: {:?}",
        status(&app)
    );

    let _ = app.close().await;
    Ok(())
}

/// **B 阶段 T4.1**：带 `scope` 的弹层占着键时，`helix.input_badges()` 里出现作用域名（大写）
/// —— 也就是状态栏 `layers` 段会显示 `FILETREE` 那样的徽标。
///
/// 断言方式：把状态栏换成 `"B:" + helix.input_badges()`，再把整屏渲染出来看那一行。
/// **不要**用"插件命令里 echo 徽标"：命令跑在**隐式宿主**上，与 app 宿主不是同一个，
/// 读到的缓存是空的（实测踩过 —— 这正是徽标早期版本"看起来没生效"的原因）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_scope_shows_in_input_badges() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("bd.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("bd.js");
    std::fs::write(
        &plugin,
        r#"
        helix.plugin("bd-demo", { deps: [], permissions: {} });
        helix.keymap.scope("bd-demo", {
          passthrough: [":"],
          esc: "blur",
          keys: { "q": { action: "close", desc: "关" } },
        });
        // 状态栏 hook 跑在 **app 宿主**上 ⇒ `input_badges()` 读到的是同一份缓存
        helix.set_statusline((ctx) => "B:" + helix.input_badges(), { replace: true, zones: [1, 1, 1] });
        helix.register_command("bd-panel", () => {
          helix.open_panel({
            side: "right", size: 30, scope: "bd-demo", focusable: true,
            render: () => "panel",
            onKey: (k) => (k && k.name === "q" ? "close" : k && k.name === "Esc" ? "ignore" : "handled"),
          });
        }, "开面板");
        helix.register_command("bd-open", () => {
          helix.open_popup({
            scope: "bd-demo", width: 30, height: 4,
            render: () => "bd",
            onKey: (k) => (k && k.name === "q" ? "close" : k && k.name === "Esc" ? "ignore" : "handled"),
          });
        }, "开弹层");
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    feed(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;

    let area = helix_view::graphics::Rect::new(0, 0, 100, 20);
    let bottom = |app: &mut Application| {
        let rows = super::helpers::render_rows(app, area);
        rows.last().cloned().unwrap_or_default()
    };

    // ① 没有插件层 ⇒ 徽标为空
    let before = bottom(&mut app);
    assert!(before.contains("B:"), "状态栏 hook 应生效: {before:?}");
    assert!(
        !before.contains("BD-DEMO"),
        "没有插件层时不该有插件徽标: {before:?}"
    );

    // ② 打开带 scope 的弹层 ⇒ 徽标出现作用域名（大写）
    feed(&mut app, ":bd-open<ret>").await?;
    let after = bottom(&mut app);
    assert!(
        after.contains("BD-DEMO"),
        "插件层应进徽标（状态栏 `layers` 段同源）: {after:?}"
    );

    // 先关掉弹层（模态层压在栈顶上，会拦住后面所有键）
    feed(&mut app, "q").await?;
    assert_eq!(app.compositor.plugin_layer_count(), 0, "q 应关掉弹层");

    // ③ **T4.4**：`esc: "blur"` 对**面板**（叶子式插件层）生效 ——
    //    Esc 回焦点到编辑器叶子，面板保留，之后的键落给编辑器。
    //    弹层是模态的（压在层栈顶上），"blur" 对它没有意义 ⇒ 弹层的 Esc 仍走它自己的 onKey。
    feed(&mut app, ":bd-panel<ret>").await?;

    // 焦点在面板上时按 Esc：面板的 onKey 对 Esc 明确返回 "ignore" ⇒ 核心按声明 blur
    feed(&mut app, "<esc>").await?;
    feed(&mut app, "i").await?;
    assert_eq!(
        app.editor.mode(),
        helix_view::document::Mode::Insert,
        "Esc 回焦点后 `i` 应落到编辑器（而不是被面板吞掉）"
    );

    let _ = app.close().await;
    Ok(())
}

/// 真实插件**接线**（B 阶段）：`open_panel({scope})` 之后，compositor 认得这一层的 scope。
///
/// 这条钉住的是"插件把 scope 传下来了"，机制本身由上面几条用例覆盖（徽标 / Esc blur）。
#[tokio::test(flavor = "multi_thread")]
async fn real_filetree_panel_binds_scope() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("ft.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("filetree_live2.js");
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../plugins/filetree/plugin.js"
    ))?;
    std::fs::write(&plugin, src)?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    feed(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    assert_eq!(
        app.compositor.top_plugin_scope(),
        None,
        "面板没开时没有插件层"
    );
    feed(&mut app, ":filetree<ret>").await?;
    assert_eq!(
        app.compositor.top_plugin_scope().as_deref(),
        Some("filetree"),
        "filetree 面板应把自己的作用域带进层栈（徽标与 Esc 兜底都靠它）"
    );

    // which-key 的 `:which-key` 用 `helix.keymap.active_scope()` + `hints()` 弹表
    // ⇒ 面板里跑它**不该**报"不在插件层"（那就是 active_scope 没推到位）
    let wk = dir.path().join("wk.js");
    std::fs::write(
        &wk,
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../plugins/which-key/plugin.js"
        ))?,
    )?;
    feed(&mut app, &format!(":plugin-load {}<ret>", wk.display())).await?;
    feed(&mut app, ":which-key<ret>").await?;
    let status = app
        .editor
        .get_status()
        .map(|(m, _)| m.to_string())
        .unwrap_or_default();
    assert!(
        !status.contains("不在插件层") && !status.contains("没有可显示"),
        "面板里 `:which-key` 应能取到作用域表: {status:?}"
    );

    let _ = app.close().await;
    Ok(())
}

/// **前缀键在插件层里触发 which-key，提示里带上该层的键位表**（端到端）。
///
/// 面板是非模态叶子：未绑定的前缀键（这里用 `space`）会冒泡给编辑器 ⇒ which-key 的
/// `set_keymap_hint` 被调用 ⇒ 因为 `active_scope()` 非空，提示里**追加**作用域段。
#[tokio::test(flavor = "multi_thread")]
async fn which_key_prefix_hint_includes_scope_inside_panel() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("wk.txt");
    std::fs::write(&file, "x\n")?;
    let ft = dir.path().join("filetree_wk.js");
    std::fs::write(
        &ft,
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../plugins/filetree/plugin.js"
        ))?,
    )?;
    let wk = dir.path().join("which_key.js");
    std::fs::write(
        &wk,
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../plugins/which-key/plugin.js"
        ))?,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    feed(&mut app, &format!(":plugin-load {}<ret>", ft.display())).await?;
    feed(&mut app, &format!(":plugin-load {}<ret>", wk.display())).await?;
    feed(&mut app, ":filetree<ret>").await?;

    eprintln!("DBG top_scope={:?}", app.compositor.top_plugin_scope());
    // `space` 是默认的前缀键（[keys.normal] 里有子表）⇒ 面板不认它，冒泡给编辑器
    feed(&mut app, " ").await?;
    eprintln!(
        "DBG autoinfo?={:?}",
        app.editor
            .autoinfo
            .as_ref()
            .map(|i| i.text.chars().take(40).collect::<String>())
    );

    // which-key 在 box 模式把提示画成浮层；这里读 autoinfo 的文本更稳（不依赖布局坐标）
    let autoinfo = app
        .editor
        .autoinfo
        .as_ref()
        .map(|i| i.text.clone())
        .unwrap_or_default();
    assert!(
        autoinfo.contains("filetree") && autoinfo.contains("面板键位"),
        "插件层里的前缀键提示应追加作用域段: {autoinfo:?}"
    );

    let _ = app.close().await;
    Ok(())
}

/// **弹窗里的文本输入**：`el("input")` + 核心自动聚焦 ⇒ 真的能打字、能提交。
///
/// 这条钉的是一个核心缺口：`apply_content` 只**收集** focusables、不设焦点，
/// 而所有节点级编辑（字符/退格/方向键/粘贴）都经 `self.focus` 路由 ⇒ 早先
/// `el("input")` 必须先按 Tab 才能打字，于是全仓 0 处使用。现在弹窗里第一个
/// input 自动拿焦点（`SurfaceOpts.auto_focus_first`，面板不受影响）。
#[tokio::test(flavor = "multi_thread")]
async fn dialog_input_autofocus_accepts_typing() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("dlg.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("dlg.js");
    std::fs::write(
        &plugin,
        r#"
        helix.plugin("dlg-demo", { deps: [], permissions: {} });
        helix.register_command("dlg-open", () => {
          let value = "";
          let pid = 0;
          pid = helix.open_popup({
            scope: "dlg-demo",
            width: 40, height: 4,
            render: () => helix.el("box", [
              helix.el("input", {
                id: "dlg-in", value, width: 32,
                onChange: (v) => { value = v; },
                // 单行 input 的 Enter 由**节点**收（核心路由）⇒ 提交 + 自己关弹层
                onKey: (k) => {
                  const nm = k && k.name !== undefined ? String(k.name) : String(k);
                  if (nm === "Enter") { helix.echo("got:" + value); helix.close_popup(pid); }
                  return "handled";
                },
              }),
              helix.el("text", "hint", { style: "ui.help" }),
            ], { border: "round", title: " 输入 ", width: 40, height: 4 }),
            onKey: (k) => {
              const nm = k && k.name !== undefined ? String(k.name) : String(k);
              if (nm === "Esc") return "close";
              if (nm === "Enter") { helix.echo("got:" + value); return "close"; }
              return "handled";
            },
          });
        }, "打开输入弹窗");
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    feed(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    feed(&mut app, ":dlg-open<ret>").await?;

    // 不按 Tab，直接打 —— 弹窗首个 input 应已自动聚焦
    feed(&mut app, "abc").await?;

    feed(&mut app, "<backspace>").await?;
    feed(&mut app, "d<ret>").await?;

    let status = app
        .editor
        .get_status()
        .map(|(m, _)| m.to_string())
        .unwrap_or_default();
    assert!(
        status.contains("got:abd"),
        "键入应进 input 并经 onChange 回写（abc → 退格 → d ⇒ abd）: {status:?}"
    );

    let _ = app.close().await;
    Ok(())
}

/// **核心弹窗**（`helix.dialog.prompt`）：渲染/输入/提交全归核心，插件只给回调。
#[tokio::test(flavor = "multi_thread")]
async fn core_dialog_prompt_submits_value() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("cd.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("cd.js");
    std::fs::write(
        &plugin,
        r#"
        helix.plugin("cd-demo", { deps: [], permissions: {} });
        helix.register_command("cd-open", () => {
          helix.dialog.prompt({
            title: "重命名 a.rs",
            value: "a.rs",
            on_submit: (v) => helix.echo("sub:" + v),
            on_cancel: () => helix.echo("cancel"),
          });
        }, "打开核心输入弹窗");
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    feed(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    feed(&mut app, ":cd-open<ret>").await?;
    // 不按 Tab：核心弹窗的输入框应已聚焦
    feed(&mut app, "b.rs").await?;
    feed(&mut app, "<ret>").await?;
    let status = app
        .editor
        .get_status()
        .map(|(m, _)| m.to_string())
        .unwrap_or_default();
    assert!(
        status.contains("sub:a.rsb.rs"),
        "输入应进核心输入框（初值 a.rs + 键入 b.rs），Enter 提交: {status:?}"
    );

    let _ = app.close().await;
    Ok(())
}

/// **核心多选弹层**（`helix.dialog.select`）：j 移动、Space 勾选、Enter 交回选中值。
#[tokio::test(flavor = "multi_thread")]
async fn core_dialog_select_multi() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("sel.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("sel.js");
    std::fs::write(
        &plugin,
        r#"
        helix.plugin("sel-demo", { deps: [], permissions: {} });
        helix.register_command("sel-open", () => {
          helix.dialog.select({
            title: "异常断点",
            multi: true,
            items: [{ label: "Catch", value: "catch" }, { label: "Throw", value: "throw" }],
            initial: ["throw"],
            on_pick: (vs) => helix.echo("picked:" + vs.join(",")),
            on_cancel: () => helix.echo("cancel"),
          });
        }, "打开多选弹层");
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    feed(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    feed(&mut app, ":sel-open<ret>").await?;
    // 画面：圆角框里应列出两项（渲染分支漏接线时这里会是空的）
    {
        let area = helix_view::graphics::Rect::new(0, 0, 100, 24);
        let screen = super::helpers::render_rows(&mut app, area).join("\n");
        assert!(
            screen.contains("Catch") && screen.contains("Throw"),
            "多选弹层应真的画出条目:\n{screen}"
        );
    }
    // 初始选中 throw；j 移到 Throw 上再 Space 取消它，然后 k 回 Catch 勾上
    feed(&mut app, "j").await?;
    feed(&mut app, " ").await?;
    feed(&mut app, "k").await?;
    feed(&mut app, " ").await?;
    feed(&mut app, "<ret>").await?;
    let status = app
        .editor
        .get_status()
        .map(|(m, _)| m.to_string())
        .unwrap_or_default();
    assert!(
        status.contains("picked:catch"),
        "j/Space/k/Space 后应只剩 catch（throw 被取消勾选）: {status:?}"
    );

    let _ = app.close().await;
    Ok(())
}

/// 核心弹窗的取消：Esc ⇒ `on_cancel`，不提交。
///
/// 单独一条用例（不和提交混在同一 app 里）：提交会关层并留下异步的 `close_popup`
/// 请求，紧接着再开一个弹窗会被那笔请求的时序干扰（实测：混在一起时第二个弹窗收不到键）。
#[tokio::test(flavor = "multi_thread")]
async fn core_dialog_cancel() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("cx.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("cx.js");
    std::fs::write(
        &plugin,
        r#"
        helix.plugin("cx-demo", { deps: [], permissions: {} });
        helix.register_command("cx-open", () => {
          helix.dialog.prompt({
            title: "改个名字",
            value: "keep",
            on_submit: (v) => helix.echo("sub:" + v),
            on_cancel: () => helix.echo("cancel"),
          });
        }, "打开输入弹窗");
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    feed(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    feed(&mut app, ":cx-open<ret>").await?;
    feed(&mut app, "<esc>").await?;
    let status = app
        .editor
        .get_status()
        .map(|(m, _)| m.to_string())
        .unwrap_or_default();
    assert!(
        status.contains("cancel"),
        "Esc 应触发 on_cancel: {status:?}"
    );
    assert!(!status.contains("sub:"), "Esc 不该提交: {status:?}");

    let _ = app.close().await;
    Ok(())
}
