//! Qualified remote Human transport. Unix process authority is never reused here.
mod identity;
mod transport;
use crate::{AppState, dispatch};
use anyhow::Result;
use crewd_proto::*;
use crewd_store::{Db, remote as facts};
use identity::Identity;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{
    net::TcpListener,
    sync::{Mutex, watch},
    task::JoinHandle,
};

#[derive(Default)]
struct ListenerState {
    endpoint: Option<RemoteEndpoint>,
    failure: Option<String>,
    task: Option<JoinHandle<()>>,
    socket: Option<Arc<TcpListener>>,
}
impl Drop for ListenerState {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
pub(crate) struct Manager {
    state: Weak<AppState>,
    db: Db,
    directory: PathBuf,
    mutation: Mutex<()>,
    identity: Mutex<Option<Arc<Identity>>>,
    listener: Mutex<ListenerState>,
    pins: Arc<crewd_remote::LiveClientPins>,
    changed: watch::Sender<u64>,
    listener_epoch: AtomicU64,
    drain: watch::Receiver<bool>,
    connections: Mutex<tokio::task::JoinSet<()>>,
}
#[derive(Clone)]
pub(crate) struct Principal {
    manager: Weak<Manager>,
    pub(crate) binding: RemoteHelloBinding,
    server_pin: String,
    client_pin: String,
    configuration_revision: String,
    listener_epoch: u64,
}
fn denied() -> RpcError {
    RpcError::new(
        RpcError::FORBIDDEN,
        "设备授权或连接作用域已失效，请在电脑上重新配对。",
    )
    .with_refusal("remote.permission_denied")
}
fn bad(message: &str) -> RpcError {
    RpcError::new(RpcError::INVALID_PARAMS, message)
}
fn revision(value: &str) -> std::result::Result<i64, RpcError> {
    let number = value.parse::<i64>().map_err(|_| bad("版本号无效"))?;
    if number <= 0 || number.to_string() != value {
        return Err(bad("版本号无效"));
    }
    Ok(number)
}
fn request_id(value: &str) -> std::result::Result<(), RpcError> {
    if !valid_remote_uuid(value) {
        return Err(bad("请求编号无效"));
    }
    Ok(())
}
fn conflict() -> RpcError {
    RpcError::new(
        RpcError::CONFLICT,
        "当前状态或请求内容已改变，请读取后再决定。",
    )
    .with_refusal("remote.revision_or_request_conflict")
}
fn store_error(error: crewd_store::StoreError) -> RpcError {
    dispatch::collab_err(crewd_collab::CollabError::Store(error))
}
fn write_error(error: crewd_store::StoreError) -> RpcError {
    match error {
        crewd_store::StoreError::Invalid(message) if message.contains("CAS conflict") => conflict(),
        crewd_store::StoreError::Invalid(_) => bad("配对或连接内容已失效，请读取后重新决定。"),
        error => store_error(error),
    }
}

impl Manager {
    pub(crate) async fn new(
        state: &Arc<AppState>,
        data_directory: PathBuf,
        drain: watch::Receiver<bool>,
    ) -> Result<Arc<Self>> {
        let (changed, _) = watch::channel(1);
        Ok(Arc::new(Self {
            state: Arc::downgrade(state),
            db: state.db.clone(),
            directory: data_directory.join("remote"),
            mutation: Mutex::new(()),
            identity: Mutex::new(None),
            listener: Mutex::new(ListenerState::default()),
            pins: Arc::new(crewd_remote::LiveClientPins::default()),
            changed,
            listener_epoch: AtomicU64::new(1),
            drain,
            connections: Mutex::new(tokio::task::JoinSet::new()),
        }))
    }
    fn state(&self) -> std::result::Result<Arc<AppState>, RpcError> {
        self.state.upgrade().ok_or_else(denied)
    }
    async fn ensure_identity(&self) -> std::result::Result<Arc<Identity>, RpcError> {
        let mut slot = self.identity.lock().await;
        if let Some(identity) = slot.as_ref() {
            return Ok(identity.clone());
        }
        let (db_id, bound, grants_exist) = self
            .db
            .call(|s| -> crewd_store::Result<_> {
                let db_id = crewd_store::meta_get(s.conn(), "dbId")?.ok_or_else(|| {
                    crewd_store::StoreError::Invalid("database identity missing".into())
                })?;
                let bound = facts::identity(s.conn())?;
                let grants: bool =
                    s.conn()
                        .query_row("SELECT EXISTS(SELECT 1 FROM remote_grants)", [], |r| {
                            r.get(0)
                        })?;
                Ok((db_id, bound, grants))
            })
            .await
            .map_err(store_error)?
            .map_err(store_error)?;
        let directory = self.directory.clone();
        let identity = tokio::task::spawn_blocking(move || {
            Identity::load_or_create(&directory, &db_id, bound.as_ref(), grants_exist)
        })
        .await
        .map_err(dispatch::internal)?
        .map_err(dispatch::internal)?;
        let public = identity.public.clone();
        self.db
            .call(move |s| {
                s.with_tx(|tx| {
                    if let Some(existing) = facts::identity(tx)? {
                        if existing != public {
                            return Err(crewd_store::StoreError::Invalid(
                                "remote identity changed during enrollment".into(),
                            ));
                        }
                    } else {
                        facts::bind_identity(tx, &public, &crewd_store::now_ts())?;
                    }
                    Ok(())
                })
            })
            .await
            .map_err(store_error)?
            .map_err(store_error)?;
        let identity = Arc::new(identity);
        *slot = Some(identity.clone());
        Ok(identity)
    }
    async fn refresh_pins(&self, authority: &str) -> std::result::Result<(), RpcError> {
        let authority = authority.to_owned();
        let grants = self
            .db
            .call(move |s| facts::grants(s.conn(), &authority))
            .await
            .map_err(store_error)?
            .map_err(store_error)?;
        self.pins
            .replace(
                grants
                    .into_iter()
                    .filter(|g| g.revoked_at.is_none())
                    .map(|g| g.client_spki_sha256)
                    .collect::<BTreeSet<_>>(),
            )
            .map_err(dispatch::internal)
    }
    pub(crate) async fn status(&self) -> std::result::Result<RemoteStatus, RpcError> {
        let (configuration, identity) = self
            .db
            .call(|s| -> crewd_store::Result<_> {
                Ok((facts::configuration(s.conn())?, facts::identity(s.conn())?))
            })
            .await
            .map_err(store_error)?
            .map_err(store_error)?;
        let listener = self.listener.lock().await;
        Ok(RemoteStatus {
            configuration,
            listening: listener.endpoint.is_some(),
            endpoint: listener.endpoint.clone(),
            identity,
            failure: listener.failure.clone(),
        })
    }
    pub(crate) async fn start_persisted(self: &Arc<Self>) {
        let result = async {
            let configuration = self
                .db
                .call(|s| facts::configuration(s.conn()))
                .await
                .map_err(store_error)?
                .map_err(store_error)?;
            if !configuration.enabled {
                return Ok(());
            }
            let identity = self.ensure_identity().await?;
            self.refresh_pins(&identity.public.authority_id).await?;
            let socket =
                TcpListener::bind((configuration.listen_host.as_str(), configuration.port))
                    .await
                    .map_err(dispatch::internal)?;
            self.install_listener(Arc::new(socket), &configuration, identity)
                .await
        }
        .await;
        if result.is_err() {
            self.listener.lock().await.failure =
                Some("远程连接未启动，请在电脑上核对连接设置或重新配对。".into());
        }
    }
    async fn install_listener(
        self: &Arc<Self>,
        socket: Arc<TcpListener>,
        configuration: &RemoteConfiguration,
        identity: Arc<Identity>,
    ) -> std::result::Result<(), RpcError> {
        let address = socket.local_addr().map_err(dispatch::internal)?;
        let tls = crewd_remote::server_configuration(&identity.certificate, self.pins.clone())
            .map_err(dispatch::internal)?;
        let epoch = self.listener_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.changed
            .send_modify(|change| *change = change.wrapping_add(1));
        let revision = configuration.revision.clone();
        let weak = Arc::downgrade(self);
        let mut listener = self.listener.lock().await;
        if let Some(previous) = listener.task.take() {
            previous.abort();
        }
        listener.endpoint = Some(RemoteEndpoint {
            host: address.ip().to_string(),
            port: address.port(),
        });
        listener.failure = None;
        listener.socket = Some(socket.clone());
        listener.task = Some(tokio::spawn(transport::listen(
            weak,
            socket,
            Arc::new(tls),
            revision,
            epoch,
        )));
        Ok(())
    }
    pub(crate) async fn stop(&self) {
        self.listener_epoch.fetch_add(1, Ordering::SeqCst);
        self.changed
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        let mut listener = self.listener.lock().await;
        if let Some(task) = listener.task.take() {
            task.abort();
        }
        listener.endpoint = None;
        listener.socket = None;
    }
    pub(crate) async fn shutdown(&self, limit: std::time::Duration) {
        // Global drain already stopped reads. Keep valid writer authority while
        // delivering owned final replies/system.shutdown, then retire the epoch.
        if let Some(task) = self.listener.lock().await.task.take() {
            task.abort();
        }
        let mut connections = self.connections.lock().await;
        if tokio::time::timeout(limit, async {
            while connections.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        }
        drop(connections);
        self.stop().await;
    }

    pub(crate) async fn local_route(
        self: &Arc<Self>,
        method: &str,
        params: Value,
    ) -> std::result::Result<Value, RpcError> {
        match method {
            "remote.status" => {
                parse_no_params(params)?;
                serde_json::to_value(self.status().await?).map_err(dispatch::internal)
            }
            "remote.grants" => {
                parse_no_params(params)?;
                // A directory read must not create private credentials or bind
                // a new authority while remote access is still disabled.
                let grants = self
                    .db
                    .call(|s| -> crewd_store::Result<_> {
                        match facts::identity(s.conn())? {
                            Some(identity) => facts::grants(s.conn(), &identity.authority_id),
                            None => Ok(Vec::new()),
                        }
                    })
                    .await
                    .map_err(store_error)?
                    .map_err(store_error)?;
                serde_json::to_value(grants).map_err(dispatch::internal)
            }
            "remote.configure" => self.configure(parse_params(params)?).await,
            "remote.invite" => self.invite(parse_params(params)?).await,
            "remote.accept" => self.accept(parse_params(params)?).await,
            "remote.revoke" => self.revoke(parse_params(params)?).await,
            _ => Err(RpcError::new(RpcError::METHOD_NOT_FOUND, "没有这个方法")),
        }
    }
    async fn configure(
        self: &Arc<Self>,
        p: RemoteConfigureParams,
    ) -> std::result::Result<Value, RpcError> {
        request_id(&p.request_id)?;
        let expected = revision(&p.expected_revision)?;
        let endpoint = RemoteEndpoint {
            host: p.listen_host.clone(),
            port: p.port,
        };
        if !endpoint.is_valid() || p.listen_host.parse::<std::net::IpAddr>().is_err() {
            return Err(bad("请选择电脑上的实际地址和有效端口"));
        }
        let _guard = self.mutation.lock().await;
        let identity = self.ensure_identity().await?;
        let authority = identity.public.authority_id.clone();
        let input = serde_json::to_string(&p).map_err(dispatch::internal)?;
        if self
            .witness(&authority, &p.request_id, "remote.configure", &input)
            .await?
            .is_some()
        {
            return serde_json::to_value(self.status().await?.configuration)
                .map_err(dispatch::internal);
        }
        // A stale CAS must not be confused with a busy listening port. Reusing
        // our own socket permits enabled->enabled edits without a trust gap.
        let current = self.status().await?.configuration;
        if current.revision != p.expected_revision {
            return Err(conflict());
        }
        let socket = if p.enabled {
            let reusable = {
                let listener = self.listener.lock().await;
                if current.enabled
                    && current.listen_host == p.listen_host
                    && current.port == p.port
                    && listener
                        .task
                        .as_ref()
                        .is_some_and(|task| !task.is_finished())
                {
                    listener.socket.clone()
                } else {
                    None
                }
            };
            Some(match reusable {
                Some(socket) => socket,
                None => Arc::new(
                    TcpListener::bind((p.listen_host.as_str(), p.port))
                        .await
                        .map_err(dispatch::internal)?,
                ),
            })
        } else {
            None
        };
        let params = p.clone();
        let authority_tx = authority.clone();
        let input_tx = input.clone();
        let configuration = self
            .db
            .call(move |s| {
                s.with_tx(|tx| {
                    if !facts::configure(
                        tx,
                        expected,
                        params.enabled,
                        &params.listen_host,
                        params.port,
                    )? {
                        return Err(crewd_store::StoreError::Invalid(
                            "remote CAS conflict".into(),
                        ));
                    }
                    let configuration = facts::configuration(tx)?;
                    facts::record_request(
                        tx,
                        &authority_tx,
                        &params.request_id,
                        "remote.configure",
                        &input_tx,
                        &serde_json::to_string(&configuration)
                            .map_err(|e| crewd_store::StoreError::Invalid(e.to_string()))?,
                        &crewd_store::now_ts(),
                    )?;
                    facts::audit(
                        tx,
                        &authority_tx,
                        None,
                        "configured",
                        Some(&params.request_id),
                        &crewd_store::now_ts(),
                        &format!("{{\"enabled\":{}}}", params.enabled),
                    )?;
                    Ok(configuration)
                })
            })
            .await
            .map_err(store_error)?
            .map_err(write_error)?;
        self.refresh_pins(&authority).await?;
        if let Some(socket) = socket {
            self.install_listener(socket, &configuration, identity)
                .await?;
        } else {
            self.stop().await;
        }
        serde_json::to_value(configuration).map_err(dispatch::internal)
    }
    async fn witness(
        &self,
        authority: &str,
        id: &str,
        method: &str,
        input: &str,
    ) -> std::result::Result<Option<Value>, RpcError> {
        let authority = authority.to_owned();
        let id = id.to_owned();
        let witness = self
            .db
            .call(move |s| facts::request_witness(s.conn(), &id, &authority))
            .await
            .map_err(store_error)?
            .map_err(store_error)?;
        match witness {
            Some(w) if w.method == method && w.input_json == input => Ok(Some(
                serde_json::from_str(&w.result_json).map_err(dispatch::internal)?,
            )),
            Some(_) => Err(conflict()),
            None => Ok(None),
        }
    }
    async fn invite(&self, p: RemoteInviteParams) -> std::result::Result<Value, RpcError> {
        request_id(&p.request_id)?;
        let label = p.device_label.clone().unwrap_or_else(|| "工作电脑".into());
        if label.trim().is_empty() || label.len() > MAX_REMOTE_LABEL_BYTES {
            return Err(bad("名称无效或过长"));
        }
        let _guard = self.mutation.lock().await;
        let identity = self.ensure_identity().await?;
        let authority = identity.public.authority_id.clone();
        let input = serde_json::to_string(&p).map_err(dispatch::internal)?;
        if let Some(witness) = self
            .witness(&authority, &p.request_id, "remote.invite", &input)
            .await?
        {
            return Ok(witness);
        }
        let status = self.status().await?;
        let endpoint = status
            .endpoint
            .filter(|_| status.configuration.enabled && status.listening)
            .ok_or_else(|| bad("请先启用电脑上的远程连接"))?;
        let now = chrono::Utc::now();
        let expires = now + chrono::Duration::minutes(5);
        let invitation = RemoteInvitation {
            invitation_id: uuid::Uuid::new_v4().to_string(),
            server: identity.public.clone(),
            endpoint,
            challenge: crewd_remote::challenge().map_err(dispatch::internal)?,
            expires_at: expires.to_rfc3339(),
            computer_label: label,
        };
        let envelope = identity
            .certificate
            .sign(&invitation)
            .map_err(dispatch::internal)?;
        let result = serde_json::to_value(&envelope).map_err(dispatch::internal)?;
        let saved = result.clone();
        let request = p.request_id.clone();
        self.db
            .call(move |s| {
                s.with_tx(|tx| {
                    facts::issue_invitation(
                        tx,
                        facts::NewInvitation {
                            id: &invitation.invitation_id,
                            authority: &authority,
                            challenge: &invitation.challenge,
                            envelope_json: &saved.to_string(),
                            expires_unix: expires.timestamp(),
                            now_unix: now.timestamp(),
                            at: &crewd_store::now_ts(),
                        },
                    )?;
                    facts::record_request(
                        tx,
                        &authority,
                        &request,
                        "remote.invite",
                        &input,
                        &saved.to_string(),
                        &crewd_store::now_ts(),
                    )?;
                    facts::audit(
                        tx,
                        &authority,
                        None,
                        "invited",
                        Some(&request),
                        &crewd_store::now_ts(),
                        "{}",
                    )?;
                    Ok(())
                })
            })
            .await
            .map_err(store_error)?
            .map_err(write_error)?;
        Ok(result)
    }
    async fn accept(&self, p: RemoteAcceptParams) -> std::result::Result<Value, RpcError> {
        request_id(&p.request_id)?;
        if !p.response_envelope.is_bounded() {
            return Err(bad("配对回复格式无效"));
        }
        let _guard = self.mutation.lock().await;
        let identity = self.ensure_identity().await?;
        let authority = identity.public.authority_id.clone();
        let input = serde_json::to_string(&p).map_err(dispatch::internal)?;
        if let Some(witness) = self
            .witness(&authority, &p.request_id, "remote.accept", &input)
            .await?
        {
            let mut result: RemoteAcceptResult =
                serde_json::from_value(witness).map_err(dispatch::internal)?;
            let id = result.grant.id.clone();
            let authority = authority.clone();
            result.grant = self
                .db
                .call(move |s| facts::grant(s.conn(), &id, &authority))
                .await
                .map_err(store_error)?
                .map_err(store_error)?
                .ok_or_else(denied)?;
            result.reused = true;
            return serde_json::to_value(result).map_err(dispatch::internal);
        }
        let bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &p.response_envelope.payload,
        )
        .map_err(|_| bad("配对回复格式无效"))?;
        if bytes.len() > MAX_REMOTE_ENVELOPE_BYTES / 2 {
            return Err(bad("配对回复过长"));
        }
        let untrusted: RemotePairingResponse =
            serde_json::from_slice(&bytes).map_err(|_| bad("配对回复格式无效"))?;
        let certificate =
            crewd_remote::decode_certificate(&untrusted.client_certificate_der_base64)
                .map_err(|_| bad("设备证书无效"))?;
        let response: RemotePairingResponse = crewd_remote::verify_envelope(
            &p.response_envelope,
            &certificate,
            crewd_remote::CertificateRole::Client,
            &untrusted.client_spki_sha256,
        )
        .map_err(|_| bad("设备配对签名无效"))?;
        if response.authority_id != authority
            || response.db_id != identity.public.db_id
            || response.device_label.trim().is_empty()
            || response.device_label.len() > MAX_REMOTE_LABEL_BYTES
        {
            return Err(bad("配对回复属于其他电脑或名称无效"));
        }
        let now = chrono::Utc::now();
        let grant_id = uuid::Uuid::new_v4().to_string();
        let receipt = identity
            .certificate
            .sign(&RemoteGrantReceipt {
                invitation_id: response.invitation_id.clone(),
                grant_id: grant_id.clone(),
                authority_id: authority.clone(),
                db_id: identity.public.db_id.clone(),
                server_spki_sha256: identity.public.spki_sha256.clone(),
                client_spki_sha256: response.client_spki_sha256.clone(),
                grant_revision: "1".into(),
                accepted_at: now.to_rfc3339(),
            })
            .map_err(dispatch::internal)?;
        let grant = RemoteGrantSummary {
            id: grant_id,
            scope: RemoteGrantScope::OwnerWorkspace,
            device_label: response.device_label.clone(),
            client_spki_sha256: response.client_spki_sha256.clone(),
            revision: "1".into(),
            created_at: now.to_rfc3339(),
            revoked_at: None,
            revoke_note: None,
        };
        let result = RemoteAcceptResult {
            request_id: p.request_id.clone(),
            reused: false,
            receipt,
            grant: grant.clone(),
        };
        let value = serde_json::to_value(&result).map_err(dispatch::internal)?;
        let saved = value.clone();
        self.db
            .call(move |s| {
                s.with_tx(|tx| {
                    let invitation = facts::invitation(tx, &response.invitation_id, &authority)?
                        .ok_or_else(|| {
                            crewd_store::StoreError::Invalid("unknown pairing invitation".into())
                        })?;
                    if invitation.challenge != response.challenge
                        || invitation.consumed_at.is_some()
                        || invitation.expires_unix <= now.timestamp()
                    {
                        return Err(crewd_store::StoreError::Invalid(
                            "expired or consumed pairing invitation".into(),
                        ));
                    }
                    facts::insert_grant(
                        tx,
                        &authority,
                        &grant,
                        &response.client_certificate_der_base64,
                    )?;
                    if !facts::consume_invitation(
                        tx,
                        &response.invitation_id,
                        &authority,
                        now.timestamp(),
                        &now.to_rfc3339(),
                    )? {
                        return Err(crewd_store::StoreError::Invalid(
                            "pairing invitation changed".into(),
                        ));
                    }
                    facts::record_request(
                        tx,
                        &authority,
                        &p.request_id,
                        "remote.accept",
                        &input,
                        &saved.to_string(),
                        &crewd_store::now_ts(),
                    )?;
                    facts::audit(
                        tx,
                        &authority,
                        Some(&grant.id),
                        "accepted",
                        Some(&p.request_id),
                        &crewd_store::now_ts(),
                        "{}",
                    )?;
                    Ok(())
                })
            })
            .await
            .map_err(store_error)?
            .map_err(write_error)?;
        self.refresh_pins(&identity.public.authority_id).await?;
        Ok(value)
    }
    async fn revoke(&self, p: RemoteRevokeParams) -> std::result::Result<Value, RpcError> {
        let expected = revision(&p.expected_revision)?;
        if p.note.trim().is_empty() || p.note.len() > 4096 {
            return Err(bad("请填写撤销理由"));
        }
        let _guard = self.mutation.lock().await;
        let identity = self.ensure_identity().await?;
        let authority = identity.public.authority_id.clone();
        let id = p.grant_id.clone();
        let grant = self
            .db
            .call(move |s| {
                s.with_tx(|tx| {
                    if !facts::revoke_grant(
                        tx,
                        &id,
                        &authority,
                        expected,
                        &p.note,
                        &crewd_store::now_ts(),
                    )? {
                        return Err(crewd_store::StoreError::Invalid(
                            "remote revoke CAS conflict".into(),
                        ));
                    }
                    facts::audit(
                        tx,
                        &authority,
                        Some(&id),
                        "revoked",
                        None,
                        &crewd_store::now_ts(),
                        "{}",
                    )?;
                    facts::grant(tx, &id, &authority)?.ok_or_else(|| {
                        crewd_store::StoreError::Invalid("remote grant missing".into())
                    })
                })
            })
            .await
            .map_err(store_error)?
            .map_err(write_error)?;
        self.refresh_pins(&identity.public.authority_id).await?;
        self.changed
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        serde_json::to_value(grant).map_err(dispatch::internal)
    }
    pub(super) async fn qualify(
        self: &Arc<Self>,
        certificate: &[u8],
        revision: &str,
        epoch: u64,
    ) -> std::result::Result<Principal, RpcError> {
        let identity = self.ensure_identity().await?;
        let pin = crewd_remote::certificate_pin(
            certificate,
            crewd_remote::CertificateRole::Client,
            crewd_remote::unix_now().map_err(dispatch::internal)?,
        )
        .map_err(|_| denied())?;
        let authority = identity.public.authority_id.clone();
        let wanted = pin.clone();
        let grant = self
            .db
            .call(move |s| facts::authenticated_grant(s.conn(), &authority, &wanted))
            .await
            .map_err(store_error)?
            .map_err(store_error)?
            .map(|g| g.0)
            .ok_or_else(denied)?;
        let state = self.state()?;
        let principal = Principal {
            manager: Arc::downgrade(self),
            binding: RemoteHelloBinding {
                authority_id: identity.public.authority_id.clone(),
                db_id: identity.public.db_id.clone(),
                runtime_binding_id: state.runtime_binding_id.clone(),
                grant_id: grant.id,
                grant_revision: grant.revision,
            },
            server_pin: identity.public.spki_sha256.clone(),
            client_pin: pin,
            configuration_revision: revision.into(),
            listener_epoch: epoch,
        };
        principal.authorize(&state, "system.hello").await?;
        Ok(principal)
    }
}
impl Principal {
    pub(crate) fn changes(&self) -> std::result::Result<watch::Receiver<u64>, RpcError> {
        Ok(self
            .manager
            .upgrade()
            .ok_or_else(denied)?
            .changed
            .subscribe())
    }
    pub(crate) async fn authorize(
        &self,
        state: &Arc<AppState>,
        method: &str,
    ) -> std::result::Result<(), RpcError> {
        if !allowed_method(method) {
            return Err(denied());
        }
        self.check_live(state).await
    }
    pub(crate) async fn check_live(
        &self,
        state: &Arc<AppState>,
    ) -> std::result::Result<(), RpcError> {
        let manager = self.manager.upgrade().ok_or_else(denied)?;
        let owner = manager.state()?;
        if !Arc::ptr_eq(state, &owner)
            || state.runtime_binding_id != self.binding.runtime_binding_id
            || manager.listener_epoch.load(Ordering::SeqCst) != self.listener_epoch
        {
            return Err(denied());
        }
        let this = self.clone();
        let grant = manager
            .db
            .call(move |s| {
                facts::live_grant(
                    s.conn(),
                    &this.binding.authority_id,
                    &this.binding.db_id,
                    &this.server_pin,
                    &this.client_pin,
                    &this.binding.grant_id,
                    &this.configuration_revision,
                )
            })
            .await
            .map_err(store_error)?
            .map_err(store_error)?;
        if grant.is_none() {
            return Err(denied());
        }
        Ok(())
    }
}
fn allowed_method(method: &str) -> bool {
    matches!(
        method,
        "system.hello"
            | "events.subscribe"
            | "crew.list"
            | "crew.get"
            | "crew.create"
            | "crew.update"
            | "crew.adopt"
            | "crew.reorder"
            | "crew.release"
            | "crew.remove"
            | "crew.restore"
            | "crew.purge"
            | "channel.list"
            | "channel.create"
            | "channel.update"
            | "channel.addMembers"
            | "channel.removeMember"
            | "channel.watch"
            | "channel.setOwner"
            | "channel.workList"
            | "channel.workLink"
            | "channel.workUnlink"
            | "message.list"
            | "message.post"
            | "message.search"
            | "read.mark"
            | "session.list"
            | "session.get"
            | "session.usage"
            | "session.inspect"
            | "session.start"
            | "session.stop"
            | "session.interrupt"
            | "session.resume"
            | "session.answer"
            | "session.respondRunnerRequest"
            | "todo.list"
            | "todo.create"
            | "todo.get"
            | "todo.close"
            | "work.list"
            | "work.get"
            | "work.create"
            | "work.update"
            | "work.block"
            | "work.assign"
            | "work.reassignLost"
            | "work.history"
            | "work.patrol"
            | "work.transferWithdraw"
            | "ask.list"
            | "ask.get"
            | "ask.answer"
            | "ask.withdraw"
            | "restart.pending"
            | "restart.answer"
            | "directory.list"
            | "document.create"
            | "document.get"
            | "document.list"
            | "document.update"
            | "document.delete"
            | "viewFolder.list"
            | "viewFolder.create"
            | "viewFolder.update"
            | "viewFolder.delete"
            | "viewFolder.assign"
    )
}
