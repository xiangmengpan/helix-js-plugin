# 文件树/终端面板补全 API 实现计划（v13）

> 自主执行。TDD → 通过 → 提交；控制者自动合并。

### 任务 1：三个 API（helix-js）

**文件：** `helix-js/src/lib.rs`

- [ ] **步骤 1：失败单测**

```rust
#[test]
fn sidecar_apis() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    let dir = std::env::temp_dir().join(format!("helix-js-sidecar-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    std::fs::write(dir.join("b.js"), "b").unwrap();

    load_script(
        r#"
        helix.register_command("sc", () => {
            const entries = helix.read_dir(DIR);
            helix.echo("count:" + entries.length + " sorted:" + (entries[0].name < entries[1].name));
            const dirs = entries.filter(e => e.is_dir);
            helix.echo("dirs:" + dirs.length + ":" + dirs[0].name);
            helix.open_file(FILE);
            helix.move_panel(7, "left");
        });
        "#,
        // DIR/FILE 用字符串替换注入
    )
    .unwrap();
    // 注入路径后运行
    let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
    // 注：简单方式——脚本里用占位符，load 前 replace
    // （实现时直接拼接路径字符串进脚本，这里示意）
    assert!(run_command("sc", &ctx).unwrap());
    let msgs = take_messages();
    assert!(msgs[0].starts_with("count:2 sorted:true"), "{msgs:?}");
    assert_eq!(msgs[1], "dirs:1:sub");
    let reqs = take_ui_requests();
    assert!(matches!(&reqs[0], UiRequest::OpenFile { path } if path.ends_with("a.txt")));
    assert!(matches!(&reqs[1], UiRequest::MovePanel { id: 7, side } if side == "left"));

    // 校验：read_dir 不存在路径 → 抛错；move_panel 非法 side → 抛错；open_file 非字符串 → 抛错
    // （各自注册命令触发，run_command is_err）
}
```

> 注意：脚本里的路径直接拼接（`format!(r#"helix.read_dir({:?})"#, dir_str)`——用 JSON 字符串转义）。read_dir 排序按名字（to_string_lossy 比较）。is_dir 用 `entry.file_type().map(|t| t.is_dir()).unwrap_or(false)`。

- [ ] **步骤 2：运行失败** → 编译错误。
- [ ] **步骤 3：实现 helix-js**
  - `UiRequest::OpenFile { path: String }`、`UiRequest::MovePanel { id: u64, side: String }`（并入枚举，既有 match 补通配）
  - `js_read_dir`：`std::fs::read_dir` → entries 收集（错误条目跳过）→ 排序 → 每个转 `{ name, is_dir, path }` JS 对象 → 数组返回
  - `js_open_file` / `js_move_panel`：校验入队
- [ ] **步骤 4：helix-js 通过**（30 个，clippy 0）

### 任务 2：helix-term drain + 集成测试

**文件：** `helix-term/src/commands/typed.rs`、`helix-term/src/ui/plugin_panel.rs`、`helix-term/tests/test/plugin_sidecar.rs`（新）

- [ ] **步骤 1：失败集成测试**（临时 mod，跑通后移除 integration.rs 改动）
  - 插件命令调 `helix.open_file("<临时文件>")` → 断言 `app.editor.documents` 含该文件（`current_ref!` 的 doc path 等于它，或 `editor.document_id_by_path` 存在）
  - 面板开右 → 命令调 `helix.move_panel(id, "left")` → 断言面板层 side == Left（层内字段——PluginPanel 加 `side()` 访问器；或渲染断言）
  - read_dir 经命令 echo 条目数 → 状态栏断言
- [ ] **步骤 2：运行失败** → 断言失败（未实现）。
- [ ] **步骤 3：实现 helix-term**
  - drain（apply_ui_requests 或事件路径）：
    - `OpenFile { path }` → `editor.open(&PathBuf::from(path), Action::Replace)`（:open 语义；错误 set_error）。注意 job 通道还是同步——open 是同步的（editor.open 直接返回），可在 drain 内直接做
    - `MovePanel { id, side }` → 走 job 通道（无 compositor）→ `compositor` 找 PluginPanel 层（按 id 匹配）→ `set_side(side)`（PluginPanel 加字段 setter；side 白名单解析在 helix-js 已做）
  - PluginPanel：`set_side(PanelSide)` + `side()` 访问器（compositor 每帧枚举，改后自动重排）
- [ ] **步骤 4：跑测试**（临时 mod）→ PASS；plugin 全组回归；cargo check/clippy
- [ ] **步骤 5：Commit**

```bash
git add helix-js helix-term
git commit -m "feat(js): open_file, move_panel, read_dir APIs"
```

---

## 自检
- 风险：OpenFile 的 drain 位置（editor.open 同步可直做——确认 editor 可变借用）；MovePanel 的 job 通道（与 ClosePanel 模式一致）；read_dir 的排序与错误条目；PanelSide 解析复用。
