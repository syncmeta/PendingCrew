//! Human transport for the typed organization directory. Attached sessions
//! enter through the separately authenticated Agent tools.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_collab::managed_objects as domain;
use crewd_proto::{Request, Response, RpcError, managed_objects_api::*};
use serde_json::Value;
use std::sync::Arc;

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, request: Request) -> Response {
    let id = request.id;
    match route(state, actor, request).await {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, error),
    }
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
                SCOPE => {
                    crewd_proto::parse_no_params(params)?;
                    value(domain::scope(conn).map_err(dispatch::collab_err)?)
                }
                GET => {
                    let params: GetParams = crewd_proto::parse_params(params)?;
                    value(domain::get(conn, human, &params.id).map_err(dispatch::collab_err)?)
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
                        domain::decisions(conn, human, params.after.as_deref())
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
