//! 中文实测集（spec §7.3「M5 先做实测」、§12「中文实测脚本单独跑」）：
//! 数据集格式、用例 → 决策输入、判对错、汇总（准确率、按置信度分桶、阈值推荐、选项顺序敏感度）。
//! 这里不碰网络；跑真 API 的是 `src/bin/jev-eval.rs`。
use crate::decisions::attribute::{self, AttributeInput, Candidate};
use crate::decisions::route::{self, RouteTodoInput};
use crate::decisions::urgency::{self, HumanRequest, UrgencyInput};
use crate::decisions::wake::{self, WakeInput};
use crate::decisions::{Built, DecisionKind, Member, Thresholds};
use crate::model::{Response, noul_confidence};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    pub crews: BTreeMap<String, CrewFixture>,
    pub cases: Vec<Case>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrewFixture {
    pub name: String,
    /// 含机长（key 固定为 "captain"）。派活时机长是固定选项，不当成员选项发。
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitingFixture {
    pub question: String,
    pub waited_minutes: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum Case {
    RouteTodo {
        id: String,
        crew: String,
        todo: String,
        /// 可接受的选项名（任一即对）。
        expect: Vec<String>,
    },
    Wake {
        id: String,
        crew: String,
        /// 这个群的成员 key。
        channel: Vec<String>,
        #[serde(default)]
        recent: Vec<String>,
        message: String,
        /// 必须叫醒的成员。
        expect: Vec<String>,
        /// 叫不叫都算对的成员。
        #[serde(default)]
        optional: Vec<String>,
    },
    Attribute {
        id: String,
        message: String,
        candidates: Vec<Candidate>,
        expect: Vec<String>,
    },
    Urgency {
        id: String,
        crew_name: String,
        #[serde(default)]
        waiting: Vec<WaitingFixture>,
        #[serde(default)]
        human_mentions: usize,
        #[serde(default)]
        issues: Vec<String>,
        #[serde(default)]
        recent: Vec<String>,
        /// 可接受的最可能级别，闭区间 [低, 高]。
        expect_level: [u8; 2],
    },
}

impl Case {
    pub fn id(&self) -> &str {
        match self {
            Case::RouteTodo { id, .. }
            | Case::Wake { id, .. }
            | Case::Attribute { id, .. }
            | Case::Urgency { id, .. } => id,
        }
    }

    pub fn kind(&self) -> DecisionKind {
        match self {
            Case::RouteTodo { .. } => DecisionKind::RouteTodo,
            Case::Wake { .. } => DecisionKind::Wake,
            Case::Attribute { .. } => DecisionKind::Attribute,
            Case::Urgency { .. } => DecisionKind::Urgency,
        }
    }
}

impl Dataset {
    pub fn parse(text: &str) -> Result<Dataset, String> {
        let ds: Dataset = serde_json::from_str(text).map_err(|e| format!("数据集解析失败：{e}"))?;
        ds.validate()?;
        Ok(ds)
    }

    fn crew(&self, key: &str) -> Result<&CrewFixture, String> {
        self.crews
            .get(key)
            .ok_or_else(|| format!("没有机组 {key:?}"))
    }

    /// 构造一次请求。种子只影响 choice 的选项顺序。
    pub fn build_case(&self, case: &Case, seed: u64) -> Result<Built, String> {
        match case {
            Case::RouteTodo { crew, todo, .. } => {
                let crew = self.crew(crew)?;
                let input = RouteTodoInput {
                    todo: todo.clone(),
                    members: crew
                        .members
                        .iter()
                        .filter(|m| m.key != route::CAPTAIN)
                        .cloned()
                        .collect(),
                };
                route::build(&input, seed)
            }
            Case::Wake {
                crew,
                channel,
                recent,
                message,
                ..
            } => {
                let crew = self.crew(crew)?;
                let members = channel
                    .iter()
                    .map(|k| {
                        crew.members
                            .iter()
                            .find(|m| &m.key == k)
                            .cloned()
                            .ok_or_else(|| format!("群成员 {k:?} 不在机组里"))
                    })
                    .collect::<Result<Vec<Member>, String>>()?;
                wake::build(&WakeInput {
                    message: message.clone(),
                    recent: recent.clone(),
                    members,
                })
            }
            Case::Attribute {
                message,
                candidates,
                ..
            } => attribute::build(
                &AttributeInput {
                    message: message.clone(),
                    candidates: candidates.clone(),
                },
                seed,
            ),
            Case::Urgency {
                crew_name,
                waiting,
                human_mentions,
                issues,
                recent,
                ..
            } => urgency::build(&UrgencyInput {
                crew_name: crew_name.clone(),
                waiting_on_human: waiting
                    .iter()
                    .map(|w| HumanRequest {
                        text: w.question.clone(),
                        waited: Duration::from_secs(w.waited_minutes * 60),
                    })
                    .collect(),
                human_mentions: *human_mentions,
                issues: issues.clone(),
                recent: recent.clone(),
            }),
        }
    }

    /// 用例自洽：id 唯一、引用的机组 / 成员存在、期望答案都在发出的选项里、每条都能构造。
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        for case in &self.cases {
            let id = case.id();
            if !ids.insert(id.to_string()) {
                return Err(format!("用例 id 重复：{id}"));
            }
            let built = self
                .build_case(case, 0)
                .map_err(|e| format!("{id} 构造失败：{e}"))?;
            let allowed: BTreeSet<&str> = built.options.iter().map(String::as_str).collect();
            let expected: Vec<&String> = match case {
                Case::RouteTodo { expect, .. } | Case::Attribute { expect, .. } => {
                    if expect.is_empty() {
                        return Err(format!("{id} 没有期望答案"));
                    }
                    expect.iter().collect()
                }
                Case::Wake {
                    expect, optional, ..
                } => expect.iter().chain(optional.iter()).collect(),
                Case::Urgency { expect_level, .. } => {
                    let [lo, hi] = *expect_level;
                    if lo > hi || usize::from(hi) >= urgency::LEVELS {
                        return Err(format!("{id} 的 expect_level 不合法"));
                    }
                    vec![]
                }
            };
            for e in expected {
                if !allowed.contains(e.as_str()) {
                    return Err(format!("{id} 期望的 {e:?} 不在选项里"));
                }
            }
        }
        Ok(())
    }

    pub fn case(&self, id: &str) -> Option<&Case> {
        self.cases.iter().find(|c| c.id() == id)
    }
}

/// 一次运行（一条用例 × 一个种子）的判定。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Judged {
    pub id: String,
    pub kind: DecisionKind,
    pub seed: u64,
    /// 模型的答案（选项名 / 叫醒集合 / 级别），出错时为空。
    pub answer: String,
    pub confidence: Option<f64>,
    pub correct: bool,
    /// choice：选中的是不是排第一的选项。
    pub picked_first: Option<bool>,
    pub options: usize,
    /// wake：每个非 optional 成员的 (p, 应不应该叫醒)。
    pub nouls: Vec<(f64, bool)>,
    pub error: Option<String>,
}

