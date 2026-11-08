//! 代码折叠（P0）：`zf`/`zo`/`za`/`zM`/`zR` + `fold-jump` 两种策略。
//!
//! 计划：`docs/superpowers/plans/2026-11-07-code-folding.md`。
//! 排版层面的行算术在 `helix-core`（`doc_formatter` / `fold`）已单测，这里测**用户动作**。

use super::*;
use helix_term::application::Application;
use helix_view::editor::FoldJump;

/// 两行正文的可折叠块：缩进启发式给出 `0..3`（头行 + 2 行正文）。
/// 光标停在头行行首（`#[|]#` 是测试框架的选区标记）。
const SRC: &str = "#[|]#fn a() {\n    let x = 1;\n    let y = 2;\n}\n";

fn build(jump: FoldJump) -> anyhow::Result<Application> {
    let mut config = test_config();
    config.editor.fold_jump = jump;
    AppBuilder::new()
        .with_input_text(SRC)
        .with_config(config)
        .build()
}

/// 当前文档的折叠数量与头行隐藏行数。
fn folds(app: &Application) -> (usize, Vec<usize>) {
    let doc = app.editor.documents().next().expect("有文档");
    let text = doc.text().slice(..);
    (
        doc.folds.len(),
        doc.folds
            .all()
            .iter()
            .map(|f| f.hidden_lines(text))
            .collect(),
    )
}

fn cursor_char(app: &Application) -> usize {
    let (view, doc) = helix_view::current_ref!(app.editor);
    doc.selection(view.id)
        .primary()
        .cursor(doc.text().slice(..))
}

/// `zM` 按缩进把块折上；`zR` 全部展开。
#[tokio::test(flavor = "multi_thread")]
async fn z_m_folds_and_z_r_opens() -> anyhow::Result<()> {
    let mut app = build(FoldJump::Open)?;
    assert_eq!(folds(&app), (0, vec![]), "默认没有折叠");

    pump(&mut app, "zM").await?;
    assert_eq!(folds(&app), (1, vec![2]), "折一个块，藏 2 行");

    pump(&mut app, "zR").await?;
    assert_eq!(folds(&app), (0, vec![]), "全部展开");
    Ok(())
}

/// `za` 一切：有就开、没有就折；`zo` 只开。
#[tokio::test(flavor = "multi_thread")]
async fn z_a_toggles_and_z_o_opens() -> anyhow::Result<()> {
    let mut app = build(FoldJump::Open)?;

    pump(&mut app, "za").await?;
    assert_eq!(folds(&app), (1, vec![2]), "光标在头行 ⇒ 折上");

    pump(&mut app, "za").await?;
    assert_eq!(folds(&app), (0, vec![]), "再按一次 ⇒ 展开");

    pump(&mut app, "za").await?;
    pump(&mut app, "zo").await?;
    assert_eq!(folds(&app), (0, vec![]), "zo 展开");
    Ok(())
}

/// `zf` 折上后重复按不重复折（幂等）。
#[tokio::test(flavor = "multi_thread")]
async fn z_f_is_idempotent() -> anyhow::Result<()> {
    let mut app = build(FoldJump::Open)?;
    pump(&mut app, "zf").await?;
    pump(&mut app, "zf").await?;
    assert_eq!(folds(&app), (1, vec![2]));
    Ok(())
}

/// 默认策略 `open`：`j` 走进折叠块正文 ⇒ 自动展开（vim `foldopen` 手感）。
#[tokio::test(flavor = "multi_thread")]
async fn jump_open_expands_the_fold() -> anyhow::Result<()> {
    let mut app = build(FoldJump::Open)?;
    pump(&mut app, "zM").await?;
    assert_eq!(folds(&app), (1, vec![2]));

    pump(&mut app, "j").await?;
    assert_eq!(folds(&app), (0, vec![]), "进块正文 ⇒ 折叠被展开");
    assert_eq!(cursor_char(&app), 9, "光标落在第二行行首");
    Ok(())
}

/// 策略 `skip`：光标不被吞进折叠区，而是**越过整个块**落到块后第一行。
#[tokio::test(flavor = "multi_thread")]
async fn jump_skip_leaves_the_fold_closed() -> anyhow::Result<()> {
    let mut app = build(FoldJump::Skip)?;
    pump(&mut app, "zM").await?;
    assert_eq!(folds(&app), (1, vec![2]));

    pump(&mut app, "j").await?;
    assert_eq!(folds(&app), (1, vec![2]), "折叠保持关着");
    let doc = app.editor.documents().next().unwrap();
    let text = doc.text().slice(..);
    let fold = doc.folds.all()[0];
    assert_eq!(cursor_char(&app), fold.end, "光标被送到块后那一行");
    assert_eq!(text.char_to_line(fold.end), 3, "也就是 `}}` 那一行");
    Ok(())
}

