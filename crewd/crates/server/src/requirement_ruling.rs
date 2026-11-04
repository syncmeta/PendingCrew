//! Current Human connection authority for requirement recovery, review and legacy closure.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_collab::{Sender, requirement_ruling as facts};
use crewd_proto::{
    Request, RequirementRulingCheckParams, RequirementRulingParams, Response, RpcError,
    TodoCloseParams, TodoCreateLookupParams, methods,
};
use serde_json::Value;
use std::sync::Arc;

pub(crate) fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::TODO_CREATE_LOOKUP
            | methods::TODO_RULING_CHECK
            | methods::TODO_RULING_RESULT
            | methods::TODO_CLOSE
    )
}

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, req: Request) -> Response {
    let id = req.id;
    match route(state, actor, req).await {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, error),
    }
}

async fn route(state: &Arc<AppState>, actor: &Actor, req: Request) -> Result<Value, RpcError> {
    actor.authorize(state, &req.method).await?;
    let actor = actor.clone();
    let owner = state.clone();
    match req.method.as_str() {
        methods::TODO_CREATE_LOOKUP => {
            let p: TodoCreateLookupParams = crewd_proto::parse_params(req.params)?;
            let result = state
                .db
                .call_result(move |store| {
                    actor.authorize_tx(&owner, store.conn(), methods::TODO_CREATE_LOOKUP)?;
                    crewd_collab::requirements::create_lookup(store.conn(), &Sender::Human, p)
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(result).map_err(dispatch::internal)
        }
        methods::TODO_RULING_CHECK => {
            let p: RequirementRulingCheckParams = crewd_proto::parse_params(req.params)?;
            let result = state
                .db
                .call_result(move |store| {
                    actor.authorize_tx(&owner, store.conn(), methods::TODO_RULING_CHECK)?;
                    facts::check(store.conn(), &Sender::Human, &p)
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(result).map_err(dispatch::internal)
        }
        methods::TODO_RULING_RESULT => {
            let p: RequirementRulingParams = crewd_proto::parse_params(req.params)?;
            let result = state
                .db
                .call_result(move |store| {
                    actor.authorize_tx(&owner, store.conn(), methods::TODO_RULING_RESULT)?;
                    facts::result(store.conn(), &p.crew_id, &p.id)
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(result).map_err(dispatch::internal)
        }
        methods::TODO_CLOSE => {
            let p: TodoCloseParams = crewd_proto::parse_params(req.params)?;
            let result = state
                .collab(move |tx| {
                    actor.authorize_tx(&owner, tx.conn(), methods::TODO_CLOSE)?;
                    facts::close_legacy(tx, p)
                })
                .await
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(result).map_err(dispatch::internal)
        }
        _ => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            "Unknown protected requirement method",
        )),
    }
}
