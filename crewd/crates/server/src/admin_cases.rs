//! Human inspection and explicit proposals for durable administrative custody.
//! Reads are passive; a proposal does not accept responsibility or admit sessions.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_proto::{Request, Response, RpcError, admin_cases::*};
use std::sync::Arc;

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, req: Request) -> Response {
    let id = req.id;
    match route(state, actor, req).await {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, error),
    }
}

async fn route(
    state: &Arc<AppState>,
    actor: &Actor,
    req: Request,
) -> Result<serde_json::Value, RpcError> {
    actor.authorize(state, &req.method).await?;
    let owner = state.clone();
    let by = actor.clone();
    match req.method.as_str() {
        crewd_proto::methods::ADMIN_CASE_CUSTODY_TRANSFER_GET => {
            let p: AdminCaseCustodyTransferGetParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(
                        &owner,
                        store.conn(),
                        crewd_proto::methods::ADMIN_CASE_CUSTODY_TRANSFER_GET,
                    )?;
                    crewd_collab::admin_case_custody::get_transfer(
                        store.conn(),
                        &crewd_collab::Sender::Human,
                        &p.request_id,
                        crewd_store::now_ms(),
                    )
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        crewd_proto::methods::ADMIN_CASE_CUSTODY_GET => {
            let p: AdminCaseCustodyGetParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(
                        &owner,
                        store.conn(),
                        crewd_proto::methods::ADMIN_CASE_CUSTODY_GET,
                    )?;
                    crewd_collab::admin_case_custody::get(
                        store.conn(),
                        &crewd_collab::Sender::Human,
                        &p.crew_id,
                        &p.case_id,
                        crewd_store::now_ms(),
                    )
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        crewd_proto::methods::ADMIN_CASE_CUSTODY_PROPOSE => {
            let p: AdminCaseCustodyPropose = crewd_proto::parse_params(req.params)?;
            let view = state
                .collab(move |tx| {
                    by.authorize_tx(
                        &owner,
                        tx.conn(),
                        crewd_proto::methods::ADMIN_CASE_CUSTODY_PROPOSE,
                    )?;
                    let captain = crewd_collab::crews::captain_of(tx.conn(), &p.crew_id)?
                        .ok_or_else(|| {
                            crewd_collab::resource_invalid("custody captain unavailable")
                        })?;
                    if !owner.admission.contains(&captain) {
                        return Err(crewd_collab::resource_invalid(
                            "custody captain admission required",
                        ));
                    }
                    crewd_collab::admin_case_custody::propose(tx, &crewd_collab::Sender::Human, p)
                })
                .await
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        LIST => {
            let p: AdminCaseListParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(&owner, store.conn(), LIST)?;
                    crewd_collab::admin_cases::list_at(store.conn(), &p, crewd_store::now_ms())
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        GET => {
            let p: AdminCaseGetParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(&owner, store.conn(), GET)?;
                    crewd_collab::admin_cases::get_at(store.conn(), &p, crewd_store::now_ms())
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        _ => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            "Unknown administrative record method",
        )),
    }
}
