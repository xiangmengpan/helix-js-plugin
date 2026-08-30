# 面板节点焦点路由实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** `open_panel` 加 `focusable` 开关,启用后面板获得与弹窗相同的节点焦点路由(Tab 移动焦点、焦点节点按键直达、Esc 取消),未启用零行为变化。

**架构:** 开关经 `UiRequest::OpenPanel` 传递到 `PluginPanel`;term 侧加 `focusables`/`focus` 字段,render 时经 `focusable_node_ids` 提取(复用弹窗机制);handle_event 加焦点路由分支(仿 plugin_popup.rs 方案乙)。

**技术栈:** Rust(helix-js boa 运行时 + helix-term)、tokio 集成测试。

**规格:** `docs/superpowers/specs/2026-08-30-js-panel-focus-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-js/src/types.rs` | `UiRequest::OpenPanel` 加 `focusable: bool` |
| `helix-js/src/popup.rs` | `js_open_panel` 解析 `focusable`(默认 false) |
| `helix-term/src/commands/typed.rs` | OpenPanel 构造传递 focusable |
| `helix-term/src/ui/plugin_panel.rs` | `PluginPanel::new` 加参数、`focusables`/`focus` 字段、refresh 提取、handle_event 焦点分支 |
| `helix-term/tests/integration.rs` | 注册 `mod plugin_panel_focus;`(或并入 plugin_panel) |
| `helix-term/tests/test/plugin_panel_focus.rs`(新) | 焦点路由集成测试 |
| `docs/plugin-api.md` | open_panel 文档加 focusable |

关键实现细节(执行时必读):

- **现有模式(必读参考)**:`helix-term/src/ui/plugin_popup.rs` 的字段(21-23)、`refresh` 提取(49-60)、`handle_event` 焦点分支(74-130)——面板完全镜像;`helix-js/src/commands.rs:1580` `focusable_node_ids` 已存在;`helix-js/src/popup.rs:170` `js_open_panel`(focusable 解析仿 side/size 的 opts.get + try_js_into + 默认值)。
- **面板 handle_event 现状**(plugin_panel.rs:85):无 onKey → Ignore(穿透);有 onKey → 构建 ctx → popup_key → 应用编辑/光标/消息。焦点分支放最前(focusable && focusables 非空时),否则走现状。**focusable 面板无 onKey 也能用焦点路由**(焦点分支独立于 onKey)。
- **焦点分支的 drain**:焦点节点事件(dispatch_node_event/dispatch_input_key)之后,需要与弹窗相同的 drain(消息 + UI 请求)——plugin_popup.rs 的 `drain_msgs` 闭包;面板的既有 drain 在 popup_key 之后(编辑/光标/消息/UI 请求),焦点分支可复用弹窗的 drain 形态(消息 + UI 请求;编辑/光标由节点事件入队后同样需要应用——**参照弹窗焦点分支是否应用编辑/光标,执行时按弹窗为准**)。
- **render 提取**:plugin_panel.rs:179 的 render 调 `render_popup(self.id, w, h, None)` → 改为传 `self.focus.as_deref()` + 仿弹窗 refresh 提取 focusables(面板的 render 方法内直接做,或抽 refresh——执行时按现有结构最小改动)。
- **焦点失效重置**:focusables 变化后 focus 不在列表 → 清空(render 后检查,仿弹窗行为——弹窗是否有此逻辑,按实际)。
- **测试命令**:`cargo build -p helix-term`(实现者);`cargo test -p helix-term --features integration --test integration plugin_panel_focus` + `plugin_panel`(控制者);clippy 零;fmt clean。

---

### 任务 1:开关与字段落地(未启用零变化)

**文件:** types.rs、popup.rs、typed.rs、plugin_panel.rs

- [ ] **步骤 1:开关传递链**

1. `helix-js/src/types.rs`:`UiRequest::OpenPanel { id, side, size, focusable: bool }`
2. `helix-js/src/popup.rs` `js_open_panel`(~240):解析 focusable(仿 side 模式,缺省 false):

```rust
let focusable = opts
    .get(JsString::from("focusable"), ctx)?
    .try_js_into::<bool>()
    .map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from(
            "open_panel: 'focusable' must be a boolean",
        )))
    })?;
```

   push 改 `UiRequest::OpenPanel { id, side, size, focusable }`。注意:`opts.get` 对缺省 key 返回 undefined——`try_js_into::<bool>` 对 undefined 的行为需确认(若报错则改缺省 false 模式:`match opts.get(...) { v if v.is_undefined() => false, v => v.try_js_into... }`,执行时按实际)。
3. `helix-term/src/commands/typed.rs`(~4771):`OpenPanel { id, side, size, focusable }` 解构 → `PluginPanel::new(id, side, focusable)`
4. `helix-term/src/ui/plugin_panel.rs`:
   - 结构体加字段(仿 plugin_popup.rs:21-23):

```rust
/// focusable 面板的节点焦点路由状态(仿 PluginPopup)
focusables: Vec<String>,
focus: Option<String>,
```

   - `new(id, side, focusable)` 加参,存 `focusable: bool` 字段,初始化空 focusables/focus
   - render(~179):`render_popup(self.id, w, h, self.focus.as_deref())` + 仿弹窗 refresh 提取 focusables(focusable 面板才提取;未启用保持现状——提取开销可忽略但语义上仅 focusable 需要)

- [ ] **步骤 2:构建 + 回归验证**

```bash
cargo build -p helix-term
cargo test -p helix-term --features integration --test integration plugin_panel   # 控制者:既有面板测试不回归
```

预期:既有 plugin_panel/plugin_popup 测试全绿(未启用面板零行为变化)。

- [ ] **步骤 3:提交**

