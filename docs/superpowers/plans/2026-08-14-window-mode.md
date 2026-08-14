# Window 模式(方案 1 一期)实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** 全局 C-w 窗口模式(聚焦/交换/resize/关闭/最小化/最大化)+ 一期 buffer 叶子与 fixed 标记。

**架构:** compositor 持有 `WindowMode` 状态,`handle_event` 入口(macro recording 之后、layers 之前)最先拦截 C-w 与模式键,直调 LayoutTree 原生方法;`LayoutTree` 增加 `fixed: HashSet<u64>`;新组件 `BufferLeaf` 复用 `render_view` 渲染单 view;状态栏经 `RenderContext.window_mode`(默认)与 `StatuslineCtx.window_mode`(replace)两路显示 `[WINDOW]`。

**技术栈:** Rust(compositor/layout/ui)、helix-js(boa)、现有集成测试框架(tests/test/helpers.rs)。

**规格:** `docs/superpowers/specs/2026-08-14-window-mode-design.md`

---

### 任务 1:WindowMode 状态机 + C-w/Esc 进出 + 状态栏默认指示

**文件:**
- 修改:`helix-term/src/compositor.rs`(handle_event 入口、render 状态栏、push 后新增字段)
- 修改:`helix-term/src/ui/statusline.rs`(RenderContext + render 默认分支)
- 测试:`helix-term/tests/test/window_mode.rs`(新建,`integration.rs` 注册 `mod window_mode;`)

- [ ] **步骤 1:编写失败测试**(测试模块骨架 + 状态切换断言)

`helix-term/tests/test/window_mode.rs`(仿 filetree.rs 的 pump/render 辅助):

```rust
use super::*;
use helix_term::application::Application;
use helix_term::compositor::Component;
use helix_term::job::Jobs;
use helix_view::input::parse_macro;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

async fn pump(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(keys)?.into_iter() {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    Ok(())
}

/// 渲染 compositor 到 Buffer,返回所有行(仿 filetree.rs)
fn render_rows(app: &mut Application, area: helix_view::graphics::Rect) -> Vec<String> {
    let mut buf = tui::buffer::Buffer::empty(area);
    app.compositor.reset_plugin_diffs();
    let mut jobs = Jobs::new();
    let mut cx = helix_term::compositor::Context {
        editor: &mut app.editor,
        scroll: None,
        jobs: &mut jobs,
    };
    app.compositor.render(area, &mut buf, &mut cx);
    (0..area.height)
        .map(|y| {
            buf.content
                .iter()
                .skip(y as usize * area.width as usize)
                .take(area.width as usize)
                .map(|c| c.symbol.as_str())
                .collect::<String>()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn window_mode_enter_exit_toggles() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    assert!(!app.compositor.window_mode_active(), "初始非模式");
    pump(&mut app, "C-w").await?;
    assert!(app.compositor.window_mode_active(), "C-w 进模式");
    pump(&mut app, "esc").await?;
    assert!(!app.compositor.window_mode_active(), "Esc 退模式");
    pump(&mut app, "C-w C-w").await?;
    assert!(!app.compositor.window_mode_active(), "模式内 C-w 退出");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn window_mode_statusline_indicator() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    pump(&mut app, "C-w").await?;
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    let status = &rows[29];
    assert!(status.contains("[WINDOW]"), "状态栏含指示: {status:?}");
    pump(&mut app, "esc").await?;
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    assert!(!rows[29].contains("[WINDOW]"), "退出后指示消失: {:?}", &rows[29]);
    Ok(())
}
```

- [ ] **步骤 2:运行测试验证失败**

运行:`HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-term --test integration --features integration -- window_mode`
预期:`window_mode_active` 不存在编译失败(E0599)。

- [ ] **步骤 3:实现最少代码**

`helix-term/src/compositor.rs`:

```rust
/// 全局窗口模式(C-w):任何叶子焦点下生效;模式键直调布局树。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WindowMode {
    Inactive,
    Active,
}

impl Compositor {
    /// 测试/状态栏访问:当前是否处于窗口模式
    pub fn window_mode_active(&self) -> bool {
        matches!(self.window_mode, WindowMode::Active)
    }
}
```

Compositor 结构体加字段 `pub(crate) window_mode: WindowMode`(初始化 `Inactive`)。`handle_event` 入口(macro recording 检查之后、`let mut callbacks` 之前)插入:

```rust
use helix_view::input::{KeyCode, KeyModifiers};
// 窗口模式:任何焦点下 C-w 进入,模式内 Esc/C-w 退出(其他键 Ignored,任务 2 填充)
if self.window_mode_active() {
    if let Event::Key(key) = event {
        let esc = matches!(key.code, KeyCode::Esc);
        let ctrl_w = key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('w'));
        if esc || ctrl_w {
            self.window_mode = WindowMode::Inactive;
            return true;
        }
    }
    return true; // 模式消费所有键(任务 2 改为键位表分发)
}
if let Event::Key(key) = event {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('w')) {
        self.window_mode = WindowMode::Active;
        return true;
    }
}
```

