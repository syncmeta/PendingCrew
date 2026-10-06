//! 唤醒规划（spec §22.4「唤醒规划」+ §22.7 Q2）：一条新消息 → 该叫醒谁、什么优先级。纯函数。
//! 移植 `Sources/Mac/LocalRunner/CrewLocalMentionWakeLogic.swift:58-103`：
//! - 被 @ 的只在群成员里选（§17）；自己不叫自己；超过 6h 的不叫（`:53`）；broadcast / @人类 本身不叫任何 session（`:74`）。
//! - 人发、没有指向任何成员 session（一个 @ 都没有，或 @ 全是 broadcast / 人类 / 已离开的 session）：先用调度层
//!   （Jev，候选 = 群成员）的答案；没有 / 低置信 → 兜底链 [`untargeted_human`]（群主 → dm 的另一方 → 机组机长；Q2）。
//! - 收听者（`listens`，窗口内、发送者匹配）与上面取并集，**不受成员资格限制**（M3a：session 可以收听自己不在的群），
//!   只要求：是活 session、不是发送者、这条消息对它可见（[`visible_to`]，收窄给别人的消息不叫它）。
//! - 同一条消息一个 session 只出现一次，保留最高的一档；只因收听被叫的是 `Auto`（观察者不继承人类优先级）。
use crate::model::{HOUR, MessageRef, Ms, Priority};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sender {
    Human,
    System,
    /// `captain`：发送时的身份是机长（M3a `SenderKind::Captain`，按发帖时记下的为准）。
    Session {
        id: String,
        captain: bool,
    },
}

impl Sender {
    /// 环路熔断用的来源键：`human` / `system` / session id。
    pub fn key(&self) -> &str {
        match self {
            Self::Human => "human",
            Self::System => "system",
            Self::Session { id, .. } => id,
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Session { id, .. } => Some(id),
            _ => None,
        }
    }

    /// 收听过滤里的一项是否匹配这个发送者：`human` / `system` / `captain`（发送时是机长的 session）/ session id。
    pub fn matches_filter(&self, filter: &str) -> bool {
        match self {
            Self::Human => filter == "human",
            Self::System => filter == "system",
            Self::Session { id, captain } => filter == id || (*captain && filter == "captain"),
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
    /// `None` = 谁发的都算；否则元素见 [`Sender::matches_filter`]。
    pub senders: Option<Vec<String>>,
}

/// 消息所在群此刻的情况（由 IO 层按 M3a 的群规则算好：大厅 = 本机组全部 session，人类群 = 根机组机长 + 显式成员）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelCtx {
    /// 群里的 session 成员：@ 与兜底只在这里选。
    pub session_members: Vec<String>,
    /// 此刻活着的 session（含不在这个群里的）：收听者只要求在这里。
    pub live_sessions: Vec<String>,
    /// 群主（§22.7 Q2；大厅 / 人类群 = 机组机长；人建的群可能不是 session）。
    pub owner: Option<String>,
    /// dm 里的 session 一方（非 dm / 对方不是 session = `None`）。
    pub dm_peer: Option<String>,
    /// 这个群所属机组的机长。
    pub crew_captain: Option<String>,
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
        Sender::Session { .. } => Priority::Auto,
    }
}

pub fn is_stale(msg: &MsgFacts, now: Ms) -> bool {
    now.since(msg.created_at) > MAX_WAKE_AGE_MS
}

/// 这条消息对 `session` 可见吗（移植 `crewd_collab::cursors::visible_to`，同一套收窄规则）：
/// 带 broadcast → 人人可见；没有收窄型 mention（只有 @人类 也算没有）或它就是发送者 → 可见；否则只有被点名的可见。
pub fn visible_to(msg: &MsgFacts, session: &str) -> bool {
    if msg.mentions.iter().any(|m| matches!(m, Mention::Broadcast)) {
        return true;
    }
    let narrowing: Vec<&str> = msg
        .mentions
        .iter()
        .filter_map(|m| match m {
            Mention::Session(id) => Some(id.as_str()),
            _ => None,
        })
        .collect();
    narrowing.is_empty() || msg.sender.session_id() == Some(session) || narrowing.contains(&session)
}

