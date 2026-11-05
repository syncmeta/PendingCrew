//! Human transport for stable tables and the common graph. Attached sessions
//! use the separately authenticated Agent port, never this public RPC path.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_collab::{managed_links as links, managed_objects, managed_tables_v2 as tables};
use crewd_proto::{Request, Response, RpcError, managed_tables_v2::*};
use serde_json::Value;
use std::sync::Arc;

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, request: Request) -> Response {
    let id = request.id;
    match route(state, actor, request).await {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, error),
    }
}

/// Revalidate the captured connection/grant at send time. Hints contain only
/// an existing crew ID and do not disclose graph targets or hidden set sizes.
pub(crate) async fn event_allowed(state: &Arc<AppState>, actor: &Actor, data: &Value) -> bool {
    let Ok(hint) = serde_json::from_value::<Changed>(data.clone()) else {
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
            let conn = store.conn();
            actor.authorize_tx(&owner, conn, TABLE_GET).is_ok()
                && crewd_collab::crews::get(conn, &hint.crew_id).is_ok()
                && actor.authorize_tx(&owner, conn, TABLE_GET).is_ok()
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
        TABLE_SUBMIT => {
            let params: Submit = crewd_proto::parse_params(params)?;
            let receipt = state
                .collab(move |tx| {
                    actor.authorize_tx(&owner, tx.conn(), TABLE_SUBMIT)?;
                    let receipt = tables::submit(
                        tx,
                        managed_objects::Actor::Human(&principal),
                        params,
                        &links::DenyExternal,
                    )?;
                    actor.authorize_tx(&owner, tx.conn(), TABLE_SUBMIT)?;
                    Ok(receipt)
                })
                .await
                .map_err(dispatch::collab_err)?;
            value(receipt)
        }
        LINK_SUBMIT => {
            let params: ManualSubmit = crewd_proto::parse_params(params)?;
            let receipt = state
                .collab(move |tx| {
                    actor.authorize_tx(&owner, tx.conn(), LINK_SUBMIT)?;
                    let receipt = links::submit(
                        tx,
                        managed_objects::Actor::Human(&principal),
                        params,
                        &links::DenyExternal,
                    )?;
                    actor.authorize_tx(&owner, tx.conn(), LINK_SUBMIT)?;
                    Ok(receipt)
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
                let external = &links::DenyExternal;
                let result = match method.as_str() {
                    TABLE_GET => {
                        let params: TableGet = crewd_proto::parse_params(params)?;
                        value(
                            tables::get(conn, by, &params.id, external)
                                .map_err(dispatch::collab_err)?,
                        )
                    }
                    TABLE_LIST => {
                        let params: TableList = crewd_proto::parse_params(params)?;
                        value(
                            tables::tables(conn, by, params.after.as_deref(), external)
                                .map_err(dispatch::collab_err)?,
                        )
                    }
                    ROW_GET => {
                        let params: TableGet = crewd_proto::parse_params(params)?;
                        value(
                            tables::row(conn, by, &params.id, external)
                                .map_err(dispatch::collab_err)?,
                        )
                    }
                    ROW_LIST => {
                        let params: RowList = crewd_proto::parse_params(params)?;
                        value(
                            tables::rows(
                                conn,
                                by,
                                &params.table_id,
                                params.after.as_deref(),
                                external,
                            )
                            .map_err(dispatch::collab_err)?,
                        )
                    }
                    TABLE_REQUEST_GET => {
                        let params: RequestGet = crewd_proto::parse_params(params)?;
                        value(
                            tables::lookup(
                                conn,
                                by,
                                &params.scope,
                                &params.crew_id,
                                &params.request_id,
                            )
                            .map_err(dispatch::collab_err)?,
                        )
                    }
                    MANAGED_LINK_RESOLVE_TARGET => {
                        let params: ResolveTarget = crewd_proto::parse_params(params)?;
                        value(
                            links::resolve(conn, by, &params.target, external)
                                .map_err(dispatch::collab_err)?,
                        )
                    }
                    LINK_LIST => {
                        let params: ListLinks = crewd_proto::parse_params(params)?;
                        value(
                            links::list(conn, by, params, external)
                                .map_err(dispatch::collab_err)?,
                        )
                    }
                    LINK_REQUEST_GET => {
                        let params: RequestGet = crewd_proto::parse_params(params)?;
                        value(
                            links::lookup(
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
                        "Unknown managed table/link method",
                    )),
                }?;
                actor
                    .authorize_tx(&owner, conn, &method)
                    .map_err(dispatch::collab_err)?;
                Ok(result)
            })
            .await
            .map_err(dispatch::internal)?,
    }
}