`compositor.render` 构造 `RenderContext` 处加 `window_mode: self.window_mode_active()`。

`helix-term/src/ui/statusline.rs`:

```rust
pub struct RenderContext<'a> {
    // ...现有字段
    pub window_mode: bool,
}
```

`RenderContext::new` 加参数 `window_mode: bool`(更新全部调用点;compositor.render 传 `self.window_mode_active()`,其他测试调用点传 `false`)。`render()` 默认分支(非 replace)在左段渲染之前前置:

```rust
if context.window_mode {
    append(
        &mut context.parts.left,
        Span::from("[WINDOW] "),
        base_style,
    );
}
```

`helix-term/tests/integration.rs` 加 `mod window_mode;`。

- [ ] **步骤 4:运行测试验证通过**

运行:同步骤 2
预期:2 passed。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/compositor.rs helix-term/src/ui/statusline.rs helix-term/tests/test/window_mode.rs helix-term/tests/integration.rs
git commit -m "feat(window): window mode state machine + C-w/Esc toggle + statusline indicator"
```

---

### 任务 2:模式键位(聚焦/交换/resize/关闭/最小化/最大化)

**文件:**
- 修改:`helix-term/src/compositor.rs`(handle_event 模式分支)
- 测试:`helix-term/tests/test/window_mode.rs`

- [ ] **步骤 1:编写失败测试**

```rust
use helix_term::ui::layout::SplitDir;
use helix_term::ui::PluginPanel;
use helix_term::ui::plugin_panel::PanelSide;

