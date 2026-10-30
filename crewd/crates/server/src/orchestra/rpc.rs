//! M3c 的客户端方法（spec §22.4「客户端 RPC（M3c 新增）」，T10b 补充 §2.1、§2.4、§2.7）。客户端都是人：
//! 起 / 恢复不受可选上限、不错开（`Origin::Human`）。dispatch 只在方法属于 [`handles`] 时才交过来；编排没起来
//! （库里的 `serve()` 不带执行器，C1）那边回 -32009 `orchestra.not_running`。
//!
//! 错误（C14 / S26）：编排的 `ControlError` 一律带拒绝码（`data.refusal`，客户端按码本地化）；内部错误 = -32603 带编号、
//! 原文只进日志；存储故障 = -32010。
use std::time::Duration;

use crewd_collab::refusal::{Code, Refusal};
use crewd_proto::orchestra::{
    BlockerCodeView, CrewSetCaptainParams, HibernateParams, HibernateResult, LatchScopeView,
    LatchView, LocaleView, LoopRecoverParams, LoopRecovered, MessageReceiptsParams, Receipt,
    ReceiptStateView, SessionIdParams, SessionStartParams, SessionStopParams, Settings,
    SettingsPatch, TargetView, WakeBlocker, WakeKindView, WakeListParams, WakeListResult,
    WakePriorityView, WakeStateView, WakeView, methods,
};
use crewd_proto::{Empty, Role, RpcError, parse_no_params, parse_params, parse_params_or_default};
use crewd_wake::breaker::{edge_id, edge_parts};
use crewd_wake::explain::Blocker;
use crewd_wake::text::Lang;
use crewd_wake::{Latch, LatchScope, Priority, WakeKind, WakeState};
use serde::Serialize;
use serde_json::Value;

use super::Handle;
use super::control::{decision_view, live_to_proto};
use super::core::{ControlError, Origin, RefusedKind, SettingsUpdate, SettingsView, StartSession};
use super::deliver::{ListedWake, ReceiptRow, TargetSummary, WakeList};
use super::hibernate::HibernateReport;
use crate::AppState;
use crate::dispatch;

/// 这个方法归编排（M3c）：dispatch 在「未知方法」之前问它。
pub fn handles(method: &str) -> bool {
    methods::ALL.contains(&method) || super::codex_rpc::handles(method)
}

/// 编排的错误 → RPC 错误（T10b 补充 §2.4）。
pub fn rpc_err(e: ControlError) -> RpcError {
    let code = match &e {
        ControlError::Internal(detail) => return dispatch::internal(detail),
        ControlError::Storage { kind, detail } => return dispatch::storage(*kind, detail),
        ControlError::NotFound(_) => RpcError::NOT_FOUND,
        ControlError::Invalid(_) => RpcError::INVALID_PARAMS,
        ControlError::Refused { kind, .. } => match kind {
            RefusedKind::NotFound => RpcError::NOT_FOUND,
            RefusedKind::Invalid => RpcError::INVALID_PARAMS,
            RefusedKind::Forbidden => RpcError::FORBIDDEN,
            RefusedKind::Conflict => RpcError::CONFLICT,
        },
        ControlError::AlreadyRunning(_)
        | ControlError::CaptainTaken(_)
        | ControlError::NotRunning(_)
        | ControlError::NoDecision(_)
        | ControlError::DecisionChanged { .. }
        | ControlError::NeedsTerminal(_)
        | ControlError::NothingToInterrupt(_)
        | ControlError::InputDraft { .. }
        | ControlError::AtCap { .. }
        | ControlError::BinaryReplaced
        | ControlError::ShuttingDown
        | ControlError::NoWorkdir(_)
        | ControlError::Worktree(_)
        | ControlError::CrewNoWorkdir(_)
        | ControlError::CrewRemoved(_)
        | ControlError::Unsupported(_) => RpcError::CONFLICT,
    };
    // `decision.changed` 带上此刻屏上的框（`data.decision`，T10d；没有框了就不带）。
    let now = match &e {
        ControlError::DecisionChanged { now: Some(d), .. } => Some(decision_view(d)),
        _ => None,
    };
    match e.refusal() {
        Some(r) => {
            let err = dispatch::refused(code, r);
            match now {
                Some(d) => err.with_decision(d),
                None => err,
            }
        }
        // 到不了：除了内部 / 存储错误，编排的错误都带拒绝码（`refusal` 的定义）。
        None => dispatch::internal(e),
    }
}

fn to_value<T: Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(dispatch::internal)
}

fn kind_view(k: WakeKind) -> WakeKindView {
    match k {
        WakeKind::Message => WakeKindView::Message,
        WakeKind::Timer => WakeKindView::Timer,
        WakeKind::AskAnswer => WakeKindView::AskAnswer,
        WakeKind::Restart => WakeKindView::Restart,
        WakeKind::System => WakeKindView::System,
        WakeKind::Checkpoint => WakeKindView::Checkpoint,
        WakeKind::StopCheck => WakeKindView::StopCheck,
    }
}

fn priority_view(p: Priority) -> WakePriorityView {
    match p {
        Priority::Auto => WakePriorityView::Auto,
        Priority::System => WakePriorityView::System,
        Priority::Human => WakePriorityView::Human,
    }
}

fn state_view(s: WakeState) -> WakeStateView {
    match s {
        WakeState::Pending => WakeStateView::Pending,
        WakeState::Inflight => WakeStateView::Inflight,
        WakeState::Delivered => WakeStateView::Delivered,
        WakeState::Dropped => WakeStateView::Dropped,
    }
}

fn receipt_state(s: WakeState) -> ReceiptStateView {
    match s {
        WakeState::Pending => ReceiptStateView::Queued,
        WakeState::Inflight => ReceiptStateView::Delivering,
        WakeState::Delivered => ReceiptStateView::Delivered,
        WakeState::Dropped => ReceiptStateView::Dropped,
    }
}

/// crewd-wake 的原因 → 线上词。词逐字相同（`blocker_codes_match_the_wire_words` 钉住）。
pub(crate) fn blocker_view(b: &Blocker) -> WakeBlocker {
    let code = match b {
        Blocker::Causal { blockage } => {
            if !blockage.holds.is_empty() {
                BlockerCodeView::Held
            } else if !blockage.unresolved.is_empty() {
                BlockerCodeView::CausalUnresolved
            } else if !blockage.hold_candidates.is_empty() {
                BlockerCodeView::CausalBudget
            } else {
                BlockerCodeView::CausalCursorUnknown
            }
        }
        Blocker::ShuttingDown => BlockerCodeView::ShuttingDown,
        Blocker::CrewRemoved => BlockerCodeView::CrewRemoved,
        Blocker::Starting => BlockerCodeView::Starting,
        Blocker::NotDue { .. } => BlockerCodeView::NotDue,
        Blocker::Held { .. } => BlockerCodeView::Held,
        Blocker::BehindHigherPriority => BlockerCodeView::BehindHigherPriority,
        Blocker::Inflight => BlockerCodeView::Inflight,
        Blocker::Working => BlockerCodeView::Working,
        Blocker::Decision => BlockerCodeView::Decision,
        Blocker::InputDraft => BlockerCodeView::InputDraft,
        Blocker::Stopping => BlockerCodeView::Stopping,
        Blocker::Quota => BlockerCodeView::Quota,
        Blocker::AuthRequired => BlockerCodeView::AuthRequired,
        Blocker::LaunchFailed => BlockerCodeView::LaunchFailed,
        Blocker::AwaitingRestartAnswer => BlockerCodeView::AwaitingRestartAnswer,
        Blocker::QuotaNotReset => BlockerCodeView::QuotaNotReset,
        Blocker::AtCap { .. } => BlockerCodeView::AtCap,
        Blocker::Stagger { .. } => BlockerCodeView::Stagger,
    };
    let detail = match b {
        Blocker::Held { scope, id } => Some(format!("{scope}:{id}")),
        Blocker::AtCap { max } => Some(max.to_string()),
        _ => None,
    };
    WakeBlocker {
        causal: match b {
            Blocker::Causal { blockage } => Some(causal_blockage_view(blockage)),
            _ => None,
        },
        code,
        until_ms: b.until().map(|t| t.0),
        detail,
    }
}

fn causal_blockage_view(
    b: &crewd_wake::explain::CausalBlockage,
) -> crewd_proto::orchestra::CausalBlockageView {
    use crewd_proto::orchestra::{
        CausalBlockageView, CausalChainCandidateView, CausalChainHoldView, CausalUnresolvedView,
    };
    CausalBlockageView {
        cursor_unknown: b.cursor_unknown,
        holds: b
            .holds
            .iter()
            .map(|h| CausalChainHoldView {
                chain_id: h.chain_id.clone(),
                from: h.from.clone(),
                target_session_id: h.target.clone(),
                hop: h.hop,
                reason: h.reason.clone(),
                at_ms: h.at_ms,
            })
            .collect(),
        unresolved: b
            .unresolved
            .iter()
            .map(|u| CausalUnresolvedView {
                message_id: u.message_id.clone(),
                seq: u.seq,
                from: u.from.clone(),
                target_session_id: u.target.clone(),
                output_id: u.output_id.clone(),
            })
            .collect(),
        hold_candidates: b
            .hold_candidates
            .iter()
            .map(|h| CausalChainCandidateView {
                chain_id: h.chain_id.clone(),
                from: h.from.clone(),
                target_session_id: h.target.clone(),
                hop: h.hop,
                reason: h.reason.clone(),
            })
            .collect(),
    }
}

