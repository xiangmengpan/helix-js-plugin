# 设计：helix JS 键位绑定（v4）

日期：2026-08-09
状态：已批准

## 目标

让 JS 插件绑定键位：`helix.map(mode, key, command)`，command 可为内置/插件命令名或 JS 回调。运行时注入到实时 keymap，立即生效。

## 新增 JS API

```js
helix.map("normal", "gd", "goto-def");       // 绑定命令名（内置或插件命令）
helix.map("insert", "C-n", () => { ... });   // 绑定 JS 回调（自动注册为隐藏插件命令）
helix.map("normal", "K", "my-plugin-cmd");
```

- `mode` 白名单：`"normal" | "insert" | "select"`（非法 → JS 报错）
- `key`：键序列字符串——单键（`"K"`）、修饰键（`"C-n"`）、空格分隔多键（`"C-n gd"`）、逐字符多键（`"gd"`）。解析失败 → JS 报错
- `command`：
  - 字符串：命令名。执行时先查内置命令表，miss 再查插件命令注册表
  - 函数：自动注册为隐藏插件命令 `__mapped_<N>`（N 自增），绑定该名字
  - 其他类型 → JS 报错
- 重复绑定同一键 = 覆盖（merge_keys 语义）
- 绑定立即生效（ArcSwap 原子替换）；重启后失效（插件启动时重新注册）

## 实现

### helix-js（src/lib.rs）

- `UiRequest` 增加变体：`MapKey { mode: String, key: String, command: String }`
- 原生函数 `js_map`（挂 helix 对象）：
  - 校验 mode 白名单、key 非空字符串、command 为字符串或函数
  - 函数 → 生成 `__mapped_<N>`（独立计数器），注册进 REGISTRY（与 register_command 同机制），command 取该名字
  - 入队 `UiRequest::MapKey`
- 无需新公共函数（复用 take_ui_requests）

### helix-term

1. **运行时 keymap 槽**（application.rs）：
   - `static KEYMAPS: OnceLock<ArcSwap<HashMap<Mode, KeyTrie>>>`（helix-term 内）
   - 启动时 `KEYMAPS.set(ArcSwap::from_pointee(config.keys.clone()))`（config.keys 已含默认 + 用户覆盖）
   - `Keymaps::new(Box::new(KEYMAPS.get().unwrap().clone()))` 传给 EditorView（ArcSwap 实现 DynAccess，改动对渲染即时可见）
   - `ponytail:` 已知限制：keymap 改为启动快照，**config-reload 不再热更新键位**（需重启）

2. **应用 MapKey**（typed.rs 共享辅助 `apply_ui_requests`）：
   - 解析键序列：按空白分词 → 每 token 先 `KeyEvent::from_str`（成功=单键）→ 失败则逐字符
   - 由键序列 + `MappableCommand::Typable { name, args: "", doc: "" }` 构建 KeyTrieNode 链（叶子 → 逐层 Node）
   - `merge_keys(&mut *KEYMAPS.load().as_ref().clone(), delta)` → `KEYMAPS.store(...)`（load 克隆避免持有读锁跨写）
   - 调用点：(a) 命令分发 Ok(true) drain；(b) `:plugin-load` 命令后；(c) 启动加载插件后（startup 无 compositor，只应用 MapKey，跳过 OpenPopup）

3. **插件命令回退**（commands.rs `MappableCommand::execute` Typable 分支）：
   - `TYPABLE_COMMAND_MAP.get(name)` miss 时 → 回退插件命令注册表
   - 把 execute_command_line 里的插件分发逻辑提取为共享辅助 `run_plugin_command(cx, name)`（序列化 ctx → run_command → drain messages/ui/edits），两处复用

## 测试

- **helix-js 单测**：js_map 校验（非法 mode / 非字符串 key / 非法 command 类型 → 报错）；函数回调注册 `__mapped_<N>` 且 MapKey 入队；字符串命令入队
- **集成测试**：
  - 插件 `helix.map("normal", "X", "echo-hello")`（插件命令 echo）→ 按 `X` → 状态栏 "hello"
  - 回调绑定：`helix.map("normal", "Y", () => helix.echo("cb"))` → 按 `Y` → 状态栏 "cb"
  - 覆盖内置：`helix.map("normal", "j", "echo-hello")` → 按 `j` → 文本不变 + 状态栏 "hello"

## 非目标（YAGNI）

- 删除绑定、模式外绑定（debug 等）、持久化、config-reload 热更新
- 键序列解析的完整语法（修饰键组合如 "C-A-x"、特殊键全表——尽力而为，解析失败报错）

## 涉及文件

- `helix-js/src/lib.rs`（UiRequest::MapKey、js_map、计数器、单测）
- `helix-term/src/application.rs`（KEYMAPS 槽、EditorView 构造）
- `helix-term/src/commands/typed.rs`（apply_ui_requests、键序列解析、run_plugin_command 提取）
- `helix-term/src/commands.rs`（MappableCommand::execute 回退）
- `helix-term/tests/test/plugin.rs`（集成测试）