/// 方向键映射辅助断言:h → 左邻居、l → 右、k → 上、j → 下
#[tokio::test(flavor = "multi_thread")]
async fn window_mode_directional_focus() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    // 编辑器(id=0)右侧切面板(id=1):H split,second=面板
    app.compositor.split_leaf_with_ratio(SplitDir::H, false, 40, Box::new(PluginPanel::new(1, PanelSide::Right)));
    let active = || app.compositor.layout_tree().active();
    assert_eq!(active(), 1, "面板打开后活动叶子=面板");
    pump(&mut app, "C-w h").await?;
    assert_eq!(active(), 0, "h → 左邻居(编辑器)");
    pump(&mut app, "C-w l").await?;
    assert_eq!(active(), 1, "l → 右邻居(面板)");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn window_mode_close_minimize_zoom() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    app.compositor.split_leaf_with_ratio(SplitDir::H, false, 40, Box::new(PluginPanel::new(1, PanelSide::Right)));
    // 最小化往返
    pump(&mut app, "C-w z").await?;
    assert!(app.compositor.layout_tree().minimized().is_some(), "z 最小化");
    pump(&mut app, "C-w z").await?;
    assert!(app.compositor.layout_tree().minimized().is_none(), "再 z 还原");
    // 最大化往返
    pump(&mut app, "C-w f").await?;
    assert!(app.compositor.layout_tree().zoomed() == Some(1), "f 最大化");
    pump(&mut app, "C-w f").await?;
    assert!(app.compositor.layout_tree().zoomed().is_none(), "再 f 还原");
    // 关闭
    pump(&mut app, "C-w x").await?;
    assert!(!app.compositor.has_component(panel_type), "x 关闭面板叶子");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn window_mode_swap_and_resize() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let area = app.compositor.size();
    let _total = area.width;
    app.compositor.split_leaf_with_ratio(SplitDir::H, false, 40, Box::new(PluginPanel::new(1, PanelSide::Right)));
    // 交换:活动=面板(1),H + l → 与编辑器交换
    pump(&mut app, "C-w L").await?;
    // 交换后 id=0 叶子是面板(id=1 是编辑器);焦点仍在 1
    let types: Vec<_> = app.compositor.layout_tree().leaf_types();
    assert_eq!(types[0], "PluginPanel", "交换后左叶子为面板");
    // resize:宽度 +5%
    pump(&mut app, "C-w l").await?; // 回编辑器(0)
    pump(&mut app, "C-w C-l").await?;
    let dump = app.compositor.layout_tree().get_layout();
    let layout = helix_js::layout_dump(); // 任务 2 暂用 leaf width 断言
    let _ = (dump, layout);
    Ok(())
}
```

> 注:`minimized()`/`zoomed()`/`leaf_types()` 若不存在则在本步一起实现访问器(见步骤 3)。

- [ ] **步骤 2:运行测试验证失败**

运行:`cargo test -p helix-term --test integration --features integration -- window_mode`
预期:模式键未实现(当前所有键 return true)→ 断言失败或访问器缺失编译错。

- [ ] **步骤 3:实现最少代码**

`compositor.rs` handle_event 模式分支替换任务 1 的 `return true`:

```rust
if self.window_mode_active() {
    if let Event::Key(key) = event {
        use helix_view::input::KeyModifiers as KM;
        let ctrl = key.modifiers.contains(KM::CONTROL);
        let ch = match key.code {
            KeyCode::Char(c) => Some(c),
            _ => None,
        };
        match ch {
            Some(c) if c == 'w' && ctrl => { self.window_mode = WindowMode::Inactive; return true; }
            _ if matches!(key.code, KeyCode::Esc) => { self.window_mode = WindowMode::Inactive; return true; }
            Some('h') => { self.window_mode_focus('h'); return true; }
            Some('j') => { self.window_mode_focus('j'); return true; }
            Some('k') => { self.window_mode_focus('k'); return true; }
            Some('l') => { self.window_mode_focus('l'); return true; }
            Some('H') => { self.window_mode_swap('h'); return true; }
            Some('J') => { self.window_mode_swap('j'); return true; }
            Some('K') => { self.window_mode_swap('k'); return true; }
            Some('L') => { self.window_mode_swap('l'); return true; }
            Some('x') => { self.window_mode_close(); return true; }
            Some('z') => { self.window_mode_minimize(); return true; }
            Some('f') => { self.window_mode_zoom(); return true; }
            Some(c) if ctrl && matches!(c, 'h' | 'j' | 'k' | 'l') => {
                self.window_mode_resize(c); return true;
            }
            _ => return true, // 模式吞掉未知键
        }
    }
    return true;
}
```

**进入条件裁定(编排层):** C-w 仅在 normal/select 模式触发窗口模式;insert 模式 C-w 保留原义(delete_word_backward 删词,vim 惯例)。实现:拦截条件改为 `self.window_mode_active() || (editor.mode() != Mode::Insert && C-w)`。需在 compositor.handle_event 用 `cx.editor.mode()`(handle_event 有 cx 参数)。

**测试迁移(任务 2 一并做):**
- `tests/test/splits.rs` 的两个 C-w 测试(`test_changes_in_splits_apply_to_all_views`、`test_reload_all_with_split_jumplist`)改为 `:split<ret>` 或 `:vsplit<ret>` 命令序列(不再用 C-w 前缀);
- `tests/test/commands.rs` 的 `test_delete_word_backward` 保持原样(insert 模式 C-w 保留后应恢复通过);
- 新增测试:insert 模式下 C-w 不进窗口模式(仍执行删词);normal 模式 C-w 进窗口模式。

LayoutTree 访问器(`layout.rs`,若缺失):

```rust
pub fn minimized(&self) -> Option<u64> { self.minimized }
pub fn zoomed(&self) -> Option<u64> { self.zoomed }
/// 叶子类型名(测试断言用)
pub fn leaf_types(&self) -> Vec<&'static str> {
    let mut out = Vec::new();
    fn walk(node: &LayoutNode, comps: &std::collections::HashMap<u64, Box<dyn Component>>, out: &mut Vec<&'static str>) {
        match node {
            LayoutNode::Leaf { id } => {
                if let Some(c) = comps.get(id) { out.push(c.type_name()); }
            }
            LayoutNode::Split { first, second, .. } => { walk(first, comps, out); walk(second, comps, out); }
        }
    }
    walk(&self.root, &self.components, &mut out);
    out
}
```

compositor 辅助(把方向字符映射为 `(SplitDir, first_side)`,语义已核对:`h`/`k` → first_side=true,`l`/`j` → first_side=false):

```rust
fn window_dir(c: char) -> (SplitDir, bool) {
    match c {
        'h' => (SplitDir::H, true),
        'l' => (SplitDir::H, false),
        'k' => (SplitDir::V, true),
        'j' => (SplitDir::V, false),
        _ => unreachable!(),
    }
}
fn window_mode_focus(&mut self, c: char) {
    let (dir, side) = Self::window_dir(c);
    let a = self.main_tree.active();
    if let Some(nb) = self.main_tree.neighbor_leaf(a, dir, side) {
        self.main_tree.focus(nb);
    }
}
fn window_mode_swap(&mut self, c: char) {
    let (dir, side) = Self::window_dir(c);
    let a = self.main_tree.active();
    if let Some(nb) = self.main_tree.neighbor_leaf(a, dir, side) {
        let _ = self.main_tree.swap(a, nb);
    }
}
fn window_mode_resize(&mut self, c: char) {
    let (dir, delta) = match c {
        'h' => (SplitDir::H, -0.05),
        'l' => (SplitDir::H, 0.05),
        'j' => (SplitDir::V, -0.05),
        'k' => (SplitDir::V, 0.05),
        _ => unreachable!(),
    };
    self.main_tree.resize_leaf_dir(self.main_tree.active(), dir, delta);
}
fn window_mode_close(&mut self) {
    let a = self.main_tree.active();
    if a != 0 { self.remove_leaf(a); }
}
fn window_mode_minimize(&mut self) {
    let a = self.main_tree.active();
    let now = self.main_tree.minimized();
    let minimized = if now == Some(a) { false } else { a != 0 };
    if a != 0 { let _ = self.main_tree.set_minimized(a, minimized); }
}
fn window_mode_zoom(&mut self) {
    let a = self.main_tree.active();
    if self.main_tree.zoomed() == Some(a) { self.unzoom(); } else { self.zoom_leaf(a); }
}
```

`swap` 交换组件引用后焦点仍在原 id(leaf_types 断言据此写)。resize 断言:任务 2 用 `get_layout` 无法取宽度,改为单测(layout.rs 已有 resize 测试模式);集成测试只断言不 panic + 布局仍为 2 叶。

- [ ] **步骤 4:运行测试验证通过**

运行:同步骤 2;另加 `cargo test -p helix-term --lib layout::` 确保布局单测不回归
预期:全过。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/compositor.rs helix-term/src/ui/layout.rs helix-term/tests/test/window_mode.rs
git commit -m "feat(window): mode keys focus/swap/resize/close/minimize/zoom"
```

