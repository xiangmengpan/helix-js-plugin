# G10b（C 批）· `plugin_terminal` 的 `handle_event` 拆分设计规格

日期:2026-10-17
状态:**待审查**(铁律 5:批准前不动代码)
关联:[`../architecture-audit.md`](../../architecture-audit.md) §9.3(S2 最后一条)· G10(`JsSurface`,已完成)· [`2026-10-17-js-surface-design.md`](2026-10-17-js-surface-design.md)

## 0. 决策记录(需要你拍板的三点)

1. **不把 terminal 接进 `JsSurface`**(G10 已明确「terminal 进第二批要单独评估」)。实测结论:**共享面几乎为零**:
   - `JsSurface` 的核心是「`lines: Vec<StyledLine>` + `focusables` + 节点键路由(`js_dispatch_node_event`/
     `js_dispatch_input_key`)+ 脏格 diff」。终端这三样**都没有**:它自己维护 `TerminalGrid`(alacritty 引擎),
     按键走的是 `cx.js_emit_term_key(pty_id, desc, shift, ctrl, alt)`(**按 pty 而非节点**),
     渲染是逐格画 `TerminalCell`。
   - 硬套只会得到「一个只有 `render_lines` 用不上的共享对象」,比现在更难读。
2. **拆法是「同一文件内按事件种类分方法」**,不是拆文件。`plugin_terminal.rs` 的生产部分约 1,000 行
   (其余 555 行是 23 条单测),三层结构(`TerminalCell` / `TerminalGrid` / `PluginTerminal`)本来就清楚;
   问题只在 `handle_event` 一个函数里并列了 5 类事件 + 3 种模式。
3. **行为零变化是唯一判据**:终端的键路径有严格的**优先级**(term-key 钩子 → C-\ → normal/insert),
   任何一个分支顺序变动都会改变「插件能不能拦下一个键」。本项只做**搬 + 命名**,不改条件。

## 1. 目标

把 147 行的 `handle_event` 变成「一眼能看懂的 5 行分派」,每个事件种类一个**有名字**的方法;
让「终端的键语义」从"读 147 行"变成"读 5 个方法名 + 需要时点进去"。

## 2. 现状与约束(已核实)

### 2.1 结构

| 项 | 行数 |
|---|---|
| `plugin_terminal.rs` 全文 | 1,558 |
| 生产部分(到 `mod tests` 之前,第 1003 行) | ~1,000 |
| `PluginTerminal::handle_event` | **147**(clippy 口径)/ 165(含签名) |
| `PluginTerminal::render` | 62 |
| `TerminalGrid::render` | 52 |
| 单测(`mod tests`) | 23 条,555 行 |

### 2.2 `handle_event` 现在的 7 段(顺序即优先级)

