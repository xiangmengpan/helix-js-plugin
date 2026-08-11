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
    mod plugin_palette;
    mod movement;
    mod plugin;
    mod plugin_doc;
    mod plugin_docchange;
    mod plugin_reload;
    mod plugin_selection;
    mod plugin_run;
    mod plugin_popup_edit;
    mod plugin_async;
    mod plugin_panel;
    mod plugin_terminal_view;
    mod plugin_fsasync;
    mod plugin_components;
    mod plugin_theme;
    mod plugin_layout;
    mod plugin_manager;
    mod plugin_multipanel;
    mod plugin_entry;
    mod plugin_statusline;
    mod splits;
}
