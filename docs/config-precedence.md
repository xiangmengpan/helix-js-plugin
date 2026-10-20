# 配置优先级：`config.toml` ↔ JS 插件

> **一句话总则**：**JS 管策略，TOML 管数据与引导。**
> 同一件事若两处都能配，一律"**后发生者生效**"（JS 运行期 > TOML 启动期），
> 但有三个例外是"只有一条路"（见 §3）——那三处别开第二个口子。

## 1. 为什么保留两份

| 职责 | 归属 | 理由 |
|---|---|---|
| 首帧**之前**就要生效的选项（`theme`、`editor.lsp.enable`、`idle_timeout`、`shell`） | **TOML** | 插件在首帧之后才加载（`Application::new` 先 `load_configured_theme` 再加载 `init.js`） |
| **用户偏好**（要持久、要能分享/同步） | **TOML** | `reload_all()` 会 `reset_plugin_state()` ⇒ JS 内存态**不能**当存储 |
| 行为 / UI / 键位 / 面板 / 插件设置 schema | **JS** | 需要逻辑；且 `helix.define_config` 已提供默认值 + 类型校验 + 文档 |

## 2. 可配两次的项：谁赢

| 项目 | TOML 入口 | JS 入口 | 生效时机 | 谁赢 | 重启后 |
|---|---|---|---|---|---|
| **键位** | `[keys.<mode>]` | `helix.map(mode, key, cmd)` | TOML：**启动一次**；JS：运行期（job 通道写入同一个 Keymaps 槽） | **JS**（`insert_binding` 把新叶 merge 进同一棵树 ⇒ 同序列覆盖） | TOML 的还在；**JS 的全部丢失**（已知限制，插件每次启动重新注册） |
| **主题** | `[theme]` | `helix.set_theme_name()` / `helix.set_theme(scopes)` | `:config-reload` 会重读 `[theme]` 并应用；JS 覆盖在调用时立即生效 | **JS**（后调者覆盖） | 回落 `[theme]` |
| **插件设置** | `[plugins.<name>]` | `helix.define_config(schema)` 只声明**默认值/类型**，无写入入口 | `:config-reload` 重建合并表；插件 load/reload 后也会重建 | 只有一个来源：**TOML 的值 + JS 的默认值** | 保留 |
| **状态栏 / 图标 / 补全 UI / 面板** | ——（无 TOML 入口） | `set_statusline` / `set_*_icon` / `set_completion_render` / `open_panel` | 注册即生效 | JS | 插件重跑时重新注册 |

## 3. 只有一条路的三个例外

1. **`editor.*`（62 个字段）**：没有任何 JS 写入口（没有 `set_option` 之类的 API）⇒ 只能写 TOML。
   好处是 `ConfigRaw` 带 `deny_unknown_fields`，键名写错直接报错；JS 侧要复刻这层保护得先有 schema。
2. **`[plugins.<name>]`**：是插件配置的**唯一**用户通道。JS 只声明 schema 与默认值，
   **不提供写回**——别为它加第二个入口（否则又变成两份实现，见 `plugin-vs-core.md` 的 R3）。
3. **`[theme]` 的初始值**：首帧要用，插件还没跑。

## 4. `:config-reload` 与 `:plugin-reload` 各管什么

| 命令 | 重读 config.toml | 重跑插件脚本 | 重应用 `[theme]` | **重载键位** |
|---|---|---|---|---|
| `:config-reload` | ✅ | ❌ | ✅ | ❌（**要重启**） |
| `:plugin-reload` | ❌ | ✅ | ✅（脚本若重新 `set_theme`） | ❌（旧绑定不撤销，只有脚本重跑时重新注册） |

> 键位不热更新是**刻意的**（`helix-term/src/application.rs:219` 的注释：启动快照）。
> 所以"改 `[keys]` 后没反应"不是 bug —— 重启。

## 5. 什么时候写哪儿

```
需要逻辑（条件 / 循环 / 读环境 / 生成）           → JS
需要首帧前生效                                    → TOML
需要被 reload 之后仍然存在                        → TOML（[plugins.*]）
只是"编辑器行为的一个开关"，且没有 JS 写入口        → TOML（editor.*)
同一件事两处都能做                                → 只留一处；已存在的靠"后发生者生效"过渡
```

## 6. 相关

- [`docs/plugin-api.md`](plugin-api.md) —— `helix.map` / `set_theme` / `define_config` 的签名与局限
- [`docs/superpowers/specs/2026-08-15-plugin-config-design.md`](superpowers/specs/2026-08-15-plugin-config-design.md) —— 插件配置（schema + 合并校验）的设计
- [`docs/plugin-vs-core.md`](plugin-vs-core.md) —— R3「别留两套实现」红线
