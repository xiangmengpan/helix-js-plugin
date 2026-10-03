//! 原生终端视图:PTY 输出交给 alacritty_terminal 引擎解析,渲染到 tui surface。
//!
//! `TerminalGrid` 是引擎(alacritty_terminal,与 zellij 的屏幕仿真同源)的薄封装,
//! `TerminalCell` 退化为取屏时的渲染视图;`PluginTerminal` 是 compositor 层:
//! 按键直通 pty、尺寸变化实时 TIOCSWINSZ、Esc 关闭(Drop 时杀 pty)。
//!
//! 引擎不自己写 pty:DA/DSR/OSC 查询的答复经 `TermSink` 回转,由本层写回(见 `take_replies`)。

use std::sync::{Arc, Mutex, MutexGuard};

use alacritty_terminal::event::{Event as EngineEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Grid as EngineGrid, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell as EngineCell, Flags};
use alacritty_terminal::term::{
    ClipboardType as EngineClipboard, Config as TermConfig, Term, TermMode as EngineMode,
};
use alacritty_terminal::vte::ansi::{Color as EngineColor, NamedColor, Processor, StdSyncHandler};

use helix_view::graphics::{Color, Modifier, Rect, Style, UnderlineStyle};
use tui::buffer::Buffer as Surface;

use crate::compositor::{Component, Compositor, Context, Event, EventResult};

/// 单个终端网格单元的渲染视图:字符 + 颜色 + 属性 + 显示宽度。
/// 宽字符(CJK):主格 width=2,后续格 width=0(render 跳过、背景连续)。
/// 引擎是唯一真源,本类型只在取屏时按需构造。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCell {
    pub ch: char,
    /// 显示宽度:0 = 被前一个宽字符占用的占位格,1 = 普通,2 = 宽字符主格
    pub width: u8,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub strikethrough: bool,
}

impl Default for TerminalCell {
    fn default() -> Self {
        Self {
            ch: ' ',
            width: 1,
            fg: None,
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            inverse: false,
            strikethrough: false,
        }
    }
}

/// 引擎历史缓冲上限(可滚回行数)
const SCROLLBACK_MAX: usize = 1000;

/// 引擎 → 宿主的回调出口。
///
/// 引擎对 DA/DSR/OSC 查询的**答复不是自己写的**:它以 `PtyWrite` 事件交给宿主,
/// 由宿主写进 pty(忽略它,那些"先问终端再决定"的 TUI 会卡)。OSC 52 写剪贴板
/// 经 `ClipboardStore` 排队,由宿主接系统剪贴板。
/// `EventListener::send_event` 取 `&self`,故用 `Arc<Mutex<..>>` 做内部可变性
/// (该 trait 本身无 `Send + Sync` 约束,但 `PluginTerminal` 要经 `job::dispatch_blocking`
/// 的 `Send` 闭包构造,所以不能用 `Rc<RefCell<..>>`)。
#[derive(Clone, Default)]
struct TermSink {
    title: Arc<Mutex<Option<String>>>,
    replies: Arc<Mutex<Vec<u8>>>,
    clipboard: Arc<Mutex<Vec<(char, String)>>>,
}

/// 取锁但不容忍中毒:send_event 在解析中途被调,panic 掉锁会连带毁掉后续所有输出。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl EventListener for TermSink {
    fn send_event(&self, event: EngineEvent) {
        match event {
            EngineEvent::Title(t) => *lock(&self.title) = Some(t),
            EngineEvent::ResetTitle => *lock(&self.title) = None,
            EngineEvent::PtyWrite(s) => lock(&self.replies).extend_from_slice(s.as_bytes()),
            EngineEvent::ClipboardStore(kind, text) => {
                // OSC 52 的选区要保留:clipboard → '+'，primary → '*'
                let reg = match kind {
                    EngineClipboard::Selection => '*',
                    EngineClipboard::Clipboard => '+',
                };
                lock(&self.clipboard).push((reg, text));
            }
            // ColorRequest / TextAreaSizeRequest / ClipboardLoad:已知未接(见设计文档)
            _ => {}
        }
    }
}

/// 引擎尺寸:可见区 + 历史。
/// **必须给历史**:否则 reflow 溢出的内容会被直接丢弃(探路实测过)。
#[derive(Clone, Copy)]
struct GridDims {
    cols: usize,
    rows: usize,
    history: usize,
}

