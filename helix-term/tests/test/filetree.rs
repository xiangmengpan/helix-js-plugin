use super::*;

use std::time::Duration;

use helix_term::application::Application;
use helix_term::ui::{js_picker::PickerRow, overlay::Overlay, picker::Picker};
use helix_view::current_ref;

// Windows 上这些用例的 `Event`/`KeyEvent` 用法已随 pty/流式路径 gate 掉 ⇒
// 导入留着但允许未使用（否则整仓 Windows gate 会因 warning 变红）。
#[cfg(windows)]
#[allow(unused_imports)]
use crossterm::event::{Event, KeyEvent};

/// 左 32 列（面板区）内容拼接
fn panel_text(app: &mut Application) -> String {
    render_rows(app, helix_view::graphics::Rect::new(0, 0, 120, 30))
        .iter()
        .map(|r| r.chars().take(32).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 面板**正文**（只留带边框的行）。负向断言（"某文件被滤掉了"）务必用它：
/// `panel_text` 会把底部状态栏一起拼进来，而状态栏里有当前文件路径 ⇒ 一律误伤。
fn panel_body(app: &mut Application) -> String {
    panel_text(app)
        .lines()
        .filter(|l| l.starts_with('│') || l.starts_with('┌') || l.starts_with('└'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 建临时目录结构 + 加载真实插件(读仓库 `plugins/filetree/plugin.js`)
async fn setup() -> anyhow::Result<(tempfile::TempDir, Application)> {
    // 配置合并缓存是进程级的:插件加载时会读 [plugins.filetree],别的用例写过的覆盖会漏进来
    let _cfg = plugin_config_lock().await;
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("src"))?;
    std::fs::write(dir.path().join("src/main.js"), "console.log(1)\n")?;
    std::fs::write(dir.path().join("readme.md"), "hi\n")?;
    std::fs::write(dir.path().join(".secret.txt"), "s\n")?;
    // 读**仓库**里的插件(不是开发机 `~/.config`:那在 CI/新机器上不存在)
    let src = std::fs::read_to_string(repo_plugin("filetree/plugin.js"))?;
    let plugin = dir.path().join("filetree.js");
    std::fs::write(&plugin, src)?;
    let mut app = AppBuilder::new().build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    Ok((dir, app))
}

/// 核心路径：reveal 定位 → 展开目录 → 打开文件（buffer 切换）
#[tokio::test(flavor = "multi_thread")]
async fn filetree_reveal_expand_open() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    // 用 join 逐段拼，别写死 `"src/main.js"`：那在 Windows 上得到 `src/main.js`，
    // 而 helix 侧给回的是 `src\\main.js` ⇒ 断言只会因为分隔符而红（实测踩过）
    let main_js = dir.path().join("src").join("main.js");
    let readme = dir.path().join("readme.md");

    // 当前文件在树 root（cwd=helix-term）外 → reveal 应重设 root 到 readme 所在目录
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    let text = panel_text(&mut app);
    assert!(
        text.contains("readme.md"),
        "reveal 后面板渲染含 readme.md: {text:?}"
    );
    assert!(text.contains("src"), "root 重设后显示 src 目录: {text:?}");

    // reveal 定位 readme.md（索引 2，排序 [src, filetree.js, readme.md]）→
    // Up×2 到 src（目录优先）→ Enter 展开 → Down 到 main.js → Enter 打开
    pump(&mut app, "<up><up>").await?;
    pump(&mut app, "<ret>").await?;
    pump(&mut app, "<down>").await?;
    pump(&mut app, "<ret>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.path()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
        main_js.to_str(),
        "Enter 应打开 src/main.js"
    );
    Ok(())
}

/// 隐藏文件切换：默认隐藏 . 文件，H 后显示
#[tokio::test(flavor = "multi_thread")]
async fn filetree_hidden_toggle() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    assert!(
        !panel_text(&mut app).contains(".secret.txt"),
        "默认隐藏 dotfile"
    );
    pump(&mut app, "H").await?;
    assert!(
        panel_text(&mut app).contains(".secret.txt"),
        "H 切换后显示 dotfile"
    );
    Ok(())
}

/// 新建文件：a → 输入弹窗 → Tab 聚焦 → 输入名字 → Enter → 异步 touch 生效
#[tokio::test(flavor = "multi_thread")]
async fn filetree_new_file_prompt() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;

    let popup_type = std::any::type_name::<helix_term::ui::Popup<helix_term::ui::PluginPopup>>();
    pump(&mut app, "a").await?;
    assert!(app.compositor.has_component(popup_type), "a → 输入弹窗打开");

    pump(&mut app, "note.txt<ret>").await?;
    assert!(
        !app.compositor.has_component(popup_type),
        "Enter 提交后弹窗关闭"
    );
    for _ in 0..50 {
        if dir.path().join("note.txt").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dir.path().join("note.txt").exists(), "note.txt 已创建");
    Ok(())
}

/// 关闭面板：q 键
#[tokio::test(flavor = "multi_thread")]
async fn filetree_close_panel() -> anyhow::Result<()> {
    let (_, mut app) = setup().await?;
    pump(&mut app, ":filetree<ret>").await?;
    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    assert!(app.compositor.has_component(panel_type), "面板打开");
    pump(&mut app, "q").await?;
    assert!(!app.compositor.has_component(panel_type), "q 关闭面板");
    Ok(())
}

/// 面板聚焦时 `C-*` 必须**穿透**给编辑器 —— 面板是非模态的（`open_panel`，不是 popup）。
///
/// 回归：filetree 的 `onKey` 曾对 ctrl/alt 返回 `"handled"`（注释却写着"穿透"）⇒
/// 用户在 `[keys]` / `helix.map` 里显式绑的 C-* 在面板聚焦时**静默失效**。
/// 这里用 `helix.map` 绑一个 C-e 探针，断言它确实触发。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_panel_passes_ctrl_keys_through() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    // 编辑器侧的 C-e 绑定（走插件命令回退，与用户 `helix.map` 同一条路）
    let probe = dir.path().join("probe.js");
    std::fs::write(
        &probe,
        r#"
helix.register_command("probe-ctrl", () => helix.echo("ctrl-through"));
helix.map("normal", "C-e", "probe-ctrl");
"#,
    )?;
    pump(&mut app, &format!(":plugin-load {}<ret>", probe.display())).await?;
    // 先打开 `dir` 里的文件再 reveal ⇒ 面板 root 由**文件路径**决定，
    // 而不是进程 cwd（cwd 是全局的，并行跑别的 `:cd` 会把 root 换走 —— 见 D3 spec §9.3）。
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    assert_ne!(
        app.compositor.layout_tree().active(),
        0,
        "前置不成立：filetree 打开后焦点应在面板"
    );

    // 面板聚焦时按 C-e：必须穿透到编辑器侧的绑定（改前被面板吞掉 ⇒ 状态栏为空）
    pump(&mut app, "<C-e>").await?;
    let st = app
        .editor
        .get_status()
        .map(|(s, _)| s.to_string())
        .unwrap_or_default();
    assert_eq!(
        st, "ctrl-through",
        "面板聚焦时 C-e 应穿透给编辑器绑定（被吞 ⇒ 空）: {st:?}"
    );

    // C-\ 不受本次改动影响：保留面板、焦点回编辑器
    pump(&mut app, "<C-\\>").await?;
    assert_eq!(
        app.compositor.layout_tree().active(),
        0,
        "C-\\ 应把焦点还给编辑器"
    );
    let text = panel_text(&mut app);
    assert!(
        text.contains("readme.md"),
        "C-\\ 只是移焦点，面板应仍在（且 root 不该被别人换走）: {text:?}"
    );
    Ok(())
}

