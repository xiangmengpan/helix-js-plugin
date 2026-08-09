# 设计：插件热重载（特性 E，串行第三——重置 D/C 状态，必须最后）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

` :plugin-reload` 重新求值所有已加载的插件脚本（启动加载 + `:plugin-load` 的），先清空插件状态再按序重跑。

## 新增 API

```js
// 无新 JS API——重载是编辑器命令
:plugin-reload    // 重载全部已加载脚本
```

## 实现

### helix-js

- thread_local：`LOADED_SCRIPTS: RefCell<Vec<(String name, String src)>>`
- `pub fn load_script_named(name: &str, src: &str) -> Result<()>`：求值 + 记录（name 用于报错定位）；`load_script(src)` 保持为 `load_script_named("<anon>", src)` 包装
- `pub fn reload_all() -> Result<()>`：
  1. 清空插件状态：REGISTRY、EVENT_HANDLERS、POPUPS、BUFFER_ICON_HOOK、STATUSLINE_HOOK、COMMAND_DOCS、CURRENT_EDITS（id 计数器保留——单调性）
  2. 按加载顺序重跑全部 (name, src)；任一失败 → 返回带 name 的错误（其余继续跑完，还是失败即停？**失败即停**，错误含 name——报告注明）
- 键位绑定不清理：keymap 注入无法撤销（无 un-merge），重跑时 merge 幂等覆盖；脚本移除的旧绑定残留——已知限制（ponytail 注释）

### helix-term

- 新 TypableCommand `plugin-reload`（completer 无）：调 `helix_js::reload_all()` → 成功 drain 消息到状态栏；失败 `set_error`
- 启动加载与 `:plugin-load` 改用 `load_script_named`（记录名字）

## 测试

- **helix-js 单测**：
  - load_script_named 记录 + reload_all 重跑（脚本注册命令 → reload → 命令仍可运行；echo 计数类验证重跑发生）
  - reload_all 清空事件处理器（load 注册 handler → reload 后 has_handlers 为 false → 重跑后又为 true）
  - 失败脚本 → reload_all 返回 Err 且错误含脚本名
- **集成测试**（tests/test/plugin_reload.rs）：`:plugin-load` → 命令可用 → `:plugin-reload` → 命令仍可用 + 状态栏无错误

## 非目标

- 单文件重载（`:plugin-reload <path>`——只做全量）
- keymap 绑定的增量撤销（残留旧绑定，注明）
- 依赖图/加载顺序拓扑排序（按加载顺序重跑）

## 涉及文件

- `helix-js/src/lib.rs`（LOADED_SCRIPTS、load_script_named、reload_all、reset、单测）
- `helix-term/src/commands/typed.rs`（plugin-reload 命令、plugin_load/启动改 named）
- `helix-term/src/application.rs`（启动加载改 named）
- `helix-term/tests/test/plugin_reload.rs`（新）