impl Dimensions for GridDims {
    fn total_lines(&self) -> usize {
        self.rows + self.history
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

fn engine_config(history: usize) -> TermConfig {
    TermConfig {
        scrolling_history: history,
        ..TermConfig::default()
    }
}

/// 终端网格(引擎支撑)。
///
/// 公开方法与旧手写实现同形,调用方(PluginTerminal / 单测)基本不用改;
/// 差别:`cell()` 返回拥有值而不是引用(引擎的 `Cell` 是另一种类型)。
pub struct TerminalGrid {
    term: Term<TermSink>,
    processor: Processor<StdSyncHandler>,
    dims: GridDims,
    cols: u16,
    rows: u16,
    sink: TermSink,
    /// 最近一次 OSC 0/2 标题(持久;`get_component_state` 的 title 字段)
    title: String,
    /// 已到达但尚未被 `take_title()` 取走的标题(钩子消费用)
    pending_title: Option<String>,
}

impl TerminalGrid {
    pub fn new(rows: u16, cols: u16) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let dims = GridDims {
            cols: cols as usize,
            rows: rows as usize,
            history: SCROLLBACK_MAX,
        };
        let sink = TermSink::default();
        let term = Term::new(engine_config(dims.history), &dims, sink.clone());
        Self {
            term,
            processor: Processor::new(),
            dims,
            cols,
            rows,
            sink,
            title: String::new(),
            pending_title: None,
        }
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// 可见区内光标位置(0-based)。已随滚回偏移折算并夹取到可见区。
    pub fn cursor(&self) -> (u16, u16) {
        let content = self.term.renderable_content();
        let off = content.display_offset as i32;
        let p = content.cursor.point;
        let row = (p.line.0 + off).clamp(0, self.rows as i32 - 1) as u16;
        let col = (p.column.0 as u16).min(self.cols - 1);
        (row, col)
    }

    /// 可见区某格(0-based)。宽字符占位格返回 width=0 的视图。
    pub fn cell(&self, row: u16, col: u16) -> Option<TerminalCell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        let grid = self.term.grid();
        let line = Line(row as i32 - grid.display_offset() as i32);
        Some(cell_view(&grid[line][Column(col as usize)]))
    }

    pub fn in_alt(&self) -> bool {
        self.term.mode().contains(EngineMode::ALT_SCREEN)
    }

    /// 引擎当前模式位(写侧判断依据:鼠标上报/括号粘贴/应用光标键等)
    pub fn engine_mode(&self) -> EngineMode {
        *self.term.mode()
    }

    /// 历史缓冲已有行数
    pub fn scrollback_len(&self) -> usize {
        self.term.grid().history_size()
    }

    /// 当前滚回偏移(0 = 显示活动区)
    pub fn scroll_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub fn set_scroll_offset(&mut self, offset: usize) {
        // 引擎的 `Scroll::Delta(+n)` 是**增大**偏移(向上滚入历史),
        // 所以由当前偏移走到目标偏移要传 `offset - cur`,不是反过来
        let cur = self.term.grid().display_offset() as i32;
        let delta = offset as i32 - cur;
        if delta != 0 {
            self.term.grid_mut().scroll_display(Scroll::Delta(delta));
        }
    }

    pub fn scroll_up_view(&mut self, n: usize) {
        self.term.grid_mut().scroll_display(Scroll::Delta(n as i32));
    }

    pub fn scroll_down_view(&mut self, n: usize) {
        self.term
            .grid_mut()
            .scroll_display(Scroll::Delta(-(n as i32)));
    }

    pub fn has_scrollback(&self) -> bool {
        self.term.grid().history_size() > 0
    }

    /// 取走最近一次 OSC 0/2 标题(取后清空,钩子消费用;pull 语义)
    pub fn take_title(&mut self) -> Option<String> {
        self.pending_title.take()
    }

    /// 最近一次标题(持久;`get_component_state` 用)
    pub fn title(&self) -> String {
        self.title.clone()
    }

    /// 取走引擎要求写回 pty 的字节(DA/DSR/OSC 查询答复)
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut *lock(&self.sink.replies))
    }

    /// 取走引擎收到的 OSC 52 剪贴板写入请求(选区寄存器字符 + 文本)
    pub fn take_clipboard(&mut self) -> Vec<(char, String)> {
        std::mem::take(&mut *lock(&self.sink.clipboard))
    }

    /// 喂一块 PTY 输出(字节;多字节 UTF-8 跨块安全)
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
        // 标题由引擎经事件 push 出来:在此汇入持久字段与 pending 槽
        if let Some(t) = lock(&self.sink.title).take() {
            self.title = t.clone();
            self.pending_title = Some(t);
        }
    }

    /// 清屏并复位解析器(引擎无 in-place 清屏 API,重建;尺寸与历史配置保留)
    pub fn clear(&mut self) {
        self.term = Term::new(
            engine_config(self.dims.history),
            &self.dims,
            self.sink.clone(),
        );
        self.processor = Processor::new();
        let _ = self.take_replies();
    }

    /// 改尺寸(引擎带 reflow:长行重排,而不是截断销毁)
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        self.dims = GridDims {
            cols: cols as usize,
            rows: rows as usize,
            history: SCROLLBACK_MAX,
        };
        self.term.resize(self.dims);
    }

    /// 把可见区画到 surface
    pub fn render(&self, area: Rect, surface: &mut Surface) {
        let content = self.term.renderable_content();
        let off = content.display_offset as i32;
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + off;
            if row < 0 || row >= area.height as i32 {
                continue;
            }
            let col = indexed.point.column.0 as u16;
            if col >= area.width {
                continue;
            }
            let view = cell_view(indexed.cell);
            if view.width == 0 {
                continue; // 宽字符占位格:保持背景连续
            }
            let Some(sc) = surface.get_mut(area.x + col, area.y + row as u16) else {
                continue;
            };
            let mut style = Style::default();
            if view.bold {
                style.add_modifier |= Modifier::BOLD;
            }
            if view.dim {
                style.add_modifier |= Modifier::DIM;
            }
            if view.italic {
                style.add_modifier |= Modifier::ITALIC;
            }
            if view.inverse {
                style.add_modifier |= Modifier::REVERSED;
            }
            if view.strikethrough {
                style.add_modifier |= Modifier::CROSSED_OUT;
            }
            if view.underline {
                style.underline_style = Some(UnderlineStyle::Line);
            }
            style.underline_color = indexed.cell.underline_color().and_then(map_color);
            style.fg = view.fg;
            style.bg = view.bg;
            // 组合字符(零宽)跟在基字符后一并写格
            let mut symbol = String::new();
            symbol.push(view.ch);
            if let Some(zw) = indexed.cell.zerowidth() {
                symbol.extend(zw.iter());
            }
            sc.set_symbol(&symbol);
            sc.set_style(style);
        }
    }

    /// 可视区纯文本(含滚回偏移带入的行;跳过宽字符占位格、去行尾空白)
    pub fn visible_text(&self) -> String {
        let grid = self.term.grid();
        let off = grid.display_offset() as i32;
        let mut out = String::new();
        for row in 0..self.rows as i32 {
            out.push_str(&row_text(grid, Line(row - off), self.cols as usize));
            out.push('\n');
        }
        out
    }

    /// 全部内容纯文本(历史 + 可见区)
    pub fn full_text(&self) -> String {
        let grid = self.term.grid();
        let mut out = String::new();
        for l in grid.topmost_line().0..=grid.bottommost_line().0 {
            out.push_str(&row_text(grid, Line(l), self.cols as usize));
            out.push('\n');
        }
        out
    }
}

/// 引擎单元 → 渲染视图
fn cell_view(c: &EngineCell) -> TerminalCell {
    let f = c.flags;
    // 引擎用字面 '\t' 单元格作制表标记(本身不代表一个可见字符):
    // 渲染上它就是空格,否则 surface 会被写入真制表符而错位。
    let ch = if c.c == '\t' { ' ' } else { c.c };
    TerminalCell {
        ch,
        width: if f.contains(Flags::WIDE_CHAR) {
            2
        } else if f.contains(Flags::WIDE_CHAR_SPACER) {
            0
        } else {
            1
        },
        fg: map_color(c.fg),
        bg: map_color(c.bg),
        bold: f.contains(Flags::BOLD),
        dim: f.contains(Flags::DIM),
        italic: f.contains(Flags::ITALIC),
        underline: f.contains(Flags::UNDERLINE),
        inverse: f.contains(Flags::INVERSE),
        strikethrough: f.contains(Flags::STRIKEOUT),
    }
}

/// 一行的纯文本(去行尾空白;宽字符占位格跳过;组合字符带上)
fn row_text(grid: &EngineGrid<EngineCell>, line: Line, cols: usize) -> String {
    let mut s = String::new();
    for col in 0..cols {
        let c = &grid[line][Column(col)];
        if c.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        // 制表标记按空格展开:与“所见即所得”一致(复制/导出不再出现真制表符)
        s.push(if c.c == '\t' { ' ' } else { c.c });
        if let Some(zw) = c.zerowidth() {
            s.extend(zw.iter());
        }
    }
    s.trim_end().to_string()
}

/// 引擎颜色 → 主题颜色。
///
/// 默认前景/背景/光标(`Foreground`/`Background`/`Cursor`/`BrightForeground`/`DimForeground`)
/// 映射为 `None`,让 helix 主题的默认色生效;调色板 16 色按 ANSI 语义映射
/// (索引 7 = 白 → `Gray`,8 = 亮黑 → `LightGray`)。
fn map_color(c: EngineColor) -> Option<Color> {
    Some(match c {
        EngineColor::Named(n) => {
            use NamedColor::*;
            match n {
                Black | DimBlack => Color::Black,
                Red | DimRed => Color::Red,
                Green | DimGreen => Color::Green,
                Yellow | DimYellow => Color::Yellow,
                Blue | DimBlue => Color::Blue,
                Magenta | DimMagenta => Color::Magenta,
                Cyan | DimCyan => Color::Cyan,
                White | DimWhite => Color::Gray,
                BrightBlack => Color::LightGray,
                BrightRed => Color::LightRed,
                BrightGreen => Color::LightGreen,
                BrightYellow => Color::LightYellow,
                BrightBlue => Color::LightBlue,
                BrightMagenta => Color::LightMagenta,
                BrightCyan => Color::LightCyan,
                BrightWhite => Color::White,
                // 默认色 / 光标色:交给主题
                Foreground | Background | Cursor | BrightForeground | DimForeground => return None,
            }
        }
        EngineColor::Indexed(i) => Color::Indexed(i),
        EngineColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    })
}