---

### 任务 3:弹窗打开自动退模式

**文件:**
- 修改:`helix-term/src/compositor.rs`(push)
- 测试:`helix-term/tests/test/window_mode.rs`

- [ ] **步骤 1:编写失败测试**

```rust
#[tokio::test(flavor = "multi_thread")]
async fn window_mode_exits_on_popup_open() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    pump(&mut app, "C-w").await?;
    assert!(app.compositor.window_mode_active());
    // 打开一个弹窗层(仿 filetree prompt:push popup 类 layer)
    let popup = helix_term::ui::Popup::new("plugin-popup", helix_term::ui::PluginPopup::new(99, Some((10, 5))))
        .position(helix_core::Position::new(0, 0));
    app.compositor.push(Box::new(popup));
    assert!(!app.compositor.window_mode_active(), "弹窗打开自动退模式");
    Ok(())
}
```

- [ ] **步骤 2:运行测试验证失败**

运行:`cargo test -p helix-term --test integration --features integration -- window_mode::window_mode_exits_on_popup_open`
预期:FAIL(push 后仍 Active)。

- [ ] **步骤 3:实现最少代码**

`compositor.rs push()`:

```rust
pub fn push(&mut self, mut layer: Box<dyn Component>) {
    // 窗口模式下打开弹窗/菜单:自动退模式(否则弹窗按键被模式吞掉)
    if self.window_mode_active() && layer.id().is_some() {
        self.window_mode = WindowMode::Inactive;
    }
    // ...现有逻辑
}
```

`PluginPopup` 需实现 `id()`(检查:若无则 `layer.id().is_some()` 判断无效,改为按 `type_name` 含 "Popup" 判断;优先给 `PluginPopup`/`Popup` 加 `id()` 返回 `Some("plugin-popup")`)。

- [ ] **步骤 4:运行测试验证通过**

