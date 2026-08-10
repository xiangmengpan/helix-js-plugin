# 面板交互输入实现计划（v10-①）

> 自主执行。TDD → 通过 → 提交；控制者自动合并（并行 wave，合并冲突由控制者解决）。

### 任务 1：面板 onKey

**文件：** `helix-js/src/lib.rs`、`helix-term/src/ui/plugin_panel.rs`

- [ ] **步骤 1：失败单测**（helix-js tests）

```rust
#[test]
fn panel_onkey() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        const pid = helix.open_panel({
            side: "right", size: 20,
            render: () => ["p"],
            onKey: (key) => { helix.echo("panel-key:" + key.name); return key.name === "Esc" ? "close" : "handled"; },
        });
        helix.echo("pid:" + pid);
        "#,
    )
    .unwrap();
    let reqs = take_ui_requests();
    assert!(matches!(&reqs[0], UiRequest::OpenPanel { id, .. }));
    let id = match &reqs[0] { UiRequest::OpenPanel { id, .. } => *id };
    assert!(take_messages()[0].starts_with("pid:"));
    // popup_key 走同一注册表：Esc → close，其他 → handled
    let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
    let esc = PluginKey { name: "Esc".into(), shift: false, ctrl: false, alt: false };
    assert_eq!(popup_key(id, &esc, &ctx).unwrap(), PopupKeyResult::Close);
    assert_eq!(take_messages(), vec!["panel-key:Esc"]);
    // 无 onKey 的面板：popup_key 缺省语义不适用（面板缺省全 Ignore）——在 helix-term 侧保证（无 on_key 时 PluginPanel 不调 popup_key 直接 Ignored）
}
```

- [ ] **步骤 2：运行失败** → 编译错误或断言失败。
- [ ] **步骤 3：实现 helix-js**：`js_open_panel` 增加可选 onKey 解析（`opts.get("onKey")`，函数校验）→ PopupCallbacks.on_key 填入
- [ ] **步骤 4：helix-js 通过**（24 个，clippy 0）
- [ ] **步骤 5：helix-term**：`PluginPanel::handle_event`——`let has_key = helix_js::panel_has_onkey(self.id)`（新辅助：查注册表 on_key 是否 Some，避免无回调时构建 ctx）；有 → 构建 CommandContext + popup_key（同 PluginPopup 的接线，含 drain edits/messages/cursor）→ Close 时移除层 + close_popup；无 → `Ignored`
  - 面板关闭路径：Close → `dispatch_blocking` 移除层（静态 id）+ close_popup(id)
- [ ] **步骤 6：集成测试**（tests/test/plugin_panel_input.rs，临时 mod）：面板带 onKey（任意键 echo）→ 按 `d` → 状态栏 "panel-key:d"；按 Esc → 面板关闭（has_component false）；无 onKey 面板 → 按键穿透（编辑器可编辑）
- [ ] **步骤 7：回归**（plugin 组、command_line、cargo check/clippy）
- [ ] **步骤 8：Commit** `feat(js): interactive panel keys (onKey)`

---

## 自检
- 风险：PluginPanel 与 PluginPopup 的 handle_event 逻辑相似——复用（提取公共辅助或复制少量）；panel_has_onkey 新辅助；缺省语义（无 onKey → 全 Ignore，不自动关）。
