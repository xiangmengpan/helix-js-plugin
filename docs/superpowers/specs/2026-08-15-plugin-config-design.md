# 设计:插件配置 plugin-config(方案 C:声明式 + 分段 + 全局段)

日期:2026-08-15
状态:已确认(方案 C;分段 [plugins.<name>] + 全局 [plugins] 混合;全部插件接入)

## 1. API

```js
// 插件声明 schema(加载时调用)
helix.define_config("filetree", {
  show_hidden: { type: "boolean", default: false, doc: "显示隐藏文件" },
  refresh_ms:  { type: "number",  default: 500,   doc: "自动刷新间隔(ms)" },
  icon_style:  { type: "enum",    default: "default", options: ["default", "nerd"], doc: "图标风格" },
});

// 读取(合并用户覆盖 + 校验后;含默认值)
helix.get_config("filetree")
// → { show_hidden: false, refresh_ms: 500, icon_style: "default" }
// 未声明 schema 的 name → null

// 配置说明(可选,渲染帮助/设置面板)
helix.get_config_docs("filetree")
// → [{key, type, default, doc}]
```

支持类型:`boolean` / `number` / `string` / `enum`(options)。本期不支持嵌套对象/数组。

## 2. config.toml 结构(分段 + 全局段混合)

```toml
[plugins]                  # 全局插件设置(无主插件名)
plugin_dir = "~/my-plugins"

[plugins.filetree]         # 按插件分段(插件名 = 子表名)
show_hidden = true
refresh_ms = 200
```

- `[plugins.<name>]` 的键对 `define_config(name)` 的 schema 校验(类型/enum)
- 未知插件名的段 / 未知 key → 警告(不崩溃),值用默认
- `[plugins]` 顶层键 → 全局插件设置(`helix.get_config("")` 或独立读取)

## 3. 实现

### 3.1 Config 捕获 plugins 段

- `helix-term/src/config.rs`:`ConfigRaw` 加 `pub plugins: Option<toml::Table>`(deny_unknown_fields 下显式字段合法);`Config` 同样加
- config.toml 的 `[plugins]`/`[plugins.filetree]` 被捕获为 toml::Table(嵌套)

### 3.2 声明注册(helix-js)

- `js_define_config(name, schema)`:schema(JS 对象)转 JSON 存注册表(thread_local `CONFIG_SCHEMAS: HashMap<String, String>`)
- `js_get_config(name)`:读合并缓存(CONFIGS OnceLock<Mutex<String>> 或现算)
- `js_get_config_docs(name)`:从注册表 schema 返回文档数组

### 3.3 合并与校验(helix-term)

- `pub fn build_plugin_configs(config: &Config) -> String`:
  - 遍历 CONFIG_SCHEMAS(name → schema JSON)
  - 取 config.plugins 的 `[plugins.<name>]` 表 → 逐键校验(type/enum)→ 错误收集
  - 合并默认 + 覆盖 → JSON `{"<name>": {...}}`
- 调用时机:config load / :config-reload 后 → `helix_js::cache_configs(json)`
- 校验错误 → editor.set_error 显示(如 `"filetree.show_hidden: 期望 boolean,得到 string"`)

### 3.4 get_config 数据流

config load → build_plugin_configs → cache_configs(JSON)→ js_get_config(name) 读缓存 JSON.parse → 返回对象

## 4. 边界

- **只读**:get_config 只读;配置修改走 :config-reload
- **类型**:boolean/number/string/enum;非法 → 报错 + 用默认值
- **性能**:config 变更少,load/reload 时构建一次;get_config 读缓存 O(1)
- **插件名**:必须合法 TOML 表名(字母数字下划线)——现有插件名全部合法

## 5. 插件接入清单(全部)

| 插件 | 配置项 |
|---|---|
| filetree | show_hidden / refresh_ms / icon_style |
| terminal.js | default_shell / dock_side |
| statusline.js | show_git_branch / show_diagnostics / mode_icons |
| which-key.js | position / show_docs |
| tabbar.js | show_dirty / max_labels |
| icons.js | style(nerd/plain) |

## 6. 规模

- T1 核心:ConfigRaw.plugins + define_config/get_config/get_config_docs + build_plugin_configs 校验合并 + 单测(2 任务)
- T2 插件接入:6 个插件 define_config + 读取改造(2-3 任务)
- 集成测试(1 任务)
