# 侧边面板实现计划（v9-③）

> 自主执行。TDD → 通过 → 提交；控制者自动合并；失败自动重试。

### 任务 1：PluginPanel 组件 + API

**文件：** `helix-js/src/lib.rs`、`helix-term/src/ui/plugin_panel.rs`（新）、`helix-term/src/ui/mod.rs`、`helix-term/src/commands/typed.rs`

- [ ] **步骤 1：失败单测**（helix-js tests）

```rust
#[test]
fn panel_api() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        const pid = helix.open_panel({ side: "right", size: 30, render: () => ["p1", "p2"] });
        helix.echo("id:" + pid);
        helix.close_panel(pid);
        "#,
    )
    .unwrap();
    let reqs = take_ui_requests();
    assert!(matches!(&reqs[0], UiRequest::OpenPanel { id, side, size } if side == "right" && *size == 30));
    assert!(matches!(&reqs[1], UiRequest::ClosePanel { id: _ }));
    assert!(take_messages()[0].starts_with("id:"));
    // 校验：side 白名单 / size / render
    assert!(load_script(r#"helix.open_panel({ side: "top", size: 10, render: () => [] });"#).is_err());
    assert!(load_script(r#"helix.open_panel({ side: "right", size: 10 });"#).is_err()); // 缺 render
    assert!(load_script(r#"helix.open_panel({ side: "right", size: "big", render: () => [] });"#).is_err());
}
```

- [ ] **步骤 2：运行失败** → 编译错误。
- [ ] **步骤 3：实现 helix-js**
  - `UiRequest::OpenPanel { id: u64, side: String, size: u16 }`、`UiRequest::ClosePanel { id: u64 }`（并入既有枚举，既有 match 补通配）
  - `js_open_panel`：side 白名单 ["right","left","bottom"]；size 数字校验；render 函数校验；分配 id（复用 NEXT_POPUP_ID 计数）→ 注册 PopupCallbacks（on_chunk=render, on_key=None, on_close 可选）→ 入队 OpenPanel → 返回 id
  - `js_close_panel`：id 校验 → 入队 ClosePanel
  - `render_popup`/`close_popup` 复用（面板渲染走同一注册表）
- [ ] **步骤 4：helix-js 通过**（22 个，clippy 0）
- [ ] **步骤 5：实现 PluginPanel 组件**
  - `helix-term/src/ui/plugin_panel.rs`：`pub struct PluginPanel { id: u64, side: PanelSide, size: u16 }`；`render` 按 side 计算 Rect（right: x=area.right-size, width=size, 全高；left: x=area.x, width=size；bottom: y=area.bottom-size, height=size, 全宽）→ `render_popup(id, w, h)` → StyledLine Span 渲染（复用样式逻辑——若 v9-② 已合并；未合并则先纯文本，合并后补）；`handle_event` 恒 `Ignored(None)`（事件穿透）
  - `ui/mod.rs` 导出
- [ ] **步骤 6：dispatch 接线 + :panel-close**
  - typed.rs `apply_ui_requests`：OpenPanel → push `PluginPanel` 层（不用 Popup 包裹——面板是独立层）；ClosePanel → job callback 里 `compositor.remove`（按 id 找——PluginPanel 加 `type_name` 查找或用 id 匹配层列表；以 compositor 既有 API 为准，若无法按 id 移除则先只支持"面板必须由打开它的命令后手动 close"，实现时选最简可用路径并注明）
  - `:panel-close` TypableCommand：调 `helix_js::close_panel`（入队 ClosePanel）——但入队后需要 drain——直接在命令里触发 apply_ui_requests？命令路径的 drain 在 run_command Ok(true) 分支……`panel-close` 是 TypableCommand，其 fn 里手动 `apply_ui_requests(helix_js::take_ui_requests())`（与 plugin_reload 同款）
- [ ] **步骤 7：集成测试**（tests/test/plugin_panel.rs，临时 mod）
  - `:panel-demo`（open_panel right 20 + 命令注册）→ 层存在（has_component PluginPanel）
  - 编辑器仍可编辑：按 `i` + `x` + `<esc>` → 文档文本变化（事件穿透证明）
  - `:panel-close` → 层消失
- [ ] **步骤 8：回归**（plugin 组、command_line、cargo check/clippy）
- [ ] **步骤 9：Commit** `feat(js): docked side panel (open_panel/close_panel)`

---

## 自检
- 覆盖：API 校验/层开合/事件穿透/集成测试。
- 风险：ClosePanel 按 id 移除层的机制（compositor API 限制——最简可用路径）；面板渲染复用 render_popup 与弹窗共用注册表（id 冲突？——id 空间共用，open_panel 与 open_popup 各占不同 id 即可）；v9-② 未合并时面板样式先纯文本。
