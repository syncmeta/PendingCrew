//! Human transport for organization-readable unverified artifact pointers.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_collab::{managed_artifacts as domain, managed_objects};
use crewd_proto::{
    Request, Response, RpcError, managed_artifacts::*, managed_objects_api::RequestParams,
};
use serde_json::Value;
use std::sync::Arc;

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, request: Request) -> Response {
    let id = request.id;
    match route(state, actor, request).await {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, error),
    }
}

pub(crate) async fn event_allowed(state: &Arc<AppState>, actor: &Actor, value: &Value) -> bool {
    let Ok(hint) = serde_json::from_value::<Changed>(value.clone()) else {
        return false;
    };
    if actor.principal(state).await.is_err() {
        return false;
    }
    let actor = actor.clone();
    let owner = state.clone();
    state
        .db
        .call(move |store| {
            actor.authorize_tx(&owner, store.conn(), GET).is_ok()
                && crewd_collab::crews::get(store.conn(), &hint.crew_id).is_ok()
        })
        .await
        .unwrap_or(false)
}

fn value<T: serde::Serialize>(result: T) -> Result<Value, RpcError> {
    serde_json::to_value(result).map_err(dispatch::internal)
}

async fn route(state: &Arc<AppState>, actor: &Actor, request: Request) -> Result<Value, RpcError> {
    let method = request.method;
    let principal = actor.principal(state).await?;
    let actor = actor.clone();
    let owner = state.clone();
    let params = request.params;
    if method == SUBMIT {
        let request = crewd_proto::parse_params(params)?;
        let receipt = state
            .collab(move |tx| {
                actor.authorize_tx(&owner, tx.conn(), SUBMIT)?;
                domain::submit(tx, managed_objects::Actor::Human(&principal), request)
            })
            .await
            .map_err(dispatch::collab_err)?;
        return value(receipt);
    }
    state
        .db
        .call(move |store| {
            let c = store.conn();
            actor
                .authorize_tx(&owner, c, &method)
                .map_err(dispatch::collab_err)?;
            let by = managed_objects::Actor::Human(&principal);
            match method.as_str() {
                PREVIEW => {
                    let p: PreviewParams = crewd_proto::parse_params(params)?;
                    value(
                        domain::preview(c, by, &p.crew_id, &p.input)
                            .map_err(dispatch::collab_err)?,
                    )
                }
                LIST => {
                    let p: ListParams = crewd_proto::parse_params_or_default(params)?;
                    value(domain::list(c, by, p.after.as_ref()).map_err(dispatch::collab_err)?)
                }
                GET => {
                    let p: GetParams = crewd_proto::parse_params(params)?;
                    value(domain::get(c, by, &p.reference).map_err(dispatch::collab_err)?)
                }
                BACKLINKS => {
                    let p: BacklinksParams = crewd_proto::parse_params(params)?;
                    value(
                        domain::backlinks_with_legacy(
                            c,
                            by,
                            &p.reference,
                            p.after.as_deref(),
                            p.include_legacy_sources,
                        )
                        .map_err(dispatch::collab_err)?,
                    )
                }
                SUBJECT => {
                    let p: SubjectParams = crewd_proto::parse_params(params)?;
                    value(
                        domain::subject_with_legacy(
                            c,
                            by,
                            &p.subject,
                            p.after.as_deref(),
                            p.include_legacy_sources,
                        )
                        .map_err(dispatch::collab_err)?,
                    )
                }
                REQUEST_GET => {
                    let p: RequestParams = crewd_proto::parse_params(params)?;
                    value(
                        domain::lookup(c, by, &p.scope, &p.crew_id, &p.request_id)
                            .map_err(dispatch::collab_err)?,
                    )
                }
                _ => Err(RpcError::new(
                    RpcError::METHOD_NOT_FOUND,
                    "Unknown artifact method",
                )),
            }
        })
        .await
        .map_err(dispatch::internal)?
}
