//! 快照：把权威终端还原成一段字节，喂进一台**同尺寸**的干净终端就得到同一副样子
//! （回滚 + 屏幕逐格、软换行、滚动区域、模式、光标、当前画笔）。
//! 消费方必须先按 `cols × lines` 建好/resize 终端，再喂 `bytes`。
//!
//! 为什么不录原始字节重放：TUI 靠整屏重绘刷新，几 MB 原始字节里绝大部分是覆盖同一块
//! 屏幕的重绘，换不来 10000 行历史；环形缓冲区的头还会切在转义序列中间。
//!
//! 已知不还原的东西与未验证的点，单一事实源见设计 spec §15
//! （`docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`），这里不再重复列。
use std::fmt::Write as _;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use crate::screen::Screen;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: usize,
    pub lines: usize,
    pub bytes: Vec<u8>,
}

/// 写字时会被复制进格子的那部分属性（= alacritty 光标模板里决定格子长相的部分）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pen {
    pub(crate) fg: Color,
    pub(crate) bg: Color,
    pub(crate) flags: Flags,
    pub(crate) underline: Option<Color>,
    pub(crate) link: Option<Hyperlink>,
}

/// SGR 能表达的格子标志。WRAPLINE / 宽字符标志由写字过程自己产生，不在画笔里。
const STYLE_FLAGS: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::INVERSE)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT);

impl Pen {
    pub(crate) fn of(cell: &Cell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            flags: cell.flags & STYLE_FLAGS,
            underline: cell.underline_color(),
            link: cell.hyperlink(),
        }
    }

    fn reset() -> Self {
        Self::of(&Cell::default())
    }
}

