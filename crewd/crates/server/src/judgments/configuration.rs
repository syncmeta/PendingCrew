//! Human configuration is an external-effect protocol, separate from business receipts.
//! A durable barrier precedes any KeyStore operation. Unique slots make a late
//! unknown write unable to change the credential of a later explicit request.
use super::*;
use crate::{dispatch, public_commands::Actor};
use crewd_proto::{Request, Response, RpcError};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
const BINDING: &str = "judgment.credential.v1";
const REQUEST_PREFIX: &str = "judgment.settings.request.v1:";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    version: u32,
    db: String,
    restore_epoch: String,
    provider: String,
    endpoint: String,
    source: String,
    slot: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    version: u32,
    request_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredResult {
    version: u32,
    db: String,
    restore_epoch: String,
    result: JudgmentSettingsResult,
}
fn invalid() -> RpcError {
    RpcError::new(RpcError::INVALID_PARAMS, "invalid judgment settings")
}
fn denied() -> RpcError {
    RpcError::new(RpcError::FORBIDDEN, "trusted local Human anchor required")
}
fn intent_invalid() -> crewd_collab::CollabError {
    crewd_collab::CollabError::Invalid(
        crewd_collab::refusal::Refusal::new(crewd_collab::refusal::Code::ResourceInvalid)
            .arg("field", "credentialUpdate"),
    )
}
fn sql_invalid() -> crewd_collab::CollabError {
    crewd_store::StoreError::Invalid("invalid judgment configuration metadata".into()).into()
}
fn read<T: serde::de::DeserializeOwned>(
    c: &Connection,
    key: &str,
) -> crewd_collab::Result<Option<T>> {
    crewd_store::meta_get(c, key)?
        .map(|v| {
            serde_json::from_str(&v)
                .map_err(crewd_store::StoreError::from)
                .map_err(Into::into)
        })
        .transpose()
}
fn put<T: Serialize>(c: &Connection, key: &str, value: &T) -> crewd_collab::Result<()> {
    crewd_store::meta_set(
        c,
        key,
        &serde_json::to_string(value).map_err(crewd_store::StoreError::from)?,
    )?;
    Ok(())
}
fn anchor(actor: &Actor) -> Result<(), RpcError> {
    match actor {
        Actor::Local(p) if p.is_human_anchor() && actor.connection_scope().is_live() => {
            p.command_principal()?;
            Ok(())
        }
        _ => Err(denied()),
    }
}
fn anchor_tx(actor: &Actor, state: &Arc<AppState>, c: &Connection) -> crewd_collab::Result<()> {
    anchor(actor).map_err(|_| {
        crewd_collab::CollabError::Forbidden(crewd_collab::refusal::Refusal::new(
            crewd_collab::refusal::Code::ResourceForbidden,
        ))
    })?;
    actor.authorize_tx(state, c, "local.clientTrust")
}
fn db(c: &Connection) -> crewd_collab::Result<String> {
    crewd_store::meta_get(c, "dbId")?.ok_or_else(sql_invalid)
}
fn binding(c: &Connection) -> crewd_collab::Result<Option<Binding>> {
    let b = read::<Binding>(c, BINDING)?;
    if b.as_ref()
        .is_some_and(|b| b.version != 1 || uuid::Uuid::parse_str(&b.slot).is_err())
    {
        return Err(sql_invalid());
    }
    Ok(b)
}
fn pending(c: &Connection) -> crewd_collab::Result<Option<Pending>> {
    let p = read::<Pending>(c, facts::PENDING_KEY)?;
    if p.as_ref()
        .is_some_and(|p| p.version != 1 || uuid::Uuid::parse_str(&p.request_id).is_err())
    {
        return Err(sql_invalid());
    }
    Ok(p)
}
fn matches(b: &Binding, cfg: &JudgmentConfig, db: &str, epoch: &str) -> bool {
    b.db == db
        && b.restore_epoch == epoch
        && b.provider == cfg.provider
        && b.source == cfg.credential_source
        && crewd_jev::client::endpoint_for(&cfg.base_url).is_ok_and(|url| url == b.endpoint)
}
fn view(c: &Connection) -> crewd_collab::Result<JudgmentSettingsView> {
    let settings = facts::configuration(c)?;
    let pending = pending(c)?;
    let stored = binding(c)?;
    let db = db(c)?;
    let epoch = crewd_store::judgment_state::epoch(c)?;
    let credential_state = if pending.is_some() {
        CredentialState::UpdateUnknown
    } else if let Some(b) = stored {
        if settings
            .config
            .as_ref()
            .is_some_and(|cfg| matches(&b, cfg, &db, &epoch))
        {
            CredentialState::RecordedUnverified
        } else {
            CredentialState::Unknown
        }
    } else {
        CredentialState::Missing
    };
    Ok(JudgmentSettingsView {
        settings,
        credential_state,
        pending_request_id: pending.map(|p| p.request_id),
    })
}
fn key_store(data: &Path, b: &Binding) -> Result<crewd_jev::key::KeyStore, ()> {
    if uuid::Uuid::parse_str(&b.slot).is_err() {
        return Err(());
    }
    if b.source == "platform" && cfg!(target_os = "macos") {
        return Ok(crewd_jev::key::KeyStore::Keychain {
            service: crewd_jev::key::KEYCHAIN_SERVICE.into(),
            account: format!("decision:{}:{}", b.db, b.slot),
        });
    }
    if !matches!(b.source.as_str(), "platform" | "file") {
        return Err(());
    }
    let dir = data.join("judgment-keys");
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|_| ())?,
        Ok(m)
            if m.is_dir()
                && !m.file_type().is_symlink()
                && m.uid() == unsafe { libc::geteuid() }
                && m.permissions().mode() & 0o077 == 0 => {}
        _ => return Err(()),
    }
    if dir.canonicalize().map_err(|_| ())? != dir {
        return Err(());
    }
    Ok(crewd_jev::key::KeyStore::File(dir.join(&b.slot)))
}
/// Reads are allowed only after exact current DB/config/binding/barrier validation.
/// No default account/file key is silently inherited by another endpoint or DB.
pub(super) async fn load_key(
    state: &Arc<AppState>,
    data: PathBuf,
    settings: &JudgmentSettings,
) -> Option<crewd_jev::client::ApiKey> {
    let expected = settings.clone();
    let b = state
        .db
        .call_result(move |s| -> crewd_collab::Result<_> {
            if pending(s.conn())?.is_some() || facts::configuration(s.conn())? != expected {
                return Ok(None);
            }
            let Some(b) = binding(s.conn())? else {
                return Ok(None);
            };
            let current_db = db(s.conn())?;
            let epoch = crewd_store::judgment_state::epoch(s.conn())?;
            Ok(expected
                .config
                .as_ref()
                .filter(|cfg| matches(&b, cfg, &current_db, &epoch))
                .map(|_| b))
        })
        .await
        .ok()?
        .ok()??;
    tokio::task::spawn_blocking(move || key_store(&data, &b).ok()?.load().ok().flatten())
        .await
        .ok()?
}
pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, req: Request) -> Response {
    let id = req.id;
    let result = route(state, actor, req).await;
    match result {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    }
}
async fn route(state: &Arc<AppState>, actor: &Actor, req: Request) -> Result<Value, RpcError> {
    if req.method == INSPECT {
        actor.authorize(state, INSPECT).await?;
        let p: JudgmentInspect = crewd_proto::parse_params(req.params)?;
        let actor = actor.clone();
        let owner = state.clone();
        let row = state
            .db
            .call_result(move |s| {
                actor.authorize_tx(&owner, s.conn(), INSPECT)?;
                let source = p.clone();
                let record = facts::inspect(s.conn(), &crewd_collab::Sender::Human, p)?;
                Ok::<_, crewd_collab::CollabError>(JudgmentInspection {
                    source,
                    record: record.as_ref().map(super::public_view),
                })
            })
            .await
            .map_err(dispatch::internal)?
            .map_err(dispatch::collab_err)?;
        return serde_json::to_value(row).map_err(dispatch::internal);
    }
    anchor(actor)?;
    actor.authorize(state, "local.clientTrust").await?;
    if req.method == SETTINGS_GET {
        let p: JudgmentSettingsGet = crewd_proto::parse_params_or_default(req.params)?;
        if p.request_id
            .as_ref()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
        {
            return Err(invalid());
        }
        let actor = actor.clone();
        let owner = state.clone();
        let response = state
            .db
            .call_result(move |s| {
                anchor_tx(&actor, &owner, s.conn())?;
                let current = view(s.conn())?;
                let request_lookup = if let Some(id) = p.request_id {
                    match read::<StoredResult>(s.conn(), &format!("{REQUEST_PREFIX}{id}"))? {
                        None => Some(JudgmentRequestLookup::NotFound),
                        Some(stored) if stored.version != 1 => return Err(sql_invalid()),
                        Some(stored)
                            if stored.db != db(s.conn())?
                                || stored.restore_epoch
                                    != crewd_store::judgment_state::epoch(s.conn())? =>
                        {
                            Some(JudgmentRequestLookup::Historical)
                        }
                        Some(stored) => Some(JudgmentRequestLookup::Known {
                            result: Box::new(stored.result),
                        }),
                    }
                } else {
                    None
                };
                Ok::<_, crewd_collab::CollabError>(JudgmentSettingsResponse {
                    view: current,
                    request_lookup,
                })
            })
            .await
            .map_err(dispatch::internal)?
            .map_err(dispatch::collab_err)?;
        return serde_json::to_value(response).map_err(dispatch::internal);
    }
    if req.method != SETTINGS_SET {
        return Err(invalid());
    }
    let p: JudgmentSettingsSet = crewd_proto::parse_params(req.params)?;
    if uuid::Uuid::parse_str(&p.request_id).is_err() {
        return Err(invalid());
    }
    if let CredentialUpdate::Set { value } = &p.credential_update
        && (p.config.is_none()
            || value.len() > 8192
            || crewd_jev::client::ApiKey::new(value).is_none())
    {
        return Err(invalid());
    }
    if let Some(cfg) = &p.config {
        validate_config(state, cfg)?;
    }
    let _exclusive = state.judgment_settings_mutation.lock().await;
    let request_key = format!("{REQUEST_PREFIX}{}", p.request_id);
    let actor_read = actor.clone();
    let owner = state.clone();
    let key = request_key.clone();
    let known = state
        .db
        .call_result(move |s| {
            anchor_tx(&actor_read, &owner, s.conn())?;
            read::<StoredResult>(s.conn(), &key)?
                .map(|stored| {
                    if stored.version != 1 {
                        return Err(sql_invalid());
                    }
                    let mut result = stored.result;
                    result.historical = stored.db != db(s.conn())?
                        || stored.restore_epoch != crewd_store::judgment_state::epoch(s.conn())?;
                    Ok(result)
                })
                .transpose()
        })
        .await
        .map_err(dispatch::internal)?
        .map_err(dispatch::collab_err)?;
    // The original request identity is lookup-only. Edited intent requires a fresh UUID.
    if let Some(known) = known {
        return serde_json::to_value(known).map_err(dispatch::internal);
    }
    let runtime = state
        .judgment_runtime
        .get()
        .ok_or_else(|| RpcError::new(RpcError::CONFLICT, "judgment runtime unavailable"))?;
    let id = p.request_id.clone();
    let cfg = p.config.clone();
    let expected = p.expected_revision.clone();
    let keep = matches!(p.credential_update, CredentialUpdate::Keep);
    let actor_tx = actor.clone();
    let owner = state.clone();
    let (old, new) = state
        .collab(move |tx| {
            anchor_tx(&actor_tx, &owner, tx.conn())?;
            let settings = facts::configuration(tx.conn())?;
            if settings.revision != expected {
                return Err(crewd_collab::CollabError::Conflict(
                    crewd_collab::refusal::Refusal::new(
                        crewd_collab::refusal::Code::ResourceRequestConflict,
                    ),
                ));
            }
            let old = binding(tx.conn())?;
            let current_db = db(tx.conn())?;
            let epoch = crewd_store::judgment_state::epoch(tx.conn())?;
            if keep {
                if pending(tx.conn())?.is_some() {
                    return Err(intent_invalid());
                }
                if let (Some(previous), Some(next)) = (&settings.config, &cfg)
                    && (previous.provider != next.provider
                        || previous.credential_source != next.credential_source
                        || crewd_jev::client::endpoint_for(&previous.base_url).ok()
                            != crewd_jev::client::endpoint_for(&next.base_url).ok())
                {
                    return Err(intent_invalid());
                }
                if old.as_ref().is_some_and(|b| {
                    cfg.as_ref()
                        .is_none_or(|c| !matches(b, c, &current_db, &epoch))
                }) {
                    return Err(intent_invalid());
                }
            }
            crewd_store::judgment_state::prepare_configuration(tx.conn())?;
            let epoch = crewd_store::judgment_state::epoch(tx.conn())?;
            let new = match &cfg {
                Some(c) if !keep => Some(Binding {
                    version: 1,
                    db: current_db,
                    restore_epoch: epoch,
                    provider: c.provider.clone(),
                    endpoint: crewd_jev::client::endpoint_for(&c.base_url)
                        .map_err(|_| sql_invalid())?,
                    source: c.credential_source.clone(),
                    slot: uuid::Uuid::new_v4().to_string(),
                }),
                _ => old.clone(),
            };
            put(
                tx.conn(),
                facts::PENDING_KEY,
                &Pending {
                    version: 1,
                    request_id: id.clone(),
                },
            )?;
            let unknown = JudgmentSettingsResult {
                historical: false,
                request_id: id,
                outcome: JudgmentConfigurationOutcome::Unknown,
                view: view(tx.conn())?,
            };
            put(
                tx.conn(),
                &format!("{REQUEST_PREFIX}{}", unknown.request_id),
                &StoredResult {
                    version: 1,
                    db: db(tx.conn())?,
                    restore_epoch: crewd_store::judgment_state::epoch(tx.conn())?,
                    result: unknown,
                },
            )?;
            Ok((old, new))
        })
        .await
        .map_err(dispatch::collab_err)?;
    runtime.invalidate().await;
    // This barrier is already durable. Failure, cancellation or late completion
    // leaves Unknown, never an automatic retry or usable credential binding.
    anchor(actor)?;
    let data = runtime.data.clone();
    let action = p.credential_update.clone();
    let old_for_effect = old.clone();
    let new_for_effect = new.clone();
    let effect = tokio::task::spawn_blocking(move || -> Result<(), ()> {
        match action {
            CredentialUpdate::Keep => Ok(()),
            CredentialUpdate::Set { value } => {
                let key = crewd_jev::client::ApiKey::new(&value).ok_or(())?;
                key_store(&data, new_for_effect.as_ref().ok_or(())?)?
                    .store(&key)
                    .map_err(|_| ())
            }
            CredentialUpdate::Clear => match old_for_effect {
                Some(b) => key_store(&data, &b)?.clear().map_err(|_| ()),
                None => Ok(()),
            },
        }
    });
    let success = matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), effect).await,
        Ok(Ok(Ok(())))
    );
    if !success {
        return serde_json::to_value(JudgmentSettingsResult {
            historical: false,
            request_id: p.request_id,
            outcome: JudgmentConfigurationOutcome::Unknown,
            view: get_view(state).await?,
        })
        .map_err(dispatch::internal);
    }
    let actor_tx = actor.clone();
    let owner = state.clone();
    let set = p.clone();
    let clear = matches!(p.credential_update, CredentialUpdate::Clear);
    let key = request_key;
    let committed = state
        .collab(move |tx| {
            anchor_tx(&actor_tx, &owner, tx.conn())?;
            if pending(tx.conn())?.as_ref().map(|p| p.request_id.as_str())
                != Some(set.request_id.as_str())
            {
                return Err(sql_invalid());
            }
            let effect_epoch = crewd_store::judgment_state::epoch(tx.conn())?;
            if !clear
                && set.config.is_some()
                && new
                    .as_ref()
                    .is_some_and(|b| effect_epoch != b.restore_epoch)
            {
                return Err(sql_invalid());
            }
            facts::set_configuration(tx, &crewd_collab::Sender::Human, set.clone())?;
            if clear || set.config.is_none() {
                tx.conn()
                    .execute("DELETE FROM meta WHERE key=?1", [BINDING])?;
            } else if let Some(new) = new {
                put(tx.conn(), BINDING, &new)?;
            }
            tx.conn()
                .execute("DELETE FROM meta WHERE key=?1", [facts::PENDING_KEY])?;
            let result = JudgmentSettingsResult {
                historical: false,
                request_id: set.request_id,
                outcome: JudgmentConfigurationOutcome::Committed,
                view: view(tx.conn())?,
            };
            put(
                tx.conn(),
                &key,
                &StoredResult {
                    version: 1,
                    db: db(tx.conn())?,
                    restore_epoch: crewd_store::judgment_state::epoch(tx.conn())?,
                    result: result.clone(),
                },
            )?;
            Ok(result)
        })
        .await
        .map_err(dispatch::collab_err)?;
    serde_json::to_value(committed).map_err(dispatch::internal)
}
async fn get_view(state: &Arc<AppState>) -> Result<JudgmentSettingsView, RpcError> {
    state
        .db
        .call_result(|s| view(s.conn()))
        .await
        .map_err(dispatch::internal)?
        .map_err(dispatch::collab_err)
}

/// Formal production never accepts plaintext endpoints. Only this private,
/// explicit cfg(test) seam can use the already-existing library fake transport.
pub(super) fn validate_config(state: &AppState, cfg: &JudgmentConfig) -> Result<(), RpcError> {
    provider_config(cfg).validate().map_err(|_| invalid())?;
    if !matches!(cfg.credential_source.as_str(), "platform" | "file") {
        return Err(invalid());
    }
    let endpoint = crewd_jev::client::endpoint_for(&cfg.base_url).map_err(|_| invalid())?;
    if endpoint.starts_with("https://") {
        return Ok(());
    }
    #[cfg(test)]
    if state.judgment_test_http.load(Ordering::Acquire)
        && (endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://[::1]:"))
    {
        return Ok(());
    }
    let _ = state;
    Err(invalid())
}
