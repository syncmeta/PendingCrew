//! claude 输入框与阻塞对话框的识别（Swift `ClaudeInputBox` 的移植）。
//! 只作用在**渲染完的整屏文本**上（[`crate::Screen::screen_rows`]），不吃原始字节：
//! claude 用光标定位摆词，字节流里 `1.Yes,Itrustthisfolder` 序号后面没有空格。
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use crate::Screen;
use crate::menu::{BOX_CHARS, PendingDecision, parse_menu, parse_option, strip};

/// 输入行的提示符字形。`>` 也认（老版本 claude 用它）。
pub const INPUT_MARKERS: [char; 3] = ['❯', '›', '>'];

/// 从底往上找多少行（输入框下面压着框线与提示条，长输入还会折行）。
pub const INPUT_SCAN_DEPTH: usize = 15;

/// `bodyTailVisible` 比对正文末尾多少个非空白字符。
pub const BODY_TAIL_CHARS: usize = 64;

fn normalize(rows: &[String]) -> Vec<String> {
    rows.iter().map(|r| r.replace('\0', " ")).collect()
}

/// 这一屏是不是摆着一个需要人回答的对话框（信任文件夹 / 命令审批 / 选登录方式）。
pub fn blocking_dialog(rows: &[String]) -> Option<PendingDecision> {
    parse_menu(&normalize(rows).join("\n"))
}

/// 输入行提示符后面的内容；`None` = 屏上没有可投递的输入框（还没就绪，或正在等人回答）。
///
/// 有对话框就一律 `None`：没编号的信任框 `❯  No, exit` 单看一行跟「人在输入框里打了字」
/// 一模一样，判据只能落在整屏这一层。
pub fn input_row(rows: &[String]) -> Option<String> {
    let rows = normalize(rows);
    let i = input_row_index(&rows)?;
    let stripped = strip(&rows[i]);
    let first = stripped.chars().next()?;
    Some(stripped[first.len_utf8()..].trim().to_string())
}

/// 输入行在 `rows`（已 [`normalize`]）里的下标；判据同 [`input_row`]。
fn input_row_index(rows: &[String]) -> Option<usize> {
    if blocking_dialog(rows).is_some() {
        return None;
    }
    let start = rows.len().saturating_sub(INPUT_SCAN_DEPTH);
    for (i, line) in rows.iter().enumerate().skip(start).rev() {
        let stripped = strip(line);
        let Some(first) = stripped.chars().next() else {
            continue;
        };
        if !INPUT_MARKERS.contains(&first) {
            continue;
        }
        if parse_option(stripped).is_some() {
            return None;
        }
        return Some(i);
    }
    None
}

/// 输入框里此刻有没有人留下的草稿（T10d 的草稿闸：有草稿就什么都不往里敲）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputDraft {
    /// 空的，或只有 claude 自己的灰色占位提示（`Try "…"`）。
    Empty,
    /// 有字，且字是正常样式（有人打的）。原样的文本，多行用 `\n` 连。
    Text(String),
    /// 分不清：占位样式与正常样式混在一起、屏上没有输入框（对话框在场 / 还没画好）、或整屏没有任何样式
    /// （没颜色的终端里占位提示和打的字长得一样）。调用方按「有草稿」处理（fail-closed，T10d Q1）。
    Unknown,
}

/// 输入框下沿（横线、下角）的起笔字符。
pub const LOWER_EDGE_CHARS: [char; 9] = ['─', '━', '═', '╰', '└', '╚', '╯', '┘', '╝'];

/// 灰阶 256 色里算「中灰」的那一段（xterm 232..=255 是 24 级灰，232 最暗、255 最亮）：232..=250。
pub const PLACEHOLDER_GREY_INDEXED: std::ops::RangeInclusive<u8> = 232..=250;

