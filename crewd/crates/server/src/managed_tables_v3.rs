//! Human V3 transport. The connection owns authority; parameters cannot create it.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_collab::{managed_links, managed_objects, managed_tables_v3 as tables};
use crewd_proto::{Request, Response, RpcError, managed_tables_v3::*};
use serde_json::Value;
use std::sync::Arc;

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, request: Request) -> Response {
    let id = request.id;
    match route(state, actor, request).await {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, error),
    }
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
    if method == SUBMIT {
        let params: Submit = crewd_proto::parse_params(params)?;
        let receipt = state
            .collab(move |tx| {
                actor.authorize_tx(&owner, tx.conn(), SUBMIT)?;
                let receipt = tables::submit(
                    tx,
                    managed_objects::Actor::Human(&principal),
                    params,
                    &managed_links::DenyExternal,
                )?;
                actor.authorize_tx(&owner, tx.conn(), SUBMIT)?;
                Ok(receipt)
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
            let by = managed_objects::Actor::Human(&principal);
            let external = &managed_links::DenyExternal;
            let result = match method.as_str() {
                TABLE_GET => {
                    let params: Get = crewd_proto::parse_params(params)?;
                    value(
                        tables::get(conn, by, &params.id, external)
                            .map_err(dispatch::collab_err)?,
                    )
                }
                TABLE_LIST => {
                    let params: TableList = crewd_proto::parse_params(params)?;
                    value(tables::tables(conn, by, params, external).map_err(dispatch::collab_err)?)
                }
                ROW_GET => {
                    let params: Get = crewd_proto::parse_params(params)?;
                    value(
                        tables::row(conn, by, &params.id, external)
                            .map_err(dispatch::collab_err)?,
                    )
                }
                ROW_LIST => {
                    let params: RowList = crewd_proto::parse_params(params)?;
                    value(tables::rows(conn, by, params, external).map_err(dispatch::collab_err)?)
                }
                ROW_LINKS => {
                    let params: RowLinks = crewd_proto::parse_params(params)?;
                    value(
                        tables::row_links(conn, by, params, external)
                            .map_err(dispatch::collab_err)?,
                    )
                }
                DEFINE_CHECK => {
                    let params: DefineCheck = crewd_proto::parse_params(params)?;
                    value(tables::define_check(conn, by, params).map_err(dispatch::collab_err)?)
                }
                REQUEST_GET => {
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
                HISTORY => {
                    let params: History = crewd_proto::parse_params(params)?;
                    value(
                        tables::history(conn, by, params, external)
                            .map_err(dispatch::collab_err)?,
                    )
                }
                HISTORY_VALUE => {
                    let params: HistoryValue = crewd_proto::parse_params(params)?;
                    value(
                        tables::history_value(conn, by, params, external)
                            .map_err(dispatch::collab_err)?,
                    )
                }
                _ => Err(RpcError::new(
                    RpcError::METHOD_NOT_FOUND,
                    "Unknown managed table method",
                )),
            }?;
            actor
                .authorize_tx(&owner, conn, &method)
                .map_err(dispatch::collab_err)?;
            Ok(result)
        })
        .await
        .map_err(dispatch::internal)?
}
