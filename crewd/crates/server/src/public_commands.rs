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
            let crew = create(tx, crewd_proto::CrewCreateParams {
                title: p.crew.title,
                working_directory: p.crew.working_directory.ok_or_else(|| collab::resource_invalid("workingDirectory"))?,
                parent_id: p.crew.parent_id,
            })?;
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
        PublicCommand::PersonalDraftOwner(_)
        | PublicCommand::PersonalDraftMember(_)
        | PublicCommand::PersonalDraftConsent(_)
        | PublicCommand::PersonalDraftCheckpoint(_)
        | PublicCommand::PersonalDraftResolve(_) => crate::personal_drafts::submit(state,actor,p).await,
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
        | PublicCommand::CodexArchive(_) | PublicCommand::CodexUnarchive(_) | PublicCommand::CodexDelete(_) | PublicCommand::CodexRevert(_)
        | PublicCommand::SessionAnswer(_)
        | PublicCommand::SessionRespondRunnerRequest(_)
        | PublicCommand::Hibernate(_)
        | PublicCommand::SessionStart(_)
        | PublicCommand::SessionStop(_)
        | PublicCommand::ClaudeRecoveryResolve(_)
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
    submit_sql_with_home(state, actor, p, None).await
}
async fn submit_sql_with_home(
    state: &Arc<AppState>, actor: &Actor, p: CommandSubmitParams,
    service_home: Option<std::path::PathBuf>,
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
                    let (command, directory) = resolve_first_message_directory(p.command.clone(), service_home.as_deref())?;
                    let result = sql_effect(tx, command, &own_actor)?;
                    // Existing receipts return above without any filesystem effect.
                    if let Some(directory) = directory { create_first_message_directory(&directory)?; }
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
/// The client freezes the name; only the selected service resolves its HOME.
/// Explicit directories retain their original storage-only semantics.
fn resolve_first_message_directory(
    mut command: PublicCommand,
    service_home: Option<&std::path::Path>,
) -> collab::Result<(PublicCommand, Option<std::path::PathBuf>)> {
    let PublicCommand::CrewCreateFirstMessage(p) = &mut command else {
        return Ok((command, None));
    };
    if p.crew.working_directory.is_some() {
        return Ok((command, None));
    }
    let title = p.crew.title.trim();
    if title.is_empty()
        || title == "."
        || title == ".."
        || title.len() > 255
        || title
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(collab::resource_invalid(
            "workingDirectory: choose an explicit directory for this title",
        ));
    }
    let home = service_home
        .map(std::path::Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .filter(|p| p.is_absolute())
        .ok_or_else(|| collab::resource_invalid("workingDirectory: service HOME unavailable"))?;
    let home = home
        .canonicalize()
        .map_err(|_| collab::resource_invalid("workingDirectory: service HOME unavailable"))?;
    let directory = home.join("CrewGround").join(title);
    let text = directory
        .to_str()
        .ok_or_else(|| collab::resource_invalid("workingDirectory"))?;
    p.crew.working_directory = Some(text.into());
    Ok((command, Some(directory)))
}

/// mkdirat is exclusive and bound to the real, non-symlink service-owned root.
/// A later SQL/transport failure never triggers deletion or an automatic retry.
fn create_first_message_directory(directory: &std::path::Path) -> collab::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    let invalid = || {
        collab::resource_invalid(
            "workingDirectory: could not create exclusive CrewGround directory",
        )
    };
    let root = directory.parent().ok_or_else(invalid)?;
    match std::fs::DirBuilder::new().mode(0o700).create(root) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(invalid()),
    }
    let fd = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)
        .map_err(|_| invalid())?;
    let meta = fd.metadata().map_err(|_| invalid())?;
    if meta.uid() != unsafe { libc::geteuid() } || meta.permissions().mode() & 0o022 != 0 {
        return Err(invalid());
    }
    let name = std::ffi::CString::new(
        directory
            .file_name()
            .ok_or_else(invalid)?
            .as_encoded_bytes(),
    )
    .map_err(|_| invalid())?;
    if unsafe { libc::mkdirat(fd.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
        return Err(invalid());
    }
    fd.sync_all().map_err(|_| invalid())?;
    Ok(())
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
                crate::personal_drafts::receipt_allowed_tx(&own_state,&own_actor,s.conn(),r).map_err(|_|collab::CollabError::Forbidden(collab::refusal::Refusal::new(collab::refusal::Code::ResourceForbidden)))?;
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
    if result.as_ref().is_some_and(|r|crewd_store::personal_drafts::is_command(&r.command)) {
        let current=result.clone();let owner=state.clone();let source=actor.clone();
        state.db.call(move|s|{if let Some(r)=&current{crate::personal_drafts::receipt_allowed_tx(&owner,&source,s.conn(),r)?}Ok::<_,RpcError>(())}).await.map_err(dispatch::internal)??;
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
    let (records, next_after) = state
        .db
        .call(move |s| -> collab::Result<_> {
            let mut rows = receipts::list_unknown(s.conn(), &principal, &p.db_id, p.after.as_deref(), limit)?;
            // Keep the original bounded ledger cursor even when a private scope
            // became unavailable. Otherwise hidden rows would terminate traversal
            // before unrelated original command receipts can be reached.
            let more = rows.len() > usize::from(limit);
            rows.truncate(usize::from(limit));
            let next = if more { rows.last().map(|r|r.request_id.clone()) } else { None };
            rows.retain(|r|crate::personal_drafts::receipt_allowed_tx(&own_state,&own_actor,s.conn(),r).is_ok());
            for r in &rows {
                for method in r.command.authorized_methods() {
                    own_actor.authorize_tx(&own_state, s.conn(), method)?;
                }
            }
            Ok((rows,next))
        })
        .await
        .map_err(dispatch::internal)?
        .map_err(dispatch::collab_err)?;
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
pub(crate) mod tests {
    use super::*;
    pub(crate) async fn state_actor() -> (Arc<AppState>, Actor, String) {
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
    fn default_first_message(s: &AppState, db: &str, title: &str, directory: Option<&str>, text: &str) -> CommandSubmitParams {
        serde_json::from_value(serde_json::json!({"requestId":uuid::Uuid::new_v4().to_string(),"dbId":db,"runtimeBindingId":s.runtime_binding_id,
            "command":{"operation":"crew.createFirstMessage","params":{"crew":{"title":title,"workingDirectory":directory,"parentId":null},"text":text}}})).unwrap()
    }
    async fn default_first_counts(s: &AppState) -> Vec<i64> {
        s.db.call(|store| -> rusqlite::Result<Vec<i64>> {
            ["crews","channels","messages","public_command_receipts"].iter().map(|table|
                store.conn().query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|r|r.get(0))).collect()
        }).await.unwrap().unwrap()
    }
    #[tokio::test]
    async fn first_message_default_directory_uses_selected_home_and_original_receipt() {
        let(s,a,db)=state_actor().await;let home=tempfile::tempdir().unwrap();let other=tempfile::tempdir().unwrap();
        let p=default_first_message(&s,&db,"Reykjavik",None,"static human introduction");
        let _=lookup(&s,&a,CommandLookupParams{request_id:p.request_id.clone(),db_id:db.clone()}).await;
        assert!(!home.path().join("CrewGround").exists());
        let first=submit_sql_with_home(&s,&a,p.clone(),Some(home.path().into())).await.unwrap();let target=home.path().canonicalize().unwrap().join("CrewGround/Reykjavik");assert!(target.is_dir());
        assert_eq!(first["result"]["crew"]["workingDirectory"],target.to_str().unwrap());let raw=s.db.call(|store|store.conn().query_row("SELECT json_type(payload_json,'$.params.crew.workingDirectory') FROM public_command_receipts",[],|r|r.get::<_,String>(0))).await.unwrap().unwrap();assert_eq!(raw,"null","receipt retains unexpanded client intent");
        let repeated=submit_sql_with_home(&s,&a,p.clone(),Some(other.path().into())).await.unwrap();assert_eq!(first,repeated);assert!(!other.path().join("CrewGround").exists());
        assert_eq!(default_first_counts(&s).await,vec![2,3,1,1]);
        let read=lookup(&s,&a,CommandLookupParams{request_id:p.request_id.clone(),db_id:db.clone()}).await.unwrap();assert_eq!(read,first);
        let mut changed=p;if let PublicCommand::CrewCreateFirstMessage(v)=&mut changed.command{v.crew.title="Kyoto".into();}
        assert_eq!(submit_sql_with_home(&s,&a,changed,Some(home.path().into())).await.unwrap_err().code,RpcError::CONFLICT);assert!(!home.path().join("CrewGround/Kyoto").exists());s.db.clone().shutdown();
    }
    #[tokio::test]
    async fn first_message_default_directory_rejects_names_and_invalid_text_without_effects() {
        let(s,a,db)=state_actor().await;let home=tempfile::tempdir().unwrap();
        for title in ["", ".", "..", "../outside", "/absolute", "nested/leaf", "nested\\leaf", "line\nbreak"] {
            let p=default_first_message(&s,&db,title,None,"message");assert!(submit_sql_with_home(&s,&a,p,Some(home.path().into())).await.is_err(),"{title:?}");
        }
        let p=default_first_message(&s,&db,"Kyoto",None,"");assert!(submit_sql_with_home(&s,&a,p,Some(home.path().into())).await.is_err());
        assert!(!home.path().join("CrewGround").exists());assert_eq!(default_first_counts(&s).await,vec![0,0,0,0]);s.db.clone().shutdown();
    }
    #[tokio::test]
    async fn first_message_default_directory_filesystem_failures_preserve_data_and_rollback_sql() {
        for kind in ["root-file","existing-leaf","symlink-root"] {
            let(s,a,db)=state_actor().await;let home=tempfile::tempdir().unwrap();let outside=tempfile::tempdir().unwrap();let root=home.path().join("CrewGround");
            match kind {
                "root-file"=>std::fs::write(&root,"original").unwrap(),
                "existing-leaf"=>{std::fs::create_dir(&root).unwrap();std::fs::create_dir(root.join("Lisbon")).unwrap();std::fs::write(root.join("Lisbon/keep.txt"),"original").unwrap();},
                _=>std::os::unix::fs::symlink(outside.path(),&root).unwrap(),
            }
            let p=default_first_message(&s,&db,"Lisbon",None,"message");assert!(submit_sql_with_home(&s,&a,p,Some(home.path().into())).await.is_err(),"{kind}");assert_eq!(default_first_counts(&s).await,vec![0,0,0,0]);
            if kind=="root-file"{assert_eq!(std::fs::read_to_string(&root).unwrap(),"original");}
            if kind=="existing-leaf"{assert_eq!(std::fs::read_to_string(root.join("Lisbon/keep.txt")).unwrap(),"original");}
            assert!(!outside.path().join("Lisbon").exists());s.db.clone().shutdown();
        }
    }
    #[tokio::test]
    async fn first_message_default_directory_explicit_paths_remain_storage_only() {
        let(s,a,db)=state_actor().await;let home=tempfile::tempdir().unwrap();let path=home.path().join("explicit-not-created");
        let p=default_first_message(&s,&db,"../display/title",Some(path.to_str().unwrap()),"message");let r=submit_sql_with_home(&s,&a,p,Some(home.path().into())).await.unwrap();
        assert_eq!(r["result"]["crew"]["workingDirectory"],path.to_str().unwrap());assert!(!path.exists());assert!(!home.path().join("CrewGround").exists());s.db.clone().shutdown();
    }
    #[tokio::test]
    async fn first_message_default_directory_commit_failure_preserves_uncertain_directory() {
        let(s,a,db)=state_actor().await;let home=tempfile::tempdir().unwrap();
        s.db.call(|store|store.conn().execute_batch("CREATE TRIGGER fail_first_message_receipt BEFORE UPDATE ON public_command_receipts BEGIN SELECT RAISE(ABORT,'synthetic receipt failure'); END")).await.unwrap().unwrap();
        let p=default_first_message(&s,&db,"Tallinn",None,"message");assert!(submit_sql_with_home(&s,&a,p.clone(),Some(home.path().into())).await.is_err());
        let target=home.path().join("CrewGround/Tallinn");assert!(target.is_dir());std::fs::write(target.join("keep.txt"),"possibly used").unwrap();assert_eq!(default_first_counts(&s).await,vec![0,0,0,0]);
        s.db.call(|store|store.conn().execute_batch("DROP TRIGGER fail_first_message_receipt")).await.unwrap().unwrap();
        assert!(submit_sql_with_home(&s,&a,p,Some(home.path().into())).await.is_err());assert_eq!(std::fs::read_to_string(target.join("keep.txt")).unwrap(),"possibly used");assert_eq!(default_first_counts(&s).await,vec![0,0,0,0]);s.db.clone().shutdown();
    }

    #[tokio::test]
    async fn core_personal_draft_owner_uses_original_receipt_and_real_local_authority() {
        let(state,actor,db)=state_actor().await;
        let request=uuid::Uuid::new_v4().to_string();
        let p=CommandSubmitParams{request_id:request.clone(),db_id:db.clone(),runtime_binding_id:state.runtime_binding_id.clone(),command:PublicCommand::PersonalDraftOwner(crewd_proto::personal_drafts::PersonalDraftOwnerParams{owner_epoch:state.db.call(|s|crewd_store::personal_drafts::epoch(s.conn())).await.unwrap().unwrap(),expected_space_id:None,expected_space_revision:"0".into(),local_enabled:true,acknowledge_restore:false})};
        let result=submit(&state,&actor,p.clone()).await;
        assert!(result.is_ok(),"real original command route must commit the new personal-space fact and receipt: {result:?}");
        let r=result.unwrap();assert_eq!(r["phase"],"committed");assert_eq!(r["requestId"],request);
        let repeated=submit(&state,&actor,p).await.unwrap();assert_eq!(r,repeated,"same immutable UUID is attribution only");
        let lookup=lookup(&state,&actor,CommandLookupParams{request_id:request,db_id:db}).await.unwrap();assert_eq!(lookup,r);
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
                crew: FirstMessageCrewParams {
                    title: "queued after read EOF".into(),
                    working_directory: Some("/private/tmp".into()),
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
                crew: FirstMessageCrewParams {
                    title: "atomic first message".into(),
                    working_directory: Some("/private/tmp".into()),
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
    async fn committed_reply_unobserved_is_recovered_by_new_runtime_without_replay() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("crewd.db");
        let store = crewd_store::Store::open(&path).unwrap();
        let db = crewd_store::meta_get(store.conn(), "dbId").unwrap().unwrap();
        let state = Arc::new(AppState::new(crewd_store::Db::spawn(store).unwrap()));
        let policy = Arc::new(crate::peer::PeerPolicy::new(None).unwrap());
        let (server, _client) = tokio::net::UnixStream::pair().unwrap();
        let actor = Actor::Local(policy.accept(&server).unwrap());
        let work = directory.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let command = default_first_message(&state, &db, "lost reply", Some(work.to_str().unwrap()), "one original message");
        let request = command.request_id.clone();
        let payload = serde_json::to_value(&command.command).unwrap();
        // The caller never consumes the committed response. Recreate only the
        // runtime after fully closing and reopening the file Store, without replay.
        drop(submit_sql(&state, &actor, command).await.unwrap());
        let before = default_first_counts(&state).await;
        assert_eq!(before, vec![2, 3, 1, 1]);
        state.db.clone().shutdown();
        let reopened = crewd_store::Store::open(&path).unwrap();
        let next = Arc::new(AppState::new(crewd_store::Db::spawn(reopened).unwrap()));
        assert_ne!(state.runtime_binding_id, next.runtime_binding_id);
        let policy = Arc::new(crate::peer::PeerPolicy::new(None).unwrap());
        let (server, _client) = tokio::net::UnixStream::pair().unwrap();
        let next_actor = Actor::Local(policy.accept(&server).unwrap());
        let receipt = lookup(&next, &next_actor, CommandLookupParams { request_id: request.clone(), db_id: db }).await.unwrap();
        assert_eq!(receipt["requestId"], request);
        assert_eq!(receipt["command"], payload);
        assert_eq!(receipt["phase"], "committed");
        assert_eq!(receipt["historical"], true);
        assert_eq!(receipt["runtimeBindingId"], state.runtime_binding_id);
        assert_eq!(receipt["result"]["message"]["text"], "one original message");
        assert_eq!(default_first_counts(&next).await, before);
        next.db.clone().shutdown();
    }
    #[tokio::test]
    async fn invalid_first_message_rolls_back_created_crew_channel_and_receipt() {
        let (s, a, db) = state_actor().await;
        let p = CommandSubmitParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            runtime_binding_id: s.runtime_binding_id.clone(),
            db_id: db,
            command: PublicCommand::CrewCreateFirstMessage(FirstMessageParams {
                crew: FirstMessageCrewParams {
                    title: "rollback crew".into(),
                    working_directory: Some("/private/tmp".into()),
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
