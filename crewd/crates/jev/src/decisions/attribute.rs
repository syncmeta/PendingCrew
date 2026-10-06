//! 随手消息归属（spec §7.2、§17）：一道 choice，选项 = 候选（最近群消息 / 未完成入口 / 工作项 / 群）+ none。
//! 候选由调用方按相关度或时间排好，只取前 20 个；发出前按种子打乱。
use super::{
    Built, DecisionKind, MAX_CANDIDATES, MAX_SUMMARY_CHARS, Thresholds, Verdict, check_keys, clip,
    gate, non_empty, read_choice,
};
use crate::model::{ChoiceCriteria, Question, Response};
use crate::prompts::{ATTRIBUTE, VOCAB};
use crate::shuffle::{seed_from, shuffle};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const QUESTION_ID: &str = "attribute";
pub const NONE: &str = "none";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    Message,
    Todo,
    WorkItem,
    Channel,
}

impl CandidateKind {
    fn word(self) -> &'static str {
        match self {
            CandidateKind::Message => VOCAB.get("kind.message"),
            CandidateKind::Todo => VOCAB.get("kind.todo"),
            CandidateKind::WorkItem => VOCAB.get("kind.work_item"),
            CandidateKind::Channel => VOCAB.get("kind.channel"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub key: String,
    pub kind: CandidateKind,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttributeInput {
    pub message: String,
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "target", content = "key", rename_all = "snake_case")]
pub enum Attribution {
    Candidate(String),
    None,
}

pub fn seed(input: &AttributeInput) -> u64 {
    seed_from(&[DecisionKind::Attribute.as_str(), &input.message])
}

pub fn build(input: &AttributeInput, seed: u64) -> Result<Built, String> {
    let message = non_empty(&input.message, "消息")?;
    let candidates: Vec<&Candidate> = input.candidates.iter().take(MAX_CANDIDATES).collect();
    if candidates.is_empty() {
        return Err("没有候选".into());
    }
    check_keys(candidates.iter().map(|c| c.key.as_str()), &[NONE])?;
    let mut options: Vec<(String, Value)> = candidates
        .iter()
        .map(|c| {
            (
                c.key.clone(),
                json!({ "kind": c.kind.word(), "summary": clip(&c.summary, MAX_SUMMARY_CHARS) }),
            )
        })
        .collect();
    options.push((NONE.into(), json!(ATTRIBUTE.get("option.none"))));
    shuffle(&mut options, seed);
    let criteria = ChoiceCriteria(options);
    let keys = criteria.keys();
    let mut questions = BTreeMap::new();
    questions.insert(
        QUESTION_ID.to_string(),
        Question::Choice {
            instructions: json!(ATTRIBUTE.get("instructions")),
            criteria,
        },
    );
    Ok(Built {
        kind: DecisionKind::Attribute,
        state: json!({ "message": message }),
        questions,
        options: keys,
    })
}

pub fn interpret(
    built: &Built,
    response: &Response,
    thresholds: &Thresholds,
) -> Verdict<Attribution> {
    match read_choice(built, response, QUESTION_ID) {
        Ok((key, confidence)) => {
            let a = if key == NONE {
                Attribution::None
            } else {
                Attribution::Candidate(key)
            };
            gate(a, confidence, thresholds.choice_min)
        }
        Err(reason) => Verdict::defer(reason),
    }
}

pub fn attribution_key(a: &Attribution) -> &str {
    match a {
        Attribution::Candidate(k) => k,
        Attribution::None => NONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decisions::Outcome;
    use crate::model::Answer;

    fn input(n: usize) -> AttributeInput {
        AttributeInput {
            message: "那个还是不行".into(),
            candidates: (1..=n)
                .map(|i| Candidate {
                    key: format!("c{i}"),
                    kind: if i % 2 == 0 {
                        CandidateKind::Todo
                    } else {
                        CandidateKind::Message
                    },
                    summary: format!("候选{i}"),
                })
                .collect(),
        }
    }

    #[test]
    fn candidates_plus_none_capped_and_described_with_vocab_words() {
        let built = build(&input(25), 5).unwrap();
        assert_eq!(built.options.len(), MAX_CANDIDATES + 1);
        assert!(built.options.contains(&NONE.to_string()));
        assert!(!built.options.contains(&"c21".to_string()));
        let wire = serde_json::to_value(&built.questions[QUESTION_ID]).unwrap();
        assert_eq!(wire["criteria"]["c2"]["kind"], VOCAB.get("kind.todo"));
        assert_eq!(wire["criteria"]["c1"]["summary"], "候选1");
    }

    #[test]
    fn none_is_reserved_and_empty_candidate_list_is_refused() {
        let mut i = input(2);
        i.candidates[0].key = "none".into();
        assert!(build(&i, 0).is_err());
        assert!(build(&input(0), 0).is_err());
    }

    #[test]
    fn none_maps_to_none() {
        let built = build(&input(2), 0).unwrap();
        let mut answers = BTreeMap::new();
        answers.insert(
            QUESTION_ID.to_string(),
            Answer::Choice {
                choice: "none".into(),
                probabilities: BTreeMap::new(),
                confidence: 0.9,
            },
        );
        let resp = Response {
            model: "m".into(),
            answers,
            usage: None,
        };
        assert_eq!(
            interpret(&built, &resp, &Thresholds::default()).outcome,
            Outcome::Act(Attribution::None)
        );
    }
}
