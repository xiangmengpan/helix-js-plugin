use super::*;

use helix_term::application::Application;
use helix_term::job::Jobs;
use helix_view::current_ref;
use helix_view::input::parse_macro;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

async fn pump(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(keys)?.into_iter() {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    Ok(())
}

// P2: :vsplit/:hsplit 改道——无参分裂产生 LayoutTree 叶子(同 doc 双视图),
// 而非 core view-tree 内部节点;产物可被 window mode 完全控制、编辑互通。
#[tokio::test(flavor = "multi_thread")]
async fn vsplit_creates_controllable_same_doc_leaf() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "AAA\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    pump(&mut app, ":vsplit<ret>").await?;

    // 1) 产物是叶子:布局树 2 叶,含 BufferLeaf(不再是 editor 内部不可控的双 view)
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 2, ":vsplit 后应 2 叶: {types:?}");
    assert!(
        types.contains(&"BufferLeaf"),
        "新叶类型为 BufferLeaf: {types:?}"
    );

    // 2) 同 doc:文档数不增(仍是同一份 a.txt)
    let doc_count = app.editor.documents.len();
    assert_eq!(doc_count, 1, "同 doc 双视图不新增文档: {doc_count}");

    // 3) 新叶可编辑(window mode 聚焦右叶 → 输入 → 命中同一 doc)
    pump(&mut app, "<C-w>l<esc>").await?; // 模式内聚焦右侧新叶,退出模式
    pump(&mut app, "iXXX<esc>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "XXXAAA\n",
        "右叶输入应命中 a.txt doc: {:?}",
        doc.text().to_string()
    );

    // 4) 回左叶(leaf 0)再编辑 → 同一 doc 继续变(编辑互通)。
    // 注:helix 同 doc 多 view 语义——另一 view 插入后,本 view 光标沿 changeset
    // 映射到插入文本之后,故 YYY 落在 XXX 后。
    pump(&mut app, "<C-w>h<esc>").await?;
    pump(&mut app, "iYYY<esc>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "XXXYYYAAA\n",
        "左右叶编辑互通(同 doc,光标沿 changeset 映射): {:?}",
        doc.text().to_string()
    );

    // 5) 清理:模式内聚焦右叶(l)再 x 关闭 → 回 1 叶
    pump(&mut app, "<C-w>lx<esc>").await?;
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 1, "x 关闭 BufferLeaf 后回单叶: {types:?}");
    Ok(())
}

// :hsplit 同款(下侧新叶,同 doc)
#[tokio::test(flavor = "multi_thread")]
async fn hsplit_creates_same_doc_leaf_below() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "AAA\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    pump(&mut app, ":hsplit<ret>").await?;
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 2, ":hsplit 后应 2 叶: {types:?}");
    assert!(
        types.contains(&"BufferLeaf"),
        "新叶类型为 BufferLeaf: {types:?}"
    );
    assert_eq!(app.editor.documents.len(), 1, "同 doc 不新增文档");
    Ok(())
}

// :vsplit <path> → 新叶打开该文件,可编辑
#[tokio::test(flavor = "multi_thread")]
async fn vsplit_path_opens_file_in_new_leaf() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "AAA\n")?;
    std::fs::write(&b, "BBB\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    pump(&mut app, &format!(":vsplit {}<ret>", b.display())).await?;
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 2, "vsplit path 应 2 叶: {types:?}");
    assert_eq!(app.editor.documents.len(), 2, "a+b 两个 doc");
    // 新叶活动且显示 b → 输入命中 b
    pump(&mut app, "iXXX<esc>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "XXXBBB\n",
        "vsplit path 后输入命中新叶 doc: {:?}",
        doc.text().to_string()
    );
    Ok(())
}

// :vsplit-new → 新空 buffer 叶
#[tokio::test(flavor = "multi_thread")]
async fn vsplit_new_creates_scratch_leaf() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "AAA\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    pump(&mut app, ":vsplit-new<ret>").await?;
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 2, "vsplit-new 应 2 叶: {types:?}");
    assert_eq!(app.editor.documents.len(), 2, "scratch doc + a");
    // scratch 叶活动,空文档可输入
    pump(&mut app, "ix<esc>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "x\n",
        "scratch 叶输入(默认含尾换行): {:?}",
        doc.text().to_string()
    );
    assert!(doc.path().is_none(), "scratch 无路径");
    Ok(())
}