/// 鼠标按钮 → X10 按钮码
fn btn_code(b: helix_view::input::MouseButton) -> u16 {
    use helix_view::input::MouseButton;
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

/// X10 风格鼠标编码的单字节码位。
/// 1005(UTF-8)模式直接按码位编;否则必须落在 ASCII 内——超出就放弃(不发出错的报告)。
fn push_x10(out: &mut String, v: u16, utf8: bool) -> Option<()> {
    if utf8 {
        out.push(char::from_u32(v as u32)?);
        Some(())
    } else if v < 0x80 {
        out.push(v as u8 as char);
        Some(())
    } else {
        None
    }
}

/// 把按键转成发给 pty 的字节序列。
/// 支持：普通字符、Ctrl 组合（C-a → \x01 等）、Enter/Backspace/Tab、
/// 方向键/Home/End/PageUp/PageDown/Delete/Insert（xterm 应序）。
fn key_to_term_bytes(key: &helix_view::input::KeyEvent, mode: EngineMode) -> Option<String> {
    use helix_view::input::{KeyCode, KeyModifiers};
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // 应用光标键模式(DECCKM,`?1h`):方向键/Home/End 发 SS3(`\eOA`)而不是 CSI(`\e[A`)。
    // vim/less 开了该模式后期望 SS3。
    let app = mode.contains(EngineMode::APP_CURSOR);
    let csi = |c: char| {
        if app {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
        }
    };
    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                // Ctrl 组合：C-a..C-z → \x01..\x1a；C-\ → 由 handle_event 拦截不在此
                let b = (c.to_ascii_lowercase() as u8) & 0x1f;
                if (1..=26).contains(&b) {
                    Some((b as char).to_string())
                } else {
                    None
                }
            } else {
                Some(c.to_string())
            }
        }
        KeyCode::Enter => Some("\r".into()),
        KeyCode::Backspace => Some("\u{7f}".into()),
        KeyCode::Tab => Some("\t".into()),
        KeyCode::Up => Some(csi('A')),
        KeyCode::Down => Some(csi('B')),
        KeyCode::Right => Some(csi('C')),
        KeyCode::Left => Some(csi('D')),
        KeyCode::Home => Some(csi('H')),
        KeyCode::End => Some(csi('F')),
        KeyCode::PageUp => Some("\x1b[5~".into()),
        KeyCode::PageDown => Some("\x1b[6~".into()),
        KeyCode::Delete => Some("\x1b[3~".into()),
        KeyCode::Insert => Some("\x1b[2~".into()),
        _ => None,
    }
}

/// 按键描述（term-key 钩子的 key.code 字符串 + 修饰键标志）
fn terminal_key_desc(key: &helix_view::input::KeyEvent) -> (String, bool, bool, bool) {
    use helix_view::input::{KeyCode, KeyModifiers};
    let code = match key.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Insert => "insert".to_string(),
        KeyCode::F(n) => format!("f{n}"),
        _ => format!("{:?}", key.code),
    };
    (
        code,
        key.modifiers.contains(KeyModifiers::SHIFT),
        key.modifiers.contains(KeyModifiers::CONTROL),
        key.modifiers.contains(KeyModifiers::ALT),
    )
}

/// 原生终端面板层：vte 网格渲染 + 按键直通 pty + 尺寸联动 TIOCSWINSZ。
/// Esc 关闭：移除层 → Drop → term_kill（杀 pty）。
/// 注意：compositor 的面板推挤只识别 PluginPanel，本层目前整面渲染
/// （覆盖编辑器区域）——多类型面板布局留给合并/后续任务。
/// 终端显示模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermMode {
    Dock,
    Fullscreen,
    Floating,
    Minimized,
}

impl TermMode {
    pub(crate) fn from_str(s: &str) -> Option<Self> {
        match s {
            "dock" => Some(Self::Dock),
            "fullscreen" => Some(Self::Fullscreen),
            "floating" => Some(Self::Floating),
            "minimized" => Some(Self::Minimized),
            _ => None,
        }
    }
}

/// 终端输入模式（lazyvim 语义）：Insert = 键直通 pty；Normal = 终端内导航（滚动缓冲）。
/// C-\ 切换：Insert → Normal（滚动查看）；Normal → 回 helix（聚焦编辑器）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermInputMode {
    Insert,
    Normal,
}

pub struct PluginTerminal {
    /// find_id 路由用的静态 id（每实例 Box::leak，组件生命周期同层）
    id: &'static str,
    view_id: u64,
    pty_id: u64,
    grid: TerminalGrid,
    size: u16,
    mode: TermMode,
    input_mode: TermInputMode,
    /// 上次渲染尺寸（None = 尚未渲染；首次渲染触发 resize + TIOCSWINSZ）
    last_size: Option<(u16, u16)>,
    /// 上次渲染区域(鼠标坐标换算用:组件不持有 area,只能由 render 记录)
    last_area: Option<Rect>,
    /// 标题条脏格 diff 渲染器(JS 视图层内容只重绘变化格)
    diff: crate::ui::comp_layout::DiffRenderer,
}

impl PluginTerminal {
    pub fn new(view_id: u64, pty_id: u64, size: u16) -> Self {
        let id: &'static str = Box::leak(format!("plugin-terminal-{view_id}").into_boxed_str());
        let cols = size.max(1);
        Self {
            id,
            view_id,
            pty_id,
            grid: TerminalGrid::new(24, cols),
            size,
            mode: TermMode::Dock,
            input_mode: TermInputMode::Insert,
            last_size: None,
            last_area: None,
            diff: Default::default(),
        }
    }

    pub(crate) fn set_mode(&mut self, mode: TermMode) {
        self.mode = mode;
    }

    pub(crate) fn set_size(&mut self, size: u16) {
        self.size = size.max(1);
    }

    pub(crate) fn clear(&mut self) {
        self.grid.clear();
    }

    pub fn view_id(&self) -> u64 {
        self.view_id
    }

    /// 终端输入模式(Insert = 键直通 pty;Normal = 滚动)——窗口模式 C-w 豁免判断用
    pub fn input_mode(&self) -> TermInputMode {
        self.input_mode
    }

    /// 清空脏格 diff 状态(测试向不同 surface 渲染时需要重置)
    pub(crate) fn reset_render_state(&mut self) {
        self.diff = Default::default();
    }

    /// 把一块 PTY 输出喂进网格（TermFeed 请求路由到此处）
    pub fn feed(&mut self, chunk: &str) {
        self.grid.feed(chunk.as_bytes());
        if let Some(title) = self.grid.take_title() {
            helix_js::emit_term_title(self.pty_id, &title);
        }
        // 引擎对 DA/DSR/OSC 查询的答复必须写回 pty(引擎不自己写):
        // 不接的话,那些“先问终端再决定”的 TUI 会一直等回包
        let replies = self.grid.take_replies();
        if !replies.is_empty() {
            let _ = helix_js::term_write(self.pty_id, &String::from_utf8_lossy(&replies));
        }
    }

