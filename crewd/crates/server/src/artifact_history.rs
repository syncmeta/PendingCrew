//! Explicit Human maintenance. Reads and invalidations never advance a job.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_collab::{artifact_history as domain, managed_objects};
use crewd_proto::{
    Request, Response, RpcError, artifact_history::*, managed_objects_api::RequestParams,
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
    let Ok(principal) = actor.principal(state).await else {
        return false;
    };
    let actor = actor.clone();
    let owner = state.clone();
    state
        .db
        .call(move |store| {
            actor.authorize_tx(&owner, store.conn(), STATUS).is_ok()
                && domain::status(
                    store.conn(),
                    managed_objects::Actor::Human(&principal),
                    StatusParams {
                        crew_id: hint.crew_id,
                        job_id: Some(hint.job_id),
                    },
                )
                .is_ok()
        })
        .await
        .unwrap_or(false)
}

fn value<T: serde::Serialize>(result: T) -> Result<Value, RpcError> {
    serde_json::to_value(result).map_err(dispatch::internal)
}

async fn route(state: &Arc<AppState>, actor: &Actor, request: Request) -> Result<Value, RpcError> {
    let principal = actor.principal(state).await?;
    let actor = actor.clone();
    let owner = state.clone();
    let method = request.method;
    let params = request.params;
    match method.as_str() {
        BEGIN => {
            let params: BeginParams = crewd_proto::parse_params(params)?;
            let receipt = state
                .collab(move |tx| {
                    actor.authorize_tx(&owner, tx.conn(), BEGIN)?;
                    domain::begin(tx, managed_objects::Actor::Human(&principal), params)
                })
                .await
                .map_err(dispatch::collab_err)?;
            value(receipt)
        }
        STEP => {
            let params: StepParams = crewd_proto::parse_params(params)?;
            let receipt = state
                .collab(move |tx| {
                    actor.authorize_tx(&owner, tx.conn(), STEP)?;
                    domain::step(tx, managed_objects::Actor::Human(&principal), params)
                })
                .await
                .map_err(dispatch::collab_err)?;
            value(receipt)
        }
        _ => state
            .db
            .call(move |store| {
                let conn = store.conn();
                actor
                    .authorize_tx(&owner, conn, &method)
                    .map_err(dispatch::collab_err)?;
                let by = managed_objects::Actor::Human(&principal);
                match method.as_str() {
                    STATUS => {
                        let params: StatusParams = crewd_proto::parse_params(params)?;
                        value(domain::status(conn, by, params).map_err(dispatch::collab_err)?)
                    }
                    REQUEST_GET => {
                        let params: RequestParams = crewd_proto::parse_params(params)?;
                        value(
                            domain::lookup(
                                conn,
                                by,
                                &params.scope,
                                &params.crew_id,
                                &params.request_id,
                            )
                            .map_err(dispatch::collab_err)?,
                        )
                    }
                    _ => Err(RpcError::new(
                        RpcError::METHOD_NOT_FOUND,
                        "Unknown artifact history method",
                    )),
                }
            })
            .await
            .map_err(dispatch::internal)?,
    }
}
