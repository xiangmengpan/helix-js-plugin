# 设计：shell 执行 API（v6）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

`helix.run(cmd)`——插件同步执行 shell 命令并获取 stdout。解锁 git 状态、文件读取、外部工具调用。

## 新增 JS API

```js
const out = helix.run("git branch --show-current");  // -> string（stdout，含换行）
try {
  helix.run("git commit -m x");
} catch (e) { /* 非零退出码 / 启动失败 → 抛错，message 含 stderr */ }
```

- 参数必须是字符串，否则 JS 报错
- 执行：`sh -c <cmd>`（POSIX 语义），同步阻塞调用线程（= 阻塞编辑器主线程——**限短命令**）
- 返回 stdout 原始字节（UTF-8 lossy），截断到 64KB（防失控输出）
- 非零退出码 / 命令启动失败 → JsError（message 附 stderr，截断同 64KB）
- 无超时（`std::process` 无内置超时——PoC 接受，ponytail 注释：挂死命令会冻结编辑器）

## 实现

**纯 helix-js**（`std::process::Command`，无新依赖，无 helix-term 改动）：

- 原生函数 `js_run`（挂 helix 对象，arity 1）：
  - `try_js_into::<String>` 校验参数
  - `Command::new("sh").arg("-c").arg(&cmd).output()` → 失败（spawn）→ JsError
  - 非零 status → JsError（含 stderr）
  - 成功 → stdout 截断转 String → 返回
- 截断常量：`const RUN_OUTPUT_LIMIT: usize = 65536;`

## 测试

- **helix-js 单测**：`helix.run("echo hi")` 返回 "hi\n"；`helix.run("exit 3")` 抛错（Err 传播）；`helix.run(42)` 类型错误；超长输出截断（`head -c 100000 /dev/zero | tr '\0' 'x'` → 长度 ≤ 65536 + 尾注？——截断语义以实现为准，测试断言长度 ≤ 65536）
- **集成测试**（tests/test/plugin_run.rs）：插件 `:runchain` 调 `helix.run("echo plugin-shell-ok")` + echo → 状态栏断言（证明编辑器内同步调用不崩）

## 非目标

- 异步执行（v7 候选）、超时/取消、环境变量/工作目录控制、stdin
- stderr 单独通道（合并进错误消息）
- 权限沙箱（插件本就完全可信）

## 涉及文件

- `helix-js/src/lib.rs`（js_run + 单测）
- `helix-term/tests/test/plugin_run.rs`（新，集成测试）
