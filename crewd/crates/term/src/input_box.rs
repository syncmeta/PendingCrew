//! claude 输入框与阻塞对话框的识别（Swift `ClaudeInputBox` 的移植）。
//! 只作用在**渲染完的整屏文本**上（[`crate::Screen::screen_rows`]），不吃原始字节：
//! claude 用光标定位摆词，字节流里 `1.Yes,Itrustthisfolder` 序号后面没有空格。
use crate::menu::{PendingDecision, parse_menu, parse_option, strip};

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
    if blocking_dialog(rows).is_some() {
        return None;
    }
    let rows = normalize(rows);
    let start = rows.len().saturating_sub(INPUT_SCAN_DEPTH);
    for line in rows[start..].iter().rev() {
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
        return Some(stripped[first.len_utf8()..].trim().to_string());
    }
    None
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
}
