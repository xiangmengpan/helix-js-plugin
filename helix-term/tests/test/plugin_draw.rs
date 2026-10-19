//! 绘制原语端到端:`helix.draw.*` 的显示列表与 `helix.el("box")` 的边框容器
//! 一路走到真实 surface(断言**渲染出来的字符**,不是中间结构)。
//!
//! 为什么要有这一层:`Content::Ops` 是新变体,单元测试只覆盖到"光栅化成行";
//! 真正容易坏的是**行之后那段共用管线**(comp_layout → DiffRenderer → Buffer 的脏格 diff
//! 与主题解析)。这里用真实弹窗跑一遍。

use super::*;

use helix_term::application::Application;

/// 整个 60×20 视口拼成文本(去掉行尾空白,便于 `contains`)
fn screen(app: &mut Application) -> String {
    render_rows(app, helix_view::graphics::Rect::new(0, 0, 60, 20))
        .iter()
        .map(|r| r.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

async fn setup(plugin_src: &str) -> anyhow::Result<(tempfile::TempDir, Application)> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;
    let plugin = dir.path().join("draw.js");
    std::fs::write(&plugin, plugin_src)?;
    let mut app = AppBuilder::new().with_file(file, None).build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    Ok((dir, app))
}

/// 弹窗里画一个带标题的圆角盒子 + 一行中英混排文字
#[tokio::test(flavor = "multi_thread")]
async fn draw_rect_and_text_reach_the_surface() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(
        r#"
    helix.register_command("draw-box", () => {
        helix.open_popup({
            width: 24, height: 7, position: "center",
            render: () => {
                helix.draw.rect(0, 0, 20, 4, { border: "round", title: "T1", style: "ui.popup" });
                helix.draw.text(2, 1, "hi 中文");
                helix.draw.text(2, 2, "ascii row");
                helix.draw.fill(12, 2, 4, 1, { style: "ui.selection" });
                return helix.draw.done();
            },
            onKey: (k) => k.name === "Esc" ? "close" : "handled",
        });
    });
    "#,
    )
    .await?;

    pump(&mut app, ":draw-box<ret>").await?;
    let text = screen(&mut app);
    assert!(text.contains("╭ T1 "), "上边框带标题: {text}");
    assert!(text.contains("╰"), "下边框: {text}");
    // 宽字符在 cell 网格里占**两格**(第二格是占位)⇒ 渲染出来的串是 "中 文";
    // 若只占一列,这里会是 "中文"(两个汉字紧挨)。
    assert!(text.contains("hi 中 文"), "宽字符应占两格: {text}");
    // 对齐用纯 ASCII 行验证:盒子 20 列宽 ⇒ 从左上角起恰好 20 列,首尾都是竖线。
    // (整屏行首还有编辑器自己的边框,所以按**左上角所在列**取切片)
    let top = text
        .lines()
        .find(|l| l.contains("╭ T1 "))
        .expect("应有上边框");
    let col = top.chars().position(|c| c == '╭').expect("找到左上角");
    let top_slice: String = top.chars().skip(col).take(20).collect();
    assert_eq!(
        top_slice.chars().count(),
        20,
        "上边框应 20 列: {top_slice:?}"
    );
    assert!(
        top_slice.starts_with('╭') && top_slice.ends_with('╮'),
        "上边框两端应是角: {top_slice:?}"
    );
    let body_row = text
        .lines()
        .find(|l| l.contains("ascii row"))
        .expect("应有 ascii 行");
    let body: String = body_row.chars().skip(col).take(20).collect();
    assert_eq!(body.chars().count(), 20, "内容行应 20 列: {body:?}");
    assert!(
        body.starts_with('│') && body.ends_with('│'),
        "内容行两端应是竖线(与上边框对齐): {body:?}"
    );

    // Esc 关掉弹层(不残留)
    pump(&mut app, "<esc>").await?;
    assert!(!screen(&mut app).contains("T1"), "Esc 后弹层应关闭");
    Ok(())
}