/// 消息 @ 到的成员 session（发送者自己除外；去重）。人发的消息里它为空 = 没指向谁。
fn directed_members(msg: &MsgFacts, ctx: &ChannelCtx) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in &msg.mentions {
        if let Mention::Session(id) = m
            && ctx.session_members.contains(id)
            && Some(id.as_str()) != msg.sender.session_id()
            && !out.contains(id)
        {
            out.push(id.clone());
        }
    }
    out
}

/// 要不要先问调度层（Arbiter）：人发的、没有指向任何成员 session、不陈旧。
pub fn needs_arbiter(msg: &MsgFacts, ctx: &ChannelCtx, now: Ms) -> bool {
    msg.sender == Sender::Human && directed_members(msg, ctx).is_empty() && !is_stale(msg, now)
}

/// 人发、没有指向、调度层没给可用答案时叫谁（§22.7 Q2）：群主（须是成员）→ dm 的另一方 session →
/// 机组机长（须是成员）→ 没人。只取第一个能用的。改兜底规则只改这里。
pub fn untargeted_human(ctx: &ChannelCtx) -> Vec<String> {
    [&ctx.owner, &ctx.dm_peer, &ctx.crew_captain]
        .into_iter()
        .flatten()
        .find(|s| ctx.session_members.contains(s))
        .into_iter()
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
    let sender_session = msg.sender.session_id();
    let mut out: Vec<WakeTarget> = Vec::new();
    // 自己不叫自己；已经在名单里的保留先到的那档（点名先于收听）。
    let push = |s: &str, priority: Priority, out: &mut Vec<WakeTarget>| {
        if Some(s) != sender_session && !out.iter().any(|t| t.session == s) {
            out.push(WakeTarget {
                session: s.to_string(),
                priority,
            });
        }
    };
    let priority = priority_of(&msg.sender);
    let directed = directed_members(msg, ctx);
    for s in &directed {
        push(s, priority, &mut out);
    }
    if msg.sender == Sender::Human && directed.is_empty() {
        let chosen: Vec<String> = arbiter
            .map(|a| a.iter().filter(|s| is_member(s)).cloned().collect())
            .unwrap_or_default();
        let chosen = if chosen.is_empty() {
            untargeted_human(ctx)
        } else {
            chosen
        };
        for s in &chosen {
            push(s, priority, &mut out);
        }
    }
    for l in &ctx.listens {
        let sender_ok = l
            .senders
            .as_ref()
            .is_none_or(|ss| ss.iter().any(|f| msg.sender.matches_filter(f)));
        let live = ctx.live_sessions.iter().any(|s| s == &l.session);
        if l.until > now && sender_ok && live && visible_to(msg, &l.session) {
            push(&l.session, Priority::Auto, &mut out);
        }
    }
    out
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

    /// 成员 = 活着的 session；`live` 之外还能再加不在群里的活 session。
    fn ctx(members: &[&str], owner: Option<&str>) -> ChannelCtx {
        ChannelCtx {
            session_members: members.iter().map(|s| s.to_string()).collect(),
            live_sessions: members.iter().map(|s| s.to_string()).collect(),
            owner: owner.map(str::to_string),
            dm_peer: None,
            crew_captain: None,
            listens: vec![],
        }
    }

    fn who(targets: &[WakeTarget]) -> Vec<&str> {
        targets.iter().map(|t| t.session.as_str()).collect()
    }

    fn s(id: &str) -> Sender {
        Sender::Session {
            id: id.into(),
            captain: false,
        }
    }

    fn captain(id: &str) -> Sender {
        Sender::Session {
            id: id.into(),
            captain: true,
        }
    }

    fn at(id: &str) -> Mention {
        Mention::Session(id.into())
    }

    fn listen(session: &str, until: Ms, senders: Option<&[&str]>) -> Listen {
        Listen {
            session: session.into(),
            until,
            senders: senders.map(|ss| ss.iter().map(|s| s.to_string()).collect()),
        }
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
        // 活着但不在这个群里的 session，被 @ 也不叫（§17：@ 只在群成员里选）。
        let mut c2 = ctx(&["cap", "w1"], Some("cap"));
        c2.live_sessions.push("elsewhere".into());
        assert!(plan(&msg(s("cap"), vec![at("elsewhere")]), &c2, None, NOW).is_empty());
    }

    #[test]
    fn broadcast_and_human_mentions_from_a_session_wake_nobody() {
        let c = ctx(&["cap", "w1"], Some("cap"));
        assert!(plan(&msg(s("w1"), vec![Mention::Broadcast]), &c, None, NOW).is_empty());
        assert!(plan(&msg(s("w1"), vec![Mention::Human]), &c, None, NOW).is_empty());
        assert!(
            plan(
                &msg(s("w1"), vec![Mention::Broadcast, Mention::Human]),
                &c,
                None,
                NOW
            )
            .is_empty()
        );
        assert!(
            plan(
                &msg(Sender::System, vec![Mention::Broadcast]),
                &c,
                None,
                NOW
            )
            .is_empty()
        );
    }

    #[test]
    fn human_message_whose_mentions_resolve_to_no_member_falls_through_to_the_untargeted_rules() {
        // Option A：reply 派生的 `[Broadcast]` / `[Broadcast, Human]`、回复了已离开的 session、
        // 显式 @broadcast、@人类、@非成员 —— 一个成员 session 都没点到，等于没指向谁。
        let c = ctx(&["cap", "w1", "w2"], Some("cap"));
        for mentions in [
            vec![Mention::Broadcast],
            vec![Mention::Broadcast, Mention::Human],
            vec![Mention::Human],
            vec![at("left")],
            vec![at("left"), Mention::Broadcast],
        ] {
            let m = msg(Sender::Human, mentions.clone());
            assert_eq!(who(&plan(&m, &c, None, NOW)), ["cap"], "{mentions:?}");
            assert!(needs_arbiter(&m, &c, NOW), "{mentions:?}");
            let jev = ["w2".to_string()];
            assert_eq!(who(&plan(&m, &c, Some(&jev), NOW)), ["w2"], "{mentions:?}");
        }
        // 点到了成员就是有指向：不走兜底、不问 Jev。
        let m = msg(Sender::Human, vec![Mention::Broadcast, at("w1")]);
        assert_eq!(who(&plan(&m, &c, None, NOW)), ["w1"]);
        assert!(!needs_arbiter(&m, &c, NOW));
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
    fn a_partial_arbiter_answer_keeps_only_the_members() {
        let c = ctx(&["cap", "w1", "w2"], Some("cap"));
        let m = msg(Sender::Human, vec![]);
        let partial = ["outsider".to_string(), "w2".to_string()];
        assert_eq!(
            who(&plan(&m, &c, Some(&partial), NOW)),
            ["w2"],
            "不退回群主"
        );
    }

    #[test]
    fn untargeted_human_fallback_chain_is_owner_then_dm_peer_then_crew_captain() {
        let mut c = ctx(&["cap", "w1", "w2"], Some("w1"));
        c.dm_peer = Some("w2".into());
        c.crew_captain = Some("cap".into());
        assert_eq!(untargeted_human(&c), ["w1"], "群主是成员 → 群主");
        c.owner = Some("gone".into());
        assert_eq!(
            untargeted_human(&c),
            ["w2"],
            "群主不是成员 → dm 的另一方 session"
        );
        c.owner = None;
        assert_eq!(untargeted_human(&c), ["w2"]);
        c.dm_peer = Some("gone".into());
        assert_eq!(untargeted_human(&c), ["cap"], "dm 对方不是成员 → 机组机长");
        c.dm_peer = None;
        assert_eq!(untargeted_human(&c), ["cap"]);
        c.crew_captain = Some("gone".into());
        assert!(untargeted_human(&c).is_empty(), "机长也不是成员 → 没人");
        c.crew_captain = None;
        assert!(untargeted_human(&c).is_empty());
        // 链只取第一个能用的，不叠加。
        c.owner = Some("w1".into());
        c.dm_peer = Some("w2".into());
        c.crew_captain = Some("cap".into());
        assert_eq!(untargeted_human(&c), ["w1"]);
    }

    #[test]
    fn plan_uses_the_fallback_chain_for_an_untargeted_human_message() {
        let mut c = ctx(&["cap", "w2"], None);
        c.dm_peer = Some("w2".into());
        c.crew_captain = Some("cap".into());
        let m = msg(Sender::Human, vec![]);
        assert_eq!(who(&plan(&m, &c, None, NOW)), ["w2"]);
        c.dm_peer = None;
        assert_eq!(who(&plan(&m, &c, None, NOW)), ["cap"]);
        c.crew_captain = None;
        assert!(
            plan(&m, &c, None, NOW).is_empty(),
            "群主是人 / 已离开、没有 dm、没有机长：只剩收听者"
        );
    }

    #[test]
    fn session_messages_without_mentions_wake_only_listeners() {
        let mut c = ctx(&["cap", "w1", "w2"], Some("cap"));
        assert!(plan(&msg(s("w1"), vec![]), &c, None, NOW).is_empty());
        c.listens.push(listen("w2", NOW.add(MIN), None));
        assert_eq!(who(&plan(&msg(s("w1"), vec![]), &c, None, NOW)), ["w2"]);
    }

    #[test]
    fn listeners_are_unioned_once_and_respect_window_and_sender_filter() {
        let mut c = ctx(&["cap", "w1", "w2", "w3"], Some("cap"));
        c.listens = vec![
            listen("w1", NOW.add(MIN), None),
            listen("w2", NOW, None),
            listen("w3", NOW.add(MIN), Some(&["human"])),
        ];
        // w1 既被 @ 又在收听：只出现一次；w2 窗口已过；w3 只收听人（但这条 @w1 对 w3 不可见，见下一条）。
        assert_eq!(
            who(&plan(&msg(s("cap"), vec![at("w1")]), &c, None, NOW)),
            ["w1"]
        );
        // 人 @w1：w3 收听人，但消息收窄给了 w1 → w3 看不见，不叫。
        assert_eq!(
            who(&plan(&msg(Sender::Human, vec![at("w1")]), &c, None, NOW)),
            ["w1"]
        );
        // 人不 @：群主 + 收听者并集。
        assert_eq!(
            who(&plan(&msg(Sender::Human, vec![]), &c, None, NOW)),
            ["cap", "w1", "w3"]
        );
    }

    #[test]
    fn sender_filter_matches_human_system_captain_and_a_session_id() {
        let mut c = ctx(&["cap", "w1", "w2", "w3", "w4"], Some("cap"));
        c.listens = vec![
            listen("w1", NOW.add(MIN), Some(&["captain"])),
            listen("w2", NOW.add(MIN), Some(&["system"])),
            listen("w3", NOW.add(MIN), Some(&["human", "w4"])),
        ];
        assert_eq!(
            who(&plan(&msg(captain("cap"), vec![]), &c, None, NOW)),
            ["w1"]
        );
        assert!(
            plan(&msg(s("cap"), vec![]), &c, None, NOW).is_empty(),
            "普通 session 不是 captain"
        );
        assert_eq!(
            who(&plan(&msg(Sender::System, vec![]), &c, None, NOW)),
            ["w2"]
        );
        assert_eq!(
            who(&plan(&msg(s("w4"), vec![]), &c, None, NOW)),
            ["w3"],
            "按 session id 匹配"
        );
        assert_eq!(
            who(&plan(&msg(Sender::Human, vec![at("w3")]), &c, None, NOW)),
            ["w3"]
        );
    }

    #[test]
    fn listeners_are_not_gated_by_membership_but_by_being_live_not_the_sender_and_seeing_the_message()
     {
        let mut c = ctx(&["cap", "w1"], Some("cap"));
        c.live_sessions
            .extend(["elsewhere".to_string(), "other".to_string()]);
        c.listens = vec![
            listen("w1", NOW.add(MIN), None),
            listen("elsewhere", NOW.add(MIN), None), // 活着、不在群里：被叫
            listen("ghost", NOW.add(MIN), None),     // 不是活 session：不叫
        ];
        assert_eq!(
            who(&plan(&msg(s("cap"), vec![]), &c, None, NOW)),
            ["w1", "elsewhere"]
        );
        // 发送者自己在收听：不叫。
        assert_eq!(
            who(&plan(&msg(s("w1"), vec![]), &c, None, NOW)),
            ["elsewhere"]
        );
        // 收窄给 cap 的消息：别的收听者看不见，不叫。
        assert!(
            plan(&msg(s("w1"), vec![at("cap")]), &c, None, NOW)
                .iter()
                .all(|t| t.session == "cap")
        );
        assert_eq!(
            who(&plan(&msg(s("w1"), vec![at("cap")]), &c, None, NOW)),
            ["cap"]
        );
        // 带 broadcast 的消息对所有人可见：收听者照叫。
        assert_eq!(
            who(&plan(
                &msg(s("cap"), vec![at("w1"), Mention::Broadcast]),
                &c,
                None,
                NOW
            )),
            ["w1", "elsewhere"]
        );
    }

    #[test]
    fn visible_to_follows_the_narrowing_rules() {
        let m = msg(s("w1"), vec![at("w2")]);
        assert!(visible_to(&m, "w2"));
        assert!(visible_to(&m, "w1"), "发送者自己看得见");
        assert!(!visible_to(&m, "w3"));
        assert!(visible_to(&msg(s("w1"), vec![]), "w3"));
        assert!(
            visible_to(&msg(s("w1"), vec![Mention::Human]), "w3"),
            "只 @人类 不收窄"
        );
        assert!(visible_to(
            &msg(s("w1"), vec![at("w2"), Mention::Broadcast]),
            "w3"
        ));
    }

    #[test]
    fn listener_only_wakes_are_auto_priority_even_from_a_human_or_system() {
        let mut c = ctx(&["cap", "w1", "w2"], Some("cap"));
        c.listens = vec![listen("w2", NOW.add(MIN), None)];
        for sender in [Sender::Human, Sender::System] {
            let t = plan(&msg(sender.clone(), vec![at("w1")]), &c, None, NOW);
            assert_eq!(t.len(), 1, "{sender:?}: 收窄给 w1，w2 看不见");
        }
        let t = plan(
            &msg(Sender::Human, vec![Mention::Broadcast, at("w1")]),
            &c,
            None,
            NOW,
        );
        let by: Vec<_> = t.iter().map(|x| (x.session.as_str(), x.priority)).collect();
        assert_eq!(by, [("w1", Priority::Human), ("w2", Priority::Auto)]);
        let t = plan(
            &msg(Sender::System, vec![Mention::Broadcast, at("w1")]),
            &c,
            None,
            NOW,
        );
        let by: Vec<_> = t.iter().map(|x| (x.session.as_str(), x.priority)).collect();
        assert_eq!(by, [("w1", Priority::System), ("w2", Priority::Auto)]);
        // 人不 @、兜底给了群主：群主 Human，收听者 Auto。
        let t = plan(&msg(Sender::Human, vec![]), &c, None, NOW);
        let by: Vec<_> = t.iter().map(|x| (x.session.as_str(), x.priority)).collect();
        assert_eq!(by, [("cap", Priority::Human), ("w2", Priority::Auto)]);
        // 又被点名又在收听：保留被点名的那档。
        c.listens = vec![listen("w1", NOW.add(MIN), None)];
        let t = plan(&msg(Sender::Human, vec![at("w1")]), &c, None, NOW);
        assert_eq!(
            t,
            vec![WakeTarget {
                session: "w1".into(),
                priority: Priority::Human
            }]
        );
    }

    #[test]
    fn arbiter_answer_and_listeners_are_unioned_once() {
        let mut c = ctx(&["cap", "w1", "w2"], Some("cap"));
        c.listens = vec![
            listen("w2", NOW.add(MIN), None),
            listen("w1", NOW.add(MIN), None),
        ];
        let jev = ["w2".to_string()];
        let t = plan(&msg(Sender::Human, vec![]), &c, Some(&jev), NOW);
        let by: Vec<_> = t.iter().map(|x| (x.session.as_str(), x.priority)).collect();
        assert_eq!(
            by,
            [("w2", Priority::Human), ("w1", Priority::Auto)],
            "w2 只出现一次、保留 Human 档"
        );
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
        let c = ctx(&["cap", "w1"], Some("cap"));
        assert!(needs_arbiter(&msg(Sender::Human, vec![]), &c, NOW));
        assert!(!needs_arbiter(&msg(Sender::Human, vec![at("w1")]), &c, NOW));
        assert!(!needs_arbiter(&msg(s("w1"), vec![]), &c, NOW));
        assert!(!needs_arbiter(&msg(Sender::System, vec![]), &c, NOW));
        let mut old = msg(Sender::Human, vec![]);
        old.created_at = NOW.add(-MAX_WAKE_AGE_MS - 1);
        assert!(!needs_arbiter(&old, &c, NOW));
    }
}