fn wake_view(w: &ListedWake) -> WakeView {
    let q = &w.wake;
    WakeView {
        id: q.id,
        session_id: q.session.clone(),
        kind: kind_view(q.kind),
        source: q.source.clone(),
        origin: q.origin.clone(),
        priority: priority_view(q.priority),
        not_before: q.not_before.0,
        state: state_view(q.state),
        attempts: q.attempts,
        note: q.note.clone(),
        blocker: w.blocker.as_ref().map(blocker_view),
        last_error: q.last_error.clone(),
        message_id: super::port::message_id(q).map(str::to_string),
    }
}

fn target_view(t: &TargetSummary) -> TargetView {
    TargetView {
        causal_diagnostics: t
            .causal_diagnostics
            .iter()
            .map(|(channel, b)| crewd_proto::orchestra::CausalChannelView {
                channel_id: channel.clone(),
                blockage: causal_blockage_view(b),
            })
            .collect(),
        session_id: t.session.clone(),
        running: t.running,
        inflight_wake_ids: t.inflight_wake_ids.clone(),
        delivery_id: t.delivery_id,
        writes_queued: u32::try_from(t.writes_queued).unwrap_or(u32::MAX),
        last_outcome: t.last_outcome.map(str::to_string),
    }
}

fn wake_list_view(l: &WakeList) -> WakeListResult {
    WakeListResult {
        wakes: l.wakes.iter().map(wake_view).collect(),
        targets: l.targets.iter().map(target_view).collect(),
    }
}

fn latch_view(l: &Latch) -> LatchView {
    match l.scope {
        LatchScope::Session => LatchView {
            scope: LatchScopeView::Session,
            target_session_id: l.id.clone(),
            from: None,
            chain_id: None,
            reason: l.reason.clone(),
            at_ms: l.at.0,
        },
        LatchScope::Edge => {
            let (from, to) = edge_parts(&l.id).unwrap_or(("", l.id.as_str()));
            LatchView {
                scope: LatchScopeView::Edge,
                target_session_id: to.to_string(),
                from: Some(from.to_string()),
                chain_id: None,
                reason: l.reason.clone(),
                at_ms: l.at.0,
            }
        }
    }
}

fn settings_view(s: &SettingsView) -> Settings {
    Settings {
        locale: match s.lang {
            Lang::Zh => LocaleView::Zh,
            Lang::En => LocaleView::En,
        },
        max_live_sessions: s.max_live.map(|n| u32::try_from(n).unwrap_or(u32::MAX)),
        hibernate_deadline_secs: s.hibernate_deadline_secs,
    }
}

/// `settings.set` 的参数 → 编排的更新。先全部核对、再写（一个值不对就什么都不改）。
fn settings_update(p: SettingsPatch) -> Result<SettingsUpdate, ControlError> {
    let lang = match p.locale.as_ref().map(|l| l.as_deref()) {
        None => None,
        Some(Some("zh")) => Some(Lang::Zh),
        Some(Some("en")) => Some(Lang::En),
        // 语言清不掉：null 与别的词一样是坏值（修复轮 M5）。
        Some(other) => {
            return Err(ControlError::Invalid(
                Refusal::new(Code::SettingsBadLocale).arg("value", other.unwrap_or("null")),
            ));
        }
    };
    if p.max_live_sessions == Some(Some(0)) {
        return Err(ControlError::Invalid(Refusal::new(
            Code::SettingsMaxLiveMin,
        )));
    }
    Ok(SettingsUpdate {
        lang,
        max_live: p.max_live_sessions.map(|v| v.map(|n| n as usize)),
        hibernate_deadline_secs: p.hibernate_deadline_secs,
    })
}

fn hibernate_view(r: HibernateReport) -> HibernateResult {
    HibernateResult {
        clean: r.clean,
        interrupted: r.interrupted,
        failed: r.failed,
        completed: r.completed,
    }
}

fn receipt_view(r: &ReceiptRow) -> Receipt {
    Receipt {
        session_id: r.session.clone(),
        state: receipt_state(r.state),
        working: r.working,
        attempts: r.attempts,
        last_error: r.last_error.clone(),
    }
}

// Owning read after the accepted start completes; mapping remains outside the FIFO job.
async fn read_started_session(
    state: &AppState,
    id: String,
) -> Result<crewd_proto::Session, RpcError> {
    state
        .db
        .call_result(move |s| crewd_collab::sessions::get(s.conn(), &id))
        .await
        .map_err(dispatch::internal)?
        .map_err(dispatch::collab_err)
}

