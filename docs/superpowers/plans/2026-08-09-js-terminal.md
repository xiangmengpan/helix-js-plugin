# 原生终端视图实现计划（方案二-C）

> 自主执行（方案二窗口内）。TDD → 通过 → 提交；控制者自动合并（并行 wave，最大特性）。

### 任务 1：helix-js open_terminal/term_feed + 网格/vte 组件（分两个任务或一个——以提交粒度为准）

**文件：** `Cargo.toml`（vte）、`helix-js/src/lib.rs`、`helix-term/src/ui/plugin_terminal.rs`（新）、`helix-term/src/ui/mod.rs`、`helix-term/src/commands/typed.rs`

- [ ] **步骤 1：失败单测（helix-js）**

```rust
#[test]
fn open_terminal_api() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.register_command("ot", () => {
            const pid = helix.open_terminal({ cmd: "cat", side: "right", size: 40 });
            helix.echo("pid:" + pid);
            helix.term_feed(pid, "abc");
        });
        "#,
    )
    .unwrap();
    let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
    assert!(run_command("ot", &ctx).unwrap());
    assert!(take_messages()[0].starts_with("pid:"));
    let reqs = take_ui_requests();
    assert!(matches!(&reqs[0], UiRequest::OpenTerminal { side, size, .. } if side == "right" && *size == 40));
    assert!(matches!(&reqs[1], UiRequest::TermFeed { chunk, .. } if chunk == "abc"));
    // 校验
    assert!(load_script(r#"helix.open_terminal({ cmd: "x", side: "top", size: 10 });"#).is_err());
    assert!(load_script(r#"helix.open_terminal({ cmd: "x", side: "right" });"#).is_err()); // 缺 size
}
```

- [ ] **步骤 2：运行失败** → 编译错误。
- [ ] **步骤 3：实现 helix-js**
  - `UiRequest::OpenTerminal { view_id: u64, pty_id: u64, cmd: String, side: String, size: u16 }`、`UiRequest::TermFeed { view_id: u64, chunk: String }`
  - `js_open_terminal`：校验 cmd/side/size/onExit（可选）→ 分配 view_id + 内部 spawn pty（onChunk 桥接闭包：`helix.term_feed(view_id, chunk)`——闭包经 eval 构造；onExit 透传用户回调）→ `UiRequest::OpenTerminal` → 返回 view_id
  - `js_term_feed`（校验 → 入队）
- [ ] **步骤 4：网格 + vte（helix-term/src/ui/plugin_terminal.rs）**
  - `TerminalCell { ch: char, fg: Option<tui::Color>, bg: Option<tui::Color>, bold: bool }`
  - `TerminalGrid { cols, rows, cells: Vec<TerminalCell>, cursor: (u16,u16), saved_cursor, alt: bool, scrollback: VecDeque<Vec<TerminalCell>>（最后 N 行）, scrollback_len: usize }`
  - `impl vte::Perform for TerminalGrid`：print（写字符 + 换行 wrap）、execute（\n \r \t \b）、csi_dispatch（CUU/CUD/CUF/CUB/CUP/ED/EL/EL/SGR 基础/ICH/IL/DL/SU/SD）、osc_dispatch 忽略、esc_dispatch（alt screen 切换）
  - `feed(&mut self, bytes: &[u8])`：vte::Parser 喂入
  - `resize(rows, cols)`：截断/填充（光标 clamp）
  - `render(area, surface, theme)`：网格画到 surface
  - `PluginTerminal { view_id, pty_id, grid }`：render 时检测尺寸变化 → `term_resize(pty_id, rows, cols)`；`handle_event` 按键 → `term_write(pty_id, key)`（Esc → 关面板）；`feed(chunk)` 公共方法
  - 单测：feed "hello" 网格内容；CSI 光标/清屏/SGR 颜色；alt screen；resize
- [ ] **步骤 5：helix-term drain**
  - `OpenTerminal` → push `PluginTerminal` 层（job 通道，与 OpenPanel 同款）
  - `TermFeed` → job → 找 PluginTerminal 层（按 view_id）→ feed（层不存在则丢弃——报告注明竞态处理）
  - 关闭面板 → kill pty（onClose 或 drop 路径）
- [ ] **步骤 6：测试**
  - 网格单测（步骤 4 内）全过
  - 集成（tests/test/plugin_terminal_view.rs，临时 mod）：`:term-native` 开原生终端 → 层存在；命令后 term_feed 模拟输出 → 渲染 Buffer → 断言文本可见
- [ ] **步骤 7：回归**（plugin 组、command_line、cargo check/clippy）
- [ ] **步骤 8：Commit**（可拆分：`feat(js): native terminal view (vte grid)` 等）

---

## 自检
- 风险：vte 0.15 API（Perform trait 方法签名以 crate 源码为准）；宽字符（PoC 单字符）；feed 的字节 vs 字符边界（vte 按字节喂——chunk 的 UTF-8 边界：vte 处理多字节字符（print 会收到完整字符——vte 内部处理 UTF-8 序列）；若块边界切坏 UTF-8 需尾部缓冲（复用 read_stream 的教训）；OpenTerminal 的 pty spawn 与 view 的时序（feed 早于层存在 → 丢弃或缓存——以实现最简为准并注明）；颜色映射（SGR 基础色 → tui Color）。
