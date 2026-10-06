//! 侧边栏紧急度（spec §7.2）：每个机组一道 score，五级。级别是有序的，**不打乱**。
//! 时长、次数在这里换成词；与最近活跃合成排序键是调用方（M5b）的事。
use super::{
    Built, DecisionKind, DeferReason, MAX_RECENT, MAX_SUMMARY_CHARS, Thresholds, Verdict,
    check_unit, clip, gate, mentions_word, non_empty, wait_word,
};
use crate::model::{Answer, Question, Response};
use crate::prompts::URGENCY;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

pub const QUESTION_ID: &str = "urgency";
pub const LEVELS: usize = 5;
/// 一次最多列出的等人项。
pub const MAX_WAITING: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct HumanRequest {
    pub text: String,
    pub waited: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UrgencyInput {
    pub crew_name: String,
    pub waiting_on_human: Vec<HumanRequest>,
    /// 群里未读的 @人类 次数（代码数，给 Jev 的是词）。
    pub human_mentions: usize,
    /// 系统发现的问题，短句（额度用尽、会话起不来……）。
    pub issues: Vec<String>,
    /// 最近几条群消息，旧在前；只取最后 5 条。
    pub recent: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Urgency {
    /// 概率加权的位置，0.0–4.0。
    pub score: f64,
    /// 概率最大的那一级。
    pub level: u8,
}

pub fn build(input: &UrgencyInput) -> Result<Built, String> {
    let name = non_empty(&input.crew_name, "机组名")?;
    let mut waiting = input.waiting_on_human.clone();
    // 等得最久的排前面，列最多 8 条。
    waiting.sort_by(|a, b| b.waited.cmp(&a.waited));
    waiting.truncate(MAX_WAITING);
    let waiting: Vec<_> = waiting
        .iter()
        .map(|w| json!({ "question": clip(&w.text, MAX_SUMMARY_CHARS), "waited": wait_word(w.waited) }))
        .collect();
    let skip = input.recent.len().saturating_sub(MAX_RECENT);
    let recent: Vec<String> = input.recent[skip..]
        .iter()
        .map(|r| clip(r, MAX_SUMMARY_CHARS))
        .collect();
    let issues: Vec<String> = input
        .issues
        .iter()
        .map(|i| clip(i, MAX_SUMMARY_CHARS))
        .filter(|i| !i.is_empty())
        .collect();
    let levels = (0..LEVELS)
        .map(|i| json!(URGENCY.get(&format!("level.{i}"))))
        .collect();
    let mut questions = BTreeMap::new();
    questions.insert(
        QUESTION_ID.to_string(),
        Question::Score {
            instructions: json!(URGENCY.get("instructions")),
            criteria: levels,
        },
    );
    Ok(Built {
        kind: DecisionKind::Urgency,
        state: json!({ "crew": {
            "name": name,
            "waiting_on_human": waiting,
            "human_mentions": mentions_word(input.human_mentions),
            "issues": issues,
            "recent": recent,
        }}),
        questions,
        options: (0..LEVELS).map(|i| i.to_string()).collect(),
    })
}

pub fn interpret(_built: &Built, response: &Response, thresholds: &Thresholds) -> Verdict<Urgency> {
    let (score, probabilities, confidence) = match response.answers.get(QUESTION_ID) {
        Some(Answer::Score {
            score,
            probabilities,
            confidence,
            ..
        }) => (*score, probabilities, *confidence),
        Some(other) => {
            return Verdict::defer(DeferReason::Malformed(format!(
                "urgency 的答案类型不对：{other:?}"
            )));
        }
        None => return Verdict::defer(DeferReason::Malformed("应答里没有 urgency".into())),
    };
    if let Err(reason) = check_unit(confidence) {
        return Verdict::defer(reason);
    }
    if !(0.0..=(LEVELS - 1) as f64).contains(&score) {
        return Verdict::defer(DeferReason::Malformed(format!("score 越界 {score}")));
    }
    let mut level: Option<(u8, f64)> = None;
    for (key, p) in probabilities {
        let Ok(l) = key.parse::<u8>() else {
            return Verdict::defer(DeferReason::Malformed(format!("级别名不是数字 {key:?}")));
        };
        if usize::from(l) >= LEVELS {
            return Verdict::defer(DeferReason::Malformed(format!("级别越界 {l}")));
        }
        if level.is_none_or(|(_, best)| *p > best) {
            level = Some((l, *p));
        }
    }
    let Some((level, _)) = level else {
        return Verdict::defer(DeferReason::Malformed("score 没有 probabilities".into()));
    };
    gate(Urgency { score, level }, confidence, thresholds.score_min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decisions::Outcome;
    use crate::prompts::VOCAB;

    fn input() -> UrgencyInput {
        UrgencyInput {
            crew_name: "PendingCrew Mac".into(),
            waiting_on_human: vec![
                HumanRequest {
                    text: "要不要发 0.1.40".into(),
                    waited: Duration::from_secs(20 * 60),
                },
                HumanRequest {
                    text: "Polar 的 webhook 密钥放哪".into(),
                    waited: Duration::from_secs(50 * 3600),
                },
            ],
            human_mentions: 4,
            issues: vec!["额度用尽".into(), "  ".into()],
            recent: vec!["a".into()],
        }
    }

    fn score_answer(score: f64, probs: &[(&str, f64)], confidence: f64) -> Response {
        let mut answers = BTreeMap::new();
        answers.insert(
            QUESTION_ID.to_string(),
            Answer::Score {
                score,
                legend: BTreeMap::new(),
                probabilities: probs.iter().map(|(k, p)| (k.to_string(), *p)).collect(),
                confidence,
            },
        );
        Response {
            model: "m".into(),
            answers,
            usage: None,
        }
    }

    #[test]
    fn levels_are_in_order_and_quantities_are_words() {
        let built = build(&input()).unwrap();
        let q = serde_json::to_value(&built.questions[QUESTION_ID]).unwrap();
        let levels = q["criteria"].as_array().unwrap();
        assert_eq!(levels.len(), LEVELS);
        assert_eq!(levels[0], URGENCY.get("level.0"));
        assert_eq!(levels[4], URGENCY.get("level.4"));
        let crew = &built.state["crew"];
        assert_eq!(
            crew["waiting_on_human"][0]["waited"],
            VOCAB.get("wait.days"),
            "最久的排前"
        );
        assert_eq!(
            crew["waiting_on_human"][1]["waited"],
            VOCAB.get("wait.minutes")
        );
        assert_eq!(crew["human_mentions"], VOCAB.get("mentions.many"));
        assert_eq!(crew["issues"], json!(["额度用尽"]));
    }

    #[test]
    fn state_never_carries_the_durations_or_counts_as_digits() {
        let built = build(&input()).unwrap();
        let text = built.state.to_string();
        // 机组名、问题原文里本来就有的数字（0.1.40）除外，代码算的量（20 分钟、50 小时、4 次）不能出现。
        for leaked in ["1200", "180000", "\"4\"", ":4,", "50"] {
            assert!(
                !text.contains(leaked),
                "state 泄漏了原始量 {leaked}：{text}"
            );
        }
    }

    #[test]
    fn argmax_level_and_gate() {
        let built = build(&input()).unwrap();
        let th = Thresholds::default();
        let v = interpret(
            &built,
            &score_answer(2.9, &[("2", 0.1), ("3", 0.9), ("4", 0.0)], 0.8),
            &th,
        );
        assert_eq!(
            v.outcome,
            Outcome::Act(Urgency {
                score: 2.9,
                level: 3
            })
        );
        let v = interpret(
            &built,
            &score_answer(2.0, &[("0", 0.5), ("4", 0.5)], 0.0),
            &th,
        );
        assert!(matches!(
            v.outcome,
            Outcome::Defer(DeferReason::LowConfidence { .. })
        ));
    }

    #[test]
    fn out_of_range_levels_or_scores_are_malformed() {
        let built = build(&input()).unwrap();
        let th = Thresholds::default();
        for resp in [
            score_answer(5.5, &[("4", 1.0)], 1.0),
            score_answer(1.0, &[("7", 1.0)], 1.0),
            score_answer(1.0, &[("x", 1.0)], 1.0),
            score_answer(1.0, &[], 1.0),
        ] {
            assert!(matches!(
                interpret(&built, &resp, &th).outcome,
                Outcome::Defer(DeferReason::Malformed(_))
            ));
        }
    }
}
