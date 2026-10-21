//! 终端请求(7 个 arm:打开/模式/关闭/清屏/保存/尺寸/喂数据)。
//!
//! 从 `typed.rs` 的 `apply_ui_requests` 拆出(G7):每个 arm 变成一个 handler,
//! 顶层只留分派。请求结构体的字段解构写在 handler 签名里,handler 体内是原 arm 的原文。

use super::*;

pub(crate) fn handle_open_terminal(
    js: &mut Option<&mut helix_js::host::JsHost>,
    OpenTerminalReq {
        view_id,
        pty_id,
        cmd,
        side,
        size,
    }: OpenTerminalReq,
) -> anyhow::Result<()> {
    // side 已在 JS 侧白名单校验，此处仅防御性映射（与 OpenPanel 同款）；
    // cmd 由 JS 侧 spawn 完成，这里只用于错误提示不真正执行
    match side.as_str() {
        "right" | "left" | "bottom" => {}
        other => bail!("open_terminal: unknown side '{other}'"),
    }
    if cmd.is_empty() {
        bail!("open_terminal: empty cmd");
    }
    let side_enum = match side.as_str() {
        "right" => crate::ui::plugin_panel::PanelSide::Right,
        "left" => crate::ui::plugin_panel::PanelSide::Left,
        _ => crate::ui::plugin_panel::PanelSide::Bottom,
    };
    let terminal = PluginTerminal::new(view_id, pty_id, size);
    // D-2:term-open 是唯一**从 UI 请求臂**发出的事件 ⇒ 有宿主时走泵入口
    match js.as_mut() {
        Some(host) => host.emit_term_open(pty_id, &cmd),
        None => helix_js::emit_term_open(pty_id, &cmd),
    }
    job::dispatch_blocking(move |_editor, compositor, mut _js| {
        // 布局树：终端成为叶子
        use crate::ui::layout::SplitDir;
        let (dir, first) = match side_enum {
            crate::ui::plugin_panel::PanelSide::Right => (SplitDir::H, false),
            crate::ui::plugin_panel::PanelSide::Left => (SplitDir::H, true),
            crate::ui::plugin_panel::PanelSide::Bottom => (SplitDir::V, false),
        };
        let _ = compositor.split_leaf_with_ratio(dir, first, size, Box::new(terminal));
    });
    Ok(())
}

pub(crate) fn handle_term_mode(TermModeReq { view_id, mode }: TermModeReq) -> anyhow::Result<()> {
    let Some(tm) = crate::ui::plugin_terminal::TermMode::from_str(&mode) else {
        bail!("set_terminal_mode: unknown mode '{mode}'");
    };
    job::dispatch_blocking(move |_editor, compositor, mut _js| {
        if let Some(term) = compositor
            .find_where::<crate::ui::plugin_terminal::PluginTerminal>(|t| t.view_id() == view_id)
        {
            term.set_mode(tm);
            // Floating：叶子浮动到视口中央浮窗（最上层）；其他模式取消浮动
            let leaf_id = compositor
                .layout_tree()
                .find_leaf_id::<crate::ui::plugin_terminal::PluginTerminal>(|t| {
                    t.view_id() == view_id
                });
            match (tm, leaf_id) {
                (crate::ui::plugin_terminal::TermMode::Floating, Some(id)) => {
                    compositor.set_float(id);
                }
                _ => compositor.unfloat(),
            }
        }
    });
    Ok(())
}

pub(crate) fn handle_term_close(TermCloseReq { view_id }: TermCloseReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor, mut _js| {
        if let Some(id) = compositor
            .layout_tree()
            .find_leaf_id::<crate::ui::plugin_terminal::PluginTerminal>(|t| t.view_id() == view_id)
        {
            compositor.remove_leaf(id);
            // 布局变更 ⇒ 同步 JS 快照并触发 layout-change(#47)。
            // 这一臂此前**漏了** sync ⇒ 关终端时 JS 侧快照会陈旧、layout-change 也不发 ✗
            // （与另 9 个臂对齐；关闭终端同样改变了布局）
            compositor.set_layout_change_kind("close");
            compositor.sync_layout_cache(_js.as_deref_mut());
        }
    });
    Ok(())
}

pub(crate) fn handle_term_clear(TermClearReq { view_id }: TermClearReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor, mut _js| {
        if let Some(term) = compositor
            .find_where::<crate::ui::plugin_terminal::PluginTerminal>(|t| t.view_id() == view_id)
        {
            term.clear();
        }
    });
    Ok(())
}

pub(crate) fn handle_term_save(TermSaveReq { view_id, path }: TermSaveReq) -> anyhow::Result<()> {
    job::dispatch_blocking(move |editor, compositor, _js| {
        let Some(term) = compositor
            .find_where::<crate::ui::plugin_terminal::PluginTerminal>(|t| t.view_id() == view_id)
        else {
            editor.set_error(format!("term_save: no terminal with view_id {view_id}"));
            return;
        };
        let path = if path.is_empty() {
            format!(
                "{}/.cache/helix/term-{view_id}.log",
                std::env::var("HOME").unwrap_or_default()
            )
        } else {
            path
        };
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&path, term.full_text()) {
            Ok(()) => editor.set_status(format!("终端已保存到 {path}")),
            Err(e) => editor.set_error(format!("term_save: {e}")),
        }
    });
    Ok(())
}

pub(crate) fn handle_term_resize(
    TermResizeReq { view_id, size }: TermResizeReq,
) -> anyhow::Result<()> {
    job::dispatch_blocking(move |_editor, compositor, mut _js| {
        if let Some(term) = compositor
            .find_where::<crate::ui::plugin_terminal::PluginTerminal>(|t| t.view_id() == view_id)
        {
            term.set_size(size);
        }
    });
    Ok(())
}

pub(crate) fn handle_term_feed(TermFeedReq { view_id, chunk }: TermFeedReq) -> anyhow::Result<()> {
    // 按 view_id 找对应终端层喂进网格；层不存在（feed 早于层 push 的竞态）→ 丢弃。
    // find_where 按实例的 view_id 字段匹配，不在热路径上每次 leak 一个 id 字符串。
    // ⚠️ D-3(第一类:worker/通道) —— 这里是 `job::dispatch_blocking` 的
    // **Send 闭包**(喂网格发生在 job 里,可能不在主线程)⇒ `&mut JsHost`(!Send)
    // **不能**跨进来。所以 `term-title`(在 `feed` 里发)永久走 legacy 隐式解析:
    // 这不是"还没做",是这条边界的存在理由。
    job::dispatch_blocking(move |_editor, compositor, mut _js| {
        if let Some(terminal) = compositor.find_where::<PluginTerminal>(|t| t.view_id() == view_id)
        {
            terminal.feed(&chunk);
        }
    });
    Ok(())
}