    /// 粘贴给 pty 的载荷:应用开了括号粘贴(2004)就包上 \e[200~ … \e[201~
    fn paste_payload(&self, contents: &str) -> String {
        if self
            .grid
            .engine_mode()
            .contains(EngineMode::BRACKETED_PASTE)
        {
            format!("\x1b[200~{contents}\x1b[201~")
        } else {
            contents.to_string()
        }
    }

    /// 把鼠标事件编码成发给 pty 的 CSI 序列。
    ///
    /// 应用未开鼠标上报、该模式不报告此类事件、或坐标超出编码范围时返回 `None`
    /// (调用方回退到本地滚动回看)。坐标从屏幕坐标换算成 pane 内 1-based。
    fn mouse_report(&self, mouse: &helix_view::input::MouseEvent) -> Option<String> {
        use helix_view::input::{KeyModifiers, MouseEventKind};

        let mode = self.grid.engine_mode();
        if !mode.intersects(EngineMode::MOUSE_MODE) {
            return None;
        }
        let area = self.last_area?;
        if mouse.column < area.x
            || mouse.row < area.y
            || mouse.column >= area.x + area.width
            || mouse.row >= area.y + area.height
        {
            return None;
        }
        let col = mouse.column - area.x + 1;
        let row = mouse.row - area.y + 1;

        // 按钮码:0 左 / 1 中 / 2 右;3 = 释放(X10);拖动 +32;无按钮移动 35;滚轮 64..67
        let (code, release) = match mouse.kind {
            MouseEventKind::Down(b) => (btn_code(b), false),
            MouseEventKind::Up(_) => (3, true),
            MouseEventKind::Drag(b) => (btn_code(b) + 32, false),
            MouseEventKind::Moved => (35, false),
            MouseEventKind::ScrollUp => (64, false),
            MouseEventKind::ScrollDown => (65, false),
            MouseEventKind::ScrollLeft => (66, false),
            MouseEventKind::ScrollRight => (67, false),
        };

        // 该模式是否报告「移动」类事件:1002 只报按住拖动,1003 报全部移动
        match mouse.kind {
            MouseEventKind::Drag(_) if mode.contains(EngineMode::MOUSE_DRAG) => {}
            MouseEventKind::Drag(_) | MouseEventKind::Moved
                if mode.contains(EngineMode::MOUSE_MOTION) => {}
            MouseEventKind::Drag(_) | MouseEventKind::Moved => return None,
            _ => {}
        }

        let mut code = code;
        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
            code += 4;
        }
        if mouse.modifiers.contains(KeyModifiers::ALT) {
            code += 8;
        }
        if mouse.modifiers.contains(KeyModifiers::CONTROL) {
            code += 16;
        }

        if mode.contains(EngineMode::SGR_MOUSE) {
            // 1006:十进制、分号分隔、大坐标无上限;释放用 'm'
            let end = if release { 'm' } else { 'M' };
            return Some(format!("\x1b[<{code};{col};{row}{end}"));
        }

        // X10 风格:ESC [ M + 单字节码位(1005 用 UTF-8 编,否则必须落在 ASCII 内)
        let utf8 = mode.contains(EngineMode::UTF8_MOUSE);
        let mut out = String::from("\x1b[M");
        push_x10(&mut out, 32 + code, utf8)?;
        push_x10(&mut out, 32 + col, utf8)?;
        push_x10(&mut out, 32 + row, utf8)?;
        Some(out)
    }

    /// 全部内容纯文本（scrollback + 屏幕；term-save 导出用）
    pub(crate) fn full_text(&self) -> String {
        self.grid.full_text()
    }
}

impl Drop for PluginTerminal {
    fn drop(&mut self) {
        // 关闭面板（Esc/层移除）→ 杀 pty；未知 id（已退出）静默
        let _ = helix_js::term_kill(self.pty_id);
        helix_js::unregister_component_state(self.view_id);
        helix_js::unregister_component_render(self.view_id);
    }
}