/// P1：`fold-method = "tree-sitter"` 用 `folds.scm` —— 精度比缩进法高：
/// `(function_item)` 的整个跨度都折起来（**`}` 那一行也藏**），缩进法会把 `}` 留在外面。
#[tokio::test(flavor = "multi_thread")]
async fn tree_sitter_folds_whole_function() -> anyhow::Result<()> {
    use helix_view::editor::FoldMethod;

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("a.rs");
    std::fs::write(&path, SRC)?;

    let mut config = test_config();
    config.editor.fold_method = FoldMethod::TreeSitter;
    let mut app = AppBuilder::new()
        .with_file(path, None)
        .with_config(config)
        .build()?;

    pump(&mut app, "zM").await?;
    assert_eq!(
        folds(&app),
        (1, vec![3]),
        "(function_item) 连同 `}}` 一起折（3 行正文）"
    );

    let doc = app.editor.documents().next().unwrap();
    let text = doc.text().slice(..);
    let fold = doc.folds.all()[0];
    assert_eq!(text.char_to_line(fold.start), 0, "头行是函数第一行");
    assert_eq!(text.char_to_line(fold.end), 4, "块后是文件末尾那一行");
    Ok(())
}

/// 没有 `folds.scm` 的语言（这里用不认识的扩展名 ⇒ 无语法）⇒ 回退缩进启发式。
#[tokio::test(flavor = "multi_thread")]
async fn tree_sitter_falls_back_to_indent() -> anyhow::Result<()> {
    use helix_view::editor::FoldMethod;

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("a.unlikelyext");
    std::fs::write(&path, SRC)?;

    let mut config = test_config();
    config.editor.fold_method = FoldMethod::TreeSitter;
    let mut app = AppBuilder::new()
        .with_file(path, None)
        .with_config(config)
        .build()?;

    pump(&mut app, "zM").await?;
    assert_eq!(
        folds(&app),
        (1, vec![2]),
        "回退 indent：只藏 2 行，`}}` 留着"
    );
    Ok(())
}

/// P2：`fold-method = "lsp"` 用 LSP 的 `textDocument/foldingRange`
/// （mock server 固定给「第 0~3 行」一个块 ⇒ 折 3 行正文）。
#[tokio::test(flavor = "multi_thread")]
async fn lsp_folding_ranges_are_used() -> anyhow::Result<()> {
    use std::time::Duration;

    use helix_view::editor::FoldMethod;

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.mock");
    std::fs::write(&file, "l0\nl1\nl2\nl3\nl4\nl5\n")?;

    let mut config = test_config_with_lsp();
    config.editor.fold_method = FoldMethod::Lsp;
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(config)
        .with_lang_loader(mock_lsp_loader("folding_basic", &[]))
        .build()?;

    // 打开文档 / LSP 握手完成时 handler 会各请求一次 foldingRange；等它落进缓存。
    // 心跳键用 `<esc>`：每次都给事件循环一个真实事件，才能把 `job::dispatch` 的回包泵进来。
    let mut arrived = false;
    for _ in 0..100 {
        if app
            .editor
            .documents()
            .any(|doc| !doc.lsp_fold_ranges.is_empty())
        {
            arrived = true;
            break;
        }
        pump(&mut app, "<esc>").await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        arrived,
        "LSP foldingRange 未到达；status={}",
        status_text(&app)
    );

    pump(&mut app, "zM").await?;
    assert_eq!(folds(&app), (1, vec![3]), "LSP 的 0..=3 行 ⇒ 藏 3 行");
    Ok(())
}

/// P3：插件 API —— `helix.fold.toggle/close/open/open_all/close_all` 与 `list()`，
/// 与键位走同一份实现（`Document.folds`）。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_fold_api_drives_the_same_state() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let plugin = dir.path().join("fold.js");
    std::fs::write(
        &plugin,
        r#"
        helix.register_command("fold-show", () => {
            const folds = helix.fold.list() || [];
            helix.echo("folds:" + folds.length);
        });
        helix.register_command("fold-do", () => {
            helix.fold.toggle(0);
            helix.fold.close(0);
            helix.fold.toggle(2);
            helix.fold.open_all();
            helix.fold.close_all();
        });
        "#,
    )?;

    let mut app = AppBuilder::new()
        .with_input_text(SRC)
        .with_config(test_config())
        .build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;

    // 插件读到的是同一份折叠状态
    pump(&mut app, ":fold-show<ret>").await?;
    assert!(
        status_text(&app).contains("folds:0"),
        "初始无折叠；status={}",
        status_text(&app)
    );

    // 变更请求：toggle(0) 折 → close(0) 幂等 → toggle(2) 展开 → open_all 清空 → close_all 折全部
    pump(&mut app, ":fold-do<ret>").await?;
    assert_eq!(
        folds(&app),
        (1, vec![2]),
        "最后一步 close_all 按 indent 折出 1 个块"
    );

    pump(&mut app, ":fold-show<ret>").await?;
    assert!(
        status_text(&app).contains("folds:1"),
        "插件能看到 close_all 的结果；status={}",
        status_text(&app)
    );
    Ok(())
}

