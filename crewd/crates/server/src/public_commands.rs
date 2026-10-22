//! Public receipt submission uses captured kernel/paired authority, never a JSON
//! principal. This module's runtime capability is gated until the full surface
//! is connected; SQL and external execution settlement remain distinct.
use crate::{AppState, dispatch};
use crewd_collab::{self as collab, Sender, channels, crews, messages};
use crewd_proto::{RpcError, commands::*};
use crewd_store::command_receipts::{self as receipts, Reserved};
use serde_json::Value;
use std::sync::Arc;
#[derive(Clone)]
pub(crate) enum Actor {
    Local(crate::peer::ConnectionPeer),
    Remote(crate::remote::Principal),
}
impl Actor {
    pub(crate) fn connection_scope(&self) -> &crate::connection_scope::ConnectionScope {
        match self {
            Self::Local(peer) => &peer.connection_scope,
            Self::Remote(principal) => &principal.connection_scope,
        }
    }
    pub(crate) fn remote_grant(&self) -> Option<String> {
        match self {
            Self::Remote(p) => Some(p.binding.grant_id.clone()),
            Self::Local(_) => None,
        }
    }
    pub(crate) async fn principal(&self, state: &Arc<AppState>) -> Result<String, RpcError> {
        if !self.connection_scope().is_live() {
            return Err(RpcError::new(
                RpcError::FORBIDDEN,
                "original command connection closed",
            ));
        }
        match self {
            Self::Local(p) => p.command_principal(),
            Self::Remote(p) => {
                p.check_live(state).await?;
                Ok(format!(
                    "remote:{}:{}",
                    p.binding.authority_id, p.binding.grant_id
                ))
            }
        }
    }
    pub(crate) fn authorize_tx(
        &self,
        state: &Arc<AppState>,
        conn: &rusqlite::Connection,
        operation: &str,
    ) -> collab::Result<()> {
        if !self.connection_scope().is_live() {
            return Err(collab::CollabError::Forbidden(
                collab::refusal::Refusal::new(collab::refusal::Code::ResourceForbidden),
            ));
        }
        let allowed = match self {
            Self::Local(p) => p.command_principal().and_then(|_| {
                if matches!(operation, "local.clientTrust" | "local.clientTrustRevoke")
                    && !p.is_human_anchor()
                {
                    return Err(RpcError::new(
                        RpcError::FORBIDDEN,
                        "old trusted Human anchor required for local trust changes",
                    ));
                }
                if operation == "crewd.hibernate" && !p.owns_daemon_child() {
                    Err(RpcError::new(
                        RpcError::FORBIDDEN,
                        "owned daemon IPC association is unconfirmed",
                    )
                    .with_refusal("command.owner_unconfirmed"))
                } else {
                    Ok(())
                }
            }),
            Self::Remote(p) => p.authorize_command_tx(state, conn, operation),
        };
        allowed.map_err(|_| {
            collab::CollabError::Forbidden(collab::refusal::Refusal::new(
                collab::refusal::Code::ResourceForbidden,
            ))
        })
    }
    pub(crate) async fn authorize(
        &self,
        state: &Arc<AppState>,
        operation: &str,
    ) -> Result<(), RpcError> {
        if !self.connection_scope().is_live() {
            return Err(RpcError::new(
                RpcError::FORBIDDEN,
                "original command connection closed",
            ));
        }
        match self {
            Self::Local(p) => {
                p.command_principal()?;
                if matches!(operation, "local.clientTrust" | "local.clientTrustRevoke")
                    && !p.is_human_anchor()
                {
                    return Err(RpcError::new(
                        RpcError::FORBIDDEN,
                        "old trusted Human anchor required for local trust changes",
                    ));
                }
                if operation == "crewd.hibernate" && !p.owns_daemon_child() {
                    return Err(RpcError::new(
                        RpcError::FORBIDDEN,
                        "owned daemon IPC association is unconfirmed",
                    )
                    .with_refusal("command.owner_unconfirmed"));
                }
                Ok(())
            }
            Self::Remote(p) => p.authorize(state, operation).await,
        }
    }
}
pub(crate) fn is_method(method: &str) -> bool {
    matches!(
        method,
        LOCAL_CLIENT_TRUST_STATUS
            | COMMAND_SUBMIT
            | COMMAND_LOOKUP
            | COMMAND_LIST_UNKNOWN
            | SESSION_CONTROL_TARGET
            | SESSION_TERMINAL_INSPECT
    )
}
/// Only the connection layer may construct Actor. Dispatch through a JSON-only
/// route cannot manufacture Human authority or a paired principal.
pub(crate) async fn handle(
    state: &Arc<AppState>,
    actor: &Actor,
    req: crewd_proto::Request,
) -> crewd_proto::Response {
    let id = req.id;
    let result = match req.method.as_str() {
        LOCAL_CLIENT_TRUST_STATUS => {
            match serde_json::from_value::<LocalClientTrustStatusParams>(req.params) {
                Ok(_) => local_trust_status(state, actor).await,
                Err(_) => Err(malformed("local trust status requires an empty object")),
            }
        }
        SESSION_TERMINAL_INSPECT => match crewd_proto::parse_params(req.params) {
            Ok(p) => crate::public_terminal::inspect(state, actor, p).await,
            Err(e) => Err(e),
        },
        COMMAND_SUBMIT => match crewd_proto::parse_params(req.params) {
            Ok(p) => submit(state, actor, p).await,
            Err(e) => Err(e),
        },
        COMMAND_LOOKUP => match crewd_proto::parse_params(req.params) {
            Ok(p) => lookup(state, actor, p).await,
            Err(e) => Err(e),
        },
        SESSION_CONTROL_TARGET => {
            match crewd_proto::parse_params::<TerminalInspectParams>(req.params) {
                Ok(p) => match actor.authorize(state, "session.inspect").await {
                    Err(e) => Err(e),
                    Ok(()) => match state.orchestra.get() {
                        Some(core) => match core.public_target(&p.session_id).await {
                            Ok(target) => serde_json::to_value(target).map_err(dispatch::internal),
                            Err(e) => Err(crate::orchestra::rpc::rpc_err(e)),
                        },
                        None => Err(
                            RpcError::new(RpcError::CONFLICT, "orchestra is not running")
                                .with_refusal("orchestra.not_running"),
                        ),
                    },
                },
                Err(e) => Err(e),
            }
        }
        COMMAND_LIST_UNKNOWN => match crewd_proto::parse_params(req.params) {
            Ok(p) => list_unknown(state, actor, p).await,
            Err(e) => Err(e),
        },
        _ => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            "unknown public command method",
        )),
    };
    match result {
        Ok(value) => crewd_proto::Response::ok(id, value),
        Err(error) => crewd_proto::Response::err(id, error),
    }
}

