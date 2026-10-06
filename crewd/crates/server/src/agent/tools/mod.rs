//! Tool dispatch: look up -> role gate (re-read on every call) -> parse args -> hand to the group
//! implementations. Receipt text always comes from the `r.*` sections of tools.*.md.
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::AppState;
use crate::agent::catalog::{self, Scope};
use crate::agent::port::{self, PortError};
use crate::agent::text::{self, Locale};
use crate::agent::{Identity, Role};
use crewd_proto::agent::CallResult;

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
    /// written for people, e.g. "not a member"). Internal errors (store failures) are not shown:
    /// the detail goes to the log with a number, the agent gets only the number (M3a policy).
    pub fn fail(&self, e: PortError) -> CallResult {
        match e {
            PortError::Internal(detail) => {
                let n = crate::dispatch::log_internal(&detail).to_string();
                let shown = self.r("r.internal", &[("n", &n)]);
                self.err("r.failed", &[("detail", &shown)])
            }
            other => self.err("r.failed", &[("detail", &other.to_string())]),
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
            cx.err("r.not_on_builtin", &[("name", tool)])
        };
    }
    // Task 12 replaces the two lines below with `match tool { ... _ => cx.err("r.no_handler", ...) }`,
    // and Tasks 12-15 add the group branches.
    let _ = args;
    cx.err("r.no_handler", &[("name", tool)])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent::Role;
    use crate::agent::testkit::Fixture;

    #[tokio::test]
    async fn builtin_crew_refuses_hidden_tools_even_when_called_directly() {
        let f = Fixture::new().await;
        let mut me = f.me(&f.captain).await;
        me.builtin_crew = true;
        let r = call(&f.state, &me, "ask", json!({"question": "示例"})).await;
        assert!(r.is_error);
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.not_on_builtin", &[("name", "ask")])
        );
        me.role = Role::Worker;
        me.builtin_crew = false;
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
