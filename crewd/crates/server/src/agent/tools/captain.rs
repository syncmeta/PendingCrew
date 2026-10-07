//! Captain tools, session control (spec §22.3; T15 supplement, T15a): `start_session`,
//! `stop_session`, `inspect_session`, `nudge_session`, `list_sessions`, `handoff_captain`.
//!
//! Authority is collab's (C1 / C2): "may act on crew X" is `crews::acts_as_captain_of(me, X)`
//! (through `port::acts_as_captain_of`; the management captain counts as the captain of every
//! crew), never `role == Captain && crew == mine`. Session control itself goes through
//! SessionControl (M3c); the session title is collab's rule (`port::resolve_title`), checked
//! before anything starts so its refusals reach the agent with their codes. These tools parse
//! arguments, resolve crews and sessions, call the port and write receipts.
use crewd_proto::SessionLive;
use crewd_proto::agent::CallResult;
use serde::Deserialize;
use serde_json::Value;

use super::channels::{Step, done, resolve_member};
use super::{Ctx, crew_arg, parse_args, post_as};
use crate::agent::catalog::NUDGE_KEYS;
use crate::agent::port::{self, CrewView, MemberRef, PortError, PostSender, Reason, StartReq};
use crate::agent::{Identity, Role};

pub(super) fn not_found_crew(cx: &Ctx<'_>, id: &str) -> CallResult {
    use crewd_collab::refusal::{Code, Refusal};
    cx.fail(PortError::NotFound(Reason::Rule(
        Refusal::new(Code::CrewNotFound).arg("crew", id),
    )))
}

/// The crew a call acts on: the `crew` argument (number or id), else my own crew.
pub(super) async fn target_crew(cx: &Ctx<'_>, arg: Option<&str>) -> Step<CrewView> {
    match arg.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => crew_arg(cx, a).await,
        None => port::crew(cx.st, &cx.me.crew_id)
            .await
            .map_err(|e| cx.fail(e))?
            .ok_or_else(|| not_found_crew(cx, &cx.me.crew_id)),
    }
}

/// `r.not_your_crew`: the crew is not mine to act on; the receipt says whom to ask (its number,
/// or without one `r.not_your_crew_unnumbered`: a raw id is never shown as a label, C5).
///
/// Deliberately not "not found" (unlike T13's unreadable channel, which answers like a missing
/// one): `directory` already lists every crew and session number, so naming the crew hides
/// nothing.
pub(super) fn not_your_crew(cx: &Ctx<'_>, crew: &CrewView) -> CallResult {
    let name = cx.name(&crew.title);
    match crew.number {
        Some(n) => cx.err(
            "r.not_your_crew",
            &[("crew", &name), ("number", &n.to_string())],
        ),
        None => cx.err("r.not_your_crew_unnumbered", &[("crew", &name)]),
    }
}

/// The target-crew rule: I act as the captain of `crew` (collab's rule, C2).
pub(super) async fn require_captain_of(cx: &Ctx<'_>, crew: &CrewView) -> Step<()> {
    if port::acts_as_captain_of(cx.st, &cx.me.session_id, &crew.id)
        .await
        .map_err(|e| cx.fail(e))?
    {
        Ok(())
    } else {
        Err(not_your_crew(cx, crew))
    }
}

/// A session argument (id or address, C3) -> that session's identity.
async fn session_arg(cx: &Ctx<'_>, arg: &str) -> Step<Identity> {
    use crewd_collab::refusal::{Code, Refusal};
    let missing = || {
        cx.fail(PortError::NotFound(Reason::Rule(
            Refusal::new(Code::SessionNotFound).arg("session", arg.trim()),
        )))
    };
    let MemberRef::Session(id) = resolve_member(cx, arg).await? else {
        return Err(missing());
    };
    port::identity(cx.st, &id)
        .await
        .map_err(|e| cx.fail(e))?
        .ok_or_else(missing)
}

/// The target-session rule: `who`'s crew is one I act as the captain of (C2); returns that crew.
async fn crew_in_reach(cx: &Ctx<'_>, who: &Identity) -> Step<CrewView> {
    let crew = port::crew(cx.st, &who.crew_id)
        .await
        .map_err(|e| cx.fail(e))?
        .ok_or_else(|| not_found_crew(cx, &who.crew_id))?;
    require_captain_of(cx, &crew).await?;
    Ok(crew)
}

/// A session argument within reach (`session_arg` + `crew_in_reach`).
async fn session_in_reach(cx: &Ctx<'_>, arg: &str) -> Step<Identity> {
    let who = session_arg(cx, arg).await?;
    crew_in_reach(cx, &who).await?;
    Ok(who)
}

