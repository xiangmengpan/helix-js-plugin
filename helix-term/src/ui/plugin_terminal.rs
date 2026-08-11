//! 原生终端视图：PTY 输出经 vte 解析成字符网格，渲染到 tui surface。
//!
//! `TerminalGrid` 是纯数据 + vte::Perform 实现（无 helix 依赖，可单测）；
//! `PluginTerminal` 是 compositor 层：按键直通 pty、尺寸变化实时 TIOCSWINSZ、
//! Esc 关闭（Drop 时杀 pty）。

use std::collections::VecDeque;

use helix_view::graphics::{Color, Modifier, Rect, Style};
use tui::buffer::Buffer as Surface;
use vte::{Params, Perform};

use crate::compositor::{Component, Compositor, Context, Event, EventResult};

/// 单个终端网格单元：字符 + SGR 颜色 + 粗体。
/// 宽字符/组合字符 PoC 不处理（单 char 一格，render 时逐格写）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCell {
    pub ch: char,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
}

impl Default for TerminalCell {
    fn default() -> Self {
        Self { ch: ' ', fg: None, bg: None, bold: false }
    }
}

/// 滚回保留的最大行数（只存不显示，PoC）
const SCROLLBACK_MAX: usize = 1000;

/// vte 解析出的终端网格：cols×rows 单元 + 光标 + 滚回 + alt screen 备份。
/// 不派生 Debug/Clone：vte::Parser 只有 Default（解析器跨 feed 调用持久，块边界
/// 切开的 CSI/OSC 序列才不损坏）。
pub struct TerminalGrid {
    /// vte 状态机（一次创建、反复 advance）：块边界落在逃逸序列中间时状态保留到下一块
    parser: vte::Parser,
    cols: u16,
    rows: u16,
    cells: Vec<TerminalCell>,
    cursor: (u16, u16),
    saved_cursor: (u16, u16),
    /// 主屏备份：进入 alt screen 时保存（cells + 光标），退出时恢复
    alt_saved: Option<(Vec<TerminalCell>, (u16, u16))>,
    /// 是否处于 alt screen（= alt_saved.is_some() 的冗余，便于测试与阅读）
    alt: bool,
    /// 滚出屏幕的最后 N 行（顶行/底行滚动时收集；显示未用）
    scrollback: VecDeque<Vec<TerminalCell>>,
    /// 累计滚出行数（含被 SCROLLBACK_MAX 裁剪掉的）
    scrollback_len: usize,
    // 当前 SGR 属性（print 时写到单元格）
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
}

impl TerminalGrid {
    pub fn new(rows: u16, cols: u16) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        Self {
            parser: vte::Parser::new(),
            cols,
            rows,
            cells: vec![TerminalCell::default(); rows as usize * cols as usize],
            cursor: (0, 0),
            saved_cursor: (0, 0),
            alt_saved: None,
            alt: false,
            scrollback: VecDeque::new(),
            scrollback_len: 0,
            fg: None,
            bg: None,
            bold: false,
        }
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn cursor(&self) -> (u16, u16) {
        self.cursor
    }

