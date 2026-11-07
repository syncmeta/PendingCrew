//! Captured Human read path; no request can manufacture model or business actions.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_proto::{Request, Response, RpcError, admin_ask_groups::*};
use rusqlite::Connection;
use std::sync::Arc;

pub(crate) fn runtime_healthy(state: &AppState) -> bool {
    state
        .judgment_runtime
        .get()
        .is_some_and(|r| r.ask_groups_healthy())
}

/// Inside the matching authenticated FIFO frame; no protocol-only capability.
pub(crate) fn enabled(state: &AppState, c: &Connection) -> crewd_collab::Result<bool> {
    Ok(runtime_healthy(state)
        && crewd_store::admin_ask_groups::available(c)?
        && runtime_healthy(state))
}
/// Root hello/paired routing must additionally retain current Human authorization.
pub(crate) async fn qualified(state: &AppState) -> crewd_collab::Result<bool> {
    let runtime = state.judgment_runtime.get().cloned();
    state
        .db
        .call_result(move |s| {
            let healthy = || runtime.as_ref().is_some_and(|r| r.ask_groups_healthy());
            Ok::<bool, crewd_collab::CollabError>(
                healthy() && crewd_store::admin_ask_groups::available(s.conn())? && healthy(),
            )
        })
        .await
        .map_err(crewd_collab::CollabError::from)?
}

/// A committed hint has no H identity/body. Both queueing and final socket send
/// must use this current authorization check, never a subscription as a grant.
pub(crate) async fn event_allowed(
    state: &Arc<AppState>,
    actor: &Actor,
    event: &str,
    data: &serde_json::Value,
) -> bool {
    let crew = if event == INVALIDATED {
        if serde_json::from_value::<Invalidated>(data.clone()).is_err() {
            return false;
        }
        None
    } else if event == CHANGED {
        let Ok(hint) = serde_json::from_value::<Changed>(data.clone()) else {
            return false;
        };
        if hint.crew_id.is_empty()
            || hint.crew_id.len() > 512
            || hint.crew_id.chars().any(char::is_control)
        {
            return false;
        }
        Some(hint.crew_id)
    } else {
        return false;
    };
    if actor.authorize(state, LIST).await.is_err() {
        return false;
    }
    let by = actor.clone();
    let owner = state.clone();
    state
        .db
        .call(move |s| {
            let c = s.conn();
            if by.authorize_tx(&owner, c, LIST).is_err()
                || !crewd_store::admin_ask_groups::available(c).unwrap_or(false)
            {
                return false;
            }
            let readable = match crew {
                // Revocation of a displayed snapshot must survive a failed
                // model runner. This empty hint reads no H identities/content.
                None => true,
                Some(crew_id) => {
                    enabled(&owner, c).unwrap_or(false)
                        && crewd_collab::admin_ask_groups::list(
                            c,
                            &crewd_collab::Sender::Human,
                            List {
                                crew_id,
                                after: None,
                                limit: Some(1),
                            },
                            crewd_store::now_ms(),
                        )
                        .is_ok()
                }
            };
            readable && by.authorize_tx(&owner, c, LIST).is_ok()
        })
        .await
        .unwrap_or(false)
}

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, req: Request) -> Response {
    let id = req.id;
    let result = async {
        if req.method != LIST {
            return Err(RpcError::new(
                RpcError::METHOD_NOT_FOUND,
                "unknown pending-question grouping method",
            ));
        }
        actor.authorize(state, LIST).await?;
        let p: List = crewd_proto::parse_params(req.params)?;
        let owner = state.clone();
        let by = actor.clone();
        let mut page = state
            .db
            .call_result(move |store| {
                by.authorize_tx(&owner, store.conn(), LIST)?;
                if !crewd_store::admin_ask_groups::available(store.conn())? {
                    return Err(crewd_store::StoreError::Invalid(
                        "pending-question grouping storage is unavailable".into(),
                    )
                    .into());
                }
                let value = crewd_collab::admin_ask_groups::list(
                    store.conn(),
                    &crewd_collab::Sender::Human,
                    p,
                    crewd_store::now_ms(),
                )?;
                by.authorize_tx(&owner, store.conn(), LIST)?;
                Ok::<_, crewd_collab::CollabError>(value)
            })
            .await
            .map_err(dispatch::internal)?
            .map_err(dispatch::collab_err)?;
        actor.authorize(state, LIST).await?;
        if !runtime_healthy(state) {
            page.items.clear();
            page.next_after = None;
            page.availability = Availability::Unconfigured;
        }
        serde_json::to_value(page).map_err(dispatch::internal)
    }
    .await;
    match result {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    }
}
