# API:加载与生命周期、命令、键位

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

## 加载与生命周期

- **唯一入口**：启动只自动加载 `~/.config/helix/init.js`（兼容旧位置 `plugins/init.js`）；其他插件脚本必须经 `helix.load` 导入。
- `:plugin-load <path>` 可手动加载任意文件；`:plugin-reload`（或 `:plugin reload`）清空全部插件状态并按加载顺序重跑（模块文件重读磁盘）。
- 每个插件在**独立 IIFE 作用域**求值：顶层 `let`/`const` 不跨插件共享；`helix` 全局对象除外。
- 插件间共享状态只能通过 `helix.*` API（命令、事件、导出）或你自己的外部文件。

```js
// ~/.config/helix/init.js —— 唯一入口示例
helix.load("icons.js");                       // 立即导入
helix.lazy("terminal.js", "term");            // 懒加载：首次 :term 才加载
```

### `helix.load(name)` / `helix.export(obj)` / `helix.lazy(name, ...commands)`

```js
// mod.js 内：
helix.export({ helper: () => 42 });

// init.js 内：
const mod = helix.load("mod.js");   // 相对名解析到插件目录；重复加载返回缓存
mod.helper();                       // 42
```

- `helix.lazy("git.js", "gitbranch", "gitstatus")`：注册命令桩，首次调用任一命令时才加载插件再转执行真实命令。

**优缺点**
- 优点：init.js 单一入口清晰；懒加载减少启动开销；IIFE 隔离防止插件互相污染。
- 局限：跨插件状态只能走 `helix.*` API（无共享作用域）；**boa 0.21 嵌套 eval 污染**——依赖必须用 `helix.plugin` 的 `deps` 声明，不要在回调里 `helix.load` 未缓存脚本（见 plugin-api §19）。

### `helix.run_command(name, ctx?)`

程序化调用插件命令（`ctx` 可选，缺省空快照）。lazy 桩的内部底座，也可直接脚本化。

**优缺点**：优点：脚本化组合命令。局限：只能调**已注册的插件命令**（内置命令需经 `:name` 或键位）。

## 命令与消息

### `helix.register_command(name, fn, doc?)`

注册 `:name` 命令。`doc`（可选字符串）在命令行输入 `:name` 时显示在提示区。

```js
helix.register_command("hello", (ctx) => {
  helix.echo("world");
}, "打印 world");
```

- `fn(ctx)` 的 `ctx` 见 editing.md。
- 命令内可编辑文档、移动光标、开弹窗/面板/终端；全部效果在命令返回后原子应用（**一个命令 = 一次撤销**）。
- 命令内发起的 UI 请求（`open_file`/`move_panel`/`set_terminal_mode`/`split` 等）在命令边界 drain 并即时应用。

**优缺点**：优点：一个命令一次撤销；UI 请求自动应用。局限：主线程同步，重活用异步 API。

### `helix.echo(text)`

在状态栏显示消息。

**优缺点**：优点：最简单调试手段。局限：覆盖式无历史；被下一条消息覆盖即丢（可用 `:yank-error` 复制错误，`echo` 内容无 yank 出口）。

## 键位绑定

### `helix.map(mode, key, command)`

| 参数 | 说明 |
|------|------|
| `mode` | `"normal" \| "insert" \| "select"` |
| `key` | 键序列：单键 `"K"`、修饰键 `"C-n"`、空格分隔多键 `"C-n gd"`、逐字符多键 `"gd"` |
| `command` | 命令名字符串（内置或已注册插件命令）**或** JS 回调（自动注册为隐藏命令 `__mapped_N`） |

```js
helix.map("normal", "W", "show-where");              // 绑定插件命令
helix.map("insert", "C-n", () => helix.echo("hi"));  // 绑定回调
helix.map("normal", ";", "fileicon");                // 覆盖内置键位
```

- 重复绑定 = 覆盖；绑定立即生效；重启失效（插件启动时重新注册）。
- 回调收到 `(ctx)`（同命令 ctx）。

**优缺点**：优点：支持多键序列/修饰键；回调形式免注册命令。局限：重启失效；Shift-Tab 不可表示（boa KeyCode 无 BackTab）。
