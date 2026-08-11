# 设计：统一入口 init.js + 导入 + 懒加载（v12）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

插件系统从"全量自动加载 `*.js`"改为"唯一入口 `~/.config/helix/init.js`，其余脚本经 `helix.load` 导入"，并支持懒加载。

## 变更

### 加载模型

- 启动只加载 `~/.config/helix/plugins/init.js`（存在则加载；缺失则无插件）
- `:plugin-load <path>` 保留（手动加载任意文件）
- 其他脚本不再自动加载——必须被 init.js（或已加载脚本）`helix.load` 导入

### 新增 JS API

```js
helix.load(name)               // -> exports：从插件目录加载脚本（相对名或绝对路径），
                               //    返回其 helix.export 的值；重复加载返回缓存
helix.export(obj)              // 在当前脚本中声明导出（被 helix.load 的返回值拿到）
helix.lazy(name, ...cmds)      // 注册命令桩：首次调用 cmds 之一时才加载 name，然后转执行
helix.run_command(name, ctx?)  // 程序化调用插件命令（ctx 可选，缺省空快照）
```

### 语义

- `helix.load`：相对名（如 `"icons.js"`）解析到插件目录；绝对路径直接用；`.js` 后缀强制；文件不存在/语法错 → 抛错
- 加载记录进 LOADED_SCRIPTS（热重载可重跑）；导出缓存按名字（重复 load 返回缓存对象）
- 嵌套加载：a.js 里 load(b.js)——b 的导出被 b 的 load 消费，a 的导出被外层消费（take 语义）
- `helix.lazy(name, ...cmds)`：对每个 cmd 注册桩 `(ctx) => { helix.load(name); helix.run_command(cmd, ctx); }`（桩函数用 JS 引擎构造闭包，不经 REGISTRY 捕获问题）
- `helix.run_command(name, ctx?)`：把 ctx JS 对象反序列化为 CommandContext（缺省 path=None/text="" /cursor(0,0)）→ 走 run_command（排队效果由外层 drain 应用——嵌套调用合法）
- 热重载：reload_all 清导出缓存后**按名重读磁盘**重跑（模块文件更新生效；init.js 本体在记录中重跑旧文本——接受，注明）

## 实现

### helix-js

- thread_local（泄漏模式）：`SCRIPT_EXPORTS: HashMap<String, JsValue>`（名字 → 导出）、`LAST_EXPORT: Option<JsValue>`（pending 导出）
- 全局：`PLUGINS_DIR: OnceLock<PathBuf>` + `pub fn set_plugins_dir(dir)`（helix-term 启动时调用）
- 原生函数：`js_load`（路径解析 + 缓存 + 读文件 + IIFE eval + take LAST_EXPORT + 记录 LOADED_SCRIPTS）、`js_export`（设 LAST_EXPORT）、`js_lazy`（eval 工厂构造桩闭包 → register_command）、`js_run_command`（ctx 反序列化 + run_command）
- `reload_all`：清 SCRIPT_EXPORTS + LAST_EXPORT；重跑时按名重读磁盘（相对名解析到 PLUGINS_DIR）

### helix-term（application.rs）

- 启动加载：从"扫全部 `*.js`"改为"只加载 `init.js`"（cfg(not(feature="integration")) 门控保留）
- `Application::new` 调 `helix_js::set_plugins_dir(config_dir()/plugins)`

## 测试

- **helix-js 单测**（用 set_plugins_dir 指临时目录 + 写临时插件文件）：
  - load/export 往返：脚本 `helix.export({a:1})` → load 返回 `{a:1}`；重复 load 返回缓存（同一对象）
  - 嵌套：a.js export a 且 load(b.js export b) → load(a) 返回 a，load(b) 返回 b
  - lazy：`helix.lazy("l.js", "lcmd")`（l.js 注册 lcmd echo）→ run_command("lcmd") → 加载发生 + lcmd 执行
  - run_command 带 ctx：命令读 ctx.cursor → js_run_command 传 {cursor:{row,col}} → echo 正确
  - 校验：load 非字符串/未知文件 → 抛错
- **集成测试**（tests/test/plugin_entry.rs，临时 mod）：`:plugin-load <init.js>`（其内容 load 临时目录下另一文件 + lazy）→ 被导入插件的命令可用；懒加载命令首次调用后生效

## 非目标

- 模块系统（ESM import/export 语法——用 helix.load/export 约定）
- 依赖图/循环检测（嵌套 load 有缓存兜底；循环 load 会无限递归——接受并注明，或加简单递归深度限制）
- init.js 本体的热更新（重载重跑记录文本；模块文件重读磁盘）

## 涉及文件

- `helix-js/src/lib.rs`（load/export/lazy/run_command + 目录 + 缓存 + reload 重读 + 单测）
- `helix-term/src/application.rs`（只加载 init.js + set_plugins_dir）
- `helix-term/tests/test/plugin_entry.rs`（新）
- 演示：`~/.config/helix/plugins/init.js`（控制器更新，导入全部 + 懒加载重插件）
- 文档：`docs/plugin-api.md` 同步（加载模型 + 新 API + lazy 示例）
