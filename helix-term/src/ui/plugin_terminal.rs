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

/// 单个终端网格单元：字符 + SGR 颜色 + 粗体 + 显示宽度。
/// 宽字符（CJK）：主格 width=2，后续格 width=0 标记占位（render 跳过、背景连续）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCell {
    pub ch: char,
    /// 显示宽度：0 = 被前一个宽字符占用的占位格，1 = 普通，2 = 宽字符主格
    pub width: u8,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
}

impl Default for TerminalCell {
    fn default() -> Self {
        Self { ch: ' ', width: 1, fg: None, bg: None, bold: false }
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
    /// 终端 normal 模式的滚动查看偏移（0 = 显示活动区；>0 从滚回显示 offset 行）
    scroll_offset: usize,
    // 当前 SGR 属性（print 时写到单元格）
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    /// OSC 0/2 标题（term-title 钩子消费；feed 后 take 清空）
    last_title: Option<String>,
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
            scroll_offset: 0,
            fg: None,
            bg: None,
            bold: false,
            last_title: None,
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

    /// 滚动查看偏移（0 = 活动区底部）；上限为实际保留的滚回行数
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// 设置滚动查看偏移（终端 normal 模式滚动缓冲；clamp 到可用滚回）
    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_offset = offset.min(self.scrollback.len());
    }

    /// 滚动偏移增加 n（向滚回深处/顶部）；clamp
    pub fn scroll_up_view(&mut self, n: usize) {
        self.set_scroll_offset(self.scroll_offset.saturating_add(n));
    }

    /// 滚动偏移减少 n（向活动区/底部）；clamp
    pub fn scroll_down_view(&mut self, n: usize) {
        self.set_scroll_offset(self.scroll_offset.saturating_sub(n));
    }

    /// 滚回有可见行（normal 模式滚动时判断）
    pub fn has_scrollback(&self) -> bool {
        !self.scrollback.is_empty()
    }

    /// 把一块字节喂进 vte 解析器（复用同一 parser：块边界切开的 CSI/OSC 序列
    /// 跨 feed 调用保持状态）。chunk 内部字节无需完整——read_stream 只保证
    /// UTF-8 字符不跨块，逃逸序列跨块由本持久 parser 承接。
    /// 取走最近一次 OSC 标题（无则 None）
    pub fn take_title(&mut self) -> Option<String> {
        self.last_title.take()
    }

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
            *cell = TerminalCell { ch: ' ', width: 1, fg: None, bg: None, bold: false };
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

