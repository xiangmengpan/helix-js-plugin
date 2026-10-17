//! `cargo xtask query-check` 的实现。
//!
//! 从 `main.rs` 的 `tasks` 模块拆出(G5):一个任务一个文件,统一入口
//! `run(args: Vec<String>)`,由 `tasks::TASKS` 表驱动 dispatch 与 help。

use crate::DynError;
use std::collections::HashSet;

pub fn run(args: Vec<String>) -> Result<(), DynError> {
    let languages = args.into_iter();
    use helix_core::syntax::LanguageData;

    let languages_to_check: HashSet<_> = languages.collect();
    let loader = helix_core::config::default_lang_loader();
    for (_language, lang_data) in loader.languages() {
        if !languages_to_check.is_empty()
            && !languages_to_check.contains(&lang_data.config().language_id)
        {
            continue;
        }
        let config = lang_data.config();
        let Some(syntax_config) = LanguageData::compile_syntax_config(config, &loader)? else {
            continue;
        };
        let grammar = syntax_config.grammar;
        LanguageData::compile_indent_query(grammar, config)?;
        LanguageData::compile_textobject_query(grammar, config)?;
        LanguageData::compile_tag_query(grammar, config)?;
        LanguageData::compile_rainbow_query(grammar, config)?;
    }

    println!("Query check succeeded");

    Ok(())
}