async fn local_trust_status(state: &Arc<AppState>, actor: &Actor) -> Result<Value, RpcError> {
    let Actor::Local(peer) = actor else {
        return Err(RpcError::new(RpcError::FORBIDDEN, "local caller required"));
    };
    peer.command_principal()?;
    let peer = peer.clone();
    // Share the Core FIFO with pin grant/revocation. No receipt, JSON PID or
    // cached client-side consent can substitute for the current OS caller.
    let status = state
        .db
        .call(move |store| -> Result<LocalClientTrustStatus, RpcError> {
            let pins =
                crewd_store::local_clients::list(store.conn()).map_err(dispatch::internal)?;
            peer.local_trust_status(&pins)
        })
        .await
        .map_err(dispatch::internal)??;
    serde_json::to_value(status).map_err(dispatch::internal)
}

fn malformed(message: &str) -> RpcError {
    RpcError::new(RpcError::INVALID_PARAMS, message).with_refusal("command.invalid")
}
fn conflict() -> RpcError {
    RpcError::new(
        RpcError::CONFLICT,
        "请求身份或作用域已改变，请读取原记录后明确决定。",
    )
    .with_refusal("command.request_conflict")
}
pub(crate) fn reserve_error(e: crewd_store::StoreError, request: &str) -> collab::CollabError {
    if matches!(&e,crewd_store::StoreError::Invalid(s) if s=="public command immutable payload conflict")
    {
        collab::CollabError::Conflict(
            collab::refusal::Refusal::new(collab::refusal::Code::ResourceRequestConflict)
                .arg("request", request),
        )
    } else if matches!(&e, crewd_store::StoreError::Invalid(s) if s=="public command receipt capacity reached")
    {
        collab::CollabError::Conflict(collab::refusal::Refusal::new(
            collab::refusal::Code::CommandReceiptCapacity,
        ))
    } else {
        e.into()
    }
}
pub(crate) async fn validate_database(state: &Arc<AppState>, db: &str) -> Result<(), RpcError> {
    let actual = state
        .db
        .call(|s| crewd_store::meta_get(s.conn(), "dbId"))
        .await
        .map_err(dispatch::internal)?
        .map_err(dispatch::internal)?;
    if actual.as_deref() != Some(db) {
        return Err(conflict());
    }
    Ok(())
}
fn create(
    tx: &mut collab::Tx<'_>,
    p: crewd_proto::CrewCreateParams,
) -> collab::Result<crewd_proto::Crew> {
    crews::create(
        tx,
        crews::NewCrew {
            title: p.title,
            working_directory: p.working_directory,
            parent_id: p.parent_id,
            title_source: Some("human".into()),
        },
    )
}
fn sql_effect(
    tx: &mut collab::Tx<'_>,
    command: PublicCommand,
    actor: &Actor,
) -> collab::Result<Value> {
    let value = match command {
        PublicCommand::LocalClientTrust(p) => {
            let Actor::Local(peer) = actor else {
                return Err(crewd_store::StoreError::Invalid("local bootstrap only".into()).into());
            };
            let db = crewd_store::meta_get(tx.conn(), "dbId")?.ok_or_else(|| {
                crewd_store::StoreError::Invalid("db identity unavailable".into())
            })?;
            let trust = peer.grant_frontend(&p, &db, &tx.now_ts()).map_err(|_| {
                collab::CollabError::Forbidden(collab::refusal::Refusal::new(
                    collab::refusal::Code::ResourceForbidden,
                ))
            })?;
            let trust = crewd_store::local_clients::grant(tx.conn(), &trust)?;
            Ok(serde_json::json!({"trust":trust,"effect":"SQL admission projection only"}))
        }
        PublicCommand::LocalClientTrustRevoke(p) => {
            crewd_store::local_clients::revoke(tx.conn(), &p.trust_id)?;
            Ok(
                serde_json::json!({"trustId":p.trust_id,"revokedInStore":true,"physicalBarrier":"requires postcommit closure; never inferred from SQL"}),
            )
        }
        PublicCommand::CrewCreate(p) => serde_json::to_value(create(tx, p)?),
        PublicCommand::CrewCreateFirstMessage(p) => {
            let crew = create(tx, p.crew)?;
            let channel = if crews::is_home(tx.conn(), &crew.id)? {
                channels::ensure_human_channel(tx, &crew.id)?.id
            } else {
                crewd_store::channels::lobby_id(&crew.id)
            };
            let message = messages::post(
                tx,
                Sender::Human,
                messages::NewPost {
                    channel_id: channel,
                    text: p.text,
                    in_reply_to: None,
                    mentions: vec![],
                    attachments: vec![],
                    category: None,
                },
            )?;
            Ok(serde_json::json!({"crew":crew,"message":message.message}))
        }
        PublicCommand::MessagePost(p) => serde_json::to_value(
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
            )?
            .message,
        ),
        _ => {
            return Err(crewd_store::StoreError::Invalid(
                "external command requires production scheduler".into(),
            )
            .into());
        }
    };
    value.map_err(|_| {
        crewd_store::StoreError::Invalid("public SQL result serialization failed".into()).into()
    })
}
async fn submit(
    state: &Arc<AppState>,
    actor: &Actor,
    p: CommandSubmitParams,
) -> Result<Value, RpcError> {
    match &p.command {
        PublicCommand::LocalClientTrust(_)
        | PublicCommand::LocalClientTrustRevoke(_)
        | PublicCommand::CrewCreate(_)
        | PublicCommand::CrewCreateFirstMessage(_)
        | PublicCommand::MessagePost(_) => submit_sql(state, actor, p).await,
        PublicCommand::CodexConfigure(_)
        | PublicCommand::CodexCompact(_)
        | PublicCommand::CodexSteer(_)
        | PublicCommand::CodexQueue(_)
        | PublicCommand::CodexReview(_)
        | PublicCommand::CodexTitleSet(_)
        | PublicCommand::CodexFork(_)
        | PublicCommand::SessionAnswer(_)
        | PublicCommand::SessionRespondRunnerRequest(_)
        | PublicCommand::Hibernate(_)
        | PublicCommand::SessionStart(_)
        | PublicCommand::SessionStop(_)
        | PublicCommand::SessionResume(_)
        | PublicCommand::SessionInterrupt(_)
        | PublicCommand::TerminalClaim(_)
        | PublicCommand::TerminalRelease(_)
        | PublicCommand::TerminalWrite(_)
        | PublicCommand::TerminalResize(_) => {
            let call = crate::public_execution::PublicExecution::capture(state, actor, p).await?;
            state
                .orchestra
                .get()
                .ok_or_else(|| {
                    RpcError::new(RpcError::CONFLICT, "orchestra is not running")
                        .with_refusal("orchestra.not_running")
                })?
                .submit_public(call)
                .await
        }
    }
}

