//! `cargo xtask theme-check` 的实现。
//!
//! 从 `main.rs` 的 `tasks` 模块拆出(G5):一个任务一个文件,统一入口
//! `run(args: Vec<String>)`,由 `tasks::TASKS` 表驱动 dispatch 与 help。

use crate::DynError;
use std::collections::HashSet;

pub fn run(args: Vec<String>) -> Result<(), DynError> {
    let themes = args.into_iter();
    use helix_view::theme::Loader;

    let themes_to_check: HashSet<_> = themes.collect();

    let theme_names = [
        vec!["default".to_string(), "base16_default".to_string()],
        Loader::read_names(&crate::path::themes()),
    ]
    .concat();
    let loader = Loader::new(&[crate::path::runtime()]);
    let mut errors_present = false;

    for name in theme_names {
        if !themes_to_check.is_empty() && !themes_to_check.contains(&name) {
            continue;
        }

        let (_, warnings) = loader.load_with_warnings(&name).unwrap();

        if !warnings.is_empty() {
            errors_present = true;
            println!("Theme '{name}' loaded with errors:");
            for warning in warnings {
                println!("\t* {}", warning);
            }
        }
    }

    match errors_present {
        true => Err("Errors found when loading bundled themes".into()),
        false => {
            println!("Theme check successful!");
            Ok(())
        }
    }
}
