//! JS API 注册表(G6) —— `build_engine` 从这里取各命名空间与根对象成员。
//!
//! 为什么拆:原先 `build_engine` 773 行、手工注册 134 个成员,宽度而非复杂度;
//! 改一个命名空间要在 773 行里找,新增一个成员要复制 6 行样板。
//!
//! 结构:
//! - `lsp` / `picker` / `server` / `pane` / `pane_layout` / `buffer` / `icons` / `plugin`
//!   —— 各自返回一个 `JsObject`(或可调用 `JsFunction`),由 `build_engine` 挂到 `helix` 上;
//! - `root_*` —— 挂在 `helix` **根对象**上的成员,接收 `&mut ObjectInitializer`;
//! - 裸路径 `commands::js_echo` / `layout_api::js_split` 之所以可用:下面这行把兄弟模块
//!   导入了本模块,子模块 `use super::*;` 就能看到。
//!   例外:`icons` / `lsp` / `picker` 与本目录同名,子模块里要写 `crate::icons::…`。
//!
//! ⚠ 改这里之后**必须**跑 `cargo xtask api-check`:它按源码文本解析注册表
//! (容器 = `let <ns>_obj = ObjectInitializer::new` 或 `let mut builder = ObjectInitializer::new`),
//! 变量改名/换文件都可能让门禁失明。

use crate::{
    commands, config, diagnostics, input, layout_api, permissions, popup, server_rows,
    server_tasks, shell, theme, watch,
};

pub(crate) mod buffer;
pub(crate) mod draw;
pub(crate) mod fuzzy;
pub(crate) mod icons;
pub(crate) mod lsp;
pub(crate) mod pane;
pub(crate) mod pane_layout;
pub(crate) mod picker;
pub(crate) mod plugin;
pub(crate) mod root_buffer;
pub(crate) mod root_doc;
pub(crate) mod root_fs;
pub(crate) mod root_term;
pub(crate) mod root_theme;
pub(crate) mod root_ui;
pub(crate) mod server;
