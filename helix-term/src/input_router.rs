//! 输入层（Input Layer）：把"模式"统一成**有序栈** —— 一个路由点、一套提示、一套徽标。
//!
//! 设计见 `docs/superpowers/specs/2026-10-27-input-layer-design.md`。
//!
//! 现状（本文件落地的是"骨架 + 平级模式层 + 调试层"）：
//!
//! ```text
//! 栈顶  Transient(Ask)        命令行/picker（由 compositor 的 layers 决定，本文件只管徽标占位）
//!       Transient(DebugPaused) 会话暂停 ⇒ dap_keymap()，未绑定的键**让下层**
//!       PaneAction(PaneMode)   C-p/n/h/y/g ⇒ pane 键位，未绑定的键吞掉（Locked 全让）
//! 栈底  Editing(Mode)         永驻（keymap 按 Mode 取表，本文件不参与）
//! ```
//!
//! 之所以先做这三层：它们正是"散沙"最明显的部分（路由在 compositor 与 editor 各一处、
//! 提示各一套、`Esc` 语义各一套）。终端输入层（`TermInputMode`）与 `Esc` 统一是下一步。

use crate::commands::MappableCommand;
use crate::compositor::PaneMode;
use helix_view::input::{KeyCode, KeyEvent, KeyModifiers};

/// 本层的键位没命中时，未绑定的键怎么处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swallow {
    /// 吞掉（本层独占；平级模式默认如此）
    Consume,
    /// 继续问下层（调试层：`i`/`a`/`d`… 必须落到编辑器）
    Pass,
}

/// 层的种类。新增能力 = 加一个变体 + 一张键位表 + 一行徽标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    /// 命令行/搜索/picker 等瞬态层（compositor 的 `layers` 管键，这里只借它算徽标）
    Transient,
    /// 调试暂停接管
    DebugPaused,
    /// 平级模式（zellij）
    Pane(PaneMode),
    /// 终端输入（焦点在终端叶子且它在 Insert 直通态）
    TerminalInput,
    /// 编辑模式（栈底，永驻）
    Editing,
}

/// 一层：`(种类, 徽标, 未命中策略)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layer {
    pub kind: LayerKind,
    /// 状态栏徽标（空串 = 不显示）
    pub badge: &'static str,
    pub swallow: Swallow,
}

/// 从"当前状态"算出有序层栈（栈顶在前）。**徽标、提示、路由都从它推出来。**
pub fn layers(pane: PaneMode, debug_paused: bool) -> Vec<Layer> {
    layers_with_terminal(pane, debug_paused, false)
}

/// 同上，外加"焦点在终端的 Insert 直通态"。终端层**不参与本文件的路由**
/// （它的键由终端组件自己转给 PTY，见设计稿 §5 的偏差说明），只提供徽标。
pub fn layers_with_terminal(
    pane: PaneMode,
    debug_paused: bool,
    terminal_insert: bool,
) -> Vec<Layer> {
    let mut out = Vec::new();
    if terminal_insert {
        out.push(Layer {
            kind: LayerKind::TerminalInput,
            badge: "TERM",
            swallow: Swallow::Pass,
        });
    }
    if debug_paused {
        out.push(Layer {
            kind: LayerKind::DebugPaused,
            badge: "DEBUG",
            swallow: Swallow::Pass,
        });
    }
    if pane != PaneMode::Normal {
        out.push(Layer {
            kind: LayerKind::Pane(pane),
            badge: pane_badge(pane),
            // Locked 的语义就是"除 C-g 外全部放行"
            swallow: if pane == PaneMode::Locked {
                Swallow::Pass
            } else {
                Swallow::Consume
            },
        });
    }
    out.push(Layer {
        kind: LayerKind::Editing,
        badge: "",
        swallow: Swallow::Pass,
    });
    out
}

fn pane_badge(mode: PaneMode) -> &'static str {
    match mode {
        PaneMode::Normal => "",
        PaneMode::Locked => "LOCKED",
        PaneMode::Pane => "PANE",
        PaneMode::Resize => "RESIZE",
        PaneMode::Move => "MOVE",
        PaneMode::Scroll => "SCROLL",
    }
}

/// 状态栏徽标串：`DEBUG│PANE`（从栈顶往下，跳过无徽标的层）。
pub fn badges(pane: PaneMode, debug_paused: bool) -> String {
    badges_with_terminal(pane, debug_paused, false)
}

