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
struct Captured {
    origin: Origin,
    original: CommandSubmitParams,
    fence: bool,
}
async fn capture(
    state: &Arc<AppState>,
    origin: Origin,
    p: CommandSubmitParams,
    fence: bool,
) -> Result<Captured, RpcError> {
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
    Ok(Captured {
        origin,
        original: p,
        fence,
    })
}
async fn submit_origin(
    state: &Arc<AppState>,
    origin: Origin,
    p: CommandSubmitParams,
    fence: bool,
) -> Result<Value, RpcError> {
    let captured = capture(state, origin, p, fence).await?;
    execute(state, captured).await
}
async fn execute(state: &Arc<AppState>, captured: Captured) -> Result<Value, RpcError> {
    let Captured {
        origin,
        original: p,
        fence,
    } = captured;
    let owner = state.clone();
    let source = origin.clone();
    let original = p.clone();
    let mut result = state
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
    let receipt = if fence {
        result.0.get_mut("receipt")
    } else {
        Some(&mut result.0)
    };
    if let Some(receipt) = receipt {
        if receipt
            .get("runtimeBindingId")
            .and_then(Value::as_str)
            .is_some_and(|id| id != state.runtime_binding_id)
        {
            receipt["historical"] = json!(true);
        }
    }
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
    let _ = state
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
            // Return the final authorized observation, never an earlier catalog
            // whose entry ACL may have changed while the actor check awaited.
            read_tx(s.conn(), &who, &name, params)
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
                    let mut receipt =
                        receipts::lookup(s.conn(), &who.principal, &p.db_id, &p.request_id)
                            .map_err(|_| denied())?;
                    if let Some(r) = &receipt {
                        if !root_command(&r.command) {
                            return Err(denied());
                        }
                        command_acl(s.conn(), &who, &r.command)?
                    }
                    if let Some(r) = &mut receipt {
                        r.historical |= r.runtime_binding_id != owner.runtime_binding_id;
                    }
                    value(receipt)
                })
                .await
                .map_err(dispatch::internal)?
        }
        _ => Err(denied()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) async fn configured() -> (
        Arc<AppState>,
        Actor,
        String,
        PersonalDraftScope,
        PersonalDraftKey,
    ) {
        let (state, actor, db) = crate::public_commands::tests::state_actor().await;
        state.db.call(|s|s.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at)VALUES('draft-crew','synthetic','now','now');INSERT INTO channels(id,crew_id,kind,name,created_by,created_at)VALUES('draft-crew/lobby','draft-crew','lobby','lobby','human','now');")).await.unwrap().unwrap();
        let epoch = state
            .db
            .call(|s| crewd_store::personal_drafts::epoch(s.conn()))
            .await
            .unwrap()
            .unwrap();
        let command = PublicCommand::PersonalDraftOwner(PersonalDraftOwnerParams {
            owner_epoch: epoch,
            expected_space_id: None,
            expected_space_revision: "0".into(),
            local_enabled: true,
            acknowledge_restore: false,
        });
        submit(
            &state,
            &actor,
            CommandSubmitParams {
                request_id: uuid::Uuid::new_v4().to_string(),
                db_id: db.clone(),
                runtime_binding_id: state.runtime_binding_id.clone(),
                command,
            },
        )
        .await
        .unwrap();
        let status = read(&state, Origin::Device(actor.clone()), STATUS, json!({}))
            .await
            .unwrap();
        let status: PersonalDraftStatus = serde_json::from_value(status).unwrap();
        let m = status.member.unwrap();
        let scope = PersonalDraftScope {
            space_id: status.space_id.unwrap(),
            space_epoch: status.space_epoch.unwrap(),
            member_id: m.member_id,
            member_revision: m.revision,
        };
        let key = PersonalDraftKey {
            authority_id: db.clone(),
            db_id: db.clone(),
            crew_id: "draft-crew".into(),
            destination: PersonalDraftDestination::Channel("draft-crew/lobby".into()),
            kind: PersonalDraftKind::ChannelMessage,
        };
        (state, actor, db, scope, key)
    }
    pub(super) fn checkpoint(
        state: &AppState,
        db: &str,
        scope: PersonalDraftScope,
        key: PersonalDraftKey,
    ) -> CommandSubmitParams {
        CommandSubmitParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            db_id: db.into(),
            runtime_binding_id: state.runtime_binding_id.clone(),
            command: PublicCommand::PersonalDraftCheckpoint(PersonalDraftCheckpointParams {
                scope,
                branch_id: uuid::Uuid::new_v4().to_string(),
                key,
                expected_revision: "0".into(),
                revision: "1".into(),
                payload: PersonalDraftPayload {
                    text: "synthetic unsent 原文".into(),
                    staged_mentions: vec![],
                    reply_target: None,
                },
            }),
        }
    }
    #[tokio::test]
    async fn core_personal_draft_captured_submit_after_fence_never_executes_and_inverse_returns_original()
     {
        for fence_first in [true, false] {
            let (state, actor, db, scope, key) = configured().await;
            let p = checkpoint(&state, &db, scope, key);
            let captured = capture(&state, Origin::Device(actor.clone()), p.clone(), false)
                .await
                .unwrap();
            if fence_first {
                let fenced = submit_origin(&state, Origin::Device(actor.clone()), p.clone(), true)
                    .await
                    .unwrap();
                assert_eq!(fenced["state"], "missingFenced");
                let late = execute(&state, captured).await.unwrap();
                assert_eq!(late, fenced["receipt"]);
                assert_eq!(late["phase"], "rejected");
                assert_eq!(late["result"]["reason"], "notSubmitted");
            } else {
                let committed = execute(&state, captured).await.unwrap();
                let fenced = submit_origin(&state, Origin::Device(actor.clone()), p.clone(), true)
                    .await
                    .unwrap();
                assert_eq!(fenced["state"], "existingTerminal");
                assert_eq!(fenced["receipt"], committed);
            }
            let rows = read(
                &state,
                Origin::Device(actor.clone()),
                BRANCHES,
                match &p.command {
                    PublicCommand::PersonalDraftCheckpoint(p) => {
                        json!({"scope":p.scope,"key":p.key})
                    }
                    _ => unreachable!(),
                },
            )
            .await
            .unwrap();
            assert_eq!(
                rows["branches"].as_array().unwrap().len(),
                usize::from(!fence_first)
            );
            let mut changed = p.clone();
            if let PublicCommand::PersonalDraftCheckpoint(p) = &mut changed.command {
                p.payload.text = "different envelope".into()
            }
            assert!(
                submit_origin(&state, Origin::Device(actor.clone()), changed, true)
                    .await
                    .is_err()
            );
        }
    }
    #[tokio::test]
    async fn core_personal_draft_first_owner_fence_and_cross_runtime_original_packet_close_only_that_gate()
     {
        let (state, actor, db) = crate::public_commands::tests::state_actor().await;
        let before = read(&state, Origin::Device(actor.clone()), STATUS, json!({}))
            .await
            .unwrap();
        assert!(before["spaceId"].is_null());
        let original = CommandSubmitParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            db_id: db,
            runtime_binding_id: state.runtime_binding_id.clone(),
            command: PublicCommand::PersonalDraftOwner(PersonalDraftOwnerParams {
                owner_epoch: before["ownerEpoch"].as_str().unwrap().into(),
                expected_space_id: None,
                expected_space_revision: "0".into(),
                local_enabled: true,
                acknowledge_restore: false,
            }),
        };
        let captured = capture(
            &state,
            Origin::Device(actor.clone()),
            original.clone(),
            false,
        )
        .await
        .unwrap();
        let negative = submit_origin(
            &state,
            Origin::Device(actor.clone()),
            original.clone(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(negative["state"], "missingFenced");
        assert_eq!(
            execute(&state, captured).await.unwrap(),
            negative["receipt"]
        );
        assert!(
            read(&state, Origin::Device(actor.clone()), STATUS, json!({}))
                .await
                .unwrap()["spaceId"]
                .is_null()
        );
        // A normal Core birth changes only runtimeBindingId, not the persisted owner authority.
        let restarted = Arc::new(AppState::new(state.db.clone()));
        assert_ne!(restarted.runtime_binding_id, state.runtime_binding_id);
        let found = submit_origin(
            &restarted,
            Origin::Device(actor.clone()),
            original.clone(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(found["state"], "existingTerminal");
        assert_eq!(found["receipt"]["historical"], true);
        assert_eq!(
            found["receipt"]["runtimeBindingId"],
            original.runtime_binding_id
        );
        let mut missing = original.clone();
        missing.request_id = uuid::Uuid::new_v4().to_string();
        let fenced = submit_origin(&restarted, Origin::Device(actor.clone()), missing, true)
            .await
            .unwrap();
        assert_eq!(fenced["state"], "missingFenced");
        assert_eq!(fenced["receipt"]["historical"], true);
        // This is a distinct explicit save, retaining the identical original parameters.
        let mut explicit = original;
        explicit.request_id = uuid::Uuid::new_v4().to_string();
        explicit.runtime_binding_id = restarted.runtime_binding_id.clone();
        let positive = submit(&restarted, &actor, explicit).await.unwrap();
        assert_eq!(positive["phase"], "committed");
        assert!(positive["result"]["member"].is_object());
        assert_eq!(
            read(&restarted, Origin::Device(actor), STATUS, json!({}))
                .await
                .unwrap()["ownerEpoch"],
            before["ownerEpoch"]
        );
    }
    #[tokio::test]
    async fn core_personal_draft_unavailable_unknown_page_keeps_original_cursor_to_unrelated_control()
     {
        let (state, actor, db, scope, key) = configured().await;
        let mut hidden = checkpoint(&state, &db, scope.clone(), key);
        hidden.request_id = "11111111-1111-4111-8111-111111111111".into();
        let mut other = hidden.clone();
        other.request_id = "22222222-2222-4222-8222-222222222222".into();
        other.command = PublicCommand::CrewCreate(crewd_proto::CrewCreateParams {
            title: "unrelated pending control".into(),
            working_directory: "/private/tmp".into(),
            parent_id: None,
        });
        let owner = state.clone();
        let a = actor.clone();
        let originals = [hidden.clone(), other.clone()];
        state
            .db
            .call(move |s| {
                s.with_tx(|c| {
                    let who = identity_tx(&owner, &a, c, STATUS).unwrap();
                    for original in originals {
                        receipts::reserve(c, &who.principal, &original, "queued")?;
                    }
                    Ok(())
                })
            })
            .await
            .unwrap()
            .unwrap();
        let disable = CommandSubmitParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            db_id: db.clone(),
            runtime_binding_id: state.runtime_binding_id.clone(),
            command: PublicCommand::PersonalDraftConsent(PersonalDraftConsentParams {
                scope,
                enabled: false,
            }),
        };
        submit(&state, &actor, disable).await.unwrap();
        let first = crate::public_commands::list_unknown(
            &state,
            &actor,
            CommandListUnknownParams {
                db_id: db.clone(),
                after: None,
                limit: Some(1),
            },
        )
        .await
        .unwrap();
        assert_eq!(first["items"], json!([]));
        assert_eq!(first["nextAfter"], hidden.request_id);
        let second = crate::public_commands::list_unknown(
            &state,
            &actor,
            CommandListUnknownParams {
                db_id: db,
                after: Some(hidden.request_id),
                limit: Some(1),
            },
        )
        .await
        .unwrap();
        assert_eq!(second["items"][0]["requestId"], other.request_id);
        assert!(second["nextAfter"].is_null());
    }
    #[tokio::test]
    async fn core_personal_draft_existing_pending_fence_never_settles_or_replays_and_forbids_other_commands()
     {
        let (state, actor, db, scope, key) = configured().await;
        let p = checkpoint(&state, &db, scope, key);
        let request = p.clone();
        let owner = state.clone();
        let a = actor.clone();
        state
            .db
            .call(move |s| {
                s.with_tx(|c| {
                    let who = identity_tx(&owner, &a, c, STATUS).unwrap();
                    receipts::reserve(c, &who.principal, &request, "synthetic queued").map(|_| ())
                })
            })
            .await
            .unwrap()
            .unwrap();
        let fenced = submit_origin(&state, Origin::Device(actor.clone()), p.clone(), true)
            .await
            .unwrap();
        assert_eq!(fenced["state"], "existingPending");
        assert_eq!(fenced["receipt"]["phase"], "reserved");
        let mut invalid = p;
        invalid.command = PublicCommand::SessionResume(CommandRuntimeTarget {
            session_id: "not-a-draft".into(),
            generation: "none".into(),
            control_revision: "0".into(),
            turn_id: None,
        });
        assert!(
            capture(&state, Origin::Device(actor), invalid, true)
                .await
                .is_err()
        );
    }
}
#[cfg(test)]
mod five_command_fence_tests {
    use super::*;
    #[tokio::test]
    async fn core_personal_draft_all_five_sql_fences_cover_both_orders_pending_and_immutable_envelope()
     {
        for operation in ["owner", "member", "consent", "checkpoint", "resolve"] {
            for fence_first in [true, false] {
                let (state, actor, db, scope, key) = super::tests::configured().await;
                let checkpoint = super::tests::checkpoint(&state, &db, scope.clone(), key.clone());
                let command = match operation {
                    "owner" => {
                        let status = read(&state, Origin::Device(actor.clone()), STATUS, json!({}))
                            .await
                            .unwrap();
                        PublicCommand::PersonalDraftOwner(PersonalDraftOwnerParams {
                            owner_epoch: status["ownerEpoch"].as_str().unwrap().into(),
                            expected_space_id: Some(scope.space_id.clone()),
                            expected_space_revision: status["spaceRevision"]
                                .as_str()
                                .unwrap()
                                .into(),
                            local_enabled: true,
                            acknowledge_restore: false,
                        })
                    }
                    "member" => {
                        let grant_id = uuid::Uuid::new_v4().to_string();
                        let gid = grant_id.clone();
                        let dbid = db.clone();
                        let pin = "a".repeat(64);
                        state
                            .db
                            .call(move |s| {
                                let identity = crewd_proto::RemotePublicIdentity {
                                    authority_id: uuid::Uuid::new_v4().to_string(),
                                    db_id: dbid,
                                    certificate_der_base64: "synthetic store-only cert fact".into(),
                                    spki_sha256: "b".repeat(64),
                                };
                                crewd_store::remote::bind_identity(
                                    s.conn(),
                                    &identity,
                                    "synthetic",
                                )
                                .unwrap();
                                crewd_store::remote::insert_grant(
                                    s.conn(),
                                    &identity.authority_id,
                                    &crewd_proto::RemoteGrantSummary {
                                        id: gid,
                                        scope: crewd_proto::RemoteGrantScope::OwnerWorkspace,
                                        device_label: "synthetic owner-control target".into(),
                                        client_spki_sha256: "a".repeat(64),
                                        revision: "1".into(),
                                        created_at: "synthetic".into(),
                                        revoked_at: None,
                                        revoke_note: None,
                                    },
                                    "synthetic store-only cert fact",
                                )
                                .unwrap()
                            })
                            .await
                            .unwrap();
                        let status = read(&state, Origin::Device(actor.clone()), STATUS, json!({}))
                            .await
                            .unwrap();
                        PublicCommand::PersonalDraftMember(PersonalDraftMemberSetParams {
                            space_id: scope.space_id.clone(),
                            space_epoch: scope.space_epoch.clone(),
                            expected_space_revision: status["spaceRevision"]
                                .as_str()
                                .unwrap()
                                .into(),
                            grant_id,
                            grant_revision: "1".into(),
                            client_spki_sha256: pin,
                            enabled: true,
                        })
                    }
                    "consent" => PublicCommand::PersonalDraftConsent(PersonalDraftConsentParams {
                        scope: scope.clone(),
                        enabled: true,
                    }),
                    "checkpoint" => checkpoint.command.clone(),
                    "resolve" => {
                        let committed = submit(&state, &actor, checkpoint.clone()).await.unwrap();
                        PublicCommand::PersonalDraftResolve(PersonalDraftResolveParams {
                            scope: scope.clone(),
                            key: key.clone(),
                            retiring: vec![PersonalDraftRetire {
                                branch_id: committed["result"]["branch"]["branchId"]
                                    .as_str()
                                    .unwrap()
                                    .into(),
                                revision: "1".into(),
                            }],
                            replacement: None,
                        })
                    }
                    _ => unreachable!(),
                };
                let p = CommandSubmitParams {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    db_id: db,
                    runtime_binding_id: state.runtime_binding_id.clone(),
                    command,
                };
                let before = state
                    .db
                    .call(|s| crewd_store::meta_get(s.conn(), "personalDraft.spaces.v1"))
                    .await
                    .unwrap()
                    .unwrap();
                let captured = capture(&state, Origin::Device(actor.clone()), p.clone(), false)
                    .await
                    .unwrap();
                if fence_first {
                    let negative =
                        submit_origin(&state, Origin::Device(actor.clone()), p.clone(), true)
                            .await
                            .unwrap();
                    assert_eq!(negative["state"], "missingFenced", "{operation}");
                    let after = execute(&state, captured).await.unwrap();
                    assert_eq!(
                        after, negative["receipt"],
                        "captured-before-FIFO {operation} cannot execute after fence"
                    );
                    let actual = state
                        .db
                        .call(|s| crewd_store::meta_get(s.conn(), "personalDraft.spaces.v1"))
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(
                        actual, before,
                        "{operation} negative fence must not change private facts"
                    );
                } else {
                    let positive = execute(&state, captured).await.unwrap();
                    assert_eq!(positive["phase"], "committed", "{operation}");
                    let found =
                        submit_origin(&state, Origin::Device(actor.clone()), p.clone(), true)
                            .await
                            .unwrap();
                    assert_eq!(found["state"], "existingTerminal");
                    assert_eq!(
                        found["receipt"], positive,
                        "{operation} original result only"
                    );
                }
                let mut changed = p.clone();
                changed.runtime_binding_id = uuid::Uuid::new_v4().to_string();
                assert!(
                    submit_origin(&state, Origin::Device(actor.clone()), changed, true)
                        .await
                        .is_err(),
                    "{operation} original runtime binding is immutable even though ordinary restart lookup is allowed"
                );
                let mut pending = p;
                pending.request_id = uuid::Uuid::new_v4().to_string();
                let queued = pending.clone();
                let owner = state.clone();
                let a = actor.clone();
                state
                    .db
                    .call(move |s| {
                        s.with_tx(|c| {
                            let who = identity_tx(&owner, &a, c, STATUS).unwrap();
                            receipts::reserve(
                                c,
                                &who.principal,
                                &queued,
                                "synthetic unresolved old row",
                            )
                            .map(|_| ())
                        })
                    })
                    .await
                    .unwrap()
                    .unwrap();
                let unresolved =
                    submit_origin(&state, Origin::Device(actor.clone()), pending, true)
                        .await
                        .unwrap();
                assert_eq!(unresolved["state"], "existingPending", "{operation}");
                assert_eq!(
                    unresolved["receipt"]["phase"], "reserved",
                    "{operation} pending never becomes a fabricated terminal"
                );
            }
        }
    }
}
