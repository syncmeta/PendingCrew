//! Messaging tools (spec §22.3, §24.2): `post`, `read_channel`, `search`, `list_channels`.
//!
//! Who may read or post where is collab's rule (B15, the management crew, dms); these tools only
//! resolve arguments, run the post gate (S61) and write receipts. Refusals pass through `Ctx::fail`.
use std::collections::HashMap;

use crewd_proto::agent::CallResult;
use crewd_proto::{ChannelKind, Mention, MentionKind};
use serde::Deserialize;
use serde_json::Value;

use super::{
    Ctx, RefArg, channel_choices, channel_label, crew_arg, msg_ref, parse_args, post_as, ref_text,
    render_line, resolve_channel,
};
use crate::agent::catalog::CATEGORIES;
use crate::agent::port::{self, ChannelView, MsgRef, PostSender};
use crate::agent::runtime::{GateRefusal, PostAttempt};

/// `messages[≤6]`.
const MAX_ITEMS: usize = 6;
/// `list_channels` shows at most this many rows.
const LIST_CAP: usize = 100;
/// Joins names in a receipt (no prose: the same in both locales).
const NAME_SEP: &str = ", ";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    text: String,
    category: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MentionArg {
    kind: String,
    target_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostArgs {
    channel: Option<String>,
    to: Option<String>,
    message: Option<String>,
    messages: Option<Vec<Item>>,
    #[serde(default)]
    mentions: Vec<MentionArg>,
    reply_to: Option<RefArg>,
    category: Option<String>,
}

/// Where a post goes.
enum Target {
    Channel(ChannelView),
    /// A dm with `peer` (a session id), reached through `address`; `existing` = the dm when the
    /// pair already has one (the gate's duplicate check needs its id before anything is written).
    Dm {
        peer: String,
        address: String,
        existing: Option<String>,
    },
}

pub async fn post(cx: &Ctx<'_>, args: Value) -> CallResult {
    let a: PostArgs = match parse_args(cx, args) {
        Ok(a) => a,
        Err(e) => return e,
    };
    // 1. Validate the whole batch before anything else (ruling 13: all or nothing).
    let items: Vec<Item> = match (a.message, a.messages) {
        (Some(text), None) => vec![Item {
            text,
            category: a.category.clone(),
        }],
        (None, Some(v)) if a.category.is_none() => v,
        _ => return cx.err("r.post_one_of", &[]),
    };
    if items.is_empty() || items.len() > MAX_ITEMS {
        return cx.err("r.post_count", &[("count", &items.len().to_string())]);
    }
    for (i, it) in items.iter().enumerate() {
        let n = (i + 1).to_string();
        if it.text.trim().is_empty() {
            return cx.err("r.post_empty", &[("index", &n)]);
        }
        if let Some(c) = it.category.as_deref().filter(|c| !CATEGORIES.contains(c)) {
            return cx.err(
                "r.bad_category",
                &[
                    ("index", &n),
                    ("value", c),
                    ("choices", &CATEGORIES.join(" / ")),
                ],
            );
        }
    }
    let mut mentions = Vec::new();
    for m in a.mentions {
        let kind = match MentionKind::parse(&m.kind) {
            Some(k) if !(k == MentionKind::Session && m.target_id.is_none()) => k,
            _ => return cx.err("r.bad_mention", &[("kind", &m.kind)]),
        };
        // C3: a session target may be given as an address, too.
        let target_id = match (kind, m.target_id) {
            (MentionKind::Session, Some(t)) => match port::resolve_address(cx.st, &t).await {
                Ok(s) => Some(s),
                Err(e) => return cx.fail(e),
            },
            (_, t) => t,
        };
        mentions.push(Mention { kind, target_id });
    }
    // One post call of this session at a time, from the target lookup to the duplicate record:
    // two identical posts sent together cannot both pass the gate.
    let lock = cx.st.agent.post_lock(&cx.me.session_id);
    let _held = lock.lock().await;
    // 2. The target: exactly one of channel / to, or reply_to alone (no default target, S64).
    let target = match resolve_target(cx, a.channel, a.to, a.reply_to.as_ref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let reply = match &a.reply_to {
        Some(r) => match port::message(cx.st, cx.me, msg_ref(r)).await {
            Ok(m) => m.map(|m| m.id),
            Err(e) => return cx.fail(e),
        },
        None => None,
    };
    // 3. The post gate, before anything is written (a new dm included).
    let texts: Vec<String> = items.iter().map(|it| it.text.clone()).collect();
    let gate_channel = match &target {
        Target::Channel(c) => Some(c.id.clone()),
        Target::Dm { existing, .. } => existing.clone(),
    };
    let human = mentions.iter().any(|m| m.kind == MentionKind::Human);
    let attempt = PostAttempt {
        session: &cx.me.session_id,
        channel: gate_channel.as_deref(),
        texts: &texts,
        human_mention: human,
    };
    if let Err(g) = cx.st.agent.admit_post(&attempt) {
        return match g {
            GateRefusal::Throttled { retry_secs } => cx.err(
                "r.post_throttled",
                &[("retry_secs", &retry_secs.to_string())],
            ),
            GateRefusal::Duplicate { seq } => {
                cx.err("r.post_duplicate", &[("seq", &seq.to_string())])
            }
            GateRefusal::HumanMentions => cx.err("r.human_mention_throttled", &[]),
            GateRefusal::RepeatedInBatch { index } => {
                cx.err("r.post_repeated", &[("index", &index.to_string())])
            }
        };
    }
    // 4. Open the dm when the target is an address.
    let (channel, dm) = match target {
        Target::Channel(c) => (c, None),
        Target::Dm { peer, address, .. } => match port::ensure_dm(cx.st, cx.me, &peer).await {
            Ok((c, created)) => (c, Some((address, created))),
            Err(e) => {
                cx.st
                    .agent
                    .refund_post(&cx.me.session_id, texts.len(), human);
                return cx.fail(e);
            }
        },
    };
    // 5. Post item by item; mentions and reply_to ride on the first one.
    let total = items.len();
    let mut posted = Vec::new();
    let mut unwatched = false;
    for (i, it) in items.into_iter().enumerate() {
        let first = i == 0;
        let res = post_as(
            cx,
            &channel.id,
            it.text.clone(),
            if first { mentions.clone() } else { vec![] },
            if first { reply.clone() } else { None },
            it.category,
            PostSender::Session(cx.me.session_id.clone()),
        )
        .await;
        match res {
            Ok(p) => {
                cx.st
                    .agent
                    .note_posted(&cx.me.session_id, &channel.id, &it.text, p.message.seq);
                unwatched |= p.unwatched_human_mention;
                posted.push(p.message);
            }
            Err(e) if posted.is_empty() => {
                cx.st.agent.refund_post(&cx.me.session_id, total, human);
                return first_failure(cx, &channel, e).await;
            }
            Err(e) => {
                // The @ rode on the first item, which was posted: only the tokens come back.
                cx.st.agent.refund_post(&cx.me.session_id, total - i, false);
                let detail = cx.fail(e).text;
                return cx.err(
                    "r.post_partial",
                    &[
                        ("sent", &seq_list(&posted)),
                        ("index", &(i + 1).to_string()),
                        ("total", &total.to_string()),
                        ("detail", &detail),
                    ],
                );
            }
        }
    }
    receipt(cx, &channel, dm, &posted, unwatched).await
}

/// The first write was refused. A dm is member-only (also for the management crew): say that
/// instead of collab's general "ask a member to add you", which a dm cannot do.
async fn first_failure(cx: &Ctx<'_>, channel: &ChannelView, e: port::PortError) -> CallResult {
    use crewd_collab::refusal::Code;
    let not_member = matches!(&e, port::PortError::Refused(port::Reason::Rule(r)) if r.code == Code::MessageSenderNotMember);
    if channel.kind == ChannelKind::Dm && not_member {
        let label = channel_label(cx, channel).await;
        return cx.err("r.dm_member_only", &[("channel", &label)]);
    }
    cx.fail(e)
}

fn seq_list(ms: &[crewd_proto::Message]) -> String {
    ms.iter()
        .map(|m| format!("#{}", m.seq))
        .collect::<Vec<_>>()
        .join(" ")
}

async fn resolve_target(
    cx: &Ctx<'_>,
    channel: Option<String>,
    to: Option<String>,
    reply_to: Option<&RefArg>,
) -> Result<Target, CallResult> {
    match (channel, to, reply_to) {
        (Some(_), Some(_), _) | (None, Some(_), Some(_)) => {
            Err(cx.err("r.post_target_conflict", &[]))
        }
        (None, None, None) => Err(no_target(cx).await),
        (None, Some(to), None) if to.trim().eq_ignore_ascii_case("human") => {
            Err(to_human(cx).await)
        }
        (None, Some(to), None) => {
            let peer = port::resolve_address(cx.st, &to)
                .await
                .map_err(|e| cx.fail(e))?;
            if peer == cx.me.session_id {
                return Err(cx.err("r.post_to_self", &[("address", to.trim())]));
            }
            // Read only: the dm this pair already has (ensure_dm reuses it), for the gate.
            let existing = port::my_channels(cx.st, &cx.me.session_id)
                .await
                .map_err(|e| cx.fail(e))?
                .into_iter()
                .find(|c| {
                    c.kind == ChannelKind::Dm
                        && !c.archived
                        && c.members.len() == 2
                        && c.has_session(&peer)
                })
                .map(|c| c.id);
            Ok(Target::Dm {
                peer,
                address: to.trim().to_string(),
                existing,
            })
        }
        (channel, None, Some(r)) => {
            let m = match port::message(cx.st, cx.me, msg_ref(r)).await {
                Ok(Some(m)) => m,
                Ok(None) => return Err(cx.err("r.no_message", &[("ref", &ref_text(r))])),
                Err(e) => return Err(cx.fail(e)),
            };
            let named = match channel {
                Some(arg) => Some(resolve_channel(cx, &arg).await?),
                None => None,
            };
            if named.as_ref().is_some_and(|c| c.id != m.channel_id) {
                return Err(cx.err("r.reply_other_channel", &[("seq", &m.seq.to_string())]));
            }
            match named {
                Some(c) => Ok(Target::Channel(c)),
                // `message` only returns messages of channels I can read: the channel resolves.
                None => resolve_channel(cx, &m.channel_id)
                    .await
                    .map(Target::Channel),
            }
        }
        (Some(arg), None, None) => resolve_channel(cx, &arg).await.map(Target::Channel),
    }
}

/// `to: "human"`: the human has no dm; point to the home human channel (or to ask before it exists).
async fn to_human(cx: &Ctx<'_>) -> CallResult {
    match port::home_human_channel_if_any(cx.st, &cx.me.crew_id).await {
        Ok(Some(home)) => {
            let label = channel_label(cx, &home).await;
            cx.err(
                "r.post_to_human",
                &[("home", &label), ("home_id", &home.id)],
            )
        }
        Ok(None) => cx.err("r.post_to_human_no_channel", &[]),
        Err(e) => cx.fail(e),
    }
}

/// `r.post_no_target`: the channels I am in (capped) and how to reach the rest.
async fn no_target(cx: &Ctx<'_>) -> CallResult {
    let views = match port::readable_views(cx.st, cx.me).await {
        Ok(v) => v,
        Err(e) => return cx.fail(e),
    };
    let mine: Vec<&ChannelView> = views
        .iter()
        .filter(|c| !c.archived && (c.has_session(&cx.me.session_id) || c.listening))
        .collect();
    let choices = channel_choices(cx, &mine).await;
    cx.err("r.post_no_target", &[("choices", &choices)])
}

/// What happened and whom it wakes (S72: the stored @ targets; M3i-a swaps in the wake planner).
async fn receipt(
    cx: &Ctx<'_>,
    channel: &ChannelView,
    dm: Option<(String, bool)>,
    posted: &[crewd_proto::Message],
    unwatched: bool,
) -> CallResult {
    let label = channel_label(cx, channel).await;
    let seqs = seq_list(posted);
    let mut out = Vec::new();
    match &dm {
        Some((address, created)) => {
            let new = if *created {
                cx.r("r.dm_opened", &[("id", &channel.id)])
            } else {
                String::new()
            };
            out.push(cx.r(
                "r.posted_dm",
                &[
                    ("peer", &label),
                    ("address", address),
                    ("seqs", &seqs),
                    ("new", &new),
                ],
            ));
        }
        None => out.push(cx.r("r.posted", &[("channel", &label), ("seqs", &seqs)])),
    }
    if channel.kind == ChannelKind::Dm {
        out.push(cx.r("r.posted_wakes_dm", &[]));
    } else {
        let names = match wake_names(cx, posted).await {
            Ok(n) => n,
            Err(e) => return e,
        };
        if names.is_empty() {
            out.push(cx.r("r.posted_wakes_none", &[]));
        } else {
            out.push(cx.r("r.posted_wakes", &[("names", &names.join(NAME_SEP))]));
        }
        // Membership AFTER the post: posting into my home human channel joins it (collab).
        let member = match port::channel(cx.st, &channel.id).await {
            Ok(c) => c.is_some_and(|c| c.has_session(&cx.me.session_id)),
            Err(e) => return cx.fail(e),
        };
        let listening = match port::listening_to(cx.st, &cx.me.session_id).await {
            Ok(l) => l.contains(&channel.id),
            Err(e) => return cx.fail(e),
        };
        if !member && !listening {
            // 26.5: a management session posting where it is not a member is not woken by replies.
            out.push(cx.r("r.posted_not_member", &[("channel", &label)]));
        }
    }
    if unwatched {
        // §17: @human in a channel the human is not watching (collab decides).
        // Read only: a receipt never creates the home human channel.
        match port::home_human_channel_if_any(cx.st, &cx.me.crew_id).await {
            Ok(Some(home)) => {
                let home_label = channel_label(cx, &home).await;
                out.push(cx.r(
                    "r.human_not_watching",
                    &[
                        ("channel", &label),
                        ("home", &home_label),
                        ("home_id", &home.id),
                    ],
                ));
            }
            Ok(None) => out.push(cx.r("r.human_not_watching_no_home", &[("channel", &label)])),
            Err(e) => return cx.fail(e),
        }
    }
    CallResult::ok(out.join("\n"))
}

/// The @ targets of the posted messages that this wakes, as the injection surface labels them:
/// not me, and not @human (the human is not woken; `r.human_not_watching` covers where it is seen).
/// Listeners are not listed (S72 placeholder; M3i-a swaps in the wake planner).
async fn wake_names(
    cx: &Ctx<'_>,
    posted: &[crewd_proto::Message],
) -> Result<Vec<String>, CallResult> {
    let Some(first) = posted.first() else {
        return Ok(vec![]);
    };
    let me = cx.me.session_id.as_str();
    let woken: Vec<crewd_proto::Message> = posted
        .iter()
        .cloned()
        .map(|mut m| {
            m.mentions
                .retain(|x| x.kind != MentionKind::Human && x.target_id.as_deref() != Some(me));
            m
        })
        .collect();
    let blocks = port::blocks(
        cx.st,
        &cx.me.session_id,
        cx.loc,
        vec![(first.channel_id.clone(), woken)],
    )
    .await
    .map_err(|e| cx.fail(e))?;
    let mut names: Vec<String> = Vec::new();
    for line in blocks.iter().flat_map(|b| &b.lines) {
        for n in &line.mentions {
            if !names.contains(n) {
                names.push(n.clone());
            }
        }
    }
    Ok(names)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    channel: Option<String>,
    limit: Option<u32>,
    before: Option<RefArg>,
}

pub async fn read_channel(cx: &Ctx<'_>, args: Value) -> CallResult {
    let a: ReadArgs = match parse_args(cx, args) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let ch = match a.channel.as_deref() {
        None => match port::lobby_of(cx.st, &cx.me.crew_id).await {
            Ok(c) => c,
            Err(e) => return cx.fail(e),
        },
        Some(arg) => match resolve_channel(cx, arg).await {
            Ok(c) => c,
            Err(e) => return e,
        },
    };
    let limit = a.limit.unwrap_or(50).clamp(1, 200);
    // A number is a bound, not a message that has to exist: `before = seq + 1, limit = 1` fetches
    // `#seq` whole (the `inject.truncated` contract). An id names a message.
    let before = match &a.before {
        None => None,
        Some(r) => match msg_ref(r) {
            MsgRef::Seq(n) => Some(n),
            MsgRef::Id(id) => match port::message(cx.st, cx.me, MsgRef::Id(id)).await {
                Ok(Some(m)) => Some(m.seq),
                Ok(None) => return cx.err("r.no_message", &[("ref", &ref_text(r))]),
                Err(e) => return cx.fail(e),
            },
        },
    };
    let msgs = match port::messages_as(cx.st, &cx.me.session_id, &ch.id, before, limit).await {
        Ok(v) => v,
        Err(e) => return cx.fail(e),
    };
    let label = channel_label(cx, &ch).await;
    if msgs.is_empty() {
        return cx.ok("r.read_empty", &[("channel", &label)]);
    }
    let mut out = vec![cx.r(
        "r.read_header",
        &[
            ("channel", &label),
            ("id", &ch.id),
            ("count", &msgs.len().to_string()),
        ],
    )];
    for m in &msgs {
        match render_line(cx, m).await {
            Ok(l) => out.push(l),
            Err(e) => return e,
        }
    }
    // The message just before this page, if any: "older ones exist" and how far the page reaches.
    let older = if msgs.len() as u32 == limit {
        match port::messages_as(cx.st, &cx.me.session_id, &ch.id, Some(msgs[0].seq), 1).await {
            Ok(v) => v.last().map(|m| m.seq),
            Err(e) => return cx.fail(e),
        }
    } else {
        None
    };
    if older.is_some() {
        out.push(cx.r("r.read_more", &[("before", &msgs[0].seq.to_string())]));
    }
    // Reading the latest page marks the channel read, but only where I am a member or listen
    // (reading someone else's channel leaves my cursors alone), and only when the page reaches
    // back to my cursor: unread messages older than the page were not delivered (R6).
    let listening = ch.listening
        || match port::listening_to(cx.st, &cx.me.session_id).await {
            Ok(l) => l.contains(&ch.id),
            Err(e) => return cx.fail(e),
        };
    if before.is_none() && (ch.has_session(&cx.me.session_id) || listening) {
        let reaches = match older {
            None => true,
            Some(o) => match port::cursor(cx.st, &cx.me.session_id, &ch.id).await {
                // No cursor: nothing is unread for me here (collab pins a missing cursor at the end).
                Ok(c) => c.is_none_or(|c| o <= c),
                Err(e) => return cx.fail(e),
            },
        };
        if reaches {
            let newest = msgs[msgs.len() - 1].seq;
            if let Err(e) = port::advance(cx.st, &cx.me.session_id, &ch.id, newest).await {
                eprintln!("crewd: read_channel cursor for {}: {e}", cx.me.session_id);
            }
        }
    }
    CallResult::ok(out.join("\n"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    channel: Option<String>,
    crew: Option<String>,
    after: Option<String>,
    before: Option<String>,
    limit: Option<u32>,
}

pub async fn search(cx: &Ctx<'_>, args: Value) -> CallResult {
    let a: SearchArgs = match parse_args(cx, args) {
        Ok(a) => a,
        Err(e) => return e,
    };
    // Scope: every channel I can read or listen to (archived ones too: they are history);
    // `channel` / `crew` narrow it.
    let mut scope = match port::readable_views(cx.st, cx.me).await {
        Ok(v) => v,
        Err(e) => return cx.fail(e),
    };
    if let Some(arg) = &a.channel {
        match resolve_channel(cx, arg).await {
            Ok(c) => scope.retain(|x| x.id == c.id),
            Err(e) => return e,
        }
    }
    if let Some(arg) = &a.crew {
        match crew_arg(cx, arg).await {
            Ok(c) => scope.retain(|x| x.crew_id == c.id),
            Err(e) => return e,
        }
    }
    let ids = scope.iter().map(|c| c.id.clone()).collect();
    let limit = a.limit.unwrap_or(50).clamp(1, 200);
    let query = a.query.trim();
    let hits = match port::search(
        cx.st,
        &cx.me.session_id,
        query,
        ids,
        a.after,
        a.before,
        limit,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return cx.fail(e),
    };
    if hits.is_empty() {
        return cx.ok("r.search_none", &[("query", query)]);
    }
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut out = Vec::new();
    for m in &hits {
        if !labels.contains_key(&m.channel_id) {
            let l = match scope.iter().find(|c| c.id == m.channel_id) {
                Some(c) => channel_label(cx, c).await,
                None => String::new(),
            };
            labels.insert(m.channel_id.clone(), l);
        }
        let line = match render_line(cx, m).await {
            Ok(l) => l,
            Err(e) => return e,
        };
        out.push(cx.r(
            "r.search_hit",
            &[("channel", &labels[&m.channel_id]), ("line", &line)],
        ));
    }
    CallResult::ok(out.join("\n"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    crew: Option<String>,
    #[serde(default)]
    include_archived: bool,
}

pub async fn list_channels(cx: &Ctx<'_>, args: Value) -> CallResult {
    let a: ListArgs = match parse_args(cx, args) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let me = cx.me.session_id.as_str();
    let views = match port::readable_views(cx.st, cx.me).await {
        Ok(v) => v,
        Err(e) => return cx.fail(e),
    };
    let mine = |c: &ChannelView| c.has_session(me) || c.listening;
    // (channel, is my home human channel)
    let mut rows: Vec<(ChannelView, bool)> = Vec::new();
    // My home human channel is not created by listing (read only); absent = one line saying so.
    let mut home_absent = false;
    let push = |rows: &mut Vec<(ChannelView, bool)>, c: &ChannelView, home: bool| {
        if !rows.iter().any(|(x, _)| x.id == c.id) {
            rows.push((c.clone(), home));
        }
    };
    match &a.crew {
        Some(arg) => {
            let crew = match crew_arg(cx, arg).await {
                Ok(c) => c,
                Err(e) => return e,
            };
            let of_crew: Vec<&ChannelView> =
                views.iter().filter(|c| c.crew_id == crew.id).collect();
            for c in of_crew.iter().filter(|c| mine(c)) {
                push(&mut rows, c, false);
            }
            for c in of_crew {
                push(&mut rows, c, false);
            }
        }
        None => {
            match port::home_human_channel_if_any(cx.st, &cx.me.crew_id).await {
                Ok(Some(home)) => push(&mut rows, &home, true),
                Ok(None) => home_absent = true,
                Err(e) => return cx.fail(e),
            }
            for c in views.iter().filter(|c| mine(c)) {
                push(&mut rows, c, false);
            }
            for c in views.iter().filter(|c| c.crew_id == cx.me.crew_id) {
                push(&mut rows, c, false);
            }
            for c in views.iter().filter(|c| c.watched) {
                push(&mut rows, c, false);
            }
        }
    }
    if !a.include_archived {
        rows.retain(|(c, home)| *home || !c.archived);
    }
    let more = rows.len().saturating_sub(LIST_CAP);
    rows.truncate(LIST_CAP);
    let mut out = vec![cx.r("r.list_header", &[])];
    if home_absent {
        out.push(cx.r("r.home_human_absent", &[]));
    }
    if rows.is_empty() {
        out.push(cx.r("r.list_none", &[]));
    }
    for (c, home) in &rows {
        out.push(list_line(cx, c, *home).await);
    }
    if more > 0 {
        out.push(cx.r("r.list_more", &[("count", &more.to_string())]));
    }
    CallResult::ok(out.join("\n"))
}

async fn list_line(cx: &Ctx<'_>, c: &ChannelView, home: bool) -> String {
    let me = cx.me.session_id.as_str();
    let mut flags = Vec::new();
    if home {
        flags.push(cx.r("r.flag_home_human", &[]));
    }
    if c.has_session(me) {
        flags.push(cx.r("r.flag_member", &[]));
    }
    if c.listening {
        flags.push(cx.r("r.flag_listening", &[]));
    }
    if c.watched {
        flags.push(cx.r("r.flag_watched", &[]));
    }
    if c.archived {
        flags.push(cx.r("r.flag_archived", &[]));
    }
    if c.owner.as_deref() == Some(me) {
        flags.push(cx.r("r.flag_owner", &[]));
    }
    let owner = match &c.owner {
        Some(o) => match port::identity(cx.st, o).await {
            Ok(Some(i)) => cx.name(&i.label),
            _ => cx.r("r.owner_none", &[]),
        },
        None => cx.r("r.owner_none", &[]),
    };
    cx.r(
        "r.list_line",
        &[
            ("id", &c.id),
            ("name", &channel_label(cx, c).await),
            ("kind", &cx.r(kind_key(c.kind), &[])),
            ("purpose", &cx.name(c.purpose.as_deref().unwrap_or(""))),
            ("owner", &owner),
            ("count", &c.members.len().to_string()),
            ("flags", &flags.join(NAME_SEP)),
        ],
    )
}

fn kind_key(k: ChannelKind) -> &'static str {
    match k {
        ChannelKind::Lobby => "r.kind_lobby",
        ChannelKind::Human => "r.kind_human",
        ChannelKind::Topic => "r.kind_topic",
        ChannelKind::Dm => "r.kind_dm",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    use crewd_collab::refusal::{Code, Refusal};
    use serde_json::json;

    use crate::agent::Role;
    use crate::agent::port::{self, ChannelKind, PortError, Reason};
    use crate::agent::runtime::{AgentConfig, AgentRuntime, Clock};
    use crate::agent::testkit::Fixture;
    use crate::agent::text::{self, Locale};
    use crate::agent::tools::call;

    async fn run(
        f: &Fixture,
        who: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> crewd_proto::agent::CallResult {
        let me = f.me(who).await;
        call(&f.state, &me, tool, args).await
    }

    async fn lobby(f: &Fixture) -> String {
        port::lobby_of(&f.state, &f.crew).await.unwrap().id
    }

    async fn last(f: &Fixture, ch: &str) -> crewd_proto::Message {
        port::messages_raw(&f.state, ch, None, 1)
            .await
            .unwrap()
            .pop()
            .unwrap()
    }

    /// The fixed parts of a template (placeholders replaced by a marker) all appear in `text`.
    fn shows(text: &str, key: &str, vars: &[&str]) -> bool {
        let filled: Vec<(&str, &str)> = vars.iter().map(|v| (*v, "\u{1}")).collect();
        text::r(Locale::Zh, key, &filled)
            .split('\u{1}')
            .filter(|part| !part.is_empty())
            .all(|part| text.contains(part))
    }

    async fn management_crew(f: &Fixture) -> String {
        f.state
            .db
            .call(|s| crewd_collab::crews::management(s.conn()))
            .await
            .unwrap()
            .unwrap()
            .expect("crew.create creates the management crew")
    }

    async fn number_of(f: &Fixture, crew: &str) -> String {
        port::crew(&f.state, crew)
            .await
            .unwrap()
            .unwrap()
            .number
            .unwrap()
            .to_string()
    }

    async fn extension_of(f: &Fixture, crew: &str, session: &str) -> String {
        port::sessions_of(&f.state, crew)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.id == session)
            .and_then(|s| s.extension)
            .unwrap()
    }

    /// A fixture whose post gate runs on a clock the test moves.
    async fn with_clock(config: AgentConfig) -> (Fixture, Arc<AtomicI64>) {
        let now = Arc::new(AtomicI64::new(1_000_000_000));
        let n = now.clone();
        let rt = AgentRuntime::new(config).with_clock(Clock::new(move || n.load(Ordering::SeqCst)));
        (Fixture::with_runtime(rt).await, now)
    }

    fn rendered_refusal(r: Refusal) -> String {
        text::r(Locale::Zh, "r.failed", &[("detail", &r.text(Locale::Zh))])
    }

    #[tokio::test]
    async fn post_without_target_is_refused_and_lists_channels() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let before = port::latest_seq(&f.state, &l).await.unwrap();
        let r = run(&f, &f.worker, "post", json!({"message": "示例：A 做完了"})).await;
        assert!(r.is_error, "{}", r.text);
        assert!(
            shows(&r.text, "r.post_no_target", &["choices"]),
            "{}",
            r.text
        );
        assert!(r.text.contains(&l), "lists my channels: {}", r.text);
        assert_eq!(port::latest_seq(&f.state, &l).await.unwrap(), before);
        // With a channel it goes there and reports the number.
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例：A 做完了", "category": "done"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        let m = last(&f, &l).await;
        assert_eq!(m.text, "示例：A 做完了");
        assert_eq!(m.sender_session_id.as_deref(), Some(f.worker.as_str()));
        assert!(r.text.contains(&format!("#{}", m.seq)), "{}", r.text);
    }

    #[tokio::test]
    async fn post_to_extension_opens_one_dm_and_reuses_it() {
        let f = Fixture::new().await;
        let other = f.add_crew("示例外组", None).await;
        let a = f.register(&f.crew, Role::Worker, "示例甲").await;
        let b = f.register(&other, Role::Worker, "示例乙").await;
        let (a_addr, b_addr) = (
            extension_of(&f, &f.crew, &a).await,
            extension_of(&f, &other, &b).await,
        );
        let r1 = run(
            &f,
            &a,
            "post",
            json!({"to": b_addr, "message": "示例：第一句"}),
        )
        .await;
        assert!(!r1.is_error, "{}", r1.text);
        let r2 = run(
            &f,
            &a,
            "post",
            json!({"to": b_addr, "message": "示例：第二句"}),
        )
        .await;
        assert!(!r2.is_error, "{}", r2.text);
        let back = run(
            &f,
            &b,
            "post",
            json!({"to": a_addr, "message": "示例：回一句"}),
        )
        .await;
        assert!(!back.is_error, "{}", back.text);
        let me_a = f.me(&a).await;
        let dms: Vec<_> = port::my_channels(&f.state, &a)
            .await
            .unwrap()
            .into_iter()
            .filter(|c| c.kind == ChannelKind::Dm)
            .collect();
        assert_eq!(dms.len(), 1, "one dm for the pair: {dms:?}");
        let dm = &dms[0];
        assert_eq!(
            dm.crew_id, f.crew,
            "a new dm belongs to the initiator's crew"
        );
        let msgs = port::messages_as(&f.state, &me_a.session_id, &dm.id, None, 10)
            .await
            .unwrap();
        assert_eq!(
            msgs.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            ["示例：第一句", "示例：第二句", "示例：回一句"]
        );
        assert!(r1.text.contains(&b_addr), "{}", r1.text);
        // B15: a crewmate of the initiator reads the dm.
        let r = run(&f, &f.worker, "read_channel", json!({"channel": dm.id})).await;
        assert!(!r.is_error && r.text.contains("示例：回一句"), "{}", r.text);
    }

    #[tokio::test]
    async fn post_to_crew_number_reaches_its_current_captain() {
        let f = Fixture::new().await;
        let other = f.add_crew("示例外组", None).await;
        f.add_session(&other, "s-o", Role::Worker, "外组 worker")
            .await;
        let n = number_of(&f, &f.crew).await;
        let dm_with = |peer: &'static str| {
            let f = &f;
            async move {
                port::my_channels(&f.state, "s-o")
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|c| c.kind == ChannelKind::Dm && c.has_session(peer))
            }
        };
        let r = run(
            &f,
            "s-o",
            "post",
            json!({"to": n, "message": "示例：找机长"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert!(dm_with("s-cap").await.is_some());
        f.set_role(&f.worker, Role::Captain).await;
        let r = run(
            &f,
            "s-o",
            "post",
            json!({"to": n, "message": "示例：找新机长"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert!(
            dm_with("s-w1").await.is_some(),
            "the number follows the captain"
        );
        // "0" = the management crew's captain.
        let mgmt = management_crew(&f).await;
        f.add_session(&mgmt, "s-mcap", Role::Captain, "管理层机长")
            .await;
        let r = run(
            &f,
            "s-o",
            "post",
            json!({"to": "0", "message": "示例：找总机组"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert!(dm_with("s-mcap").await.is_some());
    }

    #[tokio::test]
    async fn post_to_removed_crew_is_refused_with_its_code() {
        let f = Fixture::new().await;
        let gone = f.add_crew("示例已移除", None).await;
        let n = number_of(&f, &gone).await;
        f.rpc("crew.remove", json!({"crewId": gone})).await;
        let e = port::resolve_address(&f.state, &n).await.unwrap_err();
        let PortError::Refused(Reason::Rule(rule)) = &e else {
            panic!("{e:?}")
        };
        assert_eq!(rule.code, Code::DirectoryCrewRemoved);
        let r = run(&f, &f.worker, "post", json!({"to": n, "message": "示例"})).await;
        assert!(r.is_error);
        assert_eq!(
            r.text,
            rendered_refusal(Refusal::new(Code::DirectoryCrewRemoved).arg("number", n.clone()))
        );
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"to": "7-x", "message": "示例"}),
        )
        .await;
        assert_eq!(
            r.text,
            rendered_refusal(Refusal::new(Code::DirectoryBadAddress).arg("address", "7-x"))
        );
    }

    #[tokio::test]
    async fn post_to_and_reply_to_conflict() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let q = f.session_post(&f.captain, &l, "示例问题", vec![]).await;
        let n = number_of(&f, &f.crew).await;
        let before = port::latest_seq(&f.state, &l).await.unwrap();
        for args in [
            json!({"to": n, "reply_to": q.seq, "message": "x"}),
            json!({"to": n, "channel": l, "message": "x"}),
        ] {
            let r = run(&f, &f.worker, "post", args).await;
            assert!(r.is_error);
            assert_eq!(r.text, text::r(Locale::Zh, "r.post_target_conflict", &[]));
        }
        assert_eq!(port::latest_seq(&f.state, &l).await.unwrap(), before);
        // Myself by address (the captain posting to its own crew's number).
        let r = run(&f, &f.captain, "post", json!({"to": n, "message": "x"})).await;
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.post_to_self", &[("address", &n)])
        );
    }

    #[tokio::test]
    async fn reply_to_by_number_goes_to_that_channel() {
        let f = Fixture::new().await;
        let cap = f.me(&f.captain).await;
        let topic = port::create_channel(
            &f.state,
            &cap,
            ChannelKind::Topic,
            "示例话题".into(),
            None,
            vec![port::MemberRef::Session(f.worker.clone())],
        )
        .await
        .unwrap();
        let q = f
            .session_post(&f.captain, &topic.id, "这个怎么办？", vec![])
            .await;
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"message": "按 A 办", "reply_to": format!("#{}", q.seq)}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        let m = last(&f, &topic.id).await;
        assert_eq!(m.in_reply_to.as_deref(), Some(q.id.as_str()));
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"message": "x", "reply_to": 999_999}),
        )
        .await;
        assert!(r.is_error);
        // reply_to + a different channel.
        let l = lobby(&f).await;
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"message": "x", "reply_to": q.seq, "channel": l}),
        )
        .await;
        assert_eq!(
            r.text,
            text::r(
                Locale::Zh,
                "r.reply_other_channel",
                &[("seq", &q.seq.to_string())]
            )
        );
    }

    #[tokio::test]
    async fn batch_is_validated_all_or_nothing() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let before = port::latest_seq(&f.state, &l).await.unwrap();
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "messages": [
                {"text": "第一条", "category": "progress"},
                {"text": "第二条", "category": "plan"}
            ]}),
        )
        .await;
        assert!(r.is_error);
        assert!(r.text.contains('2'), "names the item: {}", r.text);
        assert_eq!(
            port::latest_seq(&f.state, &l).await.unwrap(),
            before,
            "none sent"
        );
        let seven: Vec<_> = (0..7)
            .map(|i| json!({"text": format!("第{i}条")}))
            .collect();
        assert!(
            run(
                &f,
                &f.worker,
                "post",
                json!({"channel": l, "messages": seven})
            )
            .await
            .is_error
        );
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "a", "messages": [{"text": "b"}]}),
        )
        .await;
        assert!(r.is_error, "one of the two");
        let ok = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "messages": [{"text": "进度"}, {"text": "要拍板", "category": "question"}]}),
        )
        .await;
        assert!(!ok.is_error, "{}", ok.text);
        assert_eq!(
            port::messages_raw(&f.state, &l, None, 2)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn unknown_argument_is_named() {
        let f = Fixture::new().await;
        let r = run(&f, &f.worker, "post", json!({"message": "x", "plan": 3})).await;
        assert!(r.is_error && r.text.contains("plan"), "{}", r.text);
    }

    #[tokio::test]
    async fn posting_with_captain_mention_counts_as_calling_out() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        let l = lobby(&f).await;
        crate::agent::turn::stop(
            &f.state,
            &me,
            &json!({"hook_event_name": "Stop", "prompt_id": "p0", "last_assistant_message": "好。"}),
        )
        .await;
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "机长看下", "mentions": [{"kind": "captain"}]}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        let before = port::latest_seq(&f.state, &l).await.unwrap();
        crate::agent::turn::stop(
            &f.state,
            &me,
            &json!({"hook_event_name": "Stop", "prompt_id": "p1", "last_assistant_message": "要不要继续？"}),
        )
        .await;
        assert_eq!(
            port::latest_seq(&f.state, &l).await.unwrap(),
            before,
            "the captain was called this turn: no relay"
        );
    }

    #[tokio::test]
    async fn receipt_names_whom_it_wakes() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let none = text::r(Locale::Zh, "r.posted_wakes_none", &[]);
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例进度"}),
        )
        .await;
        assert!(r.text.contains(&none), "{}", r.text);
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例：机长看下", "mentions": [{"kind": "captain"}]}),
        )
        .await;
        assert!(!r.text.contains(&none), "{}", r.text);
        let m = last(&f, &l).await;
        assert!(shows(&r.text, "r.posted_wakes", &["names"]), "{}", r.text);
        // The captain's label exactly as the injection surface writes it.
        let seen = port::blocks(&f.state, &f.worker, Locale::Zh, vec![(l.clone(), vec![m])])
            .await
            .unwrap();
        let names = seen[0].lines[0].mentions.join(", ");
        assert!(!names.is_empty());
        assert!(
            r.text
                .contains(&text::r(Locale::Zh, "r.posted_wakes", &[("names", &names)])),
            "{}",
            r.text
        );
        // A dm: the other side is always woken.
        let n = number_of(&f, &f.crew).await;
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"to": n, "message": "示例私聊"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert!(
            r.text
                .contains(&text::r(Locale::Zh, "r.posted_wakes_dm", &[])),
            "{}",
            r.text
        );
        assert!(!r.text.contains(&none), "{}", r.text);
    }

    #[tokio::test]
    async fn management_session_posts_without_joining_and_is_told_replies_wont_wake_it() {
        let f = Fixture::new().await;
        let mgmt = management_crew(&f).await;
        f.add_session(&mgmt, "s-mgr", Role::Worker, "管理层 worker")
            .await;
        let l = lobby(&f).await;
        let r = run(
            &f,
            "s-mgr",
            "post",
            json!({"channel": l, "message": "示例：全组织通知"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(last(&f, &l).await.text, "示例：全组织通知");
        assert!(
            shows(&r.text, "r.posted_not_member", &["channel"]),
            "{}",
            r.text
        );
        let ch = port::lobby_of(&f.state, &f.crew).await.unwrap();
        assert!(!ch.has_session("s-mgr"), "no auto-join");
        assert!(
            port::listening_to(&f.state, "s-mgr")
                .await
                .unwrap()
                .is_empty(),
            "no auto-listen"
        );
        // A member posting in its own lobby gets no such note.
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例"}),
        )
        .await;
        assert!(
            !shows(&r.text, "r.posted_not_member", &["channel"]),
            "{}",
            r.text
        );
    }

    #[tokio::test]
    async fn burst_is_throttled_then_refills() {
        let (f, now) = with_clock(AgentConfig::default()).await;
        let l = lobby(&f).await;
        for i in 0..6 {
            let r = run(
                &f,
                &f.worker,
                "post",
                json!({"channel": l, "message": format!("示例 {i}")}),
            )
            .await;
            assert!(!r.is_error, "{i}: {}", r.text);
        }
        let before = port::latest_seq(&f.state, &l).await.unwrap();
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例 6"}),
        )
        .await;
        assert!(r.is_error);
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.post_throttled", &[("retry_secs", "20")])
        );
        assert_eq!(port::latest_seq(&f.state, &l).await.unwrap(), before);
        // Another session has its own bucket.
        let r = run(
            &f,
            &f.captain,
            "post",
            json!({"channel": l, "message": "示例 机长"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        now.fetch_add(20_000, Ordering::SeqCst);
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例 6"}),
        )
        .await;
        assert!(!r.is_error, "one post back after 20 s: {}", r.text);
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例 7"}),
        )
        .await;
        assert!(r.is_error, "{}", r.text);
    }

    #[tokio::test]
    async fn duplicate_text_is_refused_with_the_earlier_seq() {
        let (f, now) = with_clock(AgentConfig::default()).await;
        let l = lobby(&f).await;
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例：A 做完了"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        let first = last(&f, &l).await;
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "  示例：A   做完了 "}),
        )
        .await;
        assert!(r.is_error);
        assert_eq!(
            r.text,
            text::r(
                Locale::Zh,
                "r.post_duplicate",
                &[("seq", &first.seq.to_string())]
            )
        );
        assert_eq!(last(&f, &l).await.id, first.id);
        // Another channel, or after the window: fine.
        let human = port::home_human_channel(&f.state, &f.crew).await.unwrap();
        let r = run(
            &f,
            &f.captain,
            "post",
            json!({"channel": human.id, "message": "示例：A 做完了"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        now.fetch_add(600_000, Ordering::SeqCst);
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例：A 做完了"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
    }

    #[tokio::test]
    async fn human_mentions_are_capped() {
        let (f, now) = with_clock(AgentConfig::default()).await;
        let l = lobby(&f).await;
        let human = [json!({"kind": "human"})];
        for i in 0..3 {
            let r = run(
                &f,
                &f.worker,
                "post",
                json!({"channel": l, "message": format!("示例 @人 {i}"), "mentions": human}),
            )
            .await;
            assert!(!r.is_error, "{i}: {}", r.text);
        }
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例 @人 3", "mentions": human}),
        )
        .await;
        assert!(r.is_error);
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.human_mention_throttled", &[])
        );
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例 不 @"}),
        )
        .await;
        assert!(!r.is_error, "without @human it goes: {}", r.text);
        now.fetch_add(30 * 60_000, Ordering::SeqCst);
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例 @人 4", "mentions": human}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
    }

    #[tokio::test]
    async fn a_throttled_batch_posts_nothing() {
        let (f, _now) = with_clock(AgentConfig::default()).await;
        let l = lobby(&f).await;
        for i in 0..3 {
            run(
                &f,
                &f.worker,
                "post",
                json!({"channel": l, "message": format!("示例 {i}")}),
            )
            .await;
        }
        let before = port::latest_seq(&f.state, &l).await.unwrap();
        let four: Vec<_> = (0..4).map(|i| json!({"text": format!("批 {i}")})).collect();
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "messages": four}),
        )
        .await;
        assert!(r.is_error);
        assert!(
            shows(&r.text, "r.post_throttled", &["retry_secs"]),
            "{}",
            r.text
        );
        assert_eq!(
            port::latest_seq(&f.state, &l).await.unwrap(),
            before,
            "all or nothing"
        );
        // A batch that carries a duplicate is refused whole, too.
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "messages": [{"text": "新的"}, {"text": "示例 1"}]}),
        )
        .await;
        assert!(shows(&r.text, "r.post_duplicate", &["seq"]), "{}", r.text);
        assert_eq!(port::latest_seq(&f.state, &l).await.unwrap(), before);
    }

    #[tokio::test]
    async fn human_mention_in_unwatched_channel_points_to_home_human_channel() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let human = port::home_human_channel(&f.state, &f.crew).await.unwrap();
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "@人 看一下", "mentions": [{"kind": "human"}]}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert!(
            shows(
                &r.text,
                "r.human_not_watching",
                &["channel", "home", "home_id"]
            ),
            "{}",
            r.text
        );
        assert!(r.text.contains(&human.id), "{}", r.text);
        let r = run(
            &f,
            &f.captain,
            "post",
            json!({"channel": human.id, "message": "要你拍板", "mentions": [{"kind": "human"}]}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert!(
            !shows(
                &r.text,
                "r.human_not_watching",
                &["channel", "home", "home_id"]
            ),
            "not in the human channel: {}",
            r.text
        );
        f.rpc("channel.watch", json!({"channelId": l, "on": true}))
            .await;
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "@人 再看一下", "mentions": [{"kind": "human"}]}),
        )
        .await;
        assert!(
            !shows(
                &r.text,
                "r.human_not_watching",
                &["channel", "home", "home_id"]
            ),
            "not in a watched channel: {}",
            r.text
        );
    }

    #[tokio::test]
    async fn read_channel_pages_and_marks_read_only_at_the_latest_page() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        for i in 0..5 {
            f.human_post(&l, &format!("消息{i}"), vec![]).await;
        }
        let top = port::latest_seq(&f.state, &l).await.unwrap().unwrap();
        let older = run(
            &f,
            &f.worker,
            "read_channel",
            json!({"limit": 2, "before": top}),
        )
        .await;
        assert!(
            older.text.contains("消息2")
                && older.text.contains("消息3")
                && !older.text.contains("消息4"),
            "{}",
            older.text
        );
        let me = f.me(&f.worker).await;
        let pending = crate::agent::inject::prepare(&f.state, &me)
            .await
            .unwrap()
            .and_then(|p| p.context)
            .unwrap_or_default();
        assert!(
            pending.contains("消息0") && pending.contains("消息3"),
            "an older page is not 'read', not even up to its last line: {pending}"
        );
        let latest = run(&f, &f.worker, "read_channel", json!({})).await;
        assert!(latest.text.contains("消息4"), "{}", latest.text);
        assert!(
            crate::agent::inject::prepare(&f.state, &me)
                .await
                .unwrap()
                .is_none(),
            "read up to the newest: nothing left to inject"
        );
        // A readable channel I am not in (a crewmate's topic): reading does not touch my cursor;
        // once I listen to it, reading the latest page marks it read.
        let cap = f.me(&f.captain).await;
        let topic = port::create_channel(
            &f.state,
            &cap,
            ChannelKind::Topic,
            "示例话题".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
        f.session_post(&f.captain, &topic.id, "示例：话题消息", vec![])
            .await;
        let r = run(&f, &f.worker, "read_channel", json!({"channel": topic.id})).await;
        assert!(r.text.contains("示例：话题消息"), "{}", r.text);
        let cursor = |f: &Fixture| {
            let t = topic.id.clone();
            let st = f.state.clone();
            async move {
                st.db
                    .call(move |s| {
                        s.conn()
                            .query_row(
                                "SELECT seq FROM read_cursors WHERE reader='s-w1' AND channel_id=?1",
                                [&t],
                                |r| r.get::<_, i64>(0),
                            )
                            .ok()
                    })
                    .await
                    .unwrap()
            }
        };
        assert_eq!(
            cursor(&f).await,
            None,
            "a non-member read does not mark read"
        );
        port::set_listen(&f.state, &f.worker, &topic.id, None, vec![])
            .await
            .unwrap();
        let r = run(&f, &f.worker, "read_channel", json!({"channel": topic.id})).await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(
            cursor(&f).await,
            port::latest_seq(&f.state, &topic.id).await.unwrap(),
            "listening: the latest page marks it read"
        );
    }

    #[tokio::test]
    async fn read_channel_fetches_a_truncated_message_whole() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let long = "示例长文".repeat(4_000);
        let m = f.human_post(&l, &long, vec![]).await;
        f.human_post(&l, "示例：之后的一条", vec![]).await;
        // The inject.truncated contract: before = seq + 1, limit = 1.
        for before in [json!(m.seq + 1), json!(format!("#{}", m.seq + 1))] {
            let r = run(
                &f,
                &f.worker,
                "read_channel",
                json!({"channel": l, "before": before, "limit": 1}),
            )
            .await;
            assert!(!r.is_error, "{}", r.text);
            assert!(r.text.contains(&long), "the whole text");
            assert!(!r.text.contains("示例：之后的一条"));
        }
    }

    #[tokio::test]
    async fn search_covers_readable_channels_not_child_crews() {
        let f = Fixture::new().await;
        let cap = f.me(&f.captain).await;
        let topic = port::create_channel(
            &f.state,
            &cap,
            ChannelKind::Topic,
            "机长的话题".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
        f.session_post(&f.captain, &topic.id, "暗号 青蛙七号", vec![])
            .await;
        let child = f.add_crew("示例子组", Some(&f.crew)).await;
        f.add_session(&child, "s-ch", Role::Worker, "子组 worker")
            .await;
        let ch_me = f.me("s-ch").await;
        let child_topic = port::create_channel(
            &f.state,
            &ch_me,
            ChannelKind::Topic,
            "子组话题".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
        f.session_post("s-ch", &child_topic.id, "暗号 青蛙九号", vec![])
            .await;
        f.human_post(&lobby(&f).await, "暗号 青蛙八号", vec![])
            .await;
        let r = run(&f, &f.worker, "search", json!({"query": "暗号"})).await;
        assert!(
            r.text.contains("青蛙七号")
                && r.text.contains("青蛙八号")
                && !r.text.contains("青蛙九号"),
            "{}",
            r.text
        );
        // An explicit channel I cannot read is not found.
        let r = run(
            &f,
            &f.worker,
            "search",
            json!({"query": "暗号", "channel": child_topic.id}),
        )
        .await;
        assert!(
            r.is_error && shows(&r.text, "r.no_channel", &["arg", "choices"]),
            "{}",
            r.text
        );
        // crew = that crew's readable channels.
        let n = number_of(&f, &f.crew).await;
        let r = run(&f, &f.worker, "search", json!({"query": "暗号", "crew": n})).await;
        assert!(
            r.text.contains("青蛙七号") && !r.text.contains("青蛙九号"),
            "{}",
            r.text
        );
        // The management crew reads the whole organisation.
        let mgmt = management_crew(&f).await;
        f.add_session(&mgmt, "s-mgr", Role::Worker, "管理层 worker")
            .await;
        let r = run(&f, "s-mgr", "search", json!({"query": "暗号"})).await;
        assert!(
            r.text.contains("青蛙七号") && r.text.contains("青蛙九号"),
            "{}",
            r.text
        );
    }

    #[tokio::test]
    async fn list_channels_puts_home_human_channel_first_and_flags_watched() {
        let f = Fixture::new().await;
        let human = port::home_human_channel(&f.state, &f.crew).await.unwrap();
        let cap = f.me(&f.captain).await;
        let topic = port::create_channel(
            &f.state,
            &cap,
            ChannelKind::Topic,
            "示例话题".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
        let archived = port::create_channel(
            &f.state,
            &cap,
            ChannelKind::Topic,
            "示例旧话题".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
        port::archive_channel(&f.state, &cap, &archived.id)
            .await
            .unwrap();
        f.rpc("channel.watch", json!({"channelId": topic.id, "on": true}))
            .await;
        let r = run(&f, &f.worker, "list_channels", json!({})).await;
        assert!(!r.is_error, "{}", r.text);
        let rows: Vec<&str> = r.text.lines().skip(1).collect();
        assert!(rows[0].contains(&human.id), "{}", r.text);
        assert!(rows[0].contains(&text::r(Locale::Zh, "r.flag_home_human", &[])));
        let l = lobby(&f).await;
        assert!(rows.iter().any(|x| x.contains(&l)), "{}", r.text);
        let t = rows
            .iter()
            .find(|x| x.contains(&topic.id))
            .expect("crewmate's topic");
        assert!(
            t.contains(&text::r(Locale::Zh, "r.flag_watched", &[])),
            "{t}"
        );
        assert!(!r.text.contains(&archived.id), "archived hidden");
        let r = run(
            &f,
            &f.worker,
            "list_channels",
            json!({"include_archived": true}),
        )
        .await;
        assert!(r.text.contains(&archived.id), "{}", r.text);
        // crew = that crew's readable channels.
        let other = f.add_crew("示例外组", None).await;
        let n = number_of(&f, &other).await;
        let r = run(&f, &f.worker, "list_channels", json!({"crew": n})).await;
        assert!(!r.is_error, "{}", r.text);
        assert!(
            !r.text.contains(&topic.id) && !r.text.contains(&l),
            "{}",
            r.text
        );
    }

    // ---- Fix round 1 ----

    fn human_channel_id(crew: &str) -> String {
        crewd_store::channels::human_id(crew)
    }

    #[tokio::test]
    async fn posting_into_the_home_human_channel_joins_it_and_the_receipt_says_member() {
        let f = Fixture::new().await;
        let human = port::home_human_channel(&f.state, &f.crew).await.unwrap();
        let child = f.add_crew("示例子组", Some(&f.crew)).await;
        f.add_session(&child, "s-ch", Role::Worker, "子组 worker")
            .await;
        for who in [f.worker.clone(), "s-ch".to_string()] {
            let r = run(
                &f,
                &who,
                "post",
                json!({"channel": human.id, "message": format!("示例：{who} 报到")}),
            )
            .await;
            assert!(!r.is_error, "{who}: {}", r.text);
            assert!(
                !shows(&r.text, "r.posted_not_member", &["channel"]),
                "{who} joined by posting: {}",
                r.text
            );
            let ch = port::channel(&f.state, &human.id).await.unwrap().unwrap();
            assert!(ch.has_session(&who), "{who} is a member afterwards");
        }
    }

    #[tokio::test]
    async fn concurrent_identical_posts_are_one_post_and_one_duplicate() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let me = f.me(&f.worker).await;
        let args = json!({"channel": l, "message": "示例：同一句"});
        let (a, b) = tokio::join!(
            call(&f.state, &me, "post", args.clone()),
            call(&f.state, &me, "post", args.clone())
        );
        let errors: Vec<_> = [&a, &b].into_iter().filter(|r| r.is_error).collect();
        assert_eq!(errors.len(), 1, "{} / {}", a.text, b.text);
        assert!(
            shows(&errors[0].text, "r.post_duplicate", &["seq"]),
            "{}",
            errors[0].text
        );
        let texts: Vec<_> = port::messages_raw(&f.state, &l, None, 10)
            .await
            .unwrap()
            .into_iter()
            .filter(|m| m.text == "示例：同一句")
            .collect();
        assert_eq!(texts.len(), 1);
    }

    #[tokio::test]
    async fn a_batch_repeating_its_own_text_is_refused_and_names_the_item() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let before = port::latest_seq(&f.state, &l).await.unwrap();
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "messages": [{"text": "示例 a"}, {"text": "示例 b"}, {"text": " 示例   a "}]}),
        )
        .await;
        assert!(r.is_error);
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.post_repeated", &[("index", "3")])
        );
        assert_eq!(port::latest_seq(&f.state, &l).await.unwrap(), before);
    }

    #[tokio::test]
    async fn the_wake_receipt_leaves_out_me_and_the_human() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let none = text::r(Locale::Zh, "r.posted_wakes_none", &[]);
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例 @人", "mentions": [{"kind": "human"}]}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert!(r.text.contains(&none), "@human is not 'woken': {}", r.text);
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例 @我", "mentions": [{"kind": "session", "target_id": f.worker}]}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert!(r.text.contains(&none), "@me wakes no one: {}", r.text);
    }

    #[tokio::test]
    async fn the_latest_page_does_not_mark_read_past_older_unread_messages() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        for i in 0..120 {
            f.human_post(&l, &format!("示例积压{i:03}"), vec![]).await;
        }
        let me = f.me(&f.worker).await;
        let r = run(&f, &f.worker, "read_channel", json!({})).await;
        assert!(
            r.text.contains("示例积压119") && !r.text.contains("示例积压069"),
            "{}",
            r.text
        );
        assert!(shows(&r.text, "r.read_more", &["before"]), "{}", r.text);
        let pending = crate::agent::inject::prepare(&f.state, &me)
            .await
            .unwrap()
            .and_then(|p| p.context)
            .unwrap_or_default();
        assert!(
            pending.contains("示例积压000"),
            "older unread stays unread: {pending}"
        );
        let r = run(&f, &f.worker, "read_channel", json!({"limit": 200})).await;
        assert!(r.text.contains("示例积压000"), "{}", r.text);
        assert!(
            !shows(&r.text, "r.read_more", &["before"]),
            "no older ones: {}",
            r.text
        );
        assert!(
            crate::agent::inject::prepare(&f.state, &me)
                .await
                .unwrap()
                .is_none(),
            "the whole backlog was delivered: read"
        );
    }

    #[tokio::test]
    async fn read_more_only_when_older_messages_exist() {
        let f = Fixture::new().await;
        let cap = f.me(&f.captain).await;
        let topic = port::create_channel(
            &f.state,
            &cap,
            ChannelKind::Topic,
            "示例话题".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
        f.session_post(&f.captain, &topic.id, "示例 1", vec![])
            .await;
        f.session_post(&f.captain, &topic.id, "示例 2", vec![])
            .await;
        let r = run(
            &f,
            &f.worker,
            "read_channel",
            json!({"channel": topic.id, "limit": 2}),
        )
        .await;
        assert!(r.text.contains("示例 1"), "{}", r.text);
        assert!(!shows(&r.text, "r.read_more", &["before"]), "{}", r.text);
        let r = run(
            &f,
            &f.worker,
            "read_channel",
            json!({"channel": topic.id, "limit": 1}),
        )
        .await;
        assert!(shows(&r.text, "r.read_more", &["before"]), "{}", r.text);
    }

    #[tokio::test]
    async fn a_batch_failing_part_way_refunds_the_unposted_items() {
        let (f, _now) = with_clock(AgentConfig::default()).await;
        let l = lobby(&f).await;
        let huge = "a".repeat(300 * 1024);
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "messages": [{"text": "示例 前"}, {"text": huge}, {"text": "示例 后"}]}),
        )
        .await;
        assert!(r.is_error);
        assert!(
            shows(
                &r.text,
                "r.post_partial",
                &["sent", "index", "total", "detail"]
            ),
            "{}",
            r.text
        );
        // One of six tokens was used: five more posts go, the sixth is throttled.
        for i in 0..5 {
            let r = run(
                &f,
                &f.worker,
                "post",
                json!({"channel": l, "message": format!("示例 {i}")}),
            )
            .await;
            assert!(!r.is_error, "{i}: {}", r.text);
        }
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例 多"}),
        )
        .await;
        assert!(
            shows(&r.text, "r.post_throttled", &["retry_secs"]),
            "{}",
            r.text
        );
    }

    #[tokio::test]
    async fn to_human_points_to_the_home_human_channel_and_dms_are_member_only() {
        let f = Fixture::new().await;
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"to": "human", "message": "示例"}),
        )
        .await;
        assert!(r.is_error);
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.post_to_human_no_channel", &[])
        );
        let human = port::home_human_channel(&f.state, &f.crew).await.unwrap();
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"to": "Human", "message": "示例"}),
        )
        .await;
        assert!(r.is_error);
        assert!(
            shows(&r.text, "r.post_to_human", &["home", "home_id"]),
            "{}",
            r.text
        );
        assert!(r.text.contains(&human.id), "{}", r.text);
        // A management session cannot post into someone else's dm: the dm is member-only.
        let mgmt = management_crew(&f).await;
        f.add_session(&mgmt, "s-mgr", Role::Worker, "管理层 worker")
            .await;
        let w = f.me(&f.worker).await;
        let (dm, _) = port::ensure_dm(&f.state, &w, &f.captain).await.unwrap();
        let r = run(
            &f,
            "s-mgr",
            "post",
            json!({"channel": dm.id, "message": "示例"}),
        )
        .await;
        assert!(r.is_error);
        assert!(
            shows(&r.text, "r.dm_member_only", &["channel"]),
            "{}",
            r.text
        );
    }

    #[tokio::test]
    async fn list_channels_and_receipts_do_not_create_the_home_human_channel() {
        let f = Fixture::new().await;
        let hid = human_channel_id(&f.crew);
        let r = run(&f, &f.worker, "list_channels", json!({})).await;
        assert!(!r.is_error, "{}", r.text);
        let first = r.text.lines().nth(1).unwrap_or_default();
        assert_eq!(
            first,
            text::r(Locale::Zh, "r.home_human_absent", &[]),
            "{}",
            r.text
        );
        let l = lobby(&f).await;
        let r = run(
            &f,
            &f.worker,
            "post",
            json!({"channel": l, "message": "示例 @人", "mentions": [{"kind": "human"}]}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert!(
            shows(&r.text, "r.human_not_watching_no_home", &["channel"]),
            "{}",
            r.text
        );
        assert!(
            port::channel(&f.state, &hid).await.unwrap().is_none(),
            "not created"
        );
    }

    #[tokio::test]
    async fn list_channels_of_a_crew_with_nothing_readable_says_so() {
        let f = Fixture::new().await;
        let other = f.add_crew("示例外组", None).await;
        let n = number_of(&f, &other).await;
        let r = run(&f, &f.worker, "list_channels", json!({"crew": n})).await;
        assert!(!r.is_error, "{}", r.text);
        assert!(
            r.text.contains(&text::r(Locale::Zh, "r.list_none", &[])),
            "{}",
            r.text
        );
    }
}
