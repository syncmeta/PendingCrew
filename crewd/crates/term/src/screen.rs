//! 权威终端：alacritty_terminal 的薄封装。只做三件事——喂字节、把终端自己的
//! 回复（DA1/DSR/OSC 11/XTWINOPS…）收进发件箱让调用方写回 PTY、读屏。
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{NamedColor, Processor, Rgb, Timeout};

use crate::side::SideTracker;

/// 屏幕参数。缺省值 = 今天 Swift 版的 PTY：80×25、回滚 10000 行、黑底白字。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenConfig {
    pub cols: usize,
    pub lines: usize,
    pub scrollback: usize,
    /// OSC 10 的回答。
    pub foreground: Rgb,
    /// OSC 11 的回答——claude 据此选明暗主题。黑 = 与 Swift 版（SwiftTerm 报黑）一致。
    pub background: Rgb,
}

impl Default for ScreenConfig {
    fn default() -> Self {
        Self {
            cols: 80,
            lines: 25,
            scrollback: 10_000,
            foreground: Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff,
            },
            background: Rgb { r: 0, g: 0, b: 0 },
        }
    }
}

/// 光标。`pending_wrap` = 刚写满最后一列、下一个字符才真正换行（alacritty 此时报
/// 列 = cols-1）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorPos {
    pub line: usize,
    pub col: usize,
    pub pending_wrap: bool,
    pub visible: bool,
}

/// 同步输出（`?2026`）不缓冲：alacritty 默认把 `?2026h` 之后最多 150ms 的字节攒着不画，
/// 那段时间里读屏/拍快照会漏。no-op 超时 = 立即处理（spike 已验证）。
#[derive(Default)]
pub(crate) struct NoSync;

impl Timeout for NoSync {
    fn set_timeout(&mut self, _: Duration) {}
    fn clear_timeout(&mut self) {}
    fn pending_timeout(&self) -> bool {
        false
    }
}

struct Outbox {
    bytes: Vec<u8>,
    size: WindowSize,
    foreground: Rgb,
    background: Rgb,
}

/// alacritty 的事件出口。只收「要写回 PTY」的三种，其余（标题、响铃…）丢掉。
#[derive(Clone)]
pub(crate) struct Replies(Arc<Mutex<Outbox>>);

impl Replies {
    fn lock(&self) -> std::sync::MutexGuard<'_, Outbox> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl EventListener for Replies {
    fn send_event(&self, event: Event) {
        let mut outbox = self.lock();
        match event {
            Event::PtyWrite(text) => outbox.bytes.extend_from_slice(text.as_bytes()),
            Event::ColorRequest(index, format) => {
                let rgb = color_for_index(index, outbox.foreground, outbox.background);
                outbox.bytes.extend_from_slice(format(rgb).as_bytes());
            }
            Event::TextAreaSizeRequest(format) => {
                let text = format(outbox.size);
                outbox.bytes.extend_from_slice(text.as_bytes());
            }
            _ => {}
        }
    }
}

fn window_size(cols: usize, lines: usize) -> WindowSize {
    WindowSize {
        num_lines: lines as u16,
        num_cols: cols as u16,
        cell_width: 16,
        cell_height: 16,
    }
}

/// OSC 4/10/11/12 的回答。0..=255 用 xterm 默认调色板；256 前景、257 背景、258 光标。
pub(crate) fn color_for_index(index: usize, foreground: Rgb, background: Rgb) -> Rgb {
    const BASE16: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    if index == NamedColor::Background as usize {
        return background;
    }
    if index >= NamedColor::Foreground as usize {
        return foreground;
    }
    if index < 16 {
        let (r, g, b) = BASE16[index];
        return Rgb { r, g, b };
    }
    if index < 232 {
        let step = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
        let i = index - 16;
        return Rgb {
            r: step(i / 36),
            g: step((i / 6) % 6),
            b: step(i % 6),
        };
    }
    let level = (8 + (index - 232) * 10) as u8;
    Rgb {
        r: level,
        g: level,
        b: level,
    }
}

struct Size {
    cols: usize,
    lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// 一台无头终端。纯计算：不起进程、不读时钟、不碰 IO。
pub struct Screen {
    term: Term<Replies>,
    parser: Processor<NoSync>,
    replies: Replies,
    side: SideTracker,
}

impl Screen {
    pub fn new(config: ScreenConfig) -> Self {
        let replies = Replies(Arc::new(Mutex::new(Outbox {
            bytes: Vec::new(),
            size: window_size(config.cols, config.lines),
            foreground: config.foreground,
            background: config.background,
        })));
        let term_config = Config {
            scrolling_history: config.scrollback,
            ..Config::default()
        };
        let size = Size {
            cols: config.cols,
            lines: config.lines,
        };
        let term = Term::new(term_config, &size, replies.clone());
        let side = SideTracker::new(config.lines);
        Self {
            term,
            parser: Processor::new(),
            replies,
            side,
        }
    }

