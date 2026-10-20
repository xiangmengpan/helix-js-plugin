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
                helix.draw.pop();
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

/// 阶段三:滚动容器 —— 内容按内容坐标画一次,滚动只改 dy,容器外自动裁掉
#[tokio::test(flavor = "multi_thread")]
async fn scroll_container_reaches_the_surface() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(
        r#"
    let names = ["alpha", "bravo", "charlie", "delta", "echo"];
    let top = 0;
    helix.register_command("list-ui", () => {
        helix.open_popup({
            width: 24, height: 7, position: "center",
            render: () => {
                helix.draw.rect(0, 0, 16, 4, { border: "plain" });
                helix.draw.push_scroll(1, 1, 14, 2, { dy: top });
                  names.forEach((n, i) => helix.draw.text(1, i + 1, i + " " + n));
                helix.draw.pop();
                return helix.draw.done();
            },
            // 滚动走**键位**(弹层是模态的,`:命令` 会被它吞掉 —— 这正是真实用法)
            onKey: (k) => {
                if (k.name === "Down") top = Math.min(top + 1, names.length - 2);
                else if (k.name === "Up") top = Math.max(top - 1, 0);
                else if (k.name === "Esc") return "close";
                return "handled";
            },
        });
    });
    "#,
    )
    .await?;

    pump(&mut app, ":list-ui<ret>").await?;
    let text = screen(&mut app);
    assert!(text.contains("0 alpha"), "未滚动时应看到第 0 行: {text}");
    assert!(text.contains("1 bravo"), "未滚动时应看到第 1 行: {text}");
    assert!(
        !text.contains("2 charlie"),
        "容器只 2 行高,第 3 行应在框下看不到: {text}"
    );

    // 按两次 Down ⇒ dy=2:同一条 render 里的内容 op **一字未改**,只有 dy 变了
    pump(&mut app, "<down><down>").await?;
    let text = screen(&mut app);
    assert!(text.contains("2 charlie"), "滚动后第 2 行顶上来: {text}");
    assert!(text.contains("3 delta"), "滚动后第 3 行: {text}");
    assert!(!text.contains("0 alpha"), "第 0 行已被滚出容器: {text}");
    pump(&mut app, "<esc>").await?;
    Ok(())
}

/// **真实插件样板 ②**:`picker-ui` 的 render 全部改用 `helix.draw.*`
/// (外框 rect / 查询行 text / 分隔 hline / 选中行 fill / 列表 push_scroll / 滚动条 vline)
#[tokio::test(flavor = "multi_thread")]
async fn picker_ui_draws_with_draw_api() -> anyhow::Result<()> {
    // 列表内容**自己造**:否则依赖跑测试时的 cwd,会被别处的 `:cd` 干扰(踩过)
    let dir = tempfile::tempdir()?;
    for i in 0..30 {
        std::fs::write(dir.path().join(format!("f{i:02}.txt")), "x\n")?;
    }
    let _cwd = CwdScope::enter(dir.path()).await;

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let plugin = root.join("plugins/picker-ui/plugin.js");
    assert!(plugin.is_file(), "仓库里应有 {}", plugin.display());

    let mut app = AppBuilder::new().build()?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;
    pump(&mut app, ":pick<ret>").await?;

    let text = screen(&mut app);
    assert!(text.contains("╭ picker · files"), "圆角外框 + 标题: {text}");
    assert!(text.contains("> _"), "查询行(带光标): {text}");
    assert!(text.contains('▸'), "选中行标记: {text}");
    assert!(
        text.contains("Enter") && text.contains("Esc"),
        "底部状态行: {text}"
    );
    assert!(
        text.contains('█'),
        "30 个文件装不下 ⇒ 应有滚动条当前位置: {text}"
    );

    // 按 Down 若干次:选中行仍在框内(滚动窗口跟随),但画面已经变了
    for _ in 0..12 {
        pump(&mut app, "<down>").await?;
    }
    let text2 = screen(&mut app);
    assert!(text2.contains('▸'), "滚动后选中行仍可见: {text2}");
    assert_ne!(text, text2, "按 Down 之后画面应变化(选中+滚动窗口)");
    // 滚到中途:第一行不再是 f00(内容坐标 + offset 真的生效了)
    assert!(
        !text2.contains("f00.txt"),
        "多次 Down 后 f00 应已滚出窗口: {text2}"
    );

    pump(&mut app, "<esc>").await?;
    assert!(!screen(&mut app).contains("╭ picker"), "Esc 关闭弹层");
    Ok(())
}

/// **真实插件样板 ②**:`filetree` 的面板骨架（头 + 分隔线 + 滚动树）全部用 `helix.draw.*`
#[tokio::test(flavor = "multi_thread")]
async fn filetree_draws_panel_skeleton() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "x\n")?;
    std::fs::create_dir_all(dir.path().join("sub"))?;
    std::fs::write(dir.path().join("sub/b.txt"), "y\n")?;
    let root_name = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let _cwd = CwdScope::enter(dir.path()).await;
    let mut app = AppBuilder::new().with_file(&a, None).build()?;
    pump(
        &mut app,
        &format!(
            ":plugin-load {}<ret>",
            repo_plugin("filetree/plugin.js").display()
        ),
    )
    .await?;
    pump(&mut app, &format!(":cd {}<ret>", dir.path().display())).await?;
    pump(&mut app, ":filetree<ret>").await?;

    let text = screen(&mut app);
    assert!(text.contains(&root_name), "头行应显示根目录名: {text}");
    // 开关状态标记(默认跟随开)
    assert!(text.contains('F'), "头行应有跟随标记: {text}");
    // 树内容(目录在前)
    assert!(text.contains("sub"), "应列出子目录: {text}");
    assert!(text.contains("a.txt"), "应列出文件: {text}");

    // 打开树里的文件 ⇒ buffer-open ⇒ 当前文件标记(右缘圆点)
    pump(&mut app, &format!(":open {}<ret>", a.display())).await?;
    let text2 = screen(&mut app);
    assert!(text2.contains('●'), "当前文件应有圆点标记: {text2}");

    pump(&mut app, ":filetree<ret>").await?;
    let text3 = screen(&mut app);
    assert!(
        !text3.contains("a.txt") || !text3.contains(&root_name),
        "再按一次应关面板: {text3}"
    );
    Ok(())
}
