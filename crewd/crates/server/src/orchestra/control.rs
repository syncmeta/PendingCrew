//! M3a 的 `SessionControl`（M3b 的工具经它起停会话）由编排实现；这里也是编排的实时状态变成线上形状的唯一一处。
//! agent 起 / 恢复会话受可选上限约束（`Origin::Agent`）。
use std::sync::{Arc, OnceLock};

use crewd_exec::health::HealthKind;
use crewd_proto::{DecisionView, HealthKindView, HealthView, Role, Session, SessionLive};

use super::Handle;
use super::core::{ControlError, Origin, StartSession};
use super::port::LiveView;
use crate::control::{
    BoxFuture, ControlError as M3aError, Inspection, QuotaReport, SessionControl, StartRequest,
};

/// `AppState` 先建（编排的 Port 要持有它），编排后起：句柄晚到，放在 `OnceLock` 里。
pub struct OrchestraControl {
    cell: Arc<OnceLock<Handle>>,
    db: crewd_store::Db,
}

impl OrchestraControl {
    pub fn new(cell: Arc<OnceLock<Handle>>, db: crewd_store::Db) -> Self {
        Self { cell, db }
    }

    /// 编排还没起来（或没配执行器、根本不跑）。T10b 换成拒绝码的文字。
    fn h(&self) -> Result<&Handle, M3aError> {
        self.cell
            .get()
            .ok_or_else(|| M3aError::Failed("orchestra not running".into()))
    }

    async fn session(&self, id: String) -> Result<Session, M3aError> {
        self.db
            .call(move |s| crewd_collab::sessions::get(s.conn(), &id))
            .await
            .map_err(|e| M3aError::Failed(e.to_string()))?
            .map_err(|e| match e {
                crewd_collab::CollabError::NotFound(r) => M3aError::NotFound(r.to_string()),
                e => M3aError::Failed(e.to_string()),
            })
    }
}

/// 编排的错误 → M3a 的 `SessionControl` 词表（T10a 补充 §2.4）。文字暂用 `ControlError` 的 Display（T10b 换拒绝码）。
pub fn map_error(e: ControlError) -> M3aError {
    match e {
        ControlError::NotFound(s) => M3aError::NotFound(s),
        ControlError::AlreadyRunning(s) => M3aError::AlreadyRunning(s),
        ControlError::Unsupported(s) => M3aError::Unsupported(s),
        e @ (ControlError::Invalid(_)
        | ControlError::NotRunning(_)
        | ControlError::NoWorkdir(_)
        | ControlError::CaptainTaken(_)
        | ControlError::CrewRemoved(_)) => M3aError::Invalid(e.to_string()),
        // 建 worktree 失败（C8）带原因原样给调用方（M3b 的 `start_session` 回执 = 失败 + 这段原因）。
        e @ (ControlError::AtCap { .. }
        | ControlError::Worktree(_)
        | ControlError::ShuttingDown
        | ControlError::Internal(_)) => M3aError::Failed(e.to_string()),
    }
}

