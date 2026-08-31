#[cfg(feature = "integration")]
mod test {
    mod helpers;

    use helix_core::{syntax::config::AutoPairConfig, Selection};
    use helix_term::config::Config;

    use indoc::indoc;

    use self::helpers::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn hello_world() -> anyhow::Result<()> {
        test(("#[\n|]#", "ihello world<esc>", "hello world#[|\n]#")).await?;
        Ok(())
    }

    mod auto_pairs;
    mod command_line;
    mod commands;
    mod filetree;
    mod movement;
    mod plugin;
    mod plugin_async;
    mod plugin_components;
    mod plugin_cross_buffer;
    mod plugin_decorations;
    mod plugin_doc;
    mod plugin_docchange;
    mod plugin_entry;
    mod plugin_fsasync;
    mod plugin_input;
    mod plugin_input_multiline;
    mod plugin_layout;
    mod plugin_layout2;
    mod plugin_lsp;
    mod plugin_lsp_mock;
    mod plugin_manager;
    mod plugin_multipanel;
    mod plugin_palette;
    mod plugin_panel;
    mod plugin_panel_focus;
    mod plugin_paste;
    mod plugin_picker;
    mod plugin_popup_edit;
    mod plugin_reload;
    mod plugin_reload_real;
    mod plugin_run;
    mod plugin_selection;
    mod plugin_statusline;
    mod plugin_terminal_hooks;
    mod plugin_terminal_modes;
    mod plugin_terminal_view;
    mod plugin_theme;
    mod plugin_yank_error;
    mod splits;
    mod window_mode;
}