impl Judged {
    pub fn failed(case: &Case, seed: u64, error: String) -> Judged {
        Judged {
            id: case.id().to_string(),
            kind: case.kind(),
            seed,
            answer: String::new(),
            confidence: None,
            correct: false,
            picked_first: None,
            options: 0,
            nouls: vec![],
            error: Some(error),
        }
    }
}

/// 判对错。看的是模型的答案（`Verdict::best`），不看阈值——阈值正是要从这里推出来的。
pub fn judge(case: &Case, seed: u64, built: &Built, response: &Response) -> Judged {
    let th = Thresholds::default();
    let mut j = Judged::failed(case, seed, String::new());
    j.error = None;
    j.options = built.options.len();
    match case {
        Case::RouteTodo { expect, .. } => {
            let v = route::interpret(built, response, &th);
            match v.best {
                Some(t) => {
                    let key = route::target_key(&t).to_string();
                    j.correct = expect.contains(&key);
                    j.picked_first = Some(built.options.first() == Some(&key));
                    j.answer = key;
                    j.confidence = v.confidence;
                }
                None => j.error = Some(format!("{:?}", v.outcome)),
            }
        }
        Case::Attribute { expect, .. } => {
            let v = attribute::interpret(built, response, &th);
            match v.best {
                Some(a) => {
                    let key = attribute::attribution_key(&a).to_string();
                    j.correct = expect.contains(&key);
                    j.picked_first = Some(built.options.first() == Some(&key));
                    j.answer = key;
                    j.confidence = v.confidence;
                }
                None => j.error = Some(format!("{:?}", v.outcome)),
            }
        }
        Case::Wake {
            expect, optional, ..
        } => match wake::probabilities(built, response) {
            Ok(ps) => {
                let woken: BTreeSet<&str> = ps
                    .iter()
                    .filter(|(_, p)| *p >= th.wake_yes)
                    .map(|(k, _)| k.as_str())
                    .collect();
                j.correct = wake_set_ok(&woken, expect, optional);
                j.answer = woken.into_iter().collect::<Vec<_>>().join(",");
                j.nouls = ps
                    .iter()
                    .filter(|(k, _)| !optional.contains(k))
                    .map(|(k, p)| (*p, expect.contains(k)))
                    .collect();
                j.confidence = ps.iter().map(|(_, p)| noul_confidence(*p)).reduce(f64::min);
            }
            Err(reason) => j.error = Some(reason.label()),
        },
        Case::Urgency { expect_level, .. } => {
            let v = urgency::interpret(built, response, &th);
            match v.best {
                Some(u) => {
                    j.correct = (expect_level[0]..=expect_level[1]).contains(&u.level);
                    j.answer = format!("{} ({:.2})", u.level, u.score);
                    j.confidence = v.confidence;
                }
                None => j.error = Some(format!("{:?}", v.outcome)),
            }
        }
    }
    j
}

