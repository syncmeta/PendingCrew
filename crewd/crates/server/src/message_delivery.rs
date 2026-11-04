//! Human-only, exact-scope, read-only delivery evidence.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_proto::{
    Request, Response, RpcError,
    message_delivery::{EXPLAIN, MessageDeliveryParams},
};
use std::sync::Arc;

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, request: Request) -> Response {
    let id = request.id;
    match read(state, actor, request).await {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, error),
    }
}

async fn read(
    state: &Arc<AppState>,
    actor: &Actor,
    request: Request,
) -> Result<serde_json::Value, RpcError> {
    actor.authorize(state, EXPLAIN).await?;
    let p: MessageDeliveryParams = crewd_proto::parse_params(request.params)?;
    let owner = state.clone();
    let actor = actor.clone();
    let view = state
        .db
        .call_result(move |store| {
            actor.authorize_tx(&owner, store.conn(), EXPLAIN)?;
            crewd_collab::message_delivery::read(store.conn(), &crewd_collab::Sender::Human, p)
        })
        .await
        .map_err(dispatch::internal)?
        .map_err(dispatch::collab_err)?;
    serde_json::to_value(view).map_err(dispatch::internal)
}
