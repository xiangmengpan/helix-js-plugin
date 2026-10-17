use std::path::{Path, PathBuf};

pub fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// `plugins/tools/check_api_surface.mjs` —— 对账 `plugins/helix.d.ts` 与 Rust 侧注册的 JS API。
pub fn plugin_api_surface_script() -> PathBuf {
    project_root()
        .join("plugins")
        .join("tools")
        .join("check_api_surface.mjs")
}

/// `docs/plugin-api.md ↔ 注册表` 的对账脚本(api-check 的第二半)
pub fn plugin_doc_surface_script() -> PathBuf {
    project_root()
        .join("plugins")
        .join("tools")
        .join("check_doc_surface.mjs")
}

pub fn book_gen() -> PathBuf {
    project_root().join("book/src/generated/")
}

pub fn runtime() -> PathBuf {
    project_root().join("runtime")
}

pub fn ts_queries() -> PathBuf {
    runtime().join("queries")
}

pub fn themes() -> PathBuf {
    runtime().join("themes")
}

pub fn tests_indent() -> PathBuf {
    project_root().join("tests").join("indent")
}

pub fn tests_highlight() -> PathBuf {
    project_root()
        .join("tests")
        .join("query")
        .join("highlights")
}
