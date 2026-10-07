//! Channel and directory tools (spec §22.3, §22.7 Q2; T13 supplement): `channel_create`,
//! `channel_invite`, `channel_leave`, `channel_archive`, `channel_owner`, `channel_listen`,
//! `directory`.
//!
//! Every channel rule (who may add people, leave, archive, offer / accept / force ownership,
//! listen) is collab's (C1): these tools parse arguments, call the port and write receipts; a
//! refusal reaches the agent through `Ctx::fail` with collab's code. A dm is not created here: it
//! only comes from `post.to` (S65).
use crewd_proto::agent::CallResult;
use crewd_proto::{Mention, MentionKind};
use serde::Deserialize;
use serde_json::Value;

use super::{Ctx, channel_label, parse_args, post_as, resolve_channel};
use crate::agent::port::{self, ChannelKind, MemberRef, PostSender};

/// Longest accepted `channel_listen.minutes` (8 hours).
const LISTEN_MAX_MINUTES: i64 = 480;
/// How the human is stored among a listen's `senders` (session ids otherwise).
const HUMAN_SENDER: &str = "human";
/// A listen `senders` entry that follows the captain: whoever is the captain when a message is
/// posted (the M3c wake matcher, `wake::plan::Sender::matches_filter`). Stored as is, unlike an
/// address such as `7-1`, which is frozen to its session when the listen is set.
const CAPTAIN_SENDER: &str = "captain";

/// `Result` with the receipt on both sides: `?` returns the error receipt as it is.
type Step<T> = Result<T, CallResult>;

fn done(r: Step<CallResult>) -> CallResult {
    r.unwrap_or_else(|e| e)
}

/// A member argument: `human` (or the human's label) = the human; anything else names a session,
/// as an address (`7-3`, `7` = crew 7's captain now, `0`) or an existing session id (C3). Refusals
/// carry collab's codes (`directory.*`, `session.not_found`).
pub async fn resolve_member(cx: &Ctx<'_>, s: &str) -> Step<MemberRef> {
    if port::names_the_human(s) {
        return Ok(MemberRef::Human);
    }
    port::resolve_session(cx.st, s)
        .await
        .map(MemberRef::Session)
        .map_err(|e| cx.fail(e))
}

/// All member arguments, in order, without duplicates and (when `drop_me`) without me.
async fn resolve_members(cx: &Ctx<'_>, args: &[String], drop_me: bool) -> Step<Vec<MemberRef>> {
    let mut out: Vec<MemberRef> = Vec::new();
    for a in args {
        let m = resolve_member(cx, a).await?;
        let skip_me = drop_me && matches!(&m, MemberRef::Session(s) if *s == cx.me.session_id);
        if !skip_me && !out.contains(&m) {
            out.push(m);
        }
    }
    Ok(out)
}

async fn label(cx: &Ctx<'_>, m: &MemberRef) -> Step<String> {
    port::member_label(cx.st, m, cx.loc)
        .await
        .map_err(|e| cx.fail(e))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    name: String,
    purpose: Option<String>,
    #[serde(default)]
    members: Vec<String>,
}

pub async fn create(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(create_(cx, v).await)
}

