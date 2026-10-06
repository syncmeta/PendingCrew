//! 四个决策（spec §7.2）的共用部分：输入里的成员、阈值、构造结果 `Built`、判定 `Verdict` / `Outcome`，
//! 以及「代码算好的量 → 词」的分档（jaggedness #2/#3：不给 Jev 日期、时长、计数）。
//! 构造器是纯函数：同一输入 + 同一种子 → 同一请求。不碰网络、不读时钟。
pub mod route;

use crate::error::CallError;
use crate::model::{Answer, Question, Request, Response};
use crate::prompts::VOCAB;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

/// 名单截到最近活跃的前 15 个（spec §7.3「名单截到最近活跃前 ~15」）。
pub const MAX_MEMBERS: usize = 15;
/// 叫醒是每人一道 noul，不挤在一个选项表里，上限放宽，但仍要有界。
pub const MAX_WAKE_MEMBERS: usize = 30;
pub const MAX_TEXT_CHARS: usize = 2000;
pub const MAX_SUMMARY_CHARS: usize = 200;
pub const MAX_WORK_TITLES: usize = 3;
pub const MAX_TITLE_CHARS: usize = 80;
pub const MAX_RECENT: usize = 5;
pub const MAX_CANDIDATES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    RouteTodo,
    Wake,
    Attribute,
    Urgency,
}

impl DecisionKind {
    pub const ALL: [DecisionKind; 4] = [
        DecisionKind::RouteTodo,
        DecisionKind::Wake,
        DecisionKind::Attribute,
        DecisionKind::Urgency,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DecisionKind::RouteTodo => "route_todo",
            DecisionKind::Wake => "wake",
            DecisionKind::Attribute => "attribute",
            DecisionKind::Urgency => "urgency",
        }
    }
}

/// 置信度线。默认值 = spec §7.3 的 0.5；M5a Task 9 实测后按读数改（改了要同步 `defaults_are_pinned`）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    /// choice 的 confidence 低于它 → 交机长。
    pub choice_min: f64,
    /// score 的 confidence 低于它 → 侧边栏退回只按最近活跃排。
    pub score_min: f64,
    /// noul 的 p 不低于它 → 叫醒这个成员。
    pub wake_yes: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            choice_min: 0.5,
            score_min: 0.5,
            wake_yes: 0.5,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome<T> {
    /// 中 / 高置信度：直接执行，群里不留痕（spec §7.1）。
    Act(T),
    /// 交机长按老办法处理（或侧边栏退回代码排序）。
    Defer(DeferReason),
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeferReason {
    /// 没开 / 没 key：等同低置信度（spec §7.1）。
    Unconfigured,
    /// 输入本身不能问（key 不合法、重复、空文本……）。是调用方的错，诊断日志里能看到。
    InvalidInput(String),
    Call(CallError),
    /// 应答缺题、类型不对、选了不存在的选项、置信度越界。
    Malformed(String),
    LowConfidence {
        confidence: f64,
    },
    /// 叫醒集合为空：交机长（spec §7.2）。
    NobodySelected,
}

impl DeferReason {
    pub fn label(&self) -> String {
        match self {
            DeferReason::Unconfigured => "unconfigured".into(),
            DeferReason::InvalidInput(m) => format!("invalid_input: {m}"),
            DeferReason::Call(e) => format!("call: {e}"),
            DeferReason::Malformed(m) => format!("malformed: {m}"),
            DeferReason::LowConfidence { confidence } => {
                format!("low_confidence: {confidence:.3}")
            }
            DeferReason::NobodySelected => "nobody_selected".into(),
        }
    }
}

/// 一次判断的结论。`best` 是模型给出的答案（不论置信度够不够），评测靠它算准确率；
/// 执行只看 `outcome`。
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict<T> {
    pub best: Option<T>,
    pub confidence: Option<f64>,
    pub outcome: Outcome<T>,
}

impl<T> Verdict<T> {
    pub fn defer(reason: DeferReason) -> Verdict<T> {
        Verdict {
            best: None,
            confidence: None,
            outcome: Outcome::Defer(reason),
        }
    }
}

/// 构造好的一次判断：发给 Jev 的 state + questions，以及按发出顺序排的选项名（choice）或题号（noul）。
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub kind: DecisionKind,
    pub state: Value,
    pub questions: BTreeMap<String, Question>,
    pub options: Vec<String>,
}

impl Built {
    pub fn request(&self, model: &str) -> Request {
        Request {
            model: model.to_string(),
            state: self.state.clone(),
            questions: self.questions.clone(),
        }
    }
}

