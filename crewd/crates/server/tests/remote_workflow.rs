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
        .call("events.subscribe", json!({"topics":["documents"]}))
        .await;
    let visible_doc = local(&mut owner,"document.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":crew,"title":"Before revoke visible","body":"synthetic"})).await;
    let event = remote.event().await;
    assert_eq!(event["event"], "document.changed");
    assert_eq!(event["data"]["id"], visible_doc["document"]["id"]);
    local(&mut owner,"remote.revoke",json!({"grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"Online revoke"})).await;
    let hidden_doc = local(&mut owner,"document.create",json!({"requestId":uuid::Uuid::new_v4().to_string(),"crewId":crew,"title":"After revoke must not leak","body":"synthetic"})).await;
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
            .any(|event| event["event"] == "document.changed"
                && event["data"]["id"] == hidden_doc["document"]["id"]),
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
    let mut running = service(&root).await;
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