    /// 喂一段 PTY 输出。可以在任意字节处切块（转义序列、UTF-8 中间都行）。
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
        self.side.advance(bytes);
        let focus_requests = self.side.take_focus_requests();
        if focus_requests > 0 {
            let mut outbox = self.replies.lock();
            for _ in 0..focus_requests {
                // 我们就是「有焦点的那个窗口」：SwiftTerm 在 `?1004h` 时立刻报 focus-in。
                outbox.bytes.extend_from_slice(b"\x1b[I");
            }
        }
    }

    /// 取走终端攒下的回复字节（调用方原样写回 PTY）。取完即空。
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies.lock().bytes)
    }

    /// 改尺寸（alacritty 会 reflow）。`cols < 2 || lines < 1` 忽略并返回 false。
    pub fn resize(&mut self, cols: usize, lines: usize) -> bool {
        if cols < 2 || lines < 1 {
            return false;
        }
        // alacritty 的 `Term::resize` 在尺寸没变时提前返回、保留滚动区域；旁路要同样处理。
        let changed = cols != self.term.columns() || lines != self.term.screen_lines();
        self.term.resize(Size { cols, lines });
        if changed {
            self.side.resize(lines);
        }
        self.replies.lock().size = window_size(cols, lines);
        true
    }

    /// 滚动区域：0 起，上界含、下界不含。整屏 = `(0, lines)`。
    pub fn scroll_region(&self) -> (usize, usize) {
        self.side.region()
    }

    pub fn cols(&self) -> usize {
        self.term.columns()
    }

    pub fn lines(&self) -> usize {
        self.term.screen_lines()
    }

    /// 回滚区里有多少行（不含当前屏）。
    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    pub fn cursor(&self) -> CursorPos {
        let cursor = &self.term.grid().cursor;
        CursorPos {
            line: cursor.point.line.0.max(0) as usize,
            col: cursor.point.column.0,
            pending_wrap: cursor.input_needs_wrap,
            visible: self.term.mode().contains(TermMode::SHOW_CURSOR),
        }
    }

    /// 当前屏逐行文本（TerminalScreenText 口径），尾部空行去掉。
    pub fn screen_rows(&self) -> Vec<String> {
        let mut rows: Vec<String> = (0..self.lines())
            .map(|line| self.line_text(line as i32))
            .collect();
        while rows.last().is_some_and(|row| row.trim().is_empty()) {
            rows.pop();
        }
        rows
    }

    /// 当前屏文本，最多 `max_lines` 行（取尾部），`\n` 连接。
    pub fn screen_text(&self, max_lines: usize) -> String {
        let rows = self.screen_rows();
        let start = rows.len().saturating_sub(max_lines);
        rows[start..].join("\n")
    }

    /// 回滚区逐行文本，最旧的在前。
    pub fn history_rows(&self) -> Vec<String> {
        let history = self.history_size() as i32;
        (1..=history).rev().map(|i| self.line_text(-i)).collect()
    }

    /// 一行格子 → 一行文本：宽字符后半格丢掉；`\0`/`\t` 当空格；组合字符跟在主字符后；
    /// 行尾空格去掉、行首保留。`line` 为负 = 回滚区。
    fn line_text(&self, line: i32) -> String {
        let grid = self.term.grid();
        let row = &grid[Line(line)];
        let mut out = String::with_capacity(grid.columns());
        for col in 0..grid.columns() {
            let cell: &Cell = &row[Column(col)];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            out.push(match cell.c {
                '\0' | '\t' => ' ',
                c => c,
            });
            if let Some(zerowidth) = cell.zerowidth() {
                out.extend(zerowidth.iter());
            }
        }
        let trimmed = out.trim_end_matches(' ').len();
        out.truncate(trimmed);
        out
    }

    /// 给快照编码器读格子用。不对 crate 外公开——外面不该依赖 alacritty 的类型。
    pub(crate) fn term(&self) -> &Term<Replies> {
        &self.term
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Screen {
        Screen::new(ScreenConfig::default())
    }

    #[test]
    fn default_config_is_80x25_with_10k_scrollback() {
        let s = screen();
        assert_eq!((s.cols(), s.lines()), (80, 25));
        let mut s = screen();
        for n in 0..10_100 {
            s.feed(format!("line {n}\r\n").as_bytes());
        }
        assert_eq!(s.history_size(), 10_000);
        assert_eq!(s.screen_rows()[0], "line 10076");
    }

    #[test]
    fn answers_device_queries_into_the_outbox() {
        let mut s = screen();
        s.feed(b"\x1b[c");
        assert_eq!(s.take_replies(), b"\x1b[?6c");
        assert!(s.take_replies().is_empty(), "取完即空");
        s.feed(b"\x1b[3;7H\x1b[6n");
        assert_eq!(s.take_replies(), b"\x1b[3;7R");
        s.feed(b"\x1b[18t");
        assert_eq!(s.take_replies(), b"\x1b[8;25;80t");
        s.feed(b"\x1b[14t");
        assert_eq!(s.take_replies(), b"\x1b[4;400;1280t");
    }

    #[test]
    fn osc_color_queries_use_configured_colors() {
        let mut s = Screen::new(ScreenConfig {
            background: Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            },
            ..ScreenConfig::default()
        });
        s.feed(b"\x1b]11;?\x07");
        assert_eq!(s.take_replies(), b"\x1b]11;rgb:1212/3434/5656\x07");
        s.feed(b"\x1b]10;?\x1b\\");
        assert_eq!(s.take_replies(), b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\");
        s.feed(b"\x1b]4;1;?\x07");
        assert_eq!(s.take_replies(), b"\x1b]4;1;rgb:cdcd/0000/0000\x07");
    }

    #[test]
    fn palette_matches_xterm_defaults() {
        let fg = Rgb { r: 1, g: 2, b: 3 };
        let bg = Rgb { r: 4, g: 5, b: 6 };
        assert_eq!(color_for_index(16, fg, bg), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(
            color_for_index(196, fg, bg),
            Rgb {
                r: 0xff,
                g: 0,
                b: 0
            }
        );
        assert_eq!(
            color_for_index(231, fg, bg),
            Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff
            }
        );
        assert_eq!(color_for_index(232, fg, bg), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(
            color_for_index(255, fg, bg),
            Rgb {
                r: 238,
                g: 238,
                b: 238
            }
        );
        assert_eq!(color_for_index(256, fg, bg), fg);
        assert_eq!(color_for_index(257, fg, bg), bg);
        assert_eq!(color_for_index(258, fg, bg), fg);
    }

    #[test]
    fn synchronized_output_is_drawn_immediately() {
        let mut s = screen();
        s.feed(b"\x1b[?2026hHELLO");
        assert_eq!(s.screen_rows(), vec!["HELLO"]);
    }

    #[test]
    fn chunking_anywhere_gives_the_same_screen() {
        let bytes = "\x1b[1;31m我在\x1b[0m，\x1b]0;标题\x07attach\x1b[2;5Hok".as_bytes();
        let mut whole = screen();
        whole.feed(bytes);
        let mut bytewise = screen();
        for b in bytes {
            bytewise.feed(std::slice::from_ref(b));
        }
        assert_eq!(whole.screen_rows(), bytewise.screen_rows());
        assert_eq!(whole.screen_rows(), vec!["我在，attach", "    ok"]);
    }

    #[test]
    fn resize_reflows_and_updates_size_replies() {
        let mut s = screen();
        s.feed(format!("{}END", "x".repeat(100)).as_bytes());
        assert!(s.resize(160, 30));
        assert_eq!(s.screen_rows(), vec![format!("{}END", "x".repeat(100))]);
        s.feed(b"\x1b[18t");
        assert_eq!(s.take_replies(), b"\x1b[8;30;160t");
    }

    #[test]
    fn degenerate_resize_is_ignored() {
        let mut s = screen();
        assert!(!s.resize(1, 25));
        assert!(!s.resize(80, 0));
        assert_eq!((s.cols(), s.lines()), (80, 25));
    }

    #[test]
    fn cursor_reports_pending_wrap() {
        let mut s = screen();
        s.feed("x".repeat(80).as_bytes());
        let c = s.cursor();
        assert_eq!(
            (c.line, c.col, c.pending_wrap, c.visible),
            (0, 79, true, true)
        );
        s.feed(b"\x1b[?25l");
        assert!(!s.cursor().visible);
    }

    #[test]
    fn focus_reporting_gets_an_immediate_focus_in() {
        let mut s = screen();
        s.feed(b"\x1b[?1004h");
        assert_eq!(s.take_replies(), b"\x1b[I");
        s.feed(b"\x1b[?1004l\x1b[c");
        assert_eq!(s.take_replies(), b"\x1b[?6c");
    }

    /// 旁路记下的滚动区域必须和 alacritty 实际用的那个一致：在区域底部换行，
    /// 只有区域内的行上滚，区域外的行纹丝不动。
    #[test]
    fn tracked_region_matches_alacritty_behaviour() {
        let mut s = screen();
        for n in 0..25 {
            s.feed(format!("\x1b[{};1Hrow{n}", n + 1).as_bytes());
        }
        s.feed(b"\x1b[3;5r");
        assert_eq!(s.scroll_region(), (2, 5));
        s.feed(b"\x1b[5;1H\n");
        let rows = s.screen_rows();
        assert_eq!(&rows[0..6], ["row0", "row1", "row3", "row4", "", "row5"]);
        assert!(s.resize(80, 30));
        assert_eq!(s.scroll_region(), (0, 30));
    }

    #[test]
    fn focus_request_split_across_feeds_answers_once() {
        let mut s = screen();
        s.feed(b"\x1b[?10");
        assert_eq!(s.take_replies(), b"");
        s.feed(b"04h");
        assert_eq!(s.take_replies(), b"\x1b[I");
    }

    fn fill_rows(s: &mut Screen) {
        for n in 0..25 {
            s.feed(format!("\x1b[{};1Hrow{n}", n + 1).as_bytes());
        }
    }

    #[test]
    fn same_size_resize_keeps_the_region() {
        let mut s = screen();
        fill_rows(&mut s);
        s.feed(b"\x1b[3;5r");
        assert!(s.resize(80, 25));
        assert_eq!(s.scroll_region(), (2, 5));
        s.feed(b"\x1b[5;1H\n");
        let rows = s.screen_rows();
        assert_eq!(&rows[0..6], ["row0", "row1", "row3", "row4", "", "row5"]);
    }

    #[test]
    fn deccolm_resets_the_region_like_alacritty() {
        let mut s = screen();
        fill_rows(&mut s);
        s.feed(b"\x1b[3;5r\x1b[?3h");
        assert_eq!(s.scroll_region(), (0, 25));
        // deccolm 清屏；重画后在最底行换行，应整屏上滚。
        fill_rows(&mut s);
        s.feed(b"\x1b[25;1H\n");
        let rows = s.screen_rows();
        assert_eq!(&rows[0..2], ["row1", "row2"]);
        // 末尾空行被读屏口径丢掉：row24 在第 24 行（下标 23），其后只剩空白。
        assert_eq!(rows.len(), 24);
        assert_eq!(rows[23], "row24");
    }

    // ---- 读屏口径（TerminalScreenText 移植）----

    #[test]
    fn skipped_cells_read_as_spaces() {
        let mut s = screen();
        s.feed(b"\x1b[2J\x1b[1;1HClaude\x1b[1;13Hv2.1.260");
        assert_eq!(s.screen_rows(), vec!["Claude      v2.1.260"]);
    }

    #[test]
    fn wide_chars_are_not_split() {
        let mut s = screen();
        s.feed("\x1b[2J\x1b[1;1H我在，attach 探针测试中。".as_bytes());
        assert_eq!(s.screen_rows(), vec!["我在，attach 探针测试中。"]);
    }

    #[test]
    fn wide_chars_and_gaps_mix_on_one_row() {
        let mut s = screen();
        s.feed("\x1b[2J\x1b[1;1H我在\x1b[1;9Hattach 中".as_bytes());
        assert_eq!(s.screen_rows(), vec!["我在    attach 中"]);
    }

    #[test]
    fn trailing_blanks_and_blank_rows_are_dropped() {
        let mut s = screen();
        s.feed(b"\x1b[2J\x1b[1;1Hhi\x1b[3;1H   ");
        assert_eq!(s.screen_rows(), vec!["hi"]);
        s.feed(b"\x1b[2;1H\tx");
        assert_eq!(s.screen_rows(), vec!["hi", "        x"]);
    }

    #[test]
    fn combining_marks_stay_with_their_base() {
        let mut s = screen();
        s.feed("e\u{301}x".as_bytes());
        assert_eq!(s.screen_rows(), vec!["e\u{301}x"]);
    }

    #[test]
    fn screen_text_takes_the_tail() {
        let mut s = screen();
        s.feed(b"a\r\nb\r\nc");
        assert_eq!(s.screen_text(2), "b\nc");
        assert_eq!(s.screen_text(0), "");
    }

    #[test]
    fn history_rows_are_oldest_first() {
        let mut s = screen();
        for n in 0..30 {
            s.feed(format!("line {n}\r\n").as_bytes());
        }
        let history = s.history_rows();
        assert_eq!(history.len(), 6);
        assert_eq!(history[0], "line 0");
        assert_eq!(history[5], "line 5");
    }
}