/// `[plugins.filetree] show_hidden = true` 真的改变行为（配置 → 插件,2026-10-23 接线）
///
/// 之前这个键只声明不生效（`CFG` 取了没用），写进 config.toml 只会通过校验 ⇒ 静默无效。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_config_show_hidden_applies() -> anyhow::Result<()> {
    // 配置合并缓存是进程级的 ⇒ 与其它写 [plugins.*] 的用例串行
    let _cfg = plugin_config_lock().await;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("readme.md"), "hi\n")?;
    std::fs::write(dir.path().join(".secret.txt"), "s\n")?;
    let plugin = dir.path().join("filetree.js");
    std::fs::write(
        &plugin,
        std::fs::read_to_string(repo_plugin("filetree/plugin.js"))?,
    )?;

    let mut cfg = test_config();
    cfg.plugins = Some(toml::from_str("[filetree]\nshow_hidden = true\n")?);
    let mut app = AppBuilder::new().with_config(cfg).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    pump(
        &mut app,
        &format!(":open {}<ret>", dir.path().join("readme.md").display()),
    )
    .await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    assert!(
        panel_text(&mut app).contains(".secret.txt"),
        "show_hidden=true ⇒ 面板开局就显示 dotfile（无需按 H）"
    );
    Ok(())
}

