//! `cargo xtask` 的子命令(G5:一任务一文件 + 单张 `TASKS` 表)。
//!
//! 为什么改成表驱动:原先 6 个任务写在 `main.rs` 的 `tasks` 模块里,dispatch 是
//! `match t.as_str()`,help 是手写 `println!` —— 两处各写一遍任务名,已经漂过
//! (`api-check` 的注释说"两者",实际是 3 个脚本)。现在**唯一真相源是 `TASKS`**:
//! dispatch 与 help 都由它生成,加任务只改一处。
//!
//! 加新任务的步骤:
//! 1. 新建 `tasks/<name>.rs`,写 `pub fn run(args: Vec<String>) -> Result<(), DynError>`;
//! 2. 在下面 `mod` 里加一行;
//! 3. 在 `TASKS` 里加一行(名字 / 入口 / help 文本)。
//!
//! `cargo xtask`(不带参数)打印的 help 直接来自 `TASKS`,不会再与 dispatch 漂移。

mod api;
mod api_describe;
mod docgen;
mod highlight;
mod indent;
mod query;
mod theme;

use crate::DynError;

/// 一条子命令:CLI 名、入口、help 文本。
///
/// help 文本里可以换行(用 `\n` + 对齐空格),原样打印在 `Tasks:` 区块下。
pub struct Task {
    pub name: &'static str,
    pub run: fn(Vec<String>) -> Result<(), DynError>,
    pub help: &'static str,
}

pub const TASKS: &[Task] = &[
    Task {
        name: "docgen",
        run: docgen::run,
        help: "Generate files to be included in the mdbook output.",
    },
    Task {
        name: "api-describe",
        run: api_describe::run,
        help: "Write the helix.* declaration surface (helix-js/src/api.rs) to target/api-surface.json.",
    },
    Task {
        name: "query-check",
        run: query::run,
        help: "Check that tree-sitter queries are valid for the given\n\
               languages, or all languages if none are specified.",
    },
    Task {
        name: "indent-check",
        run: indent::run,
        help: "Check indentation for the corpus files in tests/indent/\n\
               (named <language-id>.<ext>) against the configured grammars,\n\
               for the given languages, or all corpus files if none are specified.",
    },
    Task {
        name: "highlight-check",
        run: highlight::run,
        help: "Check highlight queries against the real highlighter using the\n\
               nvim-treesitter-style tests under tests/query/highlights/\n\
               <language-id>/<name>.<ext> (caret comment lines like\n\
               `// ^^^ @capture` assert the winning capture at the column\n\
               above), for the given languages, or all if none are specified.\n\
               `highlight-check --dump <language> <file>` instead prints the\n\
               winning capture per span for an arbitrary input file.",
    },
    Task {
        name: "theme-check",
        run: theme::run,
        help: "Check that the theme files in runtime/themes/ are valid for the\n\
               given themes, or all themes if none are specified.",
    },
    Task {
        name: "api-check",
        run: api::run,
        help: "Reconcile plugins/helix.d.ts, docs/plugin-api.md **and the\n\
               capability checkpoints** with the JS API registered from Rust\n\
               (helix-js/src/lib.rs · icons.rs · commands.rs); fails on drift.\n\
               Requires `node` on PATH.",
    },
];

pub fn find(name: &str) -> Option<&'static Task> {
    TASKS.iter().find(|t| t.name == name)
}

/// help 由 `TASKS` 生成 —— 不存在"help 里有、dispatch 里没有"的可能。
pub fn print_help() {
    println!("\nUsage: Run with `cargo xtask <task>`, eg. `cargo xtask docgen`.\n");
    println!("    Tasks:");
    for t in TASKS {
        let mut lines = t.help.lines();
        let first = lines.next().unwrap_or("");
        println!("        {:<26} {}", t.name, first);
        for l in lines {
            println!("        {:<26} {}", "", l);
        }
    }
    println!();
}
