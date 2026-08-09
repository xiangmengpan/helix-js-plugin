# helix JS 插件系统（PoC）实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 在 helix 中嵌入 boa JS 引擎，让插件脚本可以注册命令（`:name` 调用）、读取文档/光标、向消息栏输出。

**架构：** 新 workspace member `helix-js`（纯 Rust，不依赖 helix-view/helix-term），持有 boa Context + 命令注册表 + echo 消息队列（全局 `OnceLock<Mutex<..>>`）。helix-term 只做两件事：把编辑器状态序列化成 `CommandContext`、调用运行时。分发挂在 `execute_command_line` 静态命令表 miss 的分支，补全合并到 `complete_command_line`。

**技术栈：** Rust（helix workspace）、boa_engine 0.21.1（纯 Rust JS 引擎）、anyhow、parking_lot（workspace 已有）。

---

## 文件结构

- 创建：`helix-js/Cargo.toml`
- 创建：`helix-js/src/lib.rs` — 运行时：init / load_script / run_command / take_messages / command_names + `CommandContext`
- 修改：`Cargo.toml` — workspace members 加 `"helix-js"`；workspace.dependencies 加 `boa_engine = "0.21.1"`
- 修改：`helix-term/Cargo.toml` — dependencies 加 `helix-js`（path）
- 修改：`helix-term/src/commands/typed.rs` — `execute_command_line` 分发钩子、`complete_command_line` 补全合并、`plugin-load` 命令
- 修改：`helix-term/src/application.rs` — 启动自动加载 `~/.config/helix/plugins/*.js`
- 修改：`helix-term/tests/integration.rs` — `mod plugin;`
- 创建：`helix-term/tests/test/plugin.rs` — 集成测试

---

### 任务 1：helix-js crate 脚手架 + boa 上下文 + `helix.echo`

**文件：**
- 创建：`helix-js/Cargo.toml`
- 创建：`helix-js/src/lib.rs`
- 修改：`Cargo.toml`（workspace 根）
- 测试：`helix-js/src/lib.rs` 内 `#[cfg(test)] mod tests`

- [ ] **步骤 1：注册 workspace member**

修改根 `Cargo.toml`：
- `members` 数组在 `"helix-event"` 后加 `"helix-js"`。
- `workspace.dependencies` 加：`boa_engine = "0.21.1"`。

创建 `helix-js/Cargo.toml`：

```toml
[package]
name = "helix-js"
version = "0.1.0"
edition = "2021"
description = "JavaScript plugin runtime for the Helix editor (PoC)"

[dependencies]
boa_engine = { workspace = true }
anyhow = "1"
parking_lot = { workspace = true }
```

- [ ] **步骤 2：编写失败的测试**

在 `helix-js/src/lib.rs` 中先写测试（此时 `init`/`load_script`/`take_messages` 还不存在，编译失败即"测试失败"）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // 多个测试共享全局运行时（boa Context 是全局单例），用锁串行化避免消息队列竞争
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn echo_captures_message() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(r#"helix.echo("hello from js");"#).unwrap();
        assert_eq!(take_messages(), vec!["hello from js"]);
    }
}
```

- [ ] **步骤 3：运行测试确认失败**

运行：`cargo test -p helix-js`
预期：编译错误，`init` / `load_script` / `take_messages` 未定义（crate 尚不存在时先创建文件让测试编译不过）。

- [ ] **步骤 4：编写最少实现代码**

`helix-js/src/lib.rs`：

```rust
use std::sync::{Mutex, Once, OnceLock};

use anyhow::{anyhow, Result};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsValue, NativeFunction, Source};
use parking_lot::Mutex as PLMutex;

/// 插件命令收到的只读上下文快照（由 helix-term 序列化编辑器状态得到）
pub struct CommandContext {
    pub path: Option<String>,
    pub text: String,
    pub cursor: (usize, usize),
}

// 注意声明顺序：boa 是 Gc 堆，REGISTRY 里的 JsValue 引用 CONTEXT 的堆，
// 静态量按声明逆序 drop，所以 REGISTRY 必须在 CONTEXT 之后声明。
static CONTEXT: OnceLock<PLMutex<Context>> = OnceLock::new();
static REGISTRY: OnceLock<PLMutex<std::collections::HashMap<String, JsValue>>> = OnceLock::new();
static MESSAGES: OnceLock<PLMutex<Vec<String>>> = OnceLock::new();
static INIT: Once = Once::new();

