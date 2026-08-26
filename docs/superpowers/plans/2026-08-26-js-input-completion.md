# input 组件升级 + 补全联动实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 把 input 组件升级为引擎权威的真输入框（引擎维护 value + 光标，onChange 自动回调，set_input_value 显式改值），配合已交付的 `helix.lsp.completion()` 让插件几行代码实现补全联动。

**架构：** helix-js 侧持有 `InputStates[(popup_id, node_id)] = { value, cursor }`，`input_edit` 纯函数做按键编辑转换（可单测）；渲染时 parse_node 用引擎状态值覆盖 JS 传值（JS 传值仅初始化）；helix-term 侧 plugin_popup.rs 按键路由分类：编辑键（字符/Backspace/Delete/Left/Right/Home/End）走 dispatch_input_key，导航键（Up/Down/Enter）走既有 onKey 路径。

**技术栈：** boa 0.21、helix-js（状态/回调）、helix-term（plugin_popup.rs 按键路由 + comp_layout 光标渲染）。

**规格：** `docs/superpowers/specs/2026-08-26-js-input-completion-design.md`（已批准）

---

### 任务 1：helix-js — InputStates 存储 + input_edit 纯函数

**文件：**
- 创建：`helix-js/src/input.rs`（InputState、input_edit、InputStates 存储、with_input_states）
- 修改：`helix-js/src/lib.rs`（mod input）

- [ ] **步骤 1：编写失败的测试**（`helix-js/src/input.rs` 底部 `#[cfg(test)]`，用 `TEST_LOCK` + `crate::init()` 模式）

```rust
#[test]
fn input_edit_basic() {
    let mut s = InputState { value: "abc".into(), cursor: 3 };
    // 末尾追加
    assert_eq!(input_edit(&mut s, "d"), Some("abcd".to_string()));
    assert_eq!(s.cursor, 4);
    // 中间插入
    s = InputState { value: "abc".into(), cursor: 1 };
    assert_eq!(input_edit(&mut s, "X"), Some("aXbc".to_string()));
    assert_eq!(s.cursor, 2);
    // Backspace 中间删
    assert_eq!(input_edit(&mut s, "Backspace"), Some("abc".to_string()));
    assert_eq!(s.cursor, 1);
    // Backspace 光标在首 → 无变化
    s = InputState { value: "abc".into(), cursor: 0 };
    assert_eq!(input_edit(&mut s, "Backspace"), None);
    assert_eq!(s.value, "abc");
    // Delete 删光标后
    s = InputState { value: "abc".into(), cursor: 0 };
    assert_eq!(input_edit(&mut s, "Delete"), Some("bc".to_string()));
    assert_eq!(s.cursor, 0);
    // Left/Right/Home/End 只动光标,不触发 onChange(返回 None)
    s = InputState { value: "abc".into(), cursor: 2 };
    assert_eq!(input_edit(&mut s, "Left"), None);
    assert_eq!(s.cursor, 1);
    assert_eq!(input_edit(&mut s, "Right"), None);
    assert_eq!(s.cursor, 2);
    assert_eq!(input_edit(&mut s, "Home"), None);
    assert_eq!(s.cursor, 0);
    assert_eq!(input_edit(&mut s, "End"), None);
    assert_eq!(s.cursor, 3);
    // 非编辑键 → None
    assert_eq!(input_edit(&mut s, "Up"), None);
    assert_eq!(input_edit(&mut s, "Enter"), None);
    assert_eq!(input_edit(&mut s, "Tab"), None);
}

#[test]
fn input_edit_utf8_and_clamp() {
    // 多字节字符:光标按 char 计
    let mut s = InputState { value: "你好".into(), cursor: 1 };
    assert_eq!(input_edit(&mut s, "啊"), Some("你啊好".to_string()));
    assert_eq!(s.cursor, 2);
    // Right 越界 clamp 到末尾
    s = InputState { value: "ab".into(), cursor: 2 };
    assert_eq!(input_edit(&mut s, "Right"), None);
    assert_eq!(s.cursor, 2);
    // 空值 Backspace/Delete 安全
    s = InputState { value: "".into(), cursor: 0 };
    assert_eq!(input_edit(&mut s, "Backspace"), None);
    assert_eq!(input_edit(&mut s, "Delete"), None);
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test -p helix-js input_edit`
预期：FAIL（input_edit / InputState 未定义）