```bash
git add helix-js helix-term/src
git commit -m "feat(js,term): open_panel focusable 开关 + PluginPanel 焦点字段/focusables 提取(未启用零变化)"
```

---

### 任务 2:焦点路由分支 + 测试

**文件:** plugin_panel.rs(handle_event)、plugin_panel_focus.rs(新)、integration.rs、plugin-api.md

- [ ] **步骤 1:handle_event 焦点路由分支**

`plugin_panel.rs` `handle_event`(~85)最前插入(仿 plugin_popup.rs:74-130):

```rust
// 节点焦点路由(focusable 面板启用):Tab 移动焦点;焦点在节点时按键直达;
// Esc 取消焦点回 onKey;无焦点时走 onKey(现状)。未启用 → 走下方既有逻辑。
if self.focusable && !self.focusables.is_empty() {
    if key.name == "Tab" {
        let idx = self.focusables.iter().position(|f| Some(f) == self.focus.as_ref());
        let next = idx.map(|i| (i + 1) % self.focusables.len()).unwrap_or(0);
        self.focus = Some(self.focusables[next].clone());
        return EventResult::Consumed(None);
    }
    if let Some(fid) = self.focus.as_ref() {
        // 复用弹窗的节点事件分发(dispatch_node_event / dispatch_input_key)
        // + drain(消息/UI 请求;编辑/光标按弹窗焦点分支的实际处理)
        match key.name.as_str() {
            "Esc" => {
                self.focus = None;
                return EventResult::Consumed(None);
            }
            "Enter" => { /* 仿弹窗:input 有状态 → onKey("Enter");否则 → onPress */ }
            "Left" | "Right" | "Home" | "End" => { /* input 光标移动 */ }
            "Up" | "Down" => { /* input 候选导航(走 onKey 形态) */ }
            _ => { /* 字符 → input 插入(dispatch_input_key);否则 → 走 onKey */ }
        }
    }
}
```

   **执行时严格对照 plugin_popup.rs:74-130 的每个分支语义**(Enter/方向键/字符/Backspace/Delete 的处理与 drain),差异仅在:Esc → 取消焦点(非关闭——面板语义)。**若弹窗对 Esc 有特殊处理(如关闭),面板分支写清差异**。

- [ ] **步骤 2:写失败集成测试(6 条)**

`integration.rs` 加 `mod plugin_panel_focus;`;新建 `helix-term/tests/test/plugin_panel_focus.rs`(仿既有 plugin_panel.rs 测试形态,如 panel 的渲染/按键断言;节点树用 `helix.el` 构造 button/input——参照既有测试的节点用法):

1. `focusable_panel_tab_focuses_first_node`:open_panel({focusable:true, render: () => ({type:"column", children:[{type:"button", id:"b1", ...}, {type:"input", id:"i1", ...}]})}) → Tab → render 收到 focus="b1"(白盒:JS render 里 echo focus 或存状态断言)
2. `focusable_panel_button_enter_triggers_onpress`:Tab 聚焦 button → Enter → onPress 回调触发(消息/状态断言)
3. `focusable_panel_input_insert`:Tab 两次到 input → 字符键 → input 值变化(onChange/白盒 value 断言)
4. `focusable_panel_esc_cancels_focus`:聚焦后 Esc → 焦点清空 → 后续按键回 onKey(断言 onKey 收到键)
5. `non_focusable_panel_tab_still_onkey`:未启用 focusable 的面板 Tab 走 onKey(断言 onKey 收到 Tab;不回归)
6. `focusable_panel_focus_reset_on_node_change`:render 后节点消失 → 焦点清空 → Tab 从头(断言)

   **测试技巧**:面板按键断言参照既有 plugin_panel.rs 测试(按键序列 + 状态/消息断言);白盒断言(JS 侧存状态 + 读回)参照既有节点测试模式。若 6 条中有难以白盒断言的(如 render focus 参数),用 JS echo 到状态栏断言。

- [ ] **步骤 3:转绿 + 文档 + 提交**

```bash
cargo build -p helix-term
cargo test -p helix-term --features integration --test integration plugin_panel_focus   # 控制者
cargo test -p helix-term --features integration --test integration plugin_panel        # 控制者:回归
cargo test -p helix-js                                                                 # 控制者
cargo clippy --all-targets 2>&1 | tail -3
cargo fmt --all --check
```

`docs/plugin-api.md` open_panel 小节加 `focusable` 说明(默认 false;启用后 Tab 节点焦点/Esc 取消/节点按键直达)。

```bash
git add helix-term docs/plugin-api.md
git commit -m "feat(term): 面板节点焦点路由(focusable)——Tab 移动/Esc 取消/节点按键直达 + 集成测试"
```

---

## 自检记录

**规格覆盖度:**
- 2.1 API(focusable 参数)→ 任务 1 步骤 1 ✓
- 2.2 语义(Tab/Esc/节点直达/焦点失效/未启用零变化)→ 任务 2 步骤 1-2 ✓
- 2.3 数据流(开关传递/字段/提取)→ 任务 1 步骤 1 ✓
- 2.4 边界(Esc 不关闭/弹窗不受影响/无 focusables 走 onKey)→ 任务 2 步骤 1-2 ✓
- 3.1 测试 6 条 → 任务 2 步骤 2 ✓
- 3.2 回归(既有 panel/popup 测试)→ 任务 1 步骤 2、任务 2 步骤 3 ✓

**占位符扫描:** 无 TODO/待定;焦点分支代码为骨架,标注"执行时严格对照 plugin_popup.rs 分支语义"。✓

**类型一致性:** `focusable: bool` 在 UiRequest/PluginPanel::new/js_open_panel 三处一致;`focusables`/`focus` 字段与弹窗同名同型。✓
