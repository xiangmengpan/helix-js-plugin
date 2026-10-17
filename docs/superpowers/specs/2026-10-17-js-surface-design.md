# G10（C 批）· 抽 `JsSurface`:plugin_panel / plugin_popup / plugin_terminal 的共享骨架

日期:2026-10-17
状态:**待审查**(铁律 5:批准前不动代码)
关联:[`../architecture-audit.md`](../../architecture-audit.md) §5 G10 · G6(已完成,注册表拆分)· [`../../plugin-layout.md`](../../plugin-layout.md)

## 0. 决策记录(需要你拍板的三点)

1. **第一步只做 panel + popup,terminal 不进第一批**。terminal(1,558 行)是「终端网格 + 光标 + resize + PTY 回调」的复合体,它的共享面和这两个不同(它自己有两个内部结构 `TerminalCell` / `TerminalGrid`)。把它塞进第一批会让本项的回归面翻倍。
2. **共享的边界是「状态 + 渲染 + 节点键路由」,不是「整个 handle_event」**。两个 `handle_event` 分别是 234 / 218 行,但它们的分歧点(panel: `focusable` 开关、Esc 取消焦点、粘贴冒泡;popup: Esc 关闭、`required_size`、layer 语义)正是业务语义,不能抽掉。抽的是它们**逐段重复**的部分。
3. **判据用「行数 + 重复窗口数 + 377 条集成测试」三者一起**,不靠观感(见 §5)。

## 1. 目标

把 panel / popup 共有的三个东西收进一个 `JsSurface`,让「加第四个 JS 组件」不再需要复制 200 行:

- **状态**:`id` / `focus` / `focusables` / `lines` / `diff: DiffRenderer`;
- **刷新**:调 JS `render` → 收 `Content` → 抽 focusables → `comp_layout::render` 成行;
- **节点键路由**:Tab / Shift-Tab 切焦点、焦点在节点时把按键分发给节点事件、分发后 drain 消息 + UI 请求。

## 2. 现状与约束(已核实)

### 2.1 结构对照(实测行数)

| | `ui/plugin_panel.rs`(369 行) | `ui/plugin_popup.rs`(401 行) |
|---|---|---|
| 状态字段 | `id, side, focusable, focus, focusables, lines, diff` | `id, lines, size_hint, layer, focus, focusables, diff` |
| `new` | 11 行 | 11 行 |
| `reset_render_state` | **3 行,逐字相同** | **3 行,逐字相同** |
| `lines()` | **3 行,逐字相同** | **3 行,逐字相同** |
| 刷新 | 无独立函数(渲染在 `render` 里) | `refresh` 3 行 + `refresh_with` 19 行 |
| `handle_event` | **234 行** | **218 行** |
| `render` | 34 行 | 6 行 |
| `required_size` | — | 25 行 |
| 特有 | `zombie_selfheal` 21 行、`set_side`、`id()` | `with_layer`、`size_hint` 语义 |

**重复度实测**(方法:去注释/压空白后取 20 行滑动窗口,跳过内容过少的窗口):**73 个共享窗口**(体检报的 30 是更严格口径;两者都远大于 0,方向一致)。

### 2.2 两处「逐字相同」的重复(可直接搬)

```rust
/// 清空脏格 diff 状态(测试向不同 surface 渲染时需要重置)
pub fn reset_render_state(&mut self) { self.diff = Default::default(); }

/// 测试/诊断访问器
pub fn lines(&self) -> &[StyledLine] { &self.lines }
```

### 2.3 三处「骨架相同、语义不同」(抽时机,不抽语义)