/// truecolor 里算「中灰」的：三个分量彼此相差不超过 [`PLACEHOLDER_GREY_TOLERANCE`]，且都在这个范围里（太暗的看不见、
/// 太亮的就是正文的白）。
pub const PLACEHOLDER_GREY_RGB: std::ops::RangeInclusive<u8> = 96..=200;

/// 「灰」= r、g、b 两两之差不超过这么多。
pub const PLACEHOLDER_GREY_TOLERANCE: u8 = 16;

/// 一个格子算不算「占位样式」：claude 的占位提示是暗淡（SGR 2）或灰色前景。灰色认这几种（T10d fix R2）：命名的
/// 亮黑 / 暗前景、256 色的 8 号与中灰阶（[`PLACEHOLDER_GREY_INDEXED`]）、truecolor 的中灰（[`PLACEHOLDER_GREY_RGB`]）。
/// crewd 给子进程的是 `TERM=xterm-256color`、不设 `COLORTERM`，claude 多半走 256 色；truecolor 也认，免得它自己判出
/// 真彩时漏掉。
fn placeholder_styled(cell: &Cell) -> bool {
    cell.flags.contains(Flags::DIM)
        || match cell.fg {
            Color::Named(NamedColor::BrightBlack | NamedColor::DimForeground) => true,
            Color::Indexed(i) => i == 8 || PLACEHOLDER_GREY_INDEXED.contains(&i),
            Color::Spec(rgb) => {
                let (lo, hi) = (rgb.r.min(rgb.g).min(rgb.b), rgb.r.max(rgb.g).max(rgb.b));
                hi - lo <= PLACEHOLDER_GREY_TOLERANCE
                    && PLACEHOLDER_GREY_RGB.contains(&lo)
                    && PLACEHOLDER_GREY_RGB.contains(&hi)
            }
            _ => false,
        }
}

/// 一个格子带不带任何样式（整屏一个都没有 = 终端没颜色，样式这条判据用不上）。
fn styled(cell: &Cell) -> bool {
    let looks = Flags::DIM
        | Flags::INVERSE
        | Flags::BOLD
        | Flags::ITALIC
        | Flags::ALL_UNDERLINES
        | Flags::HIDDEN
        | Flags::STRIKEOUT;
    cell.flags.intersects(looks)
        || cell.fg != Color::Named(NamedColor::Foreground)
        || cell.bg != Color::Named(NamedColor::Background)
}

/// 一行里的可见格子（宽字符的后半格去掉；`\0` / `\t` 当空格）。
fn row_cells(screen: &Screen, line: usize) -> Vec<Cell> {
    let grid = screen.term().grid();
    let row = &grid[Line(line as i32)];
    (0..grid.columns())
        .map(|col| row[Column(col)].clone())
        .filter(|c| !c.flags.contains(Flags::WIDE_CHAR_SPACER))
        .map(|mut c| {
            if c.c == '\0' || c.c == '\t' {
                c.c = ' ';
            }
            c
        })
        .collect()
}

/// 剥掉两端的空白与框线格子（同 [`strip`] 的口径）。
fn trim_cells(cells: &[Cell]) -> &[Cell] {
    let edge = |c: &Cell| c.c.is_whitespace() || BOX_CHARS.contains(&c.c);
    let start = cells.iter().position(|c| !edge(c)).unwrap_or(cells.len());
    let end = cells
        .iter()
        .rposition(|c| !edge(c))
        .map_or(start, |i| i + 1);
    &cells[start..end.max(start)]
}

/// Native-painted cursor witness, scoped to the authoritative input row or body tail.
/// VT cursor coordinates alone are not evidence that an input key was consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputEcho {
    pub prefix: String,
    pub column: usize,
}