/// SQL receipts commit beside their exact mutations. Repeats return the same
/// receipt and do not call even the SQL author a second time.
pub(crate) async fn submit_sql(
    state: &Arc<AppState>,
    actor: &Actor,
    p: CommandSubmitParams,
) -> Result<Value, RpcError> {
    let principal = actor.principal(state).await?;
    for method in p.command.authorized_methods() {
        actor.authorize(state, method).await?;
    }
    if !crewd_proto::valid_remote_uuid(&p.request_id)
        || !crewd_proto::valid_remote_uuid(&p.runtime_binding_id)
    {
        return Err(malformed("请求编号无效。"));
    }
    let encoded = serde_json::to_vec(&p.command).map_err(dispatch::internal)?;
    if encoded.len() > MAX_COMMAND_PAYLOAD_BYTES {
        return Err(malformed("请求正文过大。"));
    }
    validate_database(state, &p.db_id).await?;
    if !matches!(
        p.command,
        PublicCommand::LocalClientTrust(_)
            | PublicCommand::LocalClientTrustRevoke(_)
            | PublicCommand::CrewCreate(_)
            | PublicCommand::CrewCreateFirstMessage(_)
            | PublicCommand::MessagePost(_)
    ) {
        return Err(RpcError::new(
            RpcError::INVALID_PARAMS,
            "执行命令尚未接入正式调度，请保留原请求。",
        )
        .with_refusal("command.unsupported"));
    }
    let own_state = state.clone();
    let own_actor = actor.clone();
    let mut receipt = state
        .collab(move |tx| {
            for method in p.command.authorized_methods() {
                own_actor.authorize_tx(&own_state, tx.conn(), method)?;
            }
            let existing = receipts::lookup(tx.conn(), &principal, &p.db_id, &p.request_id)?;
            if existing.is_none() && p.runtime_binding_id != own_state.runtime_binding_id {
                return Err(collab::CollabError::Conflict(
                    collab::refusal::Refusal::new(collab::refusal::Code::ResourceRequestConflict)
                        .arg("request", &p.request_id),
                ));
            }
            let reserved = receipts::reserve(tx.conn(), &principal, &p, &tx.now_ts())
                .map_err(|e| reserve_error(e, &p.request_id))?;
            match reserved {
                Reserved::Existing(r) => Ok(r),
                Reserved::New(_) => {
                    let result = sql_effect(tx, p.command.clone(), &own_actor)?;
                    Ok(receipts::transition(
                        tx.conn(),
                        &principal,
                        &p.db_id,
                        &p.request_id,
                        (CommandPhase::Reserved, CommandPhase::Committed),
                        Some(&result),
                        &tx.now_ts(),
                    )?)
                }
            }
        })
        .await
        .map_err(dispatch::collab_err)?;
    receipt.historical |= receipt.runtime_binding_id != state.runtime_binding_id;
    serde_json::to_value(receipt).map_err(dispatch::internal)
}
/// Read-only lookup never dispatches. The authenticated namespace is checked
/// before reading and original operation is re-authorized before disclosure.
pub(crate) async fn lookup(
    state: &Arc<AppState>,
    actor: &Actor,
    p: CommandLookupParams,
) -> Result<Value, RpcError> {
    let principal = actor.principal(state).await?;
    if !crewd_proto::valid_remote_uuid(&p.request_id) {
        return Err(malformed("请求编号无效。"));
    }
    validate_database(state, &p.db_id).await?;
    let own_state = state.clone();
    let own_actor = actor.clone();
    let mut result = state
        .db
        .call(move |s| -> collab::Result<_> {
            let result = receipts::lookup(s.conn(), &principal, &p.db_id, &p.request_id)?;
            if let Some(r) = &result {
                for method in r.command.authorized_methods() {
                    own_actor.authorize_tx(&own_state, s.conn(), method)?;
                }
            }
            Ok(result)
        })
        .await
        .map_err(dispatch::internal)?
        .map_err(dispatch::collab_err)?;
    if let Some(receipt) = &result {
        for method in receipt.command.authorized_methods() {
            actor.authorize(state, method).await?;
        }
    }
    if let Some(r) = &mut result {
        r.historical |= r.runtime_binding_id != state.runtime_binding_id;
    }
    serde_json::to_value(result).map_err(dispatch::internal)
}