/// 创建 boa 上下文并注册全局 `helix` 对象（幂等）
pub fn init() {
    INIT.call_once(|| {
        let engine = Context::default();
        CONTEXT.set(PLMutex::new(engine)).expect("CONTEXT already set");
        REGISTRY.set(PLMutex::new(Default::default())).expect("REGISTRY already set");
        MESSAGES.set(PLMutex::new(Default::default())).expect("MESSAGES already set");

        let mut engine = CONTEXT.get().unwrap().lock();
        let helix = ObjectInitializer::new(&mut engine)
            .function(NativeFunction::from_fn_ptr(js_echo), "echo", 1)
            .build();
        engine
            .register_global_property("helix", helix, Attribute::READONLY | Attribute::NON_ENUMERABLE)
            .expect("register helix object");
    });
}

fn js_echo(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let text: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    MESSAGES.get().unwrap().lock().push(text);
    Ok(JsValue::undefined())
}

/// 求值一段插件脚本；脚本里可调用 `helix.register_command` / `helix.echo`
pub fn load_script(src: &str) -> Result<()> {
    init();
    let mut engine = CONTEXT.get().unwrap().lock();
    engine
        .eval(Source::from_bytes(src))
        .map(|_| ())
        .map_err(|e| anyhow!("plugin script error: {e}"))
}

/// 取走并清空 echo 消息队列
pub fn take_messages() -> Vec<String> {
    init();
    std::mem::take(&mut *MESSAGES.get().unwrap().lock())
}
```

- [ ] **步骤 5：运行测试确认通过**

运行：`cargo test -p helix-js`
预期：`echo_captures_message` PASS。

- [ ] **步骤 6：Commit**

```bash
git add Cargo.toml Cargo.lock helix-js
git commit -m "feat(js): add helix-js crate with boa context and helix.echo"
```

---

### 任务 2：`helix.register_command` + `run_command`

**文件：**
- 修改：`helix-js/src/lib.rs`
- 测试：`helix-js/src/lib.rs` 内 `#[cfg(test)] mod tests`

- [ ] **步骤 1：编写失败的测试**

在 `tests` 模块追加（沿用任务 1 的 `TEST_LOCK`）：

```rust
#[test]
fn register_and_run_command() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.register_command("where", (ctx) => {
            helix.echo("cursor: " + ctx.cursor.row + "," + ctx.cursor.col);
        });
        "#,
    )
    .unwrap();

    let ctx = CommandContext {
        path: Some("/tmp/demo.rs".to_string()),
        text: "hello\nworld".to_string(),
        cursor: (1, 2),
    };
    assert!(run_command("where", &ctx).unwrap());
    assert_eq!(take_messages(), vec!["cursor: 1,2"]);

    // 未注册的命令返回 false
    assert!(!run_command("nope", &ctx).unwrap());
    // 非法命令名（含空白）注册时报错
    assert!(load_script(r#"helix.register_command("bad name", () => {});"#).is_err());
}
```

- [ ] **步骤 2：运行测试确认失败**

运行：`cargo test -p helix-js`
预期：编译错误，`run_command` 未定义。

- [ ] **步骤 3：编写最少实现代码**

在 `helix-js/src/lib.rs`：

```rust
use boa_engine::object::builtins::JsFunction;
```

`init()` 中给 helix 对象加第二个方法：

```rust
let helix = ObjectInitializer::new(&mut engine)
    .function(NativeFunction::from_fn_ptr(js_echo), "echo", 1)
    .function(NativeFunction::from_fn_ptr(js_register_command), "register_command", 2)
    .build();
```

新增：

```rust
fn js_register_command(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let func = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return Err(JsError::from_opaque(JsValue::from(format!(
            "invalid command name: {name:?}"
        ))));
    }
    REGISTRY.get().unwrap().lock().insert(name, func);
    Ok(JsValue::undefined())
}

/// 把 CommandContext 转成 JS 对象 { doc: { path, text }, cursor: { row, col } }
fn ctx_to_js(ctx: &CommandContext, engine: &mut Context) -> boa_engine::JsResult<JsValue> {
    let doc = ObjectInitializer::new(engine)
        .property(
            "path",
            match &ctx.path {
                Some(p) => JsValue::from(p.clone()),
                None => JsValue::null(),
            },
            Attribute::all(),
        )
        .property("text", ctx.text.clone(), Attribute::all())
        .build();
    let cursor = ObjectInitializer::new(engine)
        .property("row", ctx.cursor.0, Attribute::all())
        .property("col", ctx.cursor.1, Attribute::all())
        .build();
    Ok(JsValue::from(
        ObjectInitializer::new(engine)
            .property("doc", doc, Attribute::all())
            .property("cursor", cursor, Attribute::all())
            .build(),
    ))
}

/// 运行插件命令。返回 Ok(true) 表示已运行，Ok(false) 表示未注册
pub fn run_command(name: &str, ctx: &CommandContext) -> Result<bool> {
    init();
    let func = REGISTRY
        .get()
        .unwrap()
        .lock()
        .get(name)
        .cloned();
    let Some(func) = func else { return Ok(false) };

    let mut engine = CONTEXT.get().unwrap().lock();
    let func = func
        .as_callable()
        .and_then(JsFunction::from_object)
        .ok_or_else(|| anyhow!("registered value for '{name}' is not a function"))?;
    let arg = ctx_to_js(ctx, &mut engine)?;
    let _: JsValue = func
        .call(&mut engine, (arg,))
        .map_err(|e| anyhow!("plugin command '{name}' failed: {e}"))?;
    Ok(true)
}

/// 已注册的插件命令名（供命令行补全）
pub fn command_names() -> Vec<String> {
    init();
    REGISTRY.get().unwrap().lock().keys().cloned().collect()
}
```