    /// 单元格（行/列越界 → None）
    pub fn cell(&self, row: u16, col: u16) -> Option<&TerminalCell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        Some(&self.cells[row as usize * self.cols as usize + col as usize])
    }

    pub fn in_alt(&self) -> bool {
        self.alt
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback_len
    }

    /// 把一块字节喂进 vte 解析器（复用同一 parser：块边界切开的 CSI/OSC 序列
    /// 跨 feed 调用保持状态）。chunk 内部字节无需完整——read_stream 只保证
    /// UTF-8 字符不跨块，逃逸序列跨块由本持久 parser 承接。
    pub fn feed(&mut self, bytes: &[u8]) {
        // parser 与 Perform 实现都借 &mut self：临时取出解析器、喂完放回（vte::Parser: Default）
        let mut parser = std::mem::take(&mut self.parser);
        parser.advance(self, bytes);
        self.parser = parser;
    }

    fn idx(&self, row: u16, col: u16) -> usize {
        row as usize * self.cols as usize + col as usize
    }

    fn cell_mut(&mut self, row: u16, col: u16) -> &mut TerminalCell {
        let idx = self.idx(row, col);
        &mut self.cells[idx]
    }

    /// 把一行（截断到当前宽度）推进滚回
    fn push_scrollback(&mut self, mut line: Vec<TerminalCell>) {
        line.truncate(self.cols as usize);
        self.scrollback_len += 1;
        self.scrollback.push_back(line);
        while self.scrollback.len() > SCROLLBACK_MAX {
            self.scrollback.pop_front();
        }
    }

    /// 光标下行；已在底行时整屏上滚（顶行进滚回）
    fn linefeed(&mut self) {
        if self.cursor.0 + 1 < self.rows {
            self.cursor.0 += 1;
        } else {
            self.scroll_up(1);
        }
    }

    /// 整屏上滚 n 行：顶行依次进滚回，底行补空
    fn scroll_up(&mut self, n: u16) {
        let n = (n as usize).min(self.rows as usize);
        for _ in 0..n {
            let top: Vec<TerminalCell> = self.cells.drain(..self.cols as usize).collect();
            self.push_scrollback(top);
            self.cells
                .extend(std::iter::repeat_with(TerminalCell::default).take(self.cols as usize));
        }
    }

    /// 整屏下滚 n 行：底行依次进滚回，顶行补空（与 SU 对称）
    fn scroll_down(&mut self, n: u16) {
        let n = (n as usize).min(self.rows as usize);
        for _ in 0..n {
            let bottom: Vec<TerminalCell> =
                self.cells.split_off(self.cells.len() - self.cols as usize);
            self.push_scrollback(bottom);
            self.cells
                .splice(0..0, vec![TerminalCell::default(); self.cols as usize]);
        }
    }

    /// 光标行起插入 n 个空行（内容下移，底行进滚回）
    fn insert_lines(&mut self, n: u16) {
        let row = self.cursor.0 as usize;
        let n = (n as usize).min((self.rows as usize - row).max(1));
        let mut lines: Vec<Vec<TerminalCell>> = self
            .cells
            .chunks(self.cols as usize)
            .map(|c| c.to_vec())
            .collect();
        for _ in 0..n {
            if let Some(bottom) = lines.pop() {
                self.push_scrollback(bottom);
            }
            lines.insert(row, vec![TerminalCell::default(); self.cols as usize]);
        }
        self.cells = lines.into_iter().flatten().collect();
    }

    /// 光标行起删除 n 行（下方内容上移，底行补空；被删行不进滚回）
    fn delete_lines(&mut self, n: u16) {
        let row = self.cursor.0 as usize;
        let n = (n as usize).min((self.rows as usize - row).max(1));
        let mut lines: Vec<Vec<TerminalCell>> = self
            .cells
            .chunks(self.cols as usize)
            .map(|c| c.to_vec())
            .collect();
        for _ in 0..n {
            lines.remove(row);
            lines.push(vec![TerminalCell::default(); self.cols as usize]);
        }
        self.cells = lines.into_iter().flatten().collect();
    }

    /// 光标处插入 n 个空白（行内容右移，行尾丢弃）
    fn insert_blank(&mut self, n: u16) {
        let col = self.cursor.1 as usize;
        let n = (n as usize).min((self.cols as usize - col).max(1));
        if n == 0 {
            return;
        }
        let start = self.idx(self.cursor.0, 0);
        let end = start + self.cols as usize;
        // [col, cols-n) 右移到 [col+n, cols)，丢行尾 n 格
        self.cells.copy_within(start + col..end - n, start + col + n);
        for c in &mut self.cells[start + col..start + col + n] {
            *c = TerminalCell::default();
        }
    }

    fn clear_screen(&mut self) {
        for c in &mut self.cells {
            *c = TerminalCell::default();
        }
    }

    fn clear_above(&mut self) {
        let end = self.idx(self.cursor.0, self.cursor.1) + 1;
        for c in &mut self.cells[..end] {
            *c = TerminalCell::default();
        }
    }

    fn clear_below(&mut self) {
        let start = self.idx(self.cursor.0, self.cursor.1);
        for c in &mut self.cells[start..] {
            *c = TerminalCell::default();
        }
    }

    fn clear_to_eol(&mut self) {
        let start = self.idx(self.cursor.0, self.cursor.1);
        let end = self.idx(self.cursor.0, 0) + self.cols as usize;
        for c in &mut self.cells[start..end] {
            *c = TerminalCell::default();
        }
    }

    fn clear_to_bol(&mut self) {
        let start = self.idx(self.cursor.0, 0);
        let end = self.idx(self.cursor.0, self.cursor.1) + 1;
        for c in &mut self.cells[start..end] {
            *c = TerminalCell::default();
        }
    }

    fn clear_line(&mut self) {
        let start = self.idx(self.cursor.0, 0);
        for c in &mut self.cells[start..start + self.cols as usize] {
            *c = TerminalCell::default();
        }
    }

    fn reset_attrs(&mut self) {
        self.fg = None;
        self.bg = None;
        self.bold = false;
    }

    fn enter_alt(&mut self) {
        if !self.alt {
            // 主屏备份 + 清屏（真实终端进入 alt screen 时清屏）
            self.alt_saved = Some((std::mem::take(&mut self.cells), self.cursor));
            self.cells = vec![TerminalCell::default(); self.rows as usize * self.cols as usize];
            self.cursor = (0, 0);
            self.alt = true;
        }
    }

    fn exit_alt(&mut self) {
        if self.alt {
            if let Some((cells, cursor)) = self.alt_saved.take() {
                self.cells = cells;
                self.cursor = cursor;
            }
            self.alt = false;
        }
    }

    /// 尺寸变化：截断/填充网格，光标 clamp，滚回行按新宽度截断。
    /// alt screen 挂起时主屏备份（alt_saved）也按新尺寸重排——否则 exit_alt
    /// 把旧尺寸 cells 塞回新网格 → 越界 panic（UI 线程崩溃）。
    /// 清空网格（保留尺寸与光标位置）
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            *cell = TerminalCell { ch: ' ', fg: None, bg: None, bold: false };
        }
        self.cursor = (0, 0);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        let old_cols = self.cols;
        let old_rows = self.rows;
        self.cells = resize_cells(&self.cells, old_rows, old_cols, rows, cols);
        if let Some((cells, cursor)) = &mut self.alt_saved {
            // alt_saved 是进 alt 时从 self.cells mem::take 的主屏备份，尺寸同 (old_rows, old_cols)
            *cells = resize_cells(cells, old_rows, old_cols, rows, cols);
            cursor.0 = cursor.0.min(rows - 1);
            cursor.1 = cursor.1.min(cols - 1);
        }
        self.cols = cols;
        self.rows = rows;
        self.cursor.0 = self.cursor.0.min(rows - 1);
        self.cursor.1 = self.cursor.1.min(cols - 1);
        for line in &mut self.scrollback {
            line.truncate(cols as usize);
        }
    }

    /// 把网格画到 surface（area 左上角起，逐格写 symbol + style）
    pub fn render(&self, area: Rect, surface: &mut Surface) {
        for row in 0..area.height {
            let grid_row = row as usize;
            if grid_row >= self.rows as usize {
                break;
            }
            let base = grid_row * self.cols as usize;
            for col in 0..area.width {
                let grid_col = col as usize;
                if grid_col >= self.cols as usize {
                    break;
                }
                let cell = &self.cells[base + grid_col];
                let Some(surface_cell) = surface.get_mut(area.x + col, area.y + row) else {
                    continue;
                };
                let mut style = Style::default();
                if cell.bold {
                    style.add_modifier |= Modifier::BOLD;
                }
                style.fg = cell.fg;
                style.bg = cell.bg;
                surface_cell.set_symbol(&cell.ch.to_string());
                surface_cell.set_style(style);
            }
        }
    }
}

