//! Qualified Human search and bounded, provider-free index maintenance.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_collab::{managed_objects, organization_search as search};
use crewd_proto::{Request, Response, RpcError, organization_search as wire};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, request: Request) -> Response {
    let id = request.id;
    match route(state, actor, request).await {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, error),
    }
}

async fn route(state: &Arc<AppState>, actor: &Actor, request: Request) -> Result<Value, RpcError> {
    let principal = actor.principal(state).await?;
    let actor = actor.clone();
    let owner = state.clone();
    state
        .db
        .call(move |store| {
            let conn = store.conn();
            actor
                .authorize_tx(&owner, conn, &request.method)
                .map_err(dispatch::collab_err)?;
            let by = managed_objects::Actor::Human(&principal);
            let value = match request.method.as_str() {
                wire::SEARCH => {
                    let query = crewd_proto::parse_params::<wire::Search>(request.params)?;
                    ensure_available(&owner).map_err(dispatch::collab_err)?;
                    let clock = crate::search_clock::read().map_err(dispatch::collab_err)?;
                    serde_json::to_value(
                        search::search(conn, by, query, clock).map_err(dispatch::collab_err)?,
                    )
                    .map_err(dispatch::internal)?
                }
                wire::EXPAND => {
                    let query = crewd_proto::parse_params::<wire::Expand>(request.params)?;
                    ensure_available(&owner).map_err(dispatch::collab_err)?;
                    let clock = crate::search_clock::read().map_err(dispatch::collab_err)?;
                    serde_json::to_value(
                        search::expand(conn, by, query, clock).map_err(dispatch::collab_err)?,
                    )
                    .map_err(dispatch::internal)?
                }
                _ => {
                    return Err(RpcError::new(
                        RpcError::METHOD_NOT_FOUND,
                        "Unknown organization search method",
                    ));
                }
            };
            actor
                .authorize_tx(&owner, conn, &request.method)
                .map_err(dispatch::collab_err)?;
            Ok(value)
        })
        .await
        .map_err(dispatch::internal)?
}

/// Checked inside the FIFO after the caller's current authorization. A failed
/// maintenance step is not a successful empty result or indefinite progress.
pub(crate) fn ensure_available(state: &AppState) -> crewd_collab::Result<()> {
    if state
        .organization_search_runtime
        .get()
        .is_some_and(|runtime| runtime.healthy.load(Ordering::Acquire))
    {
        Ok(())
    } else {
        Err(crewd_collab::CollabError::Conflict(
            crewd_collab::refusal::Refusal::new(crewd_collab::refusal::Code::SearchUnavailable),
        ))
    }
}

/// Uses the same FIFO and transaction contracts as ordinary writes. It never
/// launches a provider, wakes an agent, or runs inside the actor's timer loop.
pub(crate) struct Runtime {
    worker: Mutex<Option<JoinHandle<()>>>,
    healthy: Arc<AtomicBool>,
}

