//! Passive Human inspection of durable administrative source records.
//! This surface does not admit sessions or infer execution from observation.
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
        LIST => {
            let p: AdminCaseListParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(&owner, store.conn(), LIST)?;
                    crewd_collab::admin_cases::list(store.conn(), &p)
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
                    crewd_collab::admin_cases::get(store.conn(), &p)
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
