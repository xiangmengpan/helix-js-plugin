# 插件 JS API 实现计划(二期批次 4)

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** `helix.plugin.install/update/remove` JS API 镜像命令——UiRequest 通道 + `plugin_op` 核心提取共用。

**架构:** helix-js 3 个 native fn 校验参数 → push `UiRequest::PluginOp { op, arg }`;term 侧 apply_ui_requests 消费调 `plugin_op(cx, op, arg)`(从现有 plugin() 命令提取,命令与 JS 共用)。

**技术栈:** Rust(helix-js boa + helix-term)。

**规格:** `docs/superpowers/specs/2026-08-31-js-plugin-api-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-js/src/types.rs` | `UiRequest::PluginOp { op: String, arg: Option<String> }` |
| `helix-js/src/commands.rs` | `js_plugin_install/update/remove` + 注册到 helix.plugin 对象 |
| `helix-js/src/lib.rs` | native fn 注册表(helix.plugin 嵌套对象) |
| `helix-term/src/commands/typed.rs` | plugin() 提取 plugin_op + apply_ui_requests PluginOp 分支 |
| `helix-term/tests/test/plugin_manager.rs` | integration |
| `docs/plugin-api.md` | 3 方法一节 |

关键实现位置(执行时必读):

- **UiRequest 模式**:popup.rs:109 push OpenPopup 模式;types.rs enum
- **helix.plugin 对象**:js_plugin(commands.rs)注册在哪、helix.plugin 嵌套对象怎么构建(helix.lsp 是嵌套对象先例)
- **plugin() 命令**(typed.rs):现有 match sub 的 install/update/pin/unpin/remove/status——提取为 `plugin_op(cx, op: &str, arg: Option<&str>) -> Result<()>`,plugin() 命令解析 args 后调用;pin/unpin 的 arg 必填、status/list 不需要 arg(plugin_op 内处理)
- **apply_ui_requests**(typed.rs:4751 附近):OpenPicker/OpenPopup 分支旁加 PluginOp

---

### 任务 1:helix-js 侧(3 native fn + UiRequest + 注册)

**文件:**
- 修改:`helix-js/src/types.rs`、`helix-js/src/commands.rs`、`helix-js/src/lib.rs`
- 测试:commands.rs tests mod

- [ ] **步骤 1:写失败测试(参数校验 + push)**

```rust
#[test]
fn plugin_api_arg_validation() {
    crate::init();
    // install 缺 arg → 报错
    assert!(load_script(r#"helix.plugin.install();"#).is_err());
    // remove 缺 arg → 报错
    assert!(load_script(r#"helix.plugin.remove();"#).is_err());
    // 合法调用 → push PluginOp 请求
    crate::state::with_ui_requests(|r| r.clear());
    load_script(r#"helix.plugin.remove("nope");"#).unwrap();
    let reqs = crate::state::take_ui_requests();
    assert!(reqs.iter().any(|r| matches!(r, crate::types::UiRequest::PluginOp { op, arg } if op == "remove" && arg.as_deref() == Some("nope"))));
}
```

预期:FAIL(未注册/未定义)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-js plugin_api 2>&1 | tail -8`

- [ ] **步骤 3:实现**

types.rs:

```rust
PluginOp {
    op: String,
    arg: Option<String>,
},
```

commands.rs(照 js_plugin 旁):

```rust
/// helix.plugin.install(arg):镜像 :plugin install
pub(crate) fn js_plugin_install(
    _this: &JsValue, args: &[JsValue], context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let arg: String = args.first().unwrap_or(&JsValue::undefined()).try_js_into(context).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("helix.plugin.install: arg (path or git url) required")))
    })?;
    push_plugin_op("install", Some(arg));
    Ok(JsValue::undefined())
}

pub(crate) fn js_plugin_update(...) {
    // arg 可选
    let arg: Option<String> = args.first().filter(|v| !v.is_undefined()).map(|v| v.try_js_into(context)).transpose().map_err(...)?;
    push_plugin_op("update", arg);
}

pub(crate) fn js_plugin_remove(...) {
    let arg: String = ...; // 必填
    push_plugin_op("remove", Some(arg));
}