| 片段 | panel | popup | 抽法 |
|---|---|---|---|
| 焦点移动 | `focusable` 为真才启用;Esc **取消焦点**回 onKey | 始终启用;Esc **关闭弹窗** | `JsSurface::move_focus(dir) -> bool`;Esc 留在各自 `handle_event` |
| 节点按键分发 + drain | 一样(消息 + `apply_ui_requests`) | 一样 | `JsSurface::dispatch_node_key(..) -> EventResult`(内部含 drain) |
| 粘贴 | 焦点在 input → 批量插入;否则 `Ignored` | 无 paste 分支 | `JsSurface::paste_input(..) -> Option<EventResult>` |
| 刷新渲染 | 在 `render` 里直接调 | `refresh_with` | `JsSurface::refresh(js, viewport, |id, focus| host.render_popup(..))` |

### 2.4 约束

- 两个组件都是 `Component`,`handle_event` 签名固定 `(&mut self, &Event, &mut Context) -> EventResult`。
- `render` 必须保持**脏格 diff** 行为(`DiffRenderer` 有状态,`reset_render_state` 语义不能变)。
- `Context` 里有 `js: Option<&mut JsHost>` 与 legacy 隐式解析两条路径,`refresh` 需同时支持(`Some` 透传 / `None` 走全局)。
- 集成测试是安全网:`plugin_panel`(2)/ `plugin_popup`(3)/ `plugin_panel_focus` / `window_mode`(16)/ `plugin_layout`(5)/ `plugin_input_multiline` / `plugin_paste`。

## 3. 目标架构

新文件 `helix-term/src/ui/js_surface.rs`(预估 150–190 行):

```rust
/// JS 组件的共享骨架:状态 + 刷新 + 节点键路由。
///
/// **不实现 `Component`** —— 放置/尺寸/关闭语义(panel 的 side、popup 的 layer/required_size)
/// 属于各自的容器,这层只管「一份 JS 组件状态怎么更新、怎么画、怎么把键送给节点」。
pub struct JsSurface {
    pub id: u64,
    pub focus: Option<String>,
    pub focusables: Vec<String>,
    pub lines: Vec<StyledLine>,
    diff: crate::ui::comp_layout::DiffRenderer,
}

impl JsSurface {
    pub fn new(id: u64) -> Self;
    pub fn reset_render_state(&mut self);
    pub fn lines(&self) -> &[StyledLine];

    /// 调 JS render → 抽 focusables → 布局成行。`render` 回调决定用哪个 JS 入口
    /// (popup 与 panel 目前都走 host.render_popup,差异只在 viewport 来源)。
    pub fn refresh(
        &mut self,
        js: Option<&mut helix_js::host::JsHost>,
        viewport: (u16, u16),
        render: impl FnOnce(&mut Option<&mut JsHost>, u64, Option<&str>, (u16,u16)) -> helix_js::JsResult<helix_js::Content>,
    );

    /// Tab / Shift-Tab:循环移动焦点。返回是否消费。
    pub fn move_focus(&mut self, forward: bool) -> bool;

    /// 焦点节点上的按键分发(含 drain:消息 + UI 请求即时应用)。
    /// 返回 `None` 表示该键不归节点处理,调用方继续自己的 onKey 逻辑。
    pub fn dispatch_node_key(&mut self, key: &PluginKey, cx: &mut Context) -> Option<EventResult>;

    /// 焦点在 input 时的粘贴(panel 用;popup 不调用)。
    pub fn paste_input(&mut self, contents: &str, cx: &mut Context) -> Option<EventResult>;

    /// 把 `lines` 画到 area(含脏格 diff)。
    pub fn render_lines(&mut self, area: Rect, surface: &mut Surface);
}

/// 各容器把自己的「特有语义」作为开关传进来,避免在共享层写 if panel/popup。
pub struct NodeKeyOpts {
    /// 多行 input 里的 Enter 是否插入换行(而不是触发 onKey)
    pub multiline_enter: bool,
    /// 节点按键消费后是否允许冒泡
    pub allow_bubble: bool,
}
```

两个组件保留自己的字段,只把共享状态换成一层:

```rust
pub struct PluginPanel { surface: JsSurface, side: PanelSide, focusable: bool }
pub struct PluginPopup { surface: JsSurface, size_hint: Option<(u16,u16)>, layer: Option<&'static str> }
```

