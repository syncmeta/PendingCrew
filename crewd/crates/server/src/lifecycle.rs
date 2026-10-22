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
        "lifecycle.exit" => finish_exit(state, begin_exit(state, actor, args).await?).await,
        _ => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            "unknown lifecycle method",
        )),
    }
}

pub(crate) struct PendingExit {
    mode: String,
    response: tokio::sync::oneshot::Receiver<
        Result<crate::orchestra::hibernate::HibernateReport, crate::orchestra::core::ControlError>,
    >,
}

pub(crate) async fn begin_exit(
    state: &Arc<AppState>,
    actor: &Actor,
    args: Value,
) -> Result<PendingExit, RpcError> {
    if !matches!(actor, Actor::Local(_)) {
        return Err(refused("local Human management required"));
    }
    actor.authorize(state, "session.stop").await?;
    let management = state
        .management
        .get()
        .ok_or_else(|| refused("management unavailable"))?;
    if management.status(actor.connection_scope(), false, false)?["role"] != "owner" {
        return Err(refused("current management owner required"));
    }
    let p: crewd_proto::lifecycle::LifecycleExitParams = crewd_proto::parse_params(args)?;
    if !matches!(p.mode.as_str(), "direct" | "safePause") {
        return Err(RpcError::new(
            RpcError::INVALID_PARAMS,
            "mode must be direct or safePause",
        ));
    }
    let handle = state
        .orchestra
        .get()
        .ok_or_else(|| refused("orchestra unavailable"))?;
    let wait = p
        .timeout_ms
        .unwrap_or(if p.mode == "direct" { 0 } else { 120000 })
        .min(300000);
    state.admission.close();
    let response = handle
        .enqueue_hibernate(Some(std::time::Duration::from_millis(
            if p.mode == "direct" { 0 } else { wait },
        )))
        .map_err(crate::orchestra::rpc::rpc_err)?;
    Ok(PendingExit {
        mode: p.mode,
        response,
    })
}

pub(crate) async fn finish_exit(
    state: &Arc<AppState>,
    pending: PendingExit,
) -> Result<Value, RpcError> {
    let report = pending
        .response
        .await
        .map_err(|_| refused("stop completion unknown"))?
        .map_err(crate::orchestra::rpc::rpc_err)?;
    // Completion is actual child settlement, never acknowledgement of enqueue.
    let status = if report.completed
        && report.failed.is_empty()
        && (pending.mode == "direct" || report.interrupted.is_empty())
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
        json!({"status":status,"mode":pending.mode,"clean":report.clean,
        "interrupted":report.interrupted,"failed":report.failed}),
    )
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
    async fn lifecycle_same_socket_safe_wait_accepts_owner_force_and_rejects_observer() {
        use crate::agent::testkit::{Fixture, RpcClient};
        use crate::orchestra::{
            self, OrchestraConfig,
            clock::ManualClock,
            core::Origin,
            exec::Executors,
            hooks::Hooks,
            port::{NewSession, Port},
            testkit::{MemPort, StandIn, T0},
        };
        use crewd_exec::session::SessionEvent;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let f = Fixture::new().await;
        f.state.management.set(Management::new(true)).ok().unwrap();
        let port = Arc::new(MemPort::new());
        port.lock().now = T0;
        let row = port
            .insert_session(NewSession {
                codex_options:None,
                crew_id: "c1".into(),
                captain: true,
                executor: "claude".into(),
                brief: String::new(),
                title: Some("busy".into()),
                model: None,
                effort: None,
                worktree: None,
            })
            .await
            .unwrap();
        let exec = StandIn::default();
        let mut cfg = OrchestraConfig::for_tests();
        cfg.admission = Some(f.state.admission.clone());
        f.state.admission.allow(&row.id);
        let (handle, actor) = orchestra::spawn(
            port,
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(T0)),
            cfg,
        )
        .await
        .unwrap();
        handle.resume(&row.id, Origin::Human).await.unwrap();
        for _ in 0..200 {
            if handle.live(&row.id).await.unwrap().running {
                break;
            }
            tokio::task::yield_now().await;
        }
        exec.emit(&row.id, SessionEvent::Working(true));
        for _ in 0..200 {
            if handle.live(&row.id).await.unwrap().working {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(handle.live(&row.id).await.unwrap().working);
        f.state.orchestra.set(handle).ok().unwrap();
        let socket = f.spawn_socket().await;
        let stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        async fn reply(read: &mut BufReader<tokio::net::unix::OwnedReadHalf>) -> Value {
            let mut line = String::new();
            assert!(read.read_line(&mut line).await.unwrap() > 0);
            serde_json::from_str(&line).unwrap()
        }
        write
            .write_all(b"{\"id\":1,\"method\":\"lifecycle.claim\",\"params\":{}}\n")
            .await
            .unwrap();
        assert_eq!(reply(&mut read).await["result"]["role"], "owner");
        let mut observer = RpcClient::connect(&socket).await;
        assert!(
            observer
                .call("lifecycle.exit", json!({"mode":"direct"}))
                .await
                .is_err()
        );
        assert!(f.state.admission.contains(&row.id));
        write.write_all(b"{\"id\":2,\"method\":\"lifecycle.exit\",\"params\":{\"mode\":\"safePause\",\"timeoutMs\":120000}}\n{\"id\":3,\"method\":\"lifecycle.status\",\"params\":{}}\n").await.unwrap();
        let status = tokio::time::timeout(std::time::Duration::from_millis(500), reply(&mut read))
            .await
            .expect("owner connection must remain readable during safe stop");
        assert_eq!(status["id"], 3);
        assert_eq!(status["result"]["role"], "owner");
        assert!(!f.state.admission.any());
        write
            .write_all(
                b"{\"id\":4,\"method\":\"lifecycle.exit\",\"params\":{\"mode\":\"direct\"}}\n",
            )
            .await
            .unwrap();
        let mut results = std::collections::BTreeMap::new();
        for _ in 0..2 {
            let value = tokio::time::timeout(std::time::Duration::from_secs(2), reply(&mut read))
                .await
                .unwrap();
            results.insert(value["id"].as_u64().unwrap(), value["result"].clone());
        }
        assert_eq!(results[&2]["status"], "timedOut");
        assert_eq!(results[&4]["status"], "stopped");
        assert_eq!(results[&4]["interrupted"], json!([row.id]));
        assert!(results[&4]["failed"].as_array().unwrap().is_empty());
        f.state.admission.allow("later");
        assert!(!f.state.admission.any());
        assert_eq!(exec.lock().interrupts.len(), 1);
        drop(write);
        drop(read);
        drop(observer);
        actor.finished().await.unwrap();
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