/// 网格内容按新尺寸重排：旧区域（min(rows,old_rows)×min(cols,old_cols) 交集）保留，
/// 新区域填空白，越界截断。resize 主屏与 alt_saved 共用。
fn resize_cells(
    old: &[TerminalCell],
    old_rows: u16,
    old_cols: u16,
    rows: u16,
    cols: u16,
) -> Vec<TerminalCell> {
    let mut new_cells = vec![TerminalCell::default(); rows as usize * cols as usize];
    for r in 0..rows.min(old_rows) {
        for c in 0..cols.min(old_cols) {
            new_cells[r as usize * cols as usize + c as usize] =
                old[r as usize * old_cols as usize + c as usize];
        }
    }
    new_cells
}

/// ANSI 16 色 → tui Color（0-7 基础色 / 90-97 亮色）
fn color_from_ansi(idx: u8, bright: bool) -> Color {
    const BASE: [Color; 8] = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
    ];
    const BRIGHT: [Color; 8] = [
        Color::LightGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    if bright {
        BRIGHT[idx as usize % 8]
    } else {
        BASE[idx as usize % 8]
    }
}

impl Perform for TerminalGrid {
    fn print(&mut self, c: char) {
        // 行尾延迟换行：光标已在最后一列时，下一个字符先 wrap 再写
        if self.cursor.1 >= self.cols {
            self.cursor.1 = 0;
            self.linefeed();
        }
        let (fg, bg, bold) = (self.fg, self.bg, self.bold);
        let cell = self.cell_mut(self.cursor.0, self.cursor.1);
        cell.ch = c;
        cell.fg = fg;
        cell.bg = bg;
        cell.bold = bold;
        self.cursor.1 += 1;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b => self.linefeed(),              // LF / VT
            b'\r' => self.cursor.1 = 0,                   // CR
            b'\t' => {                                    // 下一个 tab stop（8 格）
                self.cursor.1 = (self.cursor.1 / 8 + 1) * 8;
                if self.cursor.1 >= self.cols {
                    // 目标在/超过末列：光标停末列并置延迟 wrap（下一字符换行，
                    // 与 print 的末列语义一致；真实 xterm 同行为）
                    self.cursor.1 = self.cols;
                }
            }
            0x08 => self.cursor.1 = self.cursor.1.saturating_sub(1), // BS
            0x0c => {                                     // FF：清屏 + 光标归位
                self.clear_screen();
                self.cursor = (0, 0);
            }
            _ => {}                                       // BEL 等忽略
        }
    }

    fn csi_dispatch(&mut self, params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        // 私有模式仅当带 ? intermediate 才生效（DECSET/DECRST）：CSI ?1049h/l 切
        // alt screen，其余（?25h/l 光标显隐等）忽略；无 ? 的 CSI 1049h 不是私有模式。
        if _intermediates.contains(&b'?') {
            let mode = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(0);
            match (mode, action) {
                (1049, 'h') => self.enter_alt(),
                (1049, 'l') => self.exit_alt(),
                _ => {}
            }
            return;
        }
        // 光标移动类 CSI 先清除延迟 wrap 状态（真实终端：任何光标移动都取消待定换行，
        // 只动行的 A/B 若不处理会让末列待定换行错误地延续到下一字符）
        if matches!(action, 'A' | 'B' | 'C' | 'D' | 'G' | 'H' | 'f') {
            self.cursor.1 = self.cursor.1.min(self.cols - 1);
        }
        // CSI 参数读取：缺失/0 → 默认值
        let param = |i: usize, dflt: u16| {
            params
                .iter()
                .nth(i)
                .and_then(|p| p.first())
                .copied()
                .map(|v| if v == 0 { dflt } else { v })
                .unwrap_or(dflt)
        };
        match action {
            // 光标移动
            'A' => self.cursor.0 = self.cursor.0.saturating_sub(param(0, 1)), // CUU
            'B' => self.cursor.0 = (self.cursor.0 + param(0, 1)).min(self.rows - 1), // CUD
            'C' => self.cursor.1 = (self.cursor.1 + param(0, 1)).min(self.cols - 1), // CUF
            'D' => self.cursor.1 = self.cursor.1.saturating_sub(param(0, 1)), // CUB
            'G' => self.cursor.1 = param(0, 1).saturating_sub(1).min(self.cols - 1), // CHA
            'H' | 'f' => {                                    // CUP：row;col（1-based）
                self.cursor.0 = param(0, 1).saturating_sub(1).min(self.rows - 1);
                self.cursor.1 = param(1, 1).saturating_sub(1).min(self.cols - 1);
            }
            // 擦除
            'J' => match param(0, 0) {
                0 => self.clear_below(),
                1 => self.clear_above(),
                2 => self.clear_screen(),
                3 => self.scrollback.clear(), // ED 3：清滚回
                _ => {}
            },
            'K' => match param(0, 0) {
                0 => self.clear_to_eol(),
                1 => self.clear_to_bol(),
                2 => self.clear_line(),
                _ => {}
            },
            // SGR
            'm' => self.sgr(params),
            // 插入/删除
            '@' => self.insert_blank(param(0, 1)),  // ICH
            'L' => self.insert_lines(param(0, 1)),  // IL
            'M' => self.delete_lines(param(0, 1)),  // DL
            'S' => self.scroll_up(param(0, 1)),     // SU
            'T' => self.scroll_down(param(0, 1)),   // SD
            _ => {}                                 // 其余（DCH/ECH/REP 等）PoC 忽略
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => self.saved_cursor = self.cursor, // DECSC
            b'8' => self.cursor = self.saved_cursor, // DECRC
            _ => {}                                  // 字符集选择等忽略
        }
    }

    // OSC（标题等）PoC 忽略
    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}
}

