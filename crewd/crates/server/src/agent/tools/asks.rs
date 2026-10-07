//! Asks and self-configuration (spec §22.3; T14 supplement): `ask`, `withdraw_ask`,
//! `respond_todo`, `get_quota`, `schedule_wakeup`, `set_session_profile`.
//!
//! Every rule is collab's (C1): where an ask is announced (the home human channel, S68), who may
//! withdraw it (asker / the ask crew's captain / the management captain), empty question / reason /
//! response, unknown H or Todo numbers, a removed crew. These tools only check argument shape
//! (fallback pairs and ranges, the wakeup window, at least one profile field), call the port and
//! write receipts; a refusal reaches the agent through `Ctx::fail` with collab's code.
use crewd_proto::agent::CallResult;
use crewd_proto::{Mention, MentionKind};
use serde::Deserialize;
use serde_json::Value;

use super::channels::{Step, done};
use super::{Ctx, channel_label, crew_arg, parse_args};
use crate::agent::port::{self, PortError, Reason, Timer};

/// Furthest ahead an ask's fallback or a wakeup `at` may be: 7 days, in minutes.
const MAX_AHEAD_MINUTES: i64 = 7 * 24 * 60;
/// `schedule_wakeup.after_minutes` range (the catalog bounds).
const WAKEUP_MIN_MINUTES: f64 = 1.0;
const WAKEUP_MAX_MINUTES: f64 = 1440.0;
/// How much of a question / Todo item a receipt echoes (code points, ellipsis included).
const HEAD_CHARS: usize = 80;
const ELLIPSIS: char = '\u{2026}';