impl Component for PluginTerminal {
    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        use helix_view::input::{KeyCode, KeyModifiers, MouseEventKind};
        // minimized：不消费按键，编辑器照常工作
        if self.mode == TermMode::Minimized {
            return EventResult::Ignored(None);
        }
        // 鼠标：应用开了上报就编码发 pty(优先于本地滚动);否则滚轮走本地滚回
        if let Event::Mouse(mouse) = event {
            if let Some(report) = self.mouse_report(mouse) {
                let _ = helix_js::term_write(self.pty_id, &report);
                return EventResult::Consumed(None);
            }
            return match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.grid.scroll_up_view(3);
                    EventResult::Consumed(None)
                }
                MouseEventKind::ScrollDown => {
                    self.grid.scroll_down_view(3);
                    EventResult::Consumed(None)
                }
                _ => EventResult::Ignored(None),
            };
        }
        // 粘贴：直通模式下写给 pty;应用开了括号粘贴(2004)就包上标记
        if let Event::Paste(contents) = event {
            if self.input_mode == TermInputMode::Normal {
                return EventResult::Ignored(None); // 滚动查看态:交给编辑器
            }
            let payload = self.paste_payload(contents);
            let _ = helix_js::term_write(self.pty_id, &payload);
            return EventResult::Consumed(None);
        }
        let Event::Key(key) = event else {
            return EventResult::Ignored(None);
        };
        // term-key 钩子（插件决策优先）：pass/consume/minimize/close；无钩子 → 默认流程
        let key_desc = terminal_key_desc(key);
        match helix_js::emit_term_key(self.pty_id, &key_desc.0, key_desc.1, key_desc.2, key_desc.3)
        {
            Some(helix_js::TermKeyDecision::Pass) => {
                if let Some(bytes) = key_to_term_bytes(key, self.grid.engine_mode()) {
                    let _ = helix_js::term_write(self.pty_id, &bytes);
                }
                return EventResult::Consumed(None);
            }
            Some(helix_js::TermKeyDecision::Consume) => return EventResult::Consumed(None),
            Some(helix_js::TermKeyDecision::Minimize) => {
                return EventResult::Consumed(Some(Box::new(
                    |compositor: &mut Compositor, _cx: &mut Context| {
                        if let Some(id) = compositor
                            .layout_tree()
                            .find_leaf_id::<PluginTerminal>(|_| true)
                        {
                            compositor.minimize_leaf(id, true);
                        }
                    },
                )))
            }
            Some(helix_js::TermKeyDecision::Close) => {
                return EventResult::Consumed(Some(Box::new(
                    |compositor: &mut Compositor, _cx: &mut Context| {
                        compositor.remove_type::<PluginTerminal>();
                    },
                )))
            }
            None => {}
        }
        // C-\：Insert → Normal（终端内滚动查看）；Normal → 回 Insert 并聚焦编辑器
        let is_ctrl_backslash = key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('\\'));
        if is_ctrl_backslash {
            match self.input_mode {
                TermInputMode::Insert => {
                    self.input_mode = TermInputMode::Normal;
                    helix_js::emit_term_mode(self.pty_id, "normal");
                    EventResult::Consumed(None)
                }
                TermInputMode::Normal => {
                    self.input_mode = TermInputMode::Insert;
                    helix_js::emit_term_mode(self.pty_id, "insert");
                    // 回 helix：收起浮窗（若浮动）+ 焦点切回编辑器叶子（终端保留）
                    EventResult::Consumed(Some(Box::new(
                        |compositor: &mut Compositor, _cx: &mut Context| {
                            compositor.unfloat();
                            // focus_leaf 会同步布局缓存（get_layout 实时性）；layout_tree().focus 绕过
                            compositor.focus_leaf(0);
                        },
                    )))
                }
            }
        } else if self.input_mode == TermInputMode::Normal {
            // 终端 normal 模式：滚动缓冲/复制；i/a/Esc 回 insert；q 无操作（关闭归 window 模式 x）
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    self.grid.scroll_down_view(1);
                    EventResult::Consumed(None)
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.grid.scroll_up_view(1);
                    EventResult::Consumed(None)
                }
                KeyCode::PageDown => {
                    self.grid.scroll_down_view(self.grid.rows() as usize);
                    EventResult::Consumed(None)
                }
                KeyCode::PageUp => {
                    self.grid.scroll_up_view(self.grid.rows() as usize);
                    EventResult::Consumed(None)
                }
                KeyCode::Char('g') => {
                    self.grid.scroll_up_view(self.grid.scrollback_len());
                    EventResult::Consumed(None)
                }
                KeyCode::Char('G') => {
                    self.grid.set_scroll_offset(0);
                    EventResult::Consumed(None)
                }
                KeyCode::Char('i') | KeyCode::Char('a') => {
                    self.input_mode = TermInputMode::Insert;
                    EventResult::Consumed(None)
                }
                KeyCode::Esc => {
                    // normal 模式 Esc 无操作(与全局一致:只在 insert→normal 单向;回 insert 用 i/a)
                    EventResult::Consumed(None)
                }
                KeyCode::Char('y') => {
                    // 复制可视区文本（含滚回行）到 " 与 + 寄存器：编辑器里 p/+p 可粘贴
                    let text = self.grid.visible_text();
                    EventResult::Consumed(Some(Box::new(
                        move |_compositor: &mut Compositor, cx: &mut Context| {
                            let values = vec![text];
                            let _ = cx.editor.registers.write('"', values.clone());
                            let _ = cx.editor.registers.write('+', values);
                            cx.editor.set_status("终端可视区已复制（p 粘贴）");
                        },
                    )))
                }
                KeyCode::Char('q') => {
                    // 终端关闭统一交给 window 模式(x);此处仅消费,不再关闭
                    EventResult::Consumed(None)
                }
                // 其余键直通 pty（C-c 中断等）
                _ => {
                    if let Some(bytes) = key_to_term_bytes(key, self.grid.engine_mode()) {
                        let _ = helix_js::term_write(self.pty_id, &bytes);
                    }
                    EventResult::Consumed(None)
                }
            }
        } else {
            // Insert 模式：按键全部直通 pty（插入文字）
            if key.code == KeyCode::Esc {
                // Esc → 切 terminal normal 模式（滚动/复制），不关闭（关闭归 window 模式 x）
                self.input_mode = TermInputMode::Normal;
                helix_js::emit_term_mode(self.pty_id, "normal");
                return EventResult::Consumed(None);
            }
            if let Some(bytes) = key_to_term_bytes(key, self.grid.engine_mode()) {
                // 按键直通 pty（非阻塞；worker 已退出时静默）
                let _ = helix_js::term_write(self.pty_id, &bytes);
            }
            EventResult::Consumed(None)
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        self.last_area = Some(area);
        // OSC 52:引擎收到的剪贴板写入请求在此落地(只有 render 拿得到 editor)。
        // 走 registers.write 而不是直接碰 config 里的 provider:选区语义与 helix 一致。
        for (reg, text) in self.grid.take_clipboard() {
            let _ = cx.editor.registers.write(reg, vec![text]);
        }
        // 组件状态快照 → get_component_state(view_id)(JS 视图层读)
        let snap = serde_json::json!({
            "mode": match self.input_mode {
                TermInputMode::Insert => "insert",
                TermInputMode::Normal => "normal",
            },
            "title": self.grid.title(),
            "minimized": self.mode == TermMode::Minimized,
            "scroll_offset": self.grid.scroll_offset(),
        })
        .to_string();
        helix_js::register_component_state(self.view_id, move |_| snap.clone());
        // 标题条：JS 视图回调优先(顶部 1 行,网格下移);无回调 → 无标题条(现状)
        let has_view = helix_js::render_component(self.view_id, area.width, 1, None).is_ok();
        if has_view {
            if let Ok(content) = helix_js::render_component(self.view_id, area.width, 1, None) {
                let lines = crate::ui::comp_layout::render(content, (area.width, 1));
                self.diff
                    .render(&lines, area.with_height(1), surface, &cx.editor.theme);
            }
        }
        // minimized：标题条已由 JS 视图画(若有);无视图时画默认条
        if self.mode == TermMode::Minimized {
            if !has_view {
                let style = cx.editor.theme.get("ui.popup");
                surface.set_stringn(
                    area.x,
                    area.y,
                    "▁ terminal (minimized) — :term-dock to restore",
                    area.width as usize,
                    style,
                );
            }
            return;
        }
        let grid_area = if has_view { area.clip_top(1) } else { area };
        let rows = grid_area.height.max(1);
        let cols = grid_area.width.max(1);
        // 尺寸变化 → TIOCSWINSZ + 网格 resize（首次渲染 last_size=None 也会触发）
        if self.last_size != Some((rows, cols)) {
            #[cfg(unix)]
            let _ = helix_js::term_resize(self.pty_id, rows, cols);
            helix_js::emit_term_resize(self.pty_id, rows, cols);
            self.grid.resize(rows, cols);
            self.last_size = Some((rows, cols));
        }
        self.grid.render(grid_area, surface);
        // 终端光标：insert 模式在网格光标处画反色块(主题 ui.cursor)；normal 模式隐藏(滚动查看)
        if self.input_mode == TermInputMode::Insert {
            let (row, col) = self.grid.cursor();
            if let Some(cell) = surface.get_mut(grid_area.x + col, grid_area.y + row) {
                cell.set_style(cx.editor.theme.get("ui.cursor"));
            }
        }
    }

    fn id(&self) -> Option<&'static str> {
        Some(self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: u16, cols: u16) -> TerminalGrid {
        TerminalGrid::new(rows, cols)
    }

    fn line_text(g: &TerminalGrid, row: u16) -> String {
        (0..g.cols()).map(|c| g.cell(row, c).unwrap().ch).collect()
    }

    fn cell_at(g: &TerminalGrid, row: u16, col: u16) -> TerminalCell {
        g.cell(row, col).unwrap()
    }

    #[test]
    fn wide_chars_occupy_two_cells() {
        // CJK 宽字符：主格 width=2，下一格占位 width=0；render 跳过占位格
        let mut g = grid(1, 6);
        g.feed("中".as_bytes());
        let main = cell_at(&g, 0, 0);
        assert_eq!(main.ch, '中');
        assert_eq!(main.width, 2, "宽字符主格 width=2");
        let place = cell_at(&g, 0, 1);
        assert_eq!(place.width, 0, "宽字符占位格 width=0");
        assert_eq!(g.cursor(), (0, 2), "宽字符占 2 列光标");
        // 后续普通字符接着写
        g.feed(b"ab");
        assert_eq!(cell_at(&g, 0, 2).ch, 'a');
        assert_eq!(cell_at(&g, 0, 3).ch, 'b');
        // 清屏重置 width
        g.clear();
        assert_eq!(cell_at(&g, 0, 0).width, 1, "清屏后 width 重置");
        // 行尾宽字符：占位格丢弃，置延迟 wrap（下一字符换行）
        let mut g2 = grid(2, 3);
        g2.feed("abc".as_bytes()); // 光标到末列 (0,3)
        g2.feed("中".as_bytes()); // wrap 到下一行再写（末列宽字符先 wrap）
        assert_eq!(g2.cursor(), (1, 2), "末列宽字符 wrap 后占 2 格");
    }

    #[test]
    fn text_extraction_visible_and_full() {
        // 可视区/全量文本提取：scrollback + 屏幕、宽字符占位格跳过、行尾空白去除
        let mut g = grid(3, 6);
        g.feed(b"ab\r\ncd\r\nef\r\ngh"); // 3 行网格，4 次换行 → 1 行进滚回
        assert_eq!(g.scrollback_len(), 1);
        assert_eq!(
            g.visible_text(),
            "cd\nef\ngh\n",
            "可视区 = 活动区（滚回不显示）"
        );
        assert_eq!(
            g.full_text(),
            "ab\ncd\nef\ngh\n",
            "全量 = scrollback + 屏幕"
        );
        // 上滚 1 行：可视区显示滚回行 + 活动区前 2 行
        g.scroll_up_view(1);
        assert_eq!(g.visible_text(), "ab\ncd\nef\n", "滚动后可视区含滚回行");
        // 宽字符占位格跳过：不产生多余空格
        let mut g2 = grid(1, 6);
        g2.feed("中x".as_bytes());
        assert_eq!(g2.visible_text(), "中x\n", "宽字符占位格不产生空格");
    }

    #[test]
    fn scroll_view_offsets_render() {
        // 终端 normal 模式滚动缓冲：scroll_offset 让滚回行显示在顶部，活动区在底部
        let mut g = grid(2, 4);
        // 2 行网格，3 次 LF 后 2 行滚出（ab、ef），活动区剩 gh
        g.feed(b"ab\r\ncd\r\nef\r\ngh");
        assert_eq!(g.scrollback_len(), 2, "2 行滚回");
        assert!(g.has_scrollback());
        assert_eq!(g.scroll_offset(), 0);

        // 上滚 1 行：显示最后 1 行滚回（ef）+ 活动区第 0 行（gh）
        g.scroll_up_view(1);
        assert_eq!(g.scroll_offset(), 1);
        g.scroll_up_view(999); // clamp 到可用滚回
        assert_eq!(g.scroll_offset(), 2);

        // 回到底部
        g.set_scroll_offset(0);
        assert_eq!(g.scroll_offset(), 0);
        g.scroll_up_view(1);
        g.scroll_down_view(1);
        assert_eq!(g.scroll_offset(), 0);
    }

    #[test]
    fn key_to_term_bytes_ctrl_and_arrows() {
        use helix_view::input::{KeyCode, KeyEvent, KeyModifiers};
        let k = |code: KeyCode, ctrl: bool| KeyEvent {
            code,
            modifiers: if ctrl {
                KeyModifiers::CONTROL
            } else {
                KeyModifiers::NONE
            },
        };
        let kb = |code: KeyCode, ctrl: bool| key_to_term_bytes(&k(code, ctrl), EngineMode::empty());
        assert_eq!(kb(KeyCode::Char('c'), true).as_deref(), Some("\x03"), "C-c");
        assert_eq!(kb(KeyCode::Char('a'), true).as_deref(), Some("\x01"), "C-a");
        assert_eq!(kb(KeyCode::Char('x'), false).as_deref(), Some("x"));
        assert_eq!(kb(KeyCode::Up, false).as_deref(), Some("\x1b[A"), "上箭头");
        assert_eq!(
            kb(KeyCode::PageDown, false).as_deref(),
            Some("\x1b[6~"),
            "PageDown"
        );
        assert_eq!(kb(KeyCode::Home, false).as_deref(), Some("\x1b[H"), "Home");
        // C-\ 不在 key_to_term_bytes（handle_event 拦截）
        assert_eq!(kb(KeyCode::Char('\\'), true), None, "C-\\ 应被拦截");
    }

    #[test]
    fn feed_keeps_parser_state_across_chunks() {
        let mut g = grid(2, 8);
        // CSI 序列被块边界切开：parser 必须跨 feed 调用持久，否则 \x1b[3 状态丢失
        g.feed(b"\x1b[3");
        g.feed(b"2mX");
        // 完整序列 \x1b[32mX = 绿色；若每次 feed 新建 parser，SGR 参数丢失 → X 无色
        assert_eq!(
            cell_at(&g, 0, 0),
            TerminalCell {
                ch: 'X',
                fg: Some(Color::Green),
                bg: None,
                bold: false,
                width: 1,
                ..Default::default()
            }
        );
        assert_eq!(line_text(&g, 0), "X       ");
        // OSC/其他长序列同理：块边界切开的中间态不落地为可见字符
        g.feed(b"\x1b]0;ti");
        g.feed(b"tle\x07");
        assert_eq!(line_text(&g, 0), "X       "); // 标题文本未被当作内容打印
    }

    #[test]
    fn feed_print_and_wrap() {
        let mut g = grid(3, 5);
        g.feed(b"hello");
        assert_eq!(line_text(&g, 0), "hello");
        // 引擎(及 xterm)在末列用"待换行"标记:光标停在本列,下一个字符才换行
        assert_eq!(g.cursor(), (0, 4));
        // 行尾再写字符 → wrap 到下一行
        g.feed(b"X");
        assert_eq!(line_text(&g, 1), "X    ");
        assert_eq!(g.cursor(), (1, 1));
    }

    #[test]
    fn feed_newline_and_scroll() {
        let mut g = grid(2, 4);
        g.feed(b"ab\r\ncd");
        // 第 0 行 ab，光标在第 1 行第 2 列
        assert_eq!(line_text(&g, 0), "ab  ");
        assert_eq!(g.cursor(), (1, 2));
        // 底行再换行 → 整屏上滚：第 0 行进滚回，cd 到顶行，新空行在底
        g.feed(b"\r\nef");
        assert_eq!(line_text(&g, 0), "cd  ");
        assert_eq!(line_text(&g, 1), "ef  ");
        assert_eq!(g.scrollback_len(), 1);
    }

    #[test]
    fn csi_cursor_and_clear() {
        let mut g = grid(4, 6);
        g.feed(b"abcdef");
        // CUP 2;3（1-based）→ (1,2)
        g.feed(b"\x1b[2;3HX");
        assert_eq!(g.cursor(), (1, 3));
        // EL 0：清到行尾
        g.feed(b"\x1b[K");
        assert_eq!(line_text(&g, 1), "  X   ");
        // ED 2：清屏（光标不动）
        g.feed(b"\x1b[2J");
        assert_eq!(line_text(&g, 0), "      ");
        assert_eq!(g.cursor(), (1, 3));
        // CUU/CUF/CUB/CUD
        g.feed(b"\x1b[2A"); // 上 2 → (0,3)
        assert_eq!(g.cursor(), (0, 3));
        g.feed(b"\x1b[2C"); // 右 2 → (0,5)
        assert_eq!(g.cursor(), (0, 5));
        g.feed(b"\x1b[2D"); // 左 2 → (0,3)
        assert_eq!(g.cursor(), (0, 3));
        g.feed(b"\x1b[2B"); // 下 2 → (2,3)
        assert_eq!(g.cursor(), (2, 3));
        // 越界 clamp
        g.feed(b"\x1b[99A");
        assert_eq!(g.cursor(), (0, 3));
        g.feed(b"\x1b[99C");
        assert_eq!(g.cursor().1, g.cols() - 1);
    }

    #[test]
    fn sgr_colors_and_bold() {
        let mut g = grid(2, 12);
        // "red"(3) + "bold-green"(10) = 13 字符 → 第 13 个 wrap 到第 1 行
        g.feed(b"\x1b[31mred\x1b[1;32mbold-green\x1b[0mplain");
        assert_eq!(
            cell_at(&g, 0, 0),
            TerminalCell {
                ch: 'r',
                fg: Some(Color::Red),
                bg: None,
                bold: false,
                width: 1,
                ..Default::default()
            }
        );
        assert_eq!(
            cell_at(&g, 0, 3),
            TerminalCell {
                ch: 'b',
                fg: Some(Color::Green),
                bg: None,
                bold: true,
                width: 1,
                ..Default::default()
            }
        );
        // 第 13 个字符（bold-green 的 n）wrap 到第 1 行
        assert_eq!(
            cell_at(&g, 1, 0),
            TerminalCell {
                ch: 'n',
                fg: Some(Color::Green),
                bg: None,
                bold: true,
                width: 1,
                ..Default::default()
            }
        );
        assert_eq!(
            cell_at(&g, 1, 4),
            TerminalCell {
                ch: 'i',
                fg: None,
                bg: None,
                bold: false,
                width: 1,
                ..Default::default()
            }
        );
        // 256 色与真彩色
        g.feed(b"\x1b[38;5;42mX\x1b[48;2;1;2;3mY");
        assert_eq!(cell_at(&g, 1, 6).fg, Some(Color::Indexed(42)));
        assert_eq!(cell_at(&g, 1, 7).bg, Some(Color::Rgb(1, 2, 3)));
        // 默认前景/背景
        g.feed(b"\x1b[39;49mZ");
        assert_eq!(cell_at(&g, 1, 8).fg, None);
        assert_eq!(cell_at(&g, 1, 8).bg, None);
    }

    #[test]
    fn alt_screen_switch() {
        let mut g = grid(2, 4);
        g.feed(b"main");
        assert!(!g.in_alt());
        // 进入 alt screen：主屏清空，新内容在 alt 屏
        g.feed(b"\x1b[?1049h");
        assert!(g.in_alt());
        assert_eq!(line_text(&g, 0), "    ");
        // 真实 TUI 进 alt 后都会 home 光标;不 home 时会继承主屏的待换行标记
        // (见 alt_screen_inherits_pending_wrap)
        g.feed(b"\x1b[H");
        g.feed(b"alt!");
        assert_eq!(line_text(&g, 0), "alt!");
        // 退出:主屏恢复
        g.feed(b"\x1b[?1049l");
        assert!(!g.in_alt());
        assert_eq!(line_text(&g, 0), "main");
    }

    /// 进 alt screen 时光标(含"待换行"标记)被继承——xterm 与引擎同此行为。
    /// 主屏写满末列后进 alt,第一个字符会先换行。
    #[test]
    fn alt_screen_inherits_pending_wrap() {
        let mut g = grid(2, 4);
        g.feed(b"main"); // 写满首行 → 光标 (0,3) + 待换行
        g.feed(b"\x1b[?1049h");
        g.feed(b"a");
        assert_eq!(line_text(&g, 0), "    ");
        assert_eq!(line_text(&g, 1), "a   ");
    }

    #[test]
    fn alt_screen_resize_then_exit_no_panic() {
        let mut g = grid(3, 6);
        g.feed(b"main");
        // 进 alt：主屏 3×6 备份进 alt_saved
        g.feed(b"\x1b[?1049h");
        assert!(g.in_alt());
        g.feed(b"alt");
        // alt 中 resize（放大）：alt_saved 必须同步重排，否则 exit_alt 恢复旧尺寸 → 越界
        g.resize(4, 10);
        g.feed(b"!"); // 写字符不 panic
        g.feed(b"\x1b[?1049l"); // 退出：恢复重排后的主屏
        assert!(!g.in_alt());
        assert_eq!(line_text(&g, 0), "main      "); // 10 列：main 保留，其余空白
        assert_eq!(g.rows(), 4);
        // 缩小路径同样安全：再进 alt → 缩到 2×4 → 退出
        g.feed(b"\x1b[?1049h");
        g.resize(2, 4);
        g.feed(b"\x1b[?1049l");
        assert_eq!(line_text(&g, 0), "main");
        // "main" 恰好写满 4 列 → 光标带"待换行"标记,Z 换到下一行(不是覆盖 'n')
        g.feed(b"Z");
        assert_eq!(line_text(&g, 0), "main");
        assert_eq!(line_text(&g, 1), "Z   ");
    }

    /// 改尺寸走引擎 reflow:长行重排、字符不丢。
    /// 旧手写实现是截断销毁(`row.resize` 直接吐掉越出列的内容)——换引擎的主要收益之一。
    #[test]
    fn resize_reflows_and_conserves_text() {
        fn text_of(g: &TerminalGrid) -> String {
            g.full_text()
                .chars()
                .filter(|c| *c != '\n' && *c != ' ')
                .collect()
        }
        let mut g = grid(3, 5);
        g.feed(b"abcdefghijklmno"); // 15 字符 = 3×5
        assert_eq!(g.cursor(), (2, 4)); // 末列:待换行标记
        assert!(text_of(&g).ends_with("abcdefghijklmno"));

        // 缩到 2×4:重排成 4 行(4/4/4/3),可见区只显示最后 2 行,溢出的进历史
        g.resize(2, 4);
        assert_eq!(g.rows(), 2);
        let after = text_of(&g);
        assert!(
            after.ends_with("abcdefghijklmno"),
            "reflow 后 15 个字符必须全部存活(旧实现会只剩 8 个),实际: {after:?}"
        );

        // 放大回 4×8:行数生效,内容仍不丢
        g.resize(4, 8);
        assert_eq!(g.rows(), 4);
        let again = text_of(&g);
        assert!(again.ends_with("abcdefghijklmno"), "实际: {again:?}");
    }

    #[test]
    fn insert_delete_lines_and_scroll() {
        let mut g = grid(3, 4);
        g.feed(b"aaa\r\nbbb\r\nccc"); // 光标 (2,3)
                                      // IL 1（光标在底行）：底行 ccc 进滚回，光标行插入空行
        g.feed(b"\x1b[1L");
        assert_eq!(line_text(&g, 0), "aaa ");
        assert_eq!(line_text(&g, 1), "bbb ");
        assert_eq!(line_text(&g, 2), "    ");
        // IL 是"下滚":移出的底行被丢弃,不进历史(历史只在整屏上滚时增长)
        assert_eq!(g.scrollback_len(), 0);
        // 光标归位后 DL 1：删首行 aaa，内容上移，底行补空
        g.feed(b"\x1b[H\x1b[1M");
        assert_eq!(line_text(&g, 0), "bbb ");
        assert_eq!(line_text(&g, 2), "    ");
        // 造内容测 SU/SD：X/Y 覆盖 bbb 残余（正确行为：未清区域保留）
        g.feed(b"XY\r\nZ");
        assert_eq!(line_text(&g, 0), "XYb ");
        assert_eq!(line_text(&g, 1), "Z   ");
        // SU 1：顶行 XY 进滚回，整体上移，底行补空
        g.feed(b"\x1b[1S");
        assert_eq!(line_text(&g, 0), "Z   ");
        assert_eq!(line_text(&g, 2), "    ");
        // 历史 = DL 整屏上滚移出的 aaa(+1) + SU 移出的 XYb (+1)
        assert_eq!(g.scrollback_len(), 2);
        // SD 1：底行(空)进滚回，整体下移，顶行补空
        g.feed(b"\x1b[1T");
        assert_eq!(line_text(&g, 0), "    ");
        assert_eq!(line_text(&g, 1), "Z   ");
    }

    #[test]
    fn tab_and_backspace() {
        let mut g = grid(2, 8);
        g.feed(b"a\tb");
        // tab 到下一制表位;越过右边界时停在末列(xterm 同行为),不触发换行
        assert_eq!(line_text(&g, 0), "a      b");
        assert_eq!(line_text(&g, 1), "        ");
        assert_eq!(g.cursor(), (0, 7)); // 末列:待换行标记
        g.feed(b"\x08\x08");
        assert_eq!(g.cursor(), (0, 5));
    }

    // ── 写侧:鼠标上报 / 括号粘贴 / 查询回包 ──────────────────────────

    use helix_view::input::{KeyModifiers, MouseEventKind};

    fn mouse_term(seq: &str) -> PluginTerminal {
        let mut t = PluginTerminal::new(0, 0, 80);
        // 故意偏移的 pane:验证屏幕坐标→pane 内 1-based 坐标的换算
        t.last_area = Some(Rect::new(10, 2, 70, 20));
        t.feed(seq);
        t
    }

    fn mouse_ev(kind: MouseEventKind) -> helix_view::input::MouseEvent {
        helix_view::input::MouseEvent {
            kind,
            column: 12, // -10 +1 = 3
            row: 5,     // -2  +1 = 4
            modifiers: KeyModifiers::empty(),
        }
    }

    #[test]
    fn mouse_report_sgr_press_release_wheel_drag() {
        use helix_view::input::MouseButton;
        let t = mouse_term("\x1b[?1002h\x1b[?1006h"); // 按住拖动 + SGR 编码
        assert_eq!(
            t.mouse_report(&mouse_ev(MouseEventKind::Down(MouseButton::Left)))
                .as_deref(),
            Some("\x1b[<0;3;4M")
        );
        assert_eq!(
            t.mouse_report(&mouse_ev(MouseEventKind::Up(MouseButton::Left)))
                .as_deref(),
            Some("\x1b[<3;3;4m"), // 释放用 'm'
        );
        assert_eq!(
            t.mouse_report(&mouse_ev(MouseEventKind::ScrollUp))
                .as_deref(),
            Some("\x1b[<64;3;4M")
        );
        assert_eq!(
            t.mouse_report(&mouse_ev(MouseEventKind::Drag(MouseButton::Right)))
                .as_deref(),
            Some("\x1b[<34;3;4M"), // 右鍵拖动 = 2+32
        );
    }

    #[test]
    fn mouse_report_gated_by_mode() {
        use helix_view::input::MouseButton;
        // 未开上报 → 不接管(调用方回退到本地滚回)
        assert!(mouse_term("")
            .mouse_report(&mouse_ev(MouseEventKind::Down(MouseButton::Left)))
            .is_none());
        // 1000(仅点击)不报移动
        let click_only = mouse_term("\x1b[?1000h");
        assert!(click_only
            .mouse_report(&mouse_ev(MouseEventKind::Moved))
            .is_none());
        assert!(click_only
            .mouse_report(&mouse_ev(MouseEventKind::Down(MouseButton::Left)))
            .is_some());
        // 1002 报拖动、不报单纯移动;1003 报全部
        let drag = mouse_term("\x1b[?1002h");
        assert!(drag
            .mouse_report(&mouse_ev(MouseEventKind::Moved))
            .is_none());
        assert!(drag
            .mouse_report(&mouse_ev(MouseEventKind::Drag(MouseButton::Left)))
            .is_some());
        let any = mouse_term("\x1b[?1003h");
        assert!(any.mouse_report(&mouse_ev(MouseEventKind::Moved)).is_some());
    }

    #[test]
    fn mouse_report_x10_ascii_encoding() {
        use helix_view::input::MouseButton;
        // 无 1006:ESC [ M + 单字节码位(32+code / 32+col / 32+row)
        let t = mouse_term("\x1b[?1000h");
        assert_eq!(
            t.mouse_report(&mouse_ev(MouseEventKind::Down(MouseButton::Left)))
                .as_deref(),
            Some("\x1b[M #$")
        );
    }

    #[test]
    fn mouse_report_ignores_events_outside_pane() {
        use helix_view::input::MouseButton;
        let t = mouse_term("\x1b[?1000h");
        let outside = helix_view::input::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3, // 在 area.x=10 左侧
            row: 5,
            modifiers: KeyModifiers::empty(),
        };
        assert!(t.mouse_report(&outside).is_none());
    }

    #[test]
    fn paste_payload_wraps_only_when_bracketed() {
        assert_eq!(mouse_term("").paste_payload("hi"), "hi");
        assert_eq!(
            mouse_term("\x1b[?2004h").paste_payload("hi"),
            "\x1b[200~hi\x1b[201~"
        );
    }

    #[test]
    fn device_query_replies_are_collected_for_pty() {
        let mut g = grid(2, 8);
        g.feed(b"\x1b[5n"); // DSR:应用状态查询
        let replies = g.take_replies();
        assert!(
            !replies.is_empty(),
            "DA/DSR 查询的答复必须经 PtyWrite 交给宿主写回 pty"
        );
        // 取走后不重复投递
        assert!(g.take_replies().is_empty());
    }

    #[test]
    fn arrow_keys_use_ss3_in_application_cursor_mode() {
        use helix_view::input::{KeyCode, KeyEvent, KeyModifiers};
        let ev = |code| KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
        };
        // 默认:CSI
        assert_eq!(
            key_to_term_bytes(&ev(KeyCode::Up), EngineMode::empty()).as_deref(),
            Some("\x1b[A")
        );
        // DECCKM(`?1h`):vym/less 期望 SS3
        assert_eq!(
            key_to_term_bytes(&ev(KeyCode::Up), EngineMode::APP_CURSOR).as_deref(),
            Some("\x1bOA")
        );
        assert_eq!(
            key_to_term_bytes(&ev(KeyCode::End), EngineMode::APP_CURSOR).as_deref(),
            Some("\x1bOF")
        );
        // 翻页键不受 DECCKM 影响
        assert_eq!(
            key_to_term_bytes(&ev(KeyCode::PageDown), EngineMode::APP_CURSOR).as_deref(),
            Some("\x1b[6~")
        );
    }

    #[test]
    fn render_writes_to_surface() {
        let mut g = grid(2, 5);
        g.feed(b"\x1b[31mhi");
        let mut surface = Surface::empty(Rect::new(0, 0, 6, 3));
        g.render(Rect::new(0, 0, 6, 3), &mut surface);
        // 网格内容画到 surface，SGR 颜色生效
        assert_eq!(surface.get(0, 0).unwrap().symbol.as_str(), "h");
        assert_eq!(surface.get(1, 0).unwrap().symbol.as_str(), "i");
        assert_eq!(surface.get(0, 0).unwrap().fg, Color::Red);
        // 网格外区域不动（保持空白）
        assert_eq!(surface.get(5, 2).unwrap().symbol.as_str(), " ");
    }
}
