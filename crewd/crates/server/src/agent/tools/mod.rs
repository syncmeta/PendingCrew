//! Tool dispatch: look up -> role gate (re-read on every call) -> parse args -> hand to the group
//! implementations. Receipt text always comes from the `r.*` sections of tools.*.md.
pub mod channels;
pub mod messaging;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::AppState;
use crate::agent::catalog::{self, Scope};
use crate::agent::port::{
    self, ChannelView, CrewView, MsgRef, PortError, PostReq, PostSender, Posted,
};
use crate::agent::text::{self, Locale};
use crate::agent::{Identity, Role};
use crewd_proto::agent::CallResult;
use crewd_proto::{Mention, Message};

pub struct Ctx<'a> {
    pub st: &'a AppState,
    pub me: &'a Identity,
    pub loc: Locale,
}

impl Ctx<'_> {
    pub fn r(&self, key: &str, vars: &[(&str, &str)]) -> String {
        text::r(self.loc, key, vars)
    }
    /// Names (display names, crew names, channel names) go through M3a `safe_name` before they are
    /// put into a receipt (see Global Constraints).
    pub fn name(&self, s: &str) -> String {
        text::name(s)
    }
    pub fn ok(&self, key: &str, vars: &[(&str, &str)]) -> CallResult {
        CallResult::ok(self.r(key, vars))
    }
    pub fn err(&self, key: &str, vars: &[(&str, &str)]) -> CallResult {
        CallResult::err(self.r(key, vars))
    }
    /// M3a refusal / failure: the agent gets its explanation as is (they are rule explanations
    /// written for people, e.g. "not a member"; a collab refusal is rendered in the agent's locale
    /// from its stable code). Internal errors (store failures) are not shown:
    /// the detail goes to the log with a number, the agent gets only the number (M3a policy).
    pub fn fail(&self, e: PortError) -> CallResult {
        match e {
            PortError::Internal(detail) => {
                let n = crate::dispatch::log_internal(&detail).to_string();
                let shown = self.r("r.internal", &[("n", &n)]);
                self.err("r.failed", &[("detail", &shown)])
            }
            other => self.err("r.failed", &[("detail", &other.text(self.loc))]),
        }
    }
}

/// `null` is an empty object; an unknown field / a missing field / a wrong type returns serde's
/// explanation (which names the field).
pub fn decode<T: DeserializeOwned>(v: Value) -> Result<T, String> {
    let v = if v.is_null() {
        Value::Object(Default::default())
    } else {
        v
    };
    serde_json::from_value(v).map_err(|e| e.to_string())
}

pub fn parse_args<T: DeserializeOwned>(cx: &Ctx<'_>, v: Value) -> Result<T, CallResult> {
    decode(v).map_err(|d| cx.err("r.bad_args", &[("detail", &d)]))
}

/// How a channel is called in receipts and notes, as seen by the calling session: a live dm shows
/// its peer (async because of that lookup).
pub async fn channel_label(cx: &Ctx<'_>, c: &port::ChannelView) -> String {
    channel_label_for(
        cx.st,
        &cx.me.session_id,
        &c.id,
        c.kind,
        &c.name,
        &c.crew_title,
        cx.loc,
    )
    .await
}

/// Lobby and human channels have an empty `name` (M3a convention), so they are called by their
/// crew; a dm has no name of its own either, and its label needs a viewer (`channel_label_for`),
/// so without one it gets a neutral generic label. Names go through `text::name` (M3a `safe_name`).
pub fn channel_label_raw(
    loc: Locale,
    kind: crewd_proto::ChannelKind,
    name: &str,
    crew_title: &str,
) -> String {
    match kind {
        crewd_proto::ChannelKind::Lobby => {
            text::r(loc, "r.lobby_label", &[("crew", &text::name(crew_title))])
        }
        crewd_proto::ChannelKind::Human => {
            text::r(loc, "r.human_label", &[("crew", &text::name(crew_title))])
        }
        crewd_proto::ChannelKind::Dm => text::r(loc, "r.dm_generic", &[]),
        crewd_proto::ChannelKind::Topic => text::name(name),
    }
}

/// `channel_label_raw`, but a dm shows the other party (M3a `channels::display_label`). Only an
/// empty answer (the peer is gone) is called dissolved; a lookup error gets the neutral generic
/// label. The dm decoration follows the CURRENT captain pointer, while sender labels follow the
/// kind at post time.
pub async fn channel_label_for(
    st: &AppState,
    viewer: &str,
    channel: &str,
    kind: crewd_proto::ChannelKind,
    name: &str,
    crew_title: &str,
    loc: Locale,
) -> String {
    if kind == crewd_proto::ChannelKind::Dm {
        return match port::display_label(st, viewer, channel, loc).await {
            Ok(l) if !l.is_empty() => l,
            Ok(_) => text::r(loc, "r.dm_gone", &[]),
            Err(_) => channel_label_raw(loc, kind, name, crew_title),
        };
    }
    channel_label_raw(loc, kind, name, crew_title)
}