async fn create_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: CreateArgs = parse_args(cx, v)?;
    let members = resolve_members(cx, &a.members, true).await?;
    let purpose = a.purpose.filter(|p| !p.trim().is_empty());
    let ch = port::create_channel(
        cx.st,
        cx.me,
        ChannelKind::Topic,
        a.name.trim().to_string(),
        purpose,
        members,
    )
    .await
    .map_err(|e| cx.fail(e))?;
    Ok(cx.ok(
        "r.channel_created",
        &[
            ("id", &ch.id),
            ("name", &channel_label(cx, &ch).await),
            ("count", &ch.members.len().to_string()),
        ],
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteArgs {
    channel: String,
    members: Vec<String>,
}

pub async fn invite(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(invite_(cx, v).await)
}

async fn invite_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: InviteArgs = parse_args(cx, v)?;
    let ch = resolve_channel(cx, &a.channel).await?;
    let members = resolve_members(cx, &a.members, false).await?;
    let ch = port::add_members(cx.st, cx.me, &ch.id, members)
        .await
        .map_err(|e| cx.fail(e))?;
    Ok(cx.ok(
        "r.invited",
        &[
            ("channel", &channel_label(cx, &ch).await),
            ("count", &ch.members.len().to_string()),
        ],
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelArg {
    channel: String,
}

pub async fn leave(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(leave_(cx, v).await)
}

async fn leave_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: ChannelArg = parse_args(cx, v)?;
    let ch = resolve_channel(cx, &a.channel).await?;
    let me = MemberRef::Session(cx.me.session_id.clone());
    port::remove_member(cx.st, cx.me, &ch.id, me)
        .await
        .map_err(|e| cx.fail(e))?;
    Ok(cx.ok("r.left", &[("channel", &channel_label(cx, &ch).await)]))
}

pub async fn archive(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(archive_(cx, v).await)
}

async fn archive_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: ChannelArg = parse_args(cx, v)?;
    let ch = resolve_channel(cx, &a.channel).await?;
    port::archive_channel(cx.st, cx.me, &ch.id)
        .await
        .map_err(|e| cx.fail(e))?;
    Ok(cx.ok("r.archived", &[("channel", &channel_label(cx, &ch).await)]))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerArgs {
    channel: String,
    to: Option<String>,
    #[serde(default)]
    force: bool,
}

/// No `to` = accept the offer to me; `to` = offer (the owner, collab decides); `to` + `force` =
/// move it now (the captain of the channel's crew, management captain included: collab decides).
pub async fn owner(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(owner_(cx, v).await)
}

async fn owner_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: OwnerArgs = parse_args(cx, v)?;
    let ch = resolve_channel(cx, &a.channel).await?;
    let fail = |e| cx.fail(e);
    let Some(to_arg) = a.to else {
        if a.force {
            let detail = cx.r("r.owner_force_needs_to", &[]);
            return Err(cx.err("r.bad_args", &[("detail", &detail)]));
        }
        port::accept_owner_transfer(cx.st, cx.me, &ch.id)
            .await
            .map_err(fail)?;
        return Ok(cx.ok(
            "r.owner_accepted",
            &[("channel", &channel_label(cx, &ch).await)],
        ));
    };
    let to = resolve_member(cx, &to_arg).await?;
    let to_label = label(cx, &to).await?;
    let channel = channel_label(cx, &ch).await;
    if a.force {
        port::force_owner(cx.st, cx.me, &ch.id, &to)
            .await
            .map_err(fail)?;
        return Ok(cx.ok(
            "r.owner_forced",
            &[("channel", &channel), ("to", &to_label)],
        ));
    }
    port::request_owner_transfer(cx.st, cx.me, &ch.id, &to)
        .await
        .map_err(fail)?;
    // The offer is said in the channel by the owner (no system event, S60): it wakes the new owner,
    // and a reply reaches me.
    let mention = match &to {
        MemberRef::Session(s) => Mention {
            kind: MentionKind::Session,
            target_id: Some(s.clone()),
        },
        MemberRef::Human => Mention {
            kind: MentionKind::Human,
            target_id: None,
        },
    };
    // The human accepts in the app (channel menu), not with a tool.
    let notice_key = match &to {
        MemberRef::Human => "r.owner_offer_notice_human",
        MemberRef::Session(_) => "r.owner_offer_notice",
    };
    let notice = cx.r(notice_key, &[("channel", &channel), ("id", &ch.id)]);
    post_as(
        cx,
        &ch.id,
        notice,
        vec![mention],
        None,
        None,
        PostSender::Session(cx.me.session_id.clone()),
    )
    .await
    .map_err(fail)?;
    Ok(cx.ok(
        "r.owner_offered",
        &[("channel", &channel), ("to", &to_label), ("id", &ch.id)],
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListenArgs {
    channel: String,
    minutes: Option<i64>,
    senders: Option<Vec<String>>,
    #[serde(default)]
    off: bool,
}

pub async fn listen(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(listen_(cx, v).await)
}

async fn listen_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: ListenArgs = parse_args(cx, v)?;
    if a.off && (a.minutes.is_some() || a.senders.is_some()) {
        let detail = cx.r("r.listen_off_alone", &[]);
        return Err(cx.err("r.bad_args", &[("detail", &detail)]));
    }
    // A channel I cannot read is "no such channel", same as one that does not exist (collab's
    // `listen.not_readable` stays as defence in depth).
    let ch = resolve_channel(cx, &a.channel).await?;
    let channel = channel_label(cx, &ch).await;
    if a.off {
        let removed = port::clear_listen(cx.st, &cx.me.session_id, &ch.id)
            .await
            .map_err(|e| cx.fail(e))?;
        let key = if removed == 0 {
            "r.unlisten_none"
        } else {
            "r.unlistened"
        };
        return Ok(cx.ok(key, &[("channel", &channel), ("id", &ch.id)]));
    }
    let until = match a.minutes {
        None => None,
        Some(m) if (1..=LISTEN_MAX_MINUTES).contains(&m) => Some(port::now_ms() + m * 60_000),
        Some(_) => return Err(cx.err("r.listen_minutes", &[])),
    };
    // Senders are stored canonically: session ids, `human`, or `captain` (dynamic).
    let mut senders: Vec<String> = Vec::new();
    for arg in a.senders.as_deref().unwrap_or(&[]) {
        let one = if arg.trim().eq_ignore_ascii_case(CAPTAIN_SENDER) {
            CAPTAIN_SENDER.to_string()
        } else {
            match resolve_member(cx, arg).await? {
                MemberRef::Session(s) => s,
                MemberRef::Human => HUMAN_SENDER.to_string(),
            }
        };
        if !senders.contains(&one) {
            senders.push(one);
        }
    }
    port::set_listen(cx.st, &cx.me.session_id, &ch.id, until, senders)
        .await
        .map_err(|e| cx.fail(e))?;
    Ok(match a.minutes {
        Some(m) => cx.ok(
            "r.listening_for",
            &[("channel", &channel), ("minutes", &m.to_string())],
        ),
        None => cx.ok("r.listening", &[("channel", &channel), ("id", &ch.id)]),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirArgs {
    query: Option<String>,
}

/// The directory (collab `directory::list`): crews by number, each with its captain and numbered
/// members; `0` is the management crew; removed crews are not listed. Channels are
/// `list_channels`'s (S70, B11 superseded).
pub async fn directory(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(directory_(cx, v).await)
}

async fn directory_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: DirArgs = parse_args(cx, v)?;
    let fail = |e| cx.fail(e);
    let query = a.query.filter(|q| !q.trim().is_empty());
    let rows = port::directory(cx.st, query).await.map_err(fail)?;
    let mine = port::sessions_of(cx.st, &cx.me.crew_id)
        .await
        .map_err(fail)?
        .into_iter()
        .find(|s| s.id == cx.me.session_id)
        .and_then(|s| s.extension);
    let mut out = vec![match mine {
        Some(n) => cx.r("r.dir_header", &[("me", &n)]),
        None => cx.r("r.dir_header_no_number", &[("id", &cx.me.session_id)]),
    }];
    for r in &rows {
        out.push(match &r.session_id {
            None => {
                let path = if r.detail.is_empty() {
                    String::new()
                } else {
                    cx.r("r.dir_path", &[("path", &r.detail)])
                };
                cx.r(
                    "r.dir_crew",
                    &[
                        ("number", &r.number),
                        ("name", &cx.name(&r.name)),
                        ("path", &path),
                    ],
                )
            }
            Some(id) => cx.r(
                "r.dir_session",
                &[
                    ("number", &r.number),
                    ("name", &cx.name(&r.name)),
                    ("crew", &cx.name(&r.crew_title)),
                    ("id", id),
                ],
            ),
        });
    }
    if rows.is_empty() {
        out.push(cx.r("r.dir_empty", &[]));
    }
    Ok(CallResult::ok(out.join("\n")))
}

#[cfg(test)]
mod tests {
    use crewd_collab::refusal::{Code, Refusal};
    use crewd_proto::{MentionKind, SenderKind};
    use serde_json::json;

    use crate::agent::Role;
    use crate::agent::catalog;
    use crate::agent::port::{self, ChannelKind, ChannelView};
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

    fn rendered_refusal(r: Refusal) -> String {
        text::r(Locale::Zh, "r.failed", &[("detail", &r.text(Locale::Zh))])
    }

    /// The call was refused by collab with exactly this rule (C1: the code, not a sentence).
    #[track_caller]
    fn refused(r: &crewd_proto::agent::CallResult, rule: Refusal) {
        assert!(r.is_error, "expected a refusal, got: {}", r.text);
        assert_eq!(r.text, rendered_refusal(rule));
    }

    /// The fixed parts of a template (placeholders replaced by a marker) all appear in `text`.
    fn shows(text: &str, key: &str, vars: &[&str]) -> bool {
        let filled: Vec<(&str, &str)> = vars.iter().map(|v| (*v, "\u{1}")).collect();
        text::r(Locale::Zh, key, &filled)
            .split('\u{1}')
            .filter(|part| !part.is_empty())
            .all(|part| text.contains(part))
    }

    async fn channel(f: &Fixture, id: &str) -> ChannelView {
        port::channel(&f.state, id).await.unwrap().unwrap()
    }

    async fn topic_by_captain(f: &Fixture) -> ChannelView {
        let r = run(
            f,
            &f.captain,
            "channel_create",
            json!({"name": "示例话题", "purpose": "演示", "members": [f.worker]}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        port::crew_channels(&f.state, &f.crew)
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.name == "示例话题")
            .unwrap()
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

    async fn management_crew(f: &Fixture) -> String {
        f.state
            .db
            .call(|s| crewd_collab::crews::management(s.conn()))
            .await
            .unwrap()
            .unwrap()
            .expect("crew.create creates the management crew")
    }

    #[tokio::test]
    async fn create_topic_only_and_creator_owns_it() {
        let f = Fixture::new().await;
        let x = f.register(&f.crew, Role::Worker, "示例新人").await;
        let ext = extension_of(&f, &f.crew, &x).await;
        let r = run(
            &f,
            &f.captain,
            "channel_create",
            // Myself and a duplicate are dropped; an address resolves to its session.
            json!({"name": "示例话题", "members": [f.worker, f.worker, f.captain, ext]}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        let t = port::crew_channels(&f.state, &f.crew)
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.name == "示例话题")
            .unwrap();
        assert!(
            shows(&r.text, "r.channel_created", &["id", "name", "count"]) && r.text.contains(&t.id),
            "{}",
            r.text
        );
        assert_eq!(t.kind, ChannelKind::Topic);
        assert_eq!(t.owner.as_deref(), Some(f.captain.as_str()), "建群者是群主");
        assert_eq!(t.members.len(), 3, "{:?}", t.members);
        assert!(t.has_session(&f.worker) && t.has_session(&f.captain) && t.has_session(&x));
        // Members are optional: just me.
        let r = run(&f, &f.worker, "channel_create", json!({"name": "示例独角"})).await;
        assert!(!r.is_error, "{}", r.text);
        let solo = port::crew_channels(&f.state, &f.crew)
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.name == "示例独角")
            .unwrap();
        assert_eq!(solo.members.len(), 1);
        // No dm from here (S65): `kind` is no parameter any more.
        let r = run(
            &f,
            &f.worker,
            "channel_create",
            json!({"name": "示例私聊", "members": [f.captain], "kind": "dm"}),
        )
        .await;
        assert!(r.is_error && r.text.contains("kind"), "{}", r.text);
        // A non-address that is no session: collab's session.not_found.
        let r = run(
            &f,
            &f.worker,
            "channel_create",
            json!({"name": "示例", "members": ["s-nobody"]}),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::SessionNotFound).arg("session", "s-nobody"),
        );
    }

    #[tokio::test]
    async fn invite_leave_archive_follow_collab_rules() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let t = topic_by_captain(&f).await;
        // s-w2 reads the topic (its crew's channel) but is not in it.
        let r = run(
            &f,
            "s-w2",
            "channel_invite",
            json!({"channel": t.id, "members": ["s-w2"]}),
        )
        .await;
        refused(&r, Refusal::new(Code::ChannelAddMembersMemberOnly));
        let r = run(
            &f,
            &f.worker,
            "channel_invite",
            json!({"channel": "示例话题", "members": ["s-w2"]}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert!(
            shows(&r.text, "r.invited", &["channel", "count"]),
            "{}",
            r.text
        );
        assert!(channel(&f, &t.id).await.has_session("s-w2"));
        let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap();
        let r = run(&f, &f.worker, "channel_leave", json!({"channel": lobby.id})).await;
        refused(&r, Refusal::new(Code::ChannelLobbyCannotRemove));
        let r = run(
            &f,
            &f.captain,
            "channel_archive",
            json!({"channel": lobby.id}),
        )
        .await;
        refused(&r, Refusal::new(Code::ChannelFixedNoArchive));
        let r = run(&f, &f.worker, "channel_archive", json!({"channel": t.id})).await;
        refused(&r, Refusal::new(Code::ChannelManageForbidden));
        let r = run(&f, "s-w2", "channel_leave", json!({"channel": t.id})).await;
        assert!(!r.is_error, "{}", r.text);
        assert!(!channel(&f, &t.id).await.has_session("s-w2"));
        let r = run(&f, &f.captain, "channel_archive", json!({"channel": t.id})).await;
        assert!(!r.is_error, "{}", r.text);
        assert!(channel(&f, &t.id).await.archived);
    }

    #[tokio::test]
    async fn ownership_offer_accept_and_captain_force() {
        let f = Fixture::new().await;
        let t = topic_by_captain(&f).await;
        let owner = |f: &Fixture| {
            let id = t.id.clone();
            let st = f.state.clone();
            async move { port::channel(&st, &id).await.unwrap().unwrap().owner }
        };
        let r = run(
            &f,
            &f.worker,
            "channel_owner",
            json!({"channel": t.id, "to": f.worker}),
        )
        .await;
        refused(&r, Refusal::new(Code::ChannelTransferOwnerOnly));
        let r = run(&f, &f.worker, "channel_owner", json!({"channel": t.id})).await;
        refused(&r, Refusal::new(Code::ChannelNoPendingTransfer));
        // The owner offers: posted as the owner, mentioning the new owner (no system notice, S60).
        let r = run(
            &f,
            &f.captain,
            "channel_owner",
            json!({"channel": t.id, "to": f.worker}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert!(
            shows(&r.text, "r.owner_offered", &["channel", "to", "id"]),
            "{}",
            r.text
        );
        let notice = port::messages_raw(&f.state, &t.id, None, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_ne!(notice.sender_kind, SenderKind::System, "said by the owner");
        assert_eq!(
            notice.sender_session_id.as_deref(),
            Some(f.captain.as_str())
        );
        assert!(
            notice
                .mentions
                .iter()
                .any(|m| m.kind == MentionKind::Session
                    && m.target_id.as_deref() == Some(f.worker.as_str())),
            "{:?}",
            notice.mentions
        );
        assert!(notice.text.contains("channel_owner"), "{}", notice.text);
        assert_eq!(
            owner(&f).await.as_deref(),
            Some(f.captain.as_str()),
            "接受前不变"
        );
        let r = run(&f, &f.worker, "channel_owner", json!({"channel": t.id})).await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(owner(&f).await.as_deref(), Some(f.worker.as_str()));
        // The captain of the channel's crew forces it back.
        let r = run(
            &f,
            &f.captain,
            "channel_owner",
            json!({"channel": t.id, "to": f.captain, "force": true}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(owner(&f).await.as_deref(), Some(f.captain.as_str()));
        let r = run(
            &f,
            &f.worker,
            "channel_owner",
            json!({"channel": t.id, "to": f.worker, "force": true}),
        )
        .await;
        refused(&r, Refusal::new(Code::ChannelForceOwnerForbidden));
        let r = run(
            &f,
            &f.captain,
            "channel_owner",
            json!({"channel": t.id, "force": true}),
        )
        .await;
        assert_eq!(
            r.text,
            text::r(
                Locale::Zh,
                "r.bad_args",
                &[(
                    "detail",
                    &text::r(Locale::Zh, "r.owner_force_needs_to", &[])
                )]
            )
        );
        // The management captain counts as captain of every crew (C2).
        let mgmt = management_crew(&f).await;
        f.add_session(&mgmt, "s-mcap", Role::Captain, "管理层机长")
            .await;
        let r = run(
            &f,
            "s-mcap",
            "channel_owner",
            json!({"channel": t.id, "to": f.worker, "force": true}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(owner(&f).await.as_deref(), Some(f.worker.as_str()));
    }

    #[tokio::test]
    async fn listen_then_off_is_idempotent() {
        let f = Fixture::new().await;
        let cap = f.me(&f.captain).await;
        let t = port::create_channel(
            &f.state,
            &cap,
            ChannelKind::Topic,
            "旁听".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
        let r = run(
            &f,
            &f.worker,
            "channel_listen",
            json!({"channel": t.id, "minutes": 0}),
        )
        .await;
        assert_eq!(r.text, text::r(Locale::Zh, "r.listen_minutes", &[]));
        // The captain given by its address is stored as its session id; human as `human`.
        let n = number_of(&f, &f.crew).await;
        let r = run(
            &f,
            &f.worker,
            "channel_listen",
            json!({"channel": "旁听", "minutes": 30, "senders": ["human", format!("{n}-1")]}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert!(
            shows(&r.text, "r.listening_for", &["channel", "minutes"]),
            "{}",
            r.text
        );
        let id = t.id.clone();
        let rows = f
            .state
            .db
            .call(move |s| crewd_collab::listens::active_for_channel(s.conn(), &id, port::now_ms()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].senders, ["human".to_string(), f.captain.clone()]);
        assert!(
            port::my_channels(&f.state, &f.worker)
                .await
                .unwrap()
                .iter()
                .any(|c| c.id == t.id && c.listening)
        );
        let r = run(
            &f,
            &f.worker,
            "channel_listen",
            json!({"channel": t.id, "off": true, "minutes": 5}),
        )
        .await;
        assert_eq!(
            r.text,
            text::r(
                Locale::Zh,
                "r.bad_args",
                &[("detail", &text::r(Locale::Zh, "r.listen_off_alone", &[]))]
            )
        );
        let r = run(
            &f,
            &f.worker,
            "channel_listen",
            json!({"channel": t.id, "off": true}),
        )
        .await;
        assert!(
            !r.is_error && shows(&r.text, "r.unlistened", &["channel", "id"]),
            "{}",
            r.text
        );
        assert!(
            !port::my_channels(&f.state, &f.worker)
                .await
                .unwrap()
                .iter()
                .any(|c| c.id == t.id)
        );
        let r = run(
            &f,
            &f.worker,
            "channel_listen",
            json!({"channel": t.id, "off": true}),
        )
        .await;
        assert!(
            !r.is_error && shows(&r.text, "r.unlisten_none", &["channel"]),
            "idempotent: {}",
            r.text
        );
    }

    /// I1: an unreadable channel is "no such channel", word for word the same as an unknown id,
    /// for listen (set and off) and for every other channel tool; nothing is written.
    #[tokio::test]
    async fn listen_on_an_unreadable_channel_is_no_such_channel() {
        let f = Fixture::new().await;
        let other = f.add_crew("示例外组", None).await;
        f.add_session(&other, "s-cap2", Role::Captain, "外组机长")
            .await;
        let cap2 = f.me("s-cap2").await;
        let t = port::create_channel(
            &f.state,
            &cap2,
            ChannelKind::Topic,
            "外组话题".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
        // Archived, too: its state must not leak either.
        let archived = port::create_channel(
            &f.state,
            &cap2,
            ChannelKind::Topic,
            "外组旧话题".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
        port::archive_channel(&f.state, &cap2, &archived.id)
            .await
            .unwrap();
        let unknown = "ch-no-such-channel";
        let calls = |id: &str| {
            vec![
                ("channel_listen", json!({"channel": id})),
                ("channel_listen", json!({"channel": id, "off": true})),
                (
                    "channel_invite",
                    json!({"channel": id, "members": ["s-cap"]}),
                ),
                ("channel_archive", json!({"channel": id})),
                ("channel_owner", json!({"channel": id, "to": "s-cap"})),
                ("channel_owner", json!({"channel": id})),
                ("channel_leave", json!({"channel": id})),
            ]
        };
        let marker = "\u{1}";
        let baseline = run(&f, &f.worker, "channel_listen", json!({"channel": unknown})).await;
        assert!(
            baseline.is_error && shows(&baseline.text, "r.no_channel", &["arg", "choices"]),
            "{}",
            baseline.text
        );
        let want = baseline.text.replace(unknown, marker);
        for id in [unknown, t.id.as_str(), archived.id.as_str()] {
            for (tool, args) in calls(id) {
                let r = run(&f, &f.worker, tool, args).await;
                assert!(r.is_error, "{tool} {id}: {}", r.text);
                assert_eq!(r.text.replace(id, marker), want, "{tool} on {id}");
            }
        }
        let rows: i64 = f
            .state
            .db
            .call(|s| {
                Ok::<_, crewd_collab::CollabError>(
                    s.conn()
                        .query_row(
                            "SELECT COUNT(*) FROM listens WHERE session_id='s-w1'",
                            [],
                            |r| r.get(0),
                        )
                        .unwrap(),
                )
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rows, 0, "no listen row written");
    }

    /// M2: `captain` is stored as is (dynamic: whoever is captain when a message is posted, the
    /// M3c wake matcher), while an address is frozen to its session when the listen is set.
    #[tokio::test]
    async fn listen_senders_captain_is_dynamic_and_an_address_is_frozen() {
        let f = Fixture::new().await;
        let l = port::lobby_of(&f.state, &f.crew).await.unwrap();
        let n = number_of(&f, &f.crew).await;
        let other = f.add_crew("示例外组", None).await;
        f.add_session(&other, "s-cap2", Role::Captain, "外组机长")
            .await;
        let r = run(
            &f,
            &f.worker,
            "channel_listen",
            json!({"channel": l.id, "senders": ["Captain", "captain", format!("{n}-1")]}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        let id = l.id.clone();
        let rows = f
            .state
            .db
            .call(move |s| crewd_collab::listens::active_for_channel(s.conn(), &id, port::now_ms()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            rows[0].senders,
            ["captain".to_string(), f.captain.clone()],
            "captain kept literal (deduplicated), 7-1 frozen to the session"
        );
    }

    /// M4: an offer to the human posts the human's own notice (accept in the app), never a tool call.
    #[tokio::test]
    async fn ownership_offer_to_the_human_points_to_the_app() {
        let f = Fixture::new().await;
        let r = run(
            &f,
            &f.captain,
            "channel_create",
            json!({"name": "示例人类话题", "members": ["human"]}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        let t = port::crew_channels(&f.state, &f.crew)
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.name == "示例人类话题")
            .unwrap();
        let r = run(
            &f,
            &f.captain,
            "channel_owner",
            json!({"channel": t.id, "to": "human"}),
        )
        .await;
        assert!(
            !r.is_error,
            "collab accepts an offer to the human: {}",
            r.text
        );
        let notice = port::messages_raw(&f.state, &t.id, None, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let label = crate::agent::tools::channel_label_raw(
            Locale::Zh,
            ChannelKind::Topic,
            "示例人类话题",
            "",
        );
        assert_eq!(
            notice.text,
            text::r(
                Locale::Zh,
                "r.owner_offer_notice_human",
                &[("channel", &label), ("id", &t.id)]
            )
        );
        assert!(!notice.text.contains("channel_owner"), "{}", notice.text);
        assert!(
            notice.mentions.iter().any(|m| m.kind == MentionKind::Human),
            "{:?}",
            notice.mentions
        );
    }

    /// M6: an ordinary management-crew session reads the whole organisation but has no captain
    /// powers over another crew's channels; the management captain does.
    #[tokio::test]
    async fn an_ordinary_management_session_cannot_force_or_archive_another_crews_channel() {
        let f = Fixture::new().await;
        let t = topic_by_captain(&f).await;
        let mgmt = management_crew(&f).await;
        f.add_session(&mgmt, "s-mcap", Role::Captain, "管理层机长")
            .await;
        f.add_session(&mgmt, "s-mw", Role::Worker, "管理层成员")
            .await;
        let r = run(
            &f,
            "s-mw",
            "channel_owner",
            json!({"channel": t.id, "to": f.worker, "force": true}),
        )
        .await;
        refused(&r, Refusal::new(Code::ChannelForceOwnerForbidden));
        let r = run(&f, "s-mw", "channel_archive", json!({"channel": t.id})).await;
        refused(&r, Refusal::new(Code::ChannelManageForbidden));
        assert!(!channel(&f, &t.id).await.archived);
        let r = run(&f, "s-mcap", "channel_archive", json!({"channel": t.id})).await;
        assert!(!r.is_error, "{}", r.text);
        assert!(channel(&f, &t.id).await.archived);
    }

    #[tokio::test]
    async fn directory_lists_numbers_and_the_management_crew() {
        let f = Fixture::new().await;
        let x = f.register(&f.crew, Role::Worker, "示例新人").await;
        let ext = extension_of(&f, &f.crew, &x).await;
        let gone = f.add_crew("示例已移除", None).await;
        f.rpc("crew.remove", json!({"crewId": gone})).await;
        let mgmt = management_crew(&f).await;
        let mgmt_title = port::crew(&f.state, &mgmt).await.unwrap().unwrap().title;
        let n = number_of(&f, &f.crew).await;
        let r = run(&f, &x, "directory", json!({})).await;
        assert!(!r.is_error, "{}", r.text);
        let lines: Vec<&str> = r.text.lines().collect();
        assert_eq!(
            lines[0],
            text::r(Locale::Zh, "r.dir_header", &[("me", &ext)]),
            "my own number first"
        );
        let crew_line = |number: &str, name: &str| {
            text::r(
                Locale::Zh,
                "r.dir_crew",
                &[("number", number), ("name", name), ("path", "")],
            )
        };
        assert!(
            lines.contains(&crew_line("0", &mgmt_title).as_str()),
            "{}",
            r.text
        );
        assert!(
            lines.contains(&crew_line(&n, "示例机组").as_str()),
            "{}",
            r.text
        );
        let me_line = text::r(
            Locale::Zh,
            "r.dir_session",
            &[
                ("number", &ext),
                ("name", "示例新人"),
                ("crew", "示例机组"),
                ("id", &x),
            ],
        );
        assert!(lines.contains(&me_line.as_str()), "{}", r.text);
        assert!(
            r.text.contains(&format!("{n}-1")) && r.text.contains(&f.captain),
            "the captain's extension: {}",
            r.text
        );
        assert!(
            !r.text.contains("示例已移除"),
            "removed crews are absent: {}",
            r.text
        );
        // A session without an extension is told how others reach it.
        let r = run(&f, &f.worker, "directory", json!({"query": "示例新人"})).await;
        assert_eq!(
            r.text.lines().next().unwrap(),
            text::r(Locale::Zh, "r.dir_header_no_number", &[("id", &f.worker)])
        );
        assert!(r.text.contains(&me_line) && !r.text.contains(&crew_line("0", &mgmt_title)));
        let r = run(&f, &f.worker, "directory", json!({"query": "没有这个名字"})).await;
        assert!(
            r.text.contains(&text::r(Locale::Zh, "r.dir_empty", &[])),
            "{}",
            r.text
        );
    }

    #[tokio::test]
    async fn merged_and_removed_channel_tools_are_gone() {
        let f = Fixture::new().await;
        for gone in [
            "contact",
            "channel_transfer",
            "channel_accept",
            "channel_unlisten",
        ] {
            assert!(catalog::find(gone).is_none(), "{gone}");
            assert!(
                catalog::REMOVED.contains(&gone),
                "{gone} names its replacement in REMOVED"
            );
            let r = run(&f, &f.worker, gone, json!({})).await;
            assert_eq!(
                r.text,
                text::r(Locale::Zh, "r.unknown_tool", &[("name", gone)])
            );
        }
        for kept in [
            "channel_create",
            "channel_invite",
            "channel_leave",
            "channel_archive",
            "channel_owner",
            "channel_listen",
            "directory",
        ] {
            assert!(catalog::find(kept).is_some(), "{kept}");
        }
    }
}
