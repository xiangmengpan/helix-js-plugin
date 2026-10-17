//! 文档/缓冲区/退出类请求(5 个 arm)。
//!
//! 从 `typed.rs` 的 `apply_ui_requests` 拆出(G7):每个 arm 变成一个 handler,
//! 顶层只留分派。请求结构体的字段解构写在 handler 签名里,handler 体内是原 arm 的原文。

use super::*;

pub(crate) fn handle_quit() -> anyhow::Result<()> {
    // ⚠️ 2026-10-10 事故:此处曾用 `editor.close(editor.tree.focus)` ✗
    // 结果在**关闭最后一个视图**时崩溃:
    //   helix-view/src/tree.rs:323 `try_get(index).unwrap()` on None ✓
    // 根因:手动关视图**绕开了 helix 自己维护焦点的逻辑** ⇒ 视图 id 悬空 ✓
    // ⇒ 该接口**暂停**(只报告,不动作)✓:任何调用者都不可能再崩 ✓
    // 正确做法(待做):找到 `:quit` 的 handler(`TypableCommand{ name:"quit", fun: quit }` ✓)
    // 并**通过命令路径**执行它,而不是自己关视图 ✓
    // ✅ 照抄同文件的 `fn quit`(typed.rs:244)的**完整流程** ✓
    // 我上一版只做了最后一步 `close` ✗ ⇒ 关掉最后一个视图后应用没退出
    // ⇒ 焦点悬空 ⇒ tree.rs:323 `try_get().unwrap()` panic ✓
    job::dispatch_blocking(move |editor, _compositor| {
        // ① 最后一个视图且**有未保存** ⇒ 先提示,不硬关 ✓(与 :quit 同 ✓)
        if editor.tree.views().count() == 1 {
            if let Err(e) = buffers_remaining_impl(editor) {
                editor.set_error(format!("quit: {e}"));
                return;
            }
        }
        // ② **不**冲刷待写 —— 取舍说明 ✓:
        //    `:quit` 用的是 `Context::block_try_flush_writes`(同步 ✓),
        //    而本臂只有 `Editor`,其 `flush_writes` 是 **async** ✗ ⇒ 同步闭包里等不了 ✓
        //    不做这一步的后果:待写内容留在队列(不会静默丢盘 ✗ ——
        //    上一步已对"未保存"做过提示/拦截 ✓)⇒ 可接受 ✓
        // ③ 通知插件(与 :quit 一致 ✓)
        emit_plugin_event(editor, "buffer-close", None);
        // ④ 关当前视图 ⇒ 关掉最后一个即退出 ✓(此时不会再有悬空查找 ✓)
        let view_id = view!(editor).id;
        editor.close(view_id);
    });
    Ok(())
}

pub(crate) fn handle_open_scratch_file(
    OpenScratchFileReq { path }: OpenScratchFileReq,
) -> anyhow::Result<()> {
    job::dispatch_blocking(move |editor, _compositor| {
        let p = PathBuf::from(&path);
        match editor.open(&p, Action::Replace) {
            Err(err) => editor.set_error(format!("open_scratch: {err}")),
            Ok(_) => {
                // **安全前提**:不绑定路径 → 用户 `:w` 不会覆盖原始文件
                // (如 runtime/tutor)。Rust 版 :tutor 也是这么做的。
                doc_mut!(editor).set_path(None);
            }
        }
    });
    Ok(())
}

pub(crate) fn handle_open_file(OpenFileReq { path, row, col }: OpenFileReq) -> anyhow::Result<()> {
    // open 是同步的（editor.open 直接返回），:open 语义（Action::Replace）；
    // 错误经 set_error 上报（闭包内无 Result 传播路径）；
    // 带行列 → 打开后定位光标（clamp 复用 pos_to_char，JS 输入不可信）
    let path = PathBuf::from(path);
    job::dispatch_blocking(move |editor, _compositor| {
        if let Err(err) = editor.open(&path, Action::Replace) {
            editor.set_error(format!("open_file: {err}"));
        } else if let (Some(row), Some(col)) = (row, col) {
            use helix_core::Selection;
            let (view, doc) = current!(editor);
            let pos = Selection::point(pos_to_char(doc.text(), row as usize, col as usize));
            doc.set_selection(view.id, pos);
            align_view(doc, view, Align::Center);
        }
    });
    Ok(())
}

pub(crate) fn handle_focus_buffer(FocusBufferReq { id }: FocusBufferReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |editor, _compositor| {
        if let Some((doc_id, _)) = editor.documents.iter().find(|(did, _)| did.as_u64() == id) {
            editor.switch(*doc_id, helix_view::editor::Action::Replace);
        } else {
            editor.set_error(format!("focus_buffer: no buffer with id {id}"));
        }
    });
    Ok(())
}

pub(crate) fn handle_open_buffer_leaf(
    OpenBufferLeafReq { path, split }: OpenBufferLeafReq,
) -> anyhow::Result<()> {
    job::dispatch_blocking(move |editor, compositor| {
        let dir = match split.as_deref() {
            Some("h") => crate::ui::layout::SplitDir::H,
            _ => crate::ui::layout::SplitDir::V,
        };
        compositor.open_doc_in_new_leaf(editor, PathBuf::from(&path), dir, false);
    });
    Ok(())
}