fn push_plugin_op(op: &str, arg: Option<String>) {
    crate::popup::push_ui_request(crate::types::UiRequest::PluginOp { op: op.to_string(), arg });
}
```

(push_ui_request 是否 pub——查 popup.rs 的 UI_REQUESTS push 封装;没有就照 OpenPopup 的 push 写法。)

lib.rs 注册到 helix.plugin 对象(js_plugin 注册处旁,helix.plugin 嵌套对象里加):

```rust
// helix.plugin 对象加 install/update/remove
.function(NativeFunction::from_fn_ptr(commands::js_plugin_install), JsString::from("install"), 1)
.function(NativeFunction::from_fn_ptr(commands::js_plugin_update), JsString::from("update"), 1)
.function(NativeFunction::from_fn_ptr(commands::js_plugin_remove), JsString::from("remove"), 1)
```

- [ ] **步骤 4:运行测试确认通过**

运行:`cargo test -p helix-js plugin_api 2>&1 | tail -8` + `cargo build -p helix-term`(UiRequest 加 variant 会破坏 apply_ui_requests 穷尽 match——任务 2 接,先加占位或让编译错指出)
预期:PASS + 编译错在 term 侧(预期)。

- [ ] **步骤 5:Commit**

```bash
git add helix-js/src/types.rs helix-js/src/commands.rs helix-js/src/lib.rs
git commit -m "feat(js): helix.plugin.install/update/remove——参数校验 + UiRequest::PluginOp"
```

---

### 任务 2:term 侧(plugin_op 提取 + apply_ui_requests 分支)+ integration

**文件:**
- 修改:`helix-term/src/commands/typed.rs`
- 修改:`helix-term/tests/test/plugin_manager.rs`
- 测试:同文件

- [ ] **步骤 1:写失败测试(integration)**

```rust
#[tokio::test(flavor = "multi_thread")]
async fn plugin_js_api_remove_uninstalled() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    let plugin = dir.path().join("api.js");
    std::fs::write(&plugin, r#"helix.plugin.remove("nope-js-api");"#)?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin.display())), None),
            // plugin_op 执行 → 未安装报错经 set_error → 状态栏可见
            (Some(":echo \"x\"<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert!(status.as_ref().contains("not installed"), "got: {status}");
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

**注意时序**:plugin-load 里 helix.plugin.remove push 请求,apply_ui_requests drain 时执行(plugin-load 命令内已 drain)。断言在后续命令后取状态。若时序不稳,在 load 后加一个空操作命令等 drain。

预期:FAIL(PluginOp 分支未接,或报错不达状态栏)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term --features integration --test integration plugin_manager 2>&1 | tail -10`

- [ ] **步骤 3:实现 plugin_op 提取**

typed.rs:plugin() 命令重构——核心逻辑移入 plugin_op,命令变薄:

```rust
/// plugin 操作核心(:plugin <op> 与 JS API helix.plugin.<op> 共用)
fn plugin_op(cx: &mut compositor::Context, op: &str, arg: Option<&str>) -> anyhow::Result<()> {
    match op {
        "list" => { /* 现有 list 逻辑 */ }
        "install" => {
            let Some(arg) = arg else { return Err(anyhow!("usage: plugin install <path|git-url>")); };
            /* 现有 install 分支全部逻辑(用 arg 替代 args.get(1)) */
        }
        "update" => { /* 现有 update 逻辑(arg 可选) */ }
        "pin" | "unpin" => {
            let Some(name) = arg else { return Err(anyhow!("usage: plugin {op} <name>")); };
            /* 现有 pin/unpin 逻辑 */
        }
        "remove" => { /* 现有 remove 逻辑(arg 必填) */ }
        "status" => { /* 现有 status 逻辑 */ }
        other => return Err(anyhow!("unknown plugin subcommand '{other}'")),
    }
    Ok(())
}

/// :plugin 家族命令入口(Validate 后薄壳)
fn plugin(cx: &mut compositor::Context, args: Args, event: PromptEvent) -> anyhow::Result<()> {
    if event != PromptEvent::Validate { return Ok(()); }
    let Some(sub) = args.first() else {
        return Err(anyhow!("usage: plugin <list|install|remove|update|pin|unpin|status>"));
    };
    plugin_op(cx, sub, args.get(1).map(|s| s.as_str()))
}
```

apply_ui_requests 加分支(OpenPicker 旁):

```rust
helix_js::UiRequest::PluginOp { op, arg } => {
    plugin_op(cx, &op, arg.as_deref())?;
}
```

- [ ] **步骤 4:测试转绿**

运行:`cargo test -p helix-term --features integration --test integration plugin_manager 2>&1 | tail -10` + `cargo test -p helix-term --lib plugin_manager` + `cargo test -p helix-js plugin_api` + `cargo build -p helix-term`
预期:PASS。

- [ ] **步骤 5:文档 + Commit**

plugin-api.md 加 3 方法一节(install/update/remove,arg 语义,结果走状态栏)。

```bash
git add helix-term/src/commands/typed.rs helix-term/tests/test/plugin_manager.rs docs/plugin-api.md
git commit -m "feat(term): plugin_op 提取共用 + UiRequest::PluginOp 接线(JS API 生效)+ plugin-api 文档 + integration"
```

---

## 收尾

- [ ] `cargo fmt --all --check`(本批次文件)+ `cargo clippy -p helix-js -p helix-term 2>&1 | tail -3` + `cargo test -p helix-js` + `cargo test -p helix-term --lib`
- [ ] 手动验证(用户):init.js 加 `helix.plugin.remove("nope");` → 启动后状态栏报 not installed;或临时插件调用 install 本地路径
- [ ] 交接文档 `docs/superpowers/handoff/2026-08-31-js-plugin-api.md`(简短 + 二期全部完成总结)
- [ ] Commit 交接