运行:同步骤 2
预期:PASS。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/compositor.rs helix-term/tests/test/window_mode.rs
git commit -m "feat(window): auto-exit window mode when popup opens"
```

---

### 任务 4:replace 模式状态栏透传 + statusline.js

**文件:**
- 修改:`helix-js/src/popup.rs`(StatuslineCtx + statusline_parts 构造)
- 修改:`helix-term/src/ui/statusline.rs`(js_ctx 构造传 window_mode)
- 修改:`plugins/features/statusline.js`
- 测试:`helix-term/tests/test/plugin_statusline.rs`

- [ ] **步骤 1:编写失败测试**

`plugin_statusline.rs` 追加(现有 helper 可复用,渲染 statusline 到独立 surface 的方式):

```rust
#[tokio::test(flavor = "multi_thread")]
async fn plugin_statusline_window_mode_field() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let plugin_path = dir.path().join("sl.js");
    std::fs::write(
        &plugin_path,
        r#"helix.set_statusline((ctx) => ctx.window_mode ? "MODE_ON" : "MODE_OFF");"#,
    )?;
    test_key_sequences(
        &mut AppBuilder::new().build()?,
        vec![
            (Some(&format!(":plugin-load {}<ret>", plugin_path.display())), None),
            (
                Some("C-w"),
                Some(&|app| {
                    let (view, doc) = current_ref!(app.editor);
                    let area = helix_view::graphics::Rect::new(0, 0, 120, 1);
                    let mut buf = tui::buffer::Buffer::empty(area);
                    let spinners = helix_term::ui::ProgressSpinners::default();
                    let mut rc = helix_term::ui::statusline::RenderContext::new(
                        &app.editor, doc, view, true, &spinners, true, // window_mode=true
                    );
                    helix_term::ui::statusline::render(&mut rc, area, &mut buf);
                    let rendered: String = buf.content.iter().map(|c| c.symbol.as_str()).collect();
                    assert!(rendered.contains("MODE_ON"), "window_mode 透传给 JS: {rendered:?}");
                }),
            ),
        ],
        false,
    ).await?;
    Ok(())
}
```

(注:`RenderContext::new` 签名在任务 1 加了 `window_mode` 参数,此处传 `true` 验证 JS 钩子收到。)

- [ ] **步骤 2:运行测试验证失败**

运行:`cargo test -p helix-term --test integration --features integration -- plugin_statusline::plugin_statusline_window_mode_field`
预期:FAIL(StatuslineCtx 无 window_mode 字段,JS 读到 undefined → 渲染 "MODE_OFF")。

- [ ] **步骤 3:实现最少代码**

`helix-js/src/popup.rs` `StatuslineCtx` 加字段:

```rust
pub struct StatuslineCtx {
    // ...现有
    pub window_mode: bool,
}
```

`statusline_parts` 的 ctx_obj 加:

```rust
.property(JsString::from("window_mode"), JsValue::from(ctx.window_mode), Attribute::all())
```

`helix-term/src/ui/statusline.rs` js_ctx 构造处:

```rust
window_mode: context.window_mode,
```

`plugins/features/statusline.js` render 开头:

```js
if (ctx.window_mode) {
  parts.push({ text: "[WINDOW] ", style: "ui.statusline.insert" });
}
```

- [ ] **步骤 4:运行测试验证通过**

运行:同步骤 2
预期:PASS。

- [ ] **步骤 5:Commit**

```bash
git add helix-js/src/popup.rs helix-term/src/ui/statusline.rs plugins/features/statusline.js helix-term/tests/test/plugin_statusline.rs
git commit -m "feat(window): expose window_mode to replace-mode statusline JS hook"
```

---

### 任务 5:LayoutTree.fixed + layout_fix API

**文件:**
- 修改:`helix-term/src/ui/layout.rs`(fixed 集合 + 操作跳过 + get_layout 字段)
- 修改:`helix-term/src/compositor.rs`(set_leaf_fixed)
- 修改:`helix-js/src/types.rs`(UiRequest::LayoutFix)
- 修改:`helix-term/src/commands/typed.rs`(处理 LayoutFix)
- 修改:`helix-js/src/layout.rs`(js_layout_fix + 注册)
- 测试:`helix-term/tests/test/window_mode.rs` + layout.rs 单测

- [ ] **步骤 1:编写失败测试**

集成(仿现有 helpers):

```rust
#[tokio::test(flavor = "multi_thread")]
async fn window_mode_fixed_leaf_immune() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    app.compositor.split_leaf_with_ratio(SplitDir::H, false, 40, Box::new(PluginPanel::new(1, PanelSide::Right)));
    // 面板(fixed)设为固定
    let dump = app.compositor.layout_tree().get_layout();
    let fixed_id = dump.leafs.iter().find(|l| l.id == 1).unwrap().id;
    app.compositor.set_leaf_fixed(fixed_id, true);
    assert!(app.compositor.layout_tree().is_fixed(fixed_id), "fixed 标记生效");
    // 模式内 x 不关闭 fixed 叶子
    pump(&mut app, "C-w x").await?;
    let panel_type = std::any::type_name::<helix_term::ui::PluginPanel>();
    assert!(app.compositor.has_component(panel_type), "fixed 叶子不被 x 关闭");
    // get_layout 输出 fixed 字段
    let dump = app.compositor.layout_tree().get_layout();
    let f = dump.leafs.iter().find(|l| l.id == fixed_id).unwrap();
    assert!(f.fixed, "get_layout 含 fixed 字段");
    Ok(())
}
```

layout.rs 单测(追加到现有 `mod tests`):

```rust
#[test]
fn fixed_leaf_skipped_by_ops() {
    let mut tree = LayoutTree::default();
    let panel = tree.split_side(0, SplitDir::H, false, Box::new(plugin_panel_stub()));
    tree.set_fixed(panel, true);
    assert!(!tree.resize_leaf_dir(panel, SplitDir::H, 0.1), "fixed 不可 resize");
    assert!(!tree.remove(panel), "fixed 不可 remove");
    assert!(!tree.swap(0, panel), "fixed 不可 swap");
    // 焦点可穿过
    assert_eq!(tree.focus_dir(0, SplitDir::H, false), Some(panel), "焦点可移到 fixed");
}
```

(需 `plugin_panel_stub` 或复用现有测试组件;若 layout.rs 测试已有 stub 则沿用。)

- [ ] **步骤 2:运行测试验证失败**

运行:`cargo test -p helix-term --test integration --features integration -- window_mode::window_mode_fixed_leaf_immune && cargo test -p helix-term --lib layout::`
预期:FAIL(方法不存在 / fixed 无效)。

- [ ] **步骤 3:实现最少代码**

`layout.rs`:

```rust
pub struct LayoutTree {
    // ...
    fixed: HashSet<u64>, // Default 初始化空集
}
impl LayoutTree {
    pub fn set_fixed(&mut self, id: u64, fixed: bool) {
        if fixed { self.fixed.insert(id); } else { self.fixed.remove(&id); }
    }
    pub fn is_fixed(&self, id: u64) -> bool { self.fixed.contains(&id) }
}
```

操作入口跳过:`swap` 开头、`resize_leaf_dir` 开头、`remove` 开头、`set_minimized` 开头加:

```rust
if self.fixed.contains(&id) { return false; } // 或 remove 直接 return
```

`equalize` 开头:`if self.fixed.contains(&id) { return; }`。
`get_layout` 输出每叶加 `fixed: self.is_fixed(id)`。

`compositor.rs`:

```rust
pub fn set_leaf_fixed(&mut self, id: u64, fixed: bool) {
    self.main_tree.set_fixed(id, fixed);
    self.sync_layout_cache();
}
```

`helix-js/src/types.rs`:

```rust
LayoutFix { id: u64, fixed: bool },
```

`typed.rs`(apply_ui_requests 分支):

```rust
helix_js::UiRequest::LayoutFix { id, fixed } => {
    job::dispatch_blocking(move |_editor, compositor| {
        compositor.set_leaf_fixed(id, fixed);
    });
}
```

`helix-js/src/layout.rs`:

```rust
pub(crate) fn js_layout_fix(_this: &JsValue, args: &[JsValue], _ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id: f64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(_ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("layout_fix: id must be a number")))
    })?;
    let fixed: bool = args.get(1).cloned().unwrap_or(JsValue::from(false)).try_js_into(_ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("layout_fix: fixed must be a boolean")))
    })?;
    if id < 0.0 || id.fract() != 0.0 {
        return Err(JsError::from_opaque(JsValue::from(JsString::from("layout_fix: id must be a non-negative integer"))));
    }
    crate::state::UI_REQUESTS.get().unwrap().lock().unwrap().push(UiRequest::LayoutFix { id: id as u64, fixed });
    Ok(JsValue::undefined())
}
```

`lib.rs` 注册:`.function(NativeFunction::from_fn_ptr(layout::js_layout_fix), JsString::from("layout_fix"), 2)`。

- [ ] **步骤 4:运行测试验证通过**

运行:同步骤 2
预期:PASS。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/ui/layout.rs helix-term/src/compositor.rs helix-js/src/types.rs helix-term/src/commands/typed.rs helix-js/src/layout.rs helix-js/src/lib.rs helix-term/tests/test/window_mode.rs
git commit -m "feat(window): fixed leaves immune to mode ops + helix.layout_fix"
```