/// `helix.el("box")`:框里装子节点(含 button),焦点提取与渲染都不受影响
#[tokio::test(flavor = "multi_thread")]
async fn el_box_renders_frame_and_children() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(
        r#"
    helix.register_command("box-ui", () => {
        helix.open_popup({
            width: 24, height: 7, position: "center",
            render: () =>
                helix.el("box", [
                    helix.el("text", "inner"),
                    helix.el("button", "ok", { id: "b1", onPress: () => helix.echo("pressed") }),
                ], { border: "double", title: "B", width: 16, height: 5 }),
            onKey: (k) => k.name === "Esc" ? "close" : "handled",
        });
    });
    "#,
    )
    .await?;

    pump(&mut app, ":box-ui<ret>").await?;
    let text = screen(&mut app);
    assert!(text.contains("╔ B "), "双线框 + 标题: {text}");
    assert!(text.contains("╚"), "下边框: {text}");
    assert!(text.contains("inner"), "子节点在框内: {text}");
    assert!(text.contains("ok"), "button 标签: {text}");

    // Tab 聚焦 button → Enter 触发 onPress(框不参与交互,但框**里的**节点照旧)
    pump(&mut app, "<tab>").await?;
    pump(&mut app, "<ret>").await?;
    let (status, _) = app.editor.get_status().unwrap();
    assert_eq!(status.as_ref(), "pressed", "框内的 button 仍可聚焦并触发");
    pump(&mut app, "<esc>").await?;
    Ok(())
}

/// 同一次 render 里"先画后返回旧行 API" ⇒ 仍按行渲染(绘制 op 不污染),这是既有契约
#[tokio::test(flavor = "multi_thread")]
async fn mixing_ops_and_lines_prefers_the_returned_value() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(
        r#"
    helix.register_command("mixed", () => {
        helix.open_popup({
            width: 24, height: 5, position: "center",
            render: () => {
                helix.draw.rect(0, 0, 10, 3, { border: "thick" });   // 画了但不 done()
                return ["lines-win"];
            },
            onKey: (k) => k.name === "Esc" ? "close" : "handled",
        });
    });
    "#,
    )
    .await?;

    pump(&mut app, ":mixed<ret>").await?;
    let text = screen(&mut app);
    assert!(text.contains("lines-win"), "{text}");
    // 粗边框字符只可能来自那条 op(编辑器自身的边框是细线)⇒ 出现即说明"画了没用 done"仍然生效了
    assert!(
        !text.contains('━') && !text.contains('┏'),
        "绘制 op 不该被渲染: {text}"
    );
    pump(&mut app, "<esc>").await?;
    Ok(())
}

/// 阶段二:braille 半像素 + 显式裁剪区,一路到真实 surface
#[tokio::test(flavor = "multi_thread")]
async fn braille_and_clip_reach_the_surface() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(
        r#"
    helix.register_command("chart", () => {
        helix.open_popup({
            width: 30, height: 8, position: "center",
            render: () => {
                // 裁剪区:只留前 3 列 ⇒ 后面的文字看不见(放在第 3 行,别被 braille 盖掉)
                helix.draw.push_clip(0, 2, 3, 1);
                  helix.draw.text(0, 2, "CLIPPED-AWAY");
                helix.draw.pop_clip();
                // braille:一条水平线(点 y=1,即第一格的第二行点)
                const pts = [];
                for (let dx = 0; dx < 8; dx++) pts.push([dx, 1]);
                helix.draw.braille(pts, { style: "ui.text" });
                return helix.draw.done();
            },
            onKey: (k) => k.name === "Esc" ? "close" : "handled",
        });
    });
    "#,
    )
    .await?;

    pump(&mut app, ":chart<ret>").await?;
    let text = screen(&mut app);
    // "CLIPPED-AWAY" 只留前三列 "CLI"
    assert!(text.contains("CLI"), "裁剪区内可见部分: {text}");
    assert!(!text.contains("CLIPPED-AWAY"), "裁剪区外应被裁掉: {text}");
    // 8 个点、每格 2 点(左右两列都亮)= U+2812 ⠒ ⇒ 一行 4 格
    assert!(text.contains("⠒⠒⠒⠒"), "braille 一行 4 格: {text}");
    pump(&mut app, "<esc>").await?;
    Ok(())
}
