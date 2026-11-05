//! 编辑器级请求(主题、键位映射)。
//!
//! 从 `typed.rs` 的 `apply_ui_requests` 拆出(G7):每个 arm 变成一个 handler,
//! 顶层只留分派。请求结构体的字段解构写在 handler 签名里,handler 体内是原 arm 的原文。

use super::*;

pub(crate) fn handle_set_theme(SetThemeReq { name }: SetThemeReq) -> anyhow::Result<()> {
    // 复用 :theme Validate 逻辑：加载 + set_theme + 重捕获基准 + 清插件覆盖
    job::dispatch_blocking(move |editor, _compositor, mut _js| {
        let true_color = editor.config.load().true_color || crate::true_color();
        let theme = match editor.theme_loader.load(&name) {
            Ok(t) => t,
            Err(e) => {
                editor.set_error(format!("set_theme_name: 加载主题 '{name}' 失败: {e}"));
                return;
            }
        };
        if !(true_color || theme.is_16_color()) {
            editor.set_error(format!(
                "set_theme_name: 主题 '{name}' 需要 true color 支持"
            ));
            return;
        }
        if let Err(e) = editor.set_theme(theme) {
            editor.set_error(format!("set_theme_name: {e}"));
            return;
        }
        // 清插件覆盖（切基准后旧覆盖不再适用）
        helix_js::reset_theme();
        sync_theme_snapshot(editor);
        emit_plugin_event(editor, "theme-change", None);
    });
    Ok(())
}

pub(crate) fn handle_map_key(
    MapKeyReq {
        mode,
        key,
        command,
        unbind,
        owner,
    }: MapKeyReq,
) -> anyhow::Result<()> {
    // 键序列解析在 job 之前（可失败 → 调用方 set_error），闭包只捕获 owned 数据。
    let (mode, keys, cmd) = parse_plugin_binding(&mode, &key, &command)?;
    // 覆盖检测在 **job 之外**做不了（要读活的 Keymaps）⇒ 挪进闭包；提示只发一次。
    let key_label = keys
        .iter()
        .map(|k| k.key_sequence_format())
        .collect::<Vec<_>>()
        .join("");
    job::dispatch_blocking(move |editor, compositor, mut _js| {
        let Some(view) = compositor.find::<ui::EditorView>() else {
            return;
        };
        if unbind {
            // 解绑(dashboard 关屏时用)—— 真正的"摘叶",不是绑到 no_op ✗
            view.keymaps.remove_binding(mode, &keys);
            return;
        }
        // **抢键可见**（D 阶段）：绑之前先看这个序列现在是谁的。
        // 静默覆盖是"插件显得不融洽"的主要来源之一 —— 用户只看到自己的键莫名失效。
        if let Some(owner) = owner.as_deref() {
            if let Some(what) = view.keymaps.describe_override(mode, &keys, &cmd) {
                let id = format!("{mode:?}:{key_label}:{owner}:{what}");
                if crate::keymap::note_new_override(&id) {
                    let msg = format!(
                        "插件 `{owner}` 覆盖了 `{key_label}`（{what}；`:keys` 看全部，\
                         改回写 [keys.{}]）",
                        mode_str(mode)
                    );
                    log::warn!("{msg}");
                    editor.set_warning(msg);
                }
            }
        }
        view.keymaps
            .insert_binding_owned(mode, &keys, cmd, owner.as_deref());
    });
    Ok(())
}

/// `Mode` → `[keys.<mode>]` 的段名（提示里告诉用户去哪儿改）
fn mode_str(mode: helix_view::document::Mode) -> &'static str {
    use helix_view::document::Mode;
    match mode {
        Mode::Normal => "normal",
        Mode::Insert => "insert",
        Mode::Select => "select",
    }
}
