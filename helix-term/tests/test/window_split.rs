use super::*;

use helix_term::application::Application;
use helix_term::config::Config;
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
    pump(&mut app, "<C-p>l<esc>").await?; // 模式内聚焦右侧新叶,退出模式
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
    pump(&mut app, "<C-p>h<esc>").await?;
    pump(&mut app, "iYYY<esc>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "XXXYYYAAA\n",
        "左右叶编辑互通(同 doc,光标沿 changeset 映射): {:?}",
        doc.text().to_string()
    );

    // 5) 清理:模式内聚焦右叶(l)再 x 关闭 → 回 1 叶;doc 不被关闭(remove_empty_scratch
    //    经 tree.traverse 仍见 leaf0 的 view 显示同一 doc)
    pump(&mut app, "<C-p>lx<esc>").await?;
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 1, "x 关闭 BufferLeaf 后回单叶: {types:?}");
    assert_eq!(app.editor.documents.len(), 1, "关闭一叶不关文档");
    // 关闭后编辑仍命中该 doc(leaf0)
    pump(&mut app, "izzz<esc>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "XXXYYYzzzAAA\n",
        "关闭 BufferLeaf 后 leaf0 编辑照常: {:?}",
        doc.text().to_string()
    );
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

// 启动迁移:core 遗留的多 view(无叶)被收编为 BufferLeaf 叶,可被 window mode 控制
#[tokio::test(flavor = "multi_thread")]
async fn orphan_views_adopted_into_leaves() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;

    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "AAA\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    // 造一个"遗留"第二 view(直接 core tree.split,模拟旧模型/启动路径)
    {
        let e = &mut app.editor;
        let doc_id = e.tree.get(e.tree.focus).doc;
        let view = helix_view::view::View::new(doc_id, e.config().gutters.clone());
        e.tree.split(view, helix_view::tree::Layout::Vertical);
    }
    // 此时 2 个未认领 view,但只有 1 叶(旧模型:双 view 渲染在编辑器叶内)
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 1, "迁移前仍单叶(旧双 view 在叶内): {types:?}");
    // 收编
    app.compositor.adopt_orphan_views(&mut app.editor);
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 2, "迁移后 2 叶: {types:?}");
    assert!(
        types.contains(&"BufferLeaf"),
        "第二 view 进 BufferLeaf: {types:?}"
    );
    // 双叶均可编辑且同 doc 不重复
    assert_eq!(app.editor.documents.len(), 1, "同一 doc");
    Ok(())
}

// 压力:反复 分裂→聚焦→关闭→再分裂 (抓 tree.get 317 panic 复现)
#[tokio::test(flavor = "multi_thread")]
async fn split_close_stress_no_panic() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "l1\nl2\nl3\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    // 现实节奏:分裂→编辑→切窗→编辑→关闭,反复数次
    for i in 0..4 {
        pump(&mut app, ":hsplit<ret>").await?;
        pump(&mut app, "i<esc>").await?; // BufferLeaf 活动,进/退 insert 保状态干净
        pump(&mut app, "<C-p>k<esc>").await?; // 聚焦原叶
        pump(&mut app, "gg").await?;
        pump(&mut app, "<C-p>j<esc>x<esc>").await?; // 聚焦下叶并关闭
        let _ = i;
    }
    pump(&mut app, "gg<esc>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert!(!doc.text().to_string().is_empty());
    Ok(())
}

// 原生 bufferline(按文档数)探针:1/2 文档 + :hsplit 与 C-p r 分屏后顶行渲染差异
// 回归:空 scratch 分屏 → x 关闭 → scratch 文档随窗销毁(ghost buffer/bufferline 残留)
#[tokio::test(flavor = "multi_thread")]
async fn closing_scratch_leaf_drops_orphan_scratch_doc() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "AAA\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    // C-p n 新建空 scratch 分屏 → 2 docs
    pump(&mut app, "<C-p>n<esc>").await?;
    assert_eq!(app.editor.documents.len(), 2, "scratch + a.txt");
    // 关闭承载空 scratch 的叶(BufferLeaf 活动)→ scratch 文档应随之销毁(无其他 view 引用)
    pump(&mut app, "<C-p>x<esc>").await?;
    assert_eq!(
        app.editor.documents.len(),
        1,
        "空 scratch 失去最后 view 应销毁,不残留 ghost buffer"
    );
    // 有内容/有路径的文档不受影响:再开一个写内容的 scratch,关闭后保留
    pump(&mut app, "<C-p>n<esc>").await?;
    pump(&mut app, "idata<esc>").await?; // scratch 写内容 → modified
    assert_eq!(app.editor.documents.len(), 2);
    pump(&mut app, "<C-p>x<esc>").await?;
    assert_eq!(
        app.editor.documents.len(),
        2,
        "已修改的 scratch 关闭后文档保留(不丢数据)"
    );
    Ok(())
}

fn render_rows_probe(app: &mut Application) -> Vec<String> {
    use helix_view::graphics::Rect;
    let area = app.compositor.area();
    let mut buf = tui::buffer::Buffer::empty(area);
    let mut jobs = Jobs::new();
    let mut cx = helix_term::compositor::Context {
        editor: &mut app.editor,
        scroll: None,
        jobs: &mut jobs,
    };
    app.compositor.reset_plugin_diffs();
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

// bufferline 不得溢出编辑器叶到相邻叶(terminal/BufferLeaf)顶行
#[tokio::test(flavor = "multi_thread")]
async fn bufferline_clamped_to_editor_leaf() -> anyhow::Result<()> {
    let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "AAA\n")?;
    let b = dir.path().join("b.txt");
    std::fs::write(&b, "BBB\n")?;
    let mut cfg = helpers::test_config();
    cfg.editor.bufferline = helix_view::editor::BufferLine::Always;
    let mut app = AppBuilder::new()
        .with_config(cfg)
        .with_file(a, None)
        .build()?;
    // 多个文档让 bufferline 标签足够长(能溢出的宽度)
    for i in 0..10 {
        pump(&mut app, &format!(":new<ret>")).await?;
    }
    // H 分屏出右叶(BufferLeaf 承载后一个 scratch? :vsplit 同 doc 即可,验证 bufferline 不外溢)
    pump(&mut app, ":vsplit<ret>").await?;
    // 编辑器叶左侧窄条顶部 bufferline;右叶顶行必须是自己内容(边框/行号),不能是标签文字尾巴
    let area = app.compositor.area();
    let mut buf = tui::buffer::Buffer::empty(area);
    let mut jobs = Jobs::new();
    let mut cx = helix_term::compositor::Context {
        editor: &mut app.editor,
        scroll: None,
        jobs: &mut jobs,
    };
    app.compositor.render(area, &mut buf, &mut cx);
    // 找编辑器的右侧边界:布局 50/50
    let half = area.width / 2;
    // 右半区顶行(行0~1):若出现 "scratch"/"a.txt" 等标签文字 → 溢出
    let w = area.width as usize;
    let right_top: String = (0..1usize)
        .flat_map(|y| {
            buf.content[y * w + half as usize..y * w + w]
                .iter()
                .map(|c| c.symbol.as_str())
        })
        .collect();
    // 右叶应显示自己的边框/内容;文本行(如 gutter "1")允许,但不能是 bufferline 文件名字样
    // 简化断言:整个右半不含 "scratch" 标签
    assert!(
        !right_top.contains("scratch"),
        "bufferline 不应溢出到右叶顶行: {right_top:?}"
    );
    let _ = b;
    Ok(())
}