    /// 把网格画到 surface（area 左上角起，逐格写 symbol + style）。
    /// scroll_offset > 0 时顶部显示滚回行（纯查看，不改 pty 状态），活动区在底部。
    pub fn render(&self, area: Rect, surface: &mut Surface) {
        let sb = &self.scrollback;
        let scroll_rows = self.scroll_offset.min(sb.len());
        let sb_start = sb.len() - scroll_rows;
        for row in 0..area.height {
            let display_row = row as usize;
            let (line, line_row): (&[TerminalCell], usize) = if display_row < scroll_rows {
                // 滚回区：显示 scrollback[sb_start + display_row]
                (&sb[sb_start + display_row], display_row)
            } else {
                let grid_row = display_row - scroll_rows;
                if grid_row >= self.rows as usize {
                    break;
                }
                // 活动区：cells[grid_row]
                let base = grid_row * self.cols as usize;
                (&self.cells[base..base + self.cols as usize], display_row)
            };
            for col in 0..area.width {
                let grid_col = col as usize;
                if grid_col >= line.len() {
                    break;
                }
                let cell = line[grid_col];
                // 占位格（宽字符的第二个半格）：跳过，保持空格/背景连续
                if cell.width == 0 {
                    continue;
                }
                let Some(surface_cell) = surface.get_mut(area.x + col, area.y + line_row as u16) else {
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

    /// 可视区纯文本（含滚动偏移带入的滚回行；跳过宽字符占位格、去行尾空白）。
    /// 终端 normal 模式 y 复制、term-save 用。
    pub fn visible_text(&self) -> String {
        let sb = &self.scrollback;
        let scroll_rows = self.scroll_offset.min(sb.len());
        let sb_start = sb.len() - scroll_rows;
        let mut out = String::new();
        for display_row in 0..self.rows as usize {
            let line: &[TerminalCell] = if display_row < scroll_rows {
                &sb[sb_start + display_row]
            } else {
                let grid_row = display_row - scroll_rows;
                if grid_row >= self.rows as usize {
                    break;
                }
                &self.cells[grid_row * self.cols as usize..][..self.cols as usize]
            };
            let mut text = String::new();
            for cell in line {
                if cell.width == 0 {
                    continue; // 宽字符占位格
                }
                text.push(cell.ch);
            }
            out.push_str(text.trim_end());
            out.push('\n');
        }
        out
    }

    /// 全部内容纯文本（scrollback + 屏幕；跳过宽字符占位格、去行尾空白）。
    /// term-save 持久化用。
    pub fn full_text(&self) -> String {
        let mut out = String::new();
        for line in &self.scrollback {
            let mut text = String::new();
            for cell in line {
                if cell.width == 0 {
                    continue;
                }
                text.push(cell.ch);
            }
            out.push_str(text.trim_end());
            out.push('\n');
        }
        for row in 0..self.rows as usize {
            let base = row * self.cols as usize;
            let line = &self.cells[base..base + self.cols as usize];
            let mut text = String::new();
            for cell in line {
                if cell.width == 0 {
                    continue;
                }
                text.push(cell.ch);
            }
            out.push_str(text.trim_end());
            out.push('\n');
        }
        out
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
        // 宽字符（CJK）：主格 + 下一格占位（width 0）；末列时占位格丢弃
        let w = helix_core::unicode::width::UnicodeWidthChar::width(c).unwrap_or(1) as u8;
        if w > 1 {
            let cell = self.cell_mut(self.cursor.0, self.cursor.1);
            cell.ch = c;
            cell.width = 2;
            cell.fg = fg;
            cell.bg = bg;
            cell.bold = bold;
            self.cursor.1 += 1;
            if self.cursor.1 < self.cols {
                let next = self.cell_mut(self.cursor.0, self.cursor.1);
                next.ch = ' ';
                next.width = 0;
                next.fg = fg;
                next.bg = bg;
                next.bold = bold;
            }
            self.cursor.1 += 1;
            if self.cursor.1 >= self.cols {
                // 宽字符到达末列：置延迟 wrap（与 print 末列语义一致）
                self.cursor.1 = self.cols;
            }
        } else {
            let cell = self.cell_mut(self.cursor.0, self.cursor.1);
            cell.ch = c;
            cell.width = 1;
            cell.fg = fg;
            cell.bg = bg;
            cell.bold = bold;
            self.cursor.1 += 1;
        }
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

    // OSC（标题等）：0/2 ; <title> 记录到 last_title（渲染不消费，只供 term-title 钩子）
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.len() > 1 && matches!(params[0], b"0" | b"2") {
            if let Ok(title) = String::from_utf8(params[1].to_vec()) {
                self.last_title = Some(title);
            }
        }
    }
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
/// 支持：普通字符、Ctrl 组合（C-a → \x01 等）、Enter/Backspace/Tab、
/// 方向键/Home/End/PageUp/PageDown/Delete/Insert（xterm 应序）。
fn key_to_term_bytes(key: &helix_view::input::KeyEvent) -> Option<String> {
    use helix_view::input::{KeyCode, KeyModifiers};
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
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
        KeyCode::Up => Some("\x1b[A".into()),
        KeyCode::Down => Some("\x1b[B".into()),
        KeyCode::Right => Some("\x1b[C".into()),
        KeyCode::Left => Some("\x1b[D".into()),
        KeyCode::Home => Some("\x1b[H".into()),
        KeyCode::End => Some("\x1b[F".into()),
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

    /// 把一块 PTY 输出喂进网格（TermFeed 请求路由到此处）
    pub fn feed(&mut self, chunk: &str) {
        self.grid.feed(chunk.as_bytes());
        if let Some(title) = self.grid.take_title() {
            helix_js::emit_term_title(self.pty_id, &title);
        }
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
    }
}

impl Component for PluginTerminal {
    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        use helix_view::input::{KeyCode, KeyModifiers, MouseEventKind};
        // minimized：不消费按键，编辑器照常工作
        if self.mode == TermMode::Minimized {
            return EventResult::Ignored(None);
        }
        // 鼠标滚轮：滚动查看 scrollback（insert/normal 模式均可；其他鼠标事件交给编辑器）
        if let Event::Mouse(mouse) = event {
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
        let Event::Key(key) = event else {
            return EventResult::Ignored(None);
        };
        // term-key 钩子（插件决策优先）：pass/consume/minimize/close；无钩子 → 默认流程
        let key_desc = terminal_key_desc(key);
        match helix_js::emit_term_key(
            self.pty_id,
            &key_desc.0,
            key_desc.1,
            key_desc.2,
            key_desc.3,
        ) {
            Some(helix_js::TermKeyDecision::Pass) => {
                if let Some(bytes) = key_to_term_bytes(key) {
                    let _ = helix_js::term_write(self.pty_id, &bytes);
                }
                return EventResult::Consumed(None);
            }
            Some(helix_js::TermKeyDecision::Consume) => return EventResult::Consumed(None),
            Some(helix_js::TermKeyDecision::Minimize) => {
                return EventResult::Consumed(Some(Box::new(|compositor: &mut Compositor, _cx: &mut Context| {
                    if let Some(id) = compositor
                        .layout_tree()
                        .find_leaf_id::<PluginTerminal>(|_| true)
                    {
                        compositor.minimize_leaf(id, true);
                    }
                })))
            }
            Some(helix_js::TermKeyDecision::Close) => {
                return EventResult::Consumed(Some(Box::new(|compositor: &mut Compositor, _cx: &mut Context| {
                    compositor.remove_type::<PluginTerminal>();
                })))
            }
            None => {}
        }
        // C-\：Insert → Normal（终端内滚动查看）；Normal → 回 Insert 并聚焦编辑器
        let is_ctrl_backslash =
            key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('\\'));
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
            // 终端 normal 模式：导航滚动缓冲；i/a/Esc 回 insert；q 关闭
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
                KeyCode::Char('i') | KeyCode::Char('a') | KeyCode::Esc => {
                    self.input_mode = TermInputMode::Insert;
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
                    // term-close 钩子（reason="quit"）返回 false → 阻止关闭
                    if helix_js::emit_term_close(self.pty_id, "quit") {
                        return EventResult::Consumed(None);
                    }
                    EventResult::Consumed(Some(Box::new(
                        |compositor: &mut Compositor, _cx: &mut Context| {
                            compositor.remove_type::<PluginTerminal>();
                        },
                    )))
                }
                // 其余键直通 pty（C-c 中断等）
                _ => {
                    if let Some(bytes) = key_to_term_bytes(key) {
                        let _ = helix_js::term_write(self.pty_id, &bytes);
                    }
                    EventResult::Consumed(None)
                }
            }
        } else {
            // Insert 模式
            // Esc → 关闭面板（kill pty 由 Drop 兜底）；term-close 钩子（reason="esc"）返回 false → 阻止
            if key.code == KeyCode::Esc {
                if helix_js::emit_term_close(self.pty_id, "esc") {
                    return EventResult::Consumed(None);
                }
                return EventResult::Consumed(Some(Box::new(
                    |compositor: &mut Compositor, _cx: &mut Context| {
                        compositor.remove_type::<PluginTerminal>();
                    },
                )));
            }
            if let Some(bytes) = key_to_term_bytes(key) {
                // 按键直通 pty（非阻塞；worker 已退出时静默）
                let _ = helix_js::term_write(self.pty_id, &bytes);
            }
            EventResult::Consumed(None)
        }
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
            helix_js::emit_term_resize(self.pty_id, rows, cols);
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
        g2.feed("中".as_bytes());   // wrap 到下一行再写（末列宽字符先 wrap）
        assert_eq!(g2.cursor(), (1, 2), "末列宽字符 wrap 后占 2 格");
    }

    #[test]
    fn text_extraction_visible_and_full() {
        // 可视区/全量文本提取：scrollback + 屏幕、宽字符占位格跳过、行尾空白去除
        let mut g = grid(3, 6);
        g.feed(b"ab\r\ncd\r\nef\r\ngh"); // 3 行网格，4 次换行 → 1 行进滚回
        assert_eq!(g.scrollback_len(), 1);
        assert_eq!(g.visible_text(), "cd\nef\ngh\n", "可视区 = 活动区（滚回不显示）");
        assert_eq!(g.full_text(), "ab\ncd\nef\ngh\n", "全量 = scrollback + 屏幕");
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
        use helix_view::input::{KeyCode, KeyModifiers, KeyEvent};
        let k = |code: KeyCode, ctrl: bool| KeyEvent {
            code,
            modifiers: if ctrl { KeyModifiers::CONTROL } else { KeyModifiers::NONE },
        };
        assert_eq!(key_to_term_bytes(&k(KeyCode::Char('c'), true)).as_deref(), Some("\x03"), "C-c");
        assert_eq!(key_to_term_bytes(&k(KeyCode::Char('a'), true)).as_deref(), Some("\x01"), "C-a");
        assert_eq!(key_to_term_bytes(&k(KeyCode::Char('x'), false)).as_deref(), Some("x"));
        assert_eq!(key_to_term_bytes(&k(KeyCode::Up, false)).as_deref(), Some("\x1b[A"), "上箭头");
        assert_eq!(key_to_term_bytes(&k(KeyCode::PageDown, false)).as_deref(), Some("\x1b[6~"), "PageDown");
        assert_eq!(key_to_term_bytes(&k(KeyCode::Home, false)).as_deref(), Some("\x1b[H"), "Home");
        // C-\ 不在 key_to_term_bytes（handle_event 拦截）
        assert_eq!(key_to_term_bytes(&k(KeyCode::Char('\\'), true)), None, "C-\\ 应被拦截");
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
            TerminalCell { ch: 'X', fg: Some(Color::Green), bg: None, bold: false, width: 1 }
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
        assert_eq!(cell_at(&g, 0, 0), TerminalCell { ch: 'r', fg: Some(Color::Red), bg: None, bold: false, width: 1 });
        assert_eq!(cell_at(&g, 0, 3), TerminalCell { ch: 'b', fg: Some(Color::Green), bg: None, bold: true, width: 1 });
        // 第 13 个字符（bold-green 的 n）wrap 到第 1 行
        assert_eq!(cell_at(&g, 1, 0), TerminalCell { ch: 'n', fg: Some(Color::Green), bg: None, bold: true, width: 1 });
        assert_eq!(cell_at(&g, 1, 4), TerminalCell { ch: 'i', fg: None, bg: None, bold: false, width: 1 });
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