pub async fn call(st: &AppState, me: &Identity, tool: &str, args: Value) -> CallResult {
    let cx = Ctx {
        st,
        me,
        loc: port::locale(st).await.unwrap_or_default(),
    };
    let Some(spec) = catalog::find(tool) else {
        return cx.err("r.unknown_tool", &[("name", tool)]);
    };
    // One gate (`catalog::visible`, the same rule the tool list uses); the checks below only pick
    // which receipt to give.
    if !catalog::visible(spec, me) {
        return if spec.scope == Scope::Captain && me.role != Role::Captain {
            cx.err("r.captain_only", &[("name", tool)])
        } else {
            cx.err("r.not_on_management", &[("name", tool)])
        };
    }
    match tool {
        "post" => messaging::post(&cx, args).await,
        "read_channel" => messaging::read_channel(&cx, args).await,
        "search" => messaging::search(&cx, args).await,
        "list_channels" => messaging::list_channels(&cx, args).await,
        "channel_create" => channels::create(&cx, args).await,
        "channel_invite" => channels::invite(&cx, args).await,
        "channel_leave" => channels::leave(&cx, args).await,
        "channel_archive" => channels::archive(&cx, args).await,
        "channel_owner" => channels::owner(&cx, args).await,
        "channel_listen" => channels::listen(&cx, args).await,
        "directory" => channels::directory(&cx, args).await,
        // Tasks 14-15 add the other groups.
        _ => cx.err("r.no_handler", &[("name", tool)]),
    }
}

/// A message reference in tool arguments: a number (`128`), `"128"` / `"#128"`, or a message id.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
pub enum RefArg {
    Num(i64),
    Str(String),
}

/// `128` / `"128"` / `"#128"` -> a sequence number; anything else is a message id.
pub fn msg_ref(r: &RefArg) -> MsgRef {
    match r {
        RefArg::Num(n) => MsgRef::Seq(*n),
        RefArg::Str(s) => {
            let t = s.trim().trim_start_matches('#');
            t.parse::<i64>()
                .map(MsgRef::Seq)
                .unwrap_or_else(|_| MsgRef::Id(s.trim().to_string()))
        }
    }
}

/// How a message reference is written back in a receipt.
pub fn ref_text(r: &RefArg) -> String {
    match r {
        RefArg::Num(n) => format!("#{n}"),
        RefArg::Str(s) => s.clone(),
    }
}

/// At most this many channels are offered when a channel argument does not resolve.
const CHOICES_CAP: usize = 20;

/// A list of channels to choose from (`r.channel_choice` lines): the ones I am in or listen to
/// first, at most `CHOICES_CAP`.
pub async fn channel_choices(cx: &Ctx<'_>, pool: &[&ChannelView]) -> String {
    let mut pool: Vec<&ChannelView> = pool.to_vec();
    pool.sort_by_key(|c| !(c.has_session(&cx.me.session_id) || c.listening));
    let mut lines = Vec::new();
    for c in pool.into_iter().take(CHOICES_CAP) {
        let label = channel_label(cx, c).await;
        lines.push(cx.r("r.channel_choice", &[("id", &c.id), ("name", &label)]));
    }
    lines.join("\n")
}

/// A channel argument: the id of a channel I can read (B15 rules + my listens), else the unique
/// name among them. A miss or an ambiguity lists the choices (members first, capped) and points
/// to `list_channels`. A channel I cannot read is "not found", same as one that does not exist.
pub async fn resolve_channel(cx: &Ctx<'_>, arg: &str) -> Result<ChannelView, CallResult> {
    let arg = arg.trim();
    let all = port::readable_views(cx.st, cx.me)
        .await
        .map_err(|e| cx.fail(e))?;
    if let Some(c) = all.iter().find(|c| c.id == arg) {
        return Ok(c.clone());
    }
    let named: Vec<&ChannelView> = all
        .iter()
        .filter(|c| !c.name.is_empty() && c.name == arg)
        .collect();
    if let [one] = named.as_slice() {
        return Ok((*one).clone());
    }
    let (key, pool) = if named.is_empty() {
        ("r.no_channel", all.iter().collect::<Vec<_>>())
    } else {
        ("r.ambiguous_channel", named)
    };
    let choices = channel_choices(cx, &pool).await;
    Err(cx.err(key, &[("arg", arg), ("choices", &choices)]))
}

