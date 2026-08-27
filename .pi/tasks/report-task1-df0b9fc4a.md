# 任务 1 报告 — helix-js set_completion_icon 钩子 + completion_kind_icon 查询

计划: docs/superpowers/plans/2026-08-26-js-completion-icon-hook.md 任务 1
Commit: df0b9fc4a

## 实现内容(3 文件,79 insertions / 2 deletions)

1. `helix-js/src/state.rs`
   - thread_local 加 `COMPLETION_ICON_HOOK`(仿 `BUFFER_ICON_HOOK`,Box::leak 防 SIGABRT 同款注释)
   - 加 `with_completion_icon_hook` 访问器(仿 `with_buffer_icon_hook`)
2. `helix-js/src/popup.rs`
   - `js_set_completion_icon`:1 参,非 callable → JS 报错,仿 `js_set_buffer_icon`
   - `pub fn completion_kind_icon(kind: u8) -> Option<String>`:调钩子传 kind 数字;未注册/返回空串/非字符串/抛错 → None(仿 `bufferline_icon`);`JsValue::from(kind)` 已验证 boa 0.21 `impl_from_integer!` 含 u8
   - 底部 `#[cfg(test)] mod tests` 加 `completion_icon_hook` 测试(计划原文,未改断言)
3. `helix-js/src/lib.rs`
   - builder 注册 `set_completion_icon` 1 参(紧邻 `set_buffer_icon` 注册行)

## TDD 证据

- RED:`cargo test -p helix-js --lib completion_icon_hook` → `error[E0425]: cannot find function completion_kind_icon` ×4 + `error[E0603]: static TEST_LOCK is private`
- GREEN:`cargo test -p helix-js --lib` → 69 passed;0 failed(含 `popup::tests::completion_icon_hook` ok)
- clippy:`cargo clippy -p helix-js` → 零警告

## 与计划的偏差(均为让计划自带测试可编译的最小改动)

- lib.rs `mod tests` → `pub(crate) mod tests`(测试放 popup.rs 需访问 `crate::tests::TEST_LOCK`,私有 mod 下 E0603)
- lib.rs `static TEST_LOCK` → `pub(crate) static TEST_LOCK`(同上,static 本身也需可见)

## 自检

- 钩子全路径分支已覆盖:未注册/透传/空串回退/抛错回退/非函数报错
- 与现有 `bufferline_icon` 模式逐行对齐(含 `crate::init()` 惰性初始化)
- 未引入新依赖/抽象;无 ponytail 标记项

## 遗留

- 任务 2(helix-term completion.rs format 接线)待做;进度 ledger 已更新为 Task 1 complete(df0b9fc4a)
- 手动验证(init.js 注册钩子后看候选行图标)留两任务完成后
