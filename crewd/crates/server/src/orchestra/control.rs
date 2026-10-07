//! M3a 的 `SessionControl`（M3b 的工具经它起停会话）由编排实现；这里也是编排的实时状态变成线上形状的唯一一处。
//! agent 起 / 恢复会话受可选上限约束（`Origin::Agent`）。
use std::sync::{Arc, OnceLock};

use crewd_exec::health::HealthKind;
use crewd_exec::session::HeldDraft;
use crewd_proto::{DecisionView, HealthKindView, HealthView, Role, Session, SessionLive};
use crewd_term::PendingDecision;

use super::Handle;
use super::core::{ControlError, Origin, RefusedKind, StartSession};
use super::port::LiveView;
use crate::control::{
    BoxFuture, ControlError as M3aError, Inspection, QuotaReport, SessionControl, StartRequest,
};
use crewd_collab::Locale;
use crewd_collab::refusal::{Code, Refusal};

/// `AppState` 先建（编排的 Port 要持有它），编排后起：句柄晚到，放在 `OnceLock` 里。
pub struct OrchestraControl {
    cell: Arc<OnceLock<Handle>>,
    db: crewd_store::Db,
}

impl OrchestraControl {
    pub fn new(cell: Arc<OnceLock<Handle>>, db: crewd_store::Db) -> Self {
        Self { cell, db }
    }

    /// 设置里的语言（给 agent 看的拒绝文字用它，T10b 补充 §2.4）；读不出来 = 中文（记一行）。
    async fn locale(&self) -> Locale {
        match self
            .db
            .call(|s| crewd_collab::settings::locale(s.conn()))
            .await
        {
            Ok(Ok(l)) => l,
            Ok(Err(e)) => {
                eprintln!("crewd: 编排：读语言设置失败，拒绝文字先用中文：{e}");
                Locale::Zh
            }
            Err(e) => {
                eprintln!("crewd: 编排：读语言设置失败，拒绝文字先用中文：{e}");
                Locale::Zh
            }
        }
    }

    /// 编排还没起来（或没配执行器、根本不跑）：`orchestra.not_running` 的文字。
    async fn h(&self) -> Result<&Handle, M3aError> {
        match self.cell.get() {
            Some(h) => Ok(h),
            None => Err(M3aError::Failed(
                Refusal::new(Code::OrchestraNotRunning).text(self.locale().await),
            )),
        }
    }

    /// 编排的结果：错误按设置的语言变成 M3a 的词表（只在出错时读语言）。
    async fn done<T>(&self, r: Result<T, ControlError>) -> Result<T, M3aError> {
        match r {
            Ok(v) => Ok(v),
            Err(e) => Err(map_error(e, self.locale().await)),
        }
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

/// 编排的错误 → M3a 的 `SessionControl` 词表（T10a 补充 §2.4）。文字 = 拒绝码按 `l` 渲染（T10b 补充 §2.4）；
/// 存储故障只给类别；内部错误给原文（agent 的回执，不上线给客户端）。
pub fn map_error(e: ControlError, l: Locale) -> M3aError {
    let text = match e.refusal() {
        Some(r) => r.text(l),
        None => e.to_string(),
    };
    match e {
        ControlError::NotFound(_)
        | ControlError::Refused {
            kind: RefusedKind::NotFound,
            ..
        } => M3aError::NotFound(text),
        ControlError::AlreadyRunning(_) => M3aError::AlreadyRunning(text),
        ControlError::Unsupported(_) => M3aError::Unsupported(text),
        ControlError::Invalid(_)
        | ControlError::NotRunning(_)
        | ControlError::NoDecision(_)
        | ControlError::DecisionChanged { .. }
        | ControlError::NeedsTerminal(_)
        | ControlError::NothingToInterrupt(_)
        | ControlError::InputDraft { .. }
        | ControlError::NoWorkdir(_)
        | ControlError::CaptainTaken(_)
        | ControlError::CrewRemoved(_)
        | ControlError::Refused { .. } => M3aError::Invalid(text),
        // 建 worktree 失败（C8）带原因原样给调用方（M3b 的 `start_session` 回执 = 失败 + 这段原因）。
        ControlError::AtCap { .. }
        | ControlError::Worktree(_)
        | ControlError::CrewNoWorkdir(_)
        | ControlError::ShuttingDown
        | ControlError::Storage { .. }
        | ControlError::Internal(_) => M3aError::Failed(text),
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
            let r = self.h().await?.start(start, Origin::Agent).await;
            let row = self.done(r).await?;
            self.session(row.id).await
        })
    }

