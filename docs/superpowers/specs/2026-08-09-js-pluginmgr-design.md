# 设计：插件管理器（v11-③）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

`:plugin` 命令族管理插件：列出/安装/移除/重载。

## API

```
:plugin list                 # 列出已加载插件（名字 + 顺序）
:plugin install <path>       # 复制插件文件到 ~/.config/helix/plugins/ 并加载
:plugin remove <name>        # 从 plugins 目录删除文件
:plugin reload               # 既有热重载
:plugin status               # 已加载数量 + 目录路径
```

- `install <path>`：源可以是文件路径；复制为 `{name}.js`（basename 冲突检查）
- `remove <name>`：只删 plugins 目录下的 `{name}.js`（绝不删目录外文件——路径校验）
- 输出走状态栏（echo 风格）或弹窗列表（list 用弹窗显示更友好——列表长时）

## 实现

- **helix-js**：`pub fn loaded_scripts() -> Vec<String>`（LOADED_SCRIPTS 的名字列表；启动 + :plugin-load + install 都经 load_script_named 记录）
- **helix-term**（typed.rs）：`plugin` TypableCommand，args[0] = 子命令：
  - list → `helix_js::loaded_scripts()` → 状态栏 join（或 open_popup 列表）
  - install → `std::fs::copy(path, plugins_dir/name.js)`（name 从 path basename，`.js` 后缀强制）→ `load_script_named` → drain
  - remove → 校验 name（无路径分隔符）→ `std::fs::remove_file(plugins_dir/{name}.js)` → 若当前已加载，提示 reload
  - reload → 复用 plugin_reload 逻辑（提取共享）
  - status → 数量 + 目录路径（helix_loader::config_dir().join("plugins")）
- 安全：remove 的 name 白名单校验（拒绝 `/`、`..`、空）；install 的目标路径固定 plugins 目录

## 测试

- 单测：loaded_scripts 返回名字列表
- 集成（tests/test/plugin_manager.rs，临时 mod）：`:plugin install <temp.js>` → 文件出现在 plugins 目录（用临时 plugins 目录？config_dir 是固定的 ~/.config/helix——测试污染！→ install 目标目录需可注入或测试用临时 HOME？集成测试改 HOME 太重。方案：install 到既有 plugins 目录（用户机上有）风险大——改为 install 路径解析函数单测 + remove 的路径校验单测，集成只测 list/reload（不碰真实目录），报告中注明）
- 路径解析/校验单测：`plugin_name_from_path`（basename/.js 强制）、`validate_plugin_name`（拒绝路径分隔）

## 涉及文件

- `helix-js/src/lib.rs`（loaded_scripts + 单测）
- `helix-term/src/commands/typed.rs`（plugin 命令族 + 路径校验单测）