/// 一个 M3c 方法（[`handles`] 已核过）。
pub async fn call(
    h: &Handle,
    state: &AppState,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    if super::codex_rpc::handles(method) {
        return super::codex_rpc::read(h, state, method, params).await;
    }
    match method {
        methods::SESSION_INSPECT => {
            let p: crewd_proto::orchestra::SessionInspectParams = parse_params(params)?;
            let (live, screen) = h
                .inspect(&p.session_id, p.max_lines.unwrap_or(60).min(400) as usize)
                .await
                .map_err(rpc_err)?;
            let runner_request = h
                .runner_request(&p.session_id)
                .await
                .map_err(rpc_err)?
                .filter(|request| {
                    live.pending_decision
                        .as_ref()
                        .is_some_and(|d| d.fingerprint() == request.fingerprint)
                })
                .and_then(|request| {
                    let crewd_term::DecisionKind::RunnerRequest { request_id } =
                        &live.pending_decision.as_ref()?.kind
                    else {
                        return None;
                    };
                    Some(crewd_proto::orchestra::RunnerRequestView {
                        runner_request_id: request_id.clone(),
                        fingerprint: request.fingerprint,
                        method: request.method,
                        params: request.params,
                    })
                });
            to_value(crewd_proto::orchestra::SessionInspection {
                live: live_to_proto(&live),
                screen,
                runner_request,
            })
        }
        methods::SESSION_INTERRUPT => {
            let p: SessionIdParams = parse_params(params)?;
            h.interrupt(&p.session_id).await.map_err(rpc_err)?;
            to_value(Empty {})
        }
        methods::SESSION_RESPOND_REQUEST => {
            let p: crewd_proto::orchestra::SessionRespondRequestParams = parse_params(params)?;
            let r = h
                .respond_request(&p.session_id, &p.fingerprint, p.response)
                .await
                .map_err(rpc_err)?;
            to_value(crewd_proto::orchestra::SessionAnswerResult {
                cleared: r.cleared,
                decision: r.now.as_ref().map(decision_view),
            })
        }
        methods::SESSION_ANSWER => {
            let p: crewd_proto::orchestra::SessionAnswerParams = parse_params(params)?;
            let r = h
                .answer(&p.session_id, &p.fingerprint, p.choice as usize)
                .await
                .map_err(rpc_err)?;
            to_value(crewd_proto::orchestra::SessionAnswerResult {
                cleared: r.cleared,
                decision: r.now.as_ref().map(decision_view),
            })
        }
        methods::DECISION_LIST => {
            parse_no_params(params)?;
            to_value(h.decisions().await.map_err(rpc_err)?)
        }

        methods::SESSION_START => {
            let p: SessionStartParams = parse_params(params)?;
            if p.codex_inputs.is_some() {
                return Err(RpcError::new(
                    RpcError::FORBIDDEN,
                    "native inputs require the original command.submit receipt",
                ));
            }
            let req = StartSession {
                crew_id: p.crew_id,
                captain: p.role == Role::Captain,
                executor: p.executor,
                brief: p.brief,
                title: p.title,
                codex_options: p.codex_options,
                model: p.model,
                effort: p.effort,
                isolation: p.isolation,
            };
            let row = h.start(req, Origin::Human).await.map_err(rpc_err)?;
            let s = read_started_session(state, row.id).await?;
            to_value(s)
        }
        methods::SESSION_STOP => {
            let p: SessionStopParams = parse_params(params)?;
            eprintln!("crewd: 客户端停止 session {}：{:?}", p.session_id, p.reason);
            h.stop(&p.session_id).await.map_err(rpc_err)?;
            to_value(Empty {})
        }
        methods::SESSION_RESUME => {
            let p: SessionIdParams = parse_params(params)?;
            h.resume(&p.session_id, Origin::Human)
                .await
                .map_err(rpc_err)?;
            to_value(Empty {})
        }
        methods::SESSION_LIVE => {
            let p: SessionIdParams = parse_params(params)?;
            let v = h.live(&p.session_id).await.map_err(rpc_err)?;
            to_value(live_to_proto(&v))
        }
        methods::CREW_SET_CAPTAIN => {
            let p: CrewSetCaptainParams = parse_params(params)?;
            h.set_captain(&p.crew_id, &p.session_id)
                .await
                .map_err(rpc_err)?;
            to_value(Empty {})
        }
        methods::WAKE_LIST => {
            let p: WakeListParams = parse_params_or_default(params)?;
            let l = h.wakes(p.session_id).await.map_err(rpc_err)?;
            to_value(wake_list_view(&l))
        }
        methods::LOOP_LIST => {
            parse_no_params(params)?;
            let mut v: Vec<LatchView> = h
                .latches()
                .await
                .map_err(rpc_err)?
                .iter()
                .map(latch_view)
                .collect();
            v.extend(
                h.causal_holds(None)
                    .await
                    .map_err(rpc_err)?
                    .into_iter()
                    .map(|hold| LatchView {
                        scope: LatchScopeView::Chain,
                        target_session_id: hold.target,
                        from: Some(hold.from),
                        chain_id: Some(hold.chain_id),
                        reason: hold.reason,
                        at_ms: hold.at_ms,
                    }),
            );
            to_value(v)
        }
        methods::LOOP_RECOVER => {
            let p: LoopRecoverParams = parse_params(params)?;
            if p.scope == LatchScopeView::Chain {
                let (Some(chain_id), Some(from)) = (p.chain_id, p.from) else {
                    return Err(rpc_err(ControlError::Invalid(Refusal::new(
                        Code::LoopChainNeedsIdentity,
                    ))));
                };
                if chain_id.trim().is_empty()
                    || from.trim().is_empty()
                    || p.target_session_id.trim().is_empty()
                {
                    return Err(rpc_err(ControlError::Invalid(Refusal::new(
                        Code::LoopChainNeedsIdentity,
                    ))));
                }
                let recovered = h
                    .recover_chain(
                        chain_id,
                        from,
                        p.target_session_id,
                        crewd_collab::causal::HumanRecoveryAction::new(),
                    )
                    .await
                    .map_err(rpc_err)?;
                return to_value(LoopRecovered { recovered });
            }
            if p.chain_id.is_some() {
                return Err(rpc_err(ControlError::Invalid(Refusal::new(
                    Code::LoopUnexpectedChainId,
                ))));
            }
            let (scope, id) = match (p.scope, p.from) {
                (LatchScopeView::Session, _) => (LatchScope::Session, p.target_session_id),
                (LatchScopeView::Edge, Some(from)) => {
                    (LatchScope::Edge, edge_id(&from, &p.target_session_id))
                }
                (LatchScopeView::Edge, None) => {
                    return Err(rpc_err(ControlError::Invalid(Refusal::new(
                        Code::LoopEdgeNeedsFrom,
                    ))));
                }
                (LatchScopeView::Chain, _) => unreachable!("chain scope handled above"),
            };
            let recovered = h.recover(scope, id).await.map_err(rpc_err)?;
            to_value(LoopRecovered { recovered })
        }
        methods::SETTINGS_GET => {
            parse_no_params(params)?;
            to_value(settings_view(&h.settings().await.map_err(rpc_err)?))
        }
        methods::SETTINGS_SET => {
            let p: SettingsPatch = parse_params(params)?;
            let u = settings_update(p).map_err(rpc_err)?;
            to_value(settings_view(&h.set_settings(u).await.map_err(rpc_err)?))
        }
        methods::HIBERNATE => {
            let p: HibernateParams = parse_params_or_default(params)?;
            let r = h
                .hibernate(p.deadline_secs.map(|s| Duration::from_secs(s.into())))
                .await
                .map_err(rpc_err)?;
            to_value(hibernate_view(r))
        }
        methods::MESSAGE_RECEIPTS => {
            let p: MessageReceiptsParams = parse_params(params)?;
            let v: Vec<Receipt> = h
                .receipts(&p.message_id)
                .await
                .map_err(rpc_err)?
                .iter()
                .map(receipt_view)
                .collect();
            to_value(v)
        }
        other => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            format!("未知方法: {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn s4_rpc_started_session_real_sql_history() {
        let (st, port, dir) = crate::orchestra::db::fixture::seeded().await;
        let db = st.db.clone();
        let health = db.health();
        let exec = StandIn::default();
        let (h, actor) = spawn(
            Arc::new(port),
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(T0)),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        // Genuine successful RPC/real DbPort/StandIn start before touching schema.
        let value = rpc(&h, &st, methods::SESSION_START, json!({
            "crewId":"c-1", "role":"worker", "executor":"claude", "brief":"synthetic S4 postread",
            "title":"S4 postread", "isolation":false
        })).await.unwrap();
        let started: crewd_proto::Session = serde_json::from_value(value).unwrap();
        let launched = exec.lock().launches.iter().any(|l| l.session == started.id);
        let abort = actor.abort_handle();
        let stopped = h.shutdown(Duration::from_secs(3)).await;
        if !stopped {
            abort.abort();
        }
        let joined_actor = tokio::time::timeout(Duration::from_secs(3), actor.finished()).await;
        drop(h);
        // Extracted owning postread after actor close; not a same-start-call fault E2E.
        db.call(|s| {
            s.conn()
                .execute_batch("ALTER TABLE sessions RENAME TO s4_rpc_held_sessions")
        })
        .await
        .unwrap()
        .unwrap();
        let id = started.id.clone();
        let cause = db
            .call(move |s| crewd_collab::sessions::get(s.conn(), &id))
            .await
            .unwrap();
        let before = health.snapshot();
        let original = read_started_session(&st, started.id.clone()).await;
        let observed = health.snapshot();
        db.call(|s| {
            s.conn()
                .execute_batch("ALTER TABLE s4_rpc_held_sessions RENAME TO sessions")
        })
        .await
        .unwrap()
        .unwrap();
        let recovered = read_started_session(&st, started.id.clone()).await;
        let later = health.snapshot();
        drop(st);
        let joined_db = tokio::task::spawn_blocking(move || db.shutdown()).await;
        drop(dir);
        assert!(
            stopped,
            "aborted actor is fallback cleanup, not normal lifecycle proof"
        );
        assert!(
            matches!(joined_actor, Ok(Ok(()))),
            "owned actor join: {joined_actor:?}"
        );
        assert!(joined_db.is_ok(), "owned DB cleanup before history oracle");
        assert!(
            launched,
            "actual stand-in launch of successfully returned session"
        );
        assert_eq!(recovered.unwrap().id, started.id);
        assert!(matches!(cause, Err(crewd_collab::CollabError::Store(
            crewd_store::StoreError::Sql(rusqlite::Error::SqliteFailure(ref e, _)))) if e.extended_code == 1));
        let error = original.unwrap_err();
        assert_eq!(error.code, RpcError::INTERNAL);
        assert!(!error.message.contains("s4_rpc_held_sessions"));
        assert!(before.is_none());
        assert_eq!(observed, later);
        // First intended missing-observation oracle. Later stage/kind are NOTREACHED on RED.
        assert_eq!(observed.as_ref().map(|f| f.sequence), Some(1));
        let fault = observed.unwrap();
        assert_eq!(fault.stage, crewd_store::db_health::DbFaultStage::Job);
        assert_eq!(fault.kind, crewd_store::db_health::DbFaultKind::Sql);
    }

    #[tokio::test]
    async fn s4_rpc_started_session_business_refusal_keeps_no_history() {
        let (st, port, dir) = crate::orchestra::db::fixture::seeded().await;
        let db = st.db.clone();
        let health = db.health();
        let exec = StandIn::default();
        let (h, actor) = spawn(
            Arc::new(port),
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(T0)),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        // Genuine successful RPC/real DbPort/StandIn start before touching schema.
        let value = rpc(&h, &st, methods::SESSION_START, json!({
            "crewId":"c-1", "role":"worker", "executor":"claude", "brief":"synthetic S4 postread",
            "title":"S4 postread", "isolation":false
        })).await.unwrap();
        let started: crewd_proto::Session = serde_json::from_value(value).unwrap();
        let launched = exec.lock().launches.iter().any(|l| l.session == started.id);
        let abort = actor.abort_handle();
        let stopped = h.shutdown(Duration::from_secs(3)).await;
        if !stopped {
            abort.abort();
        }
        let joined_actor = tokio::time::timeout(Duration::from_secs(3), actor.finished()).await;
        drop(h);
        // Extracted owning postread after actor close; not a same-start-call fault E2E.
        let cause = db
            .call(|s| crewd_collab::sessions::get(s.conn(), "s4-rpc-no-such-session"))
            .await
            .unwrap();
        let before = health.snapshot();
        let original = read_started_session(&st, "s4-rpc-no-such-session".into()).await;
        let observed = health.snapshot();

        let recovered = read_started_session(&st, started.id.clone()).await;
        let later = health.snapshot();
        drop(st);
        let joined_db = tokio::task::spawn_blocking(move || db.shutdown()).await;
        drop(dir);
        assert!(
            stopped,
            "aborted actor is fallback cleanup, not normal lifecycle proof"
        );
        assert!(
            matches!(joined_actor, Ok(Ok(()))),
            "owned actor join: {joined_actor:?}"
        );
        assert!(joined_db.is_ok(), "owned DB cleanup before history oracle");
        assert!(
            launched,
            "actual stand-in launch of successfully returned session"
        );
        assert_eq!(recovered.unwrap().id, started.id);
        assert!(
            matches!(cause, Err(crewd_collab::CollabError::NotFound(ref r))
            if r.code == Code::SessionNotFound)
        );
        let error = original.unwrap_err();
        assert_eq!(error.code, RpcError::NOT_FOUND);
        assert_eq!(
            error.data.map(|d| d.refusal).as_deref(),
            Some("session.not_found")
        );
        assert!(before.is_none());
        assert!(observed.is_none());
        assert!(later.is_none());
    }
    use super::*;
    use crate::orchestra::core::{Cmd, Origin, StartSession};
    use crate::orchestra::testkit::Rig;
    use crewd_collab::refusal::{Code, Refusal};
    use crewd_store::StorageKind;

    /// 每个变体 → (错误号, 拒绝码)；文字缺省是中文、`data.refusal` 是码（T10b 补充 §2.4）。
    #[test]
    fn every_control_error_maps_to_its_code_and_refusal() {
        let s = || "s-1".to_string();
        let refused = |kind| ControlError::Refused {
            kind,
            refusal: Refusal::new(Code::SessionTitleTooLong)
                .arg("max", "18")
                .arg("actual", "30"),
        };
        let table = [
            (ControlError::NotFound(s()), -32004, "session.not_found"),
            (
                ControlError::AlreadyRunning(s()),
                -32009,
                "session.already_running",
            ),
            (ControlError::NotRunning(s()), -32009, "session.not_running"),
            (ControlError::AtCap { max: 3 }, -32009, "session.at_cap"),
            (
                ControlError::Unsupported("codex".into()),
                -32009,
                "session.executor_unavailable",
            ),
            (ControlError::NoWorkdir(s()), -32009, "session.no_workdir"),
            (
                ControlError::Worktree(crate::orchestra::worktree::WorktreeError::new(
                    crate::orchestra::worktree::WorktreeReason::AddFailed,
                    "/synthetic",
                    "synthetic stderr",
                )),
                -32009,
                "session.worktree.add_failed",
            ),
            (
                ControlError::CaptainTaken("c-1".into()),
                -32009,
                "session.captain_exists",
            ),
            (
                ControlError::Invalid(Refusal::new(Code::OrchestraNudgeEmpty)),
                -32602,
                "orchestra.nudge_empty",
            ),
            (
                ControlError::ShuttingDown,
                -32009,
                "orchestra.shutting_down",
            ),
            (
                ControlError::CrewRemoved("c-1".into()),
                -32009,
                "crew.removed",
            ),
            (
                ControlError::CrewNoWorkdir("c-1".into()),
                -32009,
                "crew.no_workdir",
            ),
            (
                refused(RefusedKind::NotFound),
                -32004,
                "session.title_too_long",
            ),
            (
                refused(RefusedKind::Invalid),
                -32602,
                "session.title_too_long",
            ),
            (
                refused(RefusedKind::Forbidden),
                -32003,
                "session.title_too_long",
            ),
            (
                refused(RefusedKind::Conflict),
                -32009,
                "session.title_too_long",
            ),
            (
                ControlError::NoDecision("s-1".into()),
                -32009,
                "decision.none",
            ),
            (
                ControlError::DecisionChanged {
                    session: "s-1".into(),
                    now: None,
                },
                -32009,
                "decision.changed",
            ),
            (
                ControlError::NeedsTerminal("s-1".into()),
                -32009,
                "decision.needs_terminal",
            ),
            (
                ControlError::NothingToInterrupt("s-1".into()),
                -32009,
                "session.nothing_to_interrupt",
            ),
            (
                ControlError::InputDraft {
                    session: "s-1".into(),
                    text: Some("draft".into()),
                },
                -32009,
                "session.input_draft",
            ),
            (
                ControlError::InputDraft {
                    session: "s-1".into(),
                    text: None,
                },
                -32009,
                "session.input_draft_unknown",
            ),
        ];
        for (e, code, refusal) in table {
            let r = rpc_err(e.clone());
            assert_eq!(r.code, code, "{e:?}");
            assert_eq!(
                r.data.as_ref().map(|d| d.refusal.as_str()),
                Some(refusal),
                "{e:?}"
            );
            assert_eq!(r.message, e.to_string(), "message = 中文缺省文字");
            assert!(!r.message.contains('{'), "占位符都填了：{}", r.message);
        }
    }

    /// C14 / S26：内部错误 = -32603 带编号、原文不上线；存储故障 = -32010 只说类别。两者都不带拒绝码。
    #[test]
    fn internal_and_storage_errors_keep_their_detail_off_the_wire() {
        let r = rpc_err(ControlError::Internal("合成 secret 原文".into()));
        assert_eq!(r.code, RpcError::INTERNAL);
        assert!(
            !r.message.contains("secret") && r.message.contains('#'),
            "{}",
            r.message
        );
        assert!(r.data.is_none());
        let r = rpc_err(ControlError::Storage {
            kind: StorageKind::DiskFull,
            detail: "/secret/crewd.db".into(),
        });
        assert_eq!(r.code, RpcError::STORAGE);
        assert!(
            r.message.contains(StorageKind::DiskFull.describe()),
            "{}",
            r.message
        );
        assert!(!r.message.contains("secret"), "{}", r.message);
        assert!(r.data.is_none());
    }

    /// S26：Port 读出的存储故障（MemPort 注入一个 `StoreError::Storage`）经 `from_port` 成为 `Storage`，RPC 上 -32010。
    #[tokio::test]
    async fn a_storage_failure_from_the_port_reaches_the_client_as_storage() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.port.lock().storage_error = Some(StorageKind::ReadOnly);
        let e = rig
            .cmd(|reply| Cmd::Stop {
                session: "cap".into(),
                reply,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(
                e,
                ControlError::Storage {
                    kind: StorageKind::ReadOnly,
                    ..
                }
            ),
            "{e:?}"
        );
        assert_eq!(rpc_err(e).code, RpcError::STORAGE);
    }

    /// T10a 遗留 2：写 session 行时 collab 的规则拒绝（标题太长）原样带码到客户端，不再是一个内部错误。
    #[tokio::test]
    async fn a_refused_session_row_keeps_its_collab_code() {
        use crate::orchestra::clock::ManualClock;
        use crate::orchestra::db::fixture::seeded;
        use crate::orchestra::exec::Executors;
        use crate::orchestra::hooks::Hooks;
        use crate::orchestra::testkit::StandIn;
        use crate::orchestra::{OrchestraConfig, spawn};
        use std::sync::Arc;
        let (_state, port, _d) = seeded().await;
        let stand_in = StandIn::default();
        let (h, _actor) = spawn(
            Arc::new(port),
            Executors::new().with("claude", Arc::new(stand_in.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(crewd_wake::Ms(1_000_000_000))),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        let req = StartSession {
            codex_options: None,

            crew_id: "c-1".into(),
            captain: false,
            executor: "claude".into(),
            brief: "合成任务".into(),
            title: Some("合成".repeat(15)),
            model: None,
            effort: None,
            isolation: false,
        };
        let e = h.start(req, Origin::Human).await.unwrap_err();
        assert!(
            matches!(
                &e,
                ControlError::Refused {
                    kind: RefusedKind::Invalid,
                    refusal,
                } if refusal.code == Code::SessionTitleTooLong
            ),
            "{e:?}"
        );
        let r = rpc_err(e);
        assert_eq!(r.code, RpcError::INVALID_PARAMS);
        assert_eq!(r.data.unwrap().refusal, "session.title_too_long");
        assert!(stand_in.lock().launches.is_empty(), "什么都没起");
    }

    /// 编排自己拒的（同一个 `ControlError` 经 anyhow 回来）原样带出，不变成内部错误。
    #[test]
    fn from_port_keeps_an_orchestra_refusal_and_wraps_the_rest() {
        let e = ControlError::from_port(anyhow::Error::new(ControlError::AlreadyRunning(
            "s-1".into(),
        )));
        assert_eq!(e, ControlError::AlreadyRunning("s-1".into()));
        let e = ControlError::from_port(anyhow::anyhow!("合成：别的错"));
        assert!(matches!(e, ControlError::Internal(_)), "{e:?}");
        let e = ControlError::from_port(
            anyhow::Error::new(crewd_collab::CollabError::Conflict(
                Refusal::new(Code::WakeBadTransition)
                    .arg("wake", "1")
                    .arg("from", "dropped")
                    .arg("to", "inflight"),
            ))
            .context("合成：外面再包一层"),
        );
        assert!(
            matches!(
                &e,
                ControlError::Refused {
                    kind: RefusedKind::Conflict,
                    ..
                }
            ),
            "沿错误链找到 collab 的拒绝：{e:?}"
        );
    }

    /// DbPort 的存储故障是包在 `CollabError::Store` 里上来的（collab 的读写函数都回 `CollabError`）：照样归成 `Storage`。
    #[test]
    fn a_storage_failure_inside_a_collab_error_is_still_storage() {
        let inner = crate::orchestra::testkit::storage_failure(StorageKind::Corrupt)
            .downcast::<crewd_store::StoreError>()
            .unwrap();
        let e =
            ControlError::from_port(anyhow::Error::new(crewd_collab::CollabError::Store(inner)));
        assert!(
            matches!(
                e,
                ControlError::Storage {
                    kind: StorageKind::Corrupt,
                    ..
                }
            ),
            "{e:?}"
        );
    }

    use crate::orchestra::clock::ManualClock;
    use crate::orchestra::exec::Executors;
    use crate::orchestra::hooks::Hooks;
    use crate::orchestra::port::Port;
    use crate::orchestra::testkit::{MemPort, StandIn, T0};
    use crate::orchestra::{Actor, OrchestraConfig, spawn};
    use crewd_proto::{Request, methods as all_methods};
    use serde_json::json;
    use std::sync::Arc;

    fn state() -> AppState {
        AppState::new(
            crewd_store::Db::spawn(crewd_store::Store::open_in_memory().unwrap()).unwrap(),
        )
    }

    async fn handle(port: Arc<MemPort>) -> (Handle, Actor, StandIn) {
        port.lock().now = T0;
        let exec = StandIn::default();
        let (h, a) = spawn(
            port,
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(T0)),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        (h, a, exec)
    }

    async fn rpc(h: &Handle, st: &AppState, m: &str, p: Value) -> Result<Value, RpcError> {
        assert!(handles(m), "{m}");
        call(h, st, m, p).await
    }

    #[tokio::test]
    async fn t10c_decision_list_inspect_cas_and_terminal_answer_end_to_end() {
        use crewd_exec::session::{HeldDraft, SessionEvent};
        use crewd_term::{DecisionEvent, DecisionKind, PendingDecision};
        let port = Arc::new(MemPort::new());
        let (h, _a, x) = handle(port.clone()).await;
        let st = state();
        let row = port
            .insert_session(super::super::port::NewSession {
                codex_options: None,

                crew_id: "c1".into(),
                captain: false,
                executor: "claude".into(),
                brief: "synthetic".into(),
                title: Some("Worker".into()),
                model: None,
                effort: None,
                worktree: None,
            })
            .await
            .unwrap();
        h.resume(&row.id, Origin::Human).await.unwrap();
        let d = PendingDecision {
            kind: DecisionKind::Terminal,
            prompt: "Trust?".into(),
            options: vec!["Yes".into(), "No".into()],
            numbered: true,
            selected: Some(1),
        };
        x.emit(
            &row.id,
            SessionEvent::Decision(DecisionEvent::Appeared(d.clone())),
        );
        let mut list = Value::Null;
        for _ in 0..200 {
            list = rpc(&h, &st, methods::DECISION_LIST, Value::Null)
                .await
                .unwrap();
            if list.as_array().is_some_and(|a| a.len() == 1) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(list[0]["sessionId"], row.id);
        assert_eq!(list[0]["crewId"], "c1");
        assert_eq!(list[0]["title"], "Worker");
        assert_eq!(list[0]["decision"]["fingerprint"], d.fingerprint());
        assert_eq!(list[0]["decision"]["selected"], 1);
        assert_eq!(list[0]["sinceMs"], T0.0);
        let error = rpc(
            &h,
            &st,
            methods::SESSION_ANSWER,
            json!({"sessionId":row.id,"fingerprint":"stale","choice":0}),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, -32009);
        let data = error.data.unwrap();
        assert_eq!(data.refusal, "decision.changed");
        assert_eq!(*data.decision.unwrap(), decision_view(&d));
        assert!(x.lock().answers.is_empty(), "stale choice wrote nothing");
        for lines in [None, Some(999)] {
            let mut params = json!({"sessionId":row.id});
            if let Some(n) = lines {
                params["maxLines"] = json!(n);
            }
            let inspect = rpc(&h, &st, methods::SESSION_INSPECT, params)
                .await
                .unwrap();
            assert_eq!(inspect["screen"], "合成屏幕");
            assert_eq!(
                inspect["live"]["pendingDecision"]["fingerprint"],
                d.fingerprint()
            );
        }
        assert_eq!(
            x.lock().screens,
            [(row.id.clone(), 60), (row.id.clone(), 400)]
        );
        x.emit(&row.id, SessionEvent::InputDraft(Some(HeldDraft::Unknown)));
        for _ in 0..200 {
            if h.live(&row.id).await.unwrap().input_draft.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
        let inspected = rpc(
            &h,
            &st,
            methods::SESSION_INSPECT,
            json!({"sessionId":row.id}),
        )
        .await
        .unwrap();
        assert_eq!(inspected["live"]["inputDraftUnknown"], true);
        assert!(inspected["live"].get("inputDraft").is_none());
        x.lock().box_script.insert(row.id.clone(), [None].into());
        let result = rpc(
            &h,
            &st,
            methods::SESSION_ANSWER,
            json!({"sessionId":row.id,"fingerprint":d.fingerprint(),"choice":0}),
        )
        .await
        .unwrap();
        assert_eq!(result, json!({"cleared":true}));
        assert_eq!(
            x.lock().raws[&row.id],
            [b"1".to_vec()],
            "RPC zero maps to terminal digit one"
        );
        for _ in 0..200 {
            list = rpc(&h, &st, methods::DECISION_LIST, Value::Null)
                .await
                .unwrap();
            if list == json!([]) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(list, json!([]));
        assert!(
            rpc(&h, &st, methods::DECISION_LIST, json!({"extra":1}))
                .await
                .is_err()
        );
        assert!(
            rpc(
                &h,
                &st,
                methods::SESSION_INSPECT,
                json!({"sessionId":row.id,"extra":1})
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn t10c_runner_request_is_listed_and_structurally_answered_without_keys() {
        use crewd_exec::session::SessionEvent;
        use crewd_term::{DecisionEvent, DecisionKind, PendingDecision};
        let port = Arc::new(MemPort::new());
        let (h, _a, x) = handle(port.clone()).await;
        let st = state();
        let row = port
            .insert_session(super::super::port::NewSession {
                codex_options: None,

                crew_id: "c1".into(),
                captain: false,
                executor: "claude".into(),
                brief: "synthetic".into(),
                title: Some("Runner".into()),
                model: None,
                effort: None,
                worktree: None,
            })
            .await
            .unwrap();
        h.resume(&row.id, Origin::Human).await.unwrap();
        let d = PendingDecision {
            kind: DecisionKind::RunnerRequest {
                request_id: "request-1".into(),
            },
            prompt: "item/commandExecution/requestApproval".into(),
            options: vec!["accept".into(), "decline".into()],
            numbered: false,
            selected: None,
        };
        let d2 = PendingDecision {
            kind: DecisionKind::RunnerRequest {
                request_id: "request-2".into(),
            },
            ..d.clone()
        };
        assert_ne!(d.fingerprint(), d2.fingerprint());
        x.emit(
            &row.id,
            SessionEvent::Decision(DecisionEvent::Appeared(d.clone())),
        );
        let mut list = Value::Null;
        for _ in 0..200 {
            list = rpc(&h, &st, methods::DECISION_LIST, Value::Null)
                .await
                .unwrap();
            if list.as_array().is_some_and(|a| a.len() == 1) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(list[0]["decision"]["kind"], "runner_request");
        assert!(list[0]["decision"].get("selected").is_none());
        let invalid = rpc(
            &h,
            &st,
            methods::SESSION_ANSWER,
            json!({"sessionId":row.id,"fingerprint":d.fingerprint(),"choice":2}),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid.data.unwrap().refusal, "decision.out_of_range");
        let result = rpc(
            &h,
            &st,
            methods::SESSION_ANSWER,
            json!({"sessionId":row.id,"fingerprint":d.fingerprint(),"choice":1}),
        )
        .await
        .unwrap();
        assert_eq!(result, json!({"cleared":true}));
        assert_eq!(x.lock().choices[&row.id], [1]);
        assert!(
            x.lock().raws.is_empty() && x.lock().answers.is_empty(),
            "not simulated keys"
        );
    }

    #[test]
    fn handles_exactly_the_m3c_methods() {
        for m in methods::ALL {
            assert!(handles(m), "{m}");
            assert!(all_methods::ALL.contains(m), "{m} 在客户端方法表里");
        }
        for m in [
            all_methods::HELLO,
            all_methods::MESSAGE_POST,
            "orchestra.settings.get",
            "admission.recover",
        ] {
            assert!(!handles(m), "{m}");
        }
    }

    /// C1 / T10b 补充 §2.7：库里的 `serve()` 不起编排——M3c 的方法回 -32009 `orchestra.not_running`，不是 -32601；
    /// 别的未知方法照旧 -32601。
    #[tokio::test]
    async fn without_an_orchestra_m3c_methods_say_it_is_not_running() {
        let st = state();
        for (i, m) in methods::ALL.iter().enumerate() {
            let r = crate::dispatch::handle(
                &st,
                Request {
                    id: i as u64 + 1,
                    method: m.to_string(),
                    params: Value::Null,
                },
            )
            .await;
            let e = r.into_result().unwrap_err();
            assert_eq!(e.code, RpcError::CONFLICT, "{m}");
            assert_eq!(
                e.data.map(|d| d.refusal).as_deref(),
                Some("orchestra.not_running"),
                "{m}"
            );
        }
        let e = crate::dispatch::handle(
            &st,
            Request {
                id: 99,
                method: "admission.recover".into(),
                params: Value::Null,
            },
        )
        .await
        .into_result()
        .unwrap_err();
        assert_eq!(e.code, RpcError::METHOD_NOT_FOUND, "改了名的旧方法没有别名");
    }

    #[tokio::test]
    async fn params_are_strict_and_errors_carry_codes() {
        let (h, _a, _x) = handle(Arc::new(MemPort::new())).await;
        let st = state();
        let e = rpc(
            &h,
            &st,
            methods::SESSION_STOP,
            json!({"sessionId":"s","reason":"r","x":1}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, RpcError::INVALID_PARAMS);
        let e = rpc(
            &h,
            &st,
            methods::LOOP_RECOVER,
            json!({"scope":"edge","targetSessionId":"w1"}),
        )
        .await
        .unwrap_err();
        assert_eq!(
            (e.code, e.data.map(|d| d.refusal)),
            (
                RpcError::INVALID_PARAMS,
                Some("loop.edge_needs_from".into())
            )
        );
        let e = rpc(&h, &st, methods::SESSION_LIVE, json!({"sessionId":"nope"}))
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, e.data.map(|d| d.refusal)),
            (RpcError::NOT_FOUND, Some("session.not_found".into()))
        );
        for m in [methods::SETTINGS_GET, methods::LOOP_LIST] {
            assert!(rpc(&h, &st, m, json!({"x":1})).await.is_err(), "{m}");
            assert!(rpc(&h, &st, m, Value::Null).await.is_ok(), "{m}");
        }
        // 解一条本来没有的熔断：不是错，回 false。
        let v = rpc(
            &h,
            &st,
            methods::LOOP_RECOVER,
            json!({"scope":"session","targetSessionId":"w1"}),
        )
        .await
        .unwrap();
        assert_eq!(v, json!({"recovered": false}));
        assert_eq!(
            rpc(&h, &st, methods::LOOP_LIST, Value::Null).await.unwrap(),
            json!([])
        );
    }

    #[tokio::test]
    async fn s42_human_recovery_rpc_rejects_incomplete_chain_identity_and_preserves_legacy() {
        let (h, actor, _x) = handle(Arc::new(MemPort::new())).await;
        let st = state();
        let mut results = Vec::new();
        for params in [
            json!({"scope":"chain","targetSessionId":"w1","from":"cap"}),
            json!({"scope":"chain","targetSessionId":"w1","chainId":"c1:opaque"}),
            json!({"scope":"chain","targetSessionId":"w1","from":"cap","chainId":" "}),
            json!({"scope":"chain","targetSessionId":"w1","from":" ","chainId":"c1:opaque"}),
            json!({"scope":"chain","targetSessionId":" ","from":"cap","chainId":"c1:opaque"}),
        ] {
            results.push((
                rpc(&h, &st, methods::LOOP_RECOVER, params).await,
                "loop.chain_needs_identity",
            ));
        }
        for scope in ["edge", "session"] {
            results.push((rpc(&h, &st, methods::LOOP_RECOVER,
                json!({"scope":scope,"targetSessionId":"w1","from":"cap","chainId":"c1:opaque"})).await,
                "loop.unexpected_chain_id"));
        }
        let legacy = rpc(
            &h,
            &st,
            methods::LOOP_RECOVER,
            json!({"scope":"session","targetSessionId":"w1","from":"ignored"}),
        )
        .await;
        // This checks real human RPC validation, not successful durable chain recovery.
        let stopped = h.shutdown(Duration::from_secs(5)).await;
        let joined = tokio::time::timeout(Duration::from_secs(5), actor.finished()).await;
        let db = st.db.clone();
        tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .unwrap();
        assert!(stopped, "owned empty actor must shut down normally");
        assert!(matches!(joined, Ok(Ok(()))), "actor cleanup: {joined:?}");
        for (result, refusal) in results {
            let err = result.unwrap_err();
            assert_eq!(err.code, RpcError::INVALID_PARAMS);
            assert_eq!(err.data.map(|d| d.refusal).as_deref(), Some(refusal));
        }
        assert_eq!(legacy.unwrap(), json!({"recovered":false}));
    }

    /// S49：`settings.set` 键不在 = 不改、`null` = 清掉、数 = 设成它；坏值带码、什么都不改。
    #[tokio::test]
    async fn settings_round_trip_with_absent_null_and_values() {
        let port = Arc::new(MemPort::new());
        let (h, _a, _x) = handle(port.clone()).await;
        let st = state();
        let v = rpc(&h, &st, methods::SETTINGS_GET, Value::Null)
            .await
            .unwrap();
        assert_eq!(v, json!({"locale": "zh"}));
        let v = rpc(
            &h,
            &st,
            methods::SETTINGS_SET,
            json!({"maxLiveSessions": 4, "locale": "en", "hibernateDeadlineSecs": 0}),
        )
        .await
        .unwrap();
        assert_eq!(
            v,
            json!({"locale": "en", "maxLiveSessions": 4, "hibernateDeadlineSecs": 0})
        );
        let v = rpc(
            &h,
            &st,
            methods::SETTINGS_SET,
            json!({"maxLiveSessions": null}),
        )
        .await
        .unwrap();
        assert_eq!(
            v,
            json!({"locale": "en", "hibernateDeadlineSecs": 0}),
            "只清了上限"
        );
        for (bad, code) in [
            (
                json!({"locale": "fr", "maxLiveSessions": 2}),
                "settings.bad_locale",
            ),
            (
                json!({"maxLiveSessions": 0, "locale": "zh"}),
                "settings.max_live_min",
            ),
            (
                json!({"locale": null, "maxLiveSessions": 2}),
                "settings.bad_locale",
            ),
        ] {
            let e = rpc(&h, &st, methods::SETTINGS_SET, bad.clone())
                .await
                .unwrap_err();
            assert_eq!(
                (e.code, e.data.map(|d| d.refusal)),
                (RpcError::INVALID_PARAMS, Some(code.into())),
                "{bad}"
            );
        }
        let v = rpc(&h, &st, methods::SETTINGS_GET, Value::Null)
            .await
            .unwrap();
        assert_eq!(
            v,
            json!({"locale": "en", "hibernateDeadlineSecs": 0}),
            "坏值什么都没改"
        );
        assert_eq!(port.lock().max_live, None);
    }

    /// `wake.list` 的形状：行 + 原因（码、时刻）+ 目标近况；`message.receipts` 同一条消息。
    #[tokio::test]
    async fn wake_list_and_receipts_have_the_contract_shape() {
        use crewd_wake::{Ms, NewWake, WakeKind, source};
        let port = Arc::new(MemPort::new());
        {
            let mut st = port.lock();
            st.sessions.insert(
                "w1".into(),
                crate::orchestra::port::SessionRow {
                    id: "w1".into(),
                    crew_id: "c1".into(),
                    captain: false,
                    executor: "claude".into(),
                    display_name: Some("w1".into()),
                    agent_session_id: None,
                    workdir: Some("/nonexistent/wd".into()),
                    model: None,
                    effort: None,
                    brief: None,
                    desired: crate::orchestra::port::Desired::Stopped,
                    was_working: false,
                    last_active_at: None,
                    resume_failed_at: None,
                    awaiting_question: None,
                    last_progress_at: None,
                    crew_removed: false,
                },
            );
        }
        let (h, _a, _x) = handle(port.clone()).await;
        let later = Ms(T0.0 + 3_600_000);
        crate::orchestra::port::Port::enqueue(
            &*port,
            NewWake {
                session: "w1".into(),
                kind: WakeKind::Timer,
                source: source::timer("合成"),
                priority: Priority::System,
                not_before: later,
                note: Some("合成提醒".into()),
                origin: "w1".into(),
            },
        )
        .await
        .unwrap();
        let st = state();
        let v = rpc(&h, &st, methods::WAKE_LIST, json!({"sessionId": "w1"}))
            .await
            .unwrap();
        let r: WakeListResult = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(r.wakes.len(), 1, "{v}");
        let w = &r.wakes[0];
        assert_eq!(
            (w.kind, w.state),
            (WakeKindView::Timer, WakeStateView::Pending)
        );
        assert_eq!(
            w.blocker,
            Some(WakeBlocker {
                causal: None,
                code: BlockerCodeView::NotDue,
                until_ms: Some(later.0),
                detail: None
            })
        );
        assert_eq!(w.message_id, None);
        assert_eq!(
            r.targets,
            [TargetView {
                causal_diagnostics: vec![],
                session_id: "w1".into(),
                running: false,
                inflight_wake_ids: vec![],
                delivery_id: None,
                writes_queued: 0,
                last_outcome: None,
            }]
        );
        assert_eq!(v["wakes"][0]["blocker"]["code"], "not_due");
        let all = rpc(&h, &st, methods::WAKE_LIST, Value::Null).await.unwrap();
        assert_eq!(all["wakes"].as_array().unwrap().len(), 1);
        let v = rpc(
            &h,
            &st,
            methods::MESSAGE_RECEIPTS,
            json!({"messageId": "nope"}),
        )
        .await
        .unwrap();
        assert_eq!(v, json!([]));
    }

    /// crewd-wake 的原因码与线上的词逐字相同（同一张表，两边不漂）。
    #[test]
    fn blocker_codes_match_the_wire_words() {
        use crewd_wake::{LatchScope, Ms};
        for b in [
            Blocker::ShuttingDown,
            Blocker::CrewRemoved,
            Blocker::Starting,
            Blocker::NotDue { until: Ms(1) },
            Blocker::Held {
                scope: LatchScope::Edge,
                id: "a→b".into(),
            },
            Blocker::BehindHigherPriority,
            Blocker::Inflight,
            Blocker::Working,
            Blocker::Decision,
            Blocker::InputDraft,
            Blocker::Stopping,
            Blocker::Quota,
            Blocker::AuthRequired,
            Blocker::LaunchFailed,
            Blocker::AwaitingRestartAnswer,
            Blocker::QuotaNotReset,
            Blocker::AtCap { max: 2 },
            Blocker::Stagger { until: Ms(2) },
        ] {
            let v = blocker_view(&b);
            assert_eq!(
                serde_json::to_value(v.code).unwrap(),
                json!(b.code()),
                "{b:?}"
            );
            assert_ne!(v.code, BlockerCodeView::Unknown);
        }
        let held = blocker_view(&Blocker::Held {
            scope: LatchScope::Edge,
            id: "a→b".into(),
        });
        assert_eq!(held.detail.as_deref(), Some("edge:a→b"));
        assert_eq!(
            blocker_view(&Blocker::AtCap { max: 2 }).detail.as_deref(),
            Some("2")
        );
    }

    #[test]
    fn latch_views_split_an_edge_into_its_source_and_target() {
        use crewd_wake::{LatchScope, Ms};
        let edge = Latch {
            scope: LatchScope::Edge,
            id: edge_id("cap", "w1"),
            reason: "no-progress".into(),
            at: Ms(5),
        };
        let v = latch_view(&edge);
        assert_eq!(
            (
                v.scope,
                v.target_session_id.as_str(),
                v.from.as_deref(),
                v.at_ms
            ),
            (LatchScopeView::Edge, "w1", Some("cap"), 5)
        );
        let session = Latch {
            scope: LatchScope::Session,
            id: "w1".into(),
            reason: "no-progress".into(),
            at: Ms(6),
        };
        let v = latch_view(&session);
        assert_eq!((v.target_session_id.as_str(), v.from), ("w1", None));
    }

    /// U6：`crewd.hibernate` 经 RPC 回执、`exit_requested` 变 true（之后 serve 退出，T11）。
    #[tokio::test]
    async fn hibernate_reports_through_rpc_and_asks_serve_to_exit() {
        let (h, actor, _x) = handle(Arc::new(MemPort::new())).await;
        let st = state();
        let exit = h.exit_requested();
        let v = rpc(&h, &st, methods::HIBERNATE, Value::Null).await.unwrap();
        assert_eq!(
            v,
            json!({"clean": [], "interrupted": [], "failed": [], "completed": true})
        );
        assert!(*exit.borrow());
        assert!(
            actor.finished().await.is_ok(),
            "没在跑的：休眠完 actor 收尾"
        );
        // 停机进行中再休眠 = `ShuttingDown`（Core 的测试 `hibernate_during_shutdown_is_refused…` 钉住）→ 线上 -32009 + 码。
        let e = rpc_err(ControlError::ShuttingDown);
        assert_eq!(
            (e.code, e.data.map(|d| d.refusal)),
            (RpcError::CONFLICT, Some("orchestra.shutting_down".into()))
        );
    }

    async fn hello(st: &AppState, params: Value) -> Result<Value, RpcError> {
        crate::dispatch::handle(
            st,
            Request {
                id: 1,
                method: all_methods::HELLO.into(),
                params,
            },
        )
        .await
        .into_result()
    }

    /// U1：hello 回 API 版本、最老还认的版本、构建身份；能力只在编排在跑时有（库里的 `serve()` 没有）。
    #[tokio::test]
    async fn hello_reports_versions_build_and_capabilities() {
        // Capability membership is a set; serialization order is not an API contract.
        let sorted = |v: &Value| {
            let mut values: Vec<_> = v
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            values.sort();
            values
        };
        let st = state();
        let v = hello(&st, Value::Null).await.unwrap();
        assert_eq!(v["apiVersion"], 3);
        assert_eq!(v["minApiVersion"], crewd_proto::MIN_API_VERSION);
        assert_eq!(
            sorted(&v["capabilities"]),
            sorted(&json!([
                "work.reassignLost",
                "documents",
                "viewFolders",
                "channelWork",
                "hostDirectoryBrowse",
                "hostQuota"
            ])),
            "持久工作能力不要求编排"
        );
        let build = &v["build"];
        assert!(
            build["gitSha"].as_str().is_some_and(|s| !s.is_empty()),
            "{v}"
        );
        assert!(build["exe"].as_str().is_some_and(|s| !s.is_empty()), "{v}");
        assert_eq!(build["exeStillOnDisk"], true);
        assert!(build["dirty"].is_boolean());
        let (h, _a, _x) = handle(Arc::new(MemPort::new())).await;
        let st = state();
        assert!(st.orchestra.set(h).is_ok());
        let v = hello(&st, json!({})).await.unwrap();
        assert_eq!(
            sorted(&v["capabilities"]),
            sorted(&json!([
                "orchestra",
                "session.answer",
                "session.respondRunnerRequest",
                "session.codex",
                "crewd.hibernate",
                "wake.explain",
                "message.receipts",
                "settings",
                "work.reassignLost",
                "documents",
                "viewFolders",
                "channelWork",
                "hostDirectoryBrowse",
                "hostQuota"
            ]))
        );
    }

    /// U1：太老的客户端被拒（-32009 + 码）；比 crewd 新的照收；hello 收未知字段（以后的客户端在这里加东西）。
    #[tokio::test]
    async fn hello_refuses_a_client_older_than_the_minimum_and_accepts_newer_ones() {
        let st = state();
        let old = crewd_proto::MIN_API_VERSION - 1;
        let e = hello(
            &st,
            json!({"client": {"name": "合成客户端", "version": "0.0.1"}, "apiVersion": old}),
        )
        .await
        .unwrap_err();
        assert_eq!(
            (e.code, e.data.map(|d| d.refusal)),
            (RpcError::CONFLICT, Some("system.api_incompatible".into()))
        );
        assert!(e.message.contains(&old.to_string()), "{}", e.message);
        let newer = crewd_proto::API_VERSION + 1;
        let v = hello(
            &st,
            json!({
                "client": {"name": "合成客户端", "version": "9.9", "build": "abc", "future": 1},
                "apiVersion": newer,
                "capabilities": ["something.new"],
                "somethingAddedLater": {"x": 1}
            }),
        )
        .await
        .unwrap();
        assert_eq!(v["apiVersion"], 3);
        assert!(
            hello(&st, json!({"apiVersion": "3"})).await.is_err(),
            "类型不对照样是参数错"
        );
    }

    /// T10a 复审：起一个不存在的机组里的 session = `crew.not_found`（-32004），什么都不建；两个 Port 同一个答（契约测试）。
    #[tokio::test]
    async fn starting_in_an_unknown_crew_is_not_found() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let req = StartSession {
            codex_options: None,

            crew_id: "没有这个".into(),
            captain: false,
            executor: "claude".into(),
            brief: "合成任务".into(),
            title: None,
            model: None,
            effort: None,
            isolation: false,
        };
        let e = rig
            .cmd(|reply| Cmd::Start {
                req,
                origin: Origin::Human,
                reply,
            })
            .await
            .unwrap_err();
        let r = rpc_err(e);
        assert_eq!(
            (r.code, r.data.map(|d| d.refusal)),
            (RpcError::NOT_FOUND, Some("crew.not_found".into()))
        );
        assert_eq!(rig.port.lock().sessions.len(), 1, "没写行");
    }

    /// `ControlError::X(…)` 括号里（`//` 注释之外、`#[cfg(test)] mod tests` 之前）带中日韩字符的那几段。
    /// 括号按字符配对（中文句子里用的是全角括号，不影响配对）。
    fn prose_in_control_errors(src: &str) -> Vec<String> {
        fn cjk(c: char) -> bool {
            ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3000}'..='\u{303f}').contains(&c)
        }
        let code = match src.find("#[cfg(test)]\nmod tests") {
            Some(i) => &src[..i],
            None => src,
        };
        let code: String = code
            .lines()
            .map(|l| match l.find("//") {
                Some(i) => &l[..i],
                None => l,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut hits = Vec::new();
        let mut from = 0;
        while let Some(i) = code[from..].find("ControlError::") {
            let start = from + i;
            let rest = &code[start..];
            let head = "ControlError::".len();
            let ident_end = rest[head..]
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .map_or(rest.len(), |n| n + head);
            from = start + ident_end;
            if !rest[ident_end..].starts_with('(') {
                continue;
            }
            let mut depth = 0;
            let mut end = rest.len();
            for (k, c) in rest[ident_end..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = ident_end + k + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if rest[..end].chars().any(cjk) {
                hits.push(rest[..end].to_string());
            }
        }
        hits
    }

    /// 修复轮 M3：编排给客户端 / agent 的拒绝都是码 + 数据，不在构造 `ControlError` 时塞中文句子（文字在 collab 的
    /// `prompts/refusals.*.md`）。扫 orchestra 的全部非测试源码。
    #[test]
    fn control_errors_are_built_from_codes_not_prose() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/orchestra");
        let mut seen = 0;
        let mut hits = Vec::new();
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || name == "testkit.rs" || name == "port_contract.rs" {
                continue;
            }
            seen += 1;
            for h in prose_in_control_errors(&std::fs::read_to_string(&path).unwrap()) {
                hits.push(format!("{name}: {h}"));
            }
        }
        assert!(seen >= 10, "只扫到 {seen} 个文件，目录读错了？");
        assert!(
            hits.is_empty(),
            "ControlError 里有中文句子（换成拒绝码）：\n{}",
            hits.join("\n")
        );
    }

    /// 扫描器自己会红（防「从来不发声的检查」）：修复轮之前的那一处原样抓得到；注释里的、测试里的不算；带码的不算。
    #[test]
    fn the_control_error_scanner_catches_a_prose_literal() {
        let old = "let b = x.ok_or_else(|| {\n    ControlError::Worktree(format!(\n        \"机组 {} 没有工作目录\",\n        c\n    ))\n})?;";
        assert_eq!(prose_in_control_errors(old).len(), 1);
        let ok = "let e = ControlError::CrewNoWorkdir(c.clone()); // 机组没有工作目录\n#[cfg(test)]\nmod tests {\n    ControlError::Worktree(\"合成\".into())\n}";
        assert!(prose_in_control_errors(ok).is_empty());
    }

    #[test]
    fn s42_cursor_unknown_wire_has_no_fabricated_cause_and_preserves_legacy_defaults() {
        use crewd_proto::orchestra::CausalBlockageView;
        use crewd_wake::explain::CausalBlockage;
        let empty = causal_blockage_view(&CausalBlockage::default());
        assert_eq!(serde_json::to_value(&empty).unwrap(), serde_json::json!({}));
        let old: CausalBlockageView = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(old, empty);
        let flag = CausalBlockage {
            cursor_unknown: true,
            ..Default::default()
        };
        let blocker = blocker_view(&Blocker::Causal { blockage: flag });
        assert_eq!(blocker.code, BlockerCodeView::CausalCursorUnknown);
        let wire = serde_json::to_value(&blocker).unwrap();
        assert_eq!(wire["causal"], serde_json::json!({"cursorUnknown":true}));
        assert_eq!(wire["code"], "causal_cursor_unknown");
        assert!(wire.get("detail").is_none());
        assert_eq!(
            serde_json::from_value::<WakeBlocker>(wire).unwrap(),
            blocker
        );
    }

    #[test]
    fn causal_wire_distinguishes_persisted_holds_candidates_and_unresolved_without_legacy_fields() {
        use crewd_wake::explain::{
            CausalBlockage, CausalChainCandidate, CausalChainHold, CausalUnresolved,
        };
        let candidate = CausalChainCandidate {
            chain_id: "root-budget".into(),
            from: "a".into(),
            target: "b".into(),
            hop: 17,
            reason: "budget".into(),
        };
        let mut b = CausalBlockage {
            hold_candidates: vec![candidate],
            ..Default::default()
        };
        let candidate_wire = serde_json::to_value(blocker_view(&Blocker::Causal {
            blockage: b.clone(),
        }))
        .unwrap();
        assert_eq!(candidate_wire["code"], "causal_budget");
        assert!(
            candidate_wire["causal"]["holdCandidates"][0]
                .get("atMs")
                .is_none(),
            "readonly candidate is not a persisted episode"
        );
        assert_eq!(
            candidate_wire["causal"]["holdCandidates"][0]["targetSessionId"],
            "b"
        );
        b.unresolved.push(CausalUnresolved {
            message_id: "m1".into(),
            seq: 1,
            from: "c".into(),
            target: "b".into(),
            output_id: None,
        });
        assert_eq!(
            blocker_view(&Blocker::Causal {
                blockage: b.clone()
            })
            .code,
            BlockerCodeView::CausalUnresolved
        );
        b.holds.push(CausalChainHold {
            chain_id: "root-held".into(),
            from: "d".into(),
            target: "b".into(),
            hop: 17,
            reason: "budget".into(),
            at_ms: 123,
        });
        let mixed = blocker_view(&Blocker::Causal { blockage: b });
        assert_eq!(mixed.code, BlockerCodeView::Held);
        let wire = serde_json::to_value(&mixed).unwrap();
        assert_eq!(wire["causal"]["holds"][0]["atMs"], 123);
        assert_eq!(wire["causal"]["unresolved"].as_array().unwrap().len(), 1);
        assert_eq!(
            wire["causal"]["holdCandidates"].as_array().unwrap().len(),
            1
        );
        assert!(
            wire.get("detail").is_none(),
            "never manufacture a legacy latch identity"
        );
        assert_eq!(serde_json::from_value::<WakeBlocker>(wire).unwrap(), mixed);
        let legacy = serde_json::json!({"code":"not_due", "untilMs":123});
        assert_eq!(
            serde_json::to_value(serde_json::from_value::<WakeBlocker>(legacy.clone()).unwrap())
                .unwrap(),
            legacy
        );
    }

    #[tokio::test]
    async fn unknown_causal_episode_persists_one_honest_bilingual_notice_in_actual_transaction() {
        let mut observations = Vec::new();
        for locale in [crewd_collab::Locale::Zh, crewd_collab::Locale::En] {
            let f = crate::agent::testkit::Fixture::new().await;
            let db = f.state.db.clone();
            let crew = f.crew.clone();
            let sender = f.captain.clone();
            let target = f.worker.clone();
            let observed = db
                .call(move |s| -> anyhow::Result<_> {
                    use crewd_collab::{causal, cursor_reservations as cr, messages};
                    let now = crewd_store::now_ms();
                    let channel = format!("{crew}/lobby");
                    crewd_collab::run(s, now, |tx| {
                        crewd_collab::settings::set_locale(tx, locale)?;
                        messages::post(
                            tx,
                            crewd_collab::Sender::Human,
                            messages::NewPost {
                                channel_id: channel.clone(),
                                text: "synthetic parent not yet successful".into(),
                                ..Default::default()
                            },
                        )
                    })?;
                    let (claimed, _) = crewd_collab::run(s, now, |tx| {
                        let prepared = cr::prepare_causal_unread(tx, &sender)?;
                        let ends = prepared
                            .blocks
                            .iter()
                            .filter_map(|(ch, ms)| ms.last().map(|m| (ch.clone(), m.seq)))
                            .collect::<Vec<_>>();
                        let causes = prepared.causes.select(&ends)?;
                        let windows = prepared.snapshot.windows_for(&ends)?;
                        cr::claim_output(
                            tx,
                            &prepared.snapshot,
                            &causes,
                            &windows,
                            &[],
                            &[],
                            cr::Owner::new(1)?,
                        )
                    })?;
                    let claimed = match claimed {
                        cr::Decision::Applied(c) => c,
                        other => anyhow::bail!("owned pending fixture did not claim: {other:?}"),
                    };
                    let (child, _) = crewd_collab::run(s, now, |tx| {
                        messages::post(
                            tx,
                            crewd_collab::Sender::Session(sender.clone()),
                            messages::NewPost {
                                channel_id: channel.clone(),
                                text: "synthetic post before parent success".into(),
                                ..Default::default()
                            },
                        )
                    })?;
                    let child_unresolved = matches!(
                        causal::read_message(s.conn(), &child.message)?,
                        causal::MessageCause::Unresolved { .. }
                    );
                    for _ in 0..2 {
                        crewd_collab::run(s, now, |tx| {
                            let prepared = cr::prepare_causal_unread(tx, &target)?;
                            crate::causal_fence::blocked_channels(tx, &prepared.causes)
                        })?;
                    }
                    let holds = causal::holds(s.conn(), Some(&target))?;
                    let home = crewd_collab::crews::home_of(s.conn(), &crew)?;
                    let human = format!("{home}/human");
                    let notices = messages::list(s.conn(), &human, None, None, 100)?;
                    let success = claimed.pending.observe_success(
                        1,
                        claimed.pending.generation(),
                        &uuid::Uuid::new_v4().to_string(),
                        1,
                    )?;
                    crewd_collab::run(s, now, |tx| {
                        cr::ack_output_delivered(
                            tx,
                            &claimed,
                            &success,
                            crewd_collab::wakes::Riders::Release,
                        )
                    })?;
                    Ok((locale, child_unresolved, holds, notices))
                })
                .await;
            tokio::task::spawn_blocking(move || db.shutdown())
                .await
                .unwrap();
            drop(f);
            observations.push(observed);
        }
        for observation in observations {
            let (locale, unknown, holds, notices) = observation.unwrap().unwrap();
            assert!(unknown, "actual stored pre-success child is unresolved");
            assert_eq!(holds.len(), 1, "one actual diagnostic episode");
            assert_eq!(holds[0].hop, 0);
            let notices = notices
                .iter()
                .filter(|m| m.text.contains(&holds[0].chain_id))
                .collect::<Vec<_>>();
            assert_eq!(notices.len(), 1, "repeat prepare has one durable notice");
            let text = &notices[0].text;
            match locale {
                crewd_collab::Locale::Zh => {
                    assert!(text.contains("尚不能确认"));
                    assert!(!text.contains("超过"));
                }
                crewd_collab::Locale::En => {
                    assert!(text.contains("has not been established"));
                    assert!(!text.contains("exceeds"));
                }
            }
            assert!(
                notices[0]
                    .mentions
                    .iter()
                    .any(|m| m.kind == crewd_proto::MentionKind::Human)
            );
        }
    }
}
