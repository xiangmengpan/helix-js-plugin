# 样式 API 实现计划（v9-②）

> 自主执行。TDD → 通过 → 提交；控制者自动合并；失败自动重试。

### 任务 1：StyledLine 渲染

**文件：** `helix-js/src/lib.rs`、`helix-term/src/ui/plugin_popup.rs`

- [ ] **步骤 1：失败单测**（helix-js tests）

```rust
#[test]
fn popup_styled_lines() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.open_popup({
            render: () => [
                { text: "err: ", style: "error" },
                "plain",
                { text: "warn" },
            ],
        });
        "#,
    )
    .unwrap();
    let id = match take_ui_requests()[0] { UiRequest::OpenPopup { id, .. } => id };
    let lines = render_popup(id, 40, 10).unwrap();
    assert_eq!(
        lines,
        vec![
            StyledLine { text: "err: ".into(), style: Some("error".into()) },
            StyledLine { text: "plain".into(), style: None },
            StyledLine { text: "warn".into(), style: None },
        ]
    );
    close_popup(id).unwrap();
    // 非法元素（缺 text / 非字符串非对象）→ Err
    load_script(r#"helix.open_popup({ render: () => [{ style: "error" }] });"#).unwrap();
    let id = match take_ui_requests()[0] { UiRequest::OpenPopup { id, .. } => id };
    assert!(render_popup(id, 40, 10).is_err());
    close_popup(id).unwrap();
}
```

> 注意：`render_popup` 返回类型改 `Vec<StyledLine>` 后，既有 `popup_lifecycle`/`popup_size_position` 等测试的 `assert_eq!(lines, vec!["a","b","c"])` 需适配（`StyledLine{text, style:None}`）。`PopupCallbacks` 等不受影响。

- [ ] **步骤 2：运行失败** → 编译错误（StyledLine/返回类型）。
- [ ] **步骤 3：实现**
  - `pub struct StyledLine { pub text: String, pub style: Option<String> }`
  - `render_popup` 解析：数组元素 → `value.is_object()` 且含 "text" 属性 → 读 text(String) + style(Option<String>)；字符串 → (text, None)；否则 Err
  - JS 对象属性读取用 `obj.get("text", engine)` / `obj.get("style", engine)`（is_null_or_undefined → None）
- [ ] **步骤 4：helix-js 通过**（21 个，clippy 0）
- [ ] **步骤 5：helix-term 渲染 Span 化**
  - `PluginPopup::render`：`let spans: Vec<Span> = lines.iter().map(|l| match &l.style { Some(s) => Span::styled(l.text.clone(), theme.get(s)), None => Span::raw(l.text.clone()) })`——`theme.get(name)` 对未知 key 的行为以 helix Theme API 为准（返回默认/空 Style 即可，不 panic）
  - `required_size` 仍按 text 宽度
- [ ] **步骤 6：集成测试**（tests/test/plugin_popup_style.rs，临时 mod）——插件开弹窗 render 返回 `{text:"ERR", style:"error"}` → 渲染 PluginPopup 到独立 Buffer（仿 bufferline 测试：构造 PluginPopup::new(id) + render 进 Buffer）→ 断言该行 cell 的 Style 等于 `theme.get("error")`。若主题访问在测试里路径复杂，退化为断言渲染文本存在 + 单测覆盖 style 解析，报告中注明
- [ ] **步骤 7：回归**（plugin 组、command_line、cargo check/clippy）
- [ ] **步骤 8：Commit** `feat(js): styled lines in popup render`

---

## 自检
- 覆盖：解析/缺省/非法/渲染样式解析。
- 风险：render_popup 返回类型变化波及全部弹窗测试（机械适配）；theme.get 的未知 key 行为；集成测试的主题路径。