/// 同上，带终端直通态。
pub fn badges_with_terminal(pane: PaneMode, debug_paused: bool, terminal_insert: bool) -> String {
    layers_with_terminal(pane, debug_paused, terminal_insert)
        .iter()
        .map(|l| l.badge)
        .filter(|b| !b.is_empty())
        .collect::<Vec<_>>()
        .join("│")
}

/// 一次按键在层栈上的**决策**（纯函数，不改状态；执行留给 compositor）。
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// 前缀键：进入/切换某个平级模式（同一个键再按一次 = 退出，zellij 语义）
    Toggle(PaneMode),
    /// 平级模式内的按键：交给 compositor 的分发器，`consumed=false` 表示放行给叶子
    PaneKey(PaneMode, bool),
    /// 栈顶是 `Esc`：弹掉栈顶层（平级模式 → Normal）
    Pop,
    /// 调试层的命令（已解析好）
    Debug(MappableCommand),
    /// 终端直通层：键原样交给终端叶子（Router 只做"归它"的判定）
    Terminal,
    /// 本层不管，继续往下（最终落到叶子/编辑器）
    Pass,
}

/// 前缀键 → 平级模式。
fn prefix_mode(key: &KeyEvent) -> Option<PaneMode> {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    match key.code {
        KeyCode::Char('g') => Some(PaneMode::Locked),
        KeyCode::Char('p') => Some(PaneMode::Pane),
        KeyCode::Char('n') => Some(PaneMode::Resize),
        KeyCode::Char('h') => Some(PaneMode::Move),
        KeyCode::Char('y') => Some(PaneMode::Scroll),
        _ => None,
    }
}

/// **唯一的路由决策点**：给定当前层栈状态与一次按键，决定它归谁。
///
/// 规则（与文档 §2 一一对应）：
/// 1. 平级模式**在栈上**时：前缀键 = 切换/退出；`Esc` = 弹层；其余键归该模式
///    （`Locked` 例外：除 `C-g` 外全部放行）。
/// 2. 平级模式不在栈上时：`Ctrl` 前缀键进模式（`C-g` 永不豁免；其余前缀在
///    Insert / 终端直通 / 输入态层前让路 —— 这是既有语义，写在 `allow_prefix` 里）。
/// 3. 调试层：查 `dap_keymap()`（未命中 ⇒ `Pass`，让编辑照旧）。
///
/// `allow_prefix` 由调用方给（它需要读 `editor.mode()` 与 compositor 的 layers），
/// 保持本函数**纯**：只做决策，不摸 `&mut`。
pub fn plan(pane: PaneMode, debug_paused: bool, key: KeyEvent, allow_prefix: bool) -> Plan {
    plan_layered(pane, debug_paused, false, key, allow_prefix)
}

/// 同上，外加"焦点在终端直通态"。终端层在调试/平级**之下**、编辑之上：
/// 直通时键归 PTY（`C-g` 是唯一例外），调试键与平级前缀都不抢。
pub fn plan_layered(
    pane: PaneMode,
    debug_paused: bool,
    terminal_insert: bool,
    key: KeyEvent,
    allow_prefix: bool,
) -> Plan {
    // ⓪ 终端直通
    if terminal_insert {
        if matches!(prefix_mode(&key), Some(PaneMode::Locked)) {
            return Plan::Toggle(PaneMode::Locked);
        }
        return Plan::Terminal;
    }

    // ① 平级模式在栈上
    if pane != PaneMode::Normal {
        if pane == PaneMode::Locked {
            // Locked：`C-g` 退出，其余全部放行
            return match prefix_mode(&key) {
                Some(PaneMode::Locked) => Plan::Toggle(PaneMode::Locked),
                _ => Plan::PaneKey(PaneMode::Locked, false),
            };
        }
        if let Some(m) = prefix_mode(&key) {
            return Plan::Toggle(m);
        }
        if matches!(key.code, KeyCode::Esc) {
            return Plan::Pop;
        }
        return Plan::PaneKey(pane, true);
    }

    // ② 不在平级模式：前缀键进模式（`C-g` 不受 allow_prefix 限制）
    if let Some(m) = prefix_mode(&key) {
        if m == PaneMode::Locked || allow_prefix {
            return Plan::Toggle(m);
        }
    }

    // ③ 调试层
    if debug_paused {
        if let Some(cmd) = debug_key(key) {
            return Plan::Debug(cmd);
        }
    }

    Plan::Pass
}