- [ ] **步骤 4：运行测试确认通过**

运行：`cargo test -p helix-js`
预期：两个测试均 PASS。

- [ ] **步骤 5：Commit**

```bash
git add helix-js
git commit -m "feat(js): add helix.register_command and run_command"
```

---

### 任务 3：helix-term 接线 — 分发、补全、`:plugin-load`、启动加载

**文件：**
- 修改：`helix-term/Cargo.toml`
- 修改：`helix-term/src/commands/typed.rs`
- 修改：`helix-term/src/application.rs`
- 修改：`helix-term/tests/integration.rs`
- 创建：`helix-term/tests/test/plugin.rs`
- 测试：`helix-term/tests/test/plugin.rs`（`cargo test -p helix-term --features integration --test integration plugin`）

- [ ] **步骤 1：添加依赖**

`helix-term/Cargo.toml` 的 `[dependencies]` 加：

```toml
helix-js = { path = "../helix-js" }
```

- [ ] **步骤 2：编写失败的集成测试**

`helix-term/tests/integration.rs` 的 `mod test { ... }` 内加 `mod plugin;`。

创建 `helix-term/tests/test/plugin.rs`：

```rust
use super::*;

use helix_core::diagnostic::Severity;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_load_and_run_command() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("test_plugin.js");
    std::fs::write(
        &path,
        r#"
        helix.register_command("hello", (ctx) => {
            helix.echo("cursor: " + ctx.cursor.row + "," + ctx.cursor.col);
        });
        "#,
    )?;

    test_key_sequences(
        &mut AppBuilder::new().build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", path.display())), None),
            (
                Some(":hello<ret>"),
                Some(&|app| {
                    let (status, severity) = app.editor.get_status().unwrap();
                    assert_eq!(severity, Severity::Info, "status: {status}");
                    assert_eq!(status.as_ref(), "cursor: 0,0");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}
```

- [ ] **步骤 3：运行测试确认失败**

运行：`cargo test -p helix-term --features integration --test integration plugin`
预期：FAIL —— `:hello` 报 "no such command: 'hello'"（severity Error）。

- [ ] **步骤 4：实现分发钩子 + `:plugin-load` + 补全**

`helix-term/src/commands/typed.rs`：

a) 顶部 import 区（现有 `use` 附近）加：

```rust
use helix_js::CommandContext;
```

b) `execute_command_line`（~L4125）中静态表 miss 分支替换为：

```rust
    match typed::TYPABLE_COMMAND_MAP.get(command) {
        Some(cmd) => execute_command(cx, cmd, rest, event),
        None if event == PromptEvent::Validate => {
            // 插件命令：序列化当前文档状态后交给 JS 运行时
            let (view, doc) = current_ref!(cx.editor);
            let text = doc.text();
            let pos = doc.selection(view.id).primary().cursor(text.slice(..));
            let line = text.char_to_line(pos);
            let col = pos - text.line_to_char(line);
            let ctx = CommandContext {
                path: doc.path().map(|p| p.to_string_lossy().into_owned()),
                text: text.to_string(),
                cursor: (line, col),
            };
            drop((view, doc)); // 释放对 editor 的不可变借用

            match helix_js::run_command(command, &ctx) {
                Ok(true) => {
                    let msgs = helix_js::take_messages();
                    if !msgs.is_empty() {
                        cx.editor.set_status(msgs.join(" ").into());
                    }
                    Ok(())
                }
                Ok(false) => Err(anyhow!("no such command: '{command}'")),
                Err(err) => Err(anyhow!("'{command}': {err}")),
            }
        }
        None => Ok(()),
    }
```

> 注意：`current_ref!` 的借用必须在使用完 `text`/`doc` 之后结束。上面先构建完所有 owned 数据（`text.to_string()`、`line`、`col`、`path`）再 `drop`，之后才调 `cx.editor.set_status`（可变借用）。

c) `complete_command_line`（~L4258）补全分支改为合并插件命令名：