impl Screen {
    /// 拍一张快照。纯读，不改终端状态。
    pub fn snapshot(&self) -> Snapshot {
        let term = self.term();
        let grid = term.grid();
        let mode = *term.mode();
        let cols = grid.columns();
        let lines = grid.screen_lines();
        let mut out = Encoder {
            bytes: Vec::new(),
            pen: Pen::reset(),
        };

        // 1. 硬复位：之后滚动区域、模式、字符集、画笔全是已知初值。
        out.raw("\x1bc");
        if mode.contains(TermMode::ALT_SCREEN) {
            out.raw("\x1b[?1049h");
        }

        // 2. 回滚 + 屏幕，逐行。
        let first = -(grid.history_size() as i32);
        let last = lines as i32 - 1;
        let mut prev_wrapped = false;
        for line in first..=last {
            let row = &grid[Line(line)];
            let wrapped = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            if line > first && !prev_wrapped {
                // 换行若引起上滚，新行会拿当前背景色填（BCE）——先把背景复位。
                if out.pen.bg != Color::Named(NamedColor::Background) {
                    out.raw("\x1b[49m");
                    out.pen.bg = Color::Named(NamedColor::Background);
                }
                out.raw("\r\n");
            }
            let next_starts_wide = line < last
                && grid[Line(line + 1)][Column(0)]
                    .flags
                    .contains(Flags::WIDE_CHAR);
            out.row(row, cols, wrapped, prev_wrapped, next_starts_wide);
            prev_wrapped = wrapped;
        }

        // 2b. 最底一行带 WRAPLINE 却没有下一行（竖向缩小后常见）：直接写只能停在
        //     pending-wrap，标志要靠真的折下去才有。在第 2..lines 行的滚动区域里折一次
        //     （区域不含第 0 行 → 上滚的行不进回滚），SD 滚回来，再补写被挤掉的第 1 行。
        //     少于 3 行时做不到（区域至少两行），留作已知缺口。
        let bottom_row = &grid[Line(last)];
        if lines >= 3 && bottom_row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
            let col = if bottom_row[Column(cols - 1)]
                .flags
                .contains(Flags::WIDE_CHAR_SPACER)
            {
                cols - 2
            } else {
                cols - 1
            };
            out.set_pen(&Pen::reset());
            out.raw(&format!("\x1b[2;{lines}r\x1b[{lines};{}H", col + 1));
            out.cell(&bottom_row[Column(col)]);
            out.set_pen(&Pen::reset());
            out.raw(" \x1b[T\x1b[r\x1b[2;1H");
            let row1 = &grid[Line(1)];
            let wrapped1 = row1[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            let row2 = &grid[Line(2)];
            let row2_wide = row2[Column(0)].flags.contains(Flags::WIDE_CHAR);
            out.row(row1, cols, wrapped1, false, row2_wide);
            if wrapped1 {
                // 再写一次第 2 行行首那格（内容不变），触发第 1 行的折行标志。
                out.cell(&row2[Column(0)]);
            }
        }

        // 3. 滚动区域（DECSTBM 会把光标拉回原点，所以排在内容之后、定位之前）。
        let (top, bottom) = self.scroll_region();
        if (top, bottom) != (0, lines) {
            out.raw(&format!("\x1b[{};{}r", top + 1, bottom));
        }

        // 4. 光标。原点模式要在定位之前打开（打开它会把光标拉回区域原点）。
        let cursor = self.cursor();
        let row_offset = if mode.contains(TermMode::ORIGIN) {
            out.raw("\x1b[?6h");
            top
        } else {
            0
        };
        if cursor.pending_wrap {
            // 「写满最后一列、还没换行」只能靠真的再写一次那一格造出来。
            let row = &grid[Line(cursor.line as i32)];
            let col = if row[Column(cols - 1)]
                .flags
                .contains(Flags::WIDE_CHAR_SPACER)
            {
                cols - 2
            } else {
                cols - 1
            };
            out.raw(&format!(
                "\x1b[{};{}H",
                cursor.line.saturating_sub(row_offset) + 1,
                col + 1
            ));
            out.cell(&row[Column(col)]);
        } else {
            out.raw(&format!(
                "\x1b[{};{}H",
                cursor.line.saturating_sub(row_offset) + 1,
                cursor.col + 1
            ));
        }

        // 5. 模式（都不动光标、不清 pending-wrap）。
        if mode.contains(TermMode::APP_CURSOR) {
            out.raw("\x1b[?1h");
        }
        if mode.contains(TermMode::APP_KEYPAD) {
            out.raw("\x1b=");
        }
        if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            out.raw("\x1b[?1000h");
        }
        if mode.contains(TermMode::MOUSE_DRAG) {
            out.raw("\x1b[?1002h");
        }
        if mode.contains(TermMode::MOUSE_MOTION) {
            out.raw("\x1b[?1003h");
        }
        if mode.contains(TermMode::UTF8_MOUSE) {
            out.raw("\x1b[?1005h");
        }
        if mode.contains(TermMode::SGR_MOUSE) {
            out.raw("\x1b[?1006h");
        }
        if mode.contains(TermMode::FOCUS_IN_OUT) {
            out.raw("\x1b[?1004h");
        }
        if mode.contains(TermMode::BRACKETED_PASTE) {
            out.raw("\x1b[?2004h");
        }
        if !mode.contains(TermMode::ALTERNATE_SCROLL) {
            out.raw("\x1b[?1007l");
        }
        if !mode.contains(TermMode::URGENCY_HINTS) {
            out.raw("\x1b[?1042l");
        }
        if !mode.contains(TermMode::LINE_WRAP) {
            out.raw("\x1b[?7l");
        }
        if mode.contains(TermMode::INSERT) {
            out.raw("\x1b[4h");
        }
        if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
            out.raw("\x1b[20h");
        }
        // 显隐一律显式写（SwiftTerm 的 RIS 不复位 cursorHidden）。
        out.raw(if cursor.visible {
            "\x1b[?25h"
        } else {
            "\x1b[?25l"
        });

        // 6. 当前画笔：快照之后程序接着写的字要跟原来一个颜色。
        out.set_pen(&Pen::of(&grid.cursor.template));

        Snapshot {
            cols,
            lines,
            bytes: out.bytes,
        }
    }
}

struct Encoder {
    bytes: Vec<u8>,
    pen: Pen,
}

impl Encoder {
    fn raw(&mut self, s: &str) {
        self.bytes.extend_from_slice(s.as_bytes());
    }