impl Screen {
    pub fn input_echo(&self, body: &str) -> Option<InputEcho> {
        let rows = normalize(&self.screen_rows());
        if blocking_dialog(&rows).is_some() {
            return None;
        }
        let input = input_row_index(&rows);
        let start = input.or_else(|| {
            rows.iter()
                .position(|text| body_tail_visible(body, std::slice::from_ref(text)))
        })?;
        let end = if input.is_some() {
            rows.iter()
                .enumerate()
                .skip(start + 1)
                .take(INPUT_SCAN_DEPTH)
                .find(|(_, r)| r.trim_start().starts_with(LOWER_EDGE_CHARS))
                .map_or(start + 1, |(i, _)| i)
        } else {
            start + 1
        };
        let mut found = None;
        let mut prefix = String::new();
        let grid = self.term().grid();
        for line in start..end {
            let row = &grid[Line(line as i32)];
            for column in 0..grid.columns() {
                let cell = &row[Column(column)];
                if cell.flags.contains(Flags::INVERSE) {
                    if !cell.c.is_whitespace() || found.is_some() {
                        return None;
                    }
                    found = Some(InputEcho {
                        prefix: prefix.clone(),
                        column: (line - start) * grid.columns() + column,
                    });
                }
                if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    prefix.push(if cell.c == '\0' { ' ' } else { cell.c });
                }
            }
        }
        found
    }

    /// 输入框里的草稿（[`InputDraft`]）。输入行同 [`input_row`]；输入行下面、到输入框下沿（[`LOWER_EDGE_CHARS`]
    /// 开头的那一行）之前的行算折行 / 多行草稿；往下 [`INPUT_SCAN_DEPTH`] 行内找不到下沿就只看输入行。
    ///
    /// 样式判据：提示符后面的可见格子里，占位样式（暗淡 / 灰色）的与正常样式的各有多少。反显的格子是 claude 自己画的
    /// 光标，不算任何一边。全是占位样式（或什么都没有）= `Empty`；全是正常样式 = `Text`；混着 = `Unknown`。
    pub fn input_draft(&self) -> InputDraft {
        let rows = normalize(&self.screen_rows());
        let Some(at) = input_row_index(&rows) else {
            return InputDraft::Unknown;
        };
        // 下沿 = 第一行以横线 / 下角开头的（被别的字压过一半的横线也算：重绘到一半的帧）。
        let lower_edge = rows
            .iter()
            .enumerate()
            .skip(at + 1)
            .take(INPUT_SCAN_DEPTH)
            .find(|(_, r)| r.trim_start().starts_with(LOWER_EDGE_CHARS))
            .map(|(i, _)| i);
        let lines = at..lower_edge.unwrap_or(at + 1);
        let (mut placeholder, mut plain) = (0usize, 0usize);
        let mut text: Vec<String> = Vec::new();
        for line in lines {
            let cells = row_cells(self, line);
            let mut body = trim_cells(&cells);
            if line == at {
                // 输入行：去掉提示符（剥完框线后的第一个格子）和它后面的空白。
                body = trim_cells(body.get(1..).unwrap_or(&[]));
            }
            for c in body {
                if c.flags.contains(Flags::INVERSE) || c.c.is_whitespace() {
                    continue;
                }
                if placeholder_styled(c) {
                    placeholder += 1;
                } else {
                    plain += 1;
                }
            }
            text.push(body.iter().map(|c| c.c).collect::<String>());
        }
        let cursor_only = placeholder == 0
            && plain == 0
            && text.iter().any(|t| t.chars().any(|c| !c.is_whitespace()));
        match (placeholder, plain) {
            _ if cursor_only => InputDraft::Unknown,
            (_, 0) => InputDraft::Empty,
            (0, _) if self.any_styled_cell() => {
                InputDraft::Text(text.join("\n").trim().to_string())
            }
            _ => InputDraft::Unknown,
        }
    }

    fn any_styled_cell(&self) -> bool {
        (0..self.lines()).any(|line| row_cells(self, line).iter().any(styled))
    }
}