pub(super) fn trimmed(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartArgs {
    brief: String,
    /// Required (C8): missing / not a boolean -> `r.bad_args` naming the field.
    isolation: bool,
    title: Option<String>,
    runner: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    crew: Option<String>,
}

pub async fn start_session(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(start_session_(cx, v).await)
}

async fn start_session_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: StartArgs = parse_args(cx, v)?;
    let crew = target_crew(cx, a.crew.as_deref()).await?;
    require_captain_of(cx, &crew).await?;
    let brief = a.brief.trim().to_string();
    if brief.is_empty() {
        return Err(cx.err("r.brief_empty", &[]));
    }
    let title = port::resolve_title(a.title.as_deref(), Some(&brief)).map_err(|e| cx.fail(e))?;
    let req = StartReq {
        crew_id: crew.id,
        role: Role::Worker,
        executor: trimmed(a.runner),
        brief,
        title: Some(title.clone()),
        model: trimmed(a.model),
        effort: trimmed(a.effort),
        isolation: a.isolation,
    };
    // A worktree that cannot be created is SessionControl's failure (`r.failed` with the reason):
    // nothing started, no fallback to the shared directory (C8).
    let id = port::start_session(cx.st, req)
        .await
        .map_err(|e| cx.fail(e))?;
    Ok(cx.ok(
        "r.session_started",
        &[("title", &cx.name(&title)), ("id", &id)],
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopArgs {
    session_id: String,
    reason: String,
}

pub async fn stop_session(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(stop_session_(cx, v).await)
}

async fn stop_session_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: StopArgs = parse_args(cx, v)?;
    let reason = a.reason.trim();
    if reason.is_empty() {
        return Err(cx.err("r.stop_reason_empty", &[]));
    }
    let who = session_arg(cx, &a.session_id).await?;
    if who.session_id == cx.me.session_id {
        return Err(cx.err("r.stop_self", &[]));
    }
    let crew = crew_in_reach(cx, &who).await?;
    let target = cx.name(&who.label);
    // The reason goes into the target's lobby first, as me (a management session posts there
    // without joining, C2), then the session is stopped. No notice = nothing stopped (fail
    // closed); a stop that fails after the notice says the notice is out and nothing stopped.
    let not_done = |e| cx.err("r.stop_not_done", &[("detail", &cx.detail(e))]);
    let lobby = port::lobby_of(cx.st, &crew.id).await.map_err(not_done)?;
    let notice = cx.r("r.stop_notice", &[("target", &target), ("reason", reason)]);
    post_as(
        cx,
        &lobby.id,
        notice,
        vec![],
        None,
        None,
        PostSender::Session(cx.me.session_id.clone()),
    )
    .await
    .map_err(not_done)?;
    port::stop_session(cx.st, &who.session_id, reason)
        .await
        .map_err(|e| {
            cx.err(
                "r.stop_failed_after_notice",
                &[("target", &target), ("detail", &cx.detail(e))],
            )
        })?;
    Ok(cx.ok(
        "r.session_stopped",
        &[("target", &target), ("crew", &cx.name(&crew.title))],
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionArg {
    session_id: String,
}

pub async fn inspect_session(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(inspect_session_(cx, v).await)
}

async fn inspect_session_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: SessionArg = parse_args(cx, v)?;
    let who = session_in_reach(cx, &a.session_id).await?;
    let report = port::inspect(cx.st, &who.session_id)
        .await
        .map_err(|e| cx.fail(e))?;
    Ok(cx.ok(
        "r.inspect",
        &[("target", &cx.name(&who.label)), ("report", &report)],
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NudgeArgs {
    session_id: String,
    key: String,
}

pub async fn nudge_session(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(nudge_session_(cx, v).await)
}

async fn nudge_session_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: NudgeArgs = parse_args(cx, v)?;
    // The schema's enum (Q1, S51): both keys interrupt the current turn; anything else, free text
    // included, is refused with a pointer to `post(to:)`.
    let key = a.key.trim().to_ascii_lowercase();
    if !NUDGE_KEYS.contains(&key.as_str()) {
        return Err(cx.err("r.nudge_keys_only", &[("key", &cx.name(&a.key))]));
    }
    let who = session_in_reach(cx, &a.session_id).await?;
    port::nudge(cx.st, &who.session_id, &key)
        .await
        .map_err(|e| cx.fail(e))?;
    // Sent, not confirmed (S73): confirming the effect is M3c T10c's.
    Ok(cx.ok(
        "r.nudged",
        &[("target", &cx.name(&who.label)), ("key", &key)],
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CrewOnly {
    crew: Option<String>,
}

pub async fn list_sessions(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(list_sessions_(cx, v).await)
}

/// A list row's state word, from what SessionControl reports.
fn state_key(live: &Result<SessionLive, PortError>) -> &'static str {
    match live {
        Err(_) => "r.state_unknown",
        Ok(l) if !l.running => "r.state_stopped",
        Ok(l) if l.pending_decision.is_some() || l.awaiting_question.is_some() => "r.state_waiting",
        Ok(l) if l.working => "r.state_working",
        Ok(_) => "r.state_idle",
    }
}

async fn list_sessions_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: CrewOnly = parse_args(cx, v)?;
    let crew = target_crew(cx, a.crew.as_deref()).await?;
    require_captain_of(cx, &crew).await?;
    let list = port::sessions_of(cx.st, &crew.id)
        .await
        .map_err(|e| cx.fail(e))?;
    let mut out = vec![cx.r("r.sessions_header", &[("crew", &cx.name(&crew.title))])];
    if list.is_empty() {
        out.push(cx.r("r.sessions_none", &[]));
    }
    for s in &list {
        let live = port::live(cx.st, &s.id).await;
        let role = cx.r(
            if s.role == Role::Captain {
                "r.role_captain"
            } else {
                "r.role_worker"
            },
            &[],
        );
        let address = s
            .extension
            .clone()
            .unwrap_or_else(|| cx.r("r.no_address", &[]));
        out.push(cx.r(
            "r.session_line",
            &[
                ("id", &s.id),
                ("address", &address),
                ("name", &cx.name(&s.label)),
                ("role", &role),
                ("state", &cx.r(state_key(&live), &[])),
            ],
        ));
    }
    out.push(cx.r("r.sessions_tail", &[]));
    Ok(CallResult::ok(out.join("\n")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffArgs {
    crew: Option<String>,
    to_session: Option<String>,
    runner: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    title: Option<String>,
    opening_brief: Option<String>,
}

pub async fn handoff_captain(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(handoff_captain_(cx, v).await)
}

/// Whose captaincy I may hand off: a direct child of my crew (the one rule of the tool's own), or
/// any crew I act as the captain of (collab, C2: my own crew, every crew for the management
/// captain).
async fn handoff_crew(cx: &Ctx<'_>, arg: Option<&str>) -> Step<CrewView> {
    let crew = target_crew(cx, arg).await?;
    if crew.parent_ids.contains(&cx.me.crew_id) {
        return Ok(crew);
    }
    require_captain_of(cx, &crew).await?;
    Ok(crew)
}

async fn handoff_captain_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    use crewd_collab::refusal::{Code, Refusal};
    let a: HandoffArgs = parse_args(cx, v)?;
    let to_session = trimmed(a.to_session);
    let runner = trimmed(a.runner);
    let new_only = [&a.model, &a.effort, &a.title, &a.opening_brief]
        .iter()
        .any(|x| x.as_deref().is_some_and(|s| !s.trim().is_empty()));
    let one_of = match (&to_session, &runner) {
        (Some(_), None) => !new_only,
        (None, Some(_)) => true,
        _ => false,
    };
    if !one_of {
        return Err(cx.err("r.handoff_one_of", &[]));
    }
    let crew = handoff_crew(cx, a.crew.as_deref()).await?;
    let crew_name = cx.name(&crew.title);
    let old = match &crew.captain {
        Some(o) => port::identity(cx.st, o).await.map_err(|e| cx.fail(e))?,
        None => None,
    };
    let (new_id, new_name) = if let Some(arg) = to_session {
        let who = session_arg(cx, &arg).await?;
        // collab `crews::set_captain` refuses a successor from another crew; SessionControl may
        // not reach collab before it acts, so the same code is given here first (C1).
        if who.crew_id != crew.id {
            return Err(cx.fail(PortError::Refused(Reason::Rule(
                Refusal::new(Code::CrewSessionNotInCrew)
                    .arg("session", who.session_id.as_str())
                    .arg("crew", crew.id.as_str()),
            ))));
        }
        if crew.captain.as_deref() == Some(who.session_id.as_str()) {
            return Err(cx.err(
                "r.handoff_already",
                &[("target", &cx.name(&who.label)), ("crew", &crew_name)],
            ));
        }
        port::handoff_captain(cx.st, &crew.id, &who.session_id)
            .await
            .map_err(|e| cx.fail(e))?;
        (who.session_id, cx.name(&who.label))
    } else {
        // A new captain: started as a worker (the crew may still have a captain; collab's
        // `register` takes no second one), then handed the role.
        let brief =
            trimmed(a.opening_brief).unwrap_or_else(|| cx.r("r.handoff_default_brief", &[]));
        let wanted = trimmed(a.title).unwrap_or_else(|| cx.r("r.default_captain_title", &[]));
        let title = port::resolve_title(Some(&wanted), Some(&brief)).map_err(|e| cx.fail(e))?;
        let req = StartReq {
            crew_id: crew.id.clone(),
            role: Role::Worker,
            executor: runner,
            brief,
            title: Some(title.clone()),
            model: trimmed(a.model),
            effort: trimmed(a.effort),
            // A captain works in the crew's shared directory.
            isolation: false,
        };
        let id = port::start_session(cx.st, req)
            .await
            .map_err(|e| cx.fail(e))?;
        let new_name = cx.name(&title);
        // Started but not handed over: it runs as a worker; say so, so it is not started twice.
        port::handoff_captain(cx.st, &crew.id, &id)
            .await
            .map_err(|e| {
                cx.err(
                    "r.handoff_started_not_handed",
                    &[("new", &new_name), ("id", &id), ("detail", &cx.detail(e))],
                )
            })?;
        (id, new_name)
    };
    let mut text = match &old {
        Some(o) => cx.r(
            "r.handoff_done",
            &[
                ("crew", &crew_name),
                ("new", &new_name),
                ("id", &new_id),
                ("old", &cx.name(&o.label)),
            ],
        ),
        None => cx.r(
            "r.handoff_done_vacant",
            &[("crew", &crew_name), ("new", &new_name), ("id", &new_id)],
        ),
    };
    if old.is_some_and(|o| o.session_id == cx.me.session_id) {
        text.push('\n');
        text.push_str(&cx.r("r.handoff_self", &[]));
    }
    Ok(CallResult::ok(text))
}

#[cfg(test)]
mod tests {
    use crewd_collab::refusal::{Code, Refusal};
    use crewd_proto::{SenderKind, SessionLive};
    use serde_json::json;

    use crate::agent::Role;
    use crate::agent::port;
    use crate::agent::testkit::Fixture;
    use crate::agent::text::{self, Locale};
    use crate::agent::tools::call;
    use crate::control::{ControlError, FakeCall, FakeMethod, StartRequest};

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

    /// The tool's own receipt, whole: `is_error` as given.
    #[track_caller]
    fn says(r: &crewd_proto::agent::CallResult, error: bool, key: &str, vars: &[(&str, &str)]) {
        assert_eq!(r.text, text::r(Locale::Zh, key, vars));
        assert_eq!(r.is_error, error, "{}", r.text);
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

    /// `r.not_your_crew` for `crew`, as the tool writes it.
    async fn not_yours(f: &Fixture, r: &crewd_proto::agent::CallResult, crew: &str) {
        let c = port::crew(&f.state, crew).await.unwrap().unwrap();
        let n = number_of(f, crew).await;
        says(
            r,
            true,
            "r.not_your_crew",
            &[("crew", &c.title), ("number", &n)],
        );
    }

    fn calls_of(f: &Fixture, prefix: &str) -> Vec<String> {
        f.control_calls()
            .into_iter()
            .filter(|c| c.starts_with(prefix))
            .collect()
    }

    fn start_call(r: StartRequest) -> String {
        format!("{:?}", FakeCall::Start(r))
    }

    fn worker_start(crew: &str, brief: &str, title: &str, isolation: bool) -> StartRequest {
        StartRequest {
            crew_id: crew.into(),
            role: Role::Worker,
            executor: "claude".into(),
            brief: brief.into(),
            title: Some(title.into()),
            model: None,
            effort: None,
            isolation,
        }
    }

    /// The fake's next session id (`fake-<number of calls so far>`).
    fn next_fake_id(f: &Fixture) -> String {
        format!("fake-{}", f.control_calls().len())
    }

    async fn last_lobby_message(f: &Fixture, crew: &str) -> crewd_proto::Message {
        let lobby = port::lobby_of(&f.state, crew).await.unwrap().id;
        port::messages_raw(&f.state, &lobby, None, 1)
            .await
            .unwrap()
            .pop()
            .unwrap()
    }

    /// C8 isolation as given; the title is collab's rule (S80, Q2), checked before anything starts.
    #[tokio::test]
    async fn start_session_passes_isolation_and_resolved_title() {
        let f = Fixture::new().await;
        for isolation in [false, true] {
            let id = next_fake_id(&f);
            let r = run(
                &f,
                &f.captain,
                "start_session",
                json!({"brief": "补齐示例模块的测试", "title": " 补\u{200B}测试 ", "isolation": isolation}),
            )
            .await;
            says(
                &r,
                false,
                "r.session_started",
                &[("title", "补测试"), ("id", &id)],
            );
            assert_eq!(
                calls_of(&f, "Start(").pop().unwrap(),
                start_call(worker_start(
                    &f.crew,
                    "补齐示例模块的测试",
                    "补测试",
                    isolation
                ))
            );
        }
        // No title: derived from the brief's first line, 18 code points.
        let id = next_fake_id(&f);
        let brief = "\n补齐示例模块的单元测试，覆盖边界情况和错误路径\n第二行";
        let r = run(
            &f,
            &f.captain,
            "start_session",
            json!({"brief": brief, "isolation": false}),
        )
        .await;
        let derived = "补齐示例模块的单元测试，覆盖边界情况";
        says(
            &r,
            false,
            "r.session_started",
            &[("title", derived), ("id", &id)],
        );
        assert_eq!(
            calls_of(&f, "Start(").pop().unwrap(),
            start_call(worker_start(&f.crew, brief.trim(), derived, false))
        );
        let started = calls_of(&f, "Start(").len();
        // Collab's refusals, nothing started.
        let r = run(
            &f,
            &f.captain,
            "start_session",
            json!({"brief": "x", "title": "长".repeat(19), "isolation": false}),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::SessionTitleTooLong)
                .arg("max", "18")
                .arg("actual", "19"),
        );
        let r = run(
            &f,
            &f.captain,
            "start_session",
            json!({"brief": "x", "title": "人类", "isolation": false}),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::SessionTitleIsSenderLabel).arg("value", "人类"),
        );
        let r = run(
            &f,
            &f.captain,
            "start_session",
            json!({"brief": " \n ", "isolation": false}),
        )
        .await;
        says(&r, true, "r.brief_empty", &[]);
        // C8: isolation is required and a boolean; there is no workdir parameter.
        let r = run(&f, &f.captain, "start_session", json!({"brief": "x"})).await;
        says(
            &r,
            true,
            "r.bad_args",
            &[("detail", "missing field `isolation`")],
        );
        let r = run(
            &f,
            &f.captain,
            "start_session",
            json!({"brief": "x", "isolation": false, "workdir": "/tmp"}),
        )
        .await;
        says(
            &r,
            true,
            "r.bad_args",
            &[(
                "detail",
                "unknown field `workdir`, expected one of `brief`, `isolation`, `title`, `runner`, `model`, `effort`, `crew`",
            )],
        );
        assert_eq!(calls_of(&f, "Start(").len(), started);
    }

    /// C2: the management captain acts as the captain of every crew, its own included.
    #[tokio::test]
    async fn management_captain_starts_in_another_crew() {
        let f = Fixture::new().await;
        let mgmt = management_crew(&f).await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        let n = number_of(&f, &f.crew).await;
        let id = next_fake_id(&f);
        let r = run(
            &f,
            "s-boss",
            "start_session",
            json!({"brief": "示例任务", "isolation": false, "crew": n}),
        )
        .await;
        says(
            &r,
            false,
            "r.session_started",
            &[("title", "示例任务"), ("id", &id)],
        );
        assert_eq!(
            calls_of(&f, "Start(").pop().unwrap(),
            start_call(worker_start(&f.crew, "示例任务", "示例任务", false))
        );
        // Its own crew (no `crew`): no management special case (S68).
        let r = run(
            &f,
            "s-boss",
            "start_session",
            json!({"brief": "管理层的活", "isolation": false}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(
            calls_of(&f, "Start(").pop().unwrap(),
            start_call(worker_start(&mgmt, "管理层的活", "管理层的活", false))
        );
    }

    #[tokio::test]
    async fn captain_cannot_start_in_a_crew_it_does_not_captain() {
        let f = Fixture::new().await;
        let other = f.add_crew("示例他组", None).await;
        f.add_session(&other, "s-ocap", Role::Captain, "他组机长")
            .await;
        let child = f.add_crew("示例子机组", Some(&f.crew)).await;
        for crew in [&other, &child] {
            let n = number_of(&f, crew).await;
            let r = run(
                &f,
                &f.captain,
                "start_session",
                json!({"brief": "x", "isolation": false, "crew": n}),
            )
            .await;
            not_yours(&f, &r, crew).await;
        }
        // A worker has no captain tools at all.
        let r = run(
            &f,
            &f.worker,
            "start_session",
            json!({"brief": "x", "isolation": false}),
        )
        .await;
        says(&r, true, "r.captain_only", &[("name", "start_session")]);
        assert!(calls_of(&f, "Start(").is_empty());
    }

    #[tokio::test]
    async fn stop_posts_reason_then_stops_within_reach() {
        let f = Fixture::new().await;
        // Own crew: the reason goes into the lobby as me, then the session is stopped.
        let r = run(
            &f,
            &f.captain,
            "stop_session",
            json!({"session_id": f.worker, "reason": "示例：任务取消"}),
        )
        .await;
        says(
            &r,
            false,
            "r.session_stopped",
            &[("target", "示例 worker"), ("crew", "示例机组")],
        );
        let m = last_lobby_message(&f, &f.crew).await;
        assert_eq!(
            m.text,
            text::r(
                Locale::Zh,
                "r.stop_notice",
                &[("target", "示例 worker"), ("reason", "示例：任务取消")]
            )
        );
        assert_eq!(
            m.sender_kind,
            SenderKind::Captain,
            "posted as me, not System"
        );
        assert_eq!(m.sender_session_id.as_deref(), Some("s-cap"));
        assert_eq!(
            calls_of(&f, "Stop("),
            [format!(
                "{:?}",
                FakeCall::Stop("s-w1".into(), "示例：任务取消".into())
            )]
        );
        // Another crew: not mine.
        let other = f.add_crew("示例他组", None).await;
        f.add_session(&other, "s-far", Role::Worker, "外组 worker")
            .await;
        let other_lobby = port::lobby_of(&f.state, &other).await.unwrap().id;
        let before = port::latest_seq(&f.state, &other_lobby).await.unwrap();
        let r = run(
            &f,
            &f.captain,
            "stop_session",
            json!({"session_id": "s-far", "reason": "x"}),
        )
        .await;
        not_yours(&f, &r, &other).await;
        assert_eq!(
            port::latest_seq(&f.state, &other_lobby).await.unwrap(),
            before,
            "nothing posted"
        );
        // Not myself; a blank reason is refused before anything.
        let r = run(
            &f,
            &f.captain,
            "stop_session",
            json!({"session_id": f.captain, "reason": "x"}),
        )
        .await;
        says(&r, true, "r.stop_self", &[]);
        let r = run(
            &f,
            &f.captain,
            "stop_session",
            json!({"session_id": f.worker, "reason": "  "}),
        )
        .await;
        says(&r, true, "r.stop_reason_empty", &[]);
        assert_eq!(calls_of(&f, "Stop(").len(), 1);
        // The management captain may, in any crew; its notice lands in that crew's lobby.
        let mgmt = management_crew(&f).await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        let r = run(
            &f,
            "s-boss",
            "stop_session",
            json!({"session_id": "s-far", "reason": "管理层叫停"}),
        )
        .await;
        says(
            &r,
            false,
            "r.session_stopped",
            &[("target", "外组 worker"), ("crew", "示例他组")],
        );
        let m = last_lobby_message(&f, &other).await;
        assert_eq!(m.sender_session_id.as_deref(), Some("s-boss"));
        assert_eq!(
            m.text,
            text::r(
                Locale::Zh,
                "r.stop_notice",
                &[("target", "外组 worker"), ("reason", "管理层叫停")]
            )
        );
        assert_eq!(
            calls_of(&f, "Stop(").pop().unwrap(),
            format!("{:?}", FakeCall::Stop("s-far".into(), "管理层叫停".into()))
        );
    }

    /// Q1 / S51: keys only; free text points to `post(to:)`. The receipt is honest (S73).
    #[tokio::test]
    async fn nudge_accepts_keys_only() {
        let f = Fixture::new().await;
        for (given, key) in [("esc", "esc"), ("interrupt", "interrupt"), (" ESC ", "esc")] {
            let r = run(
                &f,
                &f.captain,
                "nudge_session",
                json!({"session_id": f.worker, "key": given}),
            )
            .await;
            says(
                &r,
                false,
                "r.nudged",
                &[("target", "示例 worker"), ("key", key)],
            );
            assert_eq!(
                calls_of(&f, "Nudge(").pop().unwrap(),
                format!("{:?}", FakeCall::Nudge("s-w1".into(), key.into()))
            );
        }
        for text in ["继续", "enter", "2"] {
            let r = run(
                &f,
                &f.captain,
                "nudge_session",
                json!({"session_id": f.worker, "key": text}),
            )
            .await;
            says(&r, true, "r.nudge_keys_only", &[("key", text)]);
        }
        // The old free-text parameter is gone.
        let r = run(
            &f,
            &f.captain,
            "nudge_session",
            json!({"session_id": f.worker, "input": "Esc"}),
        )
        .await;
        says(
            &r,
            true,
            "r.bad_args",
            &[(
                "detail",
                "unknown field `input`, expected `session_id` or `key`",
            )],
        );
        let other = f.add_crew("示例他组", None).await;
        f.add_session(&other, "s-far", Role::Worker, "外组 worker")
            .await;
        let r = run(
            &f,
            &f.captain,
            "nudge_session",
            json!({"session_id": "s-far", "key": "esc"}),
        )
        .await;
        not_yours(&f, &r, &other).await;
        assert_eq!(calls_of(&f, "Nudge(").len(), 3);
    }

    #[tokio::test]
    async fn list_sessions_of_a_crew() {
        let f = Fixture::new().await;
        let n = number_of(&f, &f.crew).await;
        let reg = f.register(&f.crew, Role::Worker, "示例登记").await;
        f.set_live(SessionLive {
            session_id: "s-cap".into(),
            running: true,
            ..Default::default()
        });
        f.set_live(SessionLive {
            session_id: "s-w1".into(),
            running: true,
            working: true,
            ..Default::default()
        });
        let r = run(&f, &f.captain, "list_sessions", json!({})).await;
        let z = |k: &str| text::r(Locale::Zh, k, &[]);
        let line = |id: &str, address: &str, name: &str, role: &str, state: &str| {
            text::r(
                Locale::Zh,
                "r.session_line",
                &[
                    ("id", id),
                    ("address", address),
                    ("name", name),
                    ("role", &z(role)),
                    ("state", &z(state)),
                ],
            )
        };
        let expected = [
            text::r(Locale::Zh, "r.sessions_header", &[("crew", "示例机组")]),
            line(
                "s-cap",
                &format!("{n}-1"),
                "示例机长",
                "r.role_captain",
                "r.state_idle",
            ),
            line(
                "s-w1",
                &z("r.no_address"),
                "示例 worker",
                "r.role_worker",
                "r.state_working",
            ),
            line(
                &reg,
                &format!("{n}-2"),
                "示例登记",
                "r.role_worker",
                "r.state_stopped",
            ),
            z("r.sessions_tail"),
        ]
        .join("\n");
        assert_eq!(r.text, expected);
        assert!(!r.is_error);
        // Another crew: the management captain may list it by number; its own captain is not a
        // captain of mine.
        let other = f.add_crew("示例他组", None).await;
        f.add_session(&other, "s-ocap", Role::Captain, "他组机长")
            .await;
        let on = number_of(&f, &other).await;
        let r = run(&f, &f.captain, "list_sessions", json!({"crew": on})).await;
        not_yours(&f, &r, &other).await;
        let mgmt = management_crew(&f).await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        let r = run(&f, "s-boss", "list_sessions", json!({"crew": on})).await;
        let expected = [
            text::r(Locale::Zh, "r.sessions_header", &[("crew", "示例他组")]),
            line(
                "s-ocap",
                &format!("{on}-1"),
                "他组机长",
                "r.role_captain",
                "r.state_stopped",
            ),
            z("r.sessions_tail"),
        ]
        .join("\n");
        assert_eq!(r.text, expected);
        // A crew with no session yet.
        let empty = f.add_crew("示例空组", None).await;
        let en = number_of(&f, &empty).await;
        let r = run(&f, "s-boss", "list_sessions", json!({"crew": en})).await;
        assert_eq!(
            r.text,
            [
                text::r(Locale::Zh, "r.sessions_header", &[("crew", "示例空组")]),
                z("r.sessions_none"),
                z("r.sessions_tail"),
            ]
            .join("\n")
        );
    }

    #[tokio::test]
    async fn handoff_to_session_or_new_one() {
        let f = Fixture::new().await;
        // Exactly one of to_session / runner.
        for bad in [
            json!({}),
            json!({"to_session": f.worker, "runner": "codex"}),
            json!({"to_session": f.worker, "title": "新机长"}),
        ] {
            let r = run(&f, &f.captain, "handoff_captain", bad).await;
            says(&r, true, "r.handoff_one_of", &[]);
        }
        assert!(calls_of(&f, "Handoff(").is_empty());
        // Own crew, to an existing session (by id): I become a worker.
        let r = run(
            &f,
            &f.captain,
            "handoff_captain",
            json!({"to_session": f.worker}),
        )
        .await;
        let mut done = text::r(
            Locale::Zh,
            "r.handoff_done",
            &[
                ("crew", "示例机组"),
                ("new", "示例 worker"),
                ("id", "s-w1"),
                ("old", "示例机长"),
            ],
        );
        done.push('\n');
        done.push_str(&text::r(Locale::Zh, "r.handoff_self", &[]));
        assert_eq!(r.text, done);
        assert!(!r.is_error);
        assert_eq!(
            calls_of(&f, "Handoff(").pop().unwrap(),
            format!("{:?}", FakeCall::Handoff(f.crew.clone(), "s-w1".into()))
        );
        // The captain already: nothing handed over.
        let r = run(
            &f,
            &f.captain,
            "handoff_captain",
            json!({"to_session": f.captain}),
        )
        .await;
        says(
            &r,
            true,
            "r.handoff_already",
            &[("target", "示例机长"), ("crew", "示例机组")],
        );
        // A direct child crew: allowed; the successor must belong to it (collab's code, C1).
        let child = f.add_crew("示例子机组", Some(&f.crew)).await;
        f.add_session(&child, "s-ccap", Role::Captain, "示例子机长")
            .await;
        f.add_session(&child, "s-cw", Role::Worker, "示例子 worker")
            .await;
        let cn = number_of(&f, &child).await;
        let r = run(
            &f,
            &f.captain,
            "handoff_captain",
            json!({"crew": cn, "to_session": f.worker}),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::CrewSessionNotInCrew)
                .arg("session", "s-w1")
                .arg("crew", child.as_str()),
        );
        let r = run(
            &f,
            &f.captain,
            "handoff_captain",
            json!({"crew": cn, "to_session": "s-cw"}),
        )
        .await;
        says(
            &r,
            false,
            "r.handoff_done",
            &[
                ("crew", "示例子机组"),
                ("new", "示例子 worker"),
                ("id", "s-cw"),
                ("old", "示例子机长"),
            ],
        );
        // A new one in the child crew: started as a worker with the md defaults, then handed over.
        let id = next_fake_id(&f);
        let r = run(
            &f,
            &f.captain,
            "handoff_captain",
            json!({"crew": cn, "runner": "codex"}),
        )
        .await;
        says(
            &r,
            false,
            "r.handoff_done",
            &[
                ("crew", "示例子机组"),
                ("new", "机长"),
                ("id", &id),
                ("old", "示例子机长"),
            ],
        );
        let brief = text::r(Locale::Zh, "r.handoff_default_brief", &[]);
        assert_eq!(
            calls_of(&f, "Start(").pop().unwrap(),
            start_call(StartRequest {
                executor: "codex".into(),
                ..worker_start(&child, &brief, "机长", false)
            })
        );
        assert_eq!(
            calls_of(&f, "Handoff(").pop().unwrap(),
            format!("{:?}", FakeCall::Handoff(child.clone(), id.clone()))
        );
        // The new captain's title is collab's rule too.
        let r = run(
            &f,
            &f.captain,
            "handoff_captain",
            json!({"crew": cn, "runner": "codex", "title": "长".repeat(19)}),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::SessionTitleTooLong)
                .arg("max", "18")
                .arg("actual", "19"),
        );
        // A crew that is neither mine nor my direct child: not mine.
        let stranger = f.add_crew("示例陌生机组", None).await;
        f.add_session(&stranger, "s-st", Role::Worker, "陌生 worker")
            .await;
        let sn = number_of(&f, &stranger).await;
        let r = run(
            &f,
            &f.captain,
            "handoff_captain",
            json!({"crew": sn, "to_session": "s-st"}),
        )
        .await;
        not_yours(&f, &r, &stranger).await;
        // The management captain: any crew (this one has no captain yet).
        let mgmt = management_crew(&f).await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        let handoffs = calls_of(&f, "Handoff(").len();
        let r = run(
            &f,
            "s-boss",
            "handoff_captain",
            json!({"crew": sn, "to_session": "s-st"}),
        )
        .await;
        says(
            &r,
            false,
            "r.handoff_done_vacant",
            &[
                ("crew", "示例陌生机组"),
                ("new", "陌生 worker"),
                ("id", "s-st"),
            ],
        );
        assert_eq!(calls_of(&f, "Handoff(").len(), handoffs + 1);
    }

    /// S83: collab `crews::set_captain` (applied here as M3c does) demotes the old captain, who
    /// stays in the crew as a worker with an extension.
    #[tokio::test]
    async fn handoff_old_captain_stays_as_worker() {
        let f = Fixture::new().await;
        let r = run(
            &f,
            &f.captain,
            "handoff_captain",
            json!({"to_session": f.worker}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        f.apply_last_handoff().await;
        let old = f.me(&f.captain).await;
        assert_eq!(old.role, Role::Worker);
        assert_eq!(old.crew_id, f.crew);
        assert_eq!(f.me(&f.worker).await.role, Role::Captain);
        let n = number_of(&f, &f.crew).await;
        let ext = port::sessions_of(&f.state, &f.crew)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.id == f.captain)
            .and_then(|s| s.extension);
        assert_eq!(ext, Some(format!("{n}-2")), "a worker's extension");
        // Its captain tools are gone on the next call.
        let r = run(&f, &f.captain, "list_sessions", json!({})).await;
        says(&r, true, "r.captain_only", &[("name", "list_sessions")]);
    }

    /// C3: every session argument takes a directory address as well as an id.
    #[tokio::test]
    async fn session_id_accepts_an_address() {
        let f = Fixture::new().await;
        let n = number_of(&f, &f.crew).await;
        let reg = f.register(&f.crew, Role::Worker, "示例登记").await;
        let address = format!("{n}-2");
        let r = run(
            &f,
            &f.captain,
            "inspect_session",
            json!({"session_id": address}),
        )
        .await;
        let report = format!("session: {reg}\nrunning: false\nworking: false\nscreen:\n");
        says(
            &r,
            false,
            "r.inspect",
            &[("target", "示例登记"), ("report", &report)],
        );
        let r = run(
            &f,
            &f.captain,
            "nudge_session",
            json!({"session_id": address, "key": "esc"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(
            calls_of(&f, "Nudge(").pop().unwrap(),
            format!("{:?}", FakeCall::Nudge(reg.clone(), "esc".into()))
        );
        let r = run(
            &f,
            &f.captain,
            "stop_session",
            json!({"session_id": address, "reason": "示例"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(
            calls_of(&f, "Stop(").pop().unwrap(),
            format!("{:?}", FakeCall::Stop(reg.clone(), "示例".into()))
        );
        // A handoff successor by address.
        let r = run(
            &f,
            &f.captain,
            "handoff_captain",
            json!({"to_session": address}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(
            calls_of(&f, "Handoff(").pop().unwrap(),
            format!("{:?}", FakeCall::Handoff(f.crew.clone(), reg.clone()))
        );
        // An address nobody has: collab's code; the human is no session.
        let r = run(
            &f,
            &f.captain,
            "inspect_session",
            json!({"session_id": format!("{n}-9")}),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::DirectoryNoSuchExtension)
                .arg("number", n.as_str())
                .arg("ext", "9"),
        );
        let r = run(
            &f,
            &f.captain,
            "inspect_session",
            json!({"session_id": "human"}),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::SessionNotFound).arg("session", "human"),
        );
    }

    /// I1: the notice is posted before the stop; a stop that fails after it says the notice is out
    /// and nothing was stopped (a stop run before the post would fail with no notice: red).
    #[tokio::test]
    async fn a_stop_that_fails_after_the_notice_says_so() {
        let f = Fixture::new().await;
        f.fail_method(
            FakeMethod::Stop,
            ControlError::Failed("示例：进程没响应".into()),
        );
        let r = run(
            &f,
            &f.captain,
            "stop_session",
            json!({"session_id": f.worker, "reason": "示例：任务取消"}),
        )
        .await;
        says(
            &r,
            true,
            "r.stop_failed_after_notice",
            &[("target", "示例 worker"), ("detail", "示例：进程没响应")],
        );
        let m = last_lobby_message(&f, &f.crew).await;
        assert_eq!(
            m.text,
            text::r(
                Locale::Zh,
                "r.stop_notice",
                &[("target", "示例 worker"), ("reason", "示例：任务取消")]
            )
        );
        assert_eq!(calls_of(&f, "Stop(").len(), 1);
    }

    /// I1: no notice, no stop (fail closed): a lobby collab refuses to post in stops nothing.
    #[tokio::test]
    async fn a_stop_whose_notice_cannot_be_posted_stops_nothing() {
        let f = Fixture::new().await;
        f.rpc("crew.remove", json!({"crewId": f.crew})).await;
        let r = run(
            &f,
            &f.captain,
            "stop_session",
            json!({"session_id": f.worker, "reason": "示例"}),
        )
        .await;
        let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
        let detail = Refusal::new(Code::ChannelArchived)
            .arg("channel", lobby.as_str())
            .text(Locale::Zh);
        says(&r, true, "r.stop_not_done", &[("detail", &detail)]);
        assert!(calls_of(&f, "Stop(").is_empty());
    }

    /// I2: a new captain that started but was not handed the role is reported as such, with its
    /// id, so it is not started twice.
    #[tokio::test]
    async fn a_new_captain_started_but_not_handed_over_says_so() {
        let f = Fixture::new().await;
        f.fail_method(
            FakeMethod::Handoff,
            ControlError::Failed("示例：交接失败".into()),
        );
        let id = next_fake_id(&f);
        let r = run(
            &f,
            &f.captain,
            "handoff_captain",
            json!({"runner": "codex"}),
        )
        .await;
        says(
            &r,
            true,
            "r.handoff_started_not_handed",
            &[("new", "机长"), ("id", &id), ("detail", "示例：交接失败")],
        );
        assert_eq!(calls_of(&f, "Start(").len(), 1);
        assert_eq!(
            calls_of(&f, "Handoff("),
            [format!(
                "{:?}",
                FakeCall::Handoff(f.crew.clone(), id.clone())
            )]
        );
    }

    /// M4 / C5: a crew without a number gets the variant without `post(to:)`, never its raw id.
    #[tokio::test]
    async fn a_crew_without_a_number_is_named_not_numbered() {
        let f = Fixture::new().await;
        let other = f.add_crew("示例无号组", None).await;
        let id = other.clone();
        f.state
            .db
            .call(move |s| {
                s.conn()
                    .execute("UPDATE crews SET number=NULL WHERE id=?1", [&id])
                    .unwrap()
            })
            .await
            .unwrap();
        let r = run(&f, &f.captain, "list_sessions", json!({"crew": other})).await;
        says(
            &r,
            true,
            "r.not_your_crew_unnumbered",
            &[("crew", "示例无号组")],
        );
    }
}
