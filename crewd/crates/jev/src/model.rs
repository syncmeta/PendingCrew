//! `POST {base}/v1/systemone` 的请求 / 应答类型（官方文档 docs.typesafe.ai/api.md，2026-10-06 核过）。
//! 选项顺序就是发给模型的顺序（打乱选项防偏向第一个），所以 choice 的 criteria 用有序的 Vec，
//! 序列化成 JSON 对象时保持插入顺序。
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// 官方默认 base URL 与模型名（spec §7.4）。
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
pub const DEFAULT_MODEL: &str = "jev-latest";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Request {
    pub model: String,
    pub state: Value,
    pub questions: BTreeMap<String, Question>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Choice {
        instructions: Value,
        criteria: ChoiceCriteria,
    },
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
    Noul {
        instructions: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
}

/// 有序的「选项名 → 描述」。描述为 `Value::Null` 表示不需要额外说明（官方允许 null）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChoiceCriteria(pub Vec<(String, Value)>);

impl ChoiceCriteria {
    pub fn keys(&self) -> Vec<String> {
        self.0.iter().map(|(k, _)| k.clone()).collect()
    }
}

impl Serialize for ChoiceCriteria {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub yes: Value,
    #[serde(rename = "false")]
    pub no: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Score {
        score: f64,
        #[serde(default)]
        legend: BTreeMap<String, Value>,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Noul {
        noul: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// 官方给 noul 不返回 confidence；文档建议的同尺度量是 |2p − 1|（docs.typesafe.ai/confidence.md）。
pub fn noul_confidence(p: f64) -> f64 {
    (2.0 * p - 1.0).abs().clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn choice_criteria_keep_their_order_on_the_wire() {
        let mut questions = BTreeMap::new();
        questions.insert(
            "route".to_string(),
            Question::Choice {
                instructions: json!("谁来做？"),
                criteria: ChoiceCriteria(vec![
                    ("zeta".into(), json!("最后一个字母开头，但排在第一")),
                    ("alpha".into(), Value::Null),
                    ("mid".into(), json!({"role": "x"})),
                ]),
            },
        );
        let req = Request {
            model: DEFAULT_MODEL.into(),
            state: json!({"todo": "修滚动"}),
            questions,
        };
        let text = serde_json::to_string(&req).unwrap();
        assert_eq!(
            text,
            r#"{"model":"jev-latest","state":{"todo":"修滚动"},"questions":{"route":{"type":"choice","instructions":"谁来做？","criteria":{"zeta":"最后一个字母开头，但排在第一","alpha":null,"mid":{"role":"x"}}}}}"#
        );
    }

    #[test]
    fn noul_and_score_questions_serialize_like_the_docs() {
        let noul = Question::Noul {
            instructions: json!("要叫醒吗？"),
            criteria: Some(NoulCriteria {
                yes: json!("是"),
                no: json!("否"),
            }),
        };
        assert_eq!(
            serde_json::to_value(&noul).unwrap(),
            json!({"type": "noul", "instructions": "要叫醒吗？", "criteria": {"true": "是", "false": "否"}})
        );
        let bare = Question::Noul {
            instructions: json!("q"),
            criteria: None,
        };
        assert_eq!(
            serde_json::to_value(&bare).unwrap(),
            json!({"type": "noul", "instructions": "q"})
        );
        let score = Question::Score {
            instructions: json!("多急？"),
            criteria: vec![json!("不急"), json!("急")],
        };
        assert_eq!(
            serde_json::to_value(&score).unwrap(),
            json!({"type": "score", "instructions": "多急？", "criteria": ["不急", "急"]})
        );
    }

    #[test]
    fn documented_answer_shapes_parse() {
        let text = include_str!("../tests/fixtures/docs-shapes.json");
        let resp: Response = serde_json::from_str(text).unwrap();
        assert_eq!(resp.model, "jev-1.13.0");
        assert_eq!(
            resp.usage,
            Some(Usage {
                input_tokens: 412,
                output_tokens: 57
            })
        );
        match &resp.answers["route"] {
            Answer::Choice {
                choice,
                probabilities,
                confidence,
            } => {
                assert_eq!(choice, "scroll-fix");
                assert_eq!(probabilities.len(), 3);
                assert!((confidence - 0.7).abs() < 1e-9);
            }
            other => panic!("route 应是 choice：{other:?}"),
        }
        match &resp.answers["wake_release"] {
            Answer::Noul { noul } => assert!((noul - 0.08).abs() < 1e-9),
            other => panic!("wake_release 应是 noul：{other:?}"),
        }
        match &resp.answers["urgency"] {
            Answer::Score {
                score,
                legend,
                probabilities,
                confidence,
            } => {
                assert!((score - 1.9).abs() < 1e-9);
                assert_eq!(legend.len(), 3);
                assert_eq!(probabilities["2"], 0.9);
                assert!((confidence - 0.85).abs() < 1e-9);
            }
            other => panic!("urgency 应是 score：{other:?}"),
        }
    }

    #[test]
    fn unknown_answer_type_is_an_error_not_a_default() {
        let text = r#"{"model":"m","answers":{"x":{"type":"ranking","order":[]}}}"#;
        assert!(serde_json::from_str::<Response>(text).is_err());
    }

    #[test]
    fn noul_confidence_is_distance_from_a_coin_flip() {
        assert_eq!(noul_confidence(0.5), 0.0);
        assert_eq!(noul_confidence(1.0), 1.0);
        assert_eq!(noul_confidence(0.0), 1.0);
        assert!((noul_confidence(0.8) - 0.6).abs() < 1e-9);
    }
}