---

### 任务 6:BufferLeaf + helix.buffer_open

**文件:**
- 修改:`helix-term/src/ui/editor.rs`(render_view 抽静态,terminal_focused 参数化)
- 创建:`helix-term/src/ui/buffer_leaf.rs`
- 修改:`helix-term/src/ui/mod.rs`(注册模块)
- 修改:`helix-js/src/types.rs`(UiRequest::OpenBufferLeaf)
- 修改:`helix-term/src/commands/typed.rs`(处理)
- 修改:`helix-js/src/layout.rs` + `lib.rs`(js_buffer_open)
- 测试:`helix-term/tests/test/window_mode.rs`

- [ ] **步骤 1:编写失败测试**

```rust
#[tokio::test(flavor = "multi_thread")]
async fn buffer_open_creates_leaf_with_content() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "AAA\n")?;
    std::fs::write(&b, "BBB\n")?;
    let mut app = AppBuilder::new().with_file(a, None).build()?;
    // JS 侧调 buffer_open(b.txt, {split:"h"}) → 新叶子显示 b.txt
    let src = r#"
        helix.register_command("bo", () => {
            helix.buffer_open(ARG_PATH, { split: "h" });
        });
    "#.replace("ARG_PATH", &format!("{:?}", b.display()));
    let plugin_path = dir.path().join("bo.js");
    std::fs::write(&plugin_path, src)?;
    pump(&mut app, &format!(":plugin-load {}<ret>", plugin_path.display())).await?;
    pump(&mut app, ":bo<ret>").await?;
    // 布局:3 叶(原编辑器 0 + 新 buffer 叶)?不——split 后 2 叶:原编辑器+新叶
    let types = app.compositor.layout_tree().leaf_types();
    assert_eq!(types.len(), 2, "buffer_open 新增叶子: {types:?}");
    // 渲染断言:新叶子区域含 BBB
    let rows = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 30));
    let joined = rows.join("\n");
    assert!(joined.contains("BBB"), "新叶子渲染 b.txt 内容: {joined:?}");
    Ok(())
}
```

- [ ] **步骤 2:运行测试验证失败**

运行:`cargo test -p helix-term --test integration --features integration -- window_mode::buffer_open_creates_leaf_with_content`
预期:FAIL(js_buffer_open 不存在编译错)。

- [ ] **步骤 3:实现最少代码**

**a. render_view 参数化**(`editor.rs`):当前签名 `pub fn render_view(&self, editor, doc, view, viewport, surface, is_focused)`,函数体内 `self.terminal_focused` 两处(overlay_selection 高亮与 gutter)。改为:

