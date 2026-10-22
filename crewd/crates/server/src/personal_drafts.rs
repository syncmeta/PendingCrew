//! Core private draft transport. Every request carries captured caller authority.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_proto::{Request, Response, RpcError, commands::*, personal_drafts::*};
use crewd_store::{command_receipts as receipts, personal_drafts as facts};
use serde_json::{Value, json};
use std::sync::Arc;
fn denied() -> RpcError {
    RpcError::new(RpcError::FORBIDDEN, "当前个人草稿设备或来源权限不可用。")
        .with_refusal("personalDraft.permission_denied")
}
fn conflict() -> RpcError {
    RpcError::new(
        RpcError::CONFLICT,
        "个人草稿作用域、版本或容量已改变；请保留本机原文和原请求。",
    )
    .with_refusal("personalDraft.conflict")
}
fn value<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(dispatch::internal)
}
#[derive(Clone)]
enum Origin {
    Device(Actor),
    Root(crate::peer::RootAdminPermit),
}
impl Origin {
    async fn before(&self, state: &Arc<AppState>, method: &str) -> Result<(), RpcError> {
        match self {
            Self::Device(a) => a.authorize(state, method).await,
            Self::Root(p) => p.check(),
        }
    }
    fn inside(
        &self,
        state: &Arc<AppState>,
        c: &rusqlite::Connection,
        method: &str,
    ) -> Result<facts::Identity, RpcError> {
        match self {
            Self::Root(p) => {
                p.check()?;
                Ok(facts::Identity {
                    principal: "root-admin:0".into(),
                    local_owner: true,
                    local_writer: false,
                    remote: None,
                })
            }
            Self::Device(a) => identity_tx(state, a, c, method),
        }
    }
}
pub(crate) fn identity_tx(
    state: &Arc<AppState>,
    a: &Actor,
    c: &rusqlite::Connection,
    method: &str,
) -> Result<facts::Identity, RpcError> {
    a.authorize_tx(state, c, method)
        .map_err(dispatch::collab_err)?;
    match a {
        Actor::Local(p) => Ok(facts::Identity {
            principal: p.command_principal()?,
            local_owner: true,
            local_writer: true,
            remote: None,
        }),
        Actor::Remote(p) => {
            let owner = p.notification_owner();
            Ok(facts::Identity {
                principal: format!("remote:{}:{}", owner.authority_id, owner.grant_id),
                local_owner: false,
                local_writer: false,
                remote: Some(owner),
            })
        }
    }
}
fn key_acl(c: &rusqlite::Connection, k: &PersonalDraftKey) -> Result<(), RpcError> {
    facts::key_authorized(c, k).map_err(|_| denied())?;
    crewd_collab::team_data::authorize(c, &crewd_collab::Sender::Human, &k.crew_id, false)
        .map(|_| ())
        .map_err(|_| denied())
}
fn command_acl(
    c: &rusqlite::Connection,
    who: &facts::Identity,
    command: &PublicCommand,
) -> Result<(), RpcError> {
    facts::authorize_command(c, who, command, true).map_err(|_| denied())?;
    match command {
        PublicCommand::PersonalDraftCheckpoint(p) => key_acl(c, &p.key),
        PublicCommand::PersonalDraftResolve(p) => key_acl(c, &p.key),
        _ => Ok(()),
    }
}
pub(crate) fn receipt_allowed_tx(
    state: &Arc<AppState>,
    actor: &Actor,
    c: &rusqlite::Connection,
    r: &PublicCommandReceipt,
) -> Result<(), RpcError> {
    if !facts::is_command(&r.command) {
        return Ok(());
    }
    let who = identity_tx(state, actor, c, r.command.method())?;
    command_acl(c, &who, &r.command)
}
fn root_command(command: &PublicCommand) -> bool {
    matches!(
        command,
        PublicCommand::PersonalDraftOwner(_) | PublicCommand::PersonalDraftMember(_)
    )
}
pub(crate) fn is_method(method: &str) -> bool {
    matches!(
        method,
        STATUS | MEMBERS | CATALOG | BRANCHES | REQUEST_FENCE
    )
}
async fn submit_origin(
    state: &Arc<AppState>,
    origin: Origin,
    p: CommandSubmitParams,
    fence: bool,
) -> Result<Value, RpcError> {
    if !facts::is_command(&p.command)
        || (matches!(origin, Origin::Root(_)) && !root_command(&p.command))
    {
        return Err(denied());
    }
    origin.before(state, p.command.method()).await?;
    if !crewd_proto::valid_remote_uuid(&p.request_id)
        || !crewd_proto::valid_remote_uuid(&p.runtime_binding_id)
        || serde_json::to_vec(&p.command)
            .map_err(dispatch::internal)?
            .len()
            > MAX_COMMAND_PAYLOAD_BYTES
    {
        return Err(conflict());
    }
    let owner = state.clone();
    let source = origin.clone();
    let original = p.clone();
    let result = state
        .db
        .call(move |store| -> Result<(Value, bool), RpcError> {
            let tx = store
                .conn()
                .unchecked_transaction()
                .map_err(dispatch::internal)?;
            let who = source.inside(&owner, &tx, p.command.method())?;
            command_acl(&tx, &who, &p.command)?;
            let db = crewd_store::meta_get(&tx, "dbId").map_err(dispatch::internal)?;
            if db.as_deref() != Some(p.db_id.as_str()) {
                return Err(denied());
            }
            let at = crewd_store::now_ts();
            let (result, changed) = if fence {
                (
                    value(
                        facts::fence(
                            &tx,
                            &who,
                            &PersonalDraftRequestFenceParams {
                                original: p.clone(),
                            },
                        )
                        .map_err(|_| conflict())?,
                    )?,
                    false,
                )
            } else {
                match receipts::reserve(&tx, &who.principal, &p, &at).map_err(|_| conflict())? {
                    receipts::Reserved::Existing(r) => (value(r)?, false),
                    receipts::Reserved::New(_) => {
                        if p.runtime_binding_id != owner.runtime_binding_id {
                            return Err(conflict());
                        }
                        let mutation =
                            facts::apply(&tx, &who, &p.command).map_err(|_| conflict())?;
                        let result = value(mutation)?;
                        let receipt = receipts::transition(
                            &tx,
                            &who.principal,
                            &p.db_id,
                            &p.request_id,
                            (CommandPhase::Reserved, CommandPhase::Committed),
                            Some(&result),
                            &at,
                        )
                        .map_err(|_| conflict())?;
                        (value(receipt)?, true)
                    }
                }
            };
            tx.commit().map_err(dispatch::internal)?;
            Ok((result, changed))
        })
        .await
        .map_err(dispatch::internal)??;
    if result.1 {
        if let Some(r) = result.0.get("result") {
            if let (Some(space), Some(epoch), Some(revision)) = (
                r.get("spaceId"),
                r.get("spaceEpoch"),
                r.get("spaceRevision"),
            ) {
                state.hub.publish(crewd_proto::Event {
                    event: "personalDraft.changed".into(),
                    data: json!({"spaceId":space,"spaceEpoch":epoch,"revision":revision}),
                });
            }
        }
    }
    origin.before(state, original.command.method()).await?;
    let owner = state.clone();
    let source = origin.clone();
    let payload = result.0;
    state
        .db
        .call(move |store| {
            let who = source.inside(&owner, store.conn(), original.command.method())?;
            command_acl(store.conn(), &who, &original.command)?;
            Ok(payload)
        })
        .await
        .map_err(dispatch::internal)?
}
pub(crate) async fn submit(
    state: &Arc<AppState>,
    actor: &Actor,
    p: CommandSubmitParams,
) -> Result<Value, RpcError> {
    submit_origin(state, Origin::Device(actor.clone()), p, false).await
}
fn read_tx(
    c: &rusqlite::Connection,
    who: &facts::Identity,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    match method {
        STATUS => {
            let _: PersonalDraftStatusParams = crewd_proto::parse_params(params)?;
            value(facts::status(c, who).map_err(|_| denied())?)
        }
        MEMBERS => {
            let p = crewd_proto::parse_params(params)?;
            value(facts::members(c, who, &p).map_err(|_| denied())?)
        }
        CATALOG => {
            let p = crewd_proto::parse_params(params)?;
            let mut page = facts::catalog(c, who, &p).map_err(|_| denied())?;
            page.entries.retain(|e| key_acl(c, &e.key).is_ok());
            value(page)
        }
        BRANCHES => {
            let p: PersonalDraftBranchesParams = crewd_proto::parse_params(params)?;
            key_acl(c, &p.key)?;
            value(facts::branches(c, who, &p).map_err(|_| denied())?)
        }
        _ => Err(denied()),
    }
}
async fn read(
    state: &Arc<AppState>,
    origin: Origin,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    origin.before(state, method).await?;
    let owner = state.clone();
    let source = origin.clone();
    let name = method.to_owned();
    let args = params.clone();
    let result = state
        .db
        .call(move |s| {
            let who = source.inside(&owner, s.conn(), &name)?;
            read_tx(s.conn(), &who, &name, args)
        })
        .await
        .map_err(dispatch::internal)??;
    origin.before(state, method).await?;
    let owner = state.clone();
    let source = origin.clone();
    let name = method.to_owned();
    state
        .db
        .call(move |s| {
            let who = source.inside(&owner, s.conn(), &name)?;
            let _ = read_tx(s.conn(), &who, &name, params)?;
            Ok(result)
        })
        .await
        .map_err(dispatch::internal)?
}
pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, req: Request) -> Response {
    let r = if req.method == REQUEST_FENCE {
        match crewd_proto::parse_params::<PersonalDraftRequestFenceParams>(req.params) {
            Ok(p) => submit_origin(state, Origin::Device(actor.clone()), p.original, true).await,
            Err(e) => Err(e),
        }
    } else {
        read(
            state,
            Origin::Device(actor.clone()),
            &req.method,
            req.params,
        )
        .await
    };
    match r {
        Ok(v) => Response::ok(req.id, v),
        Err(e) => Response::err(req.id, e),
    }
}
pub(crate) async fn event_allowed(state: &Arc<AppState>, actor: &Actor, data: &Value) -> bool {
    let Ok(e) = serde_json::from_value::<PersonalDraftChanged>(data.clone()) else {
        return false;
    };
    if actor.authorize(state, STATUS).await.is_err() {
        return false;
    }
    let owner = state.clone();
    let a = actor.clone();
    state
        .db
        .call(move |s| {
            let who = identity_tx(&owner, &a, s.conn(), STATUS)?;
            facts::event_allowed(s.conn(), &who, &e).map_err(|_| denied())
        })
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false)
}
pub(crate) async fn admin(
    state: &Arc<AppState>,
    permit: crate::peer::RootAdminPermit,
    method: &str,
    args: Value,
) -> Result<Value, RpcError> {
    let origin = Origin::Root(permit);
    match method {
        "host.personalDraftStatus" => {
            let status = read(state, origin, STATUS, args).await?;
            value(HostPersonalDraftStatus {
                runtime_binding_id: state.runtime_binding_id.clone(),
                status: serde_json::from_value(status).map_err(dispatch::internal)?,
            })
        }
        "host.personalDraftMembers" => read(state, origin, MEMBERS, args).await,
        "host.personalDraftOwnerConfigure" => {
            let p: HostPersonalDraftOwnerParams = crewd_proto::parse_params(args)?;
            submit_origin(
                state,
                origin,
                CommandSubmitParams {
                    request_id: p.request_id,
                    db_id: p.db_id,
                    runtime_binding_id: p.runtime_binding_id,
                    command: PublicCommand::PersonalDraftOwner(p.params),
                },
                false,
            )
            .await
        }
        "host.personalDraftMemberSet" => {
            let p: HostPersonalDraftMemberParams = crewd_proto::parse_params(args)?;
            submit_origin(
                state,
                origin,
                CommandSubmitParams {
                    request_id: p.request_id,
                    db_id: p.db_id,
                    runtime_binding_id: p.runtime_binding_id,
                    command: PublicCommand::PersonalDraftMember(p.params),
                },
                false,
            )
            .await
        }
        "host.personalDraftRequestFence" => {
            let p: PersonalDraftRequestFenceParams = crewd_proto::parse_params(args)?;
            submit_origin(state, origin, p.original, true).await
        }
        "host.personalDraftRequestLookup" => {
            let p: CommandLookupParams = crewd_proto::parse_params(args)?;
            origin.before(state, STATUS).await?;
            let owner = state.clone();
            state
                .db
                .call(move |s| {
                    let who = origin.inside(&owner, s.conn(), STATUS)?;
                    let receipt =
                        receipts::lookup(s.conn(), &who.principal, &p.db_id, &p.request_id)
                            .map_err(|_| denied())?;
                    if let Some(r) = &receipt {
                        if !root_command(&r.command) {
                            return Err(denied());
                        }
                        command_acl(s.conn(), &who, &r.command)?
                    }
                    value(receipt)
                })
                .await
                .map_err(dispatch::internal)?
        }
        _ => Err(denied()),
    }
}
