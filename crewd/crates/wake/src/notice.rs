//! 待决策通知与 nudge 键表（移植 `Sources/Mac/LocalRunner/SessionPendingDecision.swift:339-470`）。
//! - 按法由这一屏现算：有编号说发数字；没编号说「别发数字」，可用的键从 [`NUDGE_KEYS`] 现算（一张表、一份名单）。
//! - 选靶：worker 卡住 → @机长；机长卡住 → 直接 @人（绝不 @ 自己：@机长会触发拉起，成环，`:375`）；升级 → @人。
//! - 键表与执行器对齐：`SessionHandle::raw` 把字节原样写进 PTY（claude），所以表里的字节就是终端收到的字节；
//!   文本路径的去毒规则与 `crewd_exec::session::sanitize_text` 同一套（控制字符变 U+2400 控制图形、C1 变 U+FFFD），
//!   只是这里还把换行压成空格（一条 nudge 只会提交一次）。
//!
//! 外来文字（C20）：本 crate 不依赖 collab，所以不在这里过 `render::safe_name`。**调用方（IO 层）必须在构造入参时
//! 把 `name` / `session` 以及 [`DecisionFacts`] 的 `prompt` / `options`（屏幕上抓来的外来文本）先过 `safe_name`**；
//! 这里只负责按键表与文案模板拼装，不对外来文字做任何假设。
use crate::model::MIN;
use crate::text::{Lang, get, render};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionFacts {
    /// 屏幕上抓来的提示与选项（外来文本，调用方负责去毒，见模块文档）。
    pub prompt: String,
    pub options: Vec<String>,
    /// 屏幕上本来有没有编号（事实字段，由 crewd-term 如实填）。
    pub numbered: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeStage {
    First,
    Escalate,
}

/// 首报后多久没解决就升级找人（`SessionPendingDecision.swift:389`）。
pub const DECISION_ESCALATE_MS: i64 = 5 * MIN;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// true = 发到根机组人类群并 @人；false = 发到本机组大厅并 @群主（机长）。
    pub to_human: bool,
}

/// 别名 → 发给 PTY 的字节。**不在表里的输入一律当文本发出去、并自动补一个回车**。
pub const NUDGE_KEYS: &[(&str, &[u8])] = &[
    ("enter", b"\r"),
    ("esc", b"\x1b"),
    ("回车", b"\r"), // 键名，不是文案
];

/// 能挪菜单高亮的键名（候选；只有进了 `NUDGE_KEYS` 的才算数）。
pub const MOVE_KEY_CANDIDATES: &[&str] = &["up", "down", "上", "下"]; // 键名，不是文案

fn bytes_of(alias: &str) -> Option<&'static [u8]> {
    NUDGE_KEYS
        .iter()
        .find(|(n, _)| *n == alias)
        .map(|(_, b)| *b)
}

/// 给人看的键名：从表里算；同一串字节只露字典序最小的那个别名。
pub fn nudge_key_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = NUDGE_KEYS.iter().map(|(n, _)| *n).collect();
    names.sort();
    let mut seen: Vec<&'static [u8]> = Vec::new();
    names
        .into_iter()
        .filter(|n| {
            let b = bytes_of(n).expect("名字来自表本身");
            if seen.contains(&b) {
                false
            } else {
                seen.push(b);
                true
            }
        })
        .collect()
}

pub fn selection_move_keys() -> Vec<&'static str> {
    MOVE_KEY_CANDIDATES
        .iter()
        .copied()
        .filter(|k| bytes_of(k).is_some())
        .collect()
}

/// `nudge_session` 的输入 → 写进 PTY 的字节。别名 → 表里的字节；其余 = 去毒后的文本 + 回车
/// （控制字符换成 Unicode 控制图形，换行变空格：别让文本里藏着按键）。
pub fn nudge_bytes(input: &str) -> Vec<u8> {
    let key = input.trim();
    if let Some(b) = bytes_of(&key.to_ascii_lowercase()) {
        return b.to_vec();
    }
    let mut s: String = key
        .chars()
        .map(|c| match c {
            '\r' | '\n' => ' ',
            '\u{7f}' => '\u{2421}',
            c if (c as u32) < 0x20 => {
                char::from_u32(0x2400 + c as u32).expect("U+2400..U+241F 都是合法字符")
            }
            c if ('\u{80}'..='\u{9f}').contains(&c) => '\u{fffd}',
            c => c,
        })
        .collect();
    s.push('\r');
    s.into_bytes()
}