/// 超长正文会把提示符滚出屏幕：去掉空白后比对 `prompt` 末尾 64 个字符是否出现在屏上。
pub fn body_tail_visible(prompt: &str, rows: &[String]) -> bool {
    let compact: Vec<char> = prompt.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return false;
    }
    let needle: String = compact[compact.len().saturating_sub(BODY_TAIL_CHARS)..]
        .iter()
        .collect();
    let screen: String = normalize(rows)
        .concat()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    screen.contains(&needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn finds_the_input_row_inside_the_box() {
        let screen = rows(&[
            "╭────────────────────╮",
            "│ ❯ Try \"how do I log an error?\" │",
            "╰────────────────────╯",
            "  auto mode on · ? for shortcuts",
        ]);
        assert_eq!(
            input_row(&screen).as_deref(),
            Some("Try \"how do I log an error?\"")
        );
    }

    #[test]
    fn empty_box_gives_empty_row_not_none() {
        let screen = rows(&["╭────╮", "│ >  │", "╰────╯"]);
        assert_eq!(input_row(&screen).as_deref(), Some(""));
    }

    #[test]
    fn numbered_option_under_the_marker_is_not_an_input_row() {
        let screen = rows(&["Pick", "❯ 1. Yes"]);
        assert_eq!(input_row(&screen), None);
    }

    #[test]
    fn dialog_on_screen_hides_the_input_row() {
        let screen = rows(&[
            "Quick safety check: do you trust this folder?",
            "",
            "❯  No, exit",
            "   Yes, I trust this folder",
            "",
            "Enter to confirm · Esc to cancel",
        ]);
        assert!(blocking_dialog(&screen).is_some());
        assert_eq!(input_row(&screen), None);
    }

    #[test]
    fn marker_beyond_scan_depth_is_not_found() {
        let mut screen = rows(&["❯ hello"]);
        screen.extend((0..INPUT_SCAN_DEPTH).map(|i| format!("line {i}")));
        assert_eq!(input_row(&screen), None);
        screen.remove(1);
        assert_eq!(input_row(&screen).as_deref(), Some("hello"));
    }

    #[test]
    fn nul_cells_count_as_spaces() {
        let screen = vec!["\0\0❯\0hi".to_string()];
        assert_eq!(input_row(&screen).as_deref(), Some("hi"));
    }

    #[test]
    fn body_tail_ignores_wrapping_and_whitespace() {
        let prompt = format!("{}末尾这句", "a".repeat(100));
        let screen = rows(&[&"a".repeat(80), &format!("{}末尾 这句", "a".repeat(20))]);
        assert!(body_tail_visible(&prompt, &screen));
        assert!(!body_tail_visible(&prompt, &rows(&["something else"])));
        assert!(!body_tail_visible("   ", &screen));
    }

    // ---- input_draft（T10d 草稿闸）----

    use crate::ScreenConfig;

    fn draft(bytes: &str) -> InputDraft {
        let mut s = Screen::new(ScreenConfig::default());
        s.feed(bytes.as_bytes());
        s.input_draft()
    }

    /// 输入框下面一行暗淡的提示条：让整屏「有样式」（真 claude 的屏总有），样式判据才用得上。
    const STATUS: &str = "\r\n\x1b[2m  ? for shortcuts\x1b[22m";
    const RULE: &str = "────────────────────";

    #[test]
    fn placeholder_in_dim_or_grey_is_an_empty_box() {
        let dim = format!("{RULE}\r\n❯ \x1b[2mTry \"x\"\x1b[22m\r\n{RULE}{STATUS}");
        assert_eq!(draft(&dim), InputDraft::Empty);
        let grey = format!("{RULE}\r\n❯ \x1b[90mTry \"x\"\x1b[39m\r\n{RULE}{STATUS}");
        assert_eq!(draft(&grey), InputDraft::Empty);
        // claude 自己画的光标（反显）压在占位提示第一个字上：不算正常样式。
        let cursor =
            format!("{RULE}\r\n❯ \x1b[7mT\x1b[27m\x1b[2mry \"x\"\x1b[22m\r\n{RULE}{STATUS}");
        assert_eq!(draft(&cursor), InputDraft::Empty);
        let blank = format!("{RULE}\r\n❯ \x1b[7m \x1b[27m\r\n{RULE}{STATUS}");
        assert_eq!(draft(&blank), InputDraft::Empty);
    }

    /// T10d fix R2：256 色中灰阶与 truecolor 中灰也是占位提示；太亮的灰（正文的白）、有色的不是。
    #[test]
    fn mid_grey_256_and_truecolor_placeholders_are_an_empty_box() {
        for sgr in [
            "38;5;246",
            "38;5;232",
            "38;5;250",
            "38;2;153;153;153",
            "38;2;100;110;105",
        ] {
            let s = format!("{RULE}\r\n❯ \x1b[{sgr}mTry \"x\"\x1b[39m\r\n{RULE}{STATUS}");
            assert_eq!(draft(&s), InputDraft::Empty, "{sgr}");
        }
        for sgr in [
            "38;5;251",
            "38;5;33",
            "38;2;220;220;220",
            "38;2;153;120;153",
            "38;2;60;60;60",
        ] {
            let s = format!("{RULE}\r\n❯ \x1b[{sgr}mTry \"x\"\x1b[39m\r\n{RULE}{STATUS}");
            assert_eq!(draft(&s), InputDraft::Text("Try \"x\"".into()), "{sgr}");
        }
    }

    #[test]
    fn typed_text_is_a_draft() {
        let typed = format!("{RULE}\r\n❯ 先别动\x1b[7m \x1b[27m\r\n{RULE}{STATUS}");
        assert_eq!(draft(&typed), InputDraft::Text("先别动".into()));
        // 光标停在字中间（反显的那个字照样算进文本）。
        let mid = format!("{RULE}\r\n❯ ab\x1b[7mc\x1b[27md\r\n{RULE}{STATUS}");
        assert_eq!(draft(&mid), InputDraft::Text("abcd".into()));
    }

    #[test]
    fn multi_line_drafts_are_read_down_to_the_lower_edge() {
        let rules = format!("{RULE}\r\n❯ 第一行\r\n  第二行\r\n{RULE}{STATUS}");
        assert_eq!(draft(&rules), InputDraft::Text("第一行\n第二行".into()));
        // 框线式：第一行空、第二行有字（先敲了一个换行）也是草稿。
        let boxed = format!("╭──────────╮\r\n│ ❯        │\r\n│   later  │\r\n╰──────────╯{STATUS}");
        assert_eq!(draft(&boxed), InputDraft::Text("later".into()));
        // 找不到下沿：只看输入行，下面的正文不算。
        let open = format!("❯ \x1b[2mTry\x1b[22m\r\nsomething below{STATUS}");
        assert_eq!(draft(&open), InputDraft::Empty);
    }

    #[test]
    fn mixed_unstyled_or_missing_is_unknown() {
        let mixed = format!("{RULE}\r\n❯ hel\x1b[2mlo\x1b[22m\r\n{RULE}{STATUS}");
        assert_eq!(draft(&mixed), InputDraft::Unknown);
        // 整屏没有任何样式（没颜色的终端）：占位提示和打的字长得一样。
        assert_eq!(
            draft(&format!("{RULE}\r\n❯ hello\r\n{RULE}")),
            InputDraft::Unknown
        );
        // 没有输入框 / 对话框在场。
        assert_eq!(draft("\x1b[2mloading\x1b[22m"), InputDraft::Unknown);
        let dialog = "Do you want to proceed?\r\n❯ 1. Yes\r\n  2. No\r\n\x1b[2mEsc\x1b[22m";
        assert_eq!(draft(dialog), InputDraft::Unknown);
        // 只有反显、没有别的可见字：不知道是什么。
        let inverse = format!("{RULE}\r\n❯ \x1b[7mx\x1b[27m\r\n{RULE}{STATUS}");
        assert_eq!(draft(&inverse), InputDraft::Unknown);
    }
}