/// 搜索（P0）：`/` 进过滤 → 逐字增量过滤 → Enter 直接打开首个命中 → Esc 退出恢复完整树。
///
/// 钉三件事：① 过滤真的裁掉了不命中的行（不是只改标题）；② 光标停在**首个真命中**而不是
/// 被保留的祖先行（否则 Enter 会变成"钻进 src/"，用户还得再按一次）；③ Esc 之后回到完整树。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_search_filter_open_and_escape() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let main_js = dir.path().join("src").join("main.js");
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    assert!(
        panel_text(&mut app).contains("readme.md"),
        "前置：完整树里应该有 readme.md"
    );

    // `/` 进搜索，输入 main：只剩 src（祖先行）+ src/main.js
    pump(&mut app, "/").await?;
    pump(&mut app, "main").await?;
    let text = panel_body(&mut app);
    assert!(text.contains("main.js"), "过滤后应剩 src/main.js: {text:?}");
    assert!(
        !text.contains("readme.md"),
        "不命中的 readme.md 应被滤掉（过滤没生效?）: {text:?}"
    );
    assert!(text.contains('⌕'), "find 模式应有查询行: {text:?}");

    // Esc → 回到 tree 模式，完整树回来（readme.md 重新可见）
    pump(&mut app, "<esc>").await?;
    let text = panel_body(&mut app);
    assert!(text.contains("readme.md"), "Esc 后应恢复完整树: {text:?}");
    assert!(!text.contains('⌕'), "退出后查询行应消失: {text:?}");

    // 再搜一次，Enter 直接打开首个真命中（main.js），不是钻目录
    pump(&mut app, "/").await?;
    pump(&mut app, "main").await?;
    pump(&mut app, "<ret>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.path()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
        main_js.to_str(),
        "Enter 应打开搜索命中的 src/main.js"
    );
    Ok(())
}

/// 搜索（P1）：`/` 之后 `Tab` 切「全库」——`rg --files` 的列表 + nucleo 打分，
/// 行是**扁平相对路径**（不是树）。异步列表到达后光标落在首个命中，Enter 直接打开。
///
/// 钉的是 P1 的接线：`run_async(["rg",…])` → `flat_items` → `helix.fuzzy` → 合成节点。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_search_all_scope_uses_rg_files() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let main_js = dir.path().join("src").join("main.js");
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;

    pump(&mut app, "/").await?;
    pump(&mut app, "main").await?;
    pump(&mut app, "<tab>").await?; // 切全库（异步列表）

    // rg 是异步的：等列表到达（查询行的计数从"载入中"变成"全库 · N 项"）
    // CJK 在渲染行里占两列（"全 库"）⇒ 条件判断也要先压掉空白，否则永远等不到
    let text = wait_panel(&mut app, Duration::from_secs(20), "全库列表", |t| {
        let c: String = t.chars().filter(|c| !c.is_whitespace()).collect();
        c.contains("全库·") && !c.contains("载入中")
    })
    .await?;
    // 注意：CJK 在渲染行里占两列（"全 库"，字符后跟一个填充空格）⇒ 断言前先压掉空白
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.contains("全库"), "查询行应标出全库范围: {text:?}");
    assert!(
        text.contains("src/main.js"),
        "全库行应显示**相对路径**（同名文件靠路径区分）: {text:?}"
    );

    pump(&mut app, "<ret>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.path()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
        main_js.to_str(),
        "全库视图下 Enter 应打开命中的 src/main.js"
    );
    Ok(())
}

