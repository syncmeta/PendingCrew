//! Human-only decision notices and the terminal answer plan. IO callers safe_name all external slots.
use crate::text::{Lang, get, render};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionFacts {
    pub prompt: String,
    pub options: Vec<String>,
    pub numbered: bool,
}

pub const NUDGE_KEYS: &[(&str, &[u8])] = &[
    ("enter", b"\r"),
    ("esc", b"\x1b"),
    ("回车", b"\r"), // 键名，不是文案
    ("up", b"\x1b[A"),
    ("down", b"\x1b[B"),
];

/// 编号框上直接按的数字键名（第 1..=9 项）。两位数的按法没在真 claude 上核过（T5 NOTE），不用。
pub const DIGIT_KEYS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// 键名 → 发给 PTY 的字节：[`NUDGE_KEYS`] 里的别名，或 [`DIGIT_KEYS`]（字节就是那个数字）。
pub fn key_bytes(name: &str) -> Option<&'static [u8]> {
    NUDGE_KEYS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, b)| *b)
        .or_else(|| {
            DIGIT_KEYS
                .iter()
                .find(|d| **d == name)
                .map(|d| d.as_bytes())
        })
}

/// [`answer_plan`] 不给按法的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerRefusal {
    /// 选的那一项不在框里（`choice` 从 0 起，≥ 选项数）。
    OutOfRange,
    /// 要靠方向键挪、但屏上看不出高亮在哪一项：交给人在终端里选。
    NeedsTerminal,
}

