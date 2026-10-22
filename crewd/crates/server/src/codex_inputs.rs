//! Captured Human source-bound native input staging. No provider or file service.
use crate::{AppState, dispatch, public_commands::Actor};
use base64::{Engine, engine::general_purpose::STANDARD};
use crewd_proto::{Request, Response, RpcError, codex_inputs::*, parse_params};
use serde_json::Value;
use std::sync::Arc;
pub(crate) fn is_method(method: &str) -> bool {
    matches!(method, INPUT_PREPARE | INPUT_LOOKUP | INPUT_DISCARD)
}
fn bad() -> RpcError {
    RpcError::new(
        RpcError::CONFLICT,
        "native input scope/content/original request unavailable",
    )
    .with_refusal("codex.input_unavailable")
}
async fn route(
    state: &Arc<AppState>,
    actor: &Actor,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    actor.authorize(state, method).await?;
    let principal = actor.principal(state).await?;
    let actor = actor.clone();
    let state2 = state.clone();
    let method = method.to_owned();
    // Decode before entering the serial DB owner; bound both encoded and decoded input.
    let prepared = if method == INPUT_PREPARE {
        let p = parse_params::<CodexInputPrepareParams>(params.clone())?;
        if p.bytes_base64.len() > 4 * MAX_MEDIA_BYTES.div_ceil(3) {
            return Err(bad());
        }
        let body = STANDARD.decode(&p.bytes_base64).map_err(|_| bad())?;
        if STANDARD.encode(&body) != p.bytes_base64 {
            return Err(bad());
        }
        Some((p, body))
    } else {
        None
    };
    state
        .db
        .call(move |store| {
            let operation = |c: &rusqlite::Connection| -> Result<Value, RpcError> {
                actor
                    .authorize_tx(&state2, c, &method)
                    .map_err(dispatch::collab_err)?;
                let owner = crewd_store::codex_inputs::Owner {
                    db_id: crewd_store::meta_get(c, "dbId")
                        .map_err(|_| bad())?
                        .ok_or_else(bad)?,
                    principal,
                };
                let value = match method.as_str() {
                    INPUT_PREPARE => {
                        let (p, body) = prepared.unwrap();
                        serde_json::to_value(
                            crewd_store::codex_inputs::prepare(
                                c,
                                &owner,
                                &p,
                                &body,
                                &crewd_store::now_ms().to_string(),
                            )
                            .map_err(|_| bad())?,
                        )
                    }
                    INPUT_LOOKUP => serde_json::to_value(
                        crewd_store::codex_inputs::lookup(c, &owner, &parse_params(params)?)
                            .map_err(|_| bad())?,
                    ),
                    INPUT_DISCARD => serde_json::to_value(
                        crewd_store::codex_inputs::discard(c, &owner, &parse_params(params)?)
                            .map_err(|_| bad())?,
                    ),
                    _ => return Err(bad()),
                };
                value.map_err(dispatch::internal)
            };
            if method == INPUT_LOOKUP {
                operation(store.conn())
            } else {
                // The original request receipt and immutable bytes share this transaction.
                store
                    .conn()
                    .execute_batch("BEGIN IMMEDIATE")
                    .map_err(|_| bad())?;
                match operation(store.conn()) {
                    Ok(v) => {
                        store.conn().execute_batch("COMMIT").map_err(|_| bad())?;
                        Ok(v)
                    }
                    Err(e) => {
                        let _ = store.conn().execute_batch("ROLLBACK");
                        Err(e)
                    }
                }
            }
        })
        .await
        .map_err(|_| bad())?
}
pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, req: Request) -> Response {
    let id = req.id;
    match route(state, actor, &req.method, req.params).await {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    }
}