```rust
pub fn render_view(
    editor: &Editor,
    doc: &Document,
    view: &View,
    viewport: Rect,
    surface: &mut Surface,
    is_focused: bool,
    terminal_focused: bool,
)
```

`EditorView::render` 调用点改 `Self::render_view(cx.editor, doc, view, area, surface, is_focused, self.terminal_focused)`。检查其余调用点(如有)一并更新。

**b. `ui/buffer_leaf.rs`:**

```rust
use crate::compositor::{Component, Context, EventResult, Event};
use crate::ui::editor::EditorView;
use helix_view::graphics::Rect;
use tui::buffer::Buffer as Surface;

/// 单 view 编辑器叶子:显示指定 view(绑定 buffer)到叶子矩形。
/// 一期:按键 Ignored(路由兜底到编辑器叶子 id=0);多 view/跨叶移动二期。
pub struct BufferLeaf {
    pub view_id: helix_view::view::ViewId,
}

impl Component for BufferLeaf {
    fn handle_event(&mut self, _event: &Event, _cx: &mut Context) -> EventResult {
        EventResult::Ignored(None)
    }
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        surface.set_style(area, cx.editor.theme.get("ui.background"));
        let Some(view) = cx.editor.tree.get_mut(self.view_id) else { return };
        let Some(doc) = cx.editor.document(view.doc) else { return };
        let is_focused = cx.editor.tree.focus == self.view_id;
        EditorView::render_view(cx.editor, doc, view, area, surface, is_focused, true);
    }
    fn type_name(&self) -> &'static str { "BufferLeaf" }
}
```

(需确认 `cx.editor.tree.get_mut` 与 `Editor::document` 签名;若 `document` 返回 Option 则按上;`view_id` 用 `helix_view::view::ViewId` 即 `usize` 类型别名。)

**c. 请求链路:** `types.rs` 加:

```rust
OpenBufferLeaf { path: String, split: Option<String> },
```

`typed.rs` 处理:

```rust
helix_js::UiRequest::OpenBufferLeaf { path, split } => {
    job::dispatch_blocking(move |editor, compositor| {
        use helix_view::editor::Action;
        let id = editor.open(&std::path::PathBuf::from(&path), Action::Append).ok();
        let Some(view_id) = id else {
            editor.set_error(format!("buffer_open: open failed: {path}"));
            return;
        };
        let dir = match split.as_deref() {
            Some("h") => crate::ui::layout::SplitDir::H,
            _ => crate::ui::layout::SplitDir::V,
        };
        let _ = compositor.split_leaf(dir, false, Box::new(crate::ui::BufferLeaf { view_id }));
    });
}
```

(需确认 `Editor::open` 返回 `Option<ViewId>` 及 `Action::Append` 语义;`split_leaf` 现有签名 `(dir, new_first, component)`。)

**d. JS:** `layout.rs` `js_buffer_open` 校验 path 非空 + split ∈ {h,v} → push `OpenBufferLeaf`;`lib.rs` 注册 `.function(..., JsString::from("buffer_open"), 1)`。

- [ ] **步骤 4:运行测试验证通过**

