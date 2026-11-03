//! `helix.log` 的 JS 面：参数形态、级别解析、条目落进缓冲。
//!
//! 为什么要在这里测而不是只测 `record()`：参数形态是**用户直接写的**，
//! 「`log("error")` 到底是"一条内容为 error 的消息"还是"一条空 error"」这类歧义
//! 只有走一遍 JS 才看得出来。

use super::*;

#[test]
fn log_accepts_message_only_and_level_plus_message() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::log::clear();
    load_script(
        r#"
        helix.log("plain message");
        helix.log("warn", "with level");
        helix.log("nope", "unknown level falls back to info");
        helix.log("error");
        "#,
    )
    .unwrap();

    let entries = crate::log::snapshot();
    assert_eq!(
        entries.len(),
        4,
        "四条都要进缓冲（不许静默丢）: {entries:?}"
    );
    // 单参形态 = 消息本身，级别 info（**不能**把 "error" 猜成级别 ⇒ 空 error）
    assert_eq!(entries[0].msg, "plain message");
    assert_eq!(entries[0].level, crate::log::Level::Info);
    // 两参形态：级别生效
    assert_eq!(entries[1].level, crate::log::Level::Warn);
    assert_eq!(entries[1].msg, "with level");
    // 认不出的级别退回 info，消息照记
    assert_eq!(entries[2].level, crate::log::Level::Info);
    assert_eq!(entries[2].msg, "unknown level falls back to info");
    // `log("error")` 是**一条消息**，不是空 error
    assert_eq!(entries[3].level, crate::log::Level::Info);
    assert_eq!(entries[3].msg, "error");

    crate::log::clear();
}

#[test]
fn log_entries_carry_the_calling_plugin_as_target() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::log::clear();
    // 具名脚本 ⇒ 属主是脚本名，条目 target 自动带插件名（用户不用自己拼前缀）
    let st = crate::host::state_handle_or_init();
    crate::commands::load_script_named(&st, "my-plugin", r#"helix.log("hi");"#).unwrap();
    let entries = crate::log::snapshot();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].target, "my-plugin");
    assert_eq!(entries[0].msg, "hi");
    crate::log::clear();
}
