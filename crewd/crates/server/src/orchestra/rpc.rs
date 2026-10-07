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
use super::control::live_to_proto;
use super::core::{ControlError, Origin, RefusedKind, SettingsUpdate, SettingsView, StartSession};
use super::deliver::{ListedWake, ReceiptRow, TargetSummary, WakeList};
use super::hibernate::HibernateReport;
use crate::AppState;
use crate::dispatch;

/// 这个方法归编排（M3c）：dispatch 在「未知方法」之前问它。
pub fn handles(method: &str) -> bool {
    methods::ALL.contains(&method)
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
        | ControlError::AtCap { .. }
        | ControlError::ShuttingDown
        | ControlError::NoWorkdir(_)
        | ControlError::Worktree(_)
        | ControlError::CrewRemoved(_)
        | ControlError::Unsupported(_) => RpcError::CONFLICT,
    };
    match e.refusal() {
        Some(r) => dispatch::refused(code, r),
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
        Blocker::ShuttingDown => BlockerCodeView::ShuttingDown,
        Blocker::CrewRemoved => BlockerCodeView::CrewRemoved,
        Blocker::Starting => BlockerCodeView::Starting,
        Blocker::NotDue { .. } => BlockerCodeView::NotDue,
        Blocker::Held { .. } => BlockerCodeView::Held,
        Blocker::BehindHigherPriority => BlockerCodeView::BehindHigherPriority,
        Blocker::Inflight => BlockerCodeView::Inflight,
        Blocker::Working => BlockerCodeView::Working,
        Blocker::Decision => BlockerCodeView::Decision,
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
        code,
        until_ms: b.until().map(|t| t.0),
        detail,
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
            reason: l.reason.clone(),
            at_ms: l.at.0,
        },
        LatchScope::Edge => {
            let (from, to) = edge_parts(&l.id).unwrap_or(("", l.id.as_str()));
            LatchView {
                scope: LatchScopeView::Edge,
                target_session_id: to.to_string(),
                from: Some(from.to_string()),
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
    let lang = match p.locale.as_deref() {
        None => None,
        Some("zh") => Some(Lang::Zh),
        Some("en") => Some(Lang::En),
        Some(other) => {
            return Err(ControlError::Invalid(
                Refusal::new(Code::SettingsBadLocale).arg("value", other),
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

/// 一个 M3c 方法（[`handles`] 已核过）。
pub async fn call(
    h: &Handle,
    state: &AppState,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    match method {
        methods::SESSION_START => {
            let p: SessionStartParams = parse_params(params)?;
            let req = StartSession {
                crew_id: p.crew_id,
                captain: p.role == Role::Captain,
                executor: p.executor,
                brief: p.brief,
                title: p.title,
                model: p.model,
                effort: p.effort,
                isolation: p.isolation,
            };
            let row = h.start(req, Origin::Human).await.map_err(rpc_err)?;
            let s = state
                .db
                .call(move |s| crewd_collab::sessions::get(s.conn(), &row.id))
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
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
            let v: Vec<LatchView> = h
                .latches()
                .await
                .map_err(rpc_err)?
                .iter()
                .map(latch_view)
                .collect();
            to_value(v)
        }
        methods::LOOP_RECOVER => {
            let p: LoopRecoverParams = parse_params(params)?;
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
                ControlError::Worktree("合成原因".into()),
                -32009,
                "session.worktree_failed",
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
                code: BlockerCodeView::NotDue,
                until_ms: Some(later.0),
                detail: None
            })
        );
        assert_eq!(w.message_id, None);
        assert_eq!(
            r.targets,
            [TargetView {
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
        let st = state();
        let v = hello(&st, Value::Null).await.unwrap();
        assert_eq!(v["apiVersion"], 3);
        assert_eq!(v["minApiVersion"], crewd_proto::MIN_API_VERSION);
        assert_eq!(v["capabilities"], json!([]), "没有编排：没有能力");
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
            v["capabilities"],
            json!([
                "orchestra",
                "crewd.hibernate",
                "wake.explain",
                "message.receipts",
                "settings"
            ])
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
}
