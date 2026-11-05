//! Current Human authority for crew removal, unarchive and permanent purge.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_collab::{self as collab, crews};
use crewd_proto::{CrewIdParams, Empty, Request, Response, RpcError, methods};
use crewd_store::knowledge_purge;
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) fn is_method(method: &str) -> bool {
    matches!(
        method,
        methods::CREW_REMOVE | methods::CREW_RESTORE | methods::CREW_PURGE
    )
}

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, request: Request) -> Response {
    let id = request.id;
    match route(state, actor, request).await {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, error),
    }
}

async fn route(state: &Arc<AppState>, actor: &Actor, request: Request) -> Result<Value, RpcError> {
    let params: CrewIdParams = crewd_proto::parse_params(request.params)?;
    let method = request.method;
    actor.authorize(state, &method).await?;
    let owner = state.clone();
    let actor = actor.clone();
    if method != methods::CREW_PURGE {
        let crew = state
            .collab(move |tx| {
                actor.authorize_tx(&owner, tx.conn(), &method)?;
                if method == methods::CREW_REMOVE {
                    crews::archive(tx, &params.crew_id)
                } else {
                    crews::unarchive(tx, &params.crew_id)
                }
            })
            .await
            .map_err(dispatch::collab_err)?;
        return serde_json::to_value(crew).map_err(dispatch::internal);
    }

    let knowledge = state
        .data_runtime
        .get()
        .map(|runtime| runtime.knowledge_root());
    let context = collab::TxContext::new(state.causal_fence.clone());
    state
        .db
        .call_result(move |store| {
            let id = &params.crew_id;
            actor.authorize_tx(&owner, store.conn(), methods::CREW_PURGE)?;
            crews::check_purgeable(store.conn(), id)?;
            let data = match (knowledge.as_ref(), store.backups_dir()) {
                (Some(root), _) => Some(
                    root.parent()
                        .ok_or_else(|| {
                            crewd_store::StoreError::Invalid(
                                "knowledge purge ownership unavailable".into(),
                            )
                        })?
                        .to_path_buf(),
                ),
                (None, Some(backups)) => Some(
                    backups
                        .parent()
                        .ok_or_else(|| {
                            crewd_store::StoreError::Invalid(
                                "backup data directory unavailable".into(),
                            )
                        })?
                        .canonicalize()
                        .map_err(crewd_store::StoreError::from)?,
                ),
                (None, None) => None,
            };
            let prepared = if let Some(data) = data.as_deref() {
                // Settle a previous interrupted operation before a new destructive cut.
                knowledge_purge::recover(store, data)?;
                match knowledge_purge::prepare(store, data, id) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        knowledge_purge::recover(store, data)?;
                        return Err(error.into());
                    }
                }
            } else {
                None
            };

            if prepared.is_none() {
                // Preserve the ordinary Core-only safety snapshot and raw API for
                // an exact uninitialized namespace.
                store.snapshot_before_purge(id)?;
            }
            let mutation = (|| {
                if let Some(prepared) = &prepared {
                    prepared.quarantine()?;
                }
                collab::run_with_context(store, crewd_store::now_ms(), context, |tx| {
                    actor.authorize_tx(&owner, tx.conn(), methods::CREW_PURGE)?;
                    crews::purge(tx, id)?;
                    if let Some(prepared) = &prepared {
                        prepared.record_commit(tx.conn(), tx.now_ms())?;
                    }
                    Ok(())
                })
            })();
            let settlement = prepared
                .as_ref()
                .map(|prepared| prepared.settle(store.conn()))
                .transpose();
            // The Core mutation is already visible if filesystem settlement
            // fails. Notify live clients of that known commit even when the
            // caller still needs to reconcile the cleanup result.
            if let Ok(((), changes)) = &mutation
                && !matches!(&settlement, Ok(Some(false)))
            {
                owner.hub.emit(store.conn(), changes);
            }
            if let Some(committed) = settlement? {
                if committed != mutation.is_ok() {
                    // An ambiguous SQLite return cannot justify a second purge.
                    return Err(crewd_store::StoreError::Invalid(
                        "original purge outcome requires reconciliation".into(),
                    )
                    .into());
                }
            }
            mutation?;
            Ok::<_, collab::CollabError>(())
        })
        .await
        .map_err(dispatch::internal)?
        .map_err(dispatch::collab_err)?;
    Ok(json!(Empty {}))
}

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use super::*;
    use crate::agent::testkit::{Fixture, RpcClient};
    use std::time::Duration;

    #[tokio::test]
    async fn core_only_fixture_retains_purge_and_refuses_unsafe_unqualified_knowledge_root() {
        let f = Fixture::new().await;
        assert!(f.state.data_runtime.get().is_none());
        let first = f.add_crew("Core-only lifecycle", None).await;
        let second = f.add_crew("Archived original stays", None).await;
        // Older internal fixtures seed archived data through the trusted
        // in-process adapter, independently of the public socket route.
        f.rpc("crew.remove", json!({"crewId":first})).await;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let task = f.spawn_owned_socket(stopping).await;
        let mut human = RpcClient::connect(&f.data_dir().join("crewd.sock")).await;
        human
            .call("crew.remove", json!({"crewId":second}))
            .await
            .unwrap();
        assert_eq!(
            human
                .call("crew.purge", json!({"crewId":first}))
                .await
                .unwrap(),
            json!({})
        );
        assert!(
            std::fs::read_dir(f.data_dir().join("backups"))
                .unwrap()
                .any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with("crewd-pre-delete-")
                })
        );

        let unrelated = tempfile::tempdir().unwrap();
        std::fs::write(unrelated.path().join("keep"), b"unrelated").unwrap();
        let symlink = f.data_dir().join("knowledge");
        std::os::unix::fs::symlink(unrelated.path(), &symlink).unwrap();
        assert!(
            human
                .call("crew.purge", json!({"crewId":second}))
                .await
                .is_err()
        );
        assert!(std::fs::symlink_metadata(&symlink).unwrap().is_symlink());
        assert_eq!(
            std::fs::read(unrelated.path().join("keep")).unwrap(),
            b"unrelated"
        );
        assert!(
            f.state
                .db
                .call(move |s| {
                    crews::get(s.conn(), &second).is_ok_and(|crew| crew.removed_at.is_some())
                })
                .await
                .unwrap()
        );
        drop(human);
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
    }
}