impl SessionControl for OrchestraControl {
    fn start(&self, req: StartRequest) -> BoxFuture<'_, Result<Session, M3aError>> {
        Box::pin(async move {
            let start = StartSession {
                crew_id: req.crew_id,
                captain: req.role == Role::Captain,
                executor: req.executor,
                brief: req.brief,
                title: req.title,
                model: req.model,
                effort: req.effort,
                isolation: req.isolation,
            };
            let row = self
                .h()?
                .start(start, Origin::Agent)
                .await
                .map_err(map_error)?;
            self.session(row.id).await
        })
    }

    fn resume(&self, session_id: String) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move {
            self.h()?
                .resume(&session_id, Origin::Agent)
                .await
                .map_err(map_error)
        })
    }

    fn stop(&self, session_id: String, reason: String) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move {
            eprintln!("crewd: 停止 session {session_id}：{reason}");
            self.h()?.stop(&session_id).await.map_err(map_error)
        })
    }

    /// T10c 收窄 nudge（交给人选的框不让 agent 代按）；这里原样转。
    fn nudge(&self, session_id: String, text: String) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move { self.h()?.nudge(&session_id, text).await.map_err(map_error) })
    }

    fn inspect(
        &self,
        session_id: String,
        max_lines: usize,
    ) -> BoxFuture<'_, Result<Inspection, M3aError>> {
        Box::pin(async move {
            let (live, screen) = self
                .h()?
                .inspect(&session_id, max_lines)
                .await
                .map_err(map_error)?;
            Ok(Inspection {
                live: live_to_proto(&live),
                screen,
            })
        })
    }

    /// 只落库、下次启动生效（控制者裁定 C5；中途 `/model` 切换不在 M3c）。
    fn set_profile(
        &self,
        session_id: String,
        model: Option<String>,
        effort: Option<String>,
    ) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move {
            self.h()?
                .set_profile(&session_id, model, effort)
                .await
                .map_err(map_error)
        })
    }

    fn handoff_captain(
        &self,
        crew_id: String,
        to_session_id: String,
    ) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move {
            self.h()?
                .set_captain(&crew_id, &to_session_id)
                .await
                .map_err(map_error)
        })
    }

    /// crewd 还读不到额度快照（`Health` 不带重置时刻、额度来源未定），如实返回空。
    fn quota(&self) -> BoxFuture<'_, Result<QuotaReport, M3aError>> {
        Box::pin(async { Ok(QuotaReport { lines: vec![] }) })
    }

    fn live(&self, session_id: String) -> BoxFuture<'_, Result<SessionLive, M3aError>> {
        Box::pin(async move {
            self.h()?
                .live(&session_id)
                .await
                .map(|v| live_to_proto(&v))
                .map_err(map_error)
        })
    }
}