    fn resume(&self, session_id: String) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move {
            let r = self.h().await?.resume(&session_id, Origin::Agent).await;
            self.done(r).await
        })
    }

    fn stop(&self, session_id: String, reason: String) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move {
            eprintln!("crewd: 停止 session {session_id}：{reason}");
            let r = self.h().await?.stop(&session_id).await;
            self.done(r).await
        })
    }

    /// T10c 收窄 nudge（交给人选的框不让 agent 代按）；这里原样转。文字 nudge 的 `Ok` = 已排进会话（执行器写之前再看
    /// 一次输入框：有人的草稿就挡着，等人清掉再写，T10d fix M4），不是「已敲进去」。
    fn nudge(&self, session_id: String, text: String) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move {
            let r = self.h().await?.nudge(&session_id, text).await;
            self.done(r).await
        })
    }

    fn inspect(
        &self,
        session_id: String,
        max_lines: usize,
    ) -> BoxFuture<'_, Result<Inspection, M3aError>> {
        Box::pin(async move {
            let r = self.h().await?.inspect(&session_id, max_lines).await;
            let (live, screen) = self.done(r).await?;
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
            let r = self
                .h()
                .await?
                .set_profile(&session_id, model, effort)
                .await;
            self.done(r).await
        })
    }

    fn handoff_captain(
        &self,
        crew_id: String,
        to_session_id: String,
    ) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move {
            let r = self.h().await?.set_captain(&crew_id, &to_session_id).await;
            self.done(r).await
        })
    }

    /// crewd 还读不到额度快照（`Health` 不带重置时刻、额度来源未定），如实返回空。
    fn quota(&self) -> BoxFuture<'_, Result<QuotaReport, M3aError>> {
        Box::pin(async { Ok(QuotaReport { lines: vec![] }) })
    }

    fn live(&self, session_id: String) -> BoxFuture<'_, Result<SessionLive, M3aError>> {
        Box::pin(async move {
            let r = self.h().await?.live(&session_id).await;
            Ok(live_to_proto(&self.done(r).await?))
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
        pending_decision: v.pending_decision.as_ref().map(decision_view),
        awaiting_question: v.awaiting_question.clone(),
        input_draft: match &v.input_draft {
            Some(HeldDraft::Text(t)) => Some(t.clone()),
            _ => None,
        },
        input_draft_unknown: v.input_draft == Some(HeldDraft::Unknown),
    }
}

/// 屏上的待决策框 → 线上的 `DecisionView`（`session.live` 与 `decision.changed` 的 `data.decision` 共用）。
pub fn decision_view(d: &PendingDecision) -> DecisionView {
    DecisionView {
        summary: d.prompt.clone(),
        options: d.options.clone(),
        numbered: d.numbered,
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
                selected: None,
            }),
            awaiting_question: None,
            input_draft: None,
        }
    }

    /// 每个变体 → M3a 的词（T10a 补充 §2.4）；文字 = 拒绝码按设置的语言（T10b 补充 §2.4）。
    #[test]
    fn errors_map_onto_the_m3a_vocabulary_with_refusal_text_in_the_set_language() {
        use crewd_store::StorageKind;
        let s = || "s-1".to_string();
        let invalid = ControlError::Invalid(Refusal::new(Code::OrchestraNudgeEmpty));
        let refused = |kind| ControlError::Refused {
            kind,
            refusal: Refusal::new(Code::SessionTitleTooLong)
                .arg("max", "5")
                .arg("actual", "9"),
        };
        type Want = fn(String) -> M3aError;
        let table: Vec<(ControlError, Want)> = vec![
            (ControlError::NotFound(s()), M3aError::NotFound),
            (refused(RefusedKind::NotFound), M3aError::NotFound),
            (ControlError::AlreadyRunning(s()), M3aError::AlreadyRunning),
            (
                ControlError::Unsupported("codex".into()),
                M3aError::Unsupported,
            ),
            (invalid.clone(), M3aError::Invalid),
            (ControlError::NotRunning(s()), M3aError::Invalid),
            (ControlError::NoWorkdir(s()), M3aError::Invalid),
            (ControlError::CaptainTaken("c-1".into()), M3aError::Invalid),
            (ControlError::CrewRemoved("c-1".into()), M3aError::Invalid),
            (ControlError::NoDecision(s()), M3aError::Invalid),
            (
                ControlError::DecisionChanged {
                    session: s(),
                    now: None,
                },
                M3aError::Invalid,
            ),
            (ControlError::NeedsTerminal(s()), M3aError::Invalid),
            (ControlError::NothingToInterrupt(s()), M3aError::Invalid),
            (
                ControlError::InputDraft {
                    session: s(),
                    text: Some("draft".into()),
                },
                M3aError::Invalid,
            ),
            (
                ControlError::InputDraft {
                    session: s(),
                    text: None,
                },
                M3aError::Invalid,
            ),
            (refused(RefusedKind::Invalid), M3aError::Invalid),
            (refused(RefusedKind::Forbidden), M3aError::Invalid),
            (refused(RefusedKind::Conflict), M3aError::Invalid),
            (ControlError::AtCap { max: 2 }, M3aError::Failed),
            (ControlError::Worktree("合成原因".into()), M3aError::Failed),
            (ControlError::CrewNoWorkdir("c-1".into()), M3aError::Failed),
            (ControlError::ShuttingDown, M3aError::Failed),
        ];
        for (e, want) in table {
            for l in [Locale::Zh, Locale::En] {
                let text = e.refusal().expect("规则拒绝都带码").text(l);
                assert_eq!(map_error(e.clone(), l), want(text), "{e:?} {l:?}");
            }
        }
        let en = map_error(ControlError::NotRunning(s()), Locale::En);
        assert!(
            matches!(&en, M3aError::Invalid(t) if t.contains("s-1") && t.is_ascii()),
            "英文设置给英文文字：{en:?}"
        );
        // 没有码的：存储故障只说类别（原文不给），内部错误给原文。
        let storage = ControlError::Storage {
            kind: StorageKind::DiskFull,
            detail: "/secret/crewd.db".into(),
        };
        assert_eq!(
            map_error(storage, Locale::En),
            M3aError::Failed(StorageKind::DiskFull.describe().into())
        );
        assert_eq!(
            map_error(ControlError::Internal("合成内部".into()), Locale::En),
            M3aError::Failed("合成内部".into())
        );
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
        // 文字是 `orchestra.not_running`、按设置的语言（库里没设 = 中文）。
        let zh = Refusal::new(Code::OrchestraNotRunning).text(Locale::Zh);
        assert_eq!(c.resume("s".into()).await, Err(M3aError::Failed(zh)));
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