impl TerminalGrid {
    /// SGR 解析：0 重置 / 1 粗体 / 22 去粗 / 30-37·40-47·90-97·100-107 颜色 /
    /// 38;5;n 256 色 / 38;2;r;g;b 真彩色 / 39·49 默认前景/背景
    fn sgr(&mut self, params: &Params) {
        let mut iter = params.iter();
        // 空参（CSI m）＝ 0 重置
        let Some(mut cur) = iter.next() else {
            self.reset_attrs();
            return;
        };
        loop {
            let v = cur.first().copied().unwrap_or(0);
            match v {
                0 => self.reset_attrs(),
                1 => self.bold = true,
                22 => self.bold = false,
                39 => self.fg = None,
                49 => self.bg = None,
                30..=37 => self.fg = Some(color_from_ansi(v as u8 - 30, false)),
                40..=47 => self.bg = Some(color_from_ansi(v as u8 - 40, false)),
                90..=97 => self.fg = Some(color_from_ansi(v as u8 - 90, true)),
                100..=107 => self.bg = Some(color_from_ansi(v as u8 - 100, true)),
                38 | 48 => {
                    let is_fg = v == 38;
                    match iter.next().and_then(|p| p.first()).copied() {
                        Some(5) => {
                            let idx = iter.next().and_then(|p| p.first()).copied().unwrap_or(0);
                            let color = Color::Indexed(idx as u8);
                            if is_fg {
                                self.fg = Some(color);
                            } else {
                                self.bg = Some(color);
                            }
                        }
                        Some(2) => {
                            let r = iter.next().and_then(|p| p.first()).copied().unwrap_or(0);
                            let g = iter.next().and_then(|p| p.first()).copied().unwrap_or(0);
                            let b = iter.next().and_then(|p| p.first()).copied().unwrap_or(0);
                            let color = Color::Rgb(r as u8, g as u8, b as u8);
                            if is_fg {
                                self.fg = Some(color);
                            } else {
                                self.bg = Some(color);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {} // 下划线/反转等 PoC 忽略
            }
            match iter.next() {
                Some(p) => cur = p,
                None => break,
            }
        }
    }
}

/// 把按键转成发给 pty 的字节序列。
/// Esc 是关闭面板的专用键（终端内程序无法用 Esc——PoC 取舍，简报明示）。
/// 方向键等无终端应序列，PoC 不发送。
// ponytail: 缺箭头/Home/End 应序列与 Ctrl 组合（如 C-c 直传字符本身），
// 交互程序完整支持时补 xterm 应序列表。
fn key_to_term_bytes(key: &helix_view::input::KeyEvent) -> Option<String> {
    match key.code {
        helix_view::input::KeyCode::Char(c) => Some(c.to_string()),
        helix_view::input::KeyCode::Enter => Some("\r".into()),
        helix_view::input::KeyCode::Backspace => Some("\u{7f}".into()),
        helix_view::input::KeyCode::Tab => Some("\t".into()),
        _ => None,
    }
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

pub struct PluginTerminal {
    /// find_id 路由用的静态 id（每实例 Box::leak，组件生命周期同层）
    id: &'static str,
    view_id: u64,
    pty_id: u64,
    grid: TerminalGrid,
    side: crate::ui::plugin_panel::PanelSide,
    size: u16,
    mode: TermMode,
    /// 悬浮模式位置（默认居中）
    float_pos: Option<(u16, u16)>,
    /// 上次渲染尺寸（None = 尚未渲染；首次渲染触发 resize + TIOCSWINSZ）
    last_size: Option<(u16, u16)>,
}

impl PluginTerminal {
    pub fn new(
        view_id: u64,
        pty_id: u64,
        side: crate::ui::plugin_panel::PanelSide,
        size: u16,
    ) -> Self {
        let id: &'static str = Box::leak(format!("plugin-terminal-{view_id}").into_boxed_str());
        let cols = size.max(1);
        Self {
            id,
            view_id,
            pty_id,
            grid: TerminalGrid::new(24, cols),
            side,
            size,
            mode: TermMode::Dock,
            float_pos: None,
            last_size: None,
        }
    }

    pub(crate) fn side(&self) -> crate::ui::plugin_panel::PanelSide {
        self.side
    }

    pub fn size(&self) -> u16 {
        self.size
    }

    pub fn mode(&self) -> TermMode {
        self.mode
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

    /// 按模式计算本终端的渲染区域（dock 用 layout_panels 给的 dock_rect）
    pub(crate) fn area_for(&self, area: Rect, dock_rect: Option<Rect>) -> Rect {
        match self.mode {
            TermMode::Dock => dock_rect.unwrap_or(area),
            TermMode::Fullscreen => area,
            TermMode::Floating => {
                let (w, h) = (self.size.max(20).min(area.width), 16.min(area.height));
                let (x, y) = self.float_pos.unwrap_or((
                    area.x + area.width.saturating_sub(w) / 2,
                    area.y + area.height.saturating_sub(h) / 2,
                ));
                Rect::new(x, y, w, h)
            }
            TermMode::Minimized => Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
        }
    }

    pub fn view_id(&self) -> u64 {
        self.view_id
    }

    /// 把一块 PTY 输出喂进网格（TermFeed 请求路由到此处）
    pub fn feed(&mut self, chunk: &str) {
        self.grid.feed(chunk.as_bytes());
    }
}

impl Drop for PluginTerminal {
    fn drop(&mut self) {
        // 关闭面板（Esc/层移除）→ 杀 pty；未知 id（已退出）静默
        let _ = helix_js::term_kill(self.pty_id);
    }
}

impl Component for PluginTerminal {
    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        let Event::Key(key) = event else {
            return EventResult::Ignored(None);
        };
        // minimized：不消费按键，编辑器照常工作
        if self.mode == TermMode::Minimized {
            return EventResult::Ignored(None);
        }
        // Esc → 关闭面板（kill pty 由 Drop 兜底）
        if key.code == helix_view::input::KeyCode::Esc {
            return EventResult::Consumed(Some(Box::new(|compositor: &mut Compositor, _cx: &mut Context| {
                // ponytail: remove_type 无 id 匹配——多终端并存时 Esc 关全部；
                // 需要 compositor remove-by-id 时再加（并行 wave 冲突面上不动 compositor.rs）
                compositor.remove_type::<PluginTerminal>();
            })));
        }
        if let Some(bytes) = key_to_term_bytes(key) {
            // 按键直通 pty（非阻塞；worker 已退出时静默）
            let _ = helix_js::term_write(self.pty_id, &bytes);
        }
        EventResult::Consumed(None)
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, _cx: &mut Context) {
        // minimized：只画一条标题（不渲染网格）
        if self.mode == TermMode::Minimized {
            let style = _cx.editor.theme.get("ui.popup");
            surface.set_stringn(area.x, area.y, "▁ terminal (minimized) — :term-dock to restore", area.width as usize, style);
            return;
        }
        let rows = area.height.max(1);
        let cols = area.width.max(1);
        // 尺寸变化 → TIOCSWINSZ + 网格 resize（首次渲染 last_size=None 也会触发）
        if self.last_size != Some((rows, cols)) {
            #[cfg(unix)]
            let _ = helix_js::term_resize(self.pty_id, rows, cols);
            self.grid.resize(rows, cols);
            self.last_size = Some((rows, cols));
        }
        self.grid.render(area, surface);
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
        *g.cell(row, col).unwrap()
    }

    #[test]
    fn area_for_modes() {
        use crate::ui::plugin_panel::PanelSide;
        let area = Rect::new(0, 0, 100, 40);
        let mut t = PluginTerminal::new(1, 1, PanelSide::Right, 30);
        // dock：用 layout 给的 rect
        assert_eq!(t.area_for(area, Some(Rect::new(70, 0, 30, 40))), Rect::new(70, 0, 30, 40));
        // fullscreen：全屏
        t.set_mode(TermMode::Fullscreen);
        assert_eq!(t.area_for(area, None), area);
        // minimized：底部 1 行
        t.set_mode(TermMode::Minimized);
        assert_eq!(t.area_for(area, None), Rect::new(0, 39, 100, 1));
        // floating：默认居中（宽 30，高 16）
        t.set_mode(TermMode::Floating);
        let f = t.area_for(area, None);
        assert_eq!(f.height, 16);
        assert_eq!(f.x, (100 - 30) / 2);
        assert_eq!(f.y, (40 - 16) / 2);
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
            TerminalCell { ch: 'X', fg: Some(Color::Green), bg: None, bold: false }
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
        assert_eq!(g.cursor(), (0, 5));
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
        assert_eq!(cell_at(&g, 0, 0), TerminalCell { ch: 'r', fg: Some(Color::Red), bg: None, bold: false });
        assert_eq!(cell_at(&g, 0, 3), TerminalCell { ch: 'b', fg: Some(Color::Green), bg: None, bold: true });
        // 第 13 个字符（bold-green 的 n）wrap 到第 1 行
        assert_eq!(cell_at(&g, 1, 0), TerminalCell { ch: 'n', fg: Some(Color::Green), bg: None, bold: true });
        assert_eq!(cell_at(&g, 1, 4), TerminalCell { ch: 'i', fg: None, bg: None, bold: false });
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
        g.feed(b"alt!");
        assert_eq!(line_text(&g, 0), "alt!");
        // 退出：主屏恢复
        g.feed(b"\x1b[?1049l");
        assert!(!g.in_alt());
        assert_eq!(line_text(&g, 0), "main");
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
        g.feed(b"Z"); // 写字符不 panic
        assert_eq!(line_text(&g, 0), "maiZ");
    }

    #[test]
    fn resize_truncate_and_fill() {
        let mut g = grid(3, 5);
        g.feed(b"abcdefghijklmno"); // 15 字符写满 3×5
        assert_eq!(line_text(&g, 2), "klmno");
        assert_eq!(g.cursor(), (2, 5));
        // 缩到 2×4：内容截断、光标 clamp
        g.resize(2, 4);
        assert_eq!(line_text(&g, 0), "abcd");
        assert_eq!(g.rows(), 2);
        assert_eq!(g.cursor(), (1, 3)); // (2,5) clamp 到 rows-1/cols-1
        // 放大到 4×8：新区域填充空白
        g.resize(4, 8);
        assert_eq!(line_text(&g, 0), "abcd    ");
        assert_eq!(line_text(&g, 3), "        ");
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
        assert_eq!(g.scrollback_len(), 1);
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
        assert_eq!(g.scrollback_len(), 2); // ccc + XY
        // SD 1：底行(空)进滚回，整体下移，顶行补空
        g.feed(b"\x1b[1T");
        assert_eq!(line_text(&g, 0), "    ");
        assert_eq!(line_text(&g, 1), "Z   ");
    }

    #[test]
    fn tab_and_backspace() {
        let mut g = grid(2, 8);
        g.feed(b"a\tb");
        // tab 目标第 8 列 = 末列 → 延迟 wrap：'b' 换行到下一行（真实 xterm 同行为）
        assert_eq!(line_text(&g, 0), "a       ");
        assert_eq!(line_text(&g, 1), "b       ");
        assert_eq!(g.cursor(), (1, 1));
        g.feed(b"\x08\x08");
        assert_eq!(g.cursor(), (1, 0));
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
