//! Verified archive is a deliberate local Human action. Read checks retain the read gate.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_proto::{Request, Response, RpcError, channel_archive::*};
use std::sync::Arc;
fn anchor(actor: &Actor) -> Result<(), RpcError> {
    match actor {
        Actor::Local(p) if p.is_human_anchor() && actor.connection_scope().is_live() => {
            p.command_principal()?;
            Ok(())
        }
        _ => Err(RpcError::new(
            RpcError::FORBIDDEN,
            "Only the local Human may verify channel archive",
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
    if req.method == CHECK {
        actor.authorize(state, CHECK).await?;
        let p: ChannelArchiveCheckParams = crewd_proto::parse_params(req.params)?;
        let r = state
            .db
            .call_result(move |s| {
                by.authorize_tx(&owner, s.conn(), CHECK)?;
                crewd_collab::channel_archive::check(
                    s.conn(),
                    &crewd_collab::Sender::Human,
                    p,
                    crewd_store::now_ms(),
                )
            })
            .await
            .map_err(dispatch::internal)?
            .map_err(dispatch::collab_err)?;
        return serde_json::to_value(r).map_err(dispatch::internal);
    }
    anchor(actor)?;
    actor.authorize(state, "local.clientTrust").await?;
    let archive = match req.method.as_str() {
        ARCHIVE => Some(crewd_proto::parse_params::<ChannelArchiveVerifiedParams>(
            req.params.clone(),
        )?),
        RESTORE => None,
        _ => {
            return Err(RpcError::new(
                RpcError::METHOD_NOT_FOUND,
                "Unknown channel archive method",
            ));
        }
    };
    let restore = if archive.is_none() {
        Some(crewd_proto::parse_params::<ChannelRestoreVerifiedParams>(
            req.params,
        )?)
    } else {
        None
    };
    let r = state
        .collab(move |tx| {
            anchor(&by).map_err(|_| {
                crewd_collab::CollabError::Forbidden(crewd_collab::refusal::Refusal::new(
                    crewd_collab::refusal::Code::ResourceForbidden,
                ))
            })?;
            by.authorize_tx(&owner, tx.conn(), "local.clientTrust")?;
            match archive {
                Some(p) => {
                    crewd_collab::channel_archive::archive(tx, &crewd_collab::Sender::Human, p)
                }
                None => crewd_collab::channel_archive::restore(
                    tx,
                    &crewd_collab::Sender::Human,
                    restore.expect("parsed restore"),
                ),
            }
        })
        .await
        .map_err(dispatch::collab_err)?;
    serde_json::to_value(r).map_err(dispatch::internal)
}