fn backticked(keys: &[&str]) -> String {
    keys.iter()
        .map(|k| format!("`{k}`"))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// `names` / `moves` 由调用方从键表算好传进来（生产里是 [`nudge_key_names`] / [`selection_move_keys`]），
/// 这样「表里真有挪高亮键」那一支也能在测试里走到。
fn render_options(
    lang: Lang,
    options: &[String],
    numbered: bool,
    names: &[&str],
    moves: &[&str],
) -> Vec<String> {
    if options.is_empty() {
        return vec![];
    }
    if numbered {
        let mut v: Vec<String> = options
            .iter()
            .enumerate()
            .map(|(i, o)| format!("  {}. {o}", i + 1))
            .collect();
        v.push(get(lang, "decision.options.numbered").to_string());
        return v;
    }
    let move_text = if moves.is_empty() {
        get(lang, "decision.move.none").to_string()
    } else {
        render(lang, "decision.move.keys", &[("keys", &backticked(moves))])
    };
    let mut v: Vec<String> = options.iter().map(|o| format!("  {o}")).collect();
    v.push(render(
        lang,
        "decision.options.unnumbered",
        &[("keys", &backticked(names)), ("move", &move_text)],
    ));
    v
}

/// `name` / `session` 与 `d` 里的外来文字由调用方事先过 `safe_name`（见模块文档）。
pub fn decision_notice(
    lang: Lang,
    stage: NoticeStage,
    name: &str,
    session: &str,
    is_captain: bool,
    d: &DecisionFacts,
    waited_min: i64,
) -> Notice {
    let head = match stage {
        NoticeStage::First if is_captain => {
            render(lang, "decision.first.captain", &[("name", name)])
        }
        NoticeStage::First => render(lang, "decision.first.worker", &[("name", name)]),
        NoticeStage::Escalate => render(
            lang,
            "decision.escalate",
            &[("name", name), ("minutes", &waited_min.to_string())],
        ),
    };
    let mut lines = vec![head];
    if !d.prompt.is_empty() {
        lines.push(d.prompt.clone());
    }
    lines.extend(render_options(
        lang,
        &d.options,
        d.numbered,
        &nudge_key_names(),
        &selection_move_keys(),
    ));
    lines.push(
        match stage {
            NoticeStage::First if is_captain => get(lang, "decision.tail.captain"),
            NoticeStage::First => get(lang, "decision.tail.worker"),
            NoticeStage::Escalate => get(lang, "decision.tail.escalate"),
        }
        .to_string(),
    );
    lines.push(render(lang, "decision.session", &[("session", session)]));
    Notice {
        text: lines.join("\n"),
        to_human: is_captain || stage == NoticeStage::Escalate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(numbered: bool) -> DecisionFacts {
        DecisionFacts {
            prompt: "Do you trust the files in this folder?".into(),
            options: vec!["Yes, proceed".into(), "No, exit".into()],
            numbered,
        }
    }

    #[test]
    fn worker_first_notice_goes_to_the_captain_with_numbers_when_the_screen_had_them() {
        let n = decision_notice(
            Lang::Zh,
            NoticeStage::First,
            "w1",
            "s-1",
            false,
            &d(true),
            0,
        );
        assert!(!n.to_human);
        assert!(
            n.text.contains("  1. Yes, proceed") && n.text.contains("  2. No, exit"),
            "{}",
            n.text
        );
        assert!(n.text.contains(get(Lang::Zh, "decision.options.numbered")));
        assert!(n.text.contains(get(Lang::Zh, "decision.tail.worker")));
        assert!(
            n.text
                .ends_with(&render(Lang::Zh, "decision.session", &[("session", "s-1")]))
        );
    }

    #[test]
    fn unnumbered_screen_never_gets_invented_numbers_and_lists_keys_from_the_table() {
        let n = decision_notice(
            Lang::Zh,
            NoticeStage::First,
            "w1",
            "s-1",
            false,
            &d(false),
            0,
        );
        assert!(!n.text.contains("1. "), "绝不编造编号：{}", n.text);
        for k in nudge_key_names() {
            assert!(
                n.text.contains(&format!("`{k}`")),
                "键表里的 {k} 没出现在消息里"
            );
        }
        // 键表里没有能挪高亮的键 → 交给人。
        assert!(selection_move_keys().is_empty());
        assert!(n.text.contains(get(Lang::Zh, "decision.move.none")));
    }

    #[test]
    fn captain_notice_goes_straight_to_the_human_and_escalation_always_does() {
        let n = decision_notice(
            Lang::En,
            NoticeStage::First,
            "cap",
            "s-0",
            true,
            &d(true),
            0,
        );
        assert!(n.to_human);
        assert!(n.text.contains(get(Lang::En, "decision.tail.captain")));
        let n = decision_notice(
            Lang::Zh,
            NoticeStage::Escalate,
            "w1",
            "s-1",
            false,
            &d(true),
            7,
        );
        assert!(n.to_human);
        assert!(n.text.starts_with(&render(
            Lang::Zh,
            "decision.escalate",
            &[("name", "w1"), ("minutes", "7")]
        )));
    }

    #[test]
    fn empty_prompt_and_options_are_left_out() {
        let n = decision_notice(
            Lang::Zh,
            NoticeStage::First,
            "w1",
            "s-1",
            false,
            &DecisionFacts {
                prompt: String::new(),
                options: vec![],
                numbered: false,
            },
            0,
        );
        assert_eq!(n.text.lines().count(), 3, "{}", n.text);
    }

    #[test]
    fn key_names_are_derived_one_per_byte_sequence() {
        assert_eq!(nudge_key_names(), ["enter", "esc"]);
    }

    #[test]
    fn nudge_maps_aliases_to_bytes_and_text_to_inert_text_plus_enter() {
        assert_eq!(nudge_bytes("enter"), b"\r");
        assert_eq!(nudge_bytes(" ESC "), b"\x1b");
        assert_eq!(nudge_bytes("回车"), b"\r");
        assert_eq!(nudge_bytes("2"), b"2\r");
        assert_eq!(nudge_bytes("a\x1bb\nc"), "a\u{241b}b c\r".as_bytes());
    }

    #[test]
    fn c1_controls_in_nudge_text_become_replacement_chars_like_the_executor() {
        assert_eq!(nudge_bytes("a\u{9b}b"), "a\u{fffd}b\r".as_bytes());
        assert_eq!(nudge_bytes("x\ty"), "x\u{2409}y\r".as_bytes());
    }

    #[test]
    fn move_keys_in_the_table_replace_the_hand_it_to_a_human_line() {
        let opts = vec!["A".to_string(), "B".to_string()];
        let none = render_options(Lang::En, &opts, false, &["enter", "esc"], &[]);
        assert!(
            none.last()
                .unwrap()
                .contains(get(Lang::En, "decision.move.none"))
        );
        let some = render_options(Lang::En, &opts, false, &["down", "enter"], &["down"]);
        let last = some.last().unwrap();
        assert!(
            last.contains(&render(
                Lang::En,
                "decision.move.keys",
                &[("keys", "`down`")]
            )),
            "{last}"
        );
        assert!(
            !last.contains(get(Lang::En, "decision.move.none")),
            "{last}"
        );
        assert!(last.contains("`down` / `enter`"), "{last}");
    }

    #[test]
    fn every_decision_text_key_reaches_the_output_in_both_languages() {
        for lang in [Lang::Zh, Lang::En] {
            let w = decision_notice(lang, NoticeStage::First, "w1", "s-1", false, &d(false), 0);
            let c = decision_notice(lang, NoticeStage::First, "cap", "s-0", true, &d(true), 0);
            let e = decision_notice(
                lang,
                NoticeStage::Escalate,
                "w1",
                "s-1",
                false,
                &d(false),
                5,
            );
            assert!(
                w.text
                    .starts_with(&render(lang, "decision.first.worker", &[("name", "w1")]))
            );
            assert!(c.text.starts_with(&render(
                lang,
                "decision.first.captain",
                &[("name", "cap")]
            )));
            assert!(e.text.ends_with(&format!(
                "{}\n{}",
                get(lang, "decision.tail.escalate"),
                render(lang, "decision.session", &[("session", "s-1")])
            )));
            assert!(c.text.contains(get(lang, "decision.options.numbered")));
            assert!(c.text.contains(get(lang, "decision.tail.captain")));
            assert!(w.text.contains(get(lang, "decision.tail.worker")));
        }
    }

    /// 给 agent / 群看的文字只在 prompts/*.md（Global Constraints）。判定源码里出现中日韩字符只可能是漏网的文案
    /// （键名例外，那一行带 `键名，不是文案` 标记；`expect` / `panic!` 的开发者信息例外）。注释与测试不查。
    #[test]
    fn decision_sources_carry_no_prose_literals() {
        let files = [
            ("plan.rs", include_str!("plan.rs")),
            ("breaker.rs", include_str!("breaker.rs")),
            ("queue.rs", include_str!("queue.rs")),
            ("policy.rs", include_str!("policy.rs")),
            ("notice.rs", include_str!("notice.rs")),
        ];
        let cjk = |c: char| matches!(c, '\u{3000}'..='\u{303f}' | '\u{4e00}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}');
        for (name, src) in files {
            let code = src.split("#[cfg(test)]").next().expect("split 至少一段");
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
                assert!(
                    !code_part.chars().any(cjk),
                    "{name}:{} 有文案字面量：{line}",
                    i + 1
                );
            }
        }
    }
}
