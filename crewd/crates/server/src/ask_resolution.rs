//! Human Ask reads and legacy writes with the original live connection authority.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_collab::{Sender, asks, messages};
use crewd_proto::{
    AskAnswerParams, AskGetParams, AskListParams, AskWithdrawParams, Message, MessagePostParams,
    Request, Response, RpcError, methods,
};
use serde_json::Value;
use std::sync::Arc;

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, req: Request) -> Response {
    let id = req.id;
    match route(state, actor, req).await {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, error),
    }
}

async fn route(state: &Arc<AppState>, actor: &Actor, req: Request) -> Result<Value, RpcError> {
    actor.authorize(state, &req.method).await?;
    let actor = actor.clone();
    let owner = state.clone();
    match req.method.as_str() {
        methods::ASK_LIST => {
            let p: AskListParams = crewd_proto::parse_params_or_default(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    actor.authorize_tx(&owner, store.conn(), methods::ASK_LIST)?;
                    asks::inbox_waits(
                        store.conn(),
                        crewd_store::now_ms(),
                        p.home_crew_id.as_deref(),
                    )
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        methods::ASK_RESOLVE_CHECK => {
            let p: AskGetParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    actor.authorize_tx(&owner, store.conn(), methods::ASK_RESOLVE_CHECK)?;
                    crewd_collab::ask_resolution::check(
                        store.conn(),
                        &p.crew_id,
                        &p.ask,
                        crewd_store::now_ms(),
                    )
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        methods::ASK_ANSWER => {
            let p: AskAnswerParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .collab(move |tx| {
                    actor.authorize_tx(&owner, tx.conn(), methods::ASK_ANSWER)?;
                    asks::answer(tx, &p.crew_id, &p.ask, &p.text)
                })
                .await
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        methods::ASK_WITHDRAW => {
            let p: AskWithdrawParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .collab(move |tx| {
                    actor.authorize_tx(&owner, tx.conn(), methods::ASK_WITHDRAW)?;
                    asks::withdraw(tx, &Sender::Human, &p.crew_id, &p.ask, &p.reason)
                })
                .await
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        // Replying to an open Ask is itself a Human resolution.
        methods::MESSAGE_POST => {
            let p: MessagePostParams = crewd_proto::parse_params(req.params)?;
            let message = state
                .collab(move |tx| {
                    actor.authorize_tx(&owner, tx.conn(), methods::MESSAGE_POST)?;
                    post_human(tx, p)
                })
                .await
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(message).map_err(dispatch::internal)
        }
        _ => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            "Unknown Human Ask method",
        )),
    }
}

pub(crate) fn post_human(
    tx: &mut crewd_collab::Tx<'_>,
    p: MessagePostParams,
) -> crewd_collab::Result<Message> {
    messages::post(
        tx,
        Sender::Human,
        messages::NewPost {
            channel_id: p.channel_id,
            text: p.text,
            in_reply_to: p.in_reply_to,
            mentions: p.mentions.into_iter().map(Into::into).collect(),
            attachments: p.attachments.into_iter().map(Into::into).collect(),
            category: None,
        },
    )
    .map(|posted| posted.message)
}
