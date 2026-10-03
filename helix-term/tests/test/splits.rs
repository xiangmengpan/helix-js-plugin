use super::*;
use helix_view::current_ref;

use helix_stdx::path;
use helix_term::application::Application;
use helix_term::job::Jobs;
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

#[tokio::test(flavor = "multi_thread")]
async fn test_split_write_quit_all() -> anyhow::Result<()> {
    let mut file1 = tempfile::NamedTempFile::new()?;
    let mut file2 = tempfile::NamedTempFile::new()?;
    let mut file3 = tempfile::NamedTempFile::new()?;

    let mut app = helpers::AppBuilder::new()
        .with_file(file1.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            (
                Some(&format!(
                    "ihello1<esc>:sp<ret>:o {}<ret>ihello2<esc>:sp<ret>:o {}<ret>ihello3<esc>",
                    file2.path().to_string_lossy(),
                    file3.path().to_string_lossy()
                )),
                Some(&|app| {
                    let docs: Vec<_> = app.editor.documents().collect();
                    assert_eq!(3, docs.len());

                    let doc1 = docs
                        .iter()
                        .find(|doc| doc.path().unwrap() == &path::normalize(file1.path()))
                        .unwrap();

                    assert_eq!("hello1", doc1.text().to_string());

                    let doc2 = docs
                        .iter()
                        .find(|doc| doc.path().unwrap() == &path::normalize(file2.path()))
                        .unwrap();

                    assert_eq!("hello2", doc2.text().to_string());

                    let doc3 = docs
                        .iter()
                        .find(|doc| doc.path().unwrap() == &path::normalize(file3.path()))
                        .unwrap();

                    assert_eq!("hello3", doc3.text().to_string());

                    helpers::assert_status_not_error(&app.editor);
                    assert_eq!(3, app.editor.tree.views().count());
                }),
            ),
            (
                Some(":wqa<ret>"),
                Some(&|app| {
                    helpers::assert_status_not_error(&app.editor);
                    assert_eq!(0, app.editor.tree.views().count());
                }),
            ),
        ],
        true,
    )
    .await?;

    helpers::assert_file_has_content(&mut file1, &LineFeedHandling::Native.apply("hello1"))?;
    helpers::assert_file_has_content(&mut file2, &LineFeedHandling::Native.apply("hello2"))?;
    helpers::assert_file_has_content(&mut file3, &LineFeedHandling::Native.apply("hello3"))?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_split_write_quit_same_file() -> anyhow::Result<()> {
    let mut file = tempfile::NamedTempFile::new()?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            (
                Some("O<esc>ihello<esc>:sp<ret>ogoodbye<esc>"),
                Some(&|app| {
                    assert_eq!(2, app.editor.tree.views().count());
                    helpers::assert_status_not_error(&app.editor);

                    let mut docs: Vec<_> = app.editor.documents().collect();
                    assert_eq!(1, docs.len());

                    let doc = docs.pop().unwrap();

                    assert_eq!(
                        LineFeedHandling::Native.apply("hello\ngoodbye"),
                        doc.text().to_string()
                    );

                    assert!(doc.is_modified());
                }),
            ),
            (
                Some(":wq<ret>"),
                Some(&|app| {
                    helpers::assert_status_not_error(&app.editor);
                    assert_eq!(1, app.editor.tree.views().count());

                    let mut docs: Vec<_> = app.editor.documents().collect();
                    assert_eq!(1, docs.len());

                    let doc = docs.pop().unwrap();

                    assert_eq!(
                        LineFeedHandling::Native.apply("hello\ngoodbye"),
                        doc.text().to_string()
                    );

                    assert!(!doc.is_modified());
                }),
            ),
        ],
        false,
    )
    .await?;

    helpers::assert_file_has_content(&mut file, &LineFeedHandling::Native.apply("hello\ngoodbye"))?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_changes_in_splits_apply_to_all_views() -> anyhow::Result<()> {
    // See <https://github.com/helix-editor/helix/issues/4732>.
    // Transactions must be applied to any view that has the changed document open.
    // This sequence would panic since the jumplist entry would be modified in one
    // window but not the other. Attempting to update the changelist in the other
    // window would cause a panic since it would point outside of the document.

    // The key sequence here:
    // * :vsplit<ret> Create a vertical split of the current buffer.
    //                Both views look at the same doc.
    // * [<space>     Add a line ending to the beginning of the document.
    //                The cursor is now at line 2 in window 2.
    // * <C-s>        Save that selection to the jumplist in window 2.
    // * <space>ww    Switch to window 1 (rotate_view).
    // * kd           Delete line 1 in window 1.
    // * <space>wq    Close window 1, focusing window 2 (wclose).
    // * d            Delete line 1 in window 2.
    //
    // This panicked in the past because the jumplist entry on line 2 of window 2
    // was not updated and after the `kd` step, pointed outside of the document.
    //
    // Note: 叶=窗口后 :vsplit 产生 BufferLeaf 叶,窗口切换走 window mode(C-p h/l + Esc),
    // 关闭走模式内 x;不再用 <space>ww/wq(那是 core view-tree 的窗口操作,与叶模型脱钩)。
    // 叶模型等价:V1=原编辑器叶,V2=BufferLeaf 同 doc。切换走 window mode(C-p h/l + Esc)。
    // 断言:无 panic(核心 view 同步不变式),doc 删空,状态无 error。
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "l1\nl2\nl3\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    // pump 式驱动(同 window_split;不自动 :q!,app Drop 关闭)
    pump(&mut app, ":vsplit<ret>").await?;
    {
        assert_eq!(app.editor.documents.len(), 1, "同 doc 双视图");
        let types = app.compositor.layout_tree().leaf_types();
        assert_eq!(types.len(), 2, "应 2 叶: {types:?}");
    }
    // V2(右,BufferLeaf,刚分裂后活动)光标到末行 → 切 V1 → 删光所有行 → 回 V2(光标越界需同步,核心不 panic)
    pump(&mut app, "G<C-p>h<esc>%d<C-p>l<esc>").await?;
    helpers::assert_status_not_error(&app.editor);
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "",
        "V1 删除应作用于共享 doc(且 V2 越界光标同步不 panic)"
    );

    // Transactions are applied to the views for windows lazily when they are focused.
    // This case panics if the transactions and inversions are not applied in the
    // correct order as we switch between windows.
    test((
        "#[|]#",
        "[<space>[<space>[<space>:vsplit<ret>uuu<space>wwUUU<space>wquuu",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    // See <https://github.com/helix-editor/helix/issues/4957>.
    // This sequence undoes part of the history and then adds new changes, creating a
    // new branch in the history tree. `View::sync_changes` applies transactions down
    // and up to the lowest common ancestor in the path between old and new revision
    // numbers. If we apply these up/down transactions in the wrong order, this case
    // panics.
    // The key sequence:
    // * 3[<space>    Create three empty lines so we are at the end of the document.
    // * :vsplit<ret><C-s>  Create a split and save that point at the end of the
    //                document in the jumplist.
    // * <space>ww    Switch back to the first window.
    // * uu           Undo twice (not three times which would bring us back to the
    //                root of the tree).
    // * 3[<space>    Create three empty lines. Now the end of the document is past
    //                where it was on step 1.
    // * <space>wq    Close window 1, focusing window 2 and causing a sync. This step
    //                panics if we don't apply in the right order.
    // * %d           Clean up the buffer.
    test((
        "#[|]#",
        "3[<space>:vsplit<ret><C-s><space>wwuu3[<space><space>wq%d",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_changes_in_splits_jumplist_sync() -> anyhow::Result<()> {
    // See <https://github.com/helix-editor/helix/issues/9833>
    // 切换窗口时,当前 view 须与另一 view 对同一 doc 的改动同步(否则形成越界 selection panic)。
    // 叶模型等价场景:两叶同 doc;view2 记录越界位置 → view1 删行使 doc 变短 → 切回 view2
    // (sync 触发)→ 编辑不 panic。旧 <C-p>d/gf/w/q 和弦已被 window mode 语义取代,
    // 由本场景与 apply_to_all/reload 两案共同守护 sync 路径。
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "l1\nl2\nl3\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    pump(&mut app, ":vsplit<ret>").await?; // 2 叶同 doc
                                           // view1 行下插 NEW,切 view2 再插 YYY:落后 view 聚焦时须先同步;切换/同步不 panic
    pump(&mut app, "<C-p>h<esc>oNEW<esc><C-p>l<esc>oYYY<esc>").await?;
    helpers::assert_status_not_error(&app.editor);
    let (_, doc) = current_ref!(app.editor);
    let text = doc.text().to_string();
    assert!(text.contains("NEW"), "view1 插入生效: {text:?}");
    assert!(text.contains("YYY"), "view2 插入生效: {text:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reload_all_with_split_jumplist() -> anyhow::Result<()> {
    // See reproduction from <https://github.com/helix-editor/helix/issues/9830>
    //
    // The key sequence:
    // * :hsplit<ret> Horizontal split: two views on the same document.
    // * ]<space> Add an empty line below, growing the document.
    // * %        Select the whole document.
    // * 2G       Go to line 2. `goto_line` calls `push_jump`, recording a jump
    //            whose selection is valid at the *current* (grown) revision.
    // * ms/      Surround-add `/`, growing the document again.
    // * :rla     reload-all: re-reads the file from disk (shrinking the buffer
    //            back to its original contents) but only syncs the first view of
    //            each document, leaving the other split's `doc_revisions` stale.
    // * %J       Select-all and join, forcing a sync of the stale view.
    //
    // On the unfixed code the jumplist entry recorded by `2G` is left ahead of
    // the stale view's `doc_revisions`; once that view is synced, the entry is
    // mapped through a changeset whose pre-image predates it and
    // `ChangeSet::update_positions` panics.
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new()?;
    // `:reload-all` re-reads from disk, so the file must have on-disk contents
    // for the reload to shrink the (grown) buffer back down.
    file.write_all(b"line1\nline2\nline3\n")?;
    file.flush()?;

    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;

    // 叶模型:窗口切换/关闭走 window mode;末段 C-p x 关闭 BufferLeaf,
    // 等价原 wq——同样触发被关 view 的 doc_revisions 同步(核心不 panic)。
    pump(&mut app, ":hsplit<ret>]<space>%2Gms/:rla<ret>%J").await?;
    helpers::assert_status_not_error(&app.editor);
    // 关闭 BufferLeaf(活动叶=下侧新叶):触发被关 view 的 doc_revisions 同步
    pump(&mut app, "<C-p>x<esc>").await?;
    helpers::assert_status_not_error(&app.editor);
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(
        doc.text().to_string(),
        "line1 line2 line3\n",
        "%J 作用于共享 doc(join 生效,同步未 panic): {:?}",
        doc.text().to_string()
    );

    Ok(())
}