/// `:filetree-find`：原生 picker 的数据源（`rg --files`）能选中并打开文件。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_find_command_opens_from_picker() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let main_js = dir.path().join("src").join("main.js");
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;

    let picker_type = std::any::type_name::<Overlay<Picker<PickerRow, ()>>>();
    // items 是 async（rg 在 worker 线程跑）⇒ 层要等结果回来才推上来 ⇒ 轮询等它出现
    pump_until(
        &mut app,
        ":filetree-find<ret>",
        Duration::from_secs(10),
        "picker 打开",
        |app| app.compositor.has_component(picker_type),
    )
    .await?;

    // 先把查询打进去，再**等选中项真的变成 src/main.js**，最后才 Enter。
    // 一次性 `pump("main<ret>")` 在慢机器上会让 Enter 赶在过滤之前生效 —— 实测 CI 上
    // 打开的是当前文件（readme.md）而不是选中的 main.js。
    //
    // 判据用**右侧预览**：picker 的候选行不在这条渲染路径里（`render_rows` 只拿到浮层骨架），
    // 但预览显示的是"选中项的内容"，`console.log(1)` 正是 setup 里 src/main.js 的正文。
    pump(&mut app, "main").await?;
    wait_frame(
        &mut app,
        Duration::from_secs(20),
        "picker 选中 main.js（预览显示其内容）",
        |f| f.contains("console.log(1)"),
    )
    .await?;
    pump(&mut app, "<ret>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.path()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
        main_js.to_str(),
        "picker 里 Enter 应打开选中的 src/main.js"
    );
    Ok(())
}

/// 内容搜索（P2）：`G` / `:filetree-search <词>` → `rg --json` 结果树
/// （文件行为父、命中行为子），Enter 跳到**那一行**。
///
/// 钉三件事：① rg 的输出被解析成树（不是一坨平文本）；② 行号是 **0 起始**且真的被用来跳转；
/// ③ 解析走 `--json` 而不是 `split(":")`（Windows 盘符）。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_grep_search_tree_and_jump() -> anyhow::Result<()> {
    if !has_ripgrep() {
        return Ok(()); // 内容搜索走 `rg --json`，没有 rg 就没有等价实现（全库列表才有 read_tree 兜底）
    }
    let (dir, mut app) = setup().await?;
    let main_js = dir.path().join("src").join("main.js");
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;

    // 命令形态（`<词>` 来自 helix.command_args()）
    pump(&mut app, ":filetree-search console<ret>").await?;

    // rg 是异步的：等内容树出现（文件行 + 命中行）
    let text = wait_panel(
        &mut app,
        Duration::from_secs(20),
        "内容搜索结果",
        |t| t.contains("console.log(1)"),
    )
    .await?;
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        compact.contains("内容·"),
        "查询行应显示内容搜索计数: {text:?}"
    );
    assert!(text.contains("src/main.js"), "文件行应是相对路径: {text:?}");
    assert!(
        text.contains("console.log(1)"),
        "命中行应是该行原文: {text:?}"
    );
    assert!(text.contains('1'), "命中行图标位应是行号: {text:?}");

    // 光标已在首个命中行 ⇒ Enter 直接跳到那一行（第 1 行 = 0 起始 0）
    pump(&mut app, "<ret>").await?;
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.path()
            .map(|p| p.to_string_lossy().into_owned())
            .as_deref(),
        main_js.to_str(),
        "Enter 应打开命中所在的 src/main.js"
    );
    let pos = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    assert_eq!(
        pos, 0,
        "光标应落在命中的那一行（rg 的 1 起始 → open_file 的 0 起始）"
    );
    Ok(())
}

/// 回归：搜索态里**字符键必须当输入**（`j` 曾被导航吞掉 ⇒ 搜不到名字带 j 的文件）
#[tokio::test(flavor = "multi_thread")]
async fn filetree_find_takes_letter_keys_as_input() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;

    pump(&mut app, "/").await?;
    pump(&mut app, "j").await?; // `j` 若是导航键，过滤就不会生效
    let text = panel_body(&mut app);
    assert!(
        text.contains("main.js"),
        "query \"j\" 应命中 main.js: {text:?}"
    );
    assert!(
        !text.contains("readme.md"),
        "字符 j 被当成导航吞掉了（过滤没生效）: {text:?}"
    );
    Ok(())
}