pub(crate) async fn list_unknown(
    state: &Arc<AppState>,
    actor: &Actor,
    p: CommandListUnknownParams,
) -> Result<Value, RpcError> {
    let principal = actor.principal(state).await?;
    validate_database(state, &p.db_id).await?;
    let limit = p.limit.unwrap_or(50);
    if !(1..=50).contains(&limit)
        || p.after
            .as_ref()
            .is_some_and(|s| !crewd_proto::valid_remote_uuid(s))
    {
        return Err(malformed("未知请求页参数无效。"));
    }
    let own_state = state.clone();
    let own_actor = actor.clone();
    let mut records = state
        .db
        .call(move |s| -> collab::Result<_> {
            let rows =
                receipts::list_unknown(s.conn(), &principal, &p.db_id, p.after.as_deref(), limit)?;
            for r in &rows {
                for method in r.command.authorized_methods() {
                    own_actor.authorize_tx(&own_state, s.conn(), method)?;
                }
            }
            Ok(rows)
        })
        .await
        .map_err(dispatch::internal)?
        .map_err(dispatch::collab_err)?;
    let more = records.len() > usize::from(limit);
    records.truncate(usize::from(limit));
    let next_after = if more {
        records.last().map(|r| r.request_id.clone())
    } else {
        None
    };
    let items = records
        .into_iter()
        .map(|r| CommandSummary {
            historical: r.historical || r.runtime_binding_id != state.runtime_binding_id,
            runtime_binding_id: r.runtime_binding_id,
            request_id: r.request_id,
            db_id: r.db_id,
            operation: r.command.method().into(),
            payload_sha256: r.payload_sha256,
            phase: r.phase,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect();
    serde_json::to_value(CommandUnknownPage { items, next_after }).map_err(dispatch::internal)
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn state_actor() -> (Arc<AppState>, Actor, String) {
        let store = crewd_store::Store::open_in_memory().unwrap();
        let db = crewd_store::meta_get(store.conn(), "dbId")
            .unwrap()
            .unwrap();
        let state = Arc::new(AppState::new(crewd_store::Db::spawn(store).unwrap()));
        let policy = Arc::new(crate::peer::PeerPolicy::new(None).unwrap());
        let (server, _client) = tokio::net::UnixStream::pair().unwrap();
        let peer = policy.accept(&server).unwrap();
        assert!(matches!(peer.authority(), crate::peer::Authority::Human));
        (state, Actor::Local(peer), db)
    }
    #[tokio::test]
    async fn local_trust_status_is_readonly_and_bound_to_current_caller() {
        let (state, actor, _) = state_actor().await;
        let read = |params| crewd_proto::Request {
            id: 1,
            method: "local.clientTrust.status".into(),
            params,
        };
        let status = handle(&state, &actor, read(serde_json::json!({})))
            .await
            .into_result()
            .unwrap();
        assert_eq!(
            status,
            serde_json::json!({
                "validExactCode":false,"bootstrapAllowed":true,"trustId":null
            })
        );
        let bad = handle(&state, &actor, read(serde_json::json!({"clientPid":1})))
            .await
            .into_result()
            .unwrap_err();
        assert_eq!(bad.code, RpcError::INVALID_PARAMS);
        let counts = state
            .db
            .call(|store| -> rusqlite::Result<(i64, i64)> {
                Ok((
                    store
                        .conn()
                        .query_row("SELECT COUNT(*) FROM local_client_trust", [], |r| r.get(0))?,
                    store.conn().query_row(
                        "SELECT COUNT(*) FROM public_command_receipts",
                        [],
                        |r| r.get(0),
                    )?,
                ))
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(counts, (0, 0));
        actor
            .connection_scope()
            .close_until(std::time::Instant::now() + std::time::Duration::from_secs(1))
            .unwrap();
        let closed = handle(&state, &actor, read(serde_json::json!({})))
            .await
            .into_result()
            .unwrap_err();
        assert_eq!(closed.code, RpcError::FORBIDDEN);
        state.db.clone().shutdown();
    }

    #[tokio::test]
    async fn eof_fenced_actor_cannot_dispatch_while_accepted_output_drains() {
        let (state, actor, db) = state_actor().await;
        let queued_actor = actor.clone();
        let scope = actor.connection_scope().clone();
        let guard = scope.guard();
        scope
            .fence_input_until(std::time::Instant::now() + std::time::Duration::from_secs(2))
            .unwrap();
        assert!(!scope.is_live());
        assert!(scope.output_is_live(), "already accepted output may drain");
        assert_eq!(
            queued_actor.principal(&state).await.unwrap_err().code,
            RpcError::FORBIDDEN
        );
        for operation in [
            "session.terminal.write",
            "session.terminal.resize",
            "session.terminal.claim",
            "session.answer",
            "session.respondRunnerRequest",
            "session.start",
        ] {
            assert_eq!(
                queued_actor
                    .authorize(&state, operation)
                    .await
                    .unwrap_err()
                    .code,
                RpcError::FORBIDDEN
            );
            let actor = queued_actor.clone();
            let state_for_tx = state.clone();
            assert!(
                state
                    .db
                    .call(move |store| actor.authorize_tx(&state_for_tx, store.conn(), operation))
                    .await
                    .unwrap()
                    .is_err()
            );
        }
        let pending = CommandSubmitParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            runtime_binding_id: state.runtime_binding_id.clone(),
            db_id: db,
            command: PublicCommand::CrewCreateFirstMessage(FirstMessageParams {
                crew: crewd_proto::CrewCreateParams {
                    title: "queued after read EOF".into(),
                    working_directory: "/private/tmp".into(),
                    parent_id: None,
                },
                text: "must never dispatch".into(),
            }),
        };
        assert_eq!(
            submit_sql(&state, &queued_actor, pending)
                .await
                .unwrap_err()
                .code,
            RpcError::FORBIDDEN
        );
        let counts = state
            .db
            .call(|store| -> rusqlite::Result<Vec<i64>> {
                ["crews", "channels", "messages", "public_command_receipts"]
                    .into_iter()
                    .map(|table| {
                        store.conn().query_row(
                            &format!("SELECT COUNT(*) FROM {table}"),
                            [],
                            |row| row.get(0),
                        )
                    })
                    .collect()
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(counts, vec![0, 0, 0, 0]);
        scope.close();
        assert!(
            !scope.output_is_live(),
            "revoke/hard closure also cancels output"
        );
        assert!(
            !*scope.liveness().lock().unwrap(),
            "closing output never revives input"
        );
        drop(guard);
        state.db.clone().shutdown();
    }
    #[tokio::test]
    async fn first_message_and_receipt_commit_once_under_real_kernel_human_actor() {
        let (s, a, db) = state_actor().await;
        let p = CommandSubmitParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            runtime_binding_id: s.runtime_binding_id.clone(),
            db_id: db.clone(),
            command: PublicCommand::CrewCreateFirstMessage(FirstMessageParams {
                crew: crewd_proto::CrewCreateParams {
                    title: "atomic first message".into(),
                    working_directory: "/private/tmp".into(),
                    parent_id: None,
                },
                text: "original first message".into(),
            }),
        };
        let first = submit_sql(&s, &a, p.clone()).await.unwrap();
        assert_eq!(first["phase"], "committed");
        assert_eq!(first["result"]["message"]["text"], "original first message");
        let retry = submit_sql(&s, &a, p.clone()).await.unwrap();
        assert_eq!(retry, first);
        let read = lookup(
            &s,
            &a,
            CommandLookupParams {
                request_id: p.request_id.clone(),
                db_id: db,
            },
        )
        .await
        .unwrap();
        assert_eq!(read, first);
        let count =
            s.db.call(|s| {
                s.conn()
                    .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count, 1);
        let mut changed = p.clone();
        if let PublicCommand::CrewCreateFirstMessage(v) = &mut changed.command {
            v.text = "replacement forbidden".into();
        }
        assert_eq!(
            submit_sql(&s, &a, changed).await.unwrap_err().code,
            RpcError::CONFLICT
        );
        let count =
            s.db.call(|s| {
                s.conn()
                    .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count, 1);
        s.db.clone().shutdown();
    }
    #[tokio::test]
    async fn invalid_first_message_rolls_back_created_crew_channel_and_receipt() {
        let (s, a, db) = state_actor().await;
        let p = CommandSubmitParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            runtime_binding_id: s.runtime_binding_id.clone(),
            db_id: db,
            command: PublicCommand::CrewCreateFirstMessage(FirstMessageParams {
                crew: crewd_proto::CrewCreateParams {
                    title: "rollback crew".into(),
                    working_directory: "/private/tmp".into(),
                    parent_id: None,
                },
                text: "".into(),
            }),
        };
        assert_eq!(
            submit_sql(&s, &a, p).await.unwrap_err().code,
            RpcError::INVALID_PARAMS
        );
        let counts =
            s.db.call(|s| -> rusqlite::Result<Vec<i64>> {
                ["crews", "channels", "messages", "public_command_receipts"]
                    .into_iter()
                    .map(|table| {
                        s.conn()
                            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                    })
                    .collect()
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(counts, vec![0, 0, 0, 0]);
        s.db.clone().shutdown();
    }
}
