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
- 每个字段:`type`(必填,`boolean` / `number` / `string` / `enum` / `array`)、`default`、`doc`;
  `enum` 需要额外的 `options` 数组;`array` 可带 `items: "boolean"|"number"|"string"` 逐元素校验
  (不写 `items` = 只要求"是数组";错误串会点到**第几项**)。
- **不支持嵌套对象**、带点号的多级段;数组可以(元素只能是标量)。
- 同名重复声明 = 后声明覆盖(热重载时正常重注册)。
- `name` 与 `helix.plugin(name)` 里的名字**没有绑定关系**,它只是段名;两者取一致才便于用户理解。

## 2. 用户侧(config.toml)

```toml
[plugins.filetree]
show_hidden = true
refresh_ms  = 200

[plugins.which-key]
position = "center"
language = "zh-CN"   # 或 "en"(用引擎原始英文说明)
avoid_panels = true  # 自动躲开 dapui/filetree 等插件面板(见下)
mode = "box"         # box=浮层 / line=一行摘要进状态栏,不出浮层
max_lines = 18       # box 模式的行数上限(0=不限);超出显示「… 还有 N 个键」
```

三种形态：

| `mode` | 浮层 | 状态栏 | 适用 |
|---|---|---|---|
| `box`（默认） | 完整键位表（`max_lines` 收口 + 避让面板） | — | 想要"看一眼全部键" |
| `line`（推荐） | **1 行，只列键名**（`f F e . b j …`） | 一行带说明的摘要（`Space ▸ f 文件选择器 · …`） | 常用：不挡内容，且**不会被别的消息吞掉** |
| `status` | 不画（核心 `{ hide: true }`） | 一行带说明的摘要 | 极致干净，但状态栏是共享消息位，可能被 dap/LSP 的消息盖掉 |

`line` 为什么还要留一行浮层：状态栏是**共享**的，调试时 dap 的 `set_status` 一多，
纯状态栏提示会一闪就没（实测）；1 行浮层的宽度只有几十列、高度 1，配合 `avoid_panels`
几乎总能找到空位，而且在按键期间是持久的。

`avoid_panels`(默认开):提示框按 `helix.pane.list()` 里各 **PluginPanel** 的 `area` 挑一个
不重叠的角(首选仍是 `position`,其次另外三个角,全重叠时取重叠面积最小的)。
不开就是老行为 —— 固定 `position`,在 dapui 面板开着时会被糊住。

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
- **脚本顶层读也有值**(2026-10-23):`define_config` 之后紧接着 `const CFG = helix.get_config(..)`
  时缓存还没重建(重建发生在整段脚本跑完之后),`get_config` 会**就地按 schema + 用户覆盖算一次**
  ⇒ 拿到的是默认值 + 覆盖,不是 `null`。
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
| `[plugins.filetree]` | `show_hidden` |
| `[plugins.icons]` | `nerd_font` |
| `[plugins.picker-ui]` | `max_items` / `width` / `height` |
| `[plugins.statusline]` | `left` / `center` / `right`(段名数组)· `zones`(比例数组)· `corners` |
| `[plugins.tabbar]` | `max_labels` |
| `[plugins.terminal]` | `shell` / `float_size` / `vterm_size` / `hterm_size` |
| `[plugins.which-key]` | `position` / `language` / `show_docs` / `avoid_panels` / `mode` / `max_lines` |

### 6.1 `[plugins.statusline]` —— 内容、位置、圆角

三个区各一个**段名数组**(顺序即显示顺序),段间由核心插 1 空格 gap:

```toml
[plugins.statusline]
left    = ["mode", "file", "git"]
center  = ["diagnostics"]
right   = ["position", "percent", "modified", "chars", "filetype", "lines"]
zones   = [1, 3, 2]   # 左/中/右 宽度比例(**注册时读一次** ⇒ 改后需 :plugin-reload)
corners = "line"      # off | line | powerline(需 Nerd Font 有 U+E0B6/U+E0B4)
```

可用段名:`window` `mode` `file` `git` `diagnostics` `position` `percent` `modified`
`selections` `readonly` `encoding` `line-ending` `register` `chars` `indent` `filetype`
`cwd` `code-action` `workspace-diag` `spinner` `lines`。

- 不写数组 = 默认布局(相当于旧版的观感);**拼错的段名会在状态栏里显示成红字 `!名字`**。
- 段不适用的那一帧返回空 ⇒ 自动隐藏(不会留双空格)。
- `corners = "powerline"` 的端帽颜色由 `helix.set_theme` 派生(`statusline.cap.<mode>`:
  fg = mode 段底色,bg = `ui.statusline` 底色),换主题时自动重算。
- **外部段**:其他插件可用 `helix.register_statusline_segment(name, fn)` 注册段，把 `name` 写进上面
  任一列表即生效；**内置段名优先**(同名时内置实现生效)，未注册的名字显示红字 `!名字`。

## 7. 已知限制

- **`zones` 是注册态**:`[plugins.statusline] zones` 只在 `set_statusline` 注册时读一次 ⇒ 改比例要
  `:plugin-reload`;同段的 `left/center/right/corners` 是每帧读,`:config-reload` 即时生效。
- **声明了却没人读的键已清零(2026-10-23)**:`filetree.show_hidden` 与 `tabbar.max_labels` 已接线;
  另两个键**从 schema 删除**了,因为当前拿不到实现它们的数据 —— `filetree.refresh_ms`(boa 没有
  定时器)与 `tabbar.show_dirty`(布局快照 `leafs` 里没有逐叶子的 buffer id)。写了会报
  `未在 define_config 中声明（已忽略）`,不会静默生效。
- 校验/告警一次只显示一条(见 §2)。
- `[plugins]` 顶层键目前只被捕获、没有读取入口(不存在 `get_config("")` 这种用法)。
- 不支持带点号的段(`[plugins.a.b]` 不是 `a` 的嵌套配置,`a` 只有 schema 时 `b` 会被当未知段忽略)。

## 8. 实现位置

| 层 | 位置 |
|---|---|
| JS API(`define_config` / `get_config` / `get_config_docs` + 合并/校验) | `helix-js/src/config.rs` |
| schema 注册表 / 用户覆盖 / 合并缓存(**按宿主**,`HostState` 字段) | `helix-js/src/host.rs`、`helix-js/src/state.rs` |
| `[plugins]` TOML → JSON、启动与 `:config-reload` 时重建 | `helix-term/src/commands/typed.rs`(`build_plugin_configs`) |
| 插件 load/reload 后重建 | `helix-term/src/commands/typed.rs`(`refresh_plugin_configs`)、`helix-js/src/commands.rs`(`refresh_configs_after_load`) |
| 设计文档 | [`docs/superpowers/specs/2026-08-15-plugin-config-design.md`](../superpowers/specs/2026-08-15-plugin-config-design.md) |

> 内置插件有两份副本:`plugins/`(源)与 `runtime/plugins/`(随软件分发)。
> 改完源要跑 `contrib/install-plugins.sh`;`cargo xtask api-check` 会校验两份逐文件一致。