/// 回答一个待决策框、选第 `choice` 项（0 起）要按哪几个键（纯函数；键名用 [`key_bytes`] 换字节）。每一项带「按它之前
/// 高亮应停在第几项」（执行器按之前核它，T10d 安全按键；`None` = 不核）。
/// - `choice` 超出选项 → `OutOfRange`；
/// - 有编号且 `choice < 9` → 只按那个数字（终端呈现的编号，不是 RPC 的下标）；
/// - 否则高亮位置 `selected` 已知 → 从 `selected` 一格格 `up` / `down` 挪到 `choice`，再 `enter`；
/// - 高亮位置不知道 → `NeedsTerminal`。
pub fn answer_plan(
    d: &DecisionFacts,
    selected: Option<usize>,
    choice: usize,
) -> Result<Vec<(&'static str, Option<usize>)>, AnswerRefusal> {
    if choice >= d.options.len() {
        return Err(AnswerRefusal::OutOfRange);
    }
    if d.numbered && choice < DIGIT_KEYS.len() {
        return Ok(vec![(DIGIT_KEYS[choice], None)]);
    }
    let Some(mut at) = selected.filter(|s| *s < d.options.len()) else {
        return Err(AnswerRefusal::NeedsTerminal);
    };
    let mut steps = Vec::new();
    while at != choice {
        if at < choice {
            steps.push(("down", Some(at)));
            at += 1;
        } else {
            steps.push(("up", Some(at)));
            at -= 1;
        }
    }
    steps.push(("enter", Some(choice)));
    Ok(steps)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NudgePlan {
    Interrupt,
    HumanOnly,
    Text(String),
}

const HUMAN_KEY_ALIASES: &[&str] = &[
    "left", "right", "return", "上", // 键名，不是文案
    "下", // 键名，不是文案
    "左", // 键名，不是文案
    "右", // 键名，不是文案
];

/// Agents cannot answer menus. Only interrupt tokens and inert free text survive.
pub fn nudge_plan(input: &str) -> Option<NudgePlan> {
    let key = input.trim();
    if key.is_empty() {
        return None;
    }
    let lower = key.to_ascii_lowercase();
    if lower == "esc" || lower == "interrupt" {
        return Some(NudgePlan::Interrupt);
    }
    if key.bytes().all(|b| b.is_ascii_digit())
        || NUDGE_KEYS.iter().any(|(k, _)| *k == lower)
        || HUMAN_KEY_ALIASES.contains(&lower.as_str())
    {
        return Some(NudgePlan::HumanOnly);
    }
    Some(NudgePlan::Text(
        key.chars()
            .map(|c| match c {
                '\r' | '\n' => ' ',
                '\u{7f}' => '\u{2421}',
                c if (c as u32) < 0x20 => char::from_u32(0x2400 + c as u32).unwrap(),
                c if ('\u{80}'..='\u{9f}').contains(&c) => '\u{fffd}',
                c => c,
            })
            .collect(),
    ))
}

pub fn decision_notice(lang: Lang, name: &str, session: &str, d: &DecisionFacts) -> String {
    let mut lines = vec![render(lang, "decision.first", &[("name", name)])];
    if !d.prompt.is_empty() {
        lines.push(format!("> {}", d.prompt));
    }
    lines.extend(d.options.iter().enumerate().map(|(i, o)| {
        if d.numbered {
            format!("  {}. {o}", i + 1)
        } else {
            format!("  {o}")
        }
    }));
    lines.push(get(lang, "decision.howto").into());
    lines.push(render(lang, "decision.session", &[("session", session)]));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notices_are_one_stage_and_never_invent_terminal_numbers() {
        for lang in [Lang::Zh, Lang::En] {
            for numbered in [false, true] {
                let d = DecisionFacts {
                    prompt: "Trust?".into(),
                    options: vec!["Yes".into(), "No".into()],
                    numbered,
                };
                let text = decision_notice(lang, "worker", "s", &d);
                assert!(text.contains(get(lang, "decision.howto")));
                assert_eq!(text.contains("  1. Yes"), numbered);
                assert!(!text.contains("nudge_session"));
            }
        }
    }
    #[test]
    fn agents_get_no_answer_keys_or_implicit_enter() {
        assert_eq!(nudge_plan(" "), None);
        for key in ["1", "12", "2026", "enter", "up", "down", "right", "回车"] {
            assert_eq!(nudge_plan(key), Some(NudgePlan::HumanOnly));
        }
        for key in ["esc", "INTERRUPT"] {
            assert_eq!(nudge_plan(key), Some(NudgePlan::Interrupt));
        }
        assert_eq!(
            nudge_plan("a\x1bb\nc"),
            Some(NudgePlan::Text("a\u{241b}b c".into()))
        );
    }
    #[test]
    fn decision_sources_carry_no_cjk_prose_literals() {
        let files = [
            ("plan.rs", include_str!("plan.rs")),
            ("breaker.rs", include_str!("breaker.rs")),
            ("queue.rs", include_str!("queue.rs")),
            ("policy.rs", include_str!("policy.rs")),
            ("notice.rs", include_str!("notice.rs")),
            ("model.rs", include_str!("model.rs")),
            ("lib.rs", include_str!("lib.rs")),
        ];
        for (name, src) in files {
            let code = src.split("#[cfg(test)]").next().unwrap();
            for (i, line) in code.lines().enumerate() {
                let t = line.trim_start();
                if t.starts_with("//")
                    || line.contains("键名，不是文案")
                    || line.contains(".expect(")
                    || line.contains("panic!(")
                {
                    continue;
                }
                let code_part = t.split(" //").next().unwrap_or(t);
                assert!(!code_part.chars().any(|c| matches!(c, '\u{3000}'..='\u{303f}' | '\u{4e00}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}')), "{name}:{} prose literal: {line}", i+1);
            }
        }
    }

    fn facts(n: usize, numbered: bool) -> DecisionFacts {
        DecisionFacts {
            prompt: "Pick".into(),
            options: (1..=n).map(|i| format!("opt {i}")).collect(),
            numbered,
        }
    }

    /// T10d `answer_plan` 的表。
    #[test]
    fn answer_plan_table() {
        type Plan = Result<Vec<(&'static str, Option<usize>)>, AnswerRefusal>;
        let cases: Vec<(DecisionFacts, Option<usize>, usize, Plan)> = vec![
            // 编号框、前 9 项：只按数字（不核高亮，高亮在哪都一样）。
            (facts(3, true), Some(0), 1, Ok(vec![("2", None)])),
            (facts(3, true), None, 0, Ok(vec![("1", None)])),
            (facts(9, true), Some(4), 8, Ok(vec![("9", None)])),
            // 超出选项。
            (facts(3, true), Some(0), 3, Err(AnswerRefusal::OutOfRange)),
            (facts(2, false), Some(0), 5, Err(AnswerRefusal::OutOfRange)),
            // 没编号：从高亮挪过去再回车，每一下带「按之前高亮在哪」。
            (facts(3, false), Some(0), 0, Ok(vec![("enter", Some(0))])),
            (
                facts(3, false),
                Some(0),
                2,
                Ok(vec![
                    ("down", Some(0)),
                    ("down", Some(1)),
                    ("enter", Some(2)),
                ]),
            ),
            (
                facts(3, false),
                Some(2),
                0,
                Ok(vec![("up", Some(2)), ("up", Some(1)), ("enter", Some(0))]),
            ),
            // 高亮不知道（或报的位置不在框里）→ 交给人。
            (facts(3, false), None, 1, Err(AnswerRefusal::NeedsTerminal)),
            (
                facts(3, false),
                Some(7),
                1,
                Err(AnswerRefusal::NeedsTerminal),
            ),
            // 12 个编号选项选第 11 项（下标 10）：两位数不用，靠方向键。
            (
                facts(12, true),
                Some(8),
                10,
                Ok(vec![
                    ("down", Some(8)),
                    ("down", Some(9)),
                    ("enter", Some(10)),
                ]),
            ),
            (facts(12, true), None, 10, Err(AnswerRefusal::NeedsTerminal)),
        ];
        for (d, selected, choice, want) in cases {
            assert_eq!(
                answer_plan(&d, selected, choice),
                want,
                "{d:?} selected={selected:?} choice={choice}"
            );
        }
    }

    #[test]
    fn every_key_an_answer_plan_can_name_has_bytes() {
        for name in ["up", "down", "enter"].into_iter().chain(DIGIT_KEYS) {
            assert!(key_bytes(name).is_some(), "{name}");
        }
        assert_eq!(key_bytes("7"), Some(&b"7"[..]));
        assert_eq!(key_bytes("down"), Some(&b"\x1b[B"[..]));
        assert_eq!(key_bytes("10"), None, "两位数不是一个键");
    }
}