/// 过滤表达式（P3）：`>` 进输入 → `*.md` → Enter 应用（树里只剩 md）；
/// 再 `>` 删空后 Enter 清除（全部回来）。
///
/// 钉三件事：① 表达式**应用**后留在树模式（不是一按 Enter 就丢）；② glob 的 basename 语义
/// 真的把 `src/` 整个滤掉了；③ 删空 = 清除过滤（用户不必记住怎么关）。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_filter_dsl_apply_and_clear() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    // 注意：`src/` 此时是折叠的（reveal 只展开了 readme.md 所在的那条链）⇒ 前置断言看 `src`
    assert!(
        panel_body(&mut app).contains("src"),
        "前置：完整树含 src 目录"
    );

    // `>` 进过滤输入（`<gt>` 是宏里的 `>`），输入 *.md 并应用
    pump(&mut app, "<gt>").await?;
    pump(&mut app, "*.md<ret>").await?;
    let text = panel_body(&mut app);
    assert!(
        text.contains("readme.md"),
        "`*.md` 应留下 readme.md: {text:?}"
    );
    // 进过滤时做过一次有预算的补加载 ⇒ src 的子节点已加载，`main.js` 不命中 `*.md`，
    // 于是 `src` 连"祖先保留"的资格都没有（整棵子树枝被滤掉）
    assert!(!text.contains("src"), "`*.md` 应把 src/ 整枝滤掉: {text:?}");
    assert!(!text.contains("filetree.js"), "也滤掉别的 .js: {text:?}");

    // 再进 `>`：输入框应从已应用的表达式接着改；删空 + Enter = 清除
    pump(&mut app, "<gt>").await?;
    pump(
        &mut app,
        "<backspace><backspace><backspace><backspace><ret>",
    )
    .await?;
    let text = panel_body(&mut app);
    assert!(text.contains("src"), "清空表达式后应恢复完整树: {text:?}");
    assert!(
        text.contains("filetree.js"),
        "恢复后 .js 也应回来: {text:?}"
    );
    Ok(())
}

/// git 状态列 + `#` 谓词（P3）：真仓库里 `M`/`?` 能显示，也能按 `#M` / `#?` 过滤。
///
/// 没装 git 就**跳过**（不该因为环境没 git 就红）；断言只看含文件名的行，避开边框/表头。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_git_status_and_hash_predicates() -> anyhow::Result<()> {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return Ok(()); // 环境没 git：跳过
    }
    let _cfg = plugin_config_lock().await;
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_str().unwrap().to_string();
    let git = |args: &[&str]| -> anyhow::Result<()> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(())
    };
    git(&["init", "-q"])?;
    std::fs::write(dir.path().join("alpha.txt"), "one\n")?;
    git(&["add", "alpha.txt"])?;
    git(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-qm",
        "one",
    ])?;
    std::fs::write(dir.path().join("alpha.txt"), "one\ntwo\n")?; // 已跟踪 + 改动 ⇒ M
    std::fs::write(dir.path().join("beta.txt"), "new\n")?; // 未跟踪 ⇒ ?
    std::fs::write(dir.path().join("gamma.rs"), "fn main() {}\n")?; // 未跟踪 ⇒ ?

    let plugin = dir.path().join("filetree.js");
    std::fs::write(
        &plugin,
        std::fs::read_to_string(repo_plugin("filetree/plugin.js"))?,
    )?;
    let mut cfg = test_config();
    cfg.plugins = Some(toml::from_str("[filetree]\ngit_status = true\n")?);
    let mut app = AppBuilder::new().with_config(cfg).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    pump(
        &mut app,
        &format!(":open {}<ret>", dir.path().join("alpha.txt").display()),
    )
    .await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;

    // git 状态是异步读的：**必须用 pump 推动事件循环**（只 render + sleep 不泵 JS 作业，
    // 实测 promise 的 .then 永远不跑）；`<up>` 是面板里无副作用的手柄键。
    let text = wait_panel(&mut app, Duration::from_secs(20), "git 状态列", |t| {
        t.contains('M') && t.contains('?')
    })
    .await?;
    let line_of = |text: &str, name: &str| -> String {
        text.lines()
            .find(|l| l.contains(name))
            .unwrap_or_default()
            .to_string()
    };
    assert!(
        line_of(&text, "alpha.txt").contains('M'),
        "改动过的 alpha.txt 应有 M 标记: {:?}",
        line_of(&text, "alpha.txt")
    );
    assert!(
        line_of(&text, "beta.txt").contains('?'),
        "未跟踪的 beta.txt 应有 ? 标记: {:?}",
        line_of(&text, "beta.txt")
    );

    // `#M` 谓词：只剩 alpha.txt
    pump(&mut app, "<gt>").await?;
    pump(&mut app, "#M<ret>").await?;
    let text = panel_body(&mut app);
    assert!(
        text.contains("alpha.txt"),
        "#M 应留下改动过的文件: {text:?}"
    );
    assert!(!text.contains("beta.txt"), "#M 应滤掉未跟踪文件: {text:?}");
    assert!(
        !text.contains("gamma.rs"),
        "#M 应滤掉未跟踪的 gamma.rs: {text:?}"
    );

    // `#?` 谓词：只剩未跟踪的两个
    pump(&mut app, "<gt>").await?;
    pump(&mut app, "<backspace><backspace><ret>").await?;
    pump(&mut app, "<gt>").await?;
    pump(&mut app, "#?<ret>").await?;
    let text = panel_body(&mut app);
    assert!(text.contains("beta.txt"), "#? 应留下未跟踪文件: {text:?}");
    assert!(!text.contains("alpha.txt"), "#? 应滤掉已跟踪文件: {text:?}");
    Ok(())
}