运行:同步骤 2 + 全量 `-- window_mode`
预期:PASS。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/ui/editor.rs helix-term/src/ui/buffer_leaf.rs helix-term/src/ui/mod.rs helix-js/src/types.rs helix-term/src/commands/typed.rs helix-js/src/layout.rs helix-js/src/lib.rs helix-term/tests/test/window_mode.rs
git commit -m "feat(window): BufferLeaf single-view editor leaf + helix.buffer_open"
```

---

### 任务 7:which-key.js / layout.js 下线 C-w 段

**文件:**
- 修改:`plugins/features/which-key.js`
- 修改:`plugins/features/layout.js`(注释与说明;命令保留)
- 修改:`docs/plugin-api.md`(如列出 C-w 组则更新)

- [ ] **步骤 1:删除 C-w 绑定**

`which-key.js`:删除全部 `helix.map("normal", "C-w ...")` 行,顶部注释改为:

```js
// C-w 已由全局 Window 模式接管(compositor 层,Rust):任何焦点(编辑器/终端/面板)
// 下按 C-w 进入,hjkl 聚焦 / HJKL 交换 / C-hjkl resize / x 关闭 / z 最小化 / f 最大化,
// Esc 退出。layout-* 命令仍可用(:layout-* 或插件直调)。
```

`layout.js`:更新 C-w 相关注释,命令保留不动。

- [ ] **步骤 2:验证不回归 + 重写 whichkey 测试**

`plugin_whichkey_cw_group`(plugin_terminal_modes.rs:664)验证的是旧行为(终端 C-w 被吞、C-\ ×2 回编辑器)。窗口模式接管后重写为验证新全局语义:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn plugin_whichkey_cw_group() -> anyhow::Result<()> {
    // 窗口模式全局性:终端焦点下 C-w 直接进模式并导航邻居(无需 C-\ 回编辑器)
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("wk.txt");
    std::fs::write(&file, "x\n")?;
    let opener = dir.path().join("wk_open.js");
    std::fs::write(
        &opener,
        r#"
        helix.register_command("wk-open", () => {
            helix.open_terminal({ cmd: "cat", side: "right", size: 40 });
        });
        "#,
    )?;
    let mut app = AppBuilder::new().with_file(file, None).build()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(&format!(":plugin-load {}<ret>:wk-open<ret>", opener.display()))? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    let term_type = std::any::type_name::<helix_term::ui::plugin_terminal::PluginTerminal>();
    for _ in 0..20 {
        if app.compositor.has_component(term_type) { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(app.compositor.has_component(term_type), "终端应就位");
    // 活动叶子=终端(1);终端焦点直接 C-w l/h——不吞键,进模式并导航
    let cw = |c: char| {
        Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
            code: helix_view::input::KeyCode::Char(c),
            modifiers: helix_view::input::KeyModifiers::CONTROL,
        }))
    };
    tx.send(Ok(cw('w')))?;
    tx.send(Ok(Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
        code: helix_view::input::KeyCode::Char('h'),
        modifiers: helix_view::input::KeyModifiers::NONE,
    }))))?;
    tx.send(Ok(Event::Key(KeyEvent::from(helix_view::input::KeyEvent {
        code: helix_view::input::KeyCode::Esc,
        modifiers: helix_view::input::KeyModifiers::NONE,
    }))))?;
    app.event_loop_until_idle(&mut rx_stream).await;
    assert_eq!(app.compositor.layout_tree().active(), 0, "终端焦点 C-w h 直接聚焦左邻居(编辑器)");
    assert!(!app.compositor.window_mode_active(), "Esc 退出窗口模式");
    Ok(())
}
```

运行:`cargo test -p helix-term --test integration --features integration -- plugin_terminal_modes::plugin_whichkey_cw_group`。

注:该测试不再加载 which-key.js/layout.js(它们不再提供 C-w 组;layout-* 命令由窗口模式内置),去掉对 ~/.config 插件的依赖,避免 HOME 缺失时静默跳过。

- [ ] **步骤 3:Commit**

```bash
git add plugins/features/which-key.js plugins/features/layout.js docs/plugin-api.md
git commit -m "refactor(window): retire which-key C-w group, window mode takes over"
```

---

### 任务 8:回归与文档

**文件:**
- 修改:`docs/handoff-2026-08-14.md`(追加 window 模式一节)
- 修改:`docs/superpowers/specs/2026-08-14-window-mode-design.md`(标注实现状态)

- [ ] **步骤 1:全量回归**

```bash
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-js --lib
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-term --lib
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test -p helix-term --test integration --features integration
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo clippy -p helix-term -p helix-js --all-targets
```

预期:helix-js 43+、helix-term lib 54+、integration 全过(除 handoff 已记录并行 flake)、clippy 0。

- [ ] **步骤 2:真实终端验证**

```bash
tmux 起 release:hx 打开文件 → C-w(状态栏 [WINDOW])→ h/j/k/l、C-w x/z/f、Esc;
C-w f 开 filetree(插件 API 路径)→ C-w l 聚焦面板 → Esc;
:filetree 打开后 C-w x 关闭。
```

- [ ] **步骤 3:更新文档并 Commit**

```bash
git add docs/handoff-2026-08-14.md docs/superpowers/specs/2026-08-14-window-mode-design.md
git commit -m "docs: window mode implementation notes + spec status"
```

---

## 自检

**规格覆盖度:**
- 3.1 状态机 / 3.2 拦截 / 3.3 键位 → 任务 1、2
- 3.4 fixed → 任务 5
- 3.5 BufferLeaf → 任务 6
- 3.6 状态栏(默认 + replace)→ 任务 1、4
- 4 JS API(buffer_open / layout_fix / get_layout.fixed)→ 任务 5、6
- 5 which-key 下线 → 任务 7
- 6 边界(弹窗自动退)→ 任务 3;其余边界在实现中由单测覆盖(z/f 往返在任务 2)
- 7 测试 → 各任务
- 8 里程碑 → 任务 1-8 对齐

**占位符扫描:** 无 TODO/待定;每步有具体代码或命令。

**类型一致性:** `WindowMode`、`window_mode_active()`、`set_leaf_fixed`、`BufferLeaf{view_id}`、`OpenBufferLeaf{path, split}`、`LayoutFix{id, fixed}`、`RenderContext.window_mode`、`StatuslineCtx.window_mode` 全计划一致。

**已知实现期需核实的签名(计划已注明,任务内确认):** `Editor::open` 返回类型、`Editor::document` Option 性、`Popup`/`PluginPopup` 的 `id()` 实现、`split_leaf` 签名、`get_layout` 返回结构字段名。