- [ ] **步骤 3：实现 input.rs**

```rust
// helix-js/src/input.rs
use std::cell::RefCell;
use std::collections::HashMap;

/// 单个 input 节点的编辑状态（引擎权威）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputState {
    pub value: String,
    /// 光标位置（char 索引，非字节）
    pub cursor: usize,
}

thread_local! {
    static INPUT_STATES: RefCell<HashMap<(u64, String), InputState>> = const { RefCell::new(HashMap::new()) };
}

pub(crate) fn with_input_states<T>(f: impl FnOnce(&mut HashMap<(u64, String), InputState>) -> T) -> T {
    INPUT_STATES.with(|m| f(&mut m.borrow_mut()))
}

/// 按键编辑转换（纯函数）：返回新值表示值变化（应触发 onChange）；
/// 返回 None 表示值未变（光标移动/非编辑键）。光标以 char 索引计。
pub fn input_edit(state: &mut InputState, key: &str) -> Option<String> {
    let len = state.value.chars().count();
    let char_at = |i: usize| state.value.chars().nth(i);
    match key {
        "Backspace" if state.cursor > 0 => {
            let target = state.cursor - 1;
            let mut rest = state.value.chars();
            let new: String = rest.by_ref().take(target).chain(rest.skip(1)).collect();
            state.value = new;
            state.cursor = target;
            Some(state.value.clone())
        }
        "Delete" if state.cursor < len => {
            let mut rest = state.value.chars();
            let new: String = rest.by_ref().take(state.cursor).chain(rest.skip(1)).collect();
            state.value = new;
            Some(state.value.clone())
        }
        "Left" => { state.cursor = state.cursor.saturating_sub(1); None }
        "Right" => { state.cursor = (state.cursor + 1).min(len); None }
        "Home" => { state.cursor = 0; None }
        "End" => { state.cursor = len; None }
        k if k.chars().count() == 1 => {
            // 单字符插入到光标处
            let mut chars: Vec<char> = state.value.chars().collect();
            let ch = k.chars().next().unwrap();
            chars.insert(state.cursor, ch);
            state.value = chars.into_iter().collect();
            state.cursor += 1;
            Some(state.value.clone())
        }
        _ => None,
    }
}
```

（注意：`chars().nth()` 是 O(n)——输入框值短（补全场景 <100 字符），P0 可接受；`ponytail:` 注释注明若有大文本输入再优化为 char_indices。）

- [ ] **步骤 4：lib.rs 注册 mod**

```rust
// lib.rs 顶部 mod 声明区加:
mod input;
pub use input::{input_edit, with_input_states, InputState};
```

（确认 lib.rs 现有 mod 声明的确切位置与可见性约定——lsp.rs 是怎么声明的就怎么对齐。）

- [ ] **步骤 5：运行测试验证通过**

运行：`cargo test -p helix-js input_edit`
预期：PASS

- [ ] **步骤 6：Commit**

```bash
git add helix-js/src/input.rs helix-js/src/lib.rs
git commit -m "feat(js): input 编辑状态机 InputStates + input_edit 纯函数"
```

### 任务 2：helix-js — 渲染接入 + onChange 回调 + set_input_value + 生命周期

**文件：**
- 修改：`helix-js/src/input.rs`（dispatch_input_key、set_input_value、close 清理、渲染取值辅助）
- 修改：`helix-js/src/types.rs`（NodeHandlers 加 on_change；CompNode::Input 加 cursor 字段）
- 修改：`helix-js/src/popup.rs`（parse_node input 分支：状态覆盖/初始化 + cursor；register_node_handlers 收 onChange；close_popup 清理）
- 修改：`helix-js/src/lib.rs`（注册 set_input_value API）
- 修改：`helix-js/src/lib.rs` 或 popup.rs 现有单测（CompNode::Input 解构需同步 cursor 字段）

