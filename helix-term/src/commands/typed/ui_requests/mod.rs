//! `apply_ui_requests` 的请求族 handler(G7)。
//!
//! `apply_ui_requests` 原先是一个 689 行的函数,43 个 arm 各自直接做 UI 接线
//! (建弹窗、`job::dispatch_blocking`、`compositor.replace_or_push`)。现在:
//! 每个 arm 一个 handler,按族分文件;`typed.rs` 里只剩一个 43 行的分派 match。
//!
//! `use super::*;` 是刻意的:`typed.rs` 的私有导入(`bail` / `job` / `ui` / `layer_id` …)
//! 对子模块可见,handler 体因此可以保持原样,不用重写路径。

use super::*;

pub(super) mod doc;
pub(super) mod editor;
pub(super) mod pane;
pub(super) mod picker;
pub(super) mod popup;
pub(super) mod server;
pub(super) mod term;
pub(super) mod watch;

pub(crate) mod ui_op;
pub(crate) use ui_op::{dispatch, UiOp};