/// A crew argument: number, id or exact title (`port::resolve_crew`); a removed crew by number is
/// refused with collab's `directory.crew_removed`, an unknown one with `crew.not_found`.
pub async fn crew_arg(cx: &Ctx<'_>, arg: &str) -> Result<CrewView, CallResult> {
    use crewd_collab::refusal::{Code, Refusal};
    match port::resolve_crew(cx.st, arg.trim()).await {
        Ok(Some(c)) => Ok(c),
        Ok(None) => Err(cx.fail(PortError::NotFound(port::Reason::Rule(
            Refusal::new(Code::CrewNotFound).arg("crew", arg.trim()),
        )))),
        Err(e) => Err(cx.fail(e)),
    }
}

/// One message as the agent reads it: `#seq time sender: text` (+ attachment paths). The sender
/// label is the injection surface's (`port::sender_label`, B6).
pub async fn render_line(cx: &Ctx<'_>, m: &Message) -> Result<String, CallResult> {
    let who = port::sender_label(cx.st, m, cx.loc)
        .await
        .map_err(|e| cx.fail(e))?;
    let mut line = cx.r(
        "r.line",
        &[
            ("seq", &m.seq.to_string()),
            ("time", &m.created_at),
            ("who", &who),
            ("text", &m.text),
        ],
    );
    for a in &m.attachments {
        line.push_str(&cx.r("r.line_attachment", &[("path", &a.path)]));
    }
    Ok(line)
}

/// Post one message as `sender`; a post in my own name also notes "this turn called someone"
/// (the stored mentions, so a reply's automatic @ counts too).
pub async fn post_as(
    cx: &Ctx<'_>,
    channel: &str,
    text: String,
    mentions: Vec<Mention>,
    in_reply_to: Option<String>,
    category: Option<String>,
    sender: PostSender,
) -> Result<Posted, PortError> {
    let by_me = matches!(&sender, PostSender::Session(s) if *s == cx.me.session_id);
    let p = port::post(
        cx.st,
        PostReq {
            channel_id: channel.to_string(),
            sender,
            text,
            mentions,
            in_reply_to,
            category,
            attachments: vec![], // M3a: message.post takes no attachments yet.
        },
    )
    .await?;
    if by_me {
        crate::agent::turn::note_post(cx.st, cx.me, &p.message.mentions).await;
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent::Role;
    use crate::agent::testkit::Fixture;

    #[tokio::test]
    async fn management_crew_refuses_hidden_tools_even_when_called_directly() {
        let f = Fixture::new().await;
        let mut me = f.me(&f.captain).await;
        me.management_crew = true;
        // ask is open on the management crew (N1-R9); respond_todo is still hidden there (until T15b).
        let r = call(
            &f.state,
            &me,
            "respond_todo",
            json!({"number": 1, "response": "示例"}),
        )
        .await;
        assert!(r.is_error);
        assert_eq!(
            r.text,
            text::r(
                Locale::Zh,
                "r.not_on_management",
                &[("name", "respond_todo")]
            )
        );
        me.role = Role::Worker;
        me.management_crew = false;
        let r = call(&f.state, &me, "rename_crew", json!({"name": "x"})).await;
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.captain_only", &[("name", "rename_crew")])
        );
    }

    #[tokio::test]
    async fn internal_failures_show_a_number_not_the_detail() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        let cx = Ctx {
            st: &f.state,
            me: &me,
            loc: Locale::Zh,
        };
        let r = cx.fail(PortError::Internal("no such table: secrets".into()));
        assert!(r.is_error);
        assert!(!r.text.contains("secrets"), "{}", r.text);
        let probe = text::r(Locale::Zh, "r.internal", &[("n", "@@")]);
        let (before, after) = probe.split_once("@@").unwrap();
        assert!(
            r.text.contains(before) && r.text.contains(after),
            "{}",
            r.text
        );
        // Refusals are rule explanations for the agent: shown as they are.
        let r = cx.fail(PortError::Refused("not a member".into()));
        assert!(r.text.contains("not a member"), "{}", r.text);
    }

    #[test]
    fn unknown_argument_is_named() {
        // Review Focus 5（参数半边）：未知字段整条拒、说清是哪个。
        #[derive(serde::Deserialize, Debug)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct A {
            text: String,
        }
        let e = decode::<A>(json!({"text": "x", "colour": 1})).unwrap_err();
        assert!(e.contains("colour"), "{e}");
        assert!(
            decode::<A>(json!(null)).unwrap_err().contains("text"),
            "null 当空对象"
        );
    }
}
