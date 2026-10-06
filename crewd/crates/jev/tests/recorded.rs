//! Task 9 用 `jev-eval --record` 录下的真实应答原文（每种决策一条）：我们的类型必须能解析，
//! 三种答案类型都要出现过（spec §12「录制真实响应做夹具」）。夹具缺了必须失败，不能静默跳过。
use crewd_jev::model::{Answer, Response};
use std::collections::BTreeSet;
use std::path::Path;

const RECORDED: [&str; 4] = [
    "route-01-s0.json",
    "wake-02-s0.json",
    "attr-02-s0.json",
    "urg-04-s0.json",
];

#[test]
fn recorded_real_responses_parse_with_our_types() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recorded");
    let mut kinds = BTreeSet::new();
    for name in RECORDED {
        let path = dir.join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("缺夹具 {}：{e}", path.display()));
        let response: Response =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} 解析失败：{e}"));
        assert!(
            response.model.starts_with("jev-"),
            "{name}: {}",
            response.model
        );
        for answer in response.answers.values() {
            kinds.insert(match answer {
                Answer::Choice { confidence, .. } => {
                    assert!((0.0..=1.0).contains(confidence));
                    "choice"
                }
                Answer::Score { confidence, .. } => {
                    assert!((0.0..=1.0).contains(confidence));
                    "score"
                }
                Answer::Noul { noul } => {
                    assert!((0.0..=1.0).contains(noul));
                    "noul"
                }
            });
        }
    }
    assert_eq!(kinds, BTreeSet::from(["choice", "noul", "score"]));
}