/// 编排的实时状态 → 线上的 `SessionLive`（`session.live` / `session.changed`）。health 的种类一一对应（C11）。
pub fn live_to_proto(v: &LiveView) -> SessionLive {
    SessionLive {
        session_id: v.session_id.clone(),
        running: v.running,
        working: v.working,
        health: v.health.as_ref().map(|h| HealthView {
            kind: match h.kind {
                HealthKind::AuthRequired => HealthKindView::AuthRequired,
                HealthKind::UsageLimit => HealthKindView::UsageLimit,
                HealthKind::RateLimited => HealthKindView::RateLimited,
                HealthKind::LaunchFailed => HealthKindView::LaunchFailed,
                HealthKind::BriefUndelivered => HealthKindView::BriefUndelivered,
                HealthKind::PromptUndelivered => HealthKindView::PromptUndelivered,
                HealthKind::TurnFailed => HealthKindView::TurnFailed,
                HealthKind::VersionIncompatible => HealthKindView::VersionIncompatible,
            },
            detail: h.detail.clone(),
        }),
        pending_decision: v.pending_decision.as_ref().map(|d| DecisionView {
            summary: d.prompt.clone(),
            options: d.options.clone(),
            numbered: d.numbered,
        }),
        awaiting_question: v.awaiting_question.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_exec::health::Health;
    use crewd_term::PendingDecision;

    fn view(kind: HealthKind) -> LiveView {
        LiveView {
            session_id: "s".into(),
            running: true,
            working: false,
            health: Some(Health::new(kind, "x")),
            pending_decision: Some(PendingDecision {
                prompt: "Trust?".into(),
                options: vec!["Yes".into()],
                numbered: false,
            }),
            awaiting_question: None,
        }
    }

    #[test]
    fn errors_map_onto_the_m3a_vocabulary() {
        let s = || "s".to_string();
        assert_eq!(
            map_error(ControlError::NotFound(s())),
            M3aError::NotFound(s())
        );
        assert_eq!(
            map_error(ControlError::AlreadyRunning(s())),
            M3aError::AlreadyRunning(s())
        );
        assert_eq!(
            map_error(ControlError::Unsupported(s())),
            M3aError::Unsupported(s())
        );
        for e in [
            ControlError::Invalid(s()),
            ControlError::NotRunning(s()),
            ControlError::NoWorkdir(s()),
            ControlError::CaptainTaken(s()),
            ControlError::CrewRemoved(s()),
        ] {
            assert_eq!(
                map_error(e.clone()),
                M3aError::Invalid(e.to_string()),
                "{e:?}"
            );
        }
        for e in [
            ControlError::AtCap { max: 2 },
            ControlError::Worktree(s()),
            ControlError::ShuttingDown,
            ControlError::Internal(s()),
        ] {
            assert_eq!(
                map_error(e.clone()),
                M3aError::Failed(e.to_string()),
                "{e:?}"
            );
        }
    }

    /// 编排没起来（没配执行器 / 还在起）：每个方法都回 Failed，不挂住、不 panic；quota 照实回空。
    #[tokio::test]
    async fn without_a_running_orchestra_every_call_fails_plainly() {
        let db = crewd_store::Db::spawn(crewd_store::Store::open_in_memory().unwrap()).unwrap();
        let c = OrchestraControl::new(Arc::new(OnceLock::new()), db);
        let failed =
            |r: Result<(), M3aError>| assert!(matches!(r, Err(M3aError::Failed(_))), "{r:?}");
        failed(c.resume("s".into()).await);
        failed(c.stop("s".into(), "r".into()).await);
        failed(c.nudge("s".into(), "x".into()).await);
        failed(c.set_profile("s".into(), None, None).await);
        failed(c.handoff_captain("c".into(), "s".into()).await);
        failed(c.live("s".into()).await.map(|_| ()));
        failed(c.inspect("s".into(), 10).await.map(|_| ()));
        failed(c.start(StartRequest::default()).await.map(|_| ()));
        assert_eq!(c.quota().await, Ok(QuotaReport { lines: vec![] }));
    }

    /// 起来了的编排：agent 的 start 经句柄起会话、回注册后的行（真库里的 `Session`）。
    #[tokio::test]
    async fn an_agent_start_goes_through_the_handle_and_returns_the_registered_row() {
        use crate::orchestra::clock::ManualClock;
        use crate::orchestra::db::fixture::seeded;
        use crate::orchestra::exec::Executors;
        use crate::orchestra::hooks::Hooks;
        use crate::orchestra::testkit::StandIn;
        use crate::orchestra::{OrchestraConfig, spawn};
        let (state, port, _d) = seeded().await;
        let stand_in = StandIn::default();
        let execs = Executors::new().with("claude", Arc::new(stand_in.clone()));
        let (h, _actor) = spawn(
            Arc::new(port),
            execs,
            Hooks::default(),
            Arc::new(ManualClock::new(crewd_wake::Ms(1_000_000_000))),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        let cell = Arc::new(OnceLock::new());
        let c = OrchestraControl::new(cell.clone(), state.db.clone());
        cell.set(h).ok().unwrap();
        let s = c
            .start(StartRequest {
                crew_id: "c-1".into(),
                role: Role::Worker,
                executor: "claude".into(),
                brief: "合成任务甲：整理目录".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(s.crew_id, "c-1");
        assert_eq!(
            s.title.as_deref(),
            Some("合成任务甲：整理目录"),
            "标题从 brief 派生（§26.7）"
        );
        assert_eq!(stand_in.launches(&s.id).len(), 1);
        let e = c
            .start(StartRequest {
                crew_id: "c-1".into(),
                role: Role::Captain,
                executor: "claude".into(),
                brief: "合成任务乙".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(matches!(e, M3aError::Invalid(_)), "机组已有机长：{e:?}");
    }

    #[test]
    fn live_view_keeps_the_numbered_fact_and_every_health_kind_one_to_one() {
        let p = live_to_proto(&view(HealthKind::UsageLimit));
        assert_eq!(p.health.unwrap().kind, HealthKindView::UsageLimit);
        assert!(!p.pending_decision.unwrap().numbered, "绝不编造编号");
        // C11：M2c 的两种也一一对应（不再就近映射）。
        for (k, v) in [
            (HealthKind::AuthRequired, HealthKindView::AuthRequired),
            (HealthKind::RateLimited, HealthKindView::RateLimited),
            (HealthKind::LaunchFailed, HealthKindView::LaunchFailed),
            (
                HealthKind::BriefUndelivered,
                HealthKindView::BriefUndelivered,
            ),
            (
                HealthKind::PromptUndelivered,
                HealthKindView::PromptUndelivered,
            ),
            (HealthKind::TurnFailed, HealthKindView::TurnFailed),
            (
                HealthKind::VersionIncompatible,
                HealthKindView::VersionIncompatible,
            ),
        ] {
            assert_eq!(live_to_proto(&view(k)).health.unwrap().kind, v, "{k:?}");
        }
    }
}