/// 多打开标记（③）：`●` = 当前文件，`·` = 已打开但非当前，其余没有标记。
///
/// 回归点：以前只有"当前文件"有圆点 ⇒ 切走之后面板再也看不出哪些文件还开着。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_marks_open_buffers() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let readme = dir.path().join("readme.md");
    let main_js = dir.path().join("src").join("main.js");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;

    // 再开一个文件 ⇒ readme.md 变成"已打开但非当前"，main.js 是当前
    pump(&mut app, &format!(":open {}<ret>", main_js.display())).await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    let text = panel_body(&mut app);
    let line_of = |name: &str| -> String {
        text.lines()
            .find(|l| l.contains(name))
            .unwrap_or_default()
            .to_string()
    };
    assert!(
        line_of("main.js").contains('●'),
        "当前文件应有 ●: {:?}",
        line_of("main.js")
    );
    assert!(
        line_of("readme.md").contains('·'),
        "已打开但非当前应有 ·: {:?}",
        line_of("readme.md")
    );
    assert!(
        !line_of("filetree.js").contains('●') && !line_of("filetree.js").contains('·'),
        "未打开的文件不该有标记: {:?}",
        line_of("filetree.js")
    );

    // 关掉**当前** buffer（此刻是 main.js）：标记要随之消失。
    // 核心不为 `:buffer-close` 发任何插件事件 ⇒ 面板在下次按键时会用 buffer.list() 重新播种，
    // 所以这里补一次按键（`<up>` 是面板里的无副作用键）。
    pump(&mut app, ":buffer-close!<ret>").await?;
    pump(&mut app, "<up>").await?;
    let text = panel_body(&mut app);
    let main_line = text
        .lines()
        .find(|l| l.contains("main.js"))
        .unwrap_or_default()
        .to_string();
    assert!(
        !main_line.contains('●') && !main_line.contains('·'),
        "关掉的 buffer 不该再有任何标记: {main_line:?}"
    );
    assert!(
        text.lines()
            .find(|l| l.contains("readme.md"))
            .unwrap_or_default()
            .contains('·'),
        "readme.md 仍开着（只是非当前）⇒ 仍应有 ·: {text:?}"
    );
    Ok(())
}

