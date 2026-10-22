//! Independent actual service flow; only temporary loopback credentials/data.
#![cfg(feature = "testkit")]
use crewd_proto::{
    RemoteAcceptResult, RemoteInvitation, RemotePairingResponse, RemotePublicIdentity,
    RemoteSignedEnvelope,
};
use crewd_remote::{CertificateIdentity, CertificateRole, decode_certificate, verify_envelope};
use crewd_server::agent::testkit::RpcClient;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
const LIMIT: Duration = Duration::from_secs(10);
async fn local(c: &mut RpcClient, method: &str, params: Value) -> Value {
    tokio::time::timeout(LIMIT, c.call(method, params))
        .await
        .unwrap()
        .unwrap()
}
struct RemoteRpc {
    io: BufReader<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
    next: u64,
    events: Vec<Value>,
}
impl RemoteRpc {
    async fn connect(port: u16, identity: &CertificateIdentity, server_pin: &str) -> Self {
        let config = crewd_remote::client_configuration(identity, server_pin).unwrap();
        let socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let stream = tokio::time::timeout(
            LIMIT,
            tokio_rustls::TlsConnector::from(Arc::new(config)).connect(
                tokio_rustls::rustls::pki_types::ServerName::try_from("pendingcrew.invalid")
                    .unwrap(),
                socket,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        Self {
            io: BufReader::new(stream),
            next: 1,
            events: Vec::new(),
        }
    }
    async fn connect_enrollment(port: u16, pin: &str) -> Self {
        let config = crewd_remote::enrollment_client_configuration(pin).unwrap();
        let socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let stream = tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(
                rustls::pki_types::ServerName::try_from("pendingcrew.invalid").unwrap(),
                socket,
            )
            .await
            .unwrap();
        assert_eq!(
            stream.get_ref().1.alpn_protocol(),
            Some(crewd_proto::ENROLLMENT_ALPN)
        );
        Self {
            io: BufReader::new(stream),
            next: 1,
            events: Vec::new(),
        }
    }
    async fn frame(&mut self, method: &str, params: Value) -> std::io::Result<Value> {
        let id = self.next;
        self.next += 1;
        let mut bytes =
            serde_json::to_vec(&json!({"id":id,"method":method,"params":params})).unwrap();
        bytes.push(b'\n');
        self.io.get_mut().write_all(&bytes).await?;
        loop {
            let mut line = String::new();
            let n = self.io.read_line(&mut line).await?;
            if n == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
            }
            let value: Value = serde_json::from_str(&line).map_err(std::io::Error::other)?;
            if value["id"] == id {
                return Ok(value);
            }
            self.events.push(value);
        }
    }
    async fn event(&mut self) -> Value {
        let mut line = String::new();
        let n = tokio::time::timeout(LIMIT, self.io.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert!(n > 0, "expected actual subscribed event before revoke");
        serde_json::from_str(&line).unwrap()
    }
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let frame = tokio::time::timeout(LIMIT, self.frame(method, params))
            .await
            .unwrap()
            .unwrap();
        assert!(frame.get("error").is_none(), "{method}: {frame}");
        frame["result"].clone()
    }
}

#[tokio::test]
async fn public_pairing_actual_tls_work_flow_and_online_revoke() {
    let root = tempfile::tempdir().unwrap().keep();
    eprintln!("Remote service evidence: {}", root.display());
    std::fs::create_dir(root.join("work")).unwrap();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let server_root = root.clone();
    let server = tokio::spawn(async move {
        crewd_server::serve_with(server_root, Default::default(), async {
            let _ = stopping.await;
        })
        .await
    });
    let socket = root.join("crewd.sock");
    for _ in 0..400 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(socket.exists());
    let mut owner = RpcClient::connect(&socket).await;
    let before = local(&mut owner, "remote.status", json!({})).await;
    assert_eq!(before["listening"], false);
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reserved.local_addr().unwrap().port();
    drop(reserved);
    local(&mut owner,"remote.configure",json!({"requestId":uuid::Uuid::new_v4().to_string(),"expectedRevision":before["configuration"]["revision"],"enabled":true,"listenHost":"127.0.0.1","port":port})).await;
    let status = local(&mut owner, "remote.status", json!({})).await;
    assert_eq!(status["listening"], true);
    let identity: RemotePublicIdentity =
        serde_json::from_value(status["identity"].clone()).unwrap();
    let server_cert = decode_certificate(&identity.certificate_der_base64).unwrap();
    let invite: RemoteSignedEnvelope = serde_json::from_value(local(&mut owner,"remote.invite",json!({"requestId":uuid::Uuid::new_v4().to_string(),"deviceLabel":"Independent loopback client"})).await).unwrap();
    let invitation: RemoteInvitation = verify_envelope(
        &invite,
        &server_cert,
        CertificateRole::Server,
        &identity.spki_sha256,
    )
    .unwrap();
    assert_eq!(invitation.server, identity);
    let client = CertificateIdentity::generate().unwrap();
    let response = client
        .sign(&RemotePairingResponse {
            invitation_id: invitation.invitation_id,
            authority_id: identity.authority_id.clone(),
            db_id: identity.db_id.clone(),
            challenge: invitation.challenge,
            client_certificate_der_base64: client.certificate_base64(),
            client_spki_sha256: client.pin().into(),
            device_label: "Independent loopback client".into(),
        })
        .unwrap();
    let acceptance =
        json!({"requestId":uuid::Uuid::new_v4().to_string(),"responseEnvelope":response});
    let accepted: RemoteAcceptResult =
        serde_json::from_value(local(&mut owner, "remote.accept", acceptance.clone()).await)
            .unwrap();
    assert!(!accepted.reused);
    let confirmed: crewd_proto::RemoteGrantReceipt = verify_envelope(
        &accepted.receipt,
        &server_cert,
        CertificateRole::Server,
        &identity.spki_sha256,
    )
    .unwrap();
    assert_eq!(confirmed.client_spki_sha256, client.pin());
    assert_eq!(confirmed.grant_id, accepted.grant.id);
    let repeated: RemoteAcceptResult =
        serde_json::from_value(local(&mut owner, "remote.accept", acceptance).await).unwrap();
    assert!(repeated.reused);
    assert_eq!(repeated.grant.id, accepted.grant.id);
    let current_grants = local(&mut owner, "remote.grants", json!({})).await;
    let replay = tokio::time::timeout(
        LIMIT,
        owner.call(
            "remote.accept",
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"responseEnvelope":response}),
        ),
    )
    .await
    .unwrap();
    assert!(
        replay.is_err(),
        "a consumed invitation must not authorize another fresh request"
    );
    assert_eq!(
        local(&mut owner, "remote.grants", json!({})).await,
        current_grants
    );
    let mut remote = RemoteRpc::connect(port, &client, &identity.spki_sha256).await;
    let hello = remote
        .call(
            "system.hello",
            json!({"apiVersion":3,"capabilities":["agent.attach","remote.configure"]}),
        )
        .await;
    let notification = remote.call("notification.status", json!({})).await;
    assert_eq!(notification["enabled"], false);
    assert_eq!(notification["available"], false);
    assert!(!root.join("notification-keys").exists());
    assert!(
        remote
            .call(
                "notification.deliveryGet",
                json!({"installationId":uuid::Uuid::new_v4().to_string(),"deliveryId":"1"})
            )
            .await
            .is_null()
    );
    let forbidden=remote.frame("notification.configure",json!({"requestId":uuid::Uuid::new_v4().to_string(),"expectedRevision":"0","enabled":false,"provider":null})).await.unwrap();
    assert_eq!(forbidden["error"]["code"], crewd_proto::RpcError::FORBIDDEN);
    let notification_request = uuid::Uuid::new_v4().to_string();
    let registration=remote.frame("notification.installationRegister",json!({"requestId":notification_request,"installationId":uuid::Uuid::new_v4().to_string(),"expectedGeneration":"0","deviceToken":"01ab","environment":"sandbox","topic":"com.example.app","crewIds":["home"],"kinds":["replyReady"]})).await.unwrap();
    assert!(
        registration.get("error").is_some(),
        "unconfigured source cannot register or load credentials"
    );
    assert_eq!(
        remote
            .call(
                "notification.requestLookup",
                json!({"requestId":notification_request})
            )
            .await["kind"],
        "notFound"
    );
    assert!(!root.join("notification-keys").exists());
    assert_eq!(hello["server"], "crewd");
    assert_eq!(hello["remoteBinding"]["authorityId"], identity.authority_id);
    assert_eq!(hello["remoteBinding"]["dbId"], identity.db_id);
    assert_eq!(hello["remoteBinding"]["grantId"], accepted.grant.id);
    assert_eq!(
        hello["remoteBinding"]["grantRevision"],
        accepted.grant.revision
    );
    assert!(
        uuid::Uuid::parse_str(hello["remoteBinding"]["runtimeBindingId"].as_str().unwrap()).is_ok()
    );
    let created = remote.call("crew.create",json!({"title":"Remote-owned synthetic crew","workingDirectory":root.join("work").to_str().unwrap()})).await;
    let crew = created["id"].as_str().unwrap();
    let requirement = remote.call("todo.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":crew,"text":"Actual remote principal requirement"})).await;
    assert_eq!(
        remote
            .call("todo.get", json!({"id":requirement["requirementId"]}))
            .await["text"],
        "Actual remote principal requirement"
    );
    for method in ["agent.attach", "remote.status", "remote.configure"] {
        let frame = tokio::time::timeout(LIMIT, remote.frame(method, json!({})))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            frame["error"]["code"],
            crewd_proto::RpcError::FORBIDDEN,
            "remote owner must be refused before parsing local-only {method}: {frame}"
        );
    }
    remote
        .call("events.subscribe", json!({"topics":["work"]}))
        .await;
    // Transport event coverage uses the active work API. Legacy document.create
    // is intentionally guarded in normal serve; knowledge is a separate source.
    let visible_work = local(
        &mut owner,
        "work.create",
        json!({"crewId":crew,"title":"Before revoke visible"}),
    )
    .await;
    let event = remote.event().await;
    assert_eq!(event["event"], "work.changed");
    assert_eq!(event["data"]["id"], visible_work["id"]);
    local(&mut owner,"remote.revoke",json!({"grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"Online revoke"})).await;
    let hidden_work = local(
        &mut owner,
        "work.create",
        json!({"crewId":crew,"title":"After revoke must not leak"}),
    )
    .await;
    let next = tokio::time::timeout(
        LIMIT,
        remote.frame("todo.get", json!({"id":requirement["requirementId"]})),
    )
    .await
    .unwrap();
    assert!(
        next.is_err() || next.unwrap().get("error").is_some(),
        "existing TLS connection survived durable revoke as a work principal"
    );
    assert!(
        !remote
            .events
            .iter()
            .any(|event| event["event"] == "work.changed"
                && event["data"]["id"] == hidden_work["id"]),
        "revoked connection leaked a new authoritative event"
    );
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

struct Service {
    owner: RpcClient,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}
async fn service(root: &std::path::Path) -> Service {
    let root = root.to_owned();
    let socket = root.join("crewd.sock");
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        crewd_server::serve_with(root, Default::default(), async {
            let _ = stopping.await;
        })
        .await
    });
    for _ in 0..400 {
        if socket.exists() && tokio::net::UnixStream::connect(&socket).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let owner = RpcClient::connect(&socket).await;
    Service { owner, stop, task }
}
async fn stopped(service: Service) {
    drop(service.owner);
    service.stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, service.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
async fn enabled(owner: &mut RpcClient) -> (u16, RemotePublicIdentity) {
    let before = local(owner, "remote.status", json!({})).await;
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    local(owner,"remote.configure",json!({"requestId":uuid::Uuid::new_v4().to_string(),"expectedRevision":before["configuration"]["revision"],"enabled":true,"listenHost":"127.0.0.1","port":port})).await;
    let status = local(owner, "remote.status", json!({})).await;
    assert_eq!(status["listening"], true);
    (
        port,
        serde_json::from_value(status["identity"].clone()).unwrap(),
    )
}
async fn approved(
    owner: &mut RpcClient,
    identity: &RemotePublicIdentity,
) -> (CertificateIdentity, Value, RemoteAcceptResult) {
    let envelope: RemoteSignedEnvelope = serde_json::from_value(
        local(
            owner,
            "remote.invite",
            json!({"requestId":uuid::Uuid::new_v4().to_string()}),
        )
        .await,
    )
    .unwrap();
    let invitation: RemoteInvitation = verify_envelope(
        &envelope,
        &decode_certificate(&identity.certificate_der_base64).unwrap(),
        CertificateRole::Server,
        &identity.spki_sha256,
    )
    .unwrap();
    let client = CertificateIdentity::generate().unwrap();
    let response = client
        .sign(&RemotePairingResponse {
            invitation_id: invitation.invitation_id,
            authority_id: identity.authority_id.clone(),
            db_id: identity.db_id.clone(),
            challenge: invitation.challenge,
            client_certificate_der_base64: client.certificate_base64(),
            client_spki_sha256: client.pin().into(),
            device_label: "Restore loopback device".into(),
        })
        .unwrap();
    let input = json!({"requestId":uuid::Uuid::new_v4().to_string(),"responseEnvelope":response});
    let accepted =
        serde_json::from_value(local(owner, "remote.accept", input.clone()).await).unwrap();
    (client, input, accepted)
}

#[tokio::test]
async fn remote_human_knowledge_recovery_routes_keep_context_and_live_grant() {
    let root = tempfile::tempdir().unwrap().keep();
    eprintln!("Remote knowledge evidence: {}", root.display());
    std::fs::create_dir(root.join("work")).unwrap();
    let mut running = service(&root).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (client, _, accepted) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &client, &identity.spki_sha256).await;
    remote.call("system.hello", json!({"apiVersion":3})).await;
    let crew = remote
        .call(
            "crew.create",
            json!({
                "title":"Remote knowledge", "workingDirectory":root.join("work")
            }),
        )
        .await;
    let workspace = remote
        .call("data.workspace.get", json!({"crewId":crew["id"]}))
        .await;
    assert_eq!(workspace["state"], "uninitialized");
    let original = workspace["context"].clone();
    let applied = remote
        .call(
            "data.submit",
            json!({
                "requestId":uuid::Uuid::new_v4().to_string(), "context":original,
                "change":{"kind":"apply", "message":"First authorized remote knowledge", "edits":[
                    {"kind":"createBlock", "alias":"shared", "contentType":"text/plain",
                     "content":"persistent knowledge", "sources":[], "attributes":{}}
                ]}
            }),
        )
        .await;
    assert_eq!(applied["phase"], "committed");
    let current = applied["context"].clone();
    let preview = remote.call("data.preview", json!({"context":current,
        "statements":[{"sql":"CREATE TABLE remote_items(id INTEGER PRIMARY KEY)","params":[]}]
    })).await;
    let readback = remote
        .call(
            "data.preview.get",
            json!({
                "context":current, "token":preview["token"]
            }),
        )
        .await;
    assert_eq!(readback, preview);
    let empty = remote
        .call("data.snapshots", json!({"context":current}))
        .await;
    assert!(empty["items"].as_array().unwrap().is_empty());
    // A remotely reachable method still requires the exact current knowledge context.
    let mut foreign = current.clone();
    foreign["authorityId"] = json!("foreign-authority");
    let refused = remote
        .frame(
            "data.confirm",
            json!({
                "requestId":uuid::Uuid::new_v4().to_string(), "context":foreign,
                "token":preview["token"]
            }),
        )
        .await
        .unwrap();
    assert!(
        refused.get("error").is_some(),
        "foreign context admitted: {refused}"
    );
    let confirm_id = uuid::Uuid::new_v4().to_string();
    let confirm_args = json!({"requestId":confirm_id,"context":current,"token":preview["token"]});
    let confirmed = remote.call("data.confirm", confirm_args.clone()).await;
    assert_eq!(confirmed["phase"], "committed");
    assert_eq!(remote.call("data.confirm", confirm_args).await, confirmed);
    let after = confirmed["context"].clone();
    let snapshots = remote
        .call("data.snapshots", json!({"context":after}))
        .await;
    assert_eq!(snapshots["items"].as_array().unwrap().len(), 1);
    let restore_id = uuid::Uuid::new_v4().to_string();
    let restore_args = json!({"requestId":restore_id,"context":after,
        "snapshotId":snapshots["items"][0]["id"]});
    let restored = remote.call("data.restore", restore_args.clone()).await;
    assert_eq!(restored["phase"], "committed");
    assert_ne!(restored["context"]["restoreEpoch"], after["restoreEpoch"]);
    let historical = remote.call("data.restore", restore_args).await;
    assert_eq!(historical["phase"], "historical");
    assert_eq!(historical["context"], restored["context"]);
    let schema = remote
        .call(
            "data.schema.get",
            json!({"context":restored["context"],
        "atRevision":null,"after":null,"limit":100}),
        )
        .await;
    assert!(
        schema["objects"].as_array().unwrap().is_empty(),
        "restore lost snapshot boundary"
    );
    let stale = remote
        .frame(
            "data.preview.get",
            json!({
                "context":current, "token":preview["token"]
            }),
        )
        .await
        .unwrap();
    assert!(
        stale.get("error").is_some(),
        "restore revived a stale preview"
    );
    // The new allowlist entries do not expose local daemon administration.
    let forbidden = remote
        .frame("lifecycle.exit", json!({"mode":"direct"}))
        .await
        .unwrap();
    assert_eq!(forbidden["error"]["code"], crewd_proto::RpcError::FORBIDDEN);
    local(
        &mut running.owner,
        "remote.revoke",
        json!({"grantId":accepted.grant.id,
        "expectedRevision":accepted.grant.revision,"note":"Remote knowledge finished"}),
    )
    .await;
    let post_revoke = tokio::time::timeout(
        LIMIT,
        remote.frame("data.snapshots", json!({"context":restored["context"]})),
    )
    .await
    .unwrap();
    assert!(
        post_revoke.is_err() || post_revoke.unwrap().get("error").is_some(),
        "revoked Human retained knowledge access"
    );
    // Revoking the transport leaves persisted knowledge available to the local Human.
    let local_snapshots = local(
        &mut running.owner,
        "data.snapshots",
        json!({"context":restored["context"]}),
    )
    .await;
    assert_eq!(local_snapshots["items"].as_array().unwrap().len(), 2);
    std::fs::write(
        root.join("knowledge-result.json"),
        serde_json::to_vec_pretty(&json!({
            "preview":readback,"confirmed":confirmed,"restored":restored,
            "snapshots":local_snapshots,"revoked":true
        }))
        .unwrap(),
    )
    .unwrap();
    drop(remote);
    stopped(running).await;
}

#[tokio::test]
async fn public_pair_backup_revoke_restore_same_server_identity_never_revives_old_certificate_or_accept_witness()
 {
    let root = tempfile::tempdir().unwrap().keep();
    eprintln!("Public remote restore evidence: {}", root.display());
    let mut original = service(&root).await;
    let (port, identity) = enabled(&mut original.owner).await;
    let (client, accept_input, accepted) = approved(&mut original.owner, &identity).await;
    let mut connected = RemoteRpc::connect(port, &client, &identity.spki_sha256).await;
    connected.call("system.hello", json!({})).await;
    let backup = root.join("old-active-backup.db");
    crewd_store::backup::backup_db(&root.join(crewd_store::DB_FILE), &backup).unwrap();
    let old_bytes = std::fs::read(&backup).unwrap();
    let key_bytes = std::fs::read(root.join("remote/identity.json")).unwrap();
    local(&mut original.owner,"remote.revoke",json!({"grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"Actual pre-restore revoke"})).await;
    stopped(original).await;
    let lock = crewd_server::lock_data_dir(&root).unwrap();
    crewd_store::backup::restore(&root, &backup).unwrap();
    drop(lock);
    assert_eq!(std::fs::read(&backup).unwrap(), old_bytes);
    assert_eq!(
        std::fs::read(root.join("remote/identity.json")).unwrap(),
        key_bytes
    );
    let mut restored = service(&root).await;
    let status = local(&mut restored.owner, "remote.status", json!({})).await;
    assert_eq!(status["configuration"]["enabled"], false);
    assert_eq!(status["listening"], false);
    let confirmed: RemoteAcceptResult =
        serde_json::from_value(local(&mut restored.owner, "remote.accept", accept_input).await)
            .unwrap();
    assert!(
        confirmed.reused && confirmed.grant.revoked_at.is_some(),
        "old accept witness must return the current tombstone"
    );
    assert_eq!(confirmed.grant.id, accepted.grant.id);
    let (new_port, new_identity) = enabled(&mut restored.owner).await;
    assert_eq!(
        new_identity, identity,
        "denial must not be explained by a changed db/server key"
    );
    let socket = tokio::net::TcpStream::connect(("127.0.0.1", new_port))
        .await
        .unwrap();
    let config = crewd_remote::client_configuration(&client, &identity.spki_sha256).unwrap();
    let handshake = tokio::time::timeout(
        LIMIT,
        tokio_rustls::TlsConnector::from(Arc::new(config)).connect(
            tokio_rustls::rustls::pki_types::ServerName::try_from("pendingcrew.invalid").unwrap(),
            socket,
        ),
    )
    .await
    .unwrap();
    let old_certificate_is_denied = match handshake {
        Err(_) => true,
        Ok(stream) => {
            let mut attempted = RemoteRpc {
                io: BufReader::new(stream),
                next: 1,
                events: Vec::new(),
            };
            let reply = tokio::time::timeout(LIMIT, attempted.frame("system.hello", json!({})))
                .await
                .unwrap();
            reply.is_err() || reply.unwrap().get("error").is_some()
        }
    };
    assert!(
        old_certificate_is_denied,
        "old certificate silently revived after an actual same-identity restore"
    );
    assert_eq!(std::fs::read(&backup).unwrap(), old_bytes);
    stopped(restored).await;
}

async fn durable_state(f: &crewd_server::agent::testkit::Fixture) -> Vec<String> {
    f.state
        .db
        .call(|store| {
            let mut names = store
                .conn()
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
            let tables = names
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut facts = Vec::new();
            for table in tables {
                assert!(table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
                let mut statement = store
                    .conn()
                    .prepare(&format!("SELECT * FROM \"{table}\""))?;
                let columns = statement.column_count();
                let mut rows = statement
                    .query_map([], |row| {
                        let mut values = Vec::new();
                        for column in 0..columns {
                            values.push(format!("{:?}", row.get_ref(column)?));
                        }
                        Ok(values.join("|"))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows.sort();
                facts.push(table);
                facts.extend(rows);
            }
            Ok::<_, crewd_store::StoreError>(facts)
        })
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn attached_agent_cannot_use_local_remote_administration_as_human() {
    use crewd_server::agent::testkit::Fixture;
    let fixture = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let server = fixture.spawn_owned_socket(stopping).await;
    let mut agent = RpcClient::connect(&fixture.data_dir().join("crewd.sock")).await;
    local(
        &mut agent,
        "agent.attach",
        json!({"sessionId":fixture.worker,"token":fixture.token(&fixture.worker)}),
    )
    .await;
    let before = durable_state(&fixture).await;
    for method in [
        "remote.status",
        "remote.grants",
        "remote.configure",
        "remote.invite",
        "remote.accept",
        "remote.revoke",
    ] {
        let error = tokio::time::timeout(LIMIT, agent.call(method, json!({})))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error.0,
            crewd_proto::RpcError::FORBIDDEN,
            "attached direct admin {method} must reject before parsing"
        );
    }
    assert_eq!(
        durable_state(&fixture).await,
        before,
        "denied admin cannot allocate IDs, events, wakes, witnesses or facts"
    );
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, server).await.unwrap().unwrap();
}

#[tokio::test]
#[ignore = "requires explicit frozen fake executor/helper paths; no real executor fallback"]
async fn paired_remote_keeps_high_level_fake_session_approval_stop_and_resume_flow() {
    use crewd_server::{OrchestraServe, ServeOptions};
    let fake = std::path::PathBuf::from(
        std::env::var("PENDINGCREW_QA_FAKE_CODEX_PROGRAM")
            .expect("explicit synthetic executor required"),
    );
    let helper = std::path::PathBuf::from(
        std::env::var("PENDINGCREW_QA_HELPER_PROGRAM").expect("explicit frozen helper required"),
    );
    assert!(fake.is_absolute() && helper.is_absolute());
    let measured = std::process::Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(&fake)
        .output()
        .unwrap();
    assert!(measured.status.success());
    assert_eq!(
        String::from_utf8(measured.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
        std::env::var("PENDINGCREW_QA_FAKE_CODEX_SHA256")
            .expect("verified synthetic digest required")
    );
    let root = tempfile::tempdir().unwrap().keep();
    eprintln!("Remote high-level fake evidence: {}", root.display());
    std::fs::create_dir(root.join("home")).unwrap();
    std::fs::create_dir(root.join("work")).unwrap();
    let mut orchestra = OrchestraServe::default();
    orchestra.codex = crewd_server::orchestra::exec_codex::CodexProgram::Path(fake);
    orchestra.claude =
        crewd_server::orchestra::exec_claude::ClaudeProgram::Path("/usr/bin/false".into());
    orchestra.helper = Some(helper);
    orchestra.codex_env = std::collections::BTreeMap::from([
        (
            "HOME".into(),
            root.join("home").to_string_lossy().into_owned(),
        ),
        ("SHELL".into(), "/usr/bin/false".into()),
        ("FAKE_CODEX_MODE".into(), "human_requests".into()),
        (
            "FAKE_CODEX_LOG".into(),
            root.join("fake-requests.jsonl")
                .to_string_lossy()
                .into_owned(),
        ),
    ]);
    let options = ServeOptions {
        orchestra: Some(orchestra),
        ..Default::default()
    };
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let server_root = root.clone();
    let task = tokio::spawn(async move {
        crewd_server::serve_with(server_root, options, async {
            let _ = stopping.await;
        })
        .await
    });
    for _ in 0..400 {
        if root.join("crewd.sock").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut owner = RpcClient::connect(&root.join("crewd.sock")).await;
    let (port, identity) = enabled(&mut owner).await;
    let (client, _, _) = approved(&mut owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &client, &identity.spki_sha256).await;
    remote.call("system.hello", json!({})).await;
    let crew = remote.call("crew.create",json!({"title":"Remote fake session crew","workingDirectory":root.join("work").to_str().unwrap()})).await;
    let session = remote.call("session.start",json!({"crewId":crew["id"],"role":"captain","executor":"codex","brief":"REMOTE_FAKE_ONLY_APPROVAL_TASK","isolation":false})).await;
    let id = session["id"].as_str().unwrap();
    async fn request(remote: &mut RemoteRpc, id: &str, method: &str) -> Value {
        let until = tokio::time::Instant::now() + LIMIT;
        loop {
            let view = remote
                .call("session.inspect", json!({"sessionId":id}))
                .await;
            if view["runnerRequest"]["method"] == method {
                return view["runnerRequest"].clone();
            }
            assert!(
                tokio::time::Instant::now() < until,
                "actual fake runner request {method} missing"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    let approval = request(&mut remote, id, "item/commandExecution/requestApproval").await;
    assert_eq!(
        remote
            .call(
                "session.answer",
                json!({"sessionId":id,"fingerprint":approval["fingerprint"],"choice":0})
            )
            .await["cleared"],
        true
    );
    let question = request(&mut remote, id, "item/tool/requestUserInput").await;
    assert_eq!(remote.call("session.respondRunnerRequest",json!({"sessionId":id,"fingerprint":question["fingerprint"],"response":{"answers":{"task":{"answers":["SYNTHETIC_TASK"]}}}})).await["cleared"],true);
    remote
        .call(
            "session.stop",
            json!({"sessionId":id,"reason":"Synthetic remote stop acceptance"}),
        )
        .await;
    let until = tokio::time::Instant::now() + LIMIT;
    loop {
        let view = remote
            .call("session.inspect", json!({"sessionId":id}))
            .await;
        if view["live"]["running"] == false {
            break;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "actual stopped fake session still running"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    remote.call("session.resume", json!({"sessionId":id})).await;
    let until = tokio::time::Instant::now() + LIMIT;
    loop {
        let view = remote
            .call("session.inspect", json!({"sessionId":id}))
            .await;
        if view["live"]["running"] == true {
            break;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "actual fake session failed resume"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    remote
        .call(
            "session.stop",
            json!({"sessionId":id,"reason":"Synthetic remote stop acceptance"}),
        )
        .await;
    drop(remote);
    drop(owner);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        std::fs::read_to_string(root.join("fake-requests.jsonl"))
            .unwrap()
            .contains("REMOTE_FAKE_ONLY_APPROVAL_TASK")
    );
}

#[tokio::test]
async fn admitted_tls_write_without_consuming_receipt_survives_revoke_without_grant_revival() {
    let root = tempfile::tempdir().unwrap().keep();
    eprintln!("Remote admitted-write evidence: {}", root.display());
    let mut running = service(&root).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (client, original_accept, accepted) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &client, &identity.spki_sha256).await;
    remote.call("system.hello", json!({})).await;
    std::fs::create_dir(root.join("work")).unwrap();
    let crew = local(
        &mut running.owner,
        "crew.create",
        json!({"title":"Unknown admitted TLS write","workingDirectory":root.join("work").to_str().unwrap()}),
    )
    .await;
    let request_id = uuid::Uuid::new_v4().to_string();
    let params = json!({"requestId":request_id,"crewId":crew["id"],"text":"TLS write committed but no receipt consumed"});
    let mut wire =
        serde_json::to_vec(&json!({"id":remote.next,"method":"todo.create","params":params}))
            .unwrap();
    wire.push(b'\n');
    // Actual TLS request transmission. Deliberately never read its response;
    // the caller remains uncertain even if the server has already buffered it.
    remote.io.get_mut().write_all(&wire).await.unwrap();
    let until = tokio::time::Instant::now() + LIMIT;
    let committed = loop {
        let rows = local(
            &mut running.owner,
            "todo.list",
            json!({"crewId":crew["id"],"ledger":"agent"}),
        )
        .await;
        let items = rows.as_array().expect("real TodoList is an array");
        if let Some(item) = items.iter().find(|item| item["text"] == params["text"]) {
            assert_eq!(
                items.len(),
                1,
                "one transmitted create must create exactly one fact"
            );
            break item.clone();
        }
        assert!(
            tokio::time::Instant::now() < until,
            "actual TLS write never committed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    local(&mut running.owner, "remote.revoke", json!({"grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"Revoke while caller has not consumed receipt"})).await;
    drop(remote);
    let current = local(
        &mut running.owner,
        "todo.get",
        json!({"id":committed["requirementId"]}),
    )
    .await;
    assert_eq!(
        current["text"], params["text"],
        "revocation cannot erase an admitted committed write"
    );
    let confirmation: RemoteAcceptResult =
        serde_json::from_value(local(&mut running.owner, "remote.accept", original_accept).await)
            .unwrap();
    assert!(
        confirmation.reused && confirmation.grant.revoked_at.is_some(),
        "old accept UUID must return current revoked tombstone"
    );
    let rows = local(
        &mut running.owner,
        "todo.list",
        json!({"crewId":crew["id"],"ledger":"agent"}),
    )
    .await;
    assert_eq!(
        rows.as_array().unwrap().len(),
        1,
        "read-only confirmation must not replay the write"
    );
    stopped(running).await;
}

#[tokio::test]
#[ignore = "requires explicit frozen native Swift QA executable; memory vault only"]
async fn native_apple_pairing_tls_work_event_and_revoke() {
    let program = std::path::PathBuf::from(
        std::env::var("PENDINGCREW_QA_NATIVE_PROGRAM")
            .expect("explicit compiled native QA executable required"),
    );
    assert!(program.is_absolute());
    let measured = std::process::Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(&program)
        .output()
        .unwrap();
    assert!(measured.status.success());
    assert_eq!(
        String::from_utf8(measured.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
        std::env::var("PENDINGCREW_QA_NATIVE_SHA256")
            .expect("native QA executable digest required")
    );
    let root = tempfile::tempdir().unwrap().keep();
    eprintln!("Native remote service evidence: {}", root.display());
    // Actual desktop mode exposes the typed local-admin capability. Both
    // providers are explicitly disabled executables; no account fallback.
    for name in ["home", "runtime", "projects"] {
        std::fs::create_dir(root.join(name)).unwrap();
    }
    let child_env = std::collections::BTreeMap::from([(
        "HOME".into(),
        root.join("home").to_string_lossy().into_owned(),
    )]);
    let options = crewd_server::OrchestraServe {
        codex: crewd_server::orchestra::exec_codex::CodexProgram::Path("/usr/bin/false".into()),
        claude: crewd_server::orchestra::exec_claude::ClaudeProgram::Path("/usr/bin/false".into()),
        helper: Some(std::path::PathBuf::from(
            "/private/tmp/pcw-M3-immutable-candidate-20261014-fe68022f-02/PendingCrew Codex First.app/Contents/Helpers/crewd",
        )),
        runtime_dir: Some(root.join("runtime")),
        claude_projects_dir: Some(root.join("projects")),
        codex_env: child_env.clone(),
        claude_env: child_env,
        ..Default::default()
    };
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let server_root = root.clone();
    let task = tokio::spawn(async move {
        crewd_server::serve_with(
            server_root,
            crewd_server::ServeOptions {
                orchestra: Some(options),
                ..Default::default()
            },
            async {
                let _ = stopping.await;
            },
        )
        .await
    });
    let socket = root.join("crewd.sock");
    for _ in 0..400 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut running = Service {
        owner: RpcClient::connect(&socket).await,
        stop,
        task,
    };
    let _ = enabled(&mut running.owner).await;
    let output = tokio::time::timeout(
        Duration::from_secs(40),
        tokio::process::Command::new(program)
            .arg(&root)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("native loopback flow exceeded fixed deadline")
    .unwrap();
    std::fs::write(root.join("native-stdout.log"), &output.stdout).unwrap();
    std::fs::write(root.join("native-stderr.log"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "native producer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("native-result.txt")).unwrap(),
        "Native production pairing + Network TLS13 + Core work + event + revoke PASS\n"
    );
    stopped(running).await;
}

#[cfg(unix)]
#[tokio::test]
async fn slow_tls_reader_revoke_cancels_partial_frame_denies_queued_mutation_and_shutdown_is_bounded()
 {
    use std::os::fd::AsRawFd;
    let root = tempfile::tempdir().unwrap().keep();
    eprintln!("Remote slow-writer evidence: {}", root.display());
    std::fs::create_dir(root.join("work")).unwrap();
    let mut running = service(&root).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (client, _, accepted) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &client, &identity.spki_sha256).await;
    remote.call("system.hello", json!({})).await;
    let client_socket = remote.io.get_ref().get_ref().0;
    let client_fd = client_socket.as_raw_fd();
    let server_fd = socket_accepted(port, client_socket.local_addr().unwrap().port());
    let receive = socket_set_buffer(client_fd, libc::SO_RCVBUF);
    let send = socket_set_buffer(server_fd, libc::SO_SNDBUF);
    assert!(
        receive > 0 && send > 0 && receive + send < 100_000,
        "real kernel buffers must be far smaller than the frame"
    );
    let crew = local(&mut running.owner,"crew.create",json!({"title":"Slow remote transport","workingDirectory":root.join("work").to_str().unwrap()})).await;
    let source = local(&mut running.owner,"todo.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":crew["id"],"text":"existing read fixture"})).await;
    let document = local(&mut running.owner,"document.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":crew["id"],"title":"Slow full response","body":"x".repeat(1_000_000)})).await;
    let mut wire = serde_json::to_vec(
        &json!({"id":1000,"method":"document.get","params":{"id":document["document"]["id"]}}),
    )
    .unwrap();
    wire.push(b'\n');
    remote.io.get_mut().write_all(&wire).await.unwrap();
    let deadline = tokio::time::Instant::now() + LIMIT;
    loop {
        let mut poll = libc::pollfd {
            fd: server_fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut poll, 1, 0) };
        assert!(result >= 0);
        if result == 0 && socket_incoming(client_fd) > 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "actual kernel writer never became blocked"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    std::fs::write(root.join("kernel-held-writer.json"),serde_json::to_vec(&json!({"serverSendBuffer":send,"clientReceiveBuffer":receive,"clientPendingCiphertext":socket_incoming(client_fd),"serverWritable":false,"frameBodyBytes":1_000_000})).unwrap()).unwrap();
    let mut queue = Vec::new();
    for id in 1001..1301 {
        serde_json::to_writer(
            &mut queue,
            &json!({"id":id,"method":"todo.get","params":{"id":source["requirementId"]}}),
        )
        .unwrap();
        queue.push(b'\n');
    }
    let blocked_text = "queued old mutation must not be admitted after revoke";
    serde_json::to_writer(&mut queue,&json!({"id":1301,"method":"todo.create","params":{"requestId":uuid::Uuid::new_v4().to_string(),"crewId":crew["id"],"text":blocked_text}})).unwrap();
    queue.push(b'\n');
    remote.io.get_mut().write_all(&queue).await.unwrap();
    let rows = local(
        &mut running.owner,
        "todo.list",
        json!({"crewId":crew["id"],"ledger":"agent"}),
    )
    .await;
    assert_eq!(
        rows.as_array().unwrap().len(),
        1,
        "queued mutation must not have committed before revoke"
    );
    local(&mut running.owner,"remote.revoke",json!({"grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"Revoke actual blocked writer"})).await;
    // Already emitted prefix bytes are irreversible; only completion of this
    // kernel-confirmed blocked frame is forbidden. Do not require zero bytes.
    let mut line = String::new();
    let read = tokio::time::timeout(Duration::from_secs(3), remote.io.read_line(&mut line)).await;
    let completed = read.is_ok()
        && serde_json::from_str::<Value>(&line)
            .ok()
            .is_some_and(|v| v["id"] == 1000 && v.get("result").is_some());
    assert!(
        !completed,
        "revoked slow writer completed the previously blocked full frame"
    );
    assert!(
        read.is_ok(),
        "revoked transport did not close within the fixed deadline"
    );
    let rows = local(
        &mut running.owner,
        "todo.list",
        json!({"crewId":crew["id"],"ledger":"agent"}),
    )
    .await;
    assert_eq!(
        rows.as_array().unwrap().len(),
        1,
        "old queued mutation bypassed live grant authorization"
    );
    // Keep the slow connection owned while the actual global drain completes.
    stopped(running).await;
    drop(remote);
}

#[tokio::test]
async fn default_remote_directory_reads_create_no_identity_and_change_no_durable_facts() {
    fn snapshot(root: &std::path::Path) -> Vec<String> {
        let conn = rusqlite::Connection::open_with_flags(
            root.join(crewd_store::DB_FILE),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let tables = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let mut facts = Vec::new();
        for table in tables {
            assert!(table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
            let mut stmt = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
            let columns = stmt.column_count();
            let mut rows = stmt
                .query_map([], |row| {
                    let values = (0..columns)
                        .map(|index| row.get_ref(index).map(|value| format!("{value:?}")))
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(values.join("|"))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            rows.sort();
            facts.push(table);
            facts.extend(rows);
        }
        facts
    }
    let root = tempfile::tempdir().unwrap().keep();
    eprintln!("Remote readonly directory evidence: {}", root.display());
    let mut running = service(&root).await;
    let before = snapshot(&root);
    let status = local(&mut running.owner, "remote.status", json!({})).await;
    assert_eq!(status["listening"], false);
    assert_eq!(status["configuration"]["enabled"], false);
    assert!(status["identity"].is_null());
    assert_eq!(
        local(&mut running.owner, "remote.grants", json!({})).await,
        json!([])
    );
    assert!(local(&mut running.owner, "remote.status", json!({})).await["identity"].is_null());
    assert!(
        !root.join("remote/identity.json").exists(),
        "a directory read created private remote credentials"
    );
    assert_eq!(
        snapshot(&root),
        before,
        "directory reads changed authority, IDs, work, events or remote facts"
    );
    stopped(running).await;
}

#[cfg(unix)]
fn socket_set_buffer(fd: i32, option: i32) -> i32 {
    let size: libc::c_int = 4096;
    assert_eq!(
        unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                (&size as *const libc::c_int).cast(),
                std::mem::size_of_val(&size) as libc::socklen_t,
            )
        },
        0
    );
    let mut actual: libc::c_int = 0;
    let mut length = std::mem::size_of_val(&actual) as libc::socklen_t;
    assert_eq!(
        unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                (&mut actual as *mut libc::c_int).cast(),
                &mut length,
            )
        },
        0
    );
    actual
}
#[cfg(unix)]
fn socket_incoming(fd: i32) -> i32 {
    let mut size: libc::c_int = 0;
    assert_eq!(unsafe { libc::ioctl(fd, libc::FIONREAD, &mut size) }, 0);
    size
}
#[cfg(unix)]
fn socket_accepted(port: u16, client_port: u16) -> i32 {
    let mut found = Vec::new();
    for fd in 0..4096 {
        let mut local: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of_val(&local) as libc::socklen_t;
        if unsafe {
            libc::getsockname(
                fd,
                (&mut local as *mut libc::sockaddr_storage).cast(),
                &mut len,
            )
        } != 0
            || local.ss_family as i32 != libc::AF_INET
        {
            continue;
        }
        let local =
            unsafe { &*(&local as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>() };
        if u16::from_be(local.sin_port) != port {
            continue;
        }
        let mut peer: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of_val(&peer) as libc::socklen_t;
        if unsafe {
            libc::getpeername(
                fd,
                (&mut peer as *mut libc::sockaddr_storage).cast(),
                &mut len,
            )
        } != 0
            || peer.ss_family as i32 != libc::AF_INET
        {
            continue;
        }
        let peer =
            unsafe { &*(&peer as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>() };
        if u16::from_be(peer.sin_port) == client_port {
            found.push(fd);
        }
    }
    assert_eq!(
        found.len(),
        1,
        "fixture must find its exact owned accepted loopback socket"
    );
    found[0]
}

#[cfg(unix)]
#[tokio::test]
async fn remote_global_shutdown_retires_a_still_authorized_kernel_blocked_writer_within_production_drain()
 {
    use std::os::fd::AsRawFd;
    let root = tempfile::tempdir().unwrap().keep();
    eprintln!("Remote active-drain evidence: {}", root.display());
    std::fs::create_dir(root.join("work")).unwrap();
    let mut running = service(&root).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (client, _, _) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &client, &identity.spki_sha256).await;
    remote.call("system.hello", json!({})).await;
    let socket = remote.io.get_ref().get_ref().0;
    let client_fd = socket.as_raw_fd();
    let server_fd = socket_accepted(port, socket.local_addr().unwrap().port());
    assert!(socket_set_buffer(client_fd, libc::SO_RCVBUF) < 100_000);
    assert!(socket_set_buffer(server_fd, libc::SO_SNDBUF) < 100_000);
    let crew = local(&mut running.owner,"crew.create",json!({"title":"Unrevoked drain fixture","workingDirectory":root.join("work").to_str().unwrap()})).await;
    let document = local(&mut running.owner,"document.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":crew["id"],"title":"Actual drain frame","body":"d".repeat(1_000_000)})).await;
    let mut wire = serde_json::to_vec(
        &json!({"id":9000,"method":"document.get","params":{"id":document["document"]["id"]}}),
    )
    .unwrap();
    wire.push(b'\n');
    remote.io.get_mut().write_all(&wire).await.unwrap();
    let deadline = tokio::time::Instant::now() + LIMIT;
    loop {
        let mut poll = libc::pollfd {
            fd: server_fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut poll, 1, 0) };
        assert!(result >= 0);
        if result == 0 && socket_incoming(client_fd) > 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "actual live writer not blocked"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // No revoke and no client reads: this is actual global drain of a valid
    // blocked connection, not the already-retired connection from the prior case.
    drop(running.owner);
    running.stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), running.task)
        .await
        .expect("production 10s global drain failed to bound the active slow connection")
        .unwrap()
        .unwrap();
    drop(remote);
}

#[tokio::test]
async fn enrollment_alpn_isolated_real_tls_denies_work_and_duplicate_fields() {
    let directory = tempfile::tempdir().unwrap();
    let mut running = service(&directory.path().join("core")).await;
    let (port, identity) = enabled(&mut running.owner).await;
    async fn connect(
        port: u16,
        config: rustls::ClientConfig,
    ) -> std::io::Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
        let socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(
                rustls::pki_types::ServerName::try_from("pendingcrew.invalid").unwrap(),
                socket,
            )
            .await
    }
    let config = crewd_remote::enrollment_client_configuration(&identity.spki_sha256).unwrap();
    let stream = tokio::time::timeout(LIMIT, connect(port, config))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stream.get_ref().1.alpn_protocol(),
        Some(crewd_proto::ENROLLMENT_ALPN)
    );
    assert_eq!(
        stream.get_ref().1.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    let mut anonymous = RemoteRpc {
        io: BufReader::new(stream),
        next: 1,
        events: Vec::new(),
    };
    // Anonymous registration can never enter hello, work or local administration.
    for method in ["system.hello", "crew.create", "remote.configure"] {
        let frame = anonymous.frame(method, json!({})).await.unwrap();
        assert_eq!(frame["error"]["code"], crewd_proto::RpcError::FORBIDDEN);
    }
    let config = crewd_remote::enrollment_client_configuration(&identity.spki_sha256).unwrap();
    let mut duplicate = connect(port, config).await.unwrap();
    duplicate.write_all(b"{\"id\":1,\"method\":\"enrollment.begin\",\"params\":{\"enrollmentId\":\"11111111-1111-4111-8111-111111111111\",\"enrollmentId\":\"22222222-2222-4222-8222-222222222222\",\"requestId\":\"33333333-3333-4333-8333-333333333333\",\"token\":\"x\",\"clientCertificateDerBase64\":\"x\",\"clientSpkiSha256\":\"x\",\"deviceLabel\":\"x\"}}\n").await.unwrap();
    let mut line = String::new();
    BufReader::new(duplicate)
        .read_line(&mut line)
        .await
        .unwrap();
    let error: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        error["error"]["code"],
        crewd_proto::RpcError::INVALID_PARAMS
    );
    for protocols in [
        vec![b"unknown/1".to_vec()],
        vec![crewd_proto::ENROLLMENT_ALPN.to_vec(), b"other/1".to_vec()],
    ] {
        let mut config =
            crewd_remote::enrollment_client_configuration(&identity.spki_sha256).unwrap();
        config.alpn_protocols = protocols;
        assert!(
            tokio::time::timeout(LIMIT, connect(port, config))
                .await
                .unwrap()
                .is_err()
        );
    }
    // Omitting ALPN restores mandatory client auth, never anonymous fallback.
    let mut config = crewd_remote::enrollment_client_configuration(&identity.spki_sha256).unwrap();
    config.alpn_protocols.clear();
    let rejected = async {
        use tokio::io::AsyncReadExt;
        match connect(port, config).await {
            Err(_) => true,
            Ok(mut stream) => {
                let _ = stream
                    .write_all(b"{\"id\":1,\"method\":\"system.hello\",\"params\":{}}\n")
                    .await;
                let mut bytes = [0u8; 1];
                !matches!(stream.read(&mut bytes).await,Ok(n) if n>0)
            }
        }
    };
    assert!(tokio::time::timeout(LIMIT, rejected).await.unwrap());
    assert_eq!(
        local(&mut running.owner, "remote.grants", json!({})).await,
        json!([])
    );
    stopped(running).await;
}

/// This is an exact retained ordinary binary check, never a model/provider probe.
#[tokio::test]
#[ignore = "requires approved immutable old18 normal Core path and SHA256"]
async fn enrollment_pending_challenge_isolated_from_old18_then_original_commit_recovers() {
    let binary = std::path::PathBuf::from(
        std::env::var("PCW_B_OLD18_CORE").expect("exact retained old18 normal Core"),
    );
    let expected = std::env::var("PCW_B_OLD18_SHA256").expect("exact old18 SHA256");
    fn digest(bytes: &[u8]) -> String {
        ring::digest::digest(&ring::digest::SHA256, bytes)
            .as_ref()
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect()
    }
    assert_eq!(digest(&std::fs::read(&binary).unwrap()), expected);
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("data");
    let mut current = service(&data).await;
    let (port, identity) = enabled(&mut current.owner).await;
    let token = crewd_remote::enrollment_token().unwrap();
    let issue = uuid::Uuid::new_v4().to_string();
    let enrollment = uuid::Uuid::new_v4().to_string();
    let issued=local(&mut current.owner,"remote.enrollmentCreate",json!({"requestId":issue,"enrollmentId":enrollment,"tokenHash":crewd_remote::enrollment_token_hash(&token).unwrap(),"computerLabel":"Writer barrier host","endpoint":{"host":"127.0.0.1","port":port}})).await;
    assert_eq!(issued["server"]["authorityId"], identity.authority_id);
    let client = CertificateIdentity::generate().unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let mut anonymous = RemoteRpc::connect_enrollment(port, &identity.spki_sha256).await;
    let begun=anonymous.call("enrollment.begin",json!({"enrollmentId":enrollment,"requestId":request,"token":token,"clientCertificateDerBase64":client.certificate_base64(),"clientSpkiSha256":client.pin(),"deviceLabel":"Barrier device"})).await;
    let invitation: RemoteInvitation = verify_envelope(
        &serde_json::from_value::<RemoteSignedEnvelope>(begun["invitation"].clone()).unwrap(),
        &decode_certificate(&identity.certificate_der_base64).unwrap(),
        CertificateRole::Server,
        &identity.spki_sha256,
    )
    .unwrap();
    let response = client
        .sign(&RemotePairingResponse {
            invitation_id: enrollment.clone(),
            authority_id: identity.authority_id.clone(),
            db_id: identity.db_id.clone(),
            challenge: invitation.challenge,
            client_certificate_der_base64: client.certificate_base64(),
            client_spki_sha256: client.pin().into(),
            device_label: "Barrier device".into(),
        })
        .unwrap();
    drop(anonymous);
    stopped(current).await;
    let initial_read = rusqlite::Connection::open_with_flags(
        data.join("crewd.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let before_record =
        crewd_store::meta_get(&initial_read, &format!("remote.enrollment.v1.{enrollment}"))
            .unwrap()
            .unwrap();
    let legacy_rows: i64 = initial_read
        .query_row(
            "SELECT count(*) FROM remote_invitations WHERE id=?1",
            [&enrollment],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy_rows, 0, "B pending must have no legacy approval row");
    drop(initial_read);
    struct OwnedOld(std::process::Child);
    impl Drop for OwnedOld {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                unsafe {
                    libc::kill(self.0.id() as i32, libc::SIGTERM);
                }
                let _ = self.0.wait();
            }
        }
    }
    let log_path = directory.path().join("old18.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut old = OwnedOld(
        std::process::Command::new(&binary)
            .args(["--data-dir"])
            .arg(&data)
            .arg("serve")
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let socket = data.join("crewd.sock");
    let mut ready = false;
    let mut exited = None;
    for _ in 0..400 {
        if let Some(status) = old.0.try_wait().unwrap() {
            exited = Some(status);
            break;
        }
        if socket.exists() && tokio::net::UnixStream::connect(&socket).await.is_ok() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        exited.is_none(),
        "old18 must remain readable without a schema bump: {}",
        std::fs::read_to_string(&log_path).unwrap()
    );
    assert!(ready, "old18 startup unresolved");
    let mut owner = RpcClient::connect(&socket).await;
    let accepted = owner
        .call(
            "remote.accept",
            json!({"requestId":request,"responseEnvelope":response}),
        )
        .await;
    let unsafe_accept = accepted.is_ok();
    let grants = local(&mut owner, "remote.grants", json!({})).await;
    local(&mut owner, "lifecycle.claim", json!({})).await;
    let closed = local(
        &mut owner,
        "lifecycle.exit",
        json!({"mode":"safePause","timeoutMs":1000}),
    )
    .await;
    assert_eq!(closed["status"], "stopped");
    drop(owner);
    for _ in 0..400 {
        if old.0.try_wait().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(old.0.try_wait().unwrap().is_some());
    let read = rusqlite::Connection::open_with_flags(
        data.join("crewd.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let record = crewd_store::enrollment::get(&read, &identity.authority_id, &enrollment)
        .unwrap()
        .unwrap();
    println!(
        "oldBinarySHA={expected} oldAcceptSucceeded={unsafe_accept} grants={} durableEnrollmentCommitted={} originalRequest={request}",
        grants.as_array().unwrap().len(),
        record.committed.is_some()
    );
    assert_eq!(digest(&std::fs::read(&binary).unwrap()), expected);
    assert!(
        !unsafe_accept,
        "old18 must reject unknown legacy invitation"
    );
    assert_eq!(grants, json!([]));
    assert!(record.committed.is_none());
    assert_eq!(
        crewd_store::meta_get(&read, &format!("remote.enrollment.v1.{enrollment}"))
            .unwrap()
            .unwrap(),
        before_record
    );
    assert_eq!(
        crewd_store::meta_get(&read, "dbId").unwrap().unwrap(),
        identity.db_id
    );
    assert_eq!(
        crewd_store::remote::identity(&read).unwrap().unwrap(),
        identity
    );
    drop(read);
    drop(old);
    // A normal B Core restart keeps the same authority/data and original proof.
    let mut resumed = service(&data).await;
    let status = local(&mut resumed.owner, "remote.status", json!({})).await;
    assert_eq!(status["identity"], serde_json::to_value(&identity).unwrap());
    assert_eq!(status["listening"], true);
    use base64::Engine;
    let invitation_envelope: RemoteSignedEnvelope =
        serde_json::from_value(begun["invitation"].clone()).unwrap();
    let proof = client
        .sign(&crewd_proto::EnrollmentLookupProof {
            format: 1,
            domain: crewd_proto::ENROLLMENT_LOOKUP_DOMAIN.into(),
            enrollment_id: enrollment.clone(),
            request_id: request.clone(),
            authority_id: identity.authority_id.clone(),
            db_id: identity.db_id.clone(),
            client_certificate_der_base64: client.certificate_base64(),
            client_spki_sha256: client.pin().into(),
            begin_digest: Some(digest(
                &base64::engine::general_purpose::STANDARD
                    .decode(&invitation_envelope.payload)
                    .unwrap(),
            )),
        })
        .unwrap();
    let lookup = json!({"enrollmentId":enrollment,"requestId":request,"proof":proof});
    let mut receiver = RemoteRpc::connect_enrollment(port, &identity.spki_sha256).await;
    let pending = receiver.call("enrollment.lookup", lookup.clone()).await;
    assert_eq!(pending["state"], "pending");
    assert_eq!(pending["pendingInvitation"], begun["invitation"]);
    drop(receiver);
    // First explicit commit only. Deliberately do not consume its wire response.
    let mut committing = RemoteRpc::connect_enrollment(port, &identity.spki_sha256).await;
    let mut bytes=serde_json::to_vec(&json!({"id":1,"method":"enrollment.commit","params":{"enrollmentId":enrollment,"requestId":request,"responseEnvelope":response}})).unwrap();
    bytes.push(b'\n');
    committing.io.get_mut().write_all(&bytes).await.unwrap();
    committing.io.get_mut().flush().await.unwrap();
    drop(committing);
    let mut confirmed = None;
    for _ in 0..50 {
        let mut receiver = RemoteRpc::connect_enrollment(port, &identity.spki_sha256).await;
        let value = receiver.call("enrollment.lookup", lookup.clone()).await;
        if value["state"] == "accepted" {
            confirmed = Some(value);
            break;
        }
        assert_eq!(value["state"], "pending");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let confirmed =
        confirmed.expect("original proof must recover lost commit receipt without resubmission");
    let accepted: RemoteAcceptResult =
        serde_json::from_value(confirmed["accepted"].clone()).unwrap();
    assert_eq!(accepted.request_id, request);
    let receipt: crewd_proto::RemoteGrantReceipt = verify_envelope(
        &accepted.receipt,
        &decode_certificate(&identity.certificate_der_base64).unwrap(),
        CertificateRole::Server,
        &identity.spki_sha256,
    )
    .unwrap();
    assert_eq!(receipt.client_spki_sha256, client.pin());
    assert_eq!(receipt.invitation_id, enrollment);
    assert_eq!(
        local(&mut resumed.owner, "remote.grants", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    local(&mut resumed.owner,"remote.revoke",json!({"grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"Owned enrollment protocol proof complete"})).await;
    let mut receiver = RemoteRpc::connect_enrollment(port, &identity.spki_sha256).await;
    let revoked = receiver.call("enrollment.lookup", lookup).await;
    assert_eq!(revoked["state"], "revoked");
    assert!(revoked["grantState"]["revokedAt"].is_string());
    drop(receiver);
    stopped(resumed).await;
    assert_eq!(digest(&std::fs::read(&binary).unwrap()), expected);
    println!(
        "PASS exact old18 denied pending response; no grant/legacy row/meta loss; B restart original proof+one explicit commit/lost receipt lookup/revoke"
    );
}

#[tokio::test]
async fn enrollment_real_tls_original_key_rejects_changed_request_and_atomic_bad_commit() {
    use crewd_proto::{ENROLLMENT_LOOKUP_DOMAIN, EnrollmentLookupProof};
    let directory = tempfile::tempdir().unwrap();
    let mut running = service(&directory.path().join("data")).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let token = crewd_remote::enrollment_token().unwrap();
    let enrollment = uuid::Uuid::new_v4().to_string();
    let create = json!({"requestId":uuid::Uuid::new_v4().to_string(),"enrollmentId":enrollment,"tokenHash":crewd_remote::enrollment_token_hash(&token).unwrap(),"computerLabel":"Fault controls","endpoint":{"host":"127.0.0.1","port":port}});
    let issued = local(
        &mut running.owner,
        "remote.enrollmentCreate",
        create.clone(),
    )
    .await;
    let creation = local(
        &mut running.owner,
        "remote.enrollmentLookup",
        json!({"requestId":create["requestId"],"enrollmentId":enrollment}),
    )
    .await;
    assert_eq!(creation["state"], "created");
    assert_eq!(creation["created"], issued);
    let mut altered = create.clone();
    altered["computerLabel"] = json!("Changed original creation");
    assert!(
        running
            .owner
            .call("remote.enrollmentCreate", altered)
            .await
            .is_err()
    );
    let client = CertificateIdentity::generate().unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let begin = json!({"enrollmentId":enrollment,"requestId":request,"token":token,"clientCertificateDerBase64":client.certificate_base64(),"clientSpkiSha256":client.pin(),"deviceLabel":"Original device"});
    let mut wrong = begin.clone();
    wrong["token"] = json!(crewd_remote::enrollment_token().unwrap());
    let mut receiver = RemoteRpc::connect_enrollment(port, &identity.spki_sha256).await;
    assert!(
        receiver
            .frame("enrollment.begin", wrong)
            .await
            .unwrap()
            .get("error")
            .is_some()
    );
    let begun = receiver.call("enrollment.begin", begin.clone()).await;
    drop(receiver);
    for field in ["requestId", "deviceLabel", "clientCertificateDerBase64"] {
        let mut changed = begin.clone();
        changed[field] = if field == "requestId" {
            json!(uuid::Uuid::new_v4().to_string())
        } else if field == "deviceLabel" {
            json!("Extra input")
        } else {
            json!(
                CertificateIdentity::generate()
                    .unwrap()
                    .certificate_base64()
            )
        };
        let mut receiver = RemoteRpc::connect_enrollment(port, &identity.spki_sha256).await;
        assert!(
            receiver
                .frame("enrollment.begin", changed)
                .await
                .unwrap()
                .get("error")
                .is_some(),
            "changed {field} accepted"
        );
    }
    // The original begin response may be lost: original key + UUID with no
    // digest recovers its challenge, never another key's reservation.
    let lookup_proof = EnrollmentLookupProof {
        format: 1,
        domain: ENROLLMENT_LOOKUP_DOMAIN.into(),
        enrollment_id: enrollment.clone(),
        request_id: request.clone(),
        authority_id: identity.authority_id.clone(),
        db_id: identity.db_id.clone(),
        client_certificate_der_base64: client.certificate_base64(),
        client_spki_sha256: client.pin().into(),
        begin_digest: None,
    };
    let lookup = json!({"enrollmentId":enrollment,"requestId":request,"proof":client.sign(&lookup_proof).unwrap()});
    let mut receiver = RemoteRpc::connect_enrollment(port, &identity.spki_sha256).await;
    let recovered = receiver.call("enrollment.lookup", lookup.clone()).await;
    assert_eq!(recovered["pendingInvitation"], begun["invitation"]);
    let other = CertificateIdentity::generate().unwrap();
    let mut other_proof = lookup_proof.clone();
    other_proof.client_certificate_der_base64 = other.certificate_base64();
    other_proof.client_spki_sha256 = other.pin().into();
    assert!(receiver.frame("enrollment.lookup",json!({"enrollmentId":enrollment,"requestId":request,"proof":other.sign(&other_proof).unwrap()})).await.unwrap().get("error").is_some());
    let invitation: RemoteInvitation = verify_envelope(
        &serde_json::from_value(begun["invitation"].clone()).unwrap(),
        &decode_certificate(&identity.certificate_der_base64).unwrap(),
        CertificateRole::Server,
        &identity.spki_sha256,
    )
    .unwrap();
    let mut response = RemotePairingResponse {
        invitation_id: enrollment.clone(),
        authority_id: identity.authority_id.clone(),
        db_id: identity.db_id.clone(),
        challenge: "wrong signed challenge".into(),
        client_certificate_der_base64: client.certificate_base64(),
        client_spki_sha256: client.pin().into(),
        device_label: "Original device".into(),
    };
    let failed = receiver.frame("enrollment.commit",json!({"enrollmentId":enrollment,"requestId":request,"responseEnvelope":client.sign(&response).unwrap()})).await.unwrap();
    assert!(failed.get("error").is_some());
    drop(receiver);
    assert_eq!(
        local(&mut running.owner, "remote.grants", json!({})).await,
        json!([])
    );
    // The failed signed response must not leave a legacy approval row or consume
    // the token; the first correct commit is still the same original request.
    let conn = rusqlite::Connection::open_with_flags(
        directory.path().join("data/crewd.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM remote_invitations WHERE id=?1",
            [&enrollment],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    drop(conn);
    response.challenge = invitation.challenge;
    let mut receiver = RemoteRpc::connect_enrollment(port, &identity.spki_sha256).await;
    let accepted = receiver.call("enrollment.commit",json!({"enrollmentId":enrollment,"requestId":request,"responseEnvelope":client.sign(&response).unwrap()})).await;
    assert_eq!(accepted["requestId"], request);
    let recovered = receiver.call("enrollment.lookup", lookup).await;
    assert_eq!(recovered["state"], "accepted");
    assert_eq!(
        recovered["accepted"]["grant"]["id"],
        accepted["grant"]["id"]
    );
    let mut changed = begin;
    changed["requestId"] = json!(uuid::Uuid::new_v4().to_string());
    assert!(
        receiver
            .frame("enrollment.begin", changed)
            .await
            .unwrap()
            .get("error")
            .is_some()
    );
    drop(receiver);
    assert_eq!(
        local(&mut running.owner, "remote.grants", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    stopped(running).await;
}
