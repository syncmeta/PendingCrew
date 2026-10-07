//! M3a 的 `SessionControl`（M3b 的工具经它起停会话）由编排实现；这里也是编排的实时状态变成线上形状的唯一一处。
use crewd_exec::health::HealthKind;
use crewd_proto::{DecisionView, HealthKindView, HealthView, SessionLive};

use super::port::LiveView;

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
