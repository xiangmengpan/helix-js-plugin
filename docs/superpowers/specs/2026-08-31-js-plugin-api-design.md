# 设计:插件管理器二期批次 4——JS API(helix.plugin.install/update/remove)

日期:2026-08-31
状态:已批准(方向:UiRequest 通道 + plugin() 核心逻辑提取共用)

## 背景与目标

命令面已完整(install/update/pin/unpin/remove/status,含依赖解析)。本批:`helix.plugin.install/update/remove` JS API 镜像命令,供插件编程式管理插件(自动装依赖、引导安装等)。

## 方案

### 1. API(helix-js 侧注册)

```js
helix.plugin.install(arg)   // arg: 本地路径或 git-url;镜像 :plugin install
helix.plugin.update([name]) // name 可选;镜像 :plugin update
helix.plugin.remove(name)   // 镜像 :plugin remove
```

- 返回 undefined(fire-and-forget);结果经状态栏显示(与命令一致)
- 参数校验:install 缺 arg → 报错;remove 缺 name → 报错

### 2. 通道:UiRequest

- helix-js 侧 push `UiRequest::PluginOp { op: String, arg: Option<String> }`
- term 侧 `apply_ui_requests` 消费:调提取的核心 helper
- 复用现有 UI_REQUESTS 管道(application.rs 泵 → typed.rs apply_ui_requests),零新机制

### 3. plugin() 核心逻辑提取

typed.rs 的 `plugin()`(命令入口)提取核心为 helper(命令与 JS 请求共用):

```rust
/// plugin 操作核心(命令 :plugin <op> 与 JS API helix.plugin.<op> 共用)
fn plugin_op(cx: &mut compositor::Context, op: &str, arg: Option<&str>) -> anyhow::Result<()>
```

- `plugin()` 命令:解析 sub + args → 调 plugin_op
- UiRequest::PluginOp:调 plugin_op
- 行为与现有命令逐字一致(install 的依赖解析/加载、update 汇总、remove manifest 等全复用)

### 4. 边界

- 同步阻塞(fs/git 操作,与命令一致);UI 冻结同命令
- 结果只走状态栏,无 JS promise 回调(最小版;需要结果回调二期再说)
- pin/unpin/status 不做 JS API(命令面已够,JS 面最小)

## 测试策略

- **helix-js 单测**:参数校验(install 缺 arg 报错、remove 缺 name 报错);op push 到 UiRequest 队列
- **integration**:`helix.plugin.remove("nope")`(未安装)→ 报错经 set_error;`helix.plugin.install` 无副作用路径(非法 arg 报错)

## 关键实现位置

- helix-js/src/types.rs(`UiRequest::PluginOp { op, arg }`)
- helix-js/src/commands.rs(js_plugin_install/update/remove 3 个 native fn + 注册到 helix.plugin 对象)
- helix-term/src/commands/typed.rs(plugin_op 提取 + apply_ui_requests PluginOp 分支)
- helix-term/tests/test/plugin_manager.rs(integration)
- docs/plugin-api.md(3 方法一节)