/// 等面板正文满足条件 —— **每轮都 `pump` 一次**。
///
/// 为什么必须 pump：`rg` / nucleo / git 都是异步的，它们的 promise 续体只有在事件循环被驱动时
/// 才跑（只 `sleep` + 渲染是不够的 —— 本地机器快、恰好在之前的 `pump` 里就跑完了，CI 的
/// 2 核机器上则永远等不到，于是同一个用例本地绿、CI 红）。
/// 用 `<pageup>` 当"驱动键"：它在**所有**模式里都是无害的（列表导航，光标夹到顶部），
/// 不像 `?`/`j` 之类会在搜索态被当成输入。
async fn wait_panel(
    app: &mut Application,
    budget: Duration,
    what: &str,
    ok: impl Fn(&str) -> bool,
) -> anyhow::Result<String> {
    let t0 = std::time::Instant::now();
    loop {
        let body = panel_body(app);
        if ok(&body) {
            return Ok(body);
        }
        anyhow::ensure!(
            t0.elapsed() < budget,
            "等待超时（{what}，预算 {budget:?}）：\n{body}"
        );
        pump(app, "<pageup>").await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 等**整帧**（含浮层）满足条件 —— 同样每轮 pump（见 [`wait_panel`]）。
async fn wait_frame(
    app: &mut Application,
    budget: Duration,
    what: &str,
    ok: impl Fn(&str) -> bool,
) -> anyhow::Result<String> {
    let t0 = std::time::Instant::now();
    loop {
        let frame = frame_text(app);
        if ok(&frame) {
            return Ok(frame);
        }
        anyhow::ensure!(
            t0.elapsed() < budget,
            "等待超时（{what}，预算 {budget:?}）：\n{frame}"
        );
        pump(app, "<pageup>").await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 这份构建里有没有 `rg`：内容搜索（`rg --json`）**必须**有它，全库列表则有 read_tree 兜底。
fn has_ripgrep() -> bool {
    std::process::Command::new("rg")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 整帧文本（不裁列）—— 双面板时两个根名都在同一行里，用 `panel_text`（只取左 32 列）看不到第二个
fn frame_text(app: &mut Application) -> String {
    render_rows(app, helix_view::graphics::Rect::new(0, 0, 120, 30))
        .iter()
        .map(|r| r.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 双实例（第 4 步）：`:filetree-open <dir>` 真的开出第二个面板，两个 root 各自成树；
/// 再开第三个会被 `max_panels`（默认 2）拒绝，且状态栏有说明。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_two_panels_side_by_side() -> anyhow::Result<()> {
    let (dir_a, mut app) = setup().await?;
    // 第二个 root：另一个临时目录（带一个只在它里面出现的文件名）
    let dir_b = tempfile::tempdir()?;
    std::fs::write(dir_b.path().join("beta.md"), "b\n")?;
    let name_a = dir_a
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let name_b = dir_b
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let readme = dir_a.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    pump(
        &mut app,
        &format!(":filetree-open {}<ret>", dir_b.path().display()),
    )
    .await?;

    let frame = frame_text(&mut app);
    assert!(
        frame.contains(&name_a),
        "面板 A 的 root 名应在帧里: {frame}"
    );
    assert!(
        frame.contains(&name_b),
        "面板 B 的 root 名应在帧里: {frame}"
    );
    assert!(frame.contains("beta.md"), "面板 B 应显示自己的树: {frame}");

    // 第三个：被上限拒绝（状态栏有说明，且帧里没多出第三个面板）
    pump(
        &mut app,
        &format!(":filetree-open {}<ret>", dir_a.path().display()),
    )
    .await?;
    let st = status_text(&app);
    assert!(st.contains("最多 2 个面板"), "超上限应明确说明: {st:?}");
    Ok(())
}

/// 双实例的 `:filetree-reveal`：**归属优先** —— 文件属于哪个面板的 root 就落哪个，
/// 不动 active 面板的 root（设计文档 §4.3 的 ①② 顺序按实测调换了）。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_reveal_prefers_owning_panel() -> anyhow::Result<()> {
    let (dir_a, mut app) = setup().await?;
    let dir_b = tempfile::tempdir()?;
    std::fs::write(dir_b.path().join("beta.md"), "b\n")?;
    let name_b = dir_b
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let readme = dir_a.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    pump(
        &mut app,
        &format!(":filetree-open {}<ret>", dir_b.path().display()),
    )
    .await?;
    // 此时 active 是 B；打开 A 里的另一个文件 ⇒ 跟随/定位应落在 A（B 的 root 不能被拽走）
    let other = dir_a.path().join("src").join("main.js");
    pump(&mut app, &format!(":open {}<ret>", other.display())).await?;
    let frame = frame_text(&mut app);
    assert!(
        frame.contains(&name_b),
        "B 的 root 不该被 reveal 拽走: {frame}"
    );
    assert!(
        frame.contains("main.js"),
        "A 应该显示被 reveal 的文件: {frame}"
    );
    Ok(())
}

/// 双实例下 `:filetree` 的语义：关掉**目标**面板（active），而不是关掉全部/关错那个。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_toggle_closes_active_panel() -> anyhow::Result<()> {
    let (dir_a, mut app) = setup().await?;
    let dir_b = tempfile::tempdir()?;
    std::fs::write(dir_b.path().join("beta.md"), "b\n")?;
    let name_a = dir_a
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let name_b = dir_b
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let readme = dir_a.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;
    pump(
        &mut app,
        &format!(":filetree-open {}<ret>", dir_b.path().display()),
    )
    .await?;
    assert!(frame_text(&mut app).contains(&name_b), "前置：B 已开");

    pump(&mut app, ":filetree<ret>").await?; // 关 active（B）
    let frame = frame_text(&mut app);
    assert!(!frame.contains(&name_b), "B 应被关掉: {frame}");
    assert!(frame.contains(&name_a), "A 必须还在: {frame}");
    Ok(())
}

/// 回归：打开 → `Esc` 关闭 → **能再次打开**。
///
/// 曾经不行：`close_panel` 先把 `panel.id` 置空、再 `G.panels.delete(panel.id)` ⇒ 删的是
/// `null` 键，注册表里留着一个死条目 ⇒ `toggle_panel()` 永远走"关"分支，而且**没有任何提示**
/// （用户看到的就是"按 C-e 没反应"）。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_reopen_after_esc() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let readme = dir.path().join("readme.md");
    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;

    pump(&mut app, ":filetree<ret>").await?;
    assert!(app.compositor.has_component(panel_type), "第一次应打开");

    pump(&mut app, "<esc>").await?;
    assert!(!app.compositor.has_component(panel_type), "Esc 应关闭面板");

    pump(&mut app, ":filetree<ret>").await?;
    assert!(
        app.compositor.has_component(panel_type),
        "关闭后应能再次打开；状态栏: {:?}",
        status_text(&app)
    );
    // 再关一次、再开一次（防"只能开两次"的计数器类 bug）
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree<ret>").await?;
    assert!(
        app.compositor.has_component(panel_type),
        "第二轮开/关后仍能打开"
    );
    Ok(())
}

/// filetree 占 **root**：编辑器先分屏后再开树，树仍应是**整列全高**（而不是只挂在半屏上）。
///
/// 布局目标（用户诉求）：
/// ```text
/// filetree | editor
///          | term
/// ```
#[tokio::test(flavor = "multi_thread")]
async fn filetree_takes_full_height_left_column() -> anyhow::Result<()> {
    let (dir, mut app) = setup().await?;
    let readme = dir.path().join("readme.md");
    pump(&mut app, &format!(":open {}<ret>", readme.display())).await?;
    // 先让编辑器分屏（模拟"编辑器 + 终端"已经分过的现场）
    pump(&mut app, ":vsplit<ret>").await?;
    pump(&mut app, ":filetree<ret>").await?;

    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    let height = rows.len();
    // 面板边框在最左列：全高时几乎每一行都以 │/┌/└ 开头
    let left_border = rows
        .iter()
        .filter(|r| r.starts_with('│') || r.starts_with('┌') || r.starts_with('└'))
        .count();
    assert!(
        left_border + 2 >= height,
        "面板应占满整列高度（{left_border}/{height} 行有左边框）:\n{}",
        rows.join("\n")
    );
    Ok(())
}

/// `[plugins.filetree] close_on_esc = false`：`Esc` **只把焦点还给编辑器**，面板保留；
/// 关面板改用 `q`。
///
/// 为什么需要这个开关：树常驻在左侧（`at:"root"`）时，用户按 `Esc` 的意图多是"回到编辑器"，
/// 而不是"把树关掉再开一次"。
#[tokio::test(flavor = "multi_thread")]
async fn filetree_close_on_esc_false_keeps_panel() -> anyhow::Result<()> {
    // 配置合并缓存是进程级的 ⇒ 与其它写 [plugins.*] 的用例串行
    let _cfg = plugin_config_lock().await;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("readme.md"), "hi\n")?;
    let plugin = dir.path().join("filetree.js");
    std::fs::write(
        &plugin,
        std::fs::read_to_string(repo_plugin("filetree/plugin.js"))?,
    )?;

    let mut cfg = test_config();
    cfg.plugins = Some(toml::from_str("[filetree]\nclose_on_esc = false\n")?);
    let mut app = AppBuilder::new().with_config(cfg).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    pump(
        &mut app,
        &format!(":open {}<ret>", dir.path().join("readme.md").display()),
    )
    .await?;
    pump(&mut app, ":filetree<ret>").await?;
    pump(&mut app, ":filetree-reveal<ret>").await?;

    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    assert!(app.compositor.has_component(panel_type), "前置：面板已开");

    // Esc：面板**仍在**，焦点回到编辑器
    pump(&mut app, "<esc>").await?;
    assert!(
        app.compositor.has_component(panel_type),
        "close_on_esc=false 时 Esc 不该关面板"
    );
    assert_eq!(
        app.compositor.layout_tree().active(),
        0,
        "Esc 应把焦点还给编辑器"
    );

    // 面板仍然能被关掉（不是"开了就关不掉"）：`q` 与 `:filetree` 这两条出口都没被这配置改动。
    // （`q` 的路径与配置无关，已由 `filetree_close_panel` 在默认配置下覆盖。）
    pump(&mut app, ":filetree<ret>").await?;
    assert!(!app.compositor.has_component(panel_type), "toggle 仍是开关");
    Ok(())
}
