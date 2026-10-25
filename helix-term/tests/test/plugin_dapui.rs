//! 内置插件 `dapui` 的 UI 行为（不需要真 debug adapter）。
//!
//! 用 `render_rows` 读**真实渲染结果**（不是进程内 pty、不是抓屏），断言：
//! 底部面板的 tab 条、REPL 输入路径、Output 空态。

use super::*;

fn dapui_plugin() -> String {
    format!("{}/../plugins/dapui/plugin.js", env!("CARGO_MANIFEST_DIR"))
}

fn screen(app: &mut helix_term::application::Application) -> String {
    render_rows(app, helix_view::graphics::Rect::new(0, 0, 120, 40)).join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn bottom_panel_has_repl_and_output_tabs() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "AAA\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", dapui_plugin())).await?;
    // 先渲染一次：面板的几何按 `compositor.area` 算（size/总行数），
    // 不先把 area 定下来，面板会按默认面积算出极小的比例（测试会看出 4 行）。
    let _ = screen(&mut app);
    pump(&mut app, ":dapui-repl<ret>").await?;

    let text = screen(&mut app);
    assert!(text.contains("REPL"), "底部面板应有 REPL tab: {text}");
    assert!(text.contains("Output"), "底部面板应有 Output tab: {text}");
    assert!(
        text.contains("no session"),
        "没有会话时徽标要说清（不许空白）: {text}"
    );
    Ok(())
}

/// `:dapui-output` 也能开同一个底部面板并切到 Output tab（tab 条是真渲染的）。
///
/// 注：REPL 输入 / Output 环形缓冲等**逻辑**在 `plugins/dapui/tests/dapui.test.js` 里用
/// node 断言（那里能精确控制状态）；这里只证明"在真引擎里插件能开面板并画出 tab 条"。
/// 不在这里断言面板内的行——面板高度由核心按 `size/视口行数` 算，测试环境视口与面板
/// 几何都不稳定，断行数只会带来假红。
#[tokio::test(flavor = "multi_thread")]
async fn output_command_opens_bottom_panel() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "AAA\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", dapui_plugin())).await?;
    pump(&mut app, ":dapui-output<ret>").await?;
    let text = screen(&mut app);
    assert!(text.contains("▸Output"), "Output tab 应处于激活态: {text}");
    assert!(text.contains("REPL"), "同一面板里另一个 tab 也在: {text}");
    Ok(())
}
