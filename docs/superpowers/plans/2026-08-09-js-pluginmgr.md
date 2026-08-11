# 插件管理器实现计划（v11-③）

> 自主执行。TDD → 通过 → 提交；控制者自动合并（并行 wave）。

### 任务 1：:plugin 命令族

**文件：** `helix-js/src/lib.rs`（loaded_scripts）、`helix-term/src/commands/typed.rs`

- [ ] **步骤 1：失败单测（helix-js）**

```rust
#[test]
fn loaded_scripts_list() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script_named("a.js", r#"helix.register_command("a", () => {});"#).unwrap();
    load_script_named("b.js", r#"helix.register_command("b", () => {});"#).unwrap();
    let names = loaded_scripts();
    assert!(names.iter().any(|n| n == "a.js"));
    assert!(names.iter().any(|n| n == "b.js"));
}
```

- [ ] **步骤 2：helix-js 实现** `pub fn loaded_scripts() -> Vec<String>`（LOADED_SCRIPTS 名字）
- [ ] **步骤 3：路径校验单测**（typed.rs 或单独模块）：`plugin_name_from_path(path) -> Result<String>`（basename + 强制 .js 后缀 + 拒绝目录）；`validate_plugin_name(name)`（拒绝 `/`、`..`、空、非 .js）
- [ ] **步骤 4：:plugin 命令实现**（typed.rs）
  - `plugin` TypableCommand，`positionals: (1, Some(2))`，args[0]=子命令
  - list：`loaded_scripts()` → 空则 "no plugins loaded"；否则弹窗/状态栏列出（弹窗用 open_popup 显示——复用 PluginPopup？简化：状态栏 join，报告注明弹窗方案留后续）
  - install <path>：`plugin_name_from_path` → `fs::copy(path, config_dir()/plugins/name)` → `load_script_named(name, &src)` → drain messages
  - remove <name>：`validate_plugin_name` → `fs::remove_file(config_dir()/plugins/name)` → 提示 "removed; run :plugin reload if it was loaded"
  - reload：调 `helix_js::reload_all()`（与 plugin_reload 同逻辑——提取共享辅助 `reload_plugins(cx)`）
  - status：`loaded_scripts().len()` + plugins 目录路径
- [ ] **步骤 5：集成测试**（tests/test/plugin_manager.rs，临时 mod）：`:plugin status` → 状态栏含 "loaded";`:plugin list` → 状态栏含已加载插件名。install/remove 不碰真实目录（单测覆盖路径校验）——报告中注明
- [ ] **步骤 6：回归**（plugin 组、command_line、cargo check/clippy）
- [ ] **步骤 7：Commit** `feat(term): :plugin manager command (list/install/remove/reload/status)`

---

## 自检
- 风险：config_dir 在测试环境（集成测试会读到真实 ~/.config/helix——install/remove 的集成测试避开真实目录，只测 list/status）；弹窗列表方案简化；reload 共享提取。