fn wake_set_ok(woken: &BTreeSet<&str>, expect: &[String], optional: &[String]) -> bool {
    expect.iter().all(|e| woken.contains(e.as_str()))
        && woken
            .iter()
            .all(|w| expect.iter().any(|e| e == w) || optional.iter().any(|o| o == w))
}

/// 置信度分桶（左闭右开，最后一桶含 1.0）。
pub const BUCKETS: [(f64, f64); 5] = [
    (0.0, 0.25),
    (0.25, 0.5),
    (0.5, 0.75),
    (0.75, 0.9),
    (0.9, 1.0),
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BucketStat {
    pub lo: f64,
    pub hi: f64,
    pub n: usize,
    pub correct: usize,
}

pub fn calibration(judged: &[&Judged]) -> Vec<BucketStat> {
    BUCKETS
        .iter()
        .enumerate()
        .map(|(i, &(lo, hi))| {
            let last = i == BUCKETS.len() - 1;
            let inside: Vec<&&Judged> = judged
                .iter()
                .filter(|j| {
                    j.confidence
                        .is_some_and(|c| c >= lo && (c < hi || (last && c <= hi)))
                })
                .collect();
            BucketStat {
                lo,
                hi,
                n: inside.len(),
                correct: inside.iter().filter(|j| j.correct).count(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pick {
    pub threshold: f64,
    /// 置信度 ≥ 阈值的那部分里答对的比例。
    pub precision: f64,
    /// 置信度 ≥ 阈值的比例（= Jev 自己执行、不打扰机长的比例）。
    pub coverage: f64,
    pub acted: usize,
}

fn grid(lo: f64, hi: f64) -> Vec<f64> {
    let steps = ((hi - lo) / 0.05).round() as usize;
    (0..=steps)
        .map(|i| ((lo + 0.05 * i as f64) * 100.0).round() / 100.0)
        .collect()
}

/// choice / score：在 0.30–0.90（步长 0.05）里取最小的阈值，使「置信度 ≥ 阈值」的那部分准确率 ≥ target，
/// 且至少有 3 条。没有满足的 → None（这个决策在中文上不该自动执行）。
pub fn recommend_threshold(judged: &[&Judged], target: f64) -> Option<Pick> {
    let scored: Vec<&&Judged> = judged.iter().filter(|j| j.confidence.is_some()).collect();
    if scored.is_empty() {
        return None;
    }
    for t in grid(0.30, 0.90) {
        let acted: Vec<&&&Judged> = scored
            .iter()
            .filter(|j| j.confidence.unwrap_or(0.0) >= t)
            .collect();
        if acted.len() < 3 {
            continue;
        }
        let precision = acted.iter().filter(|j| j.correct).count() as f64 / acted.len() as f64;
        if precision >= target {
            return Some(Pick {
                threshold: t,
                precision,
                coverage: acted.len() as f64 / scored.len() as f64,
                acted: acted.len(),
            });
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WakePick {
    pub threshold: f64,
    /// 每个成员单独判对（p ≥ 阈值 ⇔ 应该叫醒）的比例。
    pub member_accuracy: f64,
    pub judgements: usize,
}

/// wake：在 0.20–0.80 里取让「逐成员判对率」最高的阈值；并列取离 0.5 最近的。
pub fn recommend_wake(judged: &[&Judged]) -> Option<WakePick> {
    let all: Vec<(f64, bool)> = judged
        .iter()
        .flat_map(|j| j.nouls.iter().copied())
        .collect();
    if all.is_empty() {
        return None;
    }
    let mut best: Option<WakePick> = None;
    for t in grid(0.20, 0.80) {
        let right = all.iter().filter(|(p, e)| (*p >= t) == *e).count();
        let acc = right as f64 / all.len() as f64;
        let better = match &best {
            None => true,
            Some(b) => {
                acc > b.member_accuracy + 1e-12
                    || ((acc - b.member_accuracy).abs() <= 1e-12
                        && (t - 0.5).abs() < (b.threshold - 0.5).abs())
            }
        };
        if better {
            best = Some(WakePick {
                threshold: t,
                member_accuracy: acc,
                judgements: all.len(),
            });
        }
    }
    best
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KindSummary {
    pub kind: DecisionKind,
    pub runs: usize,
    pub errors: usize,
    pub correct: usize,
    pub accuracy: f64,
    pub buckets: Vec<BucketStat>,
    pub pick: Option<Pick>,
    pub wake_pick: Option<WakePick>,
    /// 多个种子的用例里，各种子答案完全一致的条数 / 总条数。
    pub consistent: (usize, usize),
    /// choice：选中排第一的次数 / 次数，以及随机情况下的期望比例（平均 1/选项数）。
    pub picked_first: Option<(usize, usize, f64)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    pub target_precision: f64,
    pub kinds: Vec<KindSummary>,
    pub latency_p50_ms: u64,
    pub latency_p95_ms: u64,
}

pub fn percentile_ms(latencies: &[Duration], pct: f64) -> u64 {
    if latencies.is_empty() {
        return 0;
    }
    let mut ms: Vec<u64> = latencies.iter().map(|d| d.as_millis() as u64).collect();
    ms.sort_unstable();
    let rank = ((pct / 100.0) * (ms.len() - 1) as f64).round() as usize;
    ms[rank.min(ms.len() - 1)]
}

pub fn summarize(judged: &[Judged], latencies: &[Duration], target_precision: f64) -> Summary {
    let kinds = DecisionKind::ALL
        .iter()
        .filter_map(|&kind| {
            let runs: Vec<&Judged> = judged.iter().filter(|j| j.kind == kind).collect();
            if runs.is_empty() {
                return None;
            }
            let ok: Vec<&Judged> = runs.iter().copied().filter(|j| j.error.is_none()).collect();
            let correct = ok.iter().filter(|j| j.correct).count();
            let mut by_id: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            let mut seeds: BTreeMap<&str, usize> = BTreeMap::new();
            for j in &ok {
                by_id.entry(&j.id).or_default().insert(&j.answer);
                *seeds.entry(&j.id).or_default() += 1;
            }
            let multi: Vec<&str> = seeds
                .iter()
                .filter(|(_, n)| **n >= 2)
                .map(|(id, _)| *id)
                .collect();
            let consistent = multi.iter().filter(|id| by_id[*id].len() == 1).count();
            let firsts: Vec<&Judged> = ok
                .iter()
                .copied()
                .filter(|j| j.picked_first.is_some())
                .collect();
            let picked_first = (!firsts.is_empty()).then(|| {
                let hits = firsts
                    .iter()
                    .filter(|j| j.picked_first == Some(true))
                    .count();
                let expected = firsts
                    .iter()
                    .map(|j| 1.0 / j.options.max(1) as f64)
                    .sum::<f64>()
                    / firsts.len() as f64;
                (hits, firsts.len(), expected)
            });
            Some(KindSummary {
                kind,
                runs: runs.len(),
                errors: runs.len() - ok.len(),
                correct,
                accuracy: if ok.is_empty() {
                    0.0
                } else {
                    correct as f64 / ok.len() as f64
                },
                buckets: calibration(&ok),
                pick: if kind == DecisionKind::Wake {
                    None
                } else {
                    recommend_threshold(&ok, target_precision)
                },
                wake_pick: if kind == DecisionKind::Wake {
                    recommend_wake(&ok)
                } else {
                    None
                },
                consistent: (consistent, multi.len()),
                picked_first,
            })
        })
        .collect();
    Summary {
        target_precision,
        kinds,
        latency_p50_ms: percentile_ms(latencies, 50.0),
        latency_p95_ms: percentile_ms(latencies, 95.0),
    }
}

impl Summary {
    /// 贴进 spec 的 Markdown。
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "目标准确率 {:.0}%；延迟 p50 {} ms / p95 {} ms（含网络与重试）。\n",
            self.target_precision * 100.0,
            self.latency_p50_ms,
            self.latency_p95_ms
        );
        let _ = writeln!(
            out,
            "| 决策 | 次数 | 出错 | 准确率 | 种子间一致 | 选中排第一（随机期望） | 推荐阈值 |"
        );
        let _ = writeln!(out, "|---|---|---|---|---|---|---|");
        for k in &self.kinds {
            let first = match k.picked_first {
                Some((hits, n, expected)) => format!("{hits}/{n}（{:.0}%）", expected * 100.0),
                None => "—".into(),
            };
            let pick = match (&k.pick, &k.wake_pick) {
                (Some(p), _) => format!(
                    "{:.2}（准确 {:.0}%，自动执行 {:.0}%）",
                    p.threshold,
                    p.precision * 100.0,
                    p.coverage * 100.0
                ),
                (None, Some(w)) => format!(
                    "noul {:.2}（逐成员判对 {:.0}%，{} 次判断）",
                    w.threshold,
                    w.member_accuracy * 100.0,
                    w.judgements
                ),
                (None, None) => "无（达不到目标，全交机长）".into(),
            };
            let _ = writeln!(
                out,
                "| {} | {} | {} | {:.0}% | {}/{} | {} | {} |",
                k.kind.as_str(),
                k.runs,
                k.errors,
                k.accuracy * 100.0,
                k.consistent.0,
                k.consistent.1,
                first,
                pick
            );
        }
        let _ = writeln!(out, "\n按置信度分桶（答对 / 次数）：\n");
        let _ = writeln!(
            out,
            "| 决策 | [0,.25) | [.25,.5) | [.5,.75) | [.75,.9) | [.9,1] |"
        );
        let _ = writeln!(out, "|---|---|---|---|---|---|");
        for k in &self.kinds {
            let cells: Vec<String> = k
                .buckets
                .iter()
                .map(|b| format!("{}/{}", b.correct, b.n))
                .collect();
            let _ = writeln!(out, "| {} | {} |", k.kind.as_str(), cells.join(" | "));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(id: &str, kind: DecisionKind, conf: f64, correct: bool) -> Judged {
        Judged {
            id: id.into(),
            kind,
            seed: 0,
            answer: if correct { "a".into() } else { "b".into() },
            confidence: Some(conf),
            correct,
            picked_first: Some(false),
            options: 4,
            nouls: vec![],
            error: None,
        }
    }

    #[test]
    fn the_dataset_is_well_formed_synthetic_and_balanced() {
        let text = include_str!("../eval/cases.zh.json");
        let ds = Dataset::parse(text).unwrap();
        assert_eq!(ds.cases.len(), 40);
        for kind in DecisionKind::ALL {
            let n = ds.cases.iter().filter(|c| c.kind() == kind).count();
            assert!(n >= 8, "{} 只有 {n} 条", kind.as_str());
        }
        // 仓库公开：只许合成数据（spec §12）。
        for needle in ["/Users/", "@gmail", "@qq.com", "sk-", "ghp_", "BEGIN"] {
            assert!(!text.contains(needle), "数据集里出现了 {needle}");
        }
    }

    #[test]
    fn validation_catches_bad_expectations() {
        let text = r#"{"crews":{"c":{"name":"x","members":[{"key":"captain","name":"机长","role":"机长","recency_rank":1},{"key":"a","name":"甲","role":"做甲","recency_rank":2}]}},
            "cases":[{"decision":"route_todo","id":"r","crew":"c","todo":"做甲","expect":["zzz"]}]}"#;
        let err = Dataset::parse(text).unwrap_err();
        assert!(err.contains("zzz"), "{err}");
        let dup = r#"{"crews":{"c":{"name":"x","members":[{"key":"a","name":"甲","role":"做甲","recency_rank":1}]}},
            "cases":[{"decision":"route_todo","id":"r","crew":"c","todo":"t","expect":["a"]},
                     {"decision":"route_todo","id":"r","crew":"c","todo":"t","expect":["a"]}]}"#;
        assert!(Dataset::parse(dup).unwrap_err().contains("重复"));
    }

    #[test]
    fn calibration_buckets_include_one_in_the_top_bucket() {
        let a = j("a", DecisionKind::RouteTodo, 1.0, true);
        let b = j("b", DecisionKind::RouteTodo, 0.25, false);
        let c = j("c", DecisionKind::RouteTodo, 0.1, true);
        let buckets = calibration(&[&a, &b, &c]);
        assert_eq!((buckets[4].n, buckets[4].correct), (1, 1));
        assert_eq!((buckets[1].n, buckets[1].correct), (1, 0));
        assert_eq!((buckets[0].n, buckets[0].correct), (1, 1));
    }

    #[test]
    fn threshold_is_the_lowest_that_meets_the_target() {
        let runs = [
            j("1", DecisionKind::RouteTodo, 0.95, true),
            j("2", DecisionKind::RouteTodo, 0.9, true),
            j("3", DecisionKind::RouteTodo, 0.8, true),
            j("4", DecisionKind::RouteTodo, 0.6, true),
            j("5", DecisionKind::RouteTodo, 0.4, false),
            j("6", DecisionKind::RouteTodo, 0.35, false),
        ];
        let refs: Vec<&Judged> = runs.iter().collect();
        let pick = recommend_threshold(&refs, 0.95).unwrap();
        assert_eq!(pick.threshold, 0.45);
        assert_eq!(pick.acted, 4);
        assert!((pick.coverage - 4.0 / 6.0).abs() < 1e-9);
        let bad = [
            j("1", DecisionKind::RouteTodo, 0.95, false),
            j("2", DecisionKind::RouteTodo, 0.95, true),
            j("3", DecisionKind::RouteTodo, 0.95, true),
        ];
        let refs: Vec<&Judged> = bad.iter().collect();
        assert_eq!(recommend_threshold(&refs, 0.95), None);
    }

    #[test]
    fn wake_threshold_maximises_member_accuracy_ties_toward_half() {
        let mut a = j("a", DecisionKind::Wake, 0.5, true);
        a.nouls = vec![(0.9, true), (0.62, true), (0.3, false), (0.05, false)];
        let pick = recommend_wake(&[&a]).unwrap();
        assert_eq!(pick.member_accuracy, 1.0);
        assert_eq!(pick.threshold, 0.5, "0.35–0.6 都全对，取离 0.5 最近的");
    }

    #[test]
    fn summary_counts_consistency_and_first_pick_bias() {
        let mut runs = vec![
            j("x", DecisionKind::RouteTodo, 0.9, true),
            j("x", DecisionKind::RouteTodo, 0.9, true),
            j("y", DecisionKind::RouteTodo, 0.9, true),
            j("y", DecisionKind::RouteTodo, 0.9, false),
        ];
        runs[0].picked_first = Some(true);
        let mut failed = j("z", DecisionKind::RouteTodo, 0.0, false);
        failed.error = Some("timeout".into());
        failed.confidence = None;
        runs.push(failed);
        let s = summarize(
            &runs,
            &[Duration::from_millis(900), Duration::from_millis(1200)],
            0.95,
        );
        let k = &s.kinds[0];
        assert_eq!(k.runs, 5);
        assert_eq!(k.errors, 1);
        assert_eq!(k.correct, 3);
        assert_eq!(k.consistent, (1, 2));
        assert_eq!(k.picked_first.map(|(h, n, _)| (h, n)), Some((1, 4)));
        assert!(s.markdown().contains("| route_todo | 5 | 1 | 75% | 1/2 |"));
        assert_eq!(s.latency_p50_ms, 1200);
    }
}
