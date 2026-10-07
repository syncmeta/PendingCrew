//! 从终端明文里认出「交互式选择菜单」（Swift `TerminalMenuParser` 的移植）。
//!
//! 判据缺一不可，全为了**不误报**（误报比漏报更伤：群里一刷噪音，人就不再看通知）：
//! - 编号式：末尾附近一段连续 `N. 文本` 行，序号从 1 连续，≥2 项，至少一行带光标。
//! - 光标列表式（2026-09 起的 claude 信任框）：底部有 `Enter to confirm · Esc to cancel`
//!   脚注，其上 ≥2 行连续非空、无编号的行，恰好一行带光标。
//! - 选项块后最多跟 [`TRAILING_SLACK`] 行正文，再多说明菜单已被答掉。
//! - 限流菜单交给自动应答，这里排除。

/// 选择光标字形。**不含裸 `>`**——markdown 引用、shell 提示符都用它。
pub const CURSOR_MARKERS: [char; 4] = ['❯', '›', '▸', '▶'];

/// TUI 画框字符：逐行剥掉两端的框线。
pub const BOX_CHARS: [char; 20] = [
    '│', '┃', '║', '╭', '╮', '╰', '╯', '─', '━', '═', '┌', '┐', '└', '┘', '├', '┤', '╔', '╗', '╚',
    '╝',
];

/// 选项块后还能跟几行正文（给框下沿的提示行留位）。宁小勿大：决定答完多久状态能回正。
pub const TRAILING_SLACK: usize = 2;

/// 问句往上找多少行（信任框的真问句离选项块有 6～7 行）。
pub const PROMPT_SCAN_DEPTH: usize = 12;

/// 限流菜单的特征短语（小写）。命中的菜单不报给人。
pub const RATE_LIMIT_MENU_PHRASES: [&str; 2] =
    ["/rate-limit-options", "stop and wait for limit to reset"];

/// 一个「在等人选」的现场。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDecision {
    /// 问句原文；认不出时为空串。
    pub prompt: String,
    /// 选项正文（已剥掉序号与光标），按屏幕顺序。
    pub options: Vec<String>,
    /// 屏幕上本来有没有编号。事实字段，每个生产者如实填，没有缺省。
    pub numbered: bool,
    /// 光标（[`CURSOR_MARKERS`]）此刻停在第几项（0 起，同 `options` 的下标）；屏上看不到光标 = `None`。
    /// 不进 [`PendingDecision::fingerprint`]：挪高亮不是换了一个框，但按键前要核它（T10d 的安全按键）。
    pub selected: Option<usize>,
}

impl PendingDecision {
    /// 去重指纹：同一菜单反复重绘 → 同一指纹。故意不含 `numbered` 与 `selected`。
    pub fn fingerprint(&self) -> String {
        std::iter::once(self.prompt.as_str())
            .chain(self.options.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("|")
    }
}

/// 剥掉一行两端的框线与空白。
pub fn strip(line: &str) -> &str {
    line.trim_matches(|c: char| c.is_whitespace() || BOX_CHARS.contains(&c))
}

/// 一个编号选项行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuOption {
    pub(crate) index: usize,
    pub(crate) text: String,
    pub(crate) selected: bool,
}

/// 剥干净的一行是不是 `[光标] N. 文本`。
pub(crate) fn parse_option(stripped: &str) -> Option<MenuOption> {
    let mut s = stripped;
    let mut selected = false;
    if let Some(first) = s.chars().next()
        && CURSOR_MARKERS.contains(&first)
    {
        selected = true;
        s = s[first.len_utf8()..].trim_start();
    }
    let digits = s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let index: usize = s[..digits].parse().ok()?;
    let rest = s[digits..].strip_prefix('.')?;
    if !rest.chars().next()?.is_whitespace() {
        return None;
    }
    let text = rest.trim_start();
    if text.is_empty() {
        return None;
    }
    Some(MenuOption {
        index,
        text: text.to_string(),
        selected,
    })
}

/// `Enter to confirm · Esc to cancel`。比对前去掉全部空白——字节尾窗里同一句是
/// `Entertoconfirm·Esctocancel`（claude 用光标定位摆词）。
pub fn is_confirm_footer(stripped: &str) -> bool {
    let compact: String = stripped
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    compact.contains("toconfirm") && compact.contains("tocancel")
}