```rust
    if complete_command {
        let plugin_names = helix_js::command_names();
        fuzzy_match(
            input,
            TYPABLE_COMMAND_LIST
                .iter()
                .map(|command| command.name)
                .chain(plugin_names.iter().map(String::as_str)),
            false,
        )
        .into_iter()
        .map(|(name, _)| (0.., name.into()))
        .collect()
    } else {
```

d) 新增 `plugin-load` 命令。在 `TYPABLE_COMMAND_LIST` 中（比如 `workspace-exclude` 条目之后）加：

```rust
    {
        name: "plugin-load",
        aliases: &[],
        doc: "Load a JavaScript plugin file.",
        fun: plugin_load,
        completer: CommandCompleter::all(completers::filename),
        signature: Signature { positionals: (1, Some(1)), ..Signature::DEFAULT },
    }
```

并新增函数（放在 `execute_command` 附近）：

```rust
fn plugin_load(cx: &mut compositor::Context, args: Args, event: PromptEvent) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let Some(path) = args.first() else {
        return Err(anyhow!("usage: plugin-load <path>"));
    };
    let src = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("failed to read '{path}': {e}"))?;
    helix_js::load_script(&src)
        .map_err(|e| anyhow!("plugin-load: {e}"))?;
    let msgs = helix_js::take_messages();
    if !msgs.is_empty() {
        cx.editor.set_status(msgs.join(" ").into());
    }
    Ok(())
}
```

> `Args` 支持迭代，检查 `args.first()` 是否可用；若该类型无 `first()`，改用 `for arg in args { ... }` 取第一个并 `break`。

- [ ] **步骤 5：启动自动加载**

`helix-term/src/application.rs`，在 `Self::load_configured_theme(&mut editor, ...)` 之后加：

```rust
        // Load JavaScript plugins from the config dir at startup (best effort)
        let plugin_dir = helix_loader::config_dir().join("plugins");
        if plugin_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&plugin_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "js") {
                        match std::fs::read_to_string(&path).and_then(|src| {
                            helix_js::load_script(&src).map_err(|e| std::io::Error::other(e.to_string()))
                        }) {
                            Ok(()) => {
                                for msg in helix_js::take_messages() {
                                    editor.set_status(msg.into());
                                }
                            }
                            Err(err) => editor.set_error(format!("plugin '{}' failed: {err}", path.display())),
                        }
                    }
                }
            }
        }
```

- [ ] **步骤 6：运行集成测试确认通过**

运行：`cargo test -p helix-term --features integration --test integration plugin`
预期：PASS。

再跑回归（确认没破坏既有命令分发）：
运行：`cargo test -p helix-term --features integration --test integration command_line`
预期：PASS。

- [ ] **步骤 7：手动冒烟**

```bash
mkdir -p ~/.config/helix/plugins
cat > ~/.config/helix/plugins/demo.js <<'EOF'
helix.register_command("whereami", (ctx) => {
  helix.echo("path: " + (ctx.doc.path ?? "<none>") + " at " + ctx.cursor.row + "," + ctx.cursor.col);
});
EOF
cargo run --release -- demo_file.txt   # 在编辑器里 :whereami，观察消息栏输出
```

预期：`:whereami` 在消息栏显示 "path: demo_file.txt at 0,0"。

- [ ] **步骤 8：Commit**

```bash
git add helix-term Cargo.lock
git commit -m "feat(js): wire plugin commands into command line and startup"
```

---

## 自检记录

- **规格覆盖度**：API 面（register_command / echo / ctx 只读快照）→ 任务 2；分发钩子 → 任务 3.4b；补全合并 → 任务 3.4c；`:plugin-load` → 任务 3.4d；启动自动加载 → 任务 3.5；错误处理（JS 错误 → anyhow → set_error）→ 任务 3.4b/d；测试 → 任务 1/2 单测 + 任务 3 集成测试。全部覆盖。
- **占位符扫描**：无"待定/TODO"；所有代码块为最终形态。唯一提示点是 `Args::first()`（任务 3.4d 注明了 fallback），以及 `command_names` 返回 `Vec<String>` 用 `as_str` 借用（任务 3.4c 已处理借用问题）。
- **类型一致性**：`CommandContext { path: Option<String>, text: String, cursor: (usize, usize) }` 在任务 2 定义、任务 3.4b 构造，字段名一致；`run_command -> Result<bool>`、`take_messages() -> Vec<String>`、`command_names() -> Vec<String>`、`load_script(&str) -> Result<()>` 签名在各任务中一致使用。
- **已知风险**：boa 0.21.1 的 `JsFunction::from_object`、`ObjectInitializer::function`、`register_global_property`、`try_js_into::<String>` 已在本计划编写时从 crate 源码核实存在。若编译报错，以编译器信息为准微调。
