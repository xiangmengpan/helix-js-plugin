//! bufferline 顺序相关命令:`:buffer-close-left/right [N]` 与 `:buffer-move-left/right [N]`。
//!
//! 语义(用户定义):
//! - 缺省 / `0` = 极端:`close` 关掉该侧**全部**,`move` 移到**最左/最右**;
//! - `N > 0` = `close` 只关**最近** N 个,`move` 朝该方向挪 N 格。
//!
//! 顺序只有一个来源:`Editor::documents`(IndexMap)的容器序 —— bufferline 渲染、
//! `:buffer-next/previous`、插件 `helix.buffer.list()` 读的都是它。

use std::path::PathBuf;

use super::*;
use helix_term::application::Application;

/// 建好 app 并按顺序打开 `b0.txt` … `b{n-1}.txt`,当前 buffer = 最后一个。
/// 返回的 `TempDir` 必须活到测试结束(文件要真存在)。
async fn app_with(n: usize) -> anyhow::Result<(Application, tempfile::TempDir, Vec<PathBuf>)> {
    let dir = tempfile::tempdir()?;
    let mut paths = Vec::new();
    for i in 0..n {
        let path = dir.path().join(format!("b{i}.txt"));
        std::fs::write(&path, format!("{i}\n"))?;
        paths.push(path);
    }
    let mut app = AppBuilder::new().build()?;
    for path in &paths {
        focus(&mut app, path).await?;
    }
    Ok((app, dir, paths))
}

/// `:open` 一个已打开的路径 = 只是把视图切过去(bufferline 顺序不变)。
async fn focus(app: &mut Application, path: &PathBuf) -> anyhow::Result<()> {
    pump(app, &format!(":open {}<ret>", path.display())).await
}

/// bufferline 的当前顺序(文件名)。
fn order(app: &Application) -> Vec<String> {
    app.editor
        .documents()
        .map(|doc| {
            doc.path()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "<scratch>".to_string())
        })
        .collect()
}

/// 当前 leaf 显示的文档。
fn current(app: &Application) -> String {
    let doc_id = app.editor.tree.get(app.editor.tree.focus).doc;
    app.editor
        .document(doc_id)
        .and_then(|d| d.path().and_then(|p| p.file_name()))
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<scratch>".to_string())
}

/// 缺省参数 = 关掉左边**全部**;当前 buffer 自己不动。
#[tokio::test(flavor = "multi_thread")]
async fn close_left_without_count_closes_everything_left() -> anyhow::Result<()> {
    let (mut app, _dir, _paths) = app_with(4).await?;
    assert_eq!(current(&app), "b3.txt");

    pump(&mut app, ":buffer-close-left<ret>").await?;
    assert_eq!(order(&app), vec!["b3.txt"]);
    assert_eq!(current(&app), "b3.txt", "当前 buffer 不参与关闭");
    Ok(())
}

/// `N` = 只关最近的 N 个(从当前往左数)。
#[tokio::test(flavor = "multi_thread")]
async fn close_left_with_count_closes_only_the_nearest_n() -> anyhow::Result<()> {
    let (mut app, _dir, _paths) = app_with(4).await?;

    pump(&mut app, ":buffer-close-left 2<ret>").await?;
    assert_eq!(order(&app), vec!["b0.txt", "b3.txt"]);
    Ok(())
}

/// 右侧对称:缺省全关,`N` 只关最近 N 个。
#[tokio::test(flavor = "multi_thread")]
async fn close_right_default_and_count() -> anyhow::Result<()> {
    let (mut app, _dir, paths) = app_with(4).await?;
    focus(&mut app, &paths[0]).await?;
    assert_eq!(current(&app), "b0.txt");

    pump(&mut app, ":buffer-close-right 1<ret>").await?;
    assert_eq!(order(&app), vec!["b0.txt", "b2.txt", "b3.txt"]);

    pump(&mut app, ":buffer-close-right<ret>").await?;
    assert_eq!(order(&app), vec!["b0.txt"]);
    Ok(())
}

/// 缺省 = 移到最左;`N` = 朝左挪 N 格。
#[tokio::test(flavor = "multi_thread")]
async fn move_left_default_and_count() -> anyhow::Result<()> {
    let (mut app, _dir, _paths) = app_with(4).await?;

    pump(&mut app, ":buffer-move-left<ret>").await?;
    assert_eq!(order(&app), vec!["b3.txt", "b0.txt", "b1.txt", "b2.txt"]);
    assert_eq!(current(&app), "b3.txt", "移动不影响当前文档");

    // 再往右挪回去 2 格 → 位置 2
    pump(&mut app, ":buffer-move-right 2<ret>").await?;
    assert_eq!(order(&app), vec!["b0.txt", "b1.txt", "b3.txt", "b2.txt"]);
    Ok(())
}