    fn row(
        &mut self,
        row: &alacritty_terminal::grid::Row<Cell>,
        cols: usize,
        wrapped: bool,
        continuation: bool,
        next_starts_wide: bool,
    ) {
        let blank = Cell::default();
        let mut end = if wrapped {
            cols
        } else {
            (0..cols)
                .rev()
                .find(|&c| row[Column(c)] != blank)
                .map_or(0, |c| c + 1)
        };
        // 软换行的续行至少写一格：那一格才真正触发上一行的换行。
        if continuation && end == 0 {
            end = 1;
        }
        // 终端光标在本行的列：和源格子的列对不上时（宽字符后半被擦/删）补 CHA 对齐。
        let mut cur = 0usize;
        for col in 0..end {
            let cell = &row[Column(col)];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            // 行尾放不下的宽字符留下的占位格：下一行开头写那个宽字符时会自动再造出来。
            if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
                && col == cols - 1
                && wrapped
                && next_starts_wide
            {
                continue;
            }
            if cur != col {
                self.raw(&format!("\x1b[{}G", col + 1));
            }
            if cell.c == '\t' {
                // 制表符在 alacritty 里存成格子里的 '\t'（空白格上才会写）。先按属性写个
                // 空格，回到这一格补一个 HT 让它记成 '\t'，再把光标挪回下一格。
                self.set_pen(&Pen::of(cell));
                self.raw(&format!(" \x1b[{}G\t", col + 1));
                if col + 1 < cols {
                    self.raw(&format!("\x1b[{}G", col + 2));
                }
                cur = col + 1;
                continue;
            }
            self.cell(cell);
            cur = col + 1;
            if cell.flags.contains(Flags::WIDE_CHAR) {
                cur = col + 2;
                if col + 1 < cols && !row[Column(col + 1)].flags.contains(Flags::WIDE_CHAR_SPACER) {
                    // 宽字符的后半格被 ECH/DCH 动过：写宽字符会自带一个后半格，得把它擦回去
                    // （直接往上写会连宽字符一起清掉）。擦除要用默认底色，后面该写的格子自己会写。
                    self.default_background();
                    self.raw(&format!("\x1b[{}G\x1b[X", col + 2));
                    cur = col + 1;
                }
            }
        }
        if continuation && end < cols {
            // 续行第一格触发的折行上滚会把整行填成那一格的底色：行尾没写到的格子擦回默认。
            self.default_background();
            self.raw("\x1b[K");
        }
        // 行尾关掉超链接：下一行（或下一次折行）再按需重开。
        if self.pen.link.is_some() {
            self.raw("\x1b]8;;\x1b\\");
            self.pen.link = None;
        }
    }

    fn default_background(&mut self) {
        if self.pen.bg != Color::Named(NamedColor::Background) {
            self.raw("\x1b[49m");
            self.pen.bg = Color::Named(NamedColor::Background);
        }
    }

