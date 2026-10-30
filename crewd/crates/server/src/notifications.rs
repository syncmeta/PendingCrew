//! One owned optional native sender. Public entry always carries captured caller authority.
use crate::{AppState, dispatch, public_commands::Actor};
use base64::{Engine, engine::general_purpose::STANDARD};
use crewd_proto::{Request, Response, RpcError, notifications::*, parse_params};
use crewd_store::{Db, notifications as facts};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::{Mutex, watch};

type Result<T> = std::result::Result<T, RpcError>;
fn denied() -> RpcError {
    RpcError::new(
        RpcError::FORBIDDEN,
        "通知仅允许当前已验证的人和设备管理自己的安装。",
    )
    .with_refusal("notification.permission_denied")
}
fn bad() -> RpcError {
    RpcError::new(
        RpcError::INVALID_PARAMS,
        "通知请求无效或状态已改变，请读取原结果。",
    )
    .with_refusal("notification.invalid_or_conflict")
}
fn unavailable() -> RpcError {
    RpcError::new(RpcError::CONFLICT, "通知未配置或受保护的凭据不可用。")
        .with_refusal("notification.unavailable")
}
fn store_error(_: crewd_store::StoreError) -> RpcError {
    bad()
}
fn value<T: serde::Serialize>(v: T) -> Result<Value> {
    serde_json::to_value(v).map_err(|_| bad())
}
fn env(v: PushEnvironment) -> crate::apns::Environment {
    match v {
        PushEnvironment::Sandbox => crate::apns::Environment::Sandbox,
        PushEnvironment::Production => crate::apns::Environment::Production,
    }
}
fn now() -> i64 {
    crewd_store::now_ms()
}
fn request_id(id: &str) -> Result<()> {
    if crewd_proto::valid_remote_uuid(id) {
        Ok(())
    } else {
        Err(bad())
    }
}
fn status(s: &facts::Settings, available: bool, reason: Option<&str>) -> NotificationStatus {
    NotificationStatus {
        enabled: s.enabled,
        available,
        reason: reason.map(str::to_string),
        revision: s.revision.to_string(),
        environment: s.binding.as_ref().map(|b| b.environment),
        topic: s.binding.as_ref().map(|b| b.topic.clone()),
    }
}
#[derive(Clone)]
enum ConfigurationAuthority {
    Local(Actor),
    Root(crate::peer::RootAdminPermit),
}
impl ConfigurationAuthority {
    fn authorize_tx(
        &self,
        state: &Arc<AppState>,
        c: &rusqlite::Connection,
        method: &str,
    ) -> crewd_collab::Result<()> {
        match self {
            Self::Local(actor) => actor.authorize_tx(state, c, method),
            Self::Root(permit) => permit.check().map_err(|_| {
                crewd_collab::CollabError::Forbidden(crewd_collab::refusal::Refusal::new(
                    crewd_collab::refusal::Code::ResourceForbidden,
                ))
            }),
        }
    }
    async fn read<T: Send + 'static>(
        &self,
        runtime: &Runtime,
        method: &str,
        f: impl FnOnce(&rusqlite::Connection) -> crewd_store::Result<T> + Send + 'static,
    ) -> Result<T> {
        match self {
            Self::Local(actor) => runtime.read(actor, method, f).await,
            Self::Root(permit) => {
                permit.check()?;
                let p = permit.clone();
                let result = runtime
                    .db
                    .call(move |store| {
                        p.check()?;
                        f(store.conn()).map_err(store_error)
                    })
                    .await
                    .map_err(|_| unavailable())??;
                permit.check()?;
                Ok(result)
            }
        }
    }
}
struct Ready {
    revision: i64,
    sender: Arc<crate::apns::Sender>,
    seal: Arc<Vec<u8>>,
}
enum Cache {
    Ready(Ready),
    Unavailable(i64),
}
#[cfg(test)]
#[derive(Default)]
pub(crate) struct ConfigPause {
    pub entered: tokio::sync::Notify,
    pub release: tokio::sync::Notify,
}
pub(crate) struct Runtime {
    state: Weak<AppState>,
    db: Db,
    directory: PathBuf,
    cache: Mutex<Option<Cache>>,
    mutation: Mutex<()>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    drain: watch::Receiver<bool>,
    #[cfg(test)]
    file_keys: bool,
    #[cfg(test)]
    config_pause: std::sync::Mutex<Option<Arc<ConfigPause>>>,
}
impl Runtime {
    #[cfg(test)]
    pub(crate) fn file_test_runtime(state: &Arc<AppState>, directory: PathBuf) -> Arc<Self> {
        let (_, drain) = watch::channel(false);
        Arc::new(Self {
            state: Arc::downgrade(state),
            db: state.db.clone(),
            directory,
            cache: Mutex::new(None),
            mutation: Mutex::new(()),
            task: Mutex::new(None),
            drain,
            file_keys: true,
            config_pause: std::sync::Mutex::new(None),
        })
    }

