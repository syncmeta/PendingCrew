//! Current-run execution admission and one local management connection.
//! Reconnecting only observes. Ownership transfers explicitly, never by PID.
use crate::{AppState, connection_scope::ConnectionScope, public_commands::Actor};
use crewd_proto::RpcError;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Debug)]
pub struct Admission {
    sessions: Mutex<HashSet<String>>,
    closed: AtomicBool,
    pub blocked: AtomicBool,
    pub started_at: i64,
}
impl Default for Admission {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashSet::new()),
            closed: AtomicBool::new(false),
            blocked: AtomicBool::new(false),
            started_at: crewd_store::now_ms(),
        }
    }
}
impl Admission {
    pub fn allow(&self, id: &str) {
        if !self.closed.load(Ordering::Acquire) && !self.blocked.load(Ordering::Acquire) {
            if let Ok(mut s) = self.sessions.lock() {
                s.insert(id.into());
            }
        }
    }
    pub fn contains(&self, id: &str) -> bool {
        !self.closed.load(Ordering::Acquire)
            && !self.blocked.load(Ordering::Acquire)
            && self.sessions.lock().is_ok_and(|s| s.contains(id))
    }
    pub fn any(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
            && !self.blocked.load(Ordering::Acquire)
            && self.sessions.lock().is_ok_and(|s| !s.is_empty())
    }
    pub fn revoke(&self, id: &str) {
        if let Ok(mut s) = self.sessions.lock() {
            s.remove(id);
        }
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}
pub(crate) struct Management {
    owner: Mutex<Option<ConnectionScope>>,
    pub previous_clean: bool,
}
impl Management {
    pub(crate) fn new(clean: bool) -> Self {
        Self {
            owner: Mutex::new(None),
            previous_clean: clean,
        }
    }
    fn status(
        &self,
        scope: &ConnectionScope,
        claim: bool,
        release: bool,
    ) -> Result<Value, RpcError> {
        let mut owner = self
            .owner
            .lock()
            .map_err(|_| refused("management state unavailable"))?;
        if owner.as_ref().is_some_and(|o| !o.is_live()) {
            *owner = None;
        }
        let is_owner = owner.as_ref().is_some_and(|o| o.id == scope.id);
        if claim && !is_owner {
            if owner.is_some() {
                return Err(refused("another live connection owns management"));
            }
            *owner = Some(scope.clone());
        }
        if release {
            if !is_owner {
                return Err(refused("current management owner required"));
            }
            *owner = None;
        }
        Ok(
            json!({"role":if owner.as_ref().is_some_and(|o|o.id==scope.id){"owner"}else{"observer"},"ownerPresent":owner.is_some(),"previousExit":if self.previous_clean{"clean"}else{"unclean"}}),
        )
    }
}
fn refused(s: &str) -> RpcError {
    RpcError::new(RpcError::FORBIDDEN, s)
}
pub(crate) async fn handle(
    state: &Arc<AppState>,
    actor: &Actor,
    method: &str,
    args: Value,
) -> Result<Value, RpcError> {
    if !matches!(actor, Actor::Local(_)) {
        return Err(refused("local Human management required"));
    }
    actor.authorize(state, "session.stop").await?;
    if method != "lifecycle.exit" {
        let _: crewd_proto::lifecycle::LifecycleEmpty = crewd_proto::parse_params(args.clone())?;
    }
    let scope = actor.connection_scope();
    let management = state
        .management
        .get()
        .ok_or_else(|| refused("management unavailable"))?;
    match method {
        "lifecycle.status" => management.status(scope, false, false),
        "lifecycle.claim" => management.status(scope, true, false),
        "lifecycle.release" => management.status(scope, false, true),
        "lifecycle.exit" => {
            if management.status(scope, false, false)?["role"] != "owner" {
                return Err(refused("current management owner required"));
            }
            let p: crewd_proto::lifecycle::LifecycleExitParams = crewd_proto::parse_params(args)?;
            if !matches!(p.mode.as_str(), "direct" | "safePause") {
                return Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    "mode must be direct or safePause",
                ));
            }
            state.admission.close();
            let handle = state
                .orchestra
                .get()
                .ok_or_else(|| refused("orchestra unavailable"))?;
            let wait = p
                .timeout_ms
                .unwrap_or(if p.mode == "direct" { 0 } else { 120000 })
                .min(300000);
            let report = handle
                .hibernate(Some(std::time::Duration::from_millis(
                    if p.mode == "direct" { 0 } else { wait },
                )))
                .await
                .map_err(crate::orchestra::rpc::rpc_err)?;
            // hibernate's completion contains actual child settlement, not a queue ACK.
            let status = if report.completed
                && report.failed.is_empty()
                && (p.mode == "direct" || report.interrupted.is_empty())
            {
                "stopped"
            } else {
                "timedOut"
            };
            state
                .db
                .call_result(|s| s.conn().execute_batch("PRAGMA wal_checkpoint(FULL)"))
                .await
                .map_err(|_| refused("storage completion unknown"))?
                .map_err(|_| refused("storage completion unknown"))?;
            Ok(
                json!({"status":status,"mode":p.mode,"clean":report.clean,"interrupted":report.interrupted,"failed":report.failed}),
            )
        }
        _ => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            "unknown lifecycle method",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn management_owner_release_eof_and_explicit_reattach() {
        let m = Management::new(true);
        let a = ConnectionScope::new();
        let b = ConnectionScope::new();
        assert_eq!(m.status(&a, false, false).unwrap()["role"], "observer");
        assert_eq!(m.status(&a, true, false).unwrap()["role"], "owner");
        assert!(m.status(&b, true, false).is_err());
        assert!(m.status(&b, false, true).is_err());
        m.status(&a, false, true).unwrap();
        assert_eq!(m.status(&b, false, false).unwrap()["role"], "observer");
        assert_eq!(m.status(&b, true, false).unwrap()["role"], "owner");
        b.close();
        assert_eq!(m.status(&a, false, false).unwrap()["ownerPresent"], false);
        assert_eq!(m.status(&a, true, false).unwrap()["role"], "owner");
    }
    #[test]
    fn current_run_admission_never_rehydrates_or_reopens_after_close() {
        let a = Admission::default();
        assert!(!a.any());
        assert!(!a.contains("old"));
        a.allow("selected");
        assert!(a.contains("selected"));
        assert!(!a.contains("old"));
        a.close();
        a.allow("later");
        assert!(!a.any());
        assert!(!a.contains("selected"));
        let next = Admission::default();
        assert!(!next.contains("selected"));
    }
    #[tokio::test]
    async fn lifecycle_formal_connections_nonowner_and_reattached_owner_exit() {
        use crate::agent::testkit::{Fixture, RpcClient};
        use crate::orchestra::{
            self, OrchestraConfig,
            clock::ManualClock,
            exec::Executors,
            hooks::Hooks,
            testkit::{MemPort, StandIn, T0},
        };
        for mode in ["safePause", "direct"] {
            let f = Fixture::new().await;
            assert!(f.state.management.set(Management::new(true)).is_ok());
            let port = Arc::new(MemPort::new());
            let (handle, actor) = orchestra::spawn(
                port,
                Executors::new().with("claude", Arc::new(StandIn::default())),
                Hooks::default(),
                Arc::new(ManualClock::new(T0)),
                OrchestraConfig::for_tests(),
            )
            .await
            .unwrap();
            assert!(f.state.orchestra.set(handle).is_ok());
            let socket = f.spawn_socket().await;
            let mut a = RpcClient::connect(&socket).await;
            let mut b = RpcClient::connect(&socket).await;
            assert_eq!(
                a.call("lifecycle.status", json!({})).await.unwrap()["role"],
                "observer"
            );
            a.call("lifecycle.claim", json!({})).await.unwrap();
            assert!(
                b.call("lifecycle.exit", json!({"mode":mode,"timeoutMs":0}))
                    .await
                    .is_err()
            );
            assert!(b.call("lifecycle.claim", json!({})).await.is_err());
            a.call("lifecycle.release", json!({})).await.unwrap();
            drop(a);
            assert_eq!(
                b.call("lifecycle.status", json!({})).await.unwrap()["role"],
                "observer"
            );
            b.call("lifecycle.claim", json!({})).await.unwrap();
            let result = b
                .call("lifecycle.exit", json!({"mode":mode,"timeoutMs":0}))
                .await
                .unwrap();
            assert_eq!(result["status"], "stopped");
            if mode == "safePause" {
                let sample = json!({"owner":b.call("lifecycle.status",json!({})).await.unwrap(),"exit":result});
                std::fs::write(
                    "/private/tmp/pcw-sqlite-lifecycle-serde-20261016.json",
                    serde_json::to_vec_pretty(&sample).unwrap(),
                )
                .unwrap();
            }
            assert!(result["failed"].as_array().unwrap().is_empty());
            actor.finished().await.unwrap();
        }
    }
}
