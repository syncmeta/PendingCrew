//! Idle reads are candidates only; DbPort accepts them after the actor guard.
use super::{exec::BoxFut, hooks::IdleHook};
use crewd_store::Db;
use crewd_wake::{Ms, NewWake, Priority, WakeKind};
pub const CANDIDATE_SOURCE: &str = "work-stop-candidate:";
pub struct WorkIdle(pub Db);
impl IdleHook for WorkIdle {
    fn on_idle<'a>(&'a self, session: &'a str, now: Ms) -> BoxFut<'a, Option<NewWake>> {
        let owner = session.to_owned();
        Box::pin(async move {
            let read_owner = owner.clone();
            let result = self
                .0
                .call_result(move |s| {
                    crewd_collab::stop_checks::candidate(s.conn(), &read_owner, now.0)
                })
                .await;
            match result {
                Ok(Ok(Some(c))) => match serde_json::to_string(&c) {
                    Ok(note) => Some(NewWake {
                        session: owner,
                        kind: WakeKind::StopCheck,
                        source: CANDIDATE_SOURCE.into(),
                        priority: Priority::System,
                        not_before: now,
                        note: Some(note),
                        origin: crewd_wake::breaker::SYSTEM.into(),
                    }),
                    Err(e) => {
                        eprintln!("crewd: cannot encode work idle candidate: {e}");
                        None
                    }
                },
                Ok(Ok(None)) => None,
                Ok(Err(e)) => {
                    eprintln!("crewd: cannot read work idle candidate: {e}");
                    None
                }
                Err(e) => {
                    eprintln!("crewd: work idle database unavailable: {e}");
                    None
                }
            }
        })
    }
}
