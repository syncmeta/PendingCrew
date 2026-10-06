//! 入口 Todo 派给谁（spec §7.2）：一道 choice，选项 = 机长 / 每个活跃 session / 新开 session / 新建机组。
//! 选项顺序按种子打乱（jaggedness #8）。new_crew 由调用方交机长执行（建组要起名，Jev 不生成文字）。
use super::{
    Built, DecisionKind, MAX_MEMBERS, Member, Thresholds, Verdict, check_keys, gate, member_view,
    most_recent, non_empty, read_choice,
};
use crate::model::{ChoiceCriteria, Question, Response};
use crate::prompts::ROUTE_TODO;
use crate::shuffle::{seed_from, shuffle};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const QUESTION_ID: &str = "route";
pub const CAPTAIN: &str = "captain";
pub const NEW_SESSION: &str = "new_session";
pub const NEW_CREW: &str = "new_crew";
pub const RESERVED: [&str; 3] = [CAPTAIN, NEW_SESSION, NEW_CREW];

#[derive(Debug, Clone, PartialEq)]
pub struct RouteTodoInput {
    /// 入口 Todo 原文。
    pub todo: String,
    /// 活跃 session（不含机长；机长是固定选项）。
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "target", content = "key", rename_all = "snake_case")]
pub enum RouteTarget {
    Captain,
    Member(String),
    NewSession,
    NewCrew,
}

pub fn seed(input: &RouteTodoInput) -> u64 {
    seed_from(&[DecisionKind::RouteTodo.as_str(), &input.todo])
}

pub fn build(input: &RouteTodoInput, seed: u64) -> Result<Built, String> {
    let todo = non_empty(&input.todo, "待办")?;
    let members = most_recent(&input.members, MAX_MEMBERS);
    check_keys(members.iter().map(|m| m.key.as_str()), &RESERVED)?;
    let mut options: Vec<(String, Value)> = vec![
        (CAPTAIN.into(), json!(ROUTE_TODO.get("option.captain"))),
        (
            NEW_SESSION.into(),
            json!(ROUTE_TODO.get("option.new_session")),
        ),
        (NEW_CREW.into(), json!(ROUTE_TODO.get("option.new_crew"))),
    ];
    options.extend(members.iter().map(|m| (m.key.clone(), member_view(m))));
    shuffle(&mut options, seed);
    let criteria = ChoiceCriteria(options);
    let keys = criteria.keys();
    let mut questions = BTreeMap::new();
    questions.insert(
        QUESTION_ID.to_string(),
        Question::Choice {
            instructions: json!(ROUTE_TODO.get("instructions")),
            criteria,
        },
    );
    Ok(Built {
        kind: DecisionKind::RouteTodo,
        state: json!({ "todo": todo }),
        questions,
        options: keys,
    })
}

pub fn interpret(
    built: &Built,
    response: &Response,
    thresholds: &Thresholds,
) -> Verdict<RouteTarget> {
    let (key, confidence) = match read_choice(built, response, QUESTION_ID) {
        Ok(v) => v,
        Err(reason) => return Verdict::defer(reason),
    };
    let target = match key.as_str() {
        CAPTAIN => RouteTarget::Captain,
        NEW_SESSION => RouteTarget::NewSession,
        NEW_CREW => RouteTarget::NewCrew,
        _ => RouteTarget::Member(key),
    };
    gate(target, confidence, thresholds.choice_min)
}