`handle_event` 的形态(以 popup 为例,panel 同构):

```rust
fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
    match event {
        Event::Key(key) => {
            let Some(key) = key_to_plugin_key(key) else { return EventResult::Ignored(None) };
            if let Some(r) = self.surface.dispatch_node_key(&key, cx) { return r; }   // 共享段
            match key.name.as_str() {
                "Esc" => { /* popup: 关闭;panel: 取消焦点 */ }
                "Tab" => if self.surface.move_focus(true) { /* … */ }
                // … 各自 onKey 语义
            }
        }
        _ => EventResult::Ignored(None),
    }
}
```

## 4. 边界与错误处理

- **JS 报错**必须渲染成一行错误文本(`<plugin popup error: …>`),沿用现状,不 panic、不静默。
- **不得**在共享层引入新的 `job::dispatch_blocking`;drain 仍走现有 `apply_ui_requests`。
- **不得**改变 Esc / Tab / 粘贴的**任一分支结果**;若必须动,先补一条集成测试再动(铁律 2)。
- 共享层**不持有** `Component` 语义(`required_size` 留在 popup)。

## 5. 测试计划与完成判据

### 5.1 验证命令

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features integration -- -D warnings
cargo integration-test                       # 全量 377:焦点/键路由的回归会跨族出现,别只跑子集
cargo test -p helix-term --features integration --test integration plugin_panel
cargo test -p helix-term --features integration --test integration plugin_popup
cargo test -p helix-term --features integration --test integration window_mode
```

### 5.2 完成判据(数字)

| 指标 | 现状 | 目标 |
|---|---|---|
| `plugin_panel.rs` + `plugin_popup.rs` 合计 | **770 行** | **≤ 500 行** |
| 两者之间 20 行共享窗口 | **73** | **0**(除 `JsSurface` 调用点) |
| `handle_event` 最长 | 234 | **≤ 150** |
| 集成测试 | 377 passed | **377 passed / 0 failed** |
| 新增文件 | — | `ui/js_surface.rs`(150–190 行) |

### 5.3 单测

- `JsSurface::move_focus` 循环 + 空列表(直接单测,不经 Application)。
- `refresh` 的错误路径 → 单行错误文本。
- `render_lines` 的 diff 行为:同内容二次渲染不重绘(`reset_render_state` 前后各一次)。

## 6. 里程碑

| 阶段 | 内容 | 预估 |
|---|---|---|
| M1 | 建 `js_surface.rs` + `ErrorText`/`refresh`/`render_lines`,popup 接上(先只换状态与渲染) | 2–3 小时 |
| M2 | 抽 `dispatch_node_key`(含 drain),popup `handle_event` 缩身 | 2 小时 |
| M3 | panel 接同一套(注意 `focusable` 开关 + Esc 语义差异) | 2 小时 |
| M4 | 全量验证 + 记账;terminal 另开一项(G10b) | 1 小时 |

## 7. 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| Esc / Tab 语义被「统一」 | 面板关闭、焦点丢失,集成测试可能覆盖不到全部组合 | 语义留在各自 `handle_event`;先在 spec 里列表,再逐条对照改 |
| `Option<&mut JsHost>` 的借用 | 编译期就会挡住,不会静默 | 按现有 `refresh_with` 的透传模式,不改所有权模型(G11 才动) |
| 行数没降(抽了但要传很多参数) | 收益归零 | 若 M2 后 panel+popup > 600 行,停下来报告,不硬推 |
| terminal 被顺手带上 | 回归面翻倍 | 明确列为 G10b,独立 spec? 不需要 —— 第一批通过后再评估是否值得 |

---

**为什么现在能安全做**:G6 已把注册表拆成 `register/*`(JS 入口不再散落在 700 行里),G7 已把 UI 请求 handler 拆成 43 个函数(组件侧的「谁在改我」变清晰),D5/D6 修好的测试与引用配置让 `--ignored`、`--features integration` 都能当门禁用。