/// gutter 折叠标记：`fold-column = "always"` ⇒ 没折叠时也能看到 `▼`（可折），
/// `zM` 之后那些行变成 `▶`（已折）。`auto`（默认）在没有折叠时**不占列**。
#[tokio::test(flavor = "multi_thread")]
async fn gutter_shows_foldable_and_folded_markers() -> anyhow::Result<()> {
    use helix_view::editor::FoldColumn;
    use helix_view::graphics::Rect;

    let mut config = test_config();
    config.editor.fold_column = FoldColumn::Always;
    let mut app = AppBuilder::new()
        .with_input_text(SRC)
        .with_config(config)
        .build()?;

    // 没折叠：可折的行标 `▼`
    pump(&mut app, "zR").await?;
    let screen = render_rows(&mut app, Rect::new(0, 0, 120, 30)).concat();
    assert!(
        screen.contains('▼'),
        "可折但没折的行应有 ▼；实际屏幕：{screen:?}"
    );
    assert!(!screen.contains('▶'), "还没折叠，不该有 ▶");

    // 折上：折叠头行变 `▶`
    pump(&mut app, "zM").await?;
    let screen = render_rows(&mut app, Rect::new(0, 0, 120, 30)).concat();
    assert!(screen.contains('▶'), "折叠头行应有 ▶；实际屏幕：{screen:?}");
    Ok(())
}

/// 默认 `auto`：没有折叠的文档**不占**标记列；`always` 恰好多占一列（行号右移一格）。
#[tokio::test(flavor = "multi_thread")]
async fn gutter_column_is_absent_without_folds_in_auto_mode() -> anyhow::Result<()> {
    use helix_view::editor::FoldColumn;
    use helix_view::graphics::Rect;

    /// (第一行内容里行号数字的**字符**位置, 整屏)
    async fn probe(column: FoldColumn) -> anyhow::Result<(usize, String)> {
        let mut config = test_config();
        config.editor.fold_column = column;
        let mut app = AppBuilder::new()
            .with_input_text(SRC)
            .with_config(config)
            .build()?;
        let rows = render_rows(&mut app, Rect::new(0, 0, 120, 30));
        let line = rows.get(1).cloned().unwrap_or_default();
        let pos = line
            .chars()
            .position(|c| c.is_ascii_digit())
            .ok_or_else(|| anyhow::anyhow!("行号没画出来：{line:?}"))?;
        Ok((pos, rows.concat()))
    }

    let (auto_pos, auto_screen) = probe(FoldColumn::Auto).await?;
    assert!(
        !auto_screen.contains('▼') && !auto_screen.contains('▶'),
        "auto 模式下没有折叠就不该有标记列"
    );

    let (always_pos, always_screen) = probe(FoldColumn::Always).await?;
    assert!(
        always_screen.contains('▼'),
        "always 模式下可折的行应有 ▼；实际屏幕：{always_screen:?}"
    );
    assert_eq!(
        always_pos,
        auto_pos + 1,
        "always 恰好比 auto 多占一列（行号右移一格）"
    );
    Ok(())
}

/// `helix.layout.get()` 带上折叠态；`helix.layout.restore(dump)` 把它恢复回来。
/// （`:layout save/load` 存的就是这份 JSON ⇒ 命名布局自带折叠。）
#[tokio::test(flavor = "multi_thread")]
async fn layout_get_carries_folds_and_restore_applies_them() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, SRC)?;
    let plugin = dir.path().join("lay.js");
    std::fs::write(
        &plugin,
        r#"
        helix.register_command("lay-save", () => {
            globalThis.__dump = helix.layout.get();
            const d = globalThis.__dump;
            const f = d && d.folds;
            const n = f ? Object.keys(f).length : 0;
            helix.echo("layfolds:" + n + " dump:" + (d ? Object.keys(d).join(",") : "null"));
        });
        helix.register_command("lay-open-all", () => {
            helix.fold.open_all();
        });
        helix.register_command("lay-restore", () => {
            helix.layout.restore(globalThis.__dump);
        });
        "#,
    )?;
    let mut app = AppBuilder::new()
        .with_file(file, None)
        .with_config(test_config())
        .build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;

    // 折一个块，然后取布局快照 ⇒ 里面应有 1 个文档带折叠
    pump(&mut app, "za").await?;
    assert_eq!(folds(&app).0, 1, "先折上");
    pump(&mut app, ":lay-save<ret>").await?;
    assert!(
        status_text(&app).contains("layfolds:1"),
        "layout.get() 应带 folds；status={}",
        status_text(&app)
    );

    // 全开 ⇒ 折叠没了；再 restore 那份 dump ⇒ 折叠回来
    pump(&mut app, ":lay-open-all<ret>").await?;
    assert_eq!(folds(&app).0, 0, "open_all 之后没有折叠");
    pump(&mut app, ":lay-restore<ret>").await?;
    let st = status_text(&app);
    assert_eq!(
        folds(&app),
        (1, vec![2]),
        "restore 把折叠还回来了（status={st}）"
    );
    Ok(())
}