/// 评测与诊断用：从 `Verdict` 里拿到选项名（与数据集里的期望同一套词）。
pub fn target_key(target: &RouteTarget) -> &str {
    match target {
        RouteTarget::Captain => CAPTAIN,
        RouteTarget::NewSession => NEW_SESSION,
        RouteTarget::NewCrew => NEW_CREW,
        RouteTarget::Member(k) => k,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decisions::testing::member;
    use crate::decisions::{DeferReason, Outcome};
    use crate::model::Answer;

    fn input(n: usize) -> RouteTodoInput {
        RouteTodoInput {
            todo: "  修一下终端滚动跳动  ".into(),
            members: (1..=n).map(|r| member(&format!("m{r}"), r)).collect(),
        }
    }

    fn answer(choice: &str, confidence: f64) -> Response {
        let mut answers = BTreeMap::new();
        answers.insert(
            QUESTION_ID.to_string(),
            Answer::Choice {
                choice: choice.into(),
                probabilities: BTreeMap::new(),
                confidence,
            },
        );
        Response {
            model: "jev-1.13.0".into(),
            answers,
            usage: None,
        }
    }

    #[test]
    fn options_are_the_three_fixed_ones_plus_members_shuffled_by_seed() {
        let a = build(&input(4), 1).unwrap();
        let b = build(&input(4), 1).unwrap();
        assert_eq!(a, b, "同一种子同一请求");
        let mut sorted = a.options.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            ["captain", "m1", "m2", "m3", "m4", "new_crew", "new_session"]
        );
        let firsts: std::collections::BTreeSet<String> = (0..40)
            .map(|s| build(&input(4), s).unwrap().options[0].clone())
            .collect();
        assert!(firsts.len() >= 5, "打乱后排第一的选项应该轮换：{firsts:?}");
        assert_eq!(a.state, json!({"todo": "修一下终端滚动跳动"}));
    }

    #[test]
    fn wire_order_matches_options() {
        let built = build(&input(3), 7).unwrap();
        // 必须直接序列化成文本：serde_json::Value 的对象是按键排序的，转一道 Value 顺序就没了。
        let text = serde_json::to_string(&built.request("jev-latest")).unwrap();
        let mut last = 0;
        for key in &built.options {
            let pos = text.find(&format!("\"{key}\":")).unwrap();
            assert!(pos >= last, "选项 {key} 在线上的位置乱了");
            last = pos;
        }
    }

    #[test]
    fn roster_is_capped_at_fifteen_most_recent() {
        let built = build(&input(25), 3).unwrap();
        assert_eq!(built.options.len(), 3 + MAX_MEMBERS);
        assert!(built.options.contains(&"m15".to_string()));
        assert!(!built.options.contains(&"m16".to_string()));
    }

    #[test]
    fn bad_inputs_are_refused() {
        let mut i = input(2);
        i.todo = "   ".into();
        assert!(build(&i, 0).is_err());
        let mut i = input(2);
        i.members[0].key = "captain".into();
        assert!(build(&i, 0).is_err());
        let mut i = input(2);
        i.members[1].key = "m1".into();
        assert!(build(&i, 0).is_err());
    }

    #[test]
    fn answers_map_to_targets_and_low_confidence_defers() {
        let built = build(&input(2), 0).unwrap();
        let th = Thresholds::default();
        let v = interpret(&built, &answer("m2", 0.9), &th);
        assert_eq!(v.outcome, Outcome::Act(RouteTarget::Member("m2".into())));
        let v = interpret(&built, &answer("new_crew", 0.8), &th);
        assert_eq!(v.outcome, Outcome::Act(RouteTarget::NewCrew));
        let v = interpret(&built, &answer("captain", 0.2), &th);
        assert_eq!(v.best, Some(RouteTarget::Captain), "评测要看到模型的答案");
        assert_eq!(
            v.outcome,
            Outcome::Defer(DeferReason::LowConfidence { confidence: 0.2 })
        );
    }

    #[test]
    fn invented_or_missing_answers_are_malformed() {
        let built = build(&input(2), 0).unwrap();
        let th = Thresholds::default();
        assert!(matches!(
            interpret(&built, &answer("m9", 0.99), &th).outcome,
            Outcome::Defer(DeferReason::Malformed(_))
        ));
        assert!(matches!(
            interpret(&built, &answer("m1", 1.5), &th).outcome,
            Outcome::Defer(DeferReason::Malformed(_))
        ));
        let empty = Response {
            model: "m".into(),
            answers: BTreeMap::new(),
            usage: None,
        };
        assert!(matches!(
            interpret(&built, &empty, &th).outcome,
            Outcome::Defer(DeferReason::Malformed(_))
        ));
    }
}