impl Runtime {
    pub(crate) fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }

    pub(crate) fn start(state: &Arc<AppState>) -> Arc<Self> {
        let weak = Arc::downgrade(state);
        let healthy = Arc::new(AtomicBool::new(true));
        let worker_health = healthy.clone();
        let worker = tokio::spawn(async move {
            let mut failed = false;
            let mut retry = Duration::from_secs(1);
            loop {
                let Some(state) = weak.upgrade() else { break };
                let outcome = state
                    .db
                    .call_result(|store| -> crewd_collab::Result<bool> {
                        let now = crewd_store::now_ms();
                        if !search::maintenance_needed(store.conn(), now)? {
                            return Ok(false);
                        }
                        let (more, _) =
                            crewd_collab::run(store, now, |tx| search::maintenance_step(tx, 64))?;
                        Ok(more)
                    })
                    .await;
                drop(state);
                let delay = match outcome {
                    Ok(Ok(more)) => {
                        worker_health.store(true, Ordering::Release);
                        failed = false;
                        retry = Duration::from_secs(1);
                        if more {
                            Duration::from_millis(25)
                        } else {
                            Duration::from_secs(1)
                        }
                    }
                    error => {
                        worker_health.store(false, Ordering::Release);
                        if !failed {
                            eprintln!(
                                "crewd: organization search maintenance unavailable: {error:?}"
                            );
                        }
                        failed = true;
                        let delay = retry;
                        retry = (retry * 2).min(Duration::from_secs(30));
                        delay
                    }
                };
                tokio::time::sleep(delay).await;
            }
        });
        Arc::new(Self {
            worker: Mutex::new(Some(worker)),
            healthy,
        })
    }

    pub(crate) async fn shutdown(&self) {
        if let Some(worker) = self.worker.lock().await.take() {
            worker.abort();
            let _ = worker.await;
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.get_mut().take() {
            worker.abort();
        }
    }
}

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use crate::agent::testkit::{Fixture, RpcClient};
    use serde_json::json;
    use std::time::Duration;

    async fn health(f: &Fixture, expected: bool) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if f.state
                    .organization_search_runtime
                    .get()
                    .unwrap()
                    .is_healthy()
                    == expected
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the real maintenance loop must publish its current health");
    }

    #[tokio::test]
    async fn actual_maintenance_failure_is_unavailable_and_recovers_without_business_changes() {
        let f = Fixture::new().await;
        f.enable_public_command_identity().await;
        let message = f
            .rpc(
                "message.post",
                json!({
                    "channelId": format!("{}/lobby", f.crew), "text": "maintenance-health-needle"
                }),
            )
            .await;
        let message_id = message["id"].as_str().unwrap().to_owned();
        let before = f.state.db.call_result(|store| {
            store.conn().query_row("SELECT (SELECT count(*) FROM messages),(SELECT count(*) FROM wakes),(SELECT count(*) FROM organization_work_events)", [], |row| Ok((row.get::<_,i64>(0)?, row.get::<_,i64>(1)?, row.get::<_,i64>(2)?)))
        }).await.unwrap().unwrap();
        // Owned derived data only. Renaming back restores the same rows and
        // SQLite rewrites the actual triggers, without touching business facts.
        f.state
            .db
            .call_result(|store| {
                store.conn().execute_batch(
            "ALTER TABLE organization_search_dirty RENAME TO organization_search_dirty_fault_saved"
        )
            })
            .await
            .unwrap()
            .unwrap();
        f.enable_organization_search();
        health(&f, false).await;
        let (shutdown, stopping) = tokio::sync::oneshot::channel();
        let server = f.spawn_owned_socket(stopping).await;
        let socket = f.data_dir().join("crewd.sock");
        let mut human = RpcClient::connect(&socket).await;
        let target = json!({"kind":"message","id":message_id});
        for (method, params) in [
            (
                "organization.search",
                json!({"query":"maintenance-health-needle"}),
            ),
            ("organization.search.expand", json!({"target":target})),
        ] {
            let error = human.call(method, params).await.unwrap_err();
            assert_eq!(error.0, crewd_proto::RpcError::CONFLICT);
            assert!(!error.1.contains("organization_search_dirty"));
        }
        let mut worker = RpcClient::connect(&socket).await;
        worker
            .call(
                "agent.attach",
                json!({"sessionId":f.worker,"token":f.token(&f.worker)}),
            )
            .await
            .unwrap();
        // Authorization still precedes malformed params and index health.
        assert_eq!(
            worker
                .call("organization.search", json!([]))
                .await
                .unwrap_err()
                .0,
            crewd_proto::RpcError::FORBIDDEN
        );
        for (tool, args) in [
            ("search", json!({"query":"maintenance-health-needle"})),
            ("organization_search_expand", json!({"target":target})),
        ] {
            let result = worker
                .call("agent.call", json!({"tool":tool,"args":args}))
                .await
                .unwrap();
            assert_eq!(result["isError"], true);
            let expected =
                crewd_collab::refusal::Refusal::new(crewd_collab::refusal::Code::SearchUnavailable)
                    .text(crewd_collab::Locale::Zh);
            assert!(
                result["text"].as_str().unwrap().contains(&expected),
                "{result}"
            );
            assert!(
                !result["text"]
                    .as_str()
                    .unwrap()
                    .contains("organization_search_dirty")
            );
        }
        f.state
            .db
            .call_result(|store| {
                store.conn().execute_batch(
            "ALTER TABLE organization_search_dirty_fault_saved RENAME TO organization_search_dirty"
        )
            })
            .await
            .unwrap()
            .unwrap();
        health(&f, true).await;
        let page = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let page = human
                    .call(
                        "organization.search",
                        json!({"query":"maintenance-health-needle"}),
                    )
                    .await
                    .unwrap();
                if page["indexState"] == "ready" {
                    break page;
                }
                assert_eq!(page["indexState"], "building");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|hit| hit["target"] == target)
        );
        let result = worker
            .call(
                "agent.call",
                json!({"tool":"search","args":{"query":"maintenance-health-needle"}}),
            )
            .await
            .unwrap();
        assert_eq!(result["isError"], false, "{result}");
        let after = f.state.db.call_result(|store| {
            store.conn().query_row("SELECT (SELECT count(*) FROM messages),(SELECT count(*) FROM wakes),(SELECT count(*) FROM organization_work_events)", [], |row| Ok((row.get::<_,i64>(0)?, row.get::<_,i64>(1)?, row.get::<_,i64>(2)?)))
        }).await.unwrap().unwrap();
        assert_eq!(
            before, after,
            "maintenance, refusal and recovery cannot create business progress"
        );
        f.state
            .organization_search_runtime
            .get()
            .unwrap()
            .shutdown()
            .await;
        shutdown.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(15), server)
            .await
            .unwrap()
            .unwrap();
    }
}