    pub(crate) async fn start(
        state: &Arc<AppState>,
        directory: PathBuf,
        mut drain: watch::Receiver<bool>,
    ) -> Arc<Self> {
        let runtime = Arc::new(Self {
            state: Arc::downgrade(state),
            db: state.db.clone(),
            directory,
            cache: Mutex::new(None),
            mutation: Mutex::new(()),
            task: Mutex::new(None),
            drain: drain.clone(),
            #[cfg(test)]
            file_keys: false,
            #[cfg(test)]
            config_pause: std::sync::Mutex::new(None),
        });
        let _ = runtime.db.call(|s| s.with_tx(|c| facts::restart(c))).await;
        let weak = Arc::downgrade(&runtime);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {changed=drain.changed()=>{if changed.is_err()||*drain.borrow(){break}},_=tokio::time::sleep(Duration::from_millis(500))=>{let Some(runtime)=weak.upgrade()else{break};if *drain.borrow(){break}runtime.pump().await;}}
            }
        });
        *runtime.task.lock().await = Some(task);
        runtime
    }
    #[cfg(test)]
    pub(crate) fn test_pause_config_commit(&self) -> Arc<ConfigPause> {
        let pause = Arc::new(ConfigPause::default());
        *self.config_pause.lock().unwrap() = Some(pause.clone());
        pause
    }
    pub(crate) async fn shutdown(&self) {
        if let Some(task) = self.task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
        self.cache.lock().await.take();
    }
    fn state(&self) -> Result<Arc<AppState>> {
        self.state.upgrade().ok_or_else(unavailable)
    }
    fn key_store(&self, db: &str, slot: &str) -> std::result::Result<crewd_jev::key::KeyStore, ()> {
        if !crewd_proto::valid_remote_uuid(slot)
            || db.is_empty()
            || db.len() > 128
            || !db.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(());
        }
        #[cfg(test)]
        let force_file = self.file_keys;
        #[cfg(not(test))]
        let force_file = false;
        if cfg!(target_os = "macos") && !force_file {
            return Ok(crewd_jev::key::KeyStore::Keychain {
                service: "pendingcrew-apns".into(),
                account: format!("{db}:{slot}"),
            });
        }
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let dir = self.directory.join("notification-keys");
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
        Ok(crewd_jev::key::KeyStore::File(dir.join(slot)))
    }
    async fn load(&self, db: &str, slot: &str) -> std::result::Result<Vec<u8>, ()> {
        let store = self.key_store(db, slot)?;
        tokio::task::spawn_blocking(move || {
            let key = store.load().map_err(|_| ())?.ok_or(())?;
            STANDARD.decode(key.expose()).map_err(|_| ())
        })
        .await
        .map_err(|_| ())?
    }
    async fn save(&self, db: &str, slot: &str, bytes: &[u8]) -> std::result::Result<(), ()> {
        let key = crewd_jev::client::ApiKey::new(&STANDARD.encode(bytes)).ok_or(())?;
        let store = self.key_store(db, slot)?;
        tokio::task::spawn_blocking(move || store.store(&key).map_err(|_| ()))
            .await
            .map_err(|_| ())?
    }
    async fn ready(&self, s: &facts::Settings) -> Option<Ready> {
        let mut cache = self.cache.lock().await;
        match cache.as_ref() {
            Some(Cache::Ready(v)) if v.revision == s.revision => {
                return Some(Ready {
                    revision: v.revision,
                    sender: v.sender.clone(),
                    seal: v.seal.clone(),
                });
            }
            Some(Cache::Unavailable(revision)) if *revision == s.revision => return None,
            _ => {}
        }
        let binding = s.binding.as_ref()?;
        let key = self.load(&binding.db_id, &binding.key_slot).await;
        let seal = self.load(&binding.db_id, &binding.seal_slot).await;
        let result = match (key, seal) {
            (Ok(key), Ok(seal)) if seal.len() == 32 => crate::apns::Sender::new(
                binding.key_id.clone(),
                binding.team_id.clone(),
                binding.topic.clone(),
                env(binding.environment),
                &key,
            )
            .ok()
            .map(|sender| Ready {
                revision: s.revision,
                sender: Arc::new(sender),
                seal: Arc::new(seal),
            }),
            _ => None,
        };
        match result {
            Some(v) => {
                let result = Ready {
                    revision: v.revision,
                    sender: v.sender.clone(),
                    seal: v.seal.clone(),
                };
                *cache = Some(Cache::Ready(v));
                Some(result)
            }
            None => {
                *cache = Some(Cache::Unavailable(s.revision));
                None
            }
        }
    }
    async fn pump(&self) {
        if *self.drain.borrow() || self.state.upgrade().is_none() {
            return;
        }
        // No matching queued work => no secret read, key creation or APNs client.
        let s = self
            .db
            .call(|store| -> crewd_store::Result<Option<facts::Settings>> {
                let c = store.conn();
                let s = facts::settings(c)?;
                let pending: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM notification_outbox WHERE state='pending')",
                    [],
                    |r| r.get(0),
                )?;
                Ok((s.enabled && pending).then_some(s))
            })
            .await;
        let Ok(Ok(Some(s))) = s else { return };
        let revision = s.revision;
        let candidate = self
            .db
            .call(move |store| store.with_tx(|c| facts::peek(c, now(), revision)))
            .await;
        let Ok(Ok(Some(candidate))) = candidate else {
            return;
        };
        match self.session_current(&candidate).await {
            Some(false) => {
                let id = candidate.delivery_id;
                let _ = self
                    .db
                    .call(move |store| {
                        store.with_tx(|c| {
                            facts::suppress_unclaimed(c, id, "session_observation_expired")
                        })
                    })
                    .await;
                return;
            }
            None => return,
            Some(true) => {}
        }
        let Some(ready) = self.ready(&s).await else {
            return;
        };
        let expected_id = candidate.delivery_id;
        let session_source = matches!(
            &candidate.context.source,
            NotificationSource::CodexTurn { .. }
                | NotificationSource::ManagedResponse { .. }
                | NotificationSource::Decision { .. }
        );
        let claim = if session_source {
            let Some(state) = self.state.upgrade() else {
                return;
            };
            let Some(handle) = state.orchestra.get() else {
                return;
            };
            tokio::time::timeout(Duration::from_secs(2), handle.claim_notification(candidate))
                .await
                .ok()
                .and_then(|v| v.ok())
                .flatten()
        } else {
            self.db
                .call(move |store| {
                    store.with_tx(|c| facts::claim_checked(c, now(), revision, expected_id))
                })
                .await
                .ok()
                .and_then(|v| v.ok())
                .flatten()
        };
        let Some(claim) = claim else { return };
        let aad = match facts::token_aad(&claim.owner, &claim.installation) {
            Ok(v) => v,
            Err(_) => return,
        };
        let token = open(&ready.seal, &aad, &claim.sealed_token)
            .ok()
            .and_then(|v| String::from_utf8(v).ok());
        let allowed = {
            let id = claim.delivery_id;
            let attempt = claim.attempt.clone();
            let owner = claim.owner.clone();
            let installation = claim.installation.clone();
            let context = claim.context.clone();
            let expires = claim.expires_at;
            let revision = claim.config_revision;
            let session_generation = claim.session_generation.clone();
            self.db
                .call(move |store| {
                    facts::still_allowed(
                        store.conn(),
                        &facts::Claimed {
                            delivery_id: id,
                            session_generation,
                            attempt,
                            owner,
                            installation,
                            context,
                            expires_at: expires,
                            config_revision: revision,
                            sealed_token: Vec::new(),
                        },
                        now(),
                    )
                })
                .await
                .ok()
                .and_then(|v| v.ok())
                .unwrap_or(false)
        };
        let (outcome, reason, invalid) = if !allowed || *self.drain.borrow() {
            (
                NotificationDeliveryState::Suppressed,
                Some("authority_changed".to_string()),
                false,
            )
        } else if let Some(token) = token {
            let hint = NotificationOpenContext {
                installation_id: claim.installation.installation_id.clone(),
                delivery_id: claim.delivery_id.to_string(),
                context: claim.context.clone(),
            };
            let payload=serde_json::to_vec(&json!({"aps":{"alert":{"title":"PendingCrew","body":"有新的工作动态，请打开查看。"}},"pendingCrew":hint})).unwrap_or_default();
            let result = ready
                .sender
                .send(
                    &token,
                    &claim.attempt,
                    &payload,
                    now() / 1000,
                    claim.expires_at / 1000,
                )
                .await;
            let invalid = result.invalidates_token();
            match result {
                crate::apns::Result::Accepted => {
                    (NotificationDeliveryState::AcceptedByApns, None, false)
                }
                crate::apns::Result::Rejected(reason) => {
                    (NotificationDeliveryState::Rejected, Some(reason), invalid)
                }
                crate::apns::Result::Unknown => (NotificationDeliveryState::Unknown, None, false),
            }
        } else {
            (
                NotificationDeliveryState::Suppressed,
                Some("token_unavailable".into()),
                false,
            )
        };
        let _ = self
            .db
            .call(move |store| {
                store.with_tx(|c| {
                    facts::settle(
                        c,
                        claim.delivery_id,
                        &claim.attempt,
                        outcome,
                        reason.as_deref(),
                        invalid,
                    )
                })
            })
            .await;
    }
    /// A bounded read of the existing actor; never starts/resumes a session.
    /// None means transient/unpublished observation: leave pending until its fixed expiry.
    async fn session_current(&self, candidate: &facts::Claimed) -> Option<bool> {
        let (session, expected, decision) = match &candidate.context.source {
            NotificationSource::CodexTurn { session_id, .. } => (
                session_id.as_str(),
                candidate.session_generation.as_deref(),
                None,
            ),
            NotificationSource::ManagedResponse { session_id, .. } => (
                session_id.as_str(),
                if candidate.context.kind == NotificationKind::ResponseFailed {
                    candidate.session_generation.as_deref()
                } else {
                    None
                },
                None,
            ),
            NotificationSource::Decision {
                session_id,
                generation,
                decision_id,
            } => (
                session_id.as_str(),
                Some(generation.as_str()),
                Some(decision_id.as_str()),
            ),
            _ => return Some(true),
        };
        let state = self.state.upgrade()?;
        let Some(handle) = state.orchestra.get() else {
            return Some(false);
        };
        let read = async {
            let before = handle.public_target(session).await.ok()?;
            let live = handle.live(session).await.ok()?;
            let after = handle.public_target(session).await.ok()?;
            if before != after {
                return None;
            }
            if !live.running || expected.is_some_and(|g| g != after.generation) {
                return Some(false);
            }
            if let Some(decision) = decision {
                return Some(live.pending_decision.as_ref().is_some_and(|d|matches!(&d.kind,crewd_term::DecisionKind::RunnerRequest{request_id} if request_id==decision)));
            }
            if live.working {
                return None;
            }
            Some(true)
        };
        tokio::time::timeout(Duration::from_secs(2), read)
            .await
            .ok()
            .flatten()
    }
    async fn read<T: Send + 'static>(
        &self,
        actor: &Actor,
        method: &str,
        f: impl FnOnce(&rusqlite::Connection) -> crewd_store::Result<T> + Send + 'static,
    ) -> Result<T> {
        let state = self.state()?;
        actor.authorize(&state, method).await?;
        let actor = actor.clone();
        let method = method.to_string();
        self.db
            .call(move |store| {
                actor
                    .authorize_tx(&state, store.conn(), &method)
                    .map_err(dispatch::collab_err)?;
                f(store.conn()).map_err(store_error)
            })
            .await
            .map_err(|_| unavailable())?
    }
    fn owner(actor: &Actor) -> Result<facts::Owner> {
        match actor {
            Actor::Remote(p) => Ok(p.notification_owner()),
            Actor::Local(_) => Err(denied()),
        }
    }
    pub(crate) async fn route(&self, actor: &Actor, method: &str, p: Value) -> Result<Value> {
        match method {
            "notification.status" => {
                let s = self.read(actor, method, facts::settings).await?;
                let cache = self.cache.lock().await;
                let ready =
                    matches!(cache.as_ref(),Some(Cache::Ready(v)) if v.revision==s.revision);
                value(status(
                    &s,
                    s.enabled
                        && s.binding.is_some()
                        && !matches!(cache.as_ref(),Some(Cache::Unavailable(revision)) if *revision==s.revision),
                    if !s.enabled {
                        Some("notConfigured")
                    } else if matches!(cache.as_ref(),Some(Cache::Unavailable(revision)) if *revision==s.revision)
                    {
                        Some("credentialsUnavailable")
                    } else if !ready {
                        Some("credentialsNotLoaded")
                    } else {
                        None
                    },
                ))
            }
            "notification.configure" => self.configure(actor, parse_params(p)?).await,
            "notification.installationRegister" => self.register(actor, parse_params(p)?).await,
            "notification.installationRevoke" => self.revoke(actor, parse_params(p)?).await,
            "notification.installationGet" => {
                let owner = Self::owner(actor)?;
                let p: NotificationGetParams = parse_params(p)?;
                request_id(&p.installation_id)?;
                let id = p.installation_id;
                value(
                    self.read(actor, method, move |c| {
                        owner.authorize(c)?;
                        facts::installation(c, &owner, &id)
                    })
                    .await?,
                )
            }
            "notification.deliveryGet" => {
                let owner = Self::owner(actor)?;
                let p: NotificationDeliveryGetParams = parse_params(p)?;
                value(
                    self.read(actor, method, move |c| facts::delivery_get(c, &owner, &p))
                        .await?,
                )
            }
            "notification.deliveryList" => {
                let owner = Self::owner(actor)?;
                let p: NotificationDeliveryListParams = parse_params(p)?;
                value(
                    self.read(actor, method, move |c| facts::deliveries(c, &owner, &p))
                        .await?,
                )
            }
            "notification.requestLookup" => {
                let p: NotificationLookupParams = parse_params(p)?;
                request_id(&p.request_id)?;
                let key = match actor {
                    Actor::Remote(p) => p.notification_owner().key(),
                    Actor::Local(_) => {
                        self.read(actor, method, |c| {
                            Ok(format!(
                                "local:{}",
                                crewd_store::meta_get(c, "dbId")?.unwrap_or_default()
                            ))
                        })
                        .await?
                    }
                };
                value(
                    self.read(actor, method, move |c| {
                        facts::request_lookup(c, &key, &p.request_id)
                    })
                    .await?,
                )
            }
            _ => Err(RpcError::new(
                RpcError::METHOD_NOT_FOUND,
                "unknown notification method",
            )),
        }
    }
    async fn configure(&self, actor: &Actor, p: NotificationConfigureParams) -> Result<Value> {
        if !matches!(actor, Actor::Local(_)) {
            return Err(denied());
        }
        self.configure_as(&ConfigurationAuthority::Local(actor.clone()), p)
            .await
    }
    pub(crate) async fn admin_configure(
        &self,
        permit: crate::peer::RootAdminPermit,
        p: NotificationConfigureParams,
    ) -> Result<Value> {
        permit.check()?;
        self.configure_as(&ConfigurationAuthority::Root(permit), p)
            .await
    }
    pub(crate) async fn admin_lookup(
        &self,
        permit: crate::peer::RootAdminPermit,
        p: NotificationLookupParams,
    ) -> Result<Value> {
        request_id(&p.request_id)?;
        let authority = ConfigurationAuthority::Root(permit);
        let key = authority
            .read(self, "notification.requestLookup", |c| {
                Ok(format!(
                    "local:{}",
                    crewd_store::meta_get(c, "dbId")?.unwrap_or_default()
                ))
            })
            .await?;
        value(
            authority
                .read(self, "notification.requestLookup", move |c| {
                    facts::request_lookup(c, &key, &p.request_id)
                })
                .await?,
        )
    }
    async fn configure_as(
        &self,
        authority: &ConfigurationAuthority,
        p: NotificationConfigureParams,
    ) -> Result<Value> {
        request_id(&p.request_id)?;
        let expected = facts::number(&p.expected_revision, true).map_err(store_error)?;
        let _guard = self.mutation.lock().await;
        let hash = facts::hash(&serde_json::to_vec(&p).map_err(|_| bad())?);
        let id = p.request_id.clone();
        let operation = "notification.configure";
        let identity = authority
            .read(self, operation, crate_identity)
            .await?;
        let owner = format!("local:{}", identity.db_id);
        let key = owner.clone();
        let request = id.clone();
        let digest = hash.clone();
        let old = authority
            .read(self, operation, move |c| {
                facts::existing_request(c, &key, &request, operation, &digest)
            })
            .await?;
        if let Some(old) = old {
            return value(old);
        }
        let old = authority.read(self, operation, facts::settings).await?;
        if old.revision != expected {
            return Err(bad());
        }
        let mut binding = old.binding.clone();
        let mut provider_key = None;
        let mut new_seal = None;
        if let Some(input) = p.provider {
            let bytes = STANDARD
                .decode(input.private_key_pkcs8_base64)
                .map_err(|_| bad())?;
            let sender = crate::apns::Sender::new(
                input.key_id.clone(),
                input.team_id.clone(),
                input.topic.clone(),
                env(input.environment),
                &bytes,
            )
            .map_err(|_| bad())?;
            drop(sender);
            let seal_slot = if let Some(b) = old.binding.as_ref() {
                if b.db_id != identity.db_id || b.authority_id != identity.authority_id {
                    return Err(unavailable());
                }
                b.seal_slot.clone()
            } else {
                let mut bytes = [0u8; 32];
                SystemRandom::new()
                    .fill(&mut bytes)
                    .map_err(|_| unavailable())?;
                new_seal = Some(bytes);
                uuid::Uuid::new_v4().to_string()
            };
            binding = Some(facts::ProviderBinding {
                authority_id: identity.authority_id,
                db_id: identity.db_id.clone(),
                team_id: input.team_id,
                key_id: input.key_id,
                environment: input.environment,
                topic: input.topic,
                key_slot: uuid::Uuid::new_v4().to_string(),
                seal_slot,
            });
            provider_key = Some(bytes);
        }
        if p.enabled && binding.is_none() {
            return Err(unavailable());
        }
        let state = self.state()?;
        let actor_owned = authority.clone();
        let op = operation.to_string();
        let key = owner.clone();
        let request = id.clone();
        let digest = hash.clone();
        let reserved = self
            .db
            .call(move |store| {
                store.with_tx(|c| {
                    actor_owned.authorize_tx(&state, c, &op).map_err(|_| {
                        crewd_store::StoreError::Invalid("notification permission denied".into())
                    })?;
                    facts::reserve(c, &key, &request, &op, &digest)
                })
            })
            .await
            .map_err(|_| unavailable())?
            .map_err(store_error)?;
        if !reserved {
            return value(
                authority
                    .read(self, operation, move |c| {
                        facts::request_lookup(c, &owner, &id)
                    })
                    .await?,
            );
        }
        // Protected writes happen only after a durable original-UUID reservation.
        if let (Some(bytes), Some(b)) = (provider_key.as_ref(), binding.as_ref()) {
            self.save(&b.db_id, &b.key_slot, bytes)
                .await
                .map_err(|_| unavailable())?;
            if let Some(seal) = new_seal {
                self.save(&b.db_id, &b.seal_slot, &seal)
                    .await
                    .map_err(|_| unavailable())?
            }
        }
        #[cfg(test)]
        {
            let pause = self.config_pause.lock().unwrap().clone();
            if let Some(pause) = pause {
                pause.entered.notify_one();
                pause.release.notified().await;
            }
        }
        let state = self.state()?;
        let actor = authority.clone();
        let request = id.clone();
        let key = owner.clone();
        let enabled = p.enabled;
        let result = self
            .db
            .call(move |store| {
                store.with_tx(|c| {
                    actor.authorize_tx(&state, c, operation).map_err(|_| {
                        crewd_store::StoreError::Invalid("notification permission denied".into())
                    })?;
                    let settings = facts::configure(c, expected, enabled, binding.as_ref())?;
                    let result = json!(status(
                        &settings,
                        enabled,
                        if enabled {
                            Some("credentialsNotLoaded")
                        } else {
                            Some("notConfigured")
                        }
                    ));
                    facts::result(c, &key, &request, &result)?;
                    Ok(result)
                })
            })
            .await
            .map_err(|_| unavailable())?
            .map_err(store_error)?;
        self.cache.lock().await.take();
        value(NotificationRequestLookup::Known {
            operation: operation.into(),
            result,
        })
    }
    async fn register(&self, actor: &Actor, p: NotificationRegisterParams) -> Result<Value> {
        let owner = Self::owner(actor)?;
        request_id(&p.request_id)?;
        request_id(&p.installation_id)?;
        if !crate::apns::valid_token(&p.device_token) {
            return Err(bad());
        }
        let expected = facts::number(&p.expected_generation, true).map_err(store_error)?;
        if expected == i64::MAX {
            return Err(bad());
        }
        let hash = facts::hash(&serde_json::to_vec(&p).map_err(|_| bad())?);
        let o = owner.clone();
        let request = p.request_id.clone();
        let h = hash.clone();
        let operation = "notification.installationRegister";
        if let Some(old) = self
            .read(actor, operation, move |c| {
                o.authorize(c)?;
                facts::existing_request(c, &o.key(), &request, operation, &h)
            })
            .await?
        {
            return value(old);
        }
        let settings = self.read(actor, operation, facts::settings).await?;
        if !settings.enabled {
            return Err(unavailable());
        }
        let binding = settings.binding.as_ref().ok_or_else(unavailable)?;
        let seal = self
            .load(&binding.db_id, &binding.seal_slot)
            .await
            .map_err(|_| unavailable())?;
        let summary = NotificationInstallation {
            installation_id: p.installation_id.clone(),
            generation: (expected + 1).to_string(),
            revoked: false,
            environment: p.environment,
            topic: p.topic.clone(),
            crew_ids: p.crew_ids.clone(),
            kinds: p.kinds.clone(),
        };
        let aad = facts::token_aad(&owner, &summary).map_err(store_error)?;
        let token =
            seal_token(&seal, &aad, p.device_token.as_bytes()).map_err(|_| unavailable())?;
        let revision = settings.revision;
        let state = self.state()?;
        let actor = actor.clone();
        let request = p.request_id.clone();
        let key = owner.key();
        let result = self
            .db
            .call(move |store| {
                store.with_tx(|c| {
                    actor.authorize_tx(&state, c, operation).map_err(|_| {
                        crewd_store::StoreError::Invalid("notification permission denied".into())
                    })?;
                    if !facts::reserve(c, &key, &request, operation, &hash)? {
                        return facts::request_lookup(c, &key, &request);
                    }
                    if facts::settings(c)?.revision != revision {
                        return Err(crewd_store::StoreError::Invalid(
                            "notification configuration changed".into(),
                        ));
                    }
                    let result = json!(facts::register(c, &owner, &p, &token)?);
                    facts::result(c, &key, &request, &result)?;
                    Ok(NotificationRequestLookup::Known {
                        operation: operation.into(),
                        result,
                    })
                })
            })
            .await
            .map_err(|_| unavailable())?
            .map_err(store_error)?;
        value(result)
    }
    async fn revoke(&self, actor: &Actor, p: NotificationRevokeParams) -> Result<Value> {
        let owner = Self::owner(actor)?;
        request_id(&p.request_id)?;
        request_id(&p.installation_id)?;
        let expected = facts::number(&p.expected_generation, false).map_err(store_error)?;
        let hash = facts::hash(&serde_json::to_vec(&p).map_err(|_| bad())?);
        let key = owner.key();
        let operation = "notification.installationRevoke";
        let state = self.state()?;
        let actor = actor.clone();
        value(
            self.db
                .call(move |store| {
                    store.with_tx(|c| {
                        actor.authorize_tx(&state, c, operation).map_err(|_| {
                            crewd_store::StoreError::Invalid(
                                "notification permission denied".into(),
                            )
                        })?;
                        if !facts::reserve(c, &key, &p.request_id, operation, &hash)? {
                            return facts::request_lookup(c, &key, &p.request_id);
                        }
                        let result = json!(facts::revoke(c, &owner, &p.installation_id, expected)?);
                        facts::result(c, &key, &p.request_id, &result)?;
                        Ok(NotificationRequestLookup::Known {
                            operation: operation.into(),
                            result,
                        })
                    })
                })
                .await
                .map_err(|_| unavailable())?
                .map_err(store_error)?,
        )
    }
}
fn crate_identity(
    c: &rusqlite::Connection,
) -> crewd_store::Result<crewd_proto::RemotePublicIdentity> {
    crewd_store::remote::identity(c)?.ok_or_else(|| {
        crewd_store::StoreError::Invalid("notification paired authority not configured".into())
    })
}
fn seal_token(key: &[u8], aad: &[u8], plain: &[u8]) -> std::result::Result<Vec<u8>, ()> {
    let key =
        aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key).map_err(|_| ())?);
    let mut nonce = [0; 12];
    SystemRandom::new().fill(&mut nonce).map_err(|_| ())?;
    let mut body = plain.to_vec();
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(aad),
        &mut body,
    )
    .map_err(|_| ())?;
    Ok([nonce.to_vec(), body].concat())
}
fn open(key: &[u8], aad: &[u8], sealed: &[u8]) -> std::result::Result<Vec<u8>, ()> {
    if sealed.len() < 28 {
        return Err(());
    }
    let key =
        aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key).map_err(|_| ())?);
    let nonce: aead::Nonce =
        aead::Nonce::try_assume_unique_for_key(&sealed[..12]).map_err(|_| ())?;
    let mut body = sealed[12..].to_vec();
    Ok(key
        .open_in_place(nonce, aead::Aad::from(aad), &mut body)
        .map_err(|_| ())?
        .to_vec())
}
pub(crate) fn is_method(s: &str) -> bool {
    matches!(
        s,
        "notification.status"
            | "notification.configure"
            | "notification.installationRegister"
            | "notification.installationRevoke"
            | "notification.installationGet"
            | "notification.requestLookup"
            | "notification.deliveryList"
            | "notification.deliveryGet"
    )
}
pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, req: Request) -> Response {
    let id = req.id;
    let result = match state.notifications.get() {
        Some(runtime) => runtime.route(actor, &req.method, req.params).await,
        None => Err(unavailable()),
    };
    match result {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn actual_runtime_unknown_is_durable_before_local_h2_and_never_replays_after_reopen_rotation()
     {
        use std::collections::BTreeSet;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap();
        let database = path.join("core.db");
        let store = crewd_store::Store::open(&database).unwrap();
        let authority = uuid::Uuid::new_v4().to_string();
        let db_id = crewd_store::meta_get(store.conn(), "dbId")
            .unwrap()
            .unwrap();
        let grant = uuid::Uuid::new_v4().to_string();
        store.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at)VALUES('crew','Synthetic','t','t');UPDATE remote_configuration SET enabled=1").unwrap();
        crewd_store::remote::bind_identity(
            store.conn(),
            &crewd_proto::RemotePublicIdentity {
                authority_id: authority.clone(),
                db_id: db_id.clone(),
                certificate_der_base64: "synthetic".into(),
                spki_sha256: "a".repeat(64),
            },
            "t",
        )
        .unwrap();
        store.conn().execute("INSERT INTO remote_grants(id,authority_id,device_label,client_spki_sha256,client_certificate_der_base64,revision,created_at)VALUES(?1,?2,'Synthetic',?3,'synthetic',1,'t')",rusqlite::params![grant,authority,"b".repeat(64)]).unwrap();
        let owner = facts::Owner {
            authority_id: authority.clone(),
            db_id: db_id.clone(),
            grant_id: grant,
            grant_revision: "1".into(),
            client_pin: "b".repeat(64),
        };
        let binding = facts::ProviderBinding {
            authority_id: authority,
            db_id,
            team_id: "1234567890".into(),
            key_id: "ABCDEFGHIJ".into(),
            environment: PushEnvironment::Sandbox,
            topic: "com.example.app".into(),
            key_slot: uuid::Uuid::new_v4().to_string(),
            seal_slot: uuid::Uuid::new_v4().to_string(),
        };
        let mut registration = NotificationRegisterParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            installation_id: uuid::Uuid::new_v4().to_string(),
            expected_generation: "0".into(),
            device_token: "01ab".into(),
            environment: PushEnvironment::Sandbox,
            topic: binding.topic.clone(),
            crew_ids: vec!["crew".into()],
            kinds: vec![NotificationKind::Done],
        };
        let seal = Arc::new(vec![7; 32]);
        let summary = NotificationInstallation {
            installation_id: registration.installation_id.clone(),
            generation: "1".into(),
            revoked: false,
            environment: registration.environment,
            topic: registration.topic.clone(),
            crew_ids: registration.crew_ids.clone(),
            kinds: registration.kinds.clone(),
        };
        let encrypted =
            seal_token(&seal, &facts::token_aad(&owner, &summary).unwrap(), b"01ab").unwrap();
        let tx = store.conn().unchecked_transaction().unwrap();
        facts::configure(&tx, 0, true, Some(&binding)).unwrap();
        facts::register(&tx, &owner, &registration, &encrypted).unwrap();
        let source = NotificationSource::WorkEvent {
            event_seq: "1".into(),
            work_id: Some("W1".into()),
            ask_id: None,
            entry_no: None,
            object_id: "W20".into(),
        };
        facts::enqueue(
            &tx,
            "crew",
            NotificationKind::Done,
            source.clone(),
            None,
            now(),
        )
        .unwrap();
        tx.commit().unwrap();
        let server = crewd_remote::CertificateIdentity::generate().unwrap();
        let client = crewd_remote::CertificateIdentity::generate().unwrap();
        let pins = Arc::new(crewd_remote::LiveClientPins::default());
        pins.replace(BTreeSet::from([client.pin().into()])).unwrap();
        let mut tls = crewd_remote::server_configuration(&server, pins).unwrap();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        let mut client_tls = crewd_remote::client_configuration(&client, server.pin()).unwrap();
        client_tls.alpn_protocols = vec![b"h2".to_vec()];
        let listener = Arc::new(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap());
        let endpoint = format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        );
        let key = ring::signature::EcdsaKeyPair::generate_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &SystemRandom::new(),
        )
        .unwrap();
        let sender = Arc::new(crate::apns::Sender::local_test_sender(
            client_tls,
            endpoint,
            key.as_ref(),
        ));
        let db = Db::spawn(store).unwrap();
        let state = Arc::new(AppState::new(db.clone()));
        let runtime = Runtime::file_test_runtime(&state, path.clone());
        *runtime.cache.lock().await = Some(Cache::Ready(Ready {
            revision: 1,
            sender: sender.clone(),
            seal: seal.clone(),
        }));
        let accepted = listener.clone();
        let observed = db.clone();
        let task = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(12), async move {
                eprintln!("synthetic APNs runtime h2 phase: accept TCP");
            let (socket, _) = accepted.accept().await.unwrap();
            eprintln!("synthetic APNs runtime h2 phase: TLS handshake");
            let stream = tokio_rustls::TlsAcceptor::from(Arc::new(tls))
                .accept(socket)
                .await
                .unwrap();
            assert_eq!(stream.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
            eprintln!("synthetic APNs runtime h2 phase: h2 handshake");
            let mut connection = h2::server::handshake(stream).await.unwrap();
            eprintln!("synthetic APNs runtime h2 phase: request headers");
            let (request, _reply) = connection.accept().await.unwrap().unwrap();
            assert_eq!(request.uri().path(), "/3/device/01ab");
            let attempt = request.headers()["apns-id"].to_str().unwrap().to_string();
            let expiry = request.headers()["apns-expiration"]
                .to_str()
                .unwrap()
                .parse::<i64>()
                .unwrap();
            let (phase, stored_expiry) = observed
                .call(move |s| {
                    s.conn().query_row(
                        "SELECT state,expires_at FROM notification_outbox WHERE attempt=?1",
                        [attempt],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                    )
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(phase, "unknown");
            assert_eq!(expiry, stored_expiry / 1000);
            assert!(expiry > now() / 1000);
            let mut body = request.into_body();
            let mut payload = Vec::new();
            eprintln!("synthetic APNs runtime h2 phase: read DATA while driving Connection");
            loop {
                // h2.0.4.20 requires Connection polling for later DATA frames.
                let next = tokio::select! {
                    data = body.data() => data,
                    _ = connection.accept() => panic!("synthetic APNs runtime h2 connection ended or received another stream before request body completion"),
                };
                let Some(data) = next else { break };
                let data = data.unwrap();
                payload.extend_from_slice(&data);
                body.flow_control().release_capacity(data.len()).unwrap();
            }
            eprintln!("synthetic APNs runtime h2 phase: complete request body");
            let payload: Value = serde_json::from_slice(&payload).unwrap();
            let hint: NotificationOpenContext =
                serde_json::from_value(payload["pendingCrew"].clone()).unwrap();
            let exact = observed
                .call(move |s| {
                    s.conn().query_row(
                        "SELECT id,installation_id,context FROM notification_outbox",
                        [],
                        |r| {
                            Ok((
                                r.get::<_, i64>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, String>(2)?,
                            ))
                        },
                    )
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(hint.delivery_id, exact.0.to_string());
            assert_eq!(hint.installation_id, exact.1);
            assert_eq!(
                hint.context,
                serde_json::from_str::<NotificationContext>(&exact.2).unwrap()
            );
            // Lose the response only after reading the actual native request.
            }).await.expect("synthetic APNs runtime h2 server exceeded its 12s phase deadline");
        });
        tokio::time::timeout(Duration::from_secs(12), runtime.pump())
            .await
            .expect("synthetic APNs runtime pump exceeded its 12s phase deadline");
        task.await.unwrap();
        assert_eq!(
            db.call(|s| s
                .conn()
                .query_row("SELECT state FROM notification_outbox", [], |r| r
                    .get::<_, String>(0)))
                .await
                .unwrap()
                .unwrap(),
            "unknown"
        );
        drop(runtime);
        drop(state);
        db.shutdown();
        let store = crewd_store::Store::open(&database).unwrap();
        registration.expected_generation = "1".into();
        let summary = NotificationInstallation {
            generation: "2".into(),
            ..summary
        };
        let encrypted =
            seal_token(&seal, &facts::token_aad(&owner, &summary).unwrap(), b"01ab").unwrap();
        let tx = store.conn().unchecked_transaction().unwrap();
        facts::register(&tx, &owner, &registration, &encrypted).unwrap();
        facts::enqueue(&tx, "crew", NotificationKind::Done, source, None, now()).unwrap();
        tx.commit().unwrap();
        let db = Db::spawn(store).unwrap();
        let state = Arc::new(AppState::new(db.clone()));
        let runtime = Runtime::file_test_runtime(&state, path);
        *runtime.cache.lock().await = Some(Cache::Ready(Ready {
            revision: 1,
            sender,
            seal,
        }));
        runtime.pump().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err(),
            "a durable Unknown may not even reconnect after reopen and token rotation"
        );
        assert_eq!(
            db.call(|s| s
                .conn()
                .query_row("SELECT COUNT(*) FROM notification_outbox", [], |r| r
                    .get::<_, i64>(0)))
                .await
                .unwrap()
                .unwrap(),
            1
        );
        drop(runtime);
        drop(state);
        db.shutdown();
    }

    #[test]
    fn sealed_token_is_randomized_and_bound_to_exact_owner_generation_and_provider() {
        let key = [7; 32];
        let token = b"001122aaff";
        let aad = b"authority/db/grant/installation/1/sandbox/topic";
        let first = seal_token(&key, aad, token).unwrap();
        let second = seal_token(&key, aad, token).unwrap();
        assert_ne!(first, second);
        assert!(!first.windows(token.len()).any(|v| v == token));
        assert_eq!(open(&key, aad, &first).unwrap(), token);
        for other in [
            b"authority/db/grant/installation/2/sandbox/topic".as_slice(),
            b"other-authority/db/grant/installation/1/sandbox/topic",
        ] {
            assert!(open(&key, other, &first).is_err());
        }
        assert!(open(&[8; 32], aad, &first).is_err());
        let mut corrupt = first;
        corrupt[12] ^= 1;
        assert!(open(&key, aad, &corrupt).is_err());
        assert!(open(&key, aad, b"short").is_err());
    }
    #[test]
    fn provider_and_registration_debug_redact_input_secrets() {
        let p = NotificationProvider {
            team_id: "ABCDEFGHIJ".into(),
            key_id: "KLMNOPQRST".into(),
            environment: PushEnvironment::Sandbox,
            topic: "com.example.app".into(),
            private_key_pkcs8_base64: "SENSITIVE-APPLE-KEY".into(),
        };
        assert!(!format!("{p:?}").contains("SENSITIVE"));
        let r = NotificationRegisterParams {
            request_id: "id".into(),
            installation_id: "installation".into(),
            expected_generation: "0".into(),
            device_token: "SENSITIVE-TOKEN".into(),
            environment: PushEnvironment::Sandbox,
            topic: p.topic,
            crew_ids: vec![],
            kinds: vec![],
        };
        assert!(!format!("{r:?}").contains("SENSITIVE"));
    }
}
