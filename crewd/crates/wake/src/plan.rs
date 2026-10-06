//! 唤醒规划（spec §22.4「唤醒规划」+ §22.7 Q2）：一条新消息 → 该叫醒谁、什么优先级。纯函数。
//! 移植 `Sources/Mac/LocalRunner/CrewLocalMentionWakeLogic.swift:58-103`：
//! - 只在群成员里选（§17）；自己不叫自己；超过 6h 的不叫（`:53`）；broadcast / @人类 不叫任何 session（`:74`）。
//! - 人发、一个 @ 都没有：先用调度层（Jev，候选 = 群成员）的答案；没有 / 低置信 → 群主（Q2；Swift 是「默认 @机长」，
//!   大厅与人类群的群主就是机长，语义不变）。
//! - 收听者（`listens`，窗口内、发送者匹配）与上面取并集；同一条消息一个 session 只出现一次。
use crate::model::{HOUR, MessageRef, Ms, Priority};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sender {
    Human,
    System,
    Session(String),
}

impl Sender {
    /// 收听过滤与环路熔断用的来源键：`human` / `system` / session id。
    pub fn key(&self) -> &str {
        match self {
            Self::Human => "human",
            Self::System => "system",
            Self::Session(id) => id,
        }
    }
}

/// 发送时已解析好的 @（`captain` 在发送时已换成具体 session id，§22.2）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mention {
    Session(String),
    Broadcast,
    Human,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MsgFacts {
    /// 消息 id（唤醒的 source 用它，`source::message`）。
    pub id: String,
    pub at: MessageRef,
    pub sender: Sender,
    /// 空 = 一个 @ 都没写。
    pub mentions: Vec<Mention>,
    pub created_at: Ms,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listen {
    pub session: String,
    /// 收听到这一刻为止（不含）。
    pub until: Ms,
    /// `None` = 谁发的都算；否则元素是 [`Sender::key`]。
    pub senders: Option<Vec<String>>,
}

/// 消息所在群此刻的情况（由 IO 层按 M3a 的群规则算好：大厅 = 本机组全部 session，人类群 = 根机组机长 + 显式成员）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelCtx {
    pub session_members: Vec<String>,
    /// 群主（§22.7 Q2；大厅 / 人类群 = 机组机长；人建的群可能不是 session）。
    pub owner: Option<String>,
    pub listens: Vec<Listen>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WakeTarget {
    pub session: String,
    pub priority: Priority,
}

pub const MAX_WAKE_AGE_MS: i64 = 6 * HOUR;

pub fn priority_of(sender: &Sender) -> Priority {
    match sender {
        Sender::Human => Priority::Human,
        Sender::System => Priority::System,
        Sender::Session(_) => Priority::Auto,
    }
}

pub fn is_stale(msg: &MsgFacts, now: Ms) -> bool {
    now.since(msg.created_at) > MAX_WAKE_AGE_MS
}

/// 要不要先问调度层（Arbiter）：人发的、一个 @ 都没有、不陈旧。
pub fn needs_arbiter(msg: &MsgFacts, now: Ms) -> bool {
    msg.sender == Sender::Human && msg.mentions.is_empty() && !is_stale(msg, now)
}

/// 人发、没有 @、调度层没给可用答案时叫谁（§22.7 Q2：群主）。改兜底规则只改这里。
pub fn untargeted_human(ctx: &ChannelCtx) -> Vec<String> {
    ctx.owner
        .iter()
        .filter(|o| ctx.session_members.contains(o))
        .cloned()
        .collect()
}

/// `arbiter`：调度层给的叫醒集合（`None` = 没问 / 没答 / 失败；空集 = 低置信）。
pub fn plan(
    msg: &MsgFacts,
    ctx: &ChannelCtx,
    arbiter: Option<&[String]>,
    now: Ms,
) -> Vec<WakeTarget> {
    if is_stale(msg, now) {
        return vec![];
    }
    let is_member = |s: &str| ctx.session_members.iter().any(|m| m == s);
    let sender_session = match &msg.sender {
        Sender::Session(id) => Some(id.as_str()),
        _ => None,
    };
    let mut out: Vec<String> = Vec::new();
    let push = |s: &str, out: &mut Vec<String>| {
        if Some(s) != sender_session && is_member(s) && !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    };
    for m in &msg.mentions {
        if let Mention::Session(id) = m {
            push(id, &mut out);
        }
    }
    if msg.sender == Sender::Human && msg.mentions.is_empty() {
        let chosen: Vec<String> = arbiter
            .map(|a| a.iter().filter(|s| is_member(s)).cloned().collect())
            .unwrap_or_default();
        let chosen = if chosen.is_empty() {
            untargeted_human(ctx)
        } else {
            chosen
        };
        for s in &chosen {
            push(s, &mut out);
        }
    }
    for l in &ctx.listens {
        let sender_ok = l
            .senders
            .as_ref()
            .is_none_or(|ss| ss.iter().any(|s| s == msg.sender.key()));
        if l.until > now && sender_ok {
            push(&l.session, &mut out);
        }
    }
    let priority = priority_of(&msg.sender);
    out.into_iter()
        .map(|session| WakeTarget { session, priority })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MIN, MessageRef};

    const NOW: Ms = Ms(10 * HOUR);

    fn msg(sender: Sender, mentions: Vec<Mention>) -> MsgFacts {
        MsgFacts {
            id: "m-7".into(),
            at: MessageRef {
                channel: "c1/lobby".into(),
                seq: 7,
            },
            sender,
            mentions,
            created_at: NOW,
        }
    }

    fn ctx(members: &[&str], owner: Option<&str>) -> ChannelCtx {
        ChannelCtx {
            session_members: members.iter().map(|s| s.to_string()).collect(),
            owner: owner.map(str::to_string),
            listens: vec![],
        }
    }

    fn who(targets: &[WakeTarget]) -> Vec<&str> {
        targets.iter().map(|t| t.session.as_str()).collect()
    }

    fn s(id: &str) -> Sender {
        Sender::Session(id.into())
    }

    fn at(id: &str) -> Mention {
        Mention::Session(id.into())
    }

    #[test]
    fn mention_wakes_the_member_with_the_senders_priority() {
        let c = ctx(&["cap", "w1", "w2"], Some("cap"));
        let t = plan(&msg(s("cap"), vec![at("w1")]), &c, None, NOW);
        assert_eq!(
            t,
            vec![WakeTarget {
                session: "w1".into(),
                priority: Priority::Auto
            }]
        );
        let t = plan(&msg(Sender::Human, vec![at("w2")]), &c, None, NOW);
        assert_eq!(t[0].priority, Priority::Human);
        let t = plan(&msg(Sender::System, vec![at("w2")]), &c, None, NOW);
        assert_eq!(t[0].priority, Priority::System);
    }

    #[test]
    fn captain_mention_plus_reply_to_the_captain_wakes_it_once() {
        // M3a：显式 @机长 + 回复机长 = 消息上有 `Captain(cap)` 与 `Session(cap)` 两条 mention（db.rs 都换成
        // `Mention::Session("cap")`）；同一条消息一个 session 只叫一次。
        let c = ctx(&["cap", "w1"], Some("cap"));
        let t = plan(&msg(s("w1"), vec![at("cap"), at("cap")]), &c, None, NOW);
        assert_eq!(who(&t), ["cap"]);
    }

    #[test]
    fn never_wakes_the_sender_or_a_non_member() {
        let c = ctx(&["cap", "w1"], Some("cap"));
        assert!(plan(&msg(s("w1"), vec![at("w1")]), &c, None, NOW).is_empty());
        assert!(plan(&msg(s("cap"), vec![at("outsider")]), &c, None, NOW).is_empty());
    }

    #[test]
    fn broadcast_and_human_mentions_wake_nobody_even_from_a_human() {
        let c = ctx(&["cap", "w1"], Some("cap"));
        assert!(plan(&msg(s("w1"), vec![Mention::Broadcast]), &c, None, NOW).is_empty());
        assert!(plan(&msg(s("w1"), vec![Mention::Human]), &c, None, NOW).is_empty());
        // 人显式写了 broadcast：不是「没 @ 任何人」，不交群主。
        assert!(plan(&msg(Sender::Human, vec![Mention::Broadcast]), &c, None, NOW).is_empty());
    }

    #[test]
    fn untargeted_human_goes_to_jev_then_to_the_owner() {
        let c = ctx(&["cap", "w1", "w2"], Some("cap"));
        let m = msg(Sender::Human, vec![]);
        assert_eq!(who(&plan(&m, &c, None, NOW)), ["cap"], "Jev 未接入 → 群主");
        let jev = ["w2".to_string()];
        assert_eq!(who(&plan(&m, &c, Some(&jev), NOW)), ["w2"]);
        let outsider = ["zz".to_string()];
        assert_eq!(
            who(&plan(&m, &c, Some(&outsider), NOW)),
            ["cap"],
            "Jev 选了非成员 → 当没答"
        );
        assert_eq!(
            who(&plan(&m, &c, Some(&[]), NOW)),
            ["cap"],
            "空集 = 低置信 → 群主"
        );
    }

    #[test]
    fn owner_that_is_not_a_session_member_wakes_nobody() {
        // 群主是人 / 已离开：没有 session 可叫，只剩收听者。
        let m = msg(Sender::Human, vec![]);
        assert!(plan(&m, &ctx(&["w1", "w2"], Some("gone")), None, NOW).is_empty());
        assert!(plan(&m, &ctx(&["w1", "w2"], None), None, NOW).is_empty());
    }

    #[test]
    fn session_messages_without_mentions_wake_only_listeners() {
        let mut c = ctx(&["cap", "w1", "w2"], Some("cap"));
        assert!(plan(&msg(s("w1"), vec![]), &c, None, NOW).is_empty());
        c.listens.push(Listen {
            session: "w2".into(),
            until: NOW.add(MIN),
            senders: None,
        });
        assert_eq!(who(&plan(&msg(s("w1"), vec![]), &c, None, NOW)), ["w2"]);
    }

    #[test]
    fn listeners_are_unioned_once_and_respect_window_and_sender_filter() {
        let mut c = ctx(&["cap", "w1", "w2", "w3"], Some("cap"));
        c.listens = vec![
            Listen {
                session: "w1".into(),
                until: NOW.add(MIN),
                senders: None,
            },
            Listen {
                session: "w2".into(),
                until: NOW,
                senders: None,
            },
            Listen {
                session: "w3".into(),
                until: NOW.add(MIN),
                senders: Some(vec!["human".into()]),
            },
        ];
        // w1 既被 @ 又在收听：只出现一次；w2 窗口已过；w3 只收听人。
        assert_eq!(
            who(&plan(&msg(s("cap"), vec![at("w1")]), &c, None, NOW)),
            ["w1"]
        );
        assert_eq!(
            who(&plan(&msg(Sender::Human, vec![at("w1")]), &c, None, NOW)),
            ["w1", "w3"]
        );
        // 人不 @：群主 + 收听者并集。
        assert_eq!(
            who(&plan(&msg(Sender::Human, vec![]), &c, None, NOW)),
            ["cap", "w1", "w3"]
        );
    }

    #[test]
    fn listener_that_is_the_sender_or_not_a_member_is_skipped() {
        let mut c = ctx(&["cap", "w1"], Some("cap"));
        c.listens = vec![
            Listen {
                session: "w1".into(),
                until: NOW.add(MIN),
                senders: None,
            },
            Listen {
                session: "ghost".into(),
                until: NOW.add(MIN),
                senders: None,
            },
        ];
        assert!(plan(&msg(s("w1"), vec![]), &c, None, NOW).is_empty());
    }

    #[test]
    fn messages_older_than_six_hours_wake_nobody() {
        let c = ctx(&["cap", "w1"], Some("cap"));
        let mut m = msg(s("cap"), vec![at("w1")]);
        m.created_at = NOW.add(-MAX_WAKE_AGE_MS - 1);
        assert!(plan(&m, &c, None, NOW).is_empty());
        m.created_at = NOW.add(-MAX_WAKE_AGE_MS);
        assert_eq!(plan(&m, &c, None, NOW).len(), 1, "正好 6h 还叫");
    }

    #[test]
    fn arbiter_is_asked_only_for_fresh_untargeted_human_messages() {
        assert!(needs_arbiter(&msg(Sender::Human, vec![]), NOW));
        assert!(!needs_arbiter(&msg(Sender::Human, vec![at("w1")]), NOW));
        assert!(!needs_arbiter(
            &msg(Sender::Human, vec![Mention::Broadcast]),
            NOW
        ));
        assert!(!needs_arbiter(&msg(s("w1"), vec![]), NOW));
        assert!(!needs_arbiter(&msg(Sender::System, vec![]), NOW));
        let mut old = msg(Sender::Human, vec![]);
        old.created_at = NOW.add(-MAX_WAKE_AGE_MS - 1);
        assert!(!needs_arbiter(&old, NOW));
    }
}