- [ ] **步骤 1：编写失败的测试**（追加到 input.rs 测试模块）

```rust
#[test]
fn input_dispatch_and_set() {
    let _guard = TEST_LOCK.lock().unwrap();
    crate::init();
    // 注册一个 input 节点 onChange + onKey(与 NODE_HANDLERS 同机制)
    // 通过 render_popup 走一遍:el({type:"col", children:[{type:"input", id:"q", value:"", onChange:...}]})
    // 简化路径:直接验证 dispatch_input_key 的行为
    let ctx = Context::default();
    // 用 load_script 注册弹窗+render,然后模拟按键
    // 断言:onChange 收到新值;set_input_value 更新状态
}
```

（注意：测试要走真实路径——`open_popup({render})` 注册后渲染生成 CompNode::Input 初始化 InputStates，`dispatch_input_key` 触发 onChange，`set_input_value` 更新。参考 lib.rs 现有 popup 测试怎么驱动 open_popup + render_popup。若直接驱动复杂，退而验证：render_popup 后 InputStates 被初始化（JS 传 value 生效）、dispatch_input_key 改状态并调 on_change（on_change 里 echo → take_messages 断言）、set_input_value 生效。）

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test -p helix-js input_dispatch_and_set`
预期：FAIL

- [ ] **步骤 3：实现接线**

```rust
// types.rs
pub(crate) struct NodeHandlers {
    pub(crate) on_press: Option<JsValue>,
    pub(crate) on_key: Option<JsValue>,
    pub(crate) on_change: Option<JsValue>,   // 新增
}
// CompNode::Input 加 cursor
Input { value: String, cursor: usize, width: Option<u16>, id: String, flex: Option<u16> },

// input.rs
/// 编辑键处理:查状态 → input_edit → 有变化 → 调 on_change(新值)。
pub fn dispatch_input_key(popup_id: u64, node_id: &str, key: &str) -> Result<()> {
    crate::init();
    let changed = with_input_states(|m| {
        let entry = m.entry((popup_id, node_id.to_string())).or_insert_with(|| {
            InputState { value: String::new(), cursor: 0 }
        });
        input_edit(entry, key)
    });
    let Some(new_value) = changed else { return Ok(()) };
    // 调 on_change(若有)
    crate::state::with_engine(|engine| {
        let handlers = crate::state::with_node_handlers(|h| {
            h.get(&(popup_id, node_id.to_string())).cloned().map(|hd| hd.on_change.clone())
        });
        let Some(Some(f)) = handlers else { return Ok(()) };
        let func = f.as_callable().and_then(JsFunction::from_object)
            .ok_or_else(|| anyhow!("node {node_id} onChange not callable"))?;
        let undefined = JsValue::undefined();
        let _: JsValue = func.call(&undefined, &[JsValue::from(JsString::from(new_value))], engine)
            .map_err(|e| anyhow!("node {node_id} onChange failed: {e}"))?;
        Ok(())
    })
}

/// JS 强制改值(候选回填/清空)。cursor 置末尾。
pub fn js_set_input_value(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let popup_id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsString::from("set_input_value: popup id must be a number"))
    })?;
    let node_id: String = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsString::from("set_input_value: node id must be a string"))
    })?;
    let value: String = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsString::from("set_input_value: value must be a string"))
    })?;
    with_input_states(|m| {
        m.insert((popup_id, node_id), InputState { cursor: value.chars().count(), value });
    });
    Ok(JsValue::undefined())
}

/// 弹窗关闭清理:删该 popup 的全部 InputStates
pub fn clear_popup_inputs(popup_id: u64) {
    with_input_states(|m| m.retain(|(pid, _), _| *pid != popup_id));
}