/// 群成员 / 活跃 session 的一行。`key` 是选项名（发给模型看），必须是 `[a-z0-9][a-z0-9_-]{0,31}`；
/// `recency_rank` 从 1 起，1 = 最近活跃，代码算；给 Jev 的只是分档词。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub key: String,
    pub name: String,
    pub role: String,
    #[serde(default)]
    pub work: Vec<String>,
    #[serde(default)]
    pub busy: bool,
    pub recency_rank: usize,
}

pub fn validate_key(key: &str) -> Result<(), String> {
    let bytes = key.as_bytes();
    let ok = !bytes.is_empty()
        && bytes.len() <= 32
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!("选项名不合法：{key:?}"))
    }
}

/// 校验一组选项名：合法、互不重复、不撞保留名。
pub fn check_keys<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    reserved: &[&str],
) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for key in keys {
        validate_key(key)?;
        if reserved.contains(&key) {
            return Err(format!("选项名 {key:?} 是保留名"));
        }
        if !seen.insert(key) {
            return Err(format!("选项名 {key:?} 重复"));
        }
    }
    Ok(())
}

/// 去首尾空白，超过 `max` 个字符截断并补「…」。
pub fn clip(text: &str, max: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let mut out: String = t.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn non_empty(text: &str, what: &str) -> Result<String, String> {
    let t = clip(text, MAX_TEXT_CHARS);
    if t.is_empty() {
        Err(format!("{what}是空的"))
    } else {
        Ok(t)
    }
}

/// 最近活跃排名 → 词。1–3 刚刚、4–8 不久前、其余（含 0 = 不知道）很久。
pub fn recency_word(rank: usize) -> &'static str {
    match rank {
        1..=3 => VOCAB.get("recency.recent"),
        4..=8 => VOCAB.get("recency.middle"),
        _ => VOCAB.get("recency.stale"),
    }
}

/// 已等时长 → 词。< 1h、< 8h、< 36h、更久。
pub fn wait_word(waited: Duration) -> &'static str {
    let mins = waited.as_secs() / 60;
    if mins < 60 {
        VOCAB.get("wait.minutes")
    } else if mins < 8 * 60 {
        VOCAB.get("wait.hours")
    } else if mins < 36 * 60 {
        VOCAB.get("wait.day")
    } else {
        VOCAB.get("wait.days")
    }
}

/// 叫人类的次数 → 词。0、1–2、≥3。
pub fn mentions_word(count: usize) -> &'static str {
    match count {
        0 => VOCAB.get("mentions.none"),
        1 | 2 => VOCAB.get("mentions.some"),
        _ => VOCAB.get("mentions.many"),
    }
}

/// 按最近活跃排序（0 = 不知道，排最后；同名次保持输入顺序），截前 `max` 个。
pub fn most_recent(members: &[Member], max: usize) -> Vec<&Member> {
    let mut sorted: Vec<&Member> = members.iter().collect();
    sorted.sort_by_key(|m| {
        if m.recency_rank == 0 {
            usize::MAX
        } else {
            m.recency_rank
        }
    });
    sorted.truncate(max);
    sorted
}

/// 给模型看的成员描述。字段名是英文结构名，值是数据或 vocab 里的词。
pub fn member_view(m: &Member) -> Value {
    let work: Vec<String> = m
        .work
        .iter()
        .map(|w| clip(w, MAX_TITLE_CHARS))
        .filter(|w| !w.is_empty())
        .take(MAX_WORK_TITLES)
        .collect();
    json!({
        "name": clip(&m.name, MAX_TITLE_CHARS),
        "role": clip(&m.role, MAX_SUMMARY_CHARS),
        "work": work,
        "status": if m.busy { VOCAB.get("status.busy") } else { VOCAB.get("status.idle") },
        "activity": recency_word(m.recency_rank),
    })
}

/// 取一道 choice 的答案并核对：选项必须是我们发出去的，置信度必须在 [0,1]。
pub fn read_choice(
    built: &Built,
    response: &Response,
    question: &str,
) -> Result<(String, f64), DeferReason> {
    match response.answers.get(question) {
        Some(Answer::Choice {
            choice, confidence, ..
        }) => {
            if !built.options.iter().any(|o| o == choice) {
                return Err(DeferReason::Malformed(format!(
                    "选了没发出的选项 {choice:?}"
                )));
            }
            check_unit(*confidence)?;
            Ok((choice.clone(), *confidence))
        }
        Some(other) => Err(DeferReason::Malformed(format!(
            "{question} 的答案类型不对：{other:?}"
        ))),
        None => Err(DeferReason::Malformed(format!("应答里没有 {question}"))),
    }
}

