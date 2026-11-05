//! Human transport for the typed organization directory. Attached sessions
//! enter through the separately authenticated Agent tools.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_collab::managed_objects as domain;
use crewd_proto::{
    Request, Response, RpcError, decision_asks, managed_message_sources as sources,
    managed_objects_api::*,
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

/// Check the whole invalidation against the same current read authority as
/// managedObject.get. A purged or mismatched object suppresses the entire hint.
pub(crate) async fn event_allowed(state: &Arc<AppState>, actor: &Actor, data: &Value) -> bool {
    let Ok(hint) = serde_json::from_value::<OntologyChanged>(data.clone()) else {
        return false;
    };
    match hint.object_ids.as_slice() {
        [_] => {}
        [first, second] if first != second => {}
        _ => return false,
    }
    let Ok(principal) = actor.principal(state).await else {
        return false;
    };
    let actor = actor.clone();
    let owner = state.clone();
    state
        .db
        .call(move |store| {
            let conn = store.conn();
            actor.authorize_tx(&owner, conn, GET).is_ok()
                && hint.object_ids.iter().all(|id| {
                    domain::get_with_defaults(conn, domain::Actor::Human(&principal), id, true)
                        .is_ok_and(|object| object.crew_id() == hint.crew_id)
                })
        })
        .await
        .unwrap_or(false)
}

fn value<T: serde::Serialize>(value: T) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(dispatch::internal)
}

async fn route(state: &Arc<AppState>, actor: &Actor, request: Request) -> Result<Value, RpcError> {
    let method = request.method;
    let principal = actor.principal(state).await?;
    let owner = state.clone();
    let actor = actor.clone();
    let params = request.params;
    if method == sources::RECORD {
        let request = crewd_proto::parse_params(params)?;
        let receipt = state
            .collab(move |tx| {
                actor.authorize_tx(&owner, tx.conn(), sources::RECORD)?;
                crewd_collab::managed_message_sources::record(
                    tx,
                    domain::Actor::Human(&principal),
                    request,
                )
            })
            .await
            .map_err(dispatch::collab_err)?;
        return value(receipt);
    }
    if method == SUBMIT {
        let request = crewd_proto::parse_params(params)?;
        let receipt = state
            .collab(move |tx| {
                actor.authorize_tx(&owner, tx.conn(), SUBMIT)?;
                domain::submit(tx, domain::Actor::Human(&principal), request)
            })
            .await
            .map_err(dispatch::collab_err)?;
        return value(receipt);
    }

    state
        .db
        .call(move |store| {
            let conn = store.conn();
            actor
                .authorize_tx(&owner, conn, &method)
                .map_err(dispatch::collab_err)?;
            let human = domain::Actor::Human(&principal);
            match method.as_str() {
                sources::GET => {
                    let params: sources::GetParams = crewd_proto::parse_params(params)?;
                    value(
                        crewd_collab::managed_message_sources::source(conn, human, &params.id)
                            .map_err(dispatch::collab_err)?,
                    )
                }
                sources::MESSAGE_OBJECTS => {
                    let params: sources::MessageObjectsParams = crewd_proto::parse_params(params)?;
                    value(
                        crewd_collab::managed_message_sources::objects_from_message(
                            conn,
                            human,
                            &params.message_id,
                            params.after.as_deref(),
                        )
                        .map_err(dispatch::collab_err)?,
                    )
                }
                sources::REQUEST_GET => {
                    let params: RequestParams = crewd_proto::parse_params(params)?;
                    value(
                        crewd_collab::managed_message_sources::lookup(
                            conn,
                            human,
                            &params.scope,
                            &params.crew_id,
                            &params.request_id,
                        )
                        .map_err(dispatch::collab_err)?,
                    )
                }
                SCOPE => {
                    crewd_proto::parse_no_params(params)?;
                    value(domain::scope(conn).map_err(dispatch::collab_err)?)
                }
                GET => {
                    let params: GetParams = crewd_proto::parse_params(params)?;
                    value(
                        domain::get_with_defaults(
                            conn,
                            human,
                            &params.id,
                            params.include_system_defaults,
                        )
                        .map_err(dispatch::collab_err)?,
                    )
                }
                TABLES => {
                    let params: TablesParams = crewd_proto::parse_params_or_default(params)?;
                    value(
                        domain::tables(conn, human, params.after.as_deref())
                            .map_err(dispatch::collab_err)?,
                    )
                }
                ROWS => {
                    let params: RowsParams = crewd_proto::parse_params(params)?;
                    value(
                        domain::rows(conn, human, &params.table_id, params.after.as_deref())
                            .map_err(dispatch::collab_err)?,
                    )
                }
                DECISIONS => {
                    let params: DecisionsParams = crewd_proto::parse_params_or_default(params)?;
                    value(
                        domain::decisions_with_defaults(
                            conn,
                            human,
                            params.after.as_deref(),
                            params.include_system_defaults,
                        )
                        .map_err(dispatch::collab_err)?,
                    )
                }
                decision_asks::GET => {
                    let params: decision_asks::GetParams = crewd_proto::parse_params(params)?;
                    value(
                        crewd_collab::decision_asks::get(conn, &params.crew_id, &params.ask)
                            .map_err(dispatch::collab_err)?,
                    )
                }
                decision_asks::DECISION_ORIGIN => {
                    let params: decision_asks::OriginParams = crewd_proto::parse_params(params)?;
                    value(
                        crewd_collab::decision_asks::origin(conn, &params.id)
                            .map_err(dispatch::collab_err)?,
                    )
                }
                RISKS => {
                    let params: TablesParams = crewd_proto::parse_params_or_default(params)?;
                    value(
                        domain::risks(conn, human, params.after.as_deref())
                            .map_err(dispatch::collab_err)?,
                    )
                }
                LESSONS => {
                    let params: TablesParams = crewd_proto::parse_params_or_default(params)?;
                    value(
                        domain::lessons(conn, human, params.after.as_deref())
                            .map_err(dispatch::collab_err)?,
                    )
                }
                REQUEST_GET => {
                    let params: RequestParams = crewd_proto::parse_params(params)?;
                    value(
                        domain::lookup(
                            conn,
                            human,
                            &params.scope,
                            &params.crew_id,
                            &params.request_id,
                        )
                        .map_err(dispatch::collab_err)?,
                    )
                }
                _ => Err(RpcError::new(
                    RpcError::METHOD_NOT_FOUND,
                    "Unknown object method",
                )),
            }
        })
        .await
        .map_err(dispatch::internal)?
}