    fn cell(&mut self, cell: &Cell) {
        self.set_pen(&Pen::of(cell));
        let mut buf = [0u8; 4];
        self.bytes
            .extend_from_slice(cell.c.encode_utf8(&mut buf).as_bytes());
        if let Some(zerowidth) = cell.zerowidth() {
            for c in zerowidth {
                self.bytes
                    .extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }

    fn set_pen(&mut self, pen: &Pen) {
        if (&pen.fg, &pen.bg, pen.flags, &pen.underline)
            != (
                &self.pen.fg,
                &self.pen.bg,
                self.pen.flags,
                &self.pen.underline,
            )
        {
            let sgr = sgr(pen);
            self.raw(&sgr);
        }
        if pen.link != self.pen.link {
            match &pen.link {
                Some(link) => {
                    if self.pen.link.is_some() {
                        // SwiftTerm 只在关闭时才应用链接、且新开会顶掉未关的：先显式关。
                        self.raw("\x1b]8;;\x1b\\");
                    }
                    let osc = format!("\x1b]8;id={};{}\x1b\\", link.id(), link.uri());
                    self.raw(&osc);
                }
                None => self.raw("\x1b]8;;\x1b\\"),
            }
        }
        self.pen = pen.clone();
    }
}

/// 一个画笔的完整 SGR。总是从 0 重来，不做差量（差量正是这类代码最爱漏复位的地方）。
pub(crate) fn sgr(pen: &Pen) -> String {
    let mut s = String::from("\x1b[0");
    let f = pen.flags;
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::UNDERLINE, "4"),
        (Flags::DOUBLE_UNDERLINE, "4:2"),
        (Flags::UNDERCURL, "4:3"),
        (Flags::DOTTED_UNDERLINE, "4:4"),
        (Flags::DASHED_UNDERLINE, "4:5"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if f.contains(flag) {
            s.push(';');
            s.push_str(code);
        }
    }
    color(&mut s, pen.fg, 30);
    color(&mut s, pen.bg, 40);
    if let Some(underline) = pen.underline {
        color(&mut s, underline, 50);
    }
    s.push('m');
    s
}

/// `base` 30 前景、40 背景、50 下划线色。Named 0–7 → base+n，8–15 → base+60+(n-8)
/// （下划线色没有这两档，走 58;5;n）；Indexed → 38/48/58;5;n；Spec → ;2;r;g;b。
/// 默认前景/背景不写（SGR 0 已复位）。
fn color(s: &mut String, c: Color, base: u16) {
    let extended = base + 8;
    match c {
        Color::Named(n) => {
            let n = n as usize;
            if n < 16 {
                if base == 50 {
                    let _ = write!(s, ";58;5;{n}");
                } else if n < 8 {
                    let _ = write!(s, ";{}", base as usize + n);
                } else {
                    let _ = write!(s, ";{}", base as usize + 60 + n - 8);
                }
            }
        }
        Color::Indexed(i) => {
            let _ = write!(s, ";{extended};5;{i}");
        }
        Color::Spec(rgb) => {
            let _ = write!(s, ";{extended};2;{};{};{}", rgb.r, rgb.g, rgb.b);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::screen::ScreenConfig;

    fn screen(cols: usize, lines: usize) -> Screen {
        Screen::new(ScreenConfig {
            cols,
            lines,
            ..ScreenConfig::default()
        })
    }

    fn fed(cols: usize, lines: usize, bytes: &[u8]) -> Screen {
        let mut s = screen(cols, lines);
        s.feed(bytes);
        s
    }

    /// 快照喂进一台同尺寸的新终端，逐格、光标、模式、滚动区域、画笔都得一样。
    pub(crate) fn assert_round_trip(src: &Screen, what: &str) {
        let snap = src.snapshot();
        let dst = fed(snap.cols, snap.lines, &snap.bytes);
        assert_same(src, &dst, what);
    }

    pub(crate) fn assert_same(a: &Screen, b: &Screen, what: &str) {
        let (ta, tb) = (a.term(), b.term());
        assert_eq!((a.cols(), a.lines()), (b.cols(), b.lines()), "{what}：尺寸");
        assert_eq!(a.history_size(), b.history_size(), "{what}：回滚行数");
        let first = -(a.history_size() as i32);
        for line in first..a.lines() as i32 {
            let (ra, rb) = (&ta.grid()[Line(line)], &tb.grid()[Line(line)]);
            for col in 0..a.cols() {
                let (ca, cb) = (&ra[Column(col)], &rb[Column(col)]);
                assert_eq!(ca, cb, "{what}：第 {line} 行第 {col} 格");
            }
        }
        assert_eq!(a.cursor(), b.cursor(), "{what}：光标");
        assert_eq!(ta.mode(), tb.mode(), "{what}：模式");
        assert_eq!(a.scroll_region(), b.scroll_region(), "{what}：滚动区域");
        assert_eq!(
            Pen::of(&ta.grid().cursor.template),
            Pen::of(&tb.grid().cursor.template),
            "{what}：当前画笔"
        );
    }

    #[test]
    fn starts_with_a_hard_reset() {
        let snap = fed(80, 25, b"hi").snapshot();
        assert!(snap.bytes.starts_with(b"\x1bc"));
        assert_eq!((snap.cols, snap.lines), (80, 25));
    }

    #[test]
    fn plain_text_round_trips() {
        assert_round_trip(&fed(80, 25, b"hello\r\nworld"), "纯文本");
        assert_round_trip(&fed(80, 25, b""), "空屏");
    }

    #[test]
    fn sgr_family_round_trips() {
        let bytes = concat!(
            "\x1b[1mB\x1b[2mD\x1b[3mI\x1b[4mU\x1b[4:2mUU\x1b[4:3mC\x1b[4:4mO\x1b[4:5mA",
            "\x1b[7mR\x1b[8mH\x1b[9mS\x1b[0m",
            "\x1b[31mr\x1b[91mbr\x1b[38;5;1mi1\x1b[38;5;200mi\x1b[38;2;1;2;3mt",
            "\x1b[44mb\x1b[104mbb\x1b[48;5;17mib\x1b[48;2;9;8;7mtb",
            "\x1b[4;58;5;3mu\x1b[58;2;4;5;6mv\x1b[59mw\x1b[0m."
        );
        assert_round_trip(&fed(80, 25, bytes.as_bytes()), "SGR");
    }

    #[test]
    fn named_and_indexed_colors_stay_distinct() {
        let s = fed(80, 25, b"\x1b[31mA\x1b[38;5;1mB");
        let snap = s.snapshot();
        let text = String::from_utf8_lossy(&snap.bytes);
        assert!(text.contains("\x1b[0;31mA"), "{text:?}");
        assert!(text.contains("\x1b[0;38;5;1mB"), "{text:?}");
    }

    #[test]
    fn wide_zero_width_and_tabs_round_trip() {
        let bytes =
            "我在，attach\r\ne\u{301}x 👩\u{200d}💻\r\na\tb\x1b[44m\tc\x1b[0m\r\n\x1b[78G中文";
        assert_round_trip(&fed(80, 25, bytes.as_bytes()), "宽字符/组合字符/制表符");
    }

    #[test]
    fn erased_cells_keep_their_background() {
        let bytes = b"\x1b[41m\x1b[2K\x1b[0mx\r\n\x1b[42m   \x1b[0m\r\n\x1b[43m\x1b[5X";
        assert_round_trip(&fed(80, 25, bytes), "带底色擦除");
    }

    #[test]
    fn background_does_not_bleed_into_scrolled_lines() {
        let mut bytes = String::new();
        for n in 0..30 {
            bytes.push_str(&format!("\x1b[41m{n}\x1b[0m\r\n"));
        }
        assert_round_trip(&fed(80, 25, bytes.as_bytes()), "换行上滚不带底色");
    }

    #[test]
    fn sparse_screen_and_cursor_round_trip() {
        let bytes = b"\x1b[5;10Hmid\x1b[20;70Hlow\x1b[12;33H";
        assert_round_trip(&fed(80, 25, bytes), "稀疏屏");
    }

    #[test]
    fn soft_wraps_keep_their_flag() {
        let long = format!("{}END\r\nnext", "x".repeat(170));
        let s = fed(80, 25, long.as_bytes());
        assert!(
            s.term().grid()[Line(0)][Column(79)]
                .flags
                .contains(Flags::WRAPLINE)
        );
        assert_round_trip(&s, "软换行");
    }

    #[test]
    fn blank_wrapped_continuation_is_not_lost() {
        // 续行整行空白：没有字符去触发那一折就会丢一整行（Swift 版第一版的 bug）。
        let bytes = format!("{}{}\r\nafter", "x".repeat(80), " ".repeat(80));
        assert_round_trip(&fed(80, 25, bytes.as_bytes()), "空白续行");
    }

    #[test]
    fn wide_char_that_did_not_fit_wraps_the_same_way() {
        let bytes = format!("{}中", "x".repeat(79));
        let s = fed(80, 25, bytes.as_bytes());
        assert!(
            s.term().grid()[Line(0)][Column(79)]
                .flags
                .contains(Flags::LEADING_WIDE_CHAR_SPACER)
        );
        assert_round_trip(&s, "行尾放不下的宽字符");
    }

    #[test]
    fn dangling_wrap_on_the_bottom_row_survives() {
        // 竖向缩小后，最底一行的 WRAPLINE 指向一行已经不在屏上的续行。
        let text = format!("top\r\nmid\r\n{}", "a".repeat(85));
        let mut s = fed(80, 25, text.as_bytes());
        s.feed(b"\x1b[1;1H");
        assert!(s.resize(80, 3));
        let bottom = &s.term().grid()[Line(2)];
        assert!(
            bottom[Column(79)].flags.contains(Flags::WRAPLINE),
            "前置条件：底行悬空折行"
        );
        assert_round_trip(&s, "底行悬空折行");

        // 第 1 行自己也折进底行：补写第 1 行时要把它的折行标志一并造回来。
        let mut s = fed(80, 25, format!("top\r\n{}", "a".repeat(250)).as_bytes());
        s.feed(b"\x1b[1;1H");
        assert!(s.resize(80, 3));
        let grid = s.term().grid();
        assert!(grid[Line(1)][Column(79)].flags.contains(Flags::WRAPLINE));
        assert!(grid[Line(2)][Column(79)].flags.contains(Flags::WRAPLINE));
        assert_round_trip(&s, "第 1 行与底行都折行");
    }

    #[test]
    fn wrap_on_last_row_while_scrollback_grows() {
        let mut bytes = String::new();
        for n in 0..40 {
            bytes.push_str(&format!("line {n}\r\n"));
        }
        bytes.push_str(&"y".repeat(200));
        assert_round_trip(&fed(80, 25, bytes.as_bytes()), "底行软换行 + 回滚");
    }

    #[test]
    fn scrollback_is_restored_in_full() {
        let mut s = screen(80, 25);
        for n in 0..10_050 {
            s.feed(format!("\x1b[3{}mline {n}\r\n", n % 8).as_bytes());
        }
        assert_eq!(s.history_size(), 10_000);
        assert_round_trip(&s, "满回滚");
    }

    #[test]
    fn pending_wrap_cursor_is_reproduced() {
        let s = fed(80, 25, "x".repeat(80).as_bytes());
        assert!(s.cursor().pending_wrap);
        assert_round_trip(&s, "pending-wrap");
        let s = fed(80, 25, format!("{}中", "x".repeat(78)).as_bytes());
        assert!(s.cursor().pending_wrap);
        assert_round_trip(&s, "pending-wrap 停在宽字符上");
    }

    #[test]
    fn modes_are_restored() {
        let bytes = b"\x1b[?1h\x1b=\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[?2004h\x1b[?1007l\x1b[?1042l\x1b[?7l\x1b[4h\x1b[20h\x1b[?25l";
        assert_round_trip(&fed(80, 25, bytes), "模式");
        assert_round_trip(
            &fed(80, 25, b"\x1b[?1000h\x1b[?1005h"),
            "鼠标点击 + UTF8 编码",
        );
        assert_round_trip(&fed(80, 25, b"\x1b[?1003h"), "鼠标任意移动");
    }

    #[test]
    fn scroll_region_and_origin_mode_are_restored() {
        assert_round_trip(&fed(80, 25, b"top\x1b[3;20r\x1b[10;5Hx"), "滚动区域");
        assert_round_trip(&fed(80, 25, b"\x1b[3;20r\x1b[?6h\x1b[4;7Hy"), "原点模式");
    }

    #[test]
    fn current_pen_and_hyperlink_are_restored() {
        let bytes = b"\x1b]8;id=a;https://example.com\x1b\\link\x1b]8;;\x1b\\ \x1b[1;35;48;5;22m";
        assert_round_trip(&fed(80, 25, bytes), "画笔 + 超链接");
        let open = b"\x1b]8;;https://example.com/x;y\x1b\\still linked";
        assert_round_trip(&fed(80, 25, open), "未关闭的超链接");
    }

    #[test]
    fn alternate_screen_content_is_restored() {
        let s = fed(80, 25, b"main\x1b[?1049halt screen\x1b[3;3Hz");
        assert_round_trip(&s, "alt 屏");
    }

    /// 已知缺口（钉成显式断言）：处在 alt 屏时拍的快照不带主屏内容——alacritty 不公开
    /// 非活动缓冲区。claude 从不用 alt 屏（见 claude_fixtures.rs）。哪天这条红了，
    /// 说明 alacritty 开了口子，回来收掉这个缺口。
    #[test]
    fn known_gap_main_screen_is_lost_while_in_alternate_screen() {
        let s = fed(80, 25, b"main\x1b[?1049halt");
        let snap = s.snapshot();
        let mut dst = fed(snap.cols, snap.lines, &snap.bytes);
        dst.feed(b"\x1b[?1049l");
        assert_eq!(dst.screen_rows(), Vec::<String>::new());
    }

    #[test]
    fn round_trip_survives_resizes() {
        let text = format!("{}\r\n短行\r\n{}", "w".repeat(150), "中".repeat(70));
        let mut s = fed(80, 25, text.as_bytes());
        for (cols, lines) in [(60, 25), (160, 25), (100, 10), (80, 25)] {
            assert!(s.resize(cols, lines));
            assert_round_trip(&s, &format!("resize {cols}x{lines}"));
        }
    }

    #[test]
    fn snapshot_does_not_change_the_source() {
        let s = fed(80, 25, b"\x1b[31mred\x1b[3;20r");
        let before = s.snapshot();
        let again = s.snapshot();
        assert_eq!(before, again);
    }

    fn text_of(s: &Screen) -> String {
        String::from_utf8_lossy(&s.snapshot().bytes).into_owned()
    }

    #[test]
    fn wrap_scroll_during_replay_does_not_bleed_the_trigger_background() {
        // 回放到屏底时，续行的第一格触发折行上滚；新行会被触发格的底色（BCE）填满。
        let mut bytes = String::new();
        for n in 0..40 {
            bytes.push_str(&format!("line {n}\r\n"));
        }
        bytes.push_str("\x1b[10;1H");
        bytes.push_str(&"x".repeat(80));
        bytes.push_str("\x1b[41m z\x1b[0m");
        assert_round_trip(&fed(80, 25, bytes.as_bytes()), "折行上滚不带底色");
    }

    #[test]
    fn wide_char_whose_spacer_was_erased_does_not_shift_the_row() {
        assert_round_trip(
            &fed(80, 25, "中abc\x1b[2G\x1b[P".as_bytes()),
            "宽字符后半被 DCH 删掉",
        );
        assert_round_trip(
            &fed(80, 25, "中abc\x1b[2G\x1b[X".as_bytes()),
            "宽字符后半被 ECH 擦掉",
        );
        assert_round_trip(
            &fed(80, 25, "x中\x1b[41m\x1b[3G\x1b[X".as_bytes()),
            "擦掉后半且带底色，行尾",
        );
    }

    #[test]
    fn cursor_visibility_is_always_explicit() {
        assert!(text_of(&fed(80, 25, b"hi")).contains("\x1b[?25h"));
        let hidden = text_of(&fed(80, 25, b"hi\x1b[?25l"));
        assert!(hidden.contains("\x1b[?25l"));
        assert!(!hidden.contains("\x1b[?25h"));
        assert_round_trip(&fed(80, 25, b"hi\x1b[?25l"), "隐藏光标");
    }

    #[test]
    fn hyperlinks_are_closed_before_switching_and_at_row_end() {
        let s = fed(
            80,
            25,
            b"\x1b]8;id=a;https://a\x1b\\A\x1b]8;id=b;https://b\x1b\\B\x1b]8;;\x1b\\",
        );
        let text = text_of(&s);
        assert!(
            text.contains("\x1b]8;;\x1b\\\x1b]8;id=b;https://b"),
            "{text:?}"
        );
        assert_round_trip(&s, "切换超链接");
        let s = fed(80, 25, b"\x1b]8;id=a;https://a\x1b\\link\r\nnext");
        let text = text_of(&s);
        assert!(text.contains("link\x1b]8;;\x1b\\\r\n"), "{text:?}");
        assert_round_trip(&s, "行尾关闭超链接");
    }
}