/// 缺省 = 移到最右;`N` = 朝右挪 N 格(越界 clamp 到末尾)。
#[tokio::test(flavor = "multi_thread")]
async fn move_right_default_and_count() -> anyhow::Result<()> {
    let (mut app, _dir, paths) = app_with(4).await?;
    focus(&mut app, &paths[0]).await?;

    pump(&mut app, ":buffer-move-right 2<ret>").await?;
    assert_eq!(order(&app), vec!["b1.txt", "b2.txt", "b0.txt", "b3.txt"]);

    pump(&mut app, ":buffer-move-right<ret>").await?;
    assert_eq!(order(&app), vec!["b1.txt", "b2.txt", "b3.txt", "b0.txt"]);

    // 已在最右:再按一次不变(也不报错)
    pump(&mut app, ":buffer-move-right<ret>").await?;
    assert_eq!(order(&app), vec!["b1.txt", "b2.txt", "b3.txt", "b0.txt"]);
    Ok(())
}

/// 关掉中间一个 buffer 后,其余位置**不重组**(`shift_remove` 而非 swap 删)。
#[tokio::test(flavor = "multi_thread")]
async fn closing_a_middle_buffer_keeps_the_rest_in_order() -> anyhow::Result<()> {
    let (mut app, _dir, paths) = app_with(4).await?;
    focus(&mut app, &paths[1]).await?;

    pump(&mut app, ":buffer-close<ret>").await?;
    assert_eq!(order(&app), vec!["b0.txt", "b2.txt", "b3.txt"]);
    Ok(())
}

/// 顺序是单一来源:`:buffer-next` 跟着重排后的顺序走。
#[tokio::test(flavor = "multi_thread")]
async fn buffer_next_follows_the_reordered_bufferline() -> anyhow::Result<()> {
    let (mut app, _dir, _paths) = app_with(3).await?;
    assert_eq!(current(&app), "b2.txt");

    pump(&mut app, ":buffer-move-left<ret>").await?;
    assert_eq!(order(&app), vec!["b2.txt", "b0.txt", "b1.txt"]);

    pump(&mut app, ":buffer-next<ret>").await?;
    assert_eq!(current(&app), "b0.txt", "b2 的下一个应是重排后的 b0");
    Ok(())
}

/// 脏 buffer 默认不关(与 `:buffer-close-others` 同款),`!` 才强关。
#[tokio::test(flavor = "multi_thread")]
async fn close_left_refuses_dirty_buffers_unless_forced() -> anyhow::Result<()> {
    let (mut app, _dir, paths) = app_with(4).await?;

    // b0 干净、b1 弄脏
    focus(&mut app, &paths[1]).await?;
    pump(&mut app, "iX<esc>").await?;
    focus(&mut app, &paths[3]).await?;

    pump(&mut app, ":buffer-close-left<ret>").await?;
    assert_eq!(
        order(&app),
        vec!["b1.txt", "b3.txt"],
        "干净的那侧照关,脏的留下"
    );
    assert!(
        status_text(&app).contains("unsaved"),
        "应提示未保存:{}",
        status_text(&app)
    );

    // 失败时焦点被移到第一个脏文档(`:buffer-close` 的既有语义),回到 b3 再强关
    focus(&mut app, &paths[3]).await?;
    pump(&mut app, ":buffer-close-left!<ret>").await?;
    assert_eq!(order(&app), vec!["b3.txt"]);
    Ok(())
}

/// 插件 API `helix.buffer.move(id, to)`:与 `:buffer-move-*` 共用同一份顺序
/// (`Editor.documents` 的容器序),越界 clamp 到末尾。
#[tokio::test(flavor = "multi_thread")]
async fn plugin_buffer_move_reorders_the_bufferline() -> anyhow::Result<()> {
    let (mut app, dir, _paths) = app_with(3).await?;
    assert_eq!(order(&app), vec!["b0.txt", "b1.txt", "b2.txt"]);

    let plugin = dir.path().join("bm.js");
    std::fs::write(
        &plugin,
        r#"
        helix.register_command("bm-first", () => {
            helix.buffer.move(helix.buffer.current(), 0);
        });
        helix.register_command("bm-far-right", () => {
            helix.buffer.move(helix.buffer.current(), 99);
        });
        "#,
    )?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin.display())).await?;

    // 当前 b2 → 最左
    pump(&mut app, ":bm-first<ret>").await?;
    assert_eq!(order(&app), vec!["b2.txt", "b0.txt", "b1.txt"]);

    // 越界 index 不报错,clamp 到末尾
    pump(&mut app, ":bm-far-right<ret>").await?;
    assert_eq!(order(&app), vec!["b0.txt", "b1.txt", "b2.txt"]);
    assert_eq!(current(&app), "b2.txt", "重排不动当前文档");
    Ok(())
}
