//! 人类消息叫醒谁（spec §7.2、§17）：群里每个成员一道 noul（题号 = 成员 key）。
//! 题号是请求体 `questions` 的键，**会随请求发出**——成员 key 和选项名一样对外可见，只能是不含信息的 slug。
//! 叫醒集合 = p ≥ wake_yes 的成员；空集 → 交机长。调用方只能传**这个群的成员**（§17：只在群成员里选）。
use super::{
    Built, DecisionKind, DeferReason, MAX_RECENT, MAX_SUMMARY_CHARS, MAX_WAKE_MEMBERS, Member,
    Outcome, Thresholds, Verdict, check_keys, check_unit, clip, member_view, most_recent,
    non_empty,
};
use crate::model::{Answer, NoulCriteria, Question, Response, noul_confidence};
use crate::prompts::WAKE;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct WakeInput {
    /// 人类这条消息。
    pub message: String,
    /// 之前几条群消息，旧在前；只取最后 5 条。
    pub recent: Vec<String>,
    /// 这个群的 session 成员（不含人类）。
    pub members: Vec<Member>,
}

/// 被叫醒的成员 key，按名单（最近活跃）顺序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WakeSet {
    pub wake: Vec<String>,
}

pub fn build(input: &WakeInput) -> Result<Built, String> {
    let message = non_empty(&input.message, "消息")?;
    let members = most_recent(&input.members, MAX_WAKE_MEMBERS);
    if members.is_empty() {
        return Err("群里没有 session 成员".into());
    }
    check_keys(members.iter().map(|m| m.key.as_str()), &[])?;
    let skip = input.recent.len().saturating_sub(MAX_RECENT);
    let recent: Vec<String> = input.recent[skip..]
        .iter()
        .map(|r| clip(r, MAX_SUMMARY_CHARS))
        .collect();
    let mut questions = BTreeMap::new();
    for m in &members {
        questions.insert(
            m.key.clone(),
            Question::Noul {
                instructions: json!({
                    "member": member_view(m),
                    "question": WAKE.get("question"),
                }),
                criteria: Some(NoulCriteria {
                    yes: json!(WAKE.get("true")),
                    no: json!(WAKE.get("false")),
                }),
            },
        );
    }
    Ok(Built {
        kind: DecisionKind::Wake,
        state: json!({ "message": message, "recent": recent }),
        questions,
        options: members.iter().map(|m| m.key.clone()).collect(),
    })
}

/// 每个成员的 p（按 `built.options` 顺序）；任何一题缺失 / 类型不对 / 越界 → 整个判断作废。
pub fn probabilities(
    built: &Built,
    response: &Response,
) -> Result<Vec<(String, f64)>, DeferReason> {
    built
        .options
        .iter()
        .map(|key| match response.answers.get(key) {
            Some(Answer::Noul { noul }) => {
                check_unit(*noul)?;
                Ok((key.clone(), *noul))
            }
            Some(other) => Err(DeferReason::Malformed(format!(
                "{key} 的答案类型不对：{other:?}"
            ))),
            None => Err(DeferReason::Malformed(format!("应答里没有 {key}"))),
        })
        .collect()
}

pub fn interpret(built: &Built, response: &Response, thresholds: &Thresholds) -> Verdict<WakeSet> {
    let ps = match probabilities(built, response) {
        Ok(ps) => ps,
        Err(reason) => return Verdict::defer(reason),
    };
    let wake: Vec<String> = ps
        .iter()
        .filter(|(_, p)| *p >= thresholds.wake_yes)
        .map(|(k, _)| k.clone())
        .collect();
    // 整组的置信度 = 最没把握的那一题（|2p−1| 的最小值）。
    let confidence = ps
        .iter()
        .map(|(_, p)| noul_confidence(*p))
        .fold(1.0_f64, f64::min);
    let set = WakeSet { wake };
    let outcome = if set.wake.is_empty() {
        Outcome::Defer(DeferReason::NobodySelected)
    } else {
        Outcome::Act(set.clone())
    };
    Verdict {
        best: Some(set),
        confidence: Some(confidence),
        outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decisions::testing::member;

    fn input() -> WakeInput {
        WakeInput {
            message: "@发版 0.1.40 的 dmg 能出了吗".into(),
            recent: (1..=8).map(|i| format!("旧消息{i}")).collect(),
            members: vec![
                member("captain", 2),
                member("release", 1),
                member("scroll", 3),
            ],
        }
    }

    fn nouls(ps: &[(&str, f64)]) -> Response {
        Response {
            model: "jev-1.13.0".into(),
            answers: ps
                .iter()
                .map(|(k, p)| (k.to_string(), Answer::Noul { noul: *p }))
                .collect(),
            usage: None,
        }
    }

    #[test]
    fn one_noul_per_member_and_only_the_last_five_recent_messages() {
        let built = build(&input()).unwrap();
        assert_eq!(built.options, ["release", "captain", "scroll"]);
        assert_eq!(built.questions.len(), 3);
        assert_eq!(
            built.state["recent"],
            json!(["旧消息4", "旧消息5", "旧消息6", "旧消息7", "旧消息8"])
        );
        let q = serde_json::to_value(&built.questions["release"]).unwrap();
        assert_eq!(q["type"], "noul");
        assert_eq!(q["instructions"]["member"]["name"], "成员release");
        assert_eq!(q["instructions"]["question"], WAKE.get("question"));
    }

    #[test]
    fn wake_set_is_everyone_at_or_above_the_line() {
        let built = build(&input()).unwrap();
        let v = interpret(
            &built,
            &nouls(&[("release", 0.97), ("captain", 0.5), ("scroll", 0.1)]),
            &Thresholds::default(),
        );
        let set = WakeSet {
            wake: vec!["release".into(), "captain".into()],
        };
        assert_eq!(v.outcome, Outcome::Act(set));
        assert!(
            (v.confidence.unwrap() - 0.0).abs() < 1e-9,
            "captain 恰好 0.5，最没把握"
        );
    }

    #[test]
    fn empty_set_goes_to_the_captain() {
        let built = build(&input()).unwrap();
        let v = interpret(
            &built,
            &nouls(&[("release", 0.2), ("captain", 0.1), ("scroll", 0.05)]),
            &Thresholds::default(),
        );
        assert_eq!(v.outcome, Outcome::Defer(DeferReason::NobodySelected));
        assert_eq!(v.best, Some(WakeSet { wake: vec![] }));
    }

    #[test]
    fn a_missing_member_answer_voids_the_whole_judgement() {
        let built = build(&input()).unwrap();
        let v = interpret(
            &built,
            &nouls(&[("release", 0.9), ("captain", 0.1)]),
            &Thresholds::default(),
        );
        assert!(matches!(
            v.outcome,
            Outcome::Defer(DeferReason::Malformed(_))
        ));
    }

    #[test]
    fn no_members_or_empty_message_is_refused() {
        let mut i = input();
        i.members.clear();
        assert!(build(&i).is_err());
        let mut i = input();
        i.message = "\n".into();
        assert!(build(&i).is_err());
    }
}