/// 当前平级模式的一份**全局镜像**（compositor 每帧写入）。
///
/// 为什么需要：`:keys` 这类 typed 命令只有 `&mut Editor`，拿不到 `Compositor::pane_mode()`。
static CUR_PANE_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_current_pane_mode(mode: PaneMode) {
    use crate::compositor::PaneMode as M;
    let n = match mode {
        M::Normal => 0,
        M::Locked => 1,
        M::Pane => 2,
        M::Resize => 3,
        M::Move => 4,
        M::Scroll => 5,
    };
    CUR_PANE_MODE.store(n, std::sync::atomic::Ordering::Relaxed);
}

pub fn current_pane_mode() -> PaneMode {
    use crate::compositor::PaneMode as M;
    match CUR_PANE_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => M::Locked,
        2 => M::Pane,
        3 => M::Resize,
        4 => M::Move,
        5 => M::Scroll,
        _ => M::Normal,
    }
}

/// 当前应当显示的提示（栈顶**有提示表**的那一层）：`(标题, 条目)`。
///
/// 调试层优先（它在栈顶）：标题 `Debug`，条目来自 `dap_keymap()`；否则看平级模式。
pub fn hints(pane: PaneMode, debug_paused: bool) -> Option<(String, Vec<(String, String)>)> {
    // **与 plan() 同序**：平级模式在栈上时键先归它 ⇒ 提示也先显示它；
    // 没有平级模式时才轮到调试层。（早先写反了：暂停中进 Pane 模式会显示 Debug 表。）
    if pane != PaneMode::Normal {
        let (title, entries) = pane_hints(pane);
        return Some((
            title.to_string(),
            entries
                .into_iter()
                .map(|(k, d, _)| (k.to_string(), d.to_string()))
                .collect(),
        ));
    }
    if debug_paused {
        let tree = crate::keymap::default::debug_keymap();
        if let crate::keymap::KeyTrie::Node(node) = tree.as_ref().clone() {
            let entries: Vec<(String, String)> = node.entries().into_iter().collect();
            if !entries.is_empty() {
                return Some(("Debug".to_string(), entries));
            }
        }
    }
    None
}

/// 平级模式的键位表（**唯一来源**）：内置提示与 `helix.pane_mode.keymap()` 都从这里取。
pub fn pane_hints(mode: PaneMode) -> (&'static str, Vec<(&'static str, &'static str, bool)>) {
    match mode {
        PaneMode::Normal => ("Normal", Vec::new()),
        PaneMode::Locked => (
            "C-g",
            vec![("C-g", "退出 locked(其余键原样交给当前窗口)", true)],
        ),
        PaneMode::Pane => (
            "C-p",
            vec![
                ("h j k l", "聚焦(左/下/上/右)", true),
                ("H J K L", "交换", true),
                ("p / P / Tab", "切到下一个/上一个窗口", true),
                ("n / d / r", "新分屏 / 下分 / 右分", true),
                ("x", "关闭窗口", true),
                ("f", "最大化/还原", true),
                ("z", "最小化/还原", true),
                ("w / e", "浮动 / 收回平铺", true),
                ("i", "pin(浮动时置顶)", true),
                ("s", "堆叠(与兄弟窗合并)", true),
                ("Esc / C-p", "退出", true),
            ],
        ),
        PaneMode::Resize => (
            "C-n",
            vec![
                ("h j k l", "向该方向增大(浮窗:改尺寸)", true),
                ("H J K L", "向该方向减小", true),
                ("= / - / +", "宽度 ±5%", true),
                ("Esc / C-n", "退出", true),
            ],
        ),
        PaneMode::Move => (
            "C-h",
            vec![
                ("h j k l", "与方向邻居交换(浮窗:搬位置)", true),
                ("Esc / C-h", "退出", true),
            ],
        ),
        PaneMode::Scroll => (
            "C-y",
            vec![
                ("j / k", "行滚动", true),
                ("d / u", "半页", true),
                ("C-f C-b / h l", "整页", true),
                ("Esc / C-y", "退出", true),
            ],
        ),
    }
}

