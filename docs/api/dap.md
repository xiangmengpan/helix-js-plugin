# API:`helix.dap.*`(调试)

> 总览见 [`docs/plugin-api.md`](../plugin-api.md);设计见
> [`docs/superpowers/specs/2026-10-25-dap-api-p1.md`](../superpowers/specs/2026-10-25-dap-api-p1.md)。

三条入口:**同步快照**(`sessions`/`configs`/`list_breakpoints`,零 IO)、
**Promise**(其余 19 个,失败即 reject)、**事件**(9 个 `dap-*`)。

- `row`/`line` 一律 **0 起始**(与 `helix.lsp.*` / `set_virtual_text` 一致);DAP 协议的 1 起始换算在内核里。
- `id` 省略 = **当前活动会话**;没有会话时**reject**,不静默返回 null。
- `launch`/`attach` 声明了 `process` 能力 ⇒ 插件 manifest 里要写 `permissions: { process: true }`。

```js
// 最小用法:状态栏显示会话 + 命中时列变量
helix.register_statusline_segment("dap", () => {
  const s = (helix.dap.sessions() || [])[0];
  return s ? { text: `#${s.id} ${s.state}` } : null;
});

helix.on("dap-stopped", async (e) => {
  const frames = await helix.dap.stack_frames(e.thread_id);
  const scopes = await helix.dap.scopes(frames[0].id);
  const vars = await helix.dap.variables(scopes[0].variablesReference);
  helix.echo(vars.map((v) => `${v.name}=${v.value}`).join(" "));
});
```

## 成员

| 成员 | 说明 |
|---|---|
| `sessions()` | 会话快照 `[{id,name,state,active,thread?,frame?}]`(同步) |
| `configs()` | 可用调试配置 `[{name,request,language?,source,adapter}]`(P1 只有 `source:"lang"`) |
| `list_breakpoints(path?)` | 断点快照 `[{path,line,condition?,hit?,log?,verified,message?,id?}]`,顺序稳定 |
| `launch(name?, args?, id?)` / `attach(...)` | 起会话;`name` 是 `languages.toml` 的模板名(省略 = 第一个) |
| `terminate(id?)` / `restart(id?)` | 结束 / 重启 |
| `continue(id?)` / `pause(thread?, id?)` / `step_in` / `step_out` / `next` | 执行控制(线程省略 = 当前停止线程) |
| `threads(id?)` / `stack_frames(thread,id?)` / `scopes(frame,id?)` / `variables(ref,id?)` | 查询 |
| `eval(expr, frame?, id?)` | 表达式求值 → `{result,type?,variablesReference}` |
| `set_breakpoint(path,row,opts?)` / `remove_breakpoint(path,row)` / `clear_breakpoints(path?)` | 断点(会同步更新本地断点表,adapter 回填 `verified`) |
| `set_exception_breakpoints(filters)` | 异常断点 |
| `request(name, args?, id?)` | **进阶逃生口**:任意 DAP 请求,响应 body 原样返回 |

## 事件

| 事件 | payload |
|---|---|
| `dap-session` | `{id, kind:"created"\|"removed", name?, state?}` |
| `dap-stopped` | `{id, reason?, thread_id?, text?, all_threads_stopped?}`(**栈帧已拉完**才发) |
| `dap-continued` | `{id, thread_id?}` |
| `dap-exited` / `dap-terminated` | `{id, exit_code?}` / `{id, restart?}` |
| `dap-output` | `{id, category?, output}` |
| `dap-thread` | `{id, reason, thread_id}` |
| `dap-breakpoint` | `{id, path, line, verified, message?, removed?}` |
| `dap-progress` | `{id, kind:"start"\|"update"\|"end", text?, percentage?}` |

## 启动配置从哪来（三级合并）

| 优先级 | 来源 | 说明 |
|---|---|---|
| 高 | `<workspace>/.helix/launch.toml` | 自有格式，可带注释；同名时**逐字段覆盖**下一级 |
| 中 | `<workspace>/.vscode/launch.json` | VS Code 子集（JSONC：允许注释）；`type` 当 adapter 名 |
| 低 | `languages.toml [language.debugger].templates` | 兜底，永远可用（`source: "lang"`） |

```toml
# .helix/launch.toml
[[config]]
name = "run"
language = "rust"            # 省略 = 不按语言过滤
request = "launch"           # launch | attach
program = "${workspaceFolder}/target/debug/app"
cwd = "${workspaceFolder}"
args = ["--flag"]
env = { RUST_BACKTRACE = "1" }
```

支持的变量（**未知变量报错，不静默留字面量**）：
`${workspaceFolder}` `${file}` `${fileBasename}` `${fileDirname}` `${fileBasenameNoExtension}`
`${cwd}` `${env:VAR}`。

**明确不支持**（遇到就报错，不静默忽略）：`preLaunchTask` / `compounds` / `inputs`。
`launch.json` 的 `type`（adapter 名）会被解析成对应语言的 `[language.debugger]`。

`:debug-start`（或 `<space>G l`）的 Picker 里能看到合并结果，`来源` 列即 `toml`/`json`/`lang`。

## 同名 `:` 命令（键位之外的入口）

DAP 动作原先**只有键位**（`<space>G b` 之类），`:` 补全里一个都看不到。现在都接成了 typed 命令：

| 命令 | 别名 | 作用 |
|---|---|---|
| `:debug-start [模板] [参数…]` | `:dbg` | 起会话（省略参数则弹 Picker/Prompt） |
| `:debug-toggle-breakpoint` | `:dbgb` | 当前行断点开关 |
| `:debug-continue` / `:debug-next` / `:debug-step-in` / `:debug-step-out` | `:dbgc/:dbgn/:dbgi/:dbgo` | 执行控制 |
| `:debug-eval <expr>` | — | 在当前帧求值（结果显示在状态栏） |
| `:debug-variables` | `:dbgv` | 弹层列作用域/变量（Picker） |
| `:debug-terminate` / `:debug-restart` | `:dbgq` | 结束 / 重启 |
| `:debug-sessions` | — | 多会话切换 |

`helix.dap.*` 与这些命令**共用同一份实现**（内部都走 `DapIntent` → `helix.dap.*` 的派发路径）。

## 状态栏 / 面板

- 状态栏:段名 **`dap`** 由内置插件 `dapui` 注册(`helix.register_statusline_segment`),
  把它写进 `[plugins.statusline] left/center/right` 即显示。
- 面板:内置插件 `dapui` —— **右侧**面板是变量树(`:dapui`),**底部**面板是 REPL / Output 双 tab
  (`:dapui-repl` / `:dapui-output`,面板内 `Tab` 切换);断点用 `:dapui-breakpoints`(Picker)。
  配置见 `[plugins.dapui]`(`panel_size` / `bottom_size` / `auto_open` / `auto_open_repl` /
  `inline_values` / `highlight_current_line` / `current_line_scope` / `max_depth` / `max_nodes`)。
- 编辑器内:命中时**高亮当前执行行**(默认开,scope `dap.current-line`,主题没定义时按
  `ui.cursorline.primary` 的底色派生)+ 行尾内联值(默认关)。
