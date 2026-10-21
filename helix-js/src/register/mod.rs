//! 例外清单：**表驱动注册剩下的两个特例**（其余成员见 `crate::api::API`）。
//!
//! * `icons` —— 成员来自 `crate::icons::native_fns()`（数据驱动，表里只有登记行）；
//! * 函数对象 `helix.plugin` 与常量 `TABBAR_ID` 直接在 `api.rs` 里构造。
//!
//! 历史：这里曾有 15 个文件、~700 行 `.function(...)` 样板，与 `helix.d.ts` / `docs/`
//! / 三个对账脚本各存一份事实。2026-10 收敛到 `api.rs` 一张表（见该文件的模块注释）。
//!
//! 新增 API 请改 `api.rs`，**不要**在这里加文件。

pub(crate) mod icons;
