# API:插件配置(声明 → 合并 → 读取)

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

插件在加载时**声明**自己的配置项,用户在 `config.toml` 的 `[plugins.<name>]` 段**覆盖**;
默认值的注入、类型校验、以及"插件(重)加载后重建合并结果"都由核心负责。

## 1. 声明(插件侧,加载时一次)

```js
helix.define_config("filetree", {
  show_hidden: { type: "boolean", default: false, doc: "默认显示隐藏文件" },
  refresh_ms:  { type: "number",  default: 1000,  doc: "自动刷新间隔(ms;0 = 关闭)" },
  position:    { type: "enum",    default: "left", options: ["left", "right"], doc: "面板贴哪边" },
  width:       { type: "string",  default: "30%",  doc: "面板宽度" },
});
```

- `name`:必填非空字符串,即 `config.toml` 里 `[plugins.<name>]` 的段名(须是合法 TOML 键:
  字母/数字/`_`/`-`)。
- 每个字段:`type`(必填,`boolean` / `number` / `string` / `enum`)、`default`、`doc`;
  `enum` 需要额外的 `options` 数组。
- **只支持标量**:嵌套对象、数组、带点号的多级段都不支持。
- 同名重复声明 = 后声明覆盖(热重载时正常重注册)。
- `name` 与 `helix.plugin(name)` 里的名字**没有绑定关系**,它只是段名;两者取一致才便于用户理解。

## 2. 用户侧(config.toml)

```toml
[plugins.filetree]
show_hidden = true
refresh_ms  = 200

[plugins.which-key]
position = "center"
```

- `[plugins]` 的**顶层键**是全局插件设置(如 `plugin_dir`),不是某个插件的配置。
- 类型 / 枚举写错:该键报错并回退默认值,其余键照常生效 ——
  `filetree.refresh_ms: 期望 number，得到 string（用默认值）`。
- **拼错的键**会明确报出来:`filetree.show_hiden: 未在 define_config 中声明（已忽略）`。
  即"写了没生效"不会静默发生。
- 拼错的**段名**不报错:那个插件可能只是还没加载(懒加载),报了全是噪音。
- 显示位置:启动 / `:config-reload` 走**错误**级;插件加载后触发的那次重建走状态栏消息。
  多条错误时只有一条留在状态栏(后写的覆盖前面的)。

## 3. 读取(插件侧)

```js
const cfg = helix.get_config("filetree") || {};  // 默认值 + 用户覆盖;未声明 → null
cfg.show_hidden;                                  // false

const docs = helix.get_config_docs("filetree");   // [{key,type,default,doc}] | null
```

- 每次调用返回**新对象**:改它不影响缓存,也不影响别的插件。
- 没有声明过 `define_config` 的名字 → `null`(所以插件里习惯 `|| {}` 兜底)。
- **只读**:没有写入 API;改配置走编辑 `config.toml` + `:config-reload`。
- `get_config_docs` 是给**自建设置面板/帮助**用的原料(键名 + 类型 + 默认值 + 说明),不是现成的设置 UI。

## 4. 什么时候重建(装了/换了插件不会读到 null)

schema 是在**插件脚本执行时**才注册的,所以"合并结果"必须在脚本跑完后重算。核心在下列时机重建:

| 时机 | 带上用户 `config.toml` 覆盖 |
|---|---|
| 启动(init.js 全部脚本跑完) | 是 |
| `:config-reload` | 是(重读文件) |
| `:plugin-load` / `helix.load` / `helix.lazy` 首次触发 | 是 |
| `:plugin-reload` / `helix.reload_plugins()` | 是 |

## 5. 什么时候**读**到新值,取决于插件自己

- **每次现取**(`helix.get_config` 写在 render / 命令回调里):`:config-reload` 后**立即生效**。
  现状:which-key、statusline、dashboard、picker-ui、terminal(每次开终端时取)。
- **加载时缓存一份**(模块顶层 `const CFG = helix.get_config(...)`):改完配置要 `:plugin-reload`
  脚本才会重取。现状:filetree、tabbar。
- 特例:`icons` 的 `nerd_font` 在脚本加载时**应用一次**(`helix.icons.enabled(...)`),
  `:config-reload` 不重推 —— 要 `:plugin-reload`。

> 写新插件时优先"现取":用户改完 `config.toml` + `:config-reload` 就该看到效果。

## 6. 内置插件声明的键

| 段 | 键 |
|---|---|
| `[plugins.dashboard]` | `enabled` / `logo` / `width` / `height` |
| `[plugins.filetree]` | `show_hidden` / `refresh_ms`(⚠ 见 §7) |
| `[plugins.icons]` | `nerd_font` |
| `[plugins.picker-ui]` | `max_items` / `width` / `height` |
| `[plugins.statusline]` | `show_git_branch` / `show_diagnostics` |
| `[plugins.tabbar]` | `show_dirty` / `max_labels`(⚠ 见 §7) |
| `[plugins.terminal]` | `shell` / `float_size` / `vterm_size` / `hterm_size` |
| `[plugins.which-key]` | `position` / `show_docs` |

## 7. 已知限制

- **filetree / tabbar 声明的键目前没有被插件读取**:两个插件取了 `CFG` 却没用在逻辑里
  (`show_hidden` 走的是内部状态、`refresh_ms` 没有定时器),写进 `config.toml` 只会通过校验、
  不会改变行为。要么插件侧接线,要么从 schema 里删掉 —— 两种都比"看起来能配"好。
- 校验/告警一次只显示一条(见 §2)。
- `[plugins]` 顶层键目前只被捕获、没有读取入口(不存在 `get_config("")` 这种用法)。
- 不支持带点号的段(`[plugins.a.b]` 不是 `a` 的嵌套配置,`a` 只有 schema 时 `b` 会被当未知段忽略)。

## 8. 实现位置

| 层 | 位置 |
|---|---|
| JS API(`define_config` / `get_config` / `get_config_docs` + 合并/校验) | `helix-js/src/config.rs` |
| schema 注册表 / 用户覆盖 / 合并缓存(进程级) | `helix-js/src/state.rs` |
| `[plugins]` TOML → JSON、启动与 `:config-reload` 时重建 | `helix-term/src/commands/typed.rs`(`build_plugin_configs`) |
| 插件 load/reload 后重建 | `helix-term/src/commands/typed.rs`(`refresh_plugin_configs`)、`helix-js/src/commands.rs`(`refresh_configs_after_load`) |
| 设计文档 | [`docs/superpowers/specs/2026-08-15-plugin-config-design.md`](../superpowers/specs/2026-08-15-plugin-config-design.md) |

> 内置插件有两份副本:`plugins/`(源)与 `runtime/plugins/`(随软件分发)。
> 改完源要跑 `contrib/install-plugins.sh`;`cargo xtask api-check` 会校验两份逐文件一致。
