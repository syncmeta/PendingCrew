//! Protected Human read of exact responsibility episodes.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_proto::{ChannelResponsibilityParams, Request, Response, RpcError, methods};
use std::sync::Arc;

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, req: Request) -> Response {
    let id = req.id;
    match read(state, actor, req).await {
        Ok(view) => Response::ok(id, view),
        Err(error) => Response::err(id, error),
    }
}

async fn read(
    state: &Arc<AppState>,
    actor: &Actor,
    req: Request,
) -> Result<serde_json::Value, RpcError> {
    actor
        .authorize(state, methods::WORK_CHANNEL_RESPONSIBILITY)
        .await?;
    let p: ChannelResponsibilityParams = crewd_proto::parse_params(req.params)?;
    let actor = actor.clone();
    let owner = state.clone();
    let result = state
        .db
        .call_result(move |store| {
            actor.authorize_tx(&owner, store.conn(), methods::WORK_CHANNEL_RESPONSIBILITY)?;
            crewd_collab::channel_responsibility::read(store.conn(), p)
        })
        .await
        .map_err(dispatch::internal)?
        .map_err(dispatch::collab_err)?;
    serde_json::to_value(result).map_err(dispatch::internal)
}