/// The first visible line of `s`, flattened and neutralised like a name (`safe_name`), at most
/// `HEAD_CHARS` code points.
fn head(cx: &Ctx<'_>, s: &str) -> String {
    let line = s
        .lines()
        .map(|l| cx.name(l))
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= HEAD_CHARS {
        return line;
    }
    let mut out: String = line.chars().take(HEAD_CHARS - 1).collect();
    out.push(ELLIPSIS);
    out
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskArgs {
    question: String,
    resume_note: Option<String>,
    fallback_after_minutes: Option<f64>,
    fallback: Option<String>,
}

pub async fn ask(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(ask_(cx, v).await)
}

async fn ask_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: AskArgs = parse_args(cx, v)?;
    let max = MAX_AHEAD_MINUTES as f64;
    let fallback = match (
        a.fallback_after_minutes,
        a.fallback.as_deref().map(str::trim),
    ) {
        (None, None) => None,
        (Some(m), Some(t)) if (1.0..=max).contains(&m) && !t.is_empty() => {
            Some((t.to_string(), m.ceil() as i64))
        }
        _ => return Err(cx.err("r.fallback_pair", &[])),
    };
    let note = a
        .resume_note
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    let had_note = note.is_some();
    let asked = port::open_ask(cx.st, cx.me, &a.question, fallback, note)
        .await
        .map_err(|e| cx.fail(e))?;
    // Asking is calling the human: this turn called someone (no relay at turn end).
    crate::agent::turn::note_post(
        cx.st,
        cx.me,
        &[Mention {
            kind: MentionKind::Human,
            target_id: None,
        }],
    )
    .await;
    let mut text = cx.r(
        "r.asked",
        &[
            ("ask", &asked.ask),
            ("question_head", &head(cx, &a.question)),
            ("channel", &channel_label(cx, &asked.channel).await),
        ],
    );
    if !had_note {
        text.push('\n');
        text.push_str(&cx.r("r.ask_no_resume_note", &[]));
    }
    Ok(CallResult::ok(text))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WithdrawArgs {
    ask: String,
    reason: String,
    /// Whose book holds the ask (number or id); default = my crew.
    crew: Option<String>,
}

pub async fn withdraw_ask(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(withdraw_ask_(cx, v).await)
}

async fn withdraw_ask_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    use crewd_collab::refusal::{Code, Refusal};
    let a: WithdrawArgs = parse_args(cx, v)?;
    let crew = match a.crew.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(arg) => crew_arg(cx, arg).await?,
        None => port::crew(cx.st, &cx.me.crew_id)
            .await
            .map_err(|e| cx.fail(e))?
            .ok_or_else(|| {
                cx.fail(PortError::NotFound(Reason::Rule(
                    Refusal::new(Code::CrewNotFound).arg("crew", cx.me.crew_id.clone()),
                )))
            })?,
    };
    let ask = port::withdraw_ask(cx.st, cx.me, &crew.id, &a.ask, &a.reason)
        .await
        .map_err(|e| cx.fail(e))?;
    Ok(cx.ok(
        "r.withdrawn",
        &[("ask", &ask), ("crew", &cx.name(&crew.title))],
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RespondArgs {
    number: i64,
    response: String,
}

pub async fn respond_todo(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(respond_todo_(cx, v).await)
}

async fn respond_todo_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: RespondArgs = parse_args(cx, v)?;
    let item = port::respond_todo(cx.st, cx.me, a.number, &a.response)
        .await
        .map_err(|e| cx.fail(e))?;
    Ok(cx.ok(
        "r.responded",
        &[
            ("number", &a.number.to_string()),
            ("todo_head", &head(cx, &item)),
        ],
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

pub async fn get_quota(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(get_quota_(cx, v).await)
}

async fn get_quota_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let NoArgs {} = parse_args(cx, v)?;
    let report = port::quota(cx.st).await.map_err(|e| cx.fail(e))?;
    if report.trim().is_empty() {
        return Ok(cx.ok("r.quota_none", &[]));
    }
    Ok(cx.ok("r.quota", &[("report", &report)]))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WakeArgs {
    after_minutes: Option<f64>,
    at: Option<String>,
    note: Option<String>,
}

pub async fn schedule_wakeup(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(schedule_wakeup_(cx, v).await)
}

async fn schedule_wakeup_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: WakeArgs = parse_args(cx, v)?;
    let Some(note) = a
        .note
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
    else {
        return Err(cx.err("r.wakeup_note", &[]));
    };
    let now = port::now_ms();
    let ceiling = now + MAX_AHEAD_MINUTES * 60_000;
    let at = match (a.after_minutes, a.at.as_deref()) {
        (Some(m), None) if (WAKEUP_MIN_MINUTES..=WAKEUP_MAX_MINUTES).contains(&m) => {
            now + (m * 60_000.0) as i64
        }
        (None, Some(s)) => match chrono::DateTime::parse_from_rfc3339(s.trim()) {
            Ok(t) if t.timestamp_millis() > now && t.timestamp_millis() <= ceiling => {
                t.timestamp_millis()
            }
            _ => return Err(cx.err("r.wakeup_when", &[])),
        },
        _ => return Err(cx.err("r.wakeup_when", &[])),
    };
    match port::schedule_timer(cx.st, &cx.me.session_id, at, &note)
        .await
        .map_err(|e| cx.fail(e))?
    {
        Timer::Scheduled => Ok(cx.ok("r.wakeup_set", &[("at", &wake_time(at))])),
        // S85: my crew was removed, nothing was queued; say so instead of a false receipt.
        Timer::Skipped => Err(cx.err("r.wakeup_skipped", &[])),
    }
}

/// A wakeup time as written in receipts: RFC 3339, UTC, whole seconds.
fn wake_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileArgs {
    model: Option<String>,
    effort: Option<String>,
}

pub async fn set_session_profile(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(set_session_profile_(cx, v).await)
}

async fn set_session_profile_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: ProfileArgs = parse_args(cx, v)?;
    let model = a
        .model
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    let effort = a
        .effort
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty());
    if model.is_none() && effort.is_none() {
        return Err(cx.err("r.profile_empty", &[]));
    }
    port::set_profile(cx.st, &cx.me.session_id, model, effort)
        .await
        .map_err(|e| cx.fail(e))?;
    // B8 / spec C5: the switch applies at the next start, not this turn.
    Ok(cx.ok("r.profile_next_start", &[]))
}

#[cfg(test)]
mod tests {
    use crewd_collab::refusal::{Code, Refusal};
    use crewd_proto::{ChannelKind, MentionKind};
    use serde_json::json;

    use super::wake_time;
    use crate::agent::Role;
    use crate::agent::port;
    use crate::agent::testkit::Fixture;
    use crate::agent::text::{self, Locale};
    use crate::agent::tools::{call, channel_label_raw};

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

    /// The tool's own error receipt, whole.
    #[track_caller]
    fn tool_error(r: &crewd_proto::agent::CallResult, key: &str) {
        assert!(r.is_error, "expected {key}, got: {}", r.text);
        assert_eq!(r.text, text::r(Locale::Zh, key, &[]));
    }

    fn human_label(crew_title: &str) -> String {
        channel_label_raw(Locale::Zh, ChannelKind::Human, "", crew_title)
    }

    /// `r.asked` + the no-resume-note hint, as the tool writes it.
    fn asked_receipt(ask: &str, head: &str, channel: &str, with_note: bool) -> String {
        let mut t = text::r(
            Locale::Zh,
            "r.asked",
            &[("ask", ask), ("question_head", head), ("channel", channel)],
        );
        if !with_note {
            t.push('\n');
            t.push_str(&text::r(Locale::Zh, "r.ask_no_resume_note", &[]));
        }
        t
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

    /// S68: the ask is announced in the home (top-level ancestor) human channel; the management
    /// crew's own asks go to the management human channel. The receipt names where it went.
    #[tokio::test]
    async fn ask_goes_to_the_home_human_channel() {
        let f = Fixture::new().await;
        let child = f.add_crew("示例子机组", Some(&f.crew)).await;
        f.add_session(&child, "s-child", Role::Worker, "示例子 worker")
            .await;
        let r = run(
            &f,
            "s-child",
            "ask",
            json!({"question": "A / B，我倾向 A", "resume_note": "答复后接着改 B 模块"}),
        )
        .await;
        assert_eq!(
            r.text,
            asked_receipt("H1", "A / B，我倾向 A", &human_label("示例机组"), true)
        );
        assert!(!r.is_error);
        let home = port::home_human_channel(&f.state, &child).await.unwrap();
        assert_eq!(
            home.crew_id, f.crew,
            "the top-level ancestor's human channel"
        );
        let m = port::messages_raw(&f.state, &home.id, None, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(m.text, "A / B，我倾向 A");
        assert!(m.mentions.iter().any(|x| x.kind == MentionKind::Human));
        // Numbers come from the asking crew's book, consecutively.
        let r2 = run(&f, "s-child", "ask", json!({"question": "第二个问题"})).await;
        assert_eq!(
            r2.text,
            asked_receipt("H2", "第二个问题", &human_label("示例机组"), false)
        );

        let mgmt = management_crew(&f).await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        let r = run(&f, "s-boss", "ask", json!({"question": "全局冻结发版吗？"})).await;
        let title = port::crew(&f.state, &mgmt).await.unwrap().unwrap().title;
        assert_eq!(
            r.text,
            asked_receipt("H1", "全局冻结发版吗？", &human_label(&title), false)
        );
        let mh = port::home_human_channel(&f.state, &mgmt).await.unwrap();
        assert_eq!(mh.crew_id, mgmt);
        let m = port::messages_raw(&f.state, &mh.id, None, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(m.text, "全局冻结发版吗？");
    }

    /// S73: the receipt echoes the first visible line of the question, flattened, at most 80
    /// code points (ellipsis included).
    #[tokio::test]
    async fn ask_receipt_echoes_the_question_head() {
        let f = Fixture::new().await;
        let home = human_label("示例机组");
        let r = run(
            &f,
            &f.worker,
            "ask",
            json!({"question": "\n  \u{200B}\n示例【标题】行\u{202E}\n第二行不回显", "resume_note": "接着做"}),
        )
        .await;
        assert_eq!(r.text, asked_receipt("H1", "示例「标题」行", &home, true));
        let long = "长".repeat(100);
        let r = run(
            &f,
            &f.worker,
            "ask",
            json!({"question": long, "resume_note": "接着做"}),
        )
        .await;
        let head = format!("{}\u{2026}", "长".repeat(79));
        assert_eq!(head.chars().count(), 80);
        assert_eq!(r.text, asked_receipt("H2", &head, &home, true));
        // A blank question is collab's refusal, not a tool-side copy (C1).
        let r = run(&f, &f.worker, "ask", json!({"question": " \n "})).await;
        refused(&r, Refusal::new(Code::AskQuestionEmpty));
    }

    #[tokio::test]
    async fn fallback_must_come_in_pairs_and_in_range() {
        let f = Fixture::new().await;
        for bad in [
            json!({"question": "q", "fallback": "就按 A"}),
            json!({"question": "q", "fallback_after_minutes": 30}),
            json!({"question": "q", "fallback_after_minutes": 0, "fallback": "A"}),
            json!({"question": "q", "fallback_after_minutes": 10081, "fallback": "A"}),
            json!({"question": "q", "fallback_after_minutes": 30, "fallback": "  "}),
        ] {
            let r = run(&f, &f.worker, "ask", bad.clone()).await;
            tool_error(&r, "r.fallback_pair");
        }
        let r = run(
            &f,
            &f.worker,
            "ask",
            json!({"question": "q", "fallback_after_minutes": 10080, "fallback": "A"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
    }

    #[tokio::test]
    async fn asking_counts_as_calling_out() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        crate::agent::turn::stop(
            &f.state,
            &me,
            &json!({"hook_event_name": "Stop", "prompt_id": "p0", "last_assistant_message": "好。"}),
        )
        .await;
        let r = run(&f, &f.worker, "ask", json!({"question": "要不要做 B？"})).await;
        assert!(!r.is_error, "{}", r.text);
        let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
        let before = port::latest_seq(&f.state, &lobby).await.unwrap();
        crate::agent::turn::stop(
            &f.state,
            &me,
            &json!({"hook_event_name": "Stop", "prompt_id": "p1", "last_assistant_message": "要不要做 B？"}),
        )
        .await;
        assert_eq!(
            port::latest_seq(&f.state, &lobby).await.unwrap(),
            before,
            "the ask called the human: no relay into the lobby"
        );
    }

    /// Who may withdraw is collab's (`asks::withdraw`): asker yes, crewmate no, own captain yes,
    /// the management captain in any crew via `crew`; refusals carry collab's codes.
    #[tokio::test]
    async fn withdraw_follows_collab_permissions() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        for q in ["q1", "q2", "q3"] {
            let r = run(&f, &f.worker, "ask", json!({"question": q})).await;
            assert!(!r.is_error, "{}", r.text);
        }
        let crew = "示例机组";
        // A crewmate may not.
        let r = run(
            &f,
            "s-w2",
            "withdraw_ask",
            json!({"ask": "h1", "reason": "不是我的"}),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::AskWithdrawForbidden).arg("ask", "H1"),
        );
        // The asker may; the receipt uses the canonical id and names the crew.
        let r = run(
            &f,
            &f.worker,
            "withdraw_ask",
            json!({"ask": "h1", "reason": "已经发版了"}),
        )
        .await;
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.withdrawn", &[("ask", "H1"), ("crew", crew)])
        );
        assert!(!r.is_error);
        // Withdrawn already: collab's conflict.
        let r = run(
            &f,
            &f.worker,
            "withdraw_ask",
            json!({"ask": "H1", "reason": "再撤"}),
        )
        .await;
        refused(&r, Refusal::new(Code::AskNotOpen).arg("ask", "H1"));
        // The reason is collab's rule too (no tool-side copy).
        let r = run(
            &f,
            &f.worker,
            "withdraw_ask",
            json!({"ask": "H2", "reason": " "}),
        )
        .await;
        refused(&r, Refusal::new(Code::AskReasonEmpty));
        // The crew's own captain may.
        let r = run(
            &f,
            &f.captain,
            "withdraw_ask",
            json!({"ask": "H2", "reason": "人当面答了"}),
        )
        .await;
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.withdrawn", &[("ask", "H2"), ("crew", crew)])
        );
        // The management captain may in any crew, by naming it; without `crew` it looks in its
        // own book.
        let mgmt = management_crew(&f).await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        let r = run(
            &f,
            "s-boss",
            "withdraw_ask",
            json!({"ask": "H3", "reason": "管理层撤"}),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::AskIdNotFound)
                .arg("crew", mgmt.as_str())
                .arg("id", "H3"),
        );
        let n = number_of(&f, &f.crew).await;
        let r = run(
            &f,
            "s-boss",
            "withdraw_ask",
            json!({"ask": "H3", "reason": "管理层撤", "crew": n}),
        )
        .await;
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.withdrawn", &[("ask", "H3"), ("crew", crew)])
        );
        // A plain management session is a plain session in another crew's book.
        f.add_session(&mgmt, "s-mw", Role::Worker, "示例总机组 worker")
            .await;
        let r = run(&f, &f.worker, "ask", json!({"question": "q4"})).await;
        assert!(!r.is_error, "{}", r.text);
        let r = run(
            &f,
            "s-mw",
            "withdraw_ask",
            json!({"ask": "H4", "reason": "x", "crew": f.crew}),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::AskWithdrawForbidden).arg("ask", "H4"),
        );
    }

    /// S73: the receipt echoes the item answered; the status does not change (§9).
    #[tokio::test]
    async fn respond_todo_echoes_the_item_and_keeps_status() {
        let f = Fixture::new().await;
        f.add_agent_todo(1, "示例：把文档补齐\n第二行").await;
        let r = run(
            &f,
            &f.worker,
            "respond_todo",
            json!({"number": 1, "response": "认领，今天做"}),
        )
        .await;
        assert_eq!(
            r.text,
            text::r(
                Locale::Zh,
                "r.responded",
                &[("number", "1"), ("todo_head", "示例：把文档补齐")]
            )
        );
        assert!(!r.is_error);
        let todos = f
            .rpc("todo.list", json!({"crewId": f.crew, "ledger": "agent"}))
            .await;
        let t = &todos.as_array().unwrap()[0];
        assert_eq!(t["responses"][0]["text"], "认领，今天做");
        assert_eq!(t["status"], "pending", "回应不改状态（§9）");
        let r = run(
            &f,
            &f.worker,
            "respond_todo",
            json!({"number": 99, "response": "x"}),
        )
        .await;
        refused(&r, Refusal::new(Code::TodoNotFound).arg("number", "99"));
        let r = run(
            &f,
            &f.worker,
            "respond_todo",
            json!({"number": 1, "response": "  "}),
        )
        .await;
        refused(&r, Refusal::new(Code::TodoResponseEmpty));
        let r = run(
            &f,
            &f.worker,
            "respond_todo",
            json!({"number": 1, "response": "x", "status": "completed"}),
        )
        .await;
        assert!(
            r.is_error && r.text.contains("status"),
            "the status param is gone: {}",
            r.text
        );
    }

    #[tokio::test]
    async fn schedule_wakeup_is_persisted_as_a_timer() {
        let f = Fixture::new().await;
        let r = run(
            &f,
            &f.worker,
            "schedule_wakeup",
            json!({"after_minutes": 0, "note": "x"}),
        )
        .await;
        tool_error(&r, "r.wakeup_when");
        let r = run(
            &f,
            &f.worker,
            "schedule_wakeup",
            json!({"at": "2001-01-01T00:00:00Z", "note": "x"}),
        )
        .await;
        tool_error(&r, "r.wakeup_when");
        let r = run(
            &f,
            &f.worker,
            "schedule_wakeup",
            json!({"after_minutes": 5, "at": "2099-01-01T00:00:00Z", "note": "x"}),
        )
        .await;
        tool_error(&r, "r.wakeup_when");
        let r = run(
            &f,
            &f.worker,
            "schedule_wakeup",
            json!({"after_minutes": 5}),
        )
        .await;
        tool_error(&r, "r.wakeup_note");
        assert!(f.wakes_of(&f.worker).await.is_empty());
        let before = port::now_ms();
        let r = run(
            &f,
            &f.worker,
            "schedule_wakeup",
            json!({"after_minutes": 30, "note": "额度重置后接着跑全量"}),
        )
        .await;
        let w = f.wakes_of(&f.worker).await;
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].0, "timer");
        assert!(w[0].1 >= before + 30 * 60_000 && w[0].1 <= port::now_ms() + 30 * 60_000);
        assert_eq!(w[0].2, "额度重置后接着跑全量");
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.wakeup_set", &[("at", &wake_time(w[0].1))])
        );
        assert!(!r.is_error);
        // `at` in the future works too.
        let at = port::now_ms() + 2 * 3_600_000;
        let r = run(
            &f,
            &f.worker,
            "schedule_wakeup",
            json!({"at": wake_time(at), "note": "两小时后"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(f.wakes_of(&f.worker).await.len(), 2);
    }

    /// The same 7-day ceiling as `fallback_after_minutes`.
    #[tokio::test]
    async fn at_beyond_seven_days_is_refused() {
        let f = Fixture::new().await;
        let day = 24 * 3_600_000;
        let r = run(
            &f,
            &f.worker,
            "schedule_wakeup",
            json!({"at": wake_time(port::now_ms() + 7 * day + 3_600_000), "note": "太远"}),
        )
        .await;
        tool_error(&r, "r.wakeup_when");
        assert!(f.wakes_of(&f.worker).await.is_empty());
        let r = run(
            &f,
            &f.worker,
            "schedule_wakeup",
            json!({"at": wake_time(port::now_ms() + 6 * day), "note": "六天后"}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(f.wakes_of(&f.worker).await.len(), 1);
    }

    /// S85 (scheduling side): collab skips a wake for a removed crew; the tool says so instead of
    /// a false "scheduled". An ask in a removed crew is collab's refusal.
    #[tokio::test]
    async fn schedule_wakeup_in_a_removed_crew_is_refused() {
        let f = Fixture::new().await;
        f.rpc("crew.remove", json!({"crewId": f.crew})).await;
        let r = run(
            &f,
            &f.worker,
            "schedule_wakeup",
            json!({"after_minutes": 30, "note": "示例"}),
        )
        .await;
        tool_error(&r, "r.wakeup_skipped");
        assert!(f.wakes_of(&f.worker).await.is_empty());
        let r = run(&f, &f.worker, "ask", json!({"question": "还能问吗？"})).await;
        refused(
            &r,
            Refusal::new(Code::CrewRemoved).arg("crew", f.crew.as_str()),
        );
    }

    #[tokio::test]
    async fn quota_and_profile_go_through_session_control() {
        let f = Fixture::new().await;
        let r = run(&f, &f.worker, "get_quota", json!({})).await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.quota_none", &[]),
            "the fake reports no lines"
        );
        let r = run(&f, &f.worker, "set_session_profile", json!({})).await;
        tool_error(&r, "r.profile_empty");
        let r = run(
            &f,
            &f.worker,
            "set_session_profile",
            json!({"model": "sonnet"}),
        )
        .await;
        assert_eq!(
            r.text,
            text::r(Locale::Zh, "r.profile_next_start", &[]),
            "B8: the receipt says next start"
        );
        assert!(!r.is_error);
        assert!(
            f.control_calls()
                .iter()
                .any(|c| c == "SetProfile(\"s-w1\", Some(\"sonnet\"), None)"),
            "{:?}",
            f.control_calls()
        );
        assert!(f.control_calls().iter().any(|c| c == "Quota"));
    }
}