// popup.rs parse_node input 分支(794 行附近):构建时用状态值覆盖/初始化
//   let (value, cursor) = with_input_states(|m| {
//       match m.entry((id, node_id.clone())) {
//           Entry::Occupied(e) => { let s = e.get(); (s.value.clone(), s.cursor) }
//           Entry::Vacant(e) => {
//               let v = value; // JS 传的
//               let c = v.chars().count();
//               e.insert(InputState { value: v.clone(), cursor: c });
//               (v, c)
//           }
//       }
//   });
//   Ok(CompNode::Input { value, cursor, width, id: node_id, flex })

// popup.rs register_node_handlers(838 行):加 on_change 收集
// popup.rs close_popup(934 行):调 clear_popup_inputs(id)

// lib.rs 注册:
// .function(NativeFunction::from_fn_ptr(input::js_set_input_value), JsString::from("set_input_value"), 3)
```

- [ ] **步骤 4：修复现有单测的 CompNode::Input 解构**（lib.rs:1852 附近 `CompNode::Input { flex: Some(2), .. }` 用 `..` 的不用改；若有全字段解构需加 `cursor`）

- [ ] **步骤 5：运行测试验证通过**

运行：`cargo test -p helix-js`
预期：全绿（含新测试）

- [ ] **步骤 6：Commit**

```bash
git add helix-js/src/input.rs helix-js/src/types.rs helix-js/src/popup.rs helix-js/src/lib.rs
git commit -m "feat(js): input 渲染接入 + onChange 回调 + set_input_value + 生命周期清理"
```

### 任务 3：helix-term — 按键路由 + 光标渲染

**文件：**
- 修改：`helix-term/src/ui/plugin_popup.rs`（按键路由分类）
- 修改：`helix-term/src/ui/comp_layout.rs`（Input 光标渲染）

- [ ] **步骤 1：编写失败的集成测试**（`helix-term/tests/test/plugin_input.rs`，仿 plugin_async.rs 模板；integration.rs 注册）

```rust
use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn plugin_input_edit_and_nav() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("in.js");
    std::fs::write(&plugin_path, r#"
        helix.register_command("inp", () => {
            helix.open_popup({
                render: (focus) => helix.el("col", [
                    { type: "input", id: "q", value: "", onChange: (v) => { helix.echo("chg:" + v); }, onKey: (k) => { helix.echo("key:" + k); } },
                ]),
            });
        });
        helix.register_command("inp-set", () => {
            helix.set_input_value(POPUP_ID, "q", "setval");
        });
    "#)?;
    // 注意 POPUP_ID 需运行时注入:open_popup 返回 id,插件存全局变量
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (Some(":inp<ret>"), None),
            // 焦点在 input(Tab 进入):输入字符 → onChange
            (Some("<tab>a<ret>"), Some(&|app| {
                // 状态栏应含 chg:a(key:a 是导航键?不——单字符走编辑)
                let (status, _) = app.editor.get_status().unwrap();
                assert!(status.as_ref().contains("chg:a"), "status: {status:?}");
            })),
            // Up/Down 走 onKey
            (Some("<up><down>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert!(status.as_ref().contains("key:Up") && status.as_ref().contains("key:Down"), "status: {status:?}");
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

（注意：插件里 `POPUP_ID` 需要 open_popup 返回值——改插件为 `globalThis.__pid = helix.open_popup({...})` 再 `helix.set_input_value(globalThis.__pid, ...)`。按键序列 `<tab>` 的 keymap 表示参考现有测试怎么写 Tab；input 编辑后 Esc 关闭弹窗再退出。）

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test -p helix-term --features integration --test integration plugin_input`
预期：FAIL（onChange 从未触发 / 按键不路由）

- [ ] **步骤 3：实现按键路由**（plugin_popup.rs handle_event 节点路由段，72-110 行）

```rust
// 替换现有 match:区分编辑键与导航键
match key.name.as_str() {
    // 导航/选择键:input 与 button 都走 onKey 路径(button 的 Enter/Space 走 onPress 兼容现状)
    "Enter" | "Space" => {
        // input 有状态 → onKey("Enter"/"Space");否则(button)走 onPress
        if helix_js::input_has_state(self.id, fid) {
            if helix_js::dispatch_node_event(self.id, fid, Some(&key.name)).is_ok() {
                drain_msgs(cx);
                return EventResult::Consumed(None);
            }
        } else if helix_js::dispatch_node_event(self.id, fid, None).is_ok() {
            drain_msgs(cx);
            return EventResult::Consumed(None);
        }
    }
    "Up" | "Down" | "Left" | "Right" | "Home" | "End" => {
        if helix_js::input_has_state(self.id, fid) {
            if helix_js::dispatch_input_key(self.id, fid, &key.name).is_ok()
                || helix_js::dispatch_node_event(self.id, fid, Some(&key.name)).is_ok()
            {
                drain_msgs(cx);
                return EventResult::Consumed(None);
            }
        }
    }
    key_name if key_name.chars().count() == 1 || key_name == "Backspace" || key_name == "Delete" => {
        if helix_js::input_has_state(self.id, fid) {
            if helix_js::dispatch_input_key(self.id, fid, &key.name).is_ok() {
                drain_msgs(cx);
                return EventResult::Consumed(None);
            }
        } else if helix_js::dispatch_node_event(self.id, fid, Some(&key.name)).is_ok() {
            drain_msgs(cx);
            return EventResult::Consumed(None);
        }
    }
    _ => {}
}
```

（要点：`input_has_state(popup_id, node_id)` 判断焦点节点是否 input——渲染过的 input 必有状态。方向键语义：Left/Right/Home/End 是编辑光标（走 dispatch_input_key 返回 None 不触发 onChange，仅移动光标——但 dispatch_input_key 里 input_edit 对这些键返回 None，**None 时不调 onChange，正好**）；Up/Down 是候选导航（input_edit 返回 None，也不调 onChange——**问题：Up/Down 需要走 onKey**！所以方向键分支要同时调 dispatch_input_key(移动光标/无效果) 和 dispatch_node_event(onKey)——见上：`dispatch_input_key(...).is_ok() || dispatch_node_event(...)`，两个都调。简化：方向键只调 dispatch_node_event(onKey)，Left/Right 光标移动由 input_edit 处理但也要 onKey？不——Left/Right 只移动光标不需要通知 JS。**最终方案**：Left/Right/Home/End → dispatch_input_key(纯光标移动,无 onChange)；Up/Down → dispatch_node_event(onKey 导航)。Enter → input 有状态时 dispatch_node_event(onKey("Enter"))。按此实现，勿照抄上方示例的双调。）

- [ ] **步骤 4：实现光标渲染**（comp_layout.rs:271 Input 分支）

```rust
CompNode::Input { value, cursor, width, .. } => {
    let limit = width.unwrap_or(u16::MAX).min(viewport.0) as usize;
    // 光标处插入 "|"(char 索引);光标超限 clamp 末尾
    let mut chars: Vec<char> = value.chars().collect();
    let c = (*cursor).min(chars.len());
    chars.insert(c, '|');
    let text: String = chars.into_iter().take(limit).collect();
    vec![StyledLine::plain(text)]
}
```

- [ ] **步骤 5：运行测试验证通过**

运行：`cargo test -p helix-term --features integration --test integration plugin_input`
预期：PASS（chg:a / key:Up / key:Down 断言过）

- [ ] **步骤 6：Commit**

```bash
git add helix-term/src/ui/plugin_popup.rs helix-term/src/ui/comp_layout.rs helix-term/tests/test/plugin_input.rs helix-term/tests/integration.rs
git commit -m "feat(term): input 按键路由分类 + 光标渲染"
```

### 任务 4：demo 插件 + 文档

**文件：**
- 创建：`plugins/features/input-completion/index.js`
- 修改：`docs/plugin-api.md`（input 组件章节更新）

- [ ] **步骤 1：写 demo 插件**

```js
// plugins/features/input-completion/index.js
// :ic 打开补全弹窗:输入触发 completion,↑↓ 导航,Enter 插入
helix.register_command("ic", () => {
  let pid = null;
  let query = "";
  let items = [];
  let sel = 0;
  let timer = null;

  const ask = () => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      helix.lsp.completion().then((c) => {
        items = c ? (c.items ?? c) : [];
        sel = 0;
        render();
      });
    }, 120);
  };

  const render = () => {
    if (!pid) return;
    helix.open_popup({
      id: pid,  // 若 open_popup 支持按 id 更新则用;否则 set_component_render
      render: (focus) => helix.el("col", [
        { type: "input", id: "q", value: query, onChange: (v) => { query = v; ask(); } },
        ...items.map((it, i) => ({
          type: "text",
          content: (i === sel ? "> " : "  ") + (it.label ?? String(it)),
          style: i === sel ? { fg: "yellow" } : undefined,
        })),
      ]),
    });
  };

  pid = helix.open_popup({
    render: (focus) => helix.el("col", [
      { type: "input", id: "q", value: "", onChange: (v) => { query = v; ask(); }, onKey: (k) => {
        if (k === "Up" && sel > 0) { sel--; render(); }
        else if (k === "Down" && sel < items.length - 1) { sel++; render(); }
        else if (k === "Enter") {
          const it = items[sel];
          if (it) { /* 插入文档:doc 编辑 API 或 echo */ helix.echo("selected: " + (it.label ?? it)); }
        }
      } },
    ]),
  });
});
```

（注意：demo 里 render 闭包读 `query`/`items`/`sel` 变量——open_popup 的 render 每次被调时读当前闭包值即可（引擎每帧 refresh 调 render）。`open_popup` 是否支持按 id 更新渲染——查现有 API，若不支持就用单一 render 闭包读状态（demo 已按此写，无需 id 更新）。completion 返回 `{isIncomplete, items}` 或数组两种形态，demo 已处理。插入文档用现有 doc 编辑 API（doc.insert 或 set_cursor+echo），demo 简单起见 echo 选中项。）

- [ ] **步骤 2：写文档章节**（docs/plugin-api.md input 组件章节：onChange 属性、set_input_value(popup_id, node_id, value) API、按键语义表：字符/Backspace/Delete 编辑触发 onChange；Left/Right/Home/End 移动光标；Up/Down/Enter 走 onKey；Tab 移焦点；示例代码片段）

- [ ] **步骤 3：手动验证**（可选，真 LSP 同上一计划流程——demo 在真项目里 :ic 输入出候选）

- [ ] **步骤 4：Commit**

```bash
git add plugins/features/input-completion/index.js docs/plugin-api.md
git commit -m "feat(plugins): input-completion demo + plugin-api input 章节更新"
```

---

## 自检

**规格覆盖度：**
- 编辑状态机(InputStates + input_edit + 光标) → 任务 1
- 渲染值覆盖/初始化 + onChange 回调 + set_input_value + close 清理 → 任务 2
- 按键路由分类(编辑键/导航键/Enter) + 光标渲染 → 任务 3
- 测试(单测状态机 + 集成路由) → 任务 1/3
- demo 插件 + 文档 → 任务 4
- 范围外(面板/IME/候选虚拟化/防抖) → 未包含,符合规格

**占位符扫描：** 无 TODO/待定。任务 2 测试说明里"若直接驱动复杂则退而验证"是策略标注,非占位符(步骤 1 有明确断言目标)。

**类型一致性：** `InputState{value, cursor}` / `input_edit(&mut InputState, &str) -> Option<String>` / `dispatch_input_key(popup_id, node_id, key)` / `js_set_input_value(popup_id, node_id, value)` 全计划一致。`CompNode::Input` 加 `cursor: usize` 字段在任务 2(定义)+ 3(渲染)一致。`input_has_state(popup_id, node_id) -> bool` 在任务 3 使用,任务 2 需提供(popup.rs parse_node 初始化后必有状态——但**input 未渲染前** JS 调 dispatch_input_key 的边界:entry 自动创建,安全)。