/// 调试层：把一次按键解析成 `dap_keymap()` 里的命令。
///
/// **SHIFT 兜底**：终端送大写字母是 `Char('T') + SHIFT`，而键位表里 `"T"` 编码成
/// `Char('T')`（无修饰）—— `KeyEvent` 的 `Hash/Eq` 比修饰位，直接查会"按了没反应"。
pub fn debug_key(event: KeyEvent) -> Option<MappableCommand> {
    // 用**运行时生效**的表（含用户 `[keys.debug]` 覆盖），不是内置表
    let tree = crate::keymap::default::debug_keymap();
    let lookup = |ev: &KeyEvent| match tree.search(std::slice::from_ref(ev)) {
        Some(crate::keymap::KeyTrie::MappableCommand(cmd)) => Some(cmd.clone()),
        _ => None,
    };
    if let Some(cmd) = lookup(&event) {
        return Some(cmd);
    }
    if matches!(event.code, KeyCode::Char(_)) {
        let mut alt = event;
        alt.modifiers.toggle(KeyModifiers::SHIFT);
        return lookup(&alt);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 徽标顺序 = 栈序（调试在平级之上）。
    #[test]
    fn badges_follow_stack_order() {
        assert_eq!(badges(PaneMode::Normal, false), "");
        assert_eq!(badges(PaneMode::Normal, true), "DEBUG");
        assert_eq!(badges(PaneMode::Pane, false), "PANE");
        assert_eq!(badges(PaneMode::Pane, true), "DEBUG│PANE");
        assert_eq!(badges(PaneMode::Locked, true), "DEBUG│LOCKED");
    }

    /// 未命中策略：调试层放行、平级模式吞掉、Locked 放行。
    #[test]
    fn swallow_policy_per_layer() {
        let l = layers(PaneMode::Pane, true);
        assert_eq!(l[0].kind, LayerKind::DebugPaused);
        assert_eq!(l[0].swallow, Swallow::Pass, "调试层必须放行未绑定的键");
        assert_eq!(l[1].swallow, Swallow::Consume, "平级模式独占");
        assert_eq!(layers(PaneMode::Locked, false)[0].swallow, Swallow::Pass);
        assert_eq!(
            layers(PaneMode::Normal, false).last().unwrap().kind,
            LayerKind::Editing,
            "编辑模式永远在栈底"
        );
    }

    /// 每个平级模式都有提示表（否则状态栏/which-key 会有空档）。
    #[test]
    fn every_pane_mode_has_hints() {
        for m in [
            PaneMode::Locked,
            PaneMode::Pane,
            PaneMode::Resize,
            PaneMode::Move,
            PaneMode::Scroll,
        ] {
            let (title, entries) = pane_hints(m);
            assert!(!title.is_empty());
            assert!(!entries.is_empty(), "{m:?} 缺提示条目");
        }
    }

    /// 路由决策：前缀键、Esc 弹层、Locked 放行、调试层、Pass。
    #[test]
    fn plan_routing_rules() {
        use helix_view::input::{KeyEvent, KeyModifiers};
        let ctrl_p = KeyEvent {
            code: KeyCode::Char('p'),
            modifiers: KeyModifiers::CONTROL,
        };
        let ctrl_g = KeyEvent {
            code: KeyCode::Char('g'),
            modifiers: KeyModifiers::CONTROL,
        };
        let esc = KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
        };
        let n = KeyEvent {
            code: KeyCode::Char('n'),
            modifiers: KeyModifiers::NONE,
        };
        let i = KeyEvent {
            code: KeyCode::Char('i'),
            modifiers: KeyModifiers::NONE,
        };

        // 不在模式：前缀键进模式（C-g 不受 allow_prefix 限制）
        assert_eq!(
            plan(PaneMode::Normal, false, ctrl_p, true),
            Plan::Toggle(PaneMode::Pane)
        );
        assert_eq!(
            plan(PaneMode::Normal, false, ctrl_p, false),
            Plan::Pass,
            "Insert/输入层里让路"
        );
        assert_eq!(
            plan(PaneMode::Normal, false, ctrl_g, false),
            Plan::Toggle(PaneMode::Locked)
        );

        // 在模式：同一个键退出、Esc 弹层、其它键归该模式
        assert_eq!(
            plan(PaneMode::Pane, false, ctrl_p, true),
            Plan::Toggle(PaneMode::Pane)
        );
        assert_eq!(plan(PaneMode::Pane, false, esc, true), Plan::Pop);
        assert_eq!(
            plan(PaneMode::Pane, false, n, true),
            Plan::PaneKey(PaneMode::Pane, true)
        );

        // Locked：除 C-g 外全部放行
        assert_eq!(
            plan(PaneMode::Locked, false, n, true),
            Plan::PaneKey(PaneMode::Locked, false)
        );
        assert_eq!(
            plan(PaneMode::Locked, false, ctrl_g, true),
            Plan::Toggle(PaneMode::Locked)
        );

        // 调试层：命中的键是 Debug，未命中（i）落到 Pass
        match plan(PaneMode::Normal, true, n, true) {
            Plan::Debug(_) => {}
            other => panic!("暂停时 n 应是 Debug 命令，实际 {other:?}"),
        }
        assert_eq!(
            plan(PaneMode::Normal, true, i, true),
            Plan::Pass,
            "i 必须落到编辑器(插入)"
        );
    }

    /// 终端直通层：除 C-g 外全部归终端；调试/平级层不抢。
    #[test]
    fn plan_terminal_layer() {
        use helix_view::input::{KeyEvent, KeyModifiers};
        let n = KeyEvent {
            code: KeyCode::Char('n'),
            modifiers: KeyModifiers::NONE,
        };
        let ctrl_g = KeyEvent {
            code: KeyCode::Char('g'),
            modifiers: KeyModifiers::CONTROL,
        };
        let ctrl_p = KeyEvent {
            code: KeyCode::Char('p'),
            modifiers: KeyModifiers::CONTROL,
        };
        assert_eq!(
            plan_layered(PaneMode::Normal, true, true, n, true),
            Plan::Terminal,
            "终端直通时 n 必须归终端（即使调试暂停）"
        );
        assert_eq!(
            plan_layered(PaneMode::Normal, false, true, ctrl_p, true),
            Plan::Terminal,
            "直通时 C-p 也让路（与旧行为一致）"
        );
        assert_eq!(
            plan_layered(PaneMode::Normal, false, true, ctrl_g, true),
            Plan::Toggle(PaneMode::Locked),
            "C-g 是唯一例外"
        );
    }

    /// 提示顺序与 plan() 一致：平级模式优先，没进模式才显示调试层。
    #[test]
    fn hints_follow_routing_order() {
        let (title, entries) = hints(PaneMode::Normal, true).expect("暂停时应显示调试表");
        assert_eq!(title, "Debug");
        assert!(!entries.is_empty());
        let (title2, _) = hints(PaneMode::Resize, false).expect("Resize 应有提示");
        assert_eq!(title2, "C-n");
        // 暂停 + 在 Pane 模式：键先归 Pane ⇒ 提示也是 Pane
        let (title3, _) = hints(PaneMode::Pane, true).expect("应有提示");
        assert_eq!(title3, "C-p", "此刻键归平级模式，提示必须跟着它");
        assert!(hints(PaneMode::Normal, false).is_none());
    }

    /// 终端直通态的徽标（终端键仍由组件自己转给 PTY）。
    #[test]
    fn terminal_layer_badge() {
        assert_eq!(badges_with_terminal(PaneMode::Normal, false, true), "TERM");
        assert_eq!(
            badges_with_terminal(PaneMode::Pane, false, true),
            "TERM│PANE"
        );
    }

    /// 调试层查表：含 SHIFT 翻转兜底。
    #[test]
    fn debug_key_handles_shift() {
        use helix_view::input::KeyEvent;
        let t = KeyEvent {
            code: KeyCode::Char('T'),
            modifiers: KeyModifiers::SHIFT,
        };
        assert!(debug_key(t).is_some(), "大写 T(带 SHIFT) 必须能查到");
        let n = KeyEvent {
            code: KeyCode::Char('n'),
            modifiers: KeyModifiers::NONE,
        };
        assert!(debug_key(n).is_some(), "n 必须能查到");
        let i = KeyEvent {
            code: KeyCode::Char('i'),
            modifiers: KeyModifiers::NONE,
        };
        assert!(debug_key(i).is_none(), "i 不在调试表里 ⇒ 落到编辑器(插入)");
    }
}
