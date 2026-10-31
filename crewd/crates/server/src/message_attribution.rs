//! Local Human mutation; remote reads retain the existing authenticated read gate.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_proto::{Request, Response, RpcError, message_attribution::*};
use std::sync::Arc;
fn anchor(actor: &Actor) -> Result<(), RpcError> {
    match actor {
        Actor::Local(p) if p.is_human_anchor() && actor.connection_scope().is_live() => {
            p.command_principal()?;
            Ok(())
        }
        _ => Err(RpcError::new(
            RpcError::FORBIDDEN,
            "Only the local Human may correct attribution",
        )),
    }
}
pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, req: Request) -> Response {
    let id = req.id;
    match route(state, actor, req).await {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    }
}
async fn route(
    state: &Arc<AppState>,
    actor: &Actor,
    req: Request,
) -> Result<serde_json::Value, RpcError> {
    let owner = state.clone();
    let by = actor.clone();
    match req.method.as_str() {
        GET => {
            actor.authorize(state, GET).await?;
            let p: MessageAttributionGet = crewd_proto::parse_params(req.params)?;
            let row = state
                .db
                .call_result(move |s| {
                    by.authorize_tx(&owner, s.conn(), GET)?;
                    crewd_collab::message_attribution::get(
                        s.conn(),
                        &crewd_collab::Sender::Human,
                        p,
                    )
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(row).map_err(dispatch::internal)
        }
        SET => {
            anchor(actor)?;
            actor.authorize(state, "local.clientTrust").await?;
            let p: MessageAttributionSet = crewd_proto::parse_params(req.params)?;
            let row = state
                .collab(move |tx| {
                    anchor(&by).map_err(|_| {
                        crewd_collab::CollabError::Forbidden(crewd_collab::refusal::Refusal::new(
                            crewd_collab::refusal::Code::ResourceForbidden,
                        ))
                    })?;
                    by.authorize_tx(&owner, tx.conn(), "local.clientTrust")?;
                    crewd_collab::message_attribution::set(tx, &crewd_collab::Sender::Human, p)
                })
                .await
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(row).map_err(dispatch::internal)
        }
        _ => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            "Unknown attribution method",
        )),
    }
}