| # | 段 | 行数量级 | 语义 |
|---|---|---|---|
| 1 | `mode == Minimized` → Ignored | 4 | 最小化时**不消费**按键,编辑器照常工作 |
| 2 | `Event::Mouse` | ~18 | **应用开了鼠标上报 → 编码发给 pty(优先)**;否则滚轮走本地滚回 |
| 3 | `Event::Paste` | ~10 | Normal(滚动查看)态 → **交还编辑器**;否则按 2004 括号粘贴包一层发 pty |
| 4 | `term-key` 钩子 | ~32 | Pass / Consume / Minimize / Close 四路决策,**优先于一切本地语义** |
| 5 | `C-\` | ~20 | Insert ↔ Normal 切换;回 Insert 时**收起浮窗并 focus_leaf(0)** |
| 6 | Normal 模式键 | ~52 | `j/k/PgUp/PgDn/g/G` 滚动、`y` 复制可视区、`i/a` 回 Insert、其余直通 pty |
| 7 | Insert 模式键 | ~14 | `Esc` → Normal;其余直通 pty |

### 2.3 测试网(改动的安全垫)

| 测试 | 规模 | 覆盖 |
|---|---|---|
| `helix-term/src/ui/plugin_terminal.rs` 内单测 | 23 条 | 网格/解析/宽字符/alt-screen/resize/鼠标编码 |
| `tests/test/plugin_terminal_hooks.rs` | 1,056 行 / **26** 条 | term-key 钩子的四路决策、最小化/关闭、`term-exit` |
| `tests/test/plugin_terminal_modes.rs` | 1,082 行 / **14** 条 | Insert/Normal 切换、滚动、复制、`term-mode` 事件 |
| `tests/test/plugin_terminal_view.rs` | 84 行 / 1 条 | 视图渲染冒烟 |

## 3. 目标形态

同文件内新增 5 个方法(`handle_event` 只剩分派):

```rust
impl PluginTerminal {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        // minimized：不消费按键，编辑器照常工作
        if self.mode == TermMode::Minimized {
            return EventResult::Ignored(None);
        }
        match event {
            Event::Mouse(mouse) => return self.on_mouse(mouse),
            Event::Paste(contents) => return self.on_paste(contents),
            Event::Key(key) => return self.on_key(key, cx),
            _ => return EventResult::Ignored(None),
        }
    }

    /// 鼠标：应用开了上报 → 编码发 pty（优先于本地滚动）；否则滚轮本地滚回。
    fn on_mouse(&mut self, mouse: &MouseEvent) -> EventResult;

    /// 粘贴：Normal（滚动查看）态交还编辑器；否则按 2004 括号粘贴发给 pty。
    fn on_paste(&mut self, contents: &str) -> EventResult;

    /// 按键总入口：term-key 钩子 → C-\ 切换 → 按输入模式分流。
    fn on_key(&mut self, key: &KeyEvent, cx: &mut Context) -> EventResult;

    /// term-key 钩子的四路决策；`None` = 无钩子/未拦截，交给本地语义。
    fn term_key_decision(&mut self, key: &KeyEvent, cx: &mut Context) -> Option<EventResult>;

    /// Normal（滚动查看）模式：滚动 / 复制 / 回 insert / 其余直通 pty。
    fn normal_mode_key(&mut self, key: &KeyEvent) -> EventResult;

    /// Insert 模式：Esc → Normal；其余直通 pty。
    fn insert_mode_key(&mut self, key: &KeyEvent, cx: &mut Context) -> EventResult;
}
```

- 每个方法体是**原段落的逐行搬移**(含注释),不重写条件;
- `C-\` 那段留在 `on_key`(它是两模式共同的入口逻辑,且要 `cx`);
- `render`(62 行)本项**不动**(它已有清晰结构:记录 area → 画 grid → 画标题条 → diff)。

### 3.1 明确不做

- **不做** `JsSurface` 复用(§0.1);
- **不拆文件**:三层结构已清楚,拆文件只会让"终端语义"散到两个目录;
- **不动** `render` / `TerminalGrid` / 鼠标编码 / `key_to_term_bytes`;
- **不改** 任何分支顺序或条件(见 §5 的逐条对照)。

## 4. 边界与错误处理

- `term_write` 失败仍然**静默**(worker 已退出时按键照样消费)——这是既有行为,不改成报错。
- `Event::FocusGained/Lost/Resize` 等其它事件仍 `Ignored`。
- 闭包型 `EventResult::Consumed(Some(Box::new(...)))`(最小化/关闭/复制/收浮窗)原样搬,
  不改捕获列表。

## 5. 完成判据与逐条对照

### 5.1 数字

| 指标 | 现状 | 目标 |
|---|---|---|
| `PluginTerminal::handle_event` | 147 | **≤ 20** |
| 新增方法各自 | — | **≤ 55**(normal_mode_key 最大) |
| `plugin_terminal.rs` 生产部分 | ~1,000 | **不变**(±10 行:只多方法签名) |
| 终端相关测试 | 23 + 26 + 14 + 1 | **全部通过,数量不变** |
| `cargo integration-test` | 377 | **377 passed / 0 failed** |

### 5.2 行为对照清单(改完逐条打勾)

```
[ ] Minimized → 不消费(编辑器照常)
[ ] 鼠标:上报开 → 发 pty(优先);否则滚轮本地滚 3 行;其它鼠标事件 Ignored
[ ] 粘贴:Normal 态 → Ignored(交给编辑器);否则括号粘贴包一层发 pty
[ ] 按键顺序:term-key 钩子(Pass/Consume/Minimize/Close) → C-\ → normal/insert
[ ] C-\ Insert→Normal 发 term-mode normal;C-\ Normal→Insert 发 term-mode insert + 收浮窗 + focus_leaf(0)
[ ] Normal:j/↓ +1、k/↑ -1、PgDn/PgUp ±rows、g 顶、G 底、i/a 回 Insert、Esc 无操作、
      y 复制可视区到 "/+ 寄存器、q 仅消费、其余直通 pty
[ ] Insert:Esc → Normal(发 term-mode normal);其余直通 pty
```

### 5.3 验证命令

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features integration -- -D warnings
cargo test -p helix-term --features integration --test integration plugin_terminal_hooks
cargo test -p helix-term --features integration --test integration plugin_terminal_modes
cargo test -p helix-term --features integration --test integration plugin_terminal_view
cargo test -p helix-term --lib plugin_terminal          # 23 条网格/解析单测
cargo integration-test                                  # 377,全量
```

## 6. 里程碑

| 阶段 | 内容 | 预估 |
|---|---|---|
| M1 | 抽 `on_mouse` / `on_paste`(两段最独立) | 40 分钟 |
| M2 | 抽 `term_key_decision`(四路钩子) | 40 分钟 |
| M3 | 抽 `normal_mode_key` / `insert_mode_key`,留 C-\ 在 `on_key` | 1 小时 |
| M4 | 逐条对照 §5.2 + 全量验证 + 记账 | 40 分钟 |

## 7. 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| 抽 `term_key_decision` 时把"无钩子"和"钩子返回 Pass"混为一谈 | 插件拦键失效/键丢失 | 返回 `Option<EventResult>`:`None` = 继续本地流程,`Some` = 已定论;`Pass` 要**继续发 pty 后返回 Consumed** |
| 模式分流顺序被改(Esc 在两个模式里语义相反) | Esc 关不掉/退不出 | §5.2 的清单逐条对照;`plugin_terminal_modes` 14 条覆盖 |
| 顺手"优化"了 `term_write` 的错误处理 | 行为变化(静默 → 报错会打断输入) | 明确列为不做 |
| 抽方法后 `&mut self` 借用冲突 | 编译期挡住 | 方法都取 `&mut self`;闭包里只捕获已拷贝的 `String`/`u64` |