/// 从一段明文里认菜单。`plain` 是 `\n` 分隔的若干行（屏幕行或字节尾窗）。
pub fn parse_menu(plain: &str) -> Option<PendingDecision> {
    let lines: Vec<&str> = plain.split('\n').map(strip).collect();
    let decision = parse_numbered(&lines).or_else(|| parse_cursor_list(&lines))?;
    let hay = std::iter::once(decision.prompt.as_str())
        .chain(decision.options.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if RATE_LIMIT_MENU_PHRASES.iter().any(|p| hay.contains(p)) {
        return None;
    }
    Some(decision)
}

/// 选项块上方的问句：优先带问号的那行，否则最近一行正文。
fn prompt_above(lines: &[&str], block_start: usize) -> String {
    let mut nearest = "";
    for line in lines[..block_start].iter().rev().take(PROMPT_SCAN_DEPTH) {
        if line.is_empty() {
            continue;
        }
        if nearest.is_empty() {
            nearest = line;
        }
        if line.contains('?') || line.contains('？') {
            return line.to_string();
        }
    }
    nearest.to_string()
}

/// 从底往上找第一行满足 `hit` 的非空行；途中正文超过 TRAILING_SLACK 行即放弃。
fn find_from_bottom(lines: &[&str], hit: impl Fn(&str) -> bool) -> Option<usize> {
    let mut trailing = 0;
    for (i, line) in lines.iter().enumerate().rev() {
        if line.is_empty() {
            continue;
        }
        if hit(line) {
            return Some(i);
        }
        trailing += 1;
        if trailing > TRAILING_SLACK {
            return None;
        }
    }
    None
}

fn parse_numbered(lines: &[&str]) -> Option<PendingDecision> {
    let end = find_from_bottom(lines, |l| parse_option(l).is_some())?;
    // 往上收连续选项行（不跨空行）。
    let mut start = end + 1;
    let mut parsed = Vec::new();
    while start > 0 {
        match parse_option(lines[start - 1]) {
            Some(option) => {
                parsed.push(option);
                start -= 1;
            }
            None => break,
        }
    }
    parsed.reverse();
    let consecutive = parsed.iter().enumerate().all(|(k, o)| o.index == k + 1);
    if parsed.len() < 2 || !consecutive || !parsed.iter().any(|o| o.selected) {
        return None;
    }
    // 好几行都带光标（不该发生）时取第一行：判据只要「至少一行」，这里如实报第一处。
    let selected = parsed.iter().position(|o| o.selected);
    Some(PendingDecision {
        prompt: prompt_above(lines, start),
        options: parsed.into_iter().map(|o| o.text).collect(),
        numbered: true,
        selected,
    })
}

fn parse_cursor_list(lines: &[&str]) -> Option<PendingDecision> {
    let footer = find_from_bottom(lines, is_confirm_footer)?;
    let end = lines[..footer].iter().rposition(|l| !l.is_empty())?;
    let start = lines[..=end]
        .iter()
        .rposition(|l| l.is_empty())
        .map_or(0, |i| i + 1);
    let block = &lines[start..=end];
    // 带编号的归上面那条路，免得同一屏被两条路解析出两份措辞。
    if block.len() < 2 || block.iter().any(|l| parse_option(l).is_some()) {
        return None;
    }
    let mut options = Vec::with_capacity(block.len());
    let mut cursors = 0;
    let mut selected = None;
    for (i, line) in block.iter().enumerate() {
        let first = line.chars().next()?;
        if CURSOR_MARKERS.contains(&first) {
            cursors += 1;
            selected = Some(i);
            let text = line[first.len_utf8()..].trim();
            if text.is_empty() {
                return None;
            }
            options.push(text.to_string());
        } else {
            options.push(line.to_string());
        }
    }
    if cursors != 1 {
        return None;
    }
    Some(PendingDecision {
        prompt: prompt_above(lines, start),
        options,
        numbered: false,
        selected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permission_menu() -> String {
        [
            "╭──────────────────────────────────────────────╮",
            "│ Bash command                                 │",
            "│                                              │",
            "│   rm -rf build                               │",
            "│   Remove the build directory                 │",
            "│                                              │",
            "│ Do you want to proceed?                      │",
            "│ ❯ 1. Yes                                     │",
            "│   2. Yes, and don't ask again for rm commands │",
            "│   3. No, and tell Claude what to do (esc)    │",
            "╰──────────────────────────────────────────────╯",
        ]
        .join("\n")
    }

    fn unnumbered_trust_dialog() -> String {
        [
            "Quick safety check: Is this a project you created or one you trust? (Like your",
            "own code, a well-known open source project, or work from your team). If not,",
            "take a moment to review what's in this folder first.",
            "",
            "Claude Code'll be able to read, edit, and execute files here.",
            "",
            "Security guide",
            "",
            "❯  No, exit",
            "   Yes, I trust this folder",
            "",
            "Enter to confirm · Esc to cancel",
        ]
        .join("\n")
    }

    #[test]
    fn parses_permission_menu() {
        let d = parse_menu(&permission_menu()).unwrap();
        assert_eq!(d.prompt, "Do you want to proceed?");
        assert_eq!(
            d.options,
            [
                "Yes",
                "Yes, and don't ask again for rm commands",
                "No, and tell Claude what to do (esc)"
            ]
        );
        assert!(d.numbered);
    }

    #[test]
    fn parses_plan_menu() {
        let menu = [
            "Would you like to proceed?",
            "❯ 1. Yes, and auto-accept edits",
            "  2. Yes, and manually approve edits",
            "  3. No, keep planning",
        ]
        .join("\n");
        let d = parse_menu(&menu).unwrap();
        assert_eq!(d.prompt, "Would you like to proceed?");
        assert_eq!(d.options.len(), 3);
    }

    #[test]
    fn parses_trust_folder_menu() {
        let menu = "Do you trust the files in this folder?\n❯ 1. Yes, proceed\n  2. No, exit";
        let d = parse_menu(menu).unwrap();
        assert_eq!(d.prompt, "Do you trust the files in this folder?");
        assert_eq!(d.options, ["Yes, proceed", "No, exit"]);
    }

    #[test]
    fn marker_on_second_option_still_parses() {
        let menu = "Do you want to proceed?\n  1. Yes\n❯ 2. No";
        assert_eq!(parse_menu(menu).unwrap().options, ["Yes", "No"]);
    }

    #[test]
    fn parses_unnumbered_trust_dialog() {
        let d = parse_menu(&unnumbered_trust_dialog()).unwrap();
        assert_eq!(d.options, ["No, exit", "Yes, I trust this folder"]);
        assert!(!d.numbered);
        assert!(
            d.prompt.contains("trust?") || d.prompt.contains("safety check"),
            "{}",
            d.prompt
        );
    }

    #[test]
    fn ready_input_box_is_not_a_dialog() {
        let ready = [
            "╭──────────────────────────────────────────────╮",
            "│ ❯ Try \"how do I log an error?\"               │",
            "╰──────────────────────────────────────────────╯",
            "  auto mode on · ? for shortcuts",
        ]
        .join("\n");
        assert_eq!(parse_menu(&ready), None);
    }

    #[test]
    fn multi_line_input_box_is_not_a_dialog() {
        let typing = [
            "╭──────────────────────────────────────────────╮",
            "│ ❯ 先看一下现状                                │",
            "│   再决定怎么改                                │",
            "╰──────────────────────────────────────────────╯",
            "  auto mode on · ? for shortcuts",
        ]
        .join("\n");
        assert_eq!(parse_menu(&typing), None);
    }

    #[test]
    fn prose_numbered_list_is_not_a_menu() {
        assert_eq!(
            parse_menu("我打算这么做：\n1. 先查现状\n2. 再写单测\n3. 最后接线"),
            None
        );
    }

    #[test]
    fn answered_menu_followed_by_output_is_not_pending() {
        let after = permission_menu()
            + "\n⏺ Bash(rm -rf build)\n  ⎿  (No content)\n⏺ 删好了，继续下一步。\n正在读取 src/main.swift …";
        assert_eq!(parse_menu(&after), None);
    }

    #[test]
    fn rate_limit_menu_is_excluded() {
        let menu = "What do you want to do?\n❯ 1. Stop and wait for limit to reset\n  2. Upgrade your plan";
        assert_eq!(parse_menu(menu), None);
    }

    #[test]
    fn single_option_is_not_a_menu() {
        assert_eq!(parse_menu("Continue?\n❯ 1. Yes\n"), None);
    }

    #[test]
    fn non_consecutive_numbers_are_rejected() {
        assert_eq!(parse_menu("见下：\n❯ 1. 甲\n  3. 丙\n"), None);
    }

    #[test]
    fn cursor_list_needs_exactly_one_cursor() {
        let two = "Pick one\n\n❯ a\n❯ b\n\nEnter to confirm · Esc to cancel";
        assert_eq!(parse_menu(two), None);
        let one = "Pick one\n\n❯ a\n  b\n\nEnter to confirm · Esc to cancel";
        let d = parse_menu(one).unwrap();
        assert_eq!(d.options, ["a", "b"]);
        assert_eq!(d.prompt, "Pick one");
    }

    #[test]
    fn footer_matches_without_spaces() {
        assert!(is_confirm_footer("Entertoconfirm·Esctocancel"));
        assert!(is_confirm_footer("Enter to confirm · Esc to cancel"));
        assert!(!is_confirm_footer("Enter to confirm"));
    }

    #[test]
    fn option_lines_need_a_space_after_the_dot() {
        assert_eq!(parse_option("1.Yes"), None);
        assert_eq!(parse_option("1. "), None);
        assert_eq!(
            parse_option("❯  2. No"),
            Some(MenuOption {
                index: 2,
                text: "No".into(),
                selected: true
            })
        );
        assert_eq!(parse_option("99999999999999999999999. x"), None);
    }

    #[test]
    fn fingerprint_ignores_numbered_and_selected() {
        let a = PendingDecision {
            prompt: "q".into(),
            options: vec!["a".into()],
            numbered: true,
            selected: Some(0),
        };
        let b = PendingDecision {
            numbered: false,
            selected: None,
            ..a.clone()
        };
        assert_eq!(a.fingerprint(), "q|a");
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    /// 菜单后面跟几行正文的边界：恰好 TRAILING_SLACK 行还认，多一行就当已答掉。
    #[test]
    fn trailing_slack_boundary_numbered() {
        let menu = "Do you want to proceed?\n❯ 1. Yes\n  2. No";
        let two = format!("{menu}\n⏺ 正文一\n⏺ 正文二");
        assert_eq!(TRAILING_SLACK, 2, "下面两个用例按 2 行写");
        assert!(parse_menu(&two).is_some(), "后跟 2 行正文仍是待决策");
        let three = format!("{two}\n⏺ 正文三");
        assert_eq!(parse_menu(&three), None, "后跟 3 行正文说明已答掉");
    }

    #[test]
    fn trailing_slack_boundary_cursor_list() {
        let list = "Pick one\n\n❯ a\n  b\n\nEnter to confirm · Esc to cancel";
        let two = format!("{list}\n后跟一\n后跟二");
        assert!(parse_menu(&two).is_some(), "脚注后 2 行仍是待决策");
        let three = format!("{two}\n后跟三");
        assert_eq!(parse_menu(&three), None, "脚注后 3 行说明已答掉");
    }

    /// 钉住现状（与 Swift 一致，不改行为）：真实 PTY 输出行尾是 CRLF，尾窗把 CR 和 LF
    /// 各变成一个 `\n`，行与行之间多出空行，而选项块不跨空行——字节尾窗认不出多行菜单。
    /// 屏幕源（渲染后的行）才是生产里能用的那条。要不要把 CRLF 折成一个换行是 M2b 的决定。
    #[test]
    fn crlf_byte_tail_never_recognises_a_multi_line_menu() {
        let lines = ["Do you want to proceed?", "❯ 1. Yes", "  2. No"];
        let mut lf = crate::tail::AnsiPlainTextTail::new();
        lf.feed(lines.join("\n").as_bytes());
        assert!(parse_menu(lf.tail()).is_some(), "对照：纯 LF 能认出");

        let mut crlf = crate::tail::AnsiPlainTextTail::new();
        crlf.feed(lines.join("\r\n").as_bytes());
        crlf.feed(b"\r\n");
        assert_eq!(
            crlf.tail(),
            "Do you want to proceed?\n\n❯ 1. Yes\n\n  2. No\n\n"
        );
        assert_eq!(parse_menu(crlf.tail()), None, "CRLF：CR 与 LF 各成一个换行");
    }

    /// T10d：光标停在哪一项（0 起）。编号式：第一 / 中间 / 最后。
    #[test]
    fn numbered_menu_reports_the_highlighted_option() {
        let menu = |at: usize| {
            let opts = ["Yes", "Yes, and don't ask again", "No"];
            let mut lines = vec!["Do you want to proceed?".to_string()];
            for (i, o) in opts.iter().enumerate() {
                let mark = if i == at { "❯" } else { " " };
                lines.push(format!("{mark} {}. {o}", i + 1));
            }
            lines.join("\n")
        };
        for at in [0, 1, 2] {
            let d = parse_menu(&menu(at)).unwrap();
            assert_eq!(d.selected, Some(at), "{d:?}");
            assert_eq!(d.fingerprint(), parse_menu(&menu(0)).unwrap().fingerprint());
        }
        assert_eq!(parse_menu(&permission_menu()).unwrap().selected, Some(0));
    }

    /// 光标列表式（没编号）：第一 / 中间 / 最后。
    #[test]
    fn cursor_list_reports_the_highlighted_option() {
        let list = |at: usize| {
            let opts = ["a", "b", "c"];
            let block: Vec<String> = opts
                .iter()
                .enumerate()
                .map(|(i, o)| format!("{} {o}", if i == at { "❯" } else { " " }))
                .collect();
            format!(
                "Pick one\n\n{}\n\nEnter to confirm · Esc to cancel",
                block.join("\n")
            )
        };
        for at in [0, 1, 2] {
            let d = parse_menu(&list(at)).unwrap();
            assert!(!d.numbered);
            assert_eq!(d.selected, Some(at), "{d:?}");
        }
        assert_eq!(
            parse_menu(&unnumbered_trust_dialog()).unwrap().selected,
            Some(0)
        );
    }
}