pub fn check_unit(x: f64) -> Result<(), DeferReason> {
    if (0.0..=1.0).contains(&x) {
        Ok(())
    } else {
        Err(DeferReason::Malformed(format!("越界的概率 / 置信度 {x}")))
    }
}

/// 置信度够线就执行，否则交机长；`best` 无论如何都留着给评测。
pub fn gate<T: Clone>(best: T, confidence: f64, min: f64) -> Verdict<T> {
    let outcome = if confidence >= min {
        Outcome::Act(best.clone())
    } else {
        Outcome::Defer(DeferReason::LowConfidence { confidence })
    };
    Verdict {
        best: Some(best),
        confidence: Some(confidence),
        outcome,
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::Member;

    pub fn member(key: &str, rank: usize) -> Member {
        Member {
            key: key.into(),
            name: format!("成员{key}"),
            role: format!("负责 {key}"),
            work: vec![format!("{key} 的工作")],
            busy: false,
            recency_rank: rank,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::member;
    use super::*;

    #[test]
    fn defaults_are_pinned() {
        assert_eq!(
            Thresholds::default(),
            Thresholds {
                choice_min: 0.5,
                score_min: 0.5,
                wake_yes: 0.5
            }
        );
    }

    #[test]
    fn keys_must_be_ascii_slugs_unique_and_not_reserved() {
        assert!(validate_key("scroll-fix").is_ok());
        assert!(validate_key("w3_b").is_ok());
        assert!(validate_key("").is_err());
        assert!(validate_key("Scroll").is_err());
        assert!(validate_key("-x").is_err());
        assert!(validate_key("滚动").is_err());
        assert!(validate_key(&"a".repeat(33)).is_err());
        assert!(check_keys(["a", "b"], &["captain"]).is_ok());
        assert!(check_keys(["a", "a"], &[]).is_err());
        assert!(check_keys(["captain"], &["captain"]).is_err());
    }

    #[test]
    fn clip_counts_characters_not_bytes() {
        assert_eq!(clip("  修滚动  ", 10), "修滚动");
        assert_eq!(clip("一二三四五", 3), "一二…");
        assert_eq!(clip("abc", 3), "abc");
    }

    #[test]
    fn quantities_become_words_without_digits() {
        assert_eq!(recency_word(1), VOCAB.get("recency.recent"));
        assert_eq!(recency_word(3), VOCAB.get("recency.recent"));
        assert_eq!(recency_word(4), VOCAB.get("recency.middle"));
        assert_eq!(recency_word(8), VOCAB.get("recency.middle"));
        assert_eq!(recency_word(9), VOCAB.get("recency.stale"));
        assert_eq!(recency_word(0), VOCAB.get("recency.stale"));
        let m = Duration::from_secs(60);
        assert_eq!(wait_word(m * 59), VOCAB.get("wait.minutes"));
        assert_eq!(wait_word(m * 60), VOCAB.get("wait.hours"));
        assert_eq!(wait_word(m * 479), VOCAB.get("wait.hours"));
        assert_eq!(wait_word(m * 480), VOCAB.get("wait.day"));
        assert_eq!(wait_word(m * 36 * 60), VOCAB.get("wait.days"));
        assert_eq!(mentions_word(0), VOCAB.get("mentions.none"));
        assert_eq!(mentions_word(2), VOCAB.get("mentions.some"));
        assert_eq!(mentions_word(3), VOCAB.get("mentions.many"));
    }

    #[test]
    fn roster_is_cut_to_the_most_recent_and_unknown_rank_goes_last() {
        let mut members: Vec<Member> = (1..=20)
            .rev()
            .map(|r| member(&format!("m{r}"), r))
            .collect();
        members.push(member("unknown", 0));
        let top = most_recent(&members, MAX_MEMBERS);
        assert_eq!(top.len(), 15);
        assert_eq!(top[0].key, "m1");
        assert_eq!(top[14].key, "m15");
        assert!(top.iter().all(|m| m.key != "unknown"));
    }

    #[test]
    fn member_view_has_no_raw_numbers() {
        let mut m = member("a", 2);
        m.work = vec!["一".into(), "二".into(), "三".into(), "四".into()];
        m.busy = true;
        let v = member_view(&m);
        assert_eq!(v["work"].as_array().unwrap().len(), MAX_WORK_TITLES);
        assert_eq!(v["status"], VOCAB.get("status.busy"));
        assert_eq!(v["activity"], VOCAB.get("recency.recent"));
        assert!(!v.to_string().contains("recency_rank"));
    }
}
