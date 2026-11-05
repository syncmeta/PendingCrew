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
    let missing_effect_report = remote.frame(
        crewd_proto::admin_cases::EFFECTS,
        json!({"crewId":crew,"id":"AC00000000000000000001"}),
    ).await.unwrap();
    assert_eq!(
        missing_effect_report["error"]["code"],
        crewd_proto::RpcError::INVALID_PARAMS,
        "paired Human reaches the case lookup through the current grant"
    );
    let brief = remote.call(crewd_proto::admin_cases::HUMAN_BRIEF, json!({"crewId":crew})).await;
    assert_eq!(brief["crewId"], crew);
    assert_eq!(brief["humanWaits"], json!([]));
    // Paired Human access reaches the same passive lookup as the local UI.
    // A missing UUID must not become a new transfer or a remote permission error.
    let missing_custody = remote
        .frame(
            "adminCase.custodyTransferGet",
            json!({
                "requestId":uuid::Uuid::new_v4().to_string()
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        missing_custody["error"]["code"],
        crewd_proto::RpcError::NOT_FOUND
    );
    let missing_legacy = remote
        .frame(
            "adminCase.legacyReceipt",
            json!({"requestId":uuid::Uuid::new_v4().to_string()}),
        )
        .await
        .unwrap();
    assert_eq!(
        missing_legacy["error"]["code"],
        crewd_proto::RpcError::NOT_FOUND,
        "paired Human must reach pure historical receipt lookup through the same grant"
    );
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
    service_with(root, Default::default()).await
}
async fn service_with(root: &std::path::Path, options: crewd_server::ServeOptions) -> Service {
    let root = root.to_owned();
    let socket = root.join("crewd.sock");
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        crewd_server::serve_with(root, options, async {
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
async fn paired_human_ask_resolves_once_and_revoked_connection_cannot_resolve() {
    use crewd_collab::{asks, sessions};
    use crewd_proto::{
        Role, commands::{ASK_INBOX_SCHEDULE, ASK_RESOLVE}, methods::ASK_RESOLVE_CHECK,
    };

    fn schedule_packet(hello: &Value, checked: &Value, until: Option<i64>) -> Value {
        draft_packet(
            hello,
            ASK_INBOX_SCHEDULE,
            json!({
                "crewId": checked["ask"]["crewId"], "ask": checked["ask"]["ask"],
                "expectedResolutionRevision": checked["resolutionRevision"],
                "expectedInboxRevision": checked["inbox"]["revision"],
                "untilMs": until,
            }),
        )
    }

    fn resolve_packet(hello: &Value, checked: &Value, disposition: &str, text: &str) -> Value {
        draft_packet(
            hello,
            ASK_RESOLVE,
            json!({
                "crewId":checked["ask"]["crewId"],"ask":checked["ask"]["ask"],
                "expectedResolutionRevision":checked["resolutionRevision"],
                "disposition":disposition,"text":text
            }),
        )
    }
    fn lookup(packet: &Value) -> Value {
        json!({"dbId":packet["dbId"],"requestId":packet["requestId"]})
    }

    let dir = tempfile::tempdir().unwrap();
    let mut running = service(dir.path()).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, accepted) = approved(&mut running.owner, &identity).await;
    let crew = local(
        &mut running.owner,
        "crew.create",
        json!({
            "title":"Paired Ask fixture","workingDirectory":dir.path()
        }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    stopped(running).await;
    let questions = {
        let mut store = crewd_store::Store::open_data_dir(dir.path()).unwrap();
        crewd_collab::run(&mut store, crewd_store::now_ms(), |tx| {
            let asker = sessions::register(
                tx,
                sessions::NewSession {
                    crew_id: crew.clone(),
                    role: Role::Worker,
                    executor: "codex".into(),
                    title: Some("fixture worker".into()),
                    ..Default::default()
                },
            )?;
            [
                "original Answer fixture",
                "original Withdraw fixture",
                "original revoked fixture",
                "original legacy reply fixture",
                "original legacy lowercase fixture",
            ]
            .into_iter()
            .map(|question| {
                asks::open(
                    tx,
                    &asker.id,
                    asks::OpenAsk {
                        question: question.into(),
                        default: None,
                        resume_note: None,
                    },
                )
            })
            .collect::<crewd_collab::Result<Vec<_>>>()
        })
        .unwrap()
        .0
    };
    let mut running = service(dir.path()).await;
    assert_eq!(
        local(&mut running.owner, "remote.status", json!({})).await["listening"],
        true
    );
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    assert_eq!(hello["remoteBinding"]["grantId"], accepted.grant.id);
    for method in [ASK_RESOLVE_CHECK, ASK_RESOLVE, ASK_INBOX_SCHEDULE] {
        assert!(
            hello["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == method)
        );
    }
    let mut checks = Vec::new();
    for q in &questions {
        let checked = remote
            .call(ASK_RESOLVE_CHECK, json!({"crewId":crew,"ask":q.ask}))
            .await;
        assert_eq!(checked["ask"], serde_json::to_value(q).unwrap());
        assert_eq!(checked["resolutionRevision"].as_str().unwrap().len(), 64);
        checks.push(checked);
    }
    let until = crewd_store::now_ms() + 10 * 60_000;
    let defer = schedule_packet(&hello, &checks[2], Some(until));
    remote.call("events.subscribe", json!({"topics":["ask"]})).await;
    let saved = remote.call("command.submit", defer.clone()).await;
    let event = match remote.events.pop() {
        Some(event) => event,
        None => remote.event().await,
    };
    assert_eq!(event["event"], "ask.inboxChanged");
    assert_eq!(event["data"], json!({"crewId":crew}));
    assert_eq!(saved["phase"], "committed");
    assert_eq!(saved["result"]["deferredUntilMs"], until);
    assert_eq!(remote.call("command.submit", defer.clone()).await, saved);
    let rows = remote.call("ask.list", json!({})).await;
    let row = rows.as_array().unwrap().iter()
        .find(|row| row["ask"]["ask"] == questions[2].ask).unwrap();
    assert_eq!(row["ask"], serde_json::to_value(&questions[2]).unwrap());
    assert_eq!(row["deferredUntilMs"], until);

    let wrong = resolve_packet(&hello, &checks[0], "answer", "must not apply");
    let mut wrong_target = wrong.clone();
    wrong_target["command"]["params"]["ask"] = json!(questions[1].ask);
    let refused = tokio::time::timeout(LIMIT, remote.frame("command.submit", wrong_target))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refused["error"]["code"], crewd_proto::RpcError::CONFLICT);
    assert!(
        remote
            .call("command.lookup", lookup(&wrong))
            .await
            .is_null()
    );

    let answer = resolve_packet(&hello, &checks[0], "answer", " paired original answer ");
    let receipt = remote.call("command.submit", answer.clone()).await;
    assert_eq!(receipt["phase"], "committed");
    assert_eq!(
        receipt["command"]["params"]["text"],
        " paired original answer "
    );
    assert_eq!(receipt["result"]["resolved"]["state"], "answered");
    assert_eq!(
        receipt["result"]["resolved"]["answer"],
        "paired original answer"
    );
    assert_eq!(
        receipt["result"]["originalResolutionRevision"],
        checks[0]["resolutionRevision"]
    );
    assert_eq!(remote.call("command.submit", answer.clone()).await, receipt);
    let mut changed = answer.clone();
    changed["command"]["params"]["text"] = json!("different replay");
    let refused = tokio::time::timeout(LIMIT, remote.frame("command.submit", changed))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refused["error"]["code"], crewd_proto::RpcError::CONFLICT);
    drop(remote);
    stopped(running).await;
    let mut running = service(dir.path()).await;
    assert_eq!(local(&mut running.owner, "remote.status", json!({})).await["listening"], true);
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    let mut prior_answer = receipt.clone();
    prior_answer["historical"] = json!(true);
    let mut prior_schedule = saved.clone();
    prior_schedule["historical"] = json!(true);
    assert_eq!(
        remote.call("command.lookup", lookup(&answer)).await,
        prior_answer
    );
    assert_eq!(remote.call("command.lookup", lookup(&defer)).await, prior_schedule);
    let scheduled = remote.call(
        ASK_RESOLVE_CHECK, json!({"crewId":crew,"ask":questions[2].ask}),
    ).await;
    assert_eq!(scheduled["inbox"]["deferredUntilMs"], until);
    let clear = schedule_packet(&hello, &scheduled, None);
    let cleared = remote.call("command.submit", clear.clone()).await;
    assert_eq!(cleared["phase"], "committed");
    assert!(cleared["result"].get("deferredUntilMs").is_none());
    assert_eq!(cleared["result"]["originalInboxRevision"], saved["result"]["inboxRevision"]);
    assert_eq!(remote.call("command.lookup", lookup(&defer)).await, prior_schedule);
    let current = remote.call(
        ASK_RESOLVE_CHECK, json!({"crewId":crew,"ask":questions[2].ask}),
    ).await;
    assert!(current["inbox"].get("deferredUntilMs").is_none());
    let revoked_defer = schedule_packet(&hello, &current, Some(until));
    let resolved_check = remote
        .call(
            ASK_RESOLVE_CHECK,
            json!({"crewId":crew,"ask":questions[0].ask}),
        )
        .await;
    assert!(resolved_check["resolutionRevision"].is_null());
    assert_eq!(resolved_check["ask"], receipt["result"]["resolved"]);

    let withdraw = resolve_packet(&hello, &checks[1], "withdraw", "paired original reason");
    let withdrawn = remote.call("command.submit", withdraw.clone()).await;
    assert_eq!(withdrawn["result"]["resolved"]["state"], "withdrawn");
    assert_eq!(
        withdrawn["result"]["resolved"]["withdrawReason"],
        "paired original reason"
    );
    assert_eq!(
        remote.call("command.lookup", lookup(&withdraw)).await,
        withdrawn
    );
    let channel = crewd_store::channels::human_id(&crew);
    let reply = remote
        .call(
            "message.post",
            json!({
                "channelId":channel,"inReplyTo":questions[3].message_id,
                "text":"legacy paired reply still answers the original question"
            }),
        )
        .await;
    assert_eq!(reply["senderKind"], "human");
    assert_eq!(reply["inReplyTo"], json!(questions[3].message_id));
    let answered = remote
        .call("ask.get", json!({"crewId":crew,"ask":questions[3].ask}))
        .await;
    assert_eq!(answered["state"], "answered");
    assert_eq!(answered["answer"], reply["text"]);
    let legacy_answer = remote.call("ask.answer", json!({
        "crewId":crew,"ask":questions[4].ask.to_ascii_lowercase(),"text":"Legacy direct answer"
    })).await;
    assert_eq!(legacy_answer["ask"], questions[4].ask);
    assert_eq!(legacy_answer["state"], "answered");
    assert_eq!(legacy_answer["answer"], "Legacy direct answer");
    let revoked = resolve_packet(
        &hello,
        &checks[2],
        "answer",
        "must remain open after revoke",
    );
    let revoked_id = revoked["requestId"].as_str().unwrap().to_owned();
    let revoked_defer_id = revoked_defer["requestId"].as_str().unwrap().to_owned();
    local(&mut running.owner, "remote.revoke", json!({
        "grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"revoke Ask fixture"
    })).await;
    for (method, params) in [
        ("command.submit", revoked),
        ("command.submit", revoked_defer),
        ("command.lookup", lookup(&defer)),
        ("ask.list", json!({})),
        (
            ASK_RESOLVE_CHECK,
            json!({"crewId":crew,"ask":questions[2].ask}),
        ),
        ("command.lookup", lookup(&answer)),
        (
            "ask.answer",
            json!({"crewId":crew,"ask":questions[2].ask,"text":"legacy after revoke"}),
        ),
        (
            "ask.withdraw",
            json!({"crewId":crew,"ask":questions[2].ask,"reason":"legacy after revoke"}),
        ),
        (
            "message.post",
            json!({"channelId":channel,"inReplyTo":questions[2].message_id,"text":"legacy reply after revoke"}),
        ),
    ] {
        if let Ok(frame) = tokio::time::timeout(LIMIT, remote.frame(method, params))
            .await
            .unwrap()
        {
            assert_eq!(
                frame["error"]["code"],
                crewd_proto::RpcError::FORBIDDEN,
                "revoked TLS used {method}"
            );
        }
    }
    assert_eq!(
        local(
            &mut running.owner,
            "ask.get",
            json!({"crewId":crew,"ask":questions[2].ask})
        )
        .await["state"],
        "open"
    );
    drop(remote);
    stopped(running).await;
    let store = crewd_store::Store::open_data_dir(dir.path()).unwrap();
    let receipts: i64 = store
        .conn()
        .query_row(
            "SELECT count(*) FROM public_command_receipts WHERE request_id IN (?1, ?2)",
            [revoked_id, revoked_defer_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(receipts, 0);
}

#[tokio::test]
async fn paired_structured_ask_captures_direct_group_and_system_decisions() {
    use crewd_collab::{asks, decision_asks as domain, managed_objects, sessions};
    use crewd_proto::{
        Role, commands::ASK_RESOLVE, decision_asks::*, managed_objects::DecisionAbout,
        methods::ASK_RESOLVE_CHECK,
    };

    let dir = tempfile::tempdir().unwrap();
    let mut running = service(dir.path()).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, accepted) = approved(&mut running.owner, &identity).await;
    let crew = local(&mut running.owner, "crew.create", json!({
        "title":"Structured Ask fixture","workingDirectory":dir.path()
    })).await["id"].as_str().unwrap().to_owned();
    stopped(running).await;
    let questions = {
        let now = crewd_store::now_ms();
        let mut store = crewd_store::Store::open_data_dir(dir.path()).unwrap();
        let (questions, _) = crewd_collab::run(&mut store, now - 180_000, |tx| {
            let worker = sessions::register(tx, sessions::NewSession {
                crew_id: crew.clone(),
                role: Role::Worker,
                executor: "codex".into(),
                title: Some("H fixture worker".into()),
                ..Default::default()
            })?;
            let scope = managed_objects::scope(tx.conn())?;
            ["Automatic", "Direct", "Group", "Revoked"].into_iter().enumerate().map(|(index, name)| {
                let (receipt, fresh) = domain::open(tx, &worker.id, Open {
                    scope: scope.clone(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    draft: Draft {
                        question: format!("{name} choice"),
                        context: "Published decision context".into(),
                        options: vec![OptionDraft { label: "First".into() }, OptionDraft { label: "Second".into() }],
                        about: vec![DecisionAbout::Crew { id: crew.clone() }],
                        default: (index == 0).then(|| DefaultChoice { option_id: "B".into(), after_minutes: 1 }),
                        reversibility: None,
                    },
                })?;
                assert!(fresh);
                asks::get(tx.conn(), &crew, &receipt.opened.ask)
            }).collect::<crewd_collab::Result<Vec<_>>>()
        }).unwrap();
        let (defaults, _) = crewd_collab::run(&mut store, now, asks::apply_all_due_defaults).unwrap();
        assert_eq!(defaults, [questions[0].ask.clone()]);
        questions
    };
    let mut running = service(dir.path()).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    assert!(hello["capabilities"].as_array().unwrap().iter().any(|cap| cap == CAPABILITY));
    let key = |index: usize| json!({"crewId":crew,"ask":questions[index].ask});

    let defaulted = remote.call(GET, key(0)).await;
    assert_eq!(defaulted["capture"]["source"], "systemDefault");
    assert_eq!(defaulted["capture"]["optionId"], "B");
    assert!(defaulted["capture"]["humanReason"].is_null());
    let default_id = &defaulted["capture"]["decisionId"];
    assert!(remote.frame("managedObject.get", json!({"id":default_id})).await.unwrap()["error"].is_object());
    assert_eq!(remote.call("managedObject.decisions", json!({})).await["items"], json!([]));
    let default_object = remote.call("managedObject.get", json!({"id":default_id,"includeSystemDefaults":true})).await;
    assert_eq!(default_object["decision"]["author"]["kind"], "systemDefault");
    assert_eq!(default_object["decision"]["draft"]["choice"], "Second");
    assert_eq!(remote.call(DECISION_ORIGIN, json!({"id":default_id})).await, defaulted);

    let checked = remote.call(ASK_RESOLVE_CHECK, key(1)).await;
    let direct = draft_packet(&hello, ASK_RESOLVE, json!({
        "crewId":crew,"ask":questions[1].ask,"expectedResolutionRevision":checked["resolutionRevision"],
        "disposition":"answer","text":"A\nHuman reasoning"
    }));
    let committed = remote.call("command.submit", direct.clone()).await;
    assert_eq!(remote.call("command.submit", direct).await, committed);
    let answered = remote.call(GET, key(1)).await;
    assert_eq!(answered["capture"]["source"], "human");
    assert_eq!(answered["capture"]["humanReason"], "Human reasoning");
    assert_eq!(answered["capture"]["eventSeq"], committed["result"]["eventSeq"]);

    let channel = crewd_store::channels::human_id(&crew);
    let before = remote.call("message.list", json!({"channelId":channel})).await;
    let unstructured = remote.frame("message.post", json!({
        "channelId":channel,"inReplyTo":questions[2].message_id,"text":"Maybe the second one"
    })).await.unwrap();
    assert!(unstructured["error"].is_object());
    assert_eq!(remote.call("message.list", json!({"channelId":channel})).await, before);
    assert!(remote.call(GET, key(2)).await["capture"].is_null());
    let group = draft_packet(&hello, "message.post", json!({
        "channelId":channel,"inReplyTo":questions[2].message_id,"text":"B"
    }));
    let group_receipt = remote.call("command.submit", group.clone()).await;
    assert_eq!(remote.call("command.submit", group).await, group_receipt);
    let group_decision = remote.call(GET, key(2)).await;
    assert_eq!(group_decision["capture"]["source"], "human");
    assert_eq!(group_decision["capture"]["optionId"], "B");
    assert_eq!(remote.call("managedObject.decisions", json!({})).await["items"].as_array().unwrap().len(), 2);
    assert_eq!(remote.call("managedObject.decisions", json!({"includeSystemDefaults":true})).await["items"].as_array().unwrap().len(), 3);

    local(&mut running.owner, "remote.revoke", json!({
        "grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"close structured fixture"
    })).await;
    let revoked = remote.frame("message.post", json!({
        "channelId":channel,"inReplyTo":questions[3].message_id,"text":"A"
    })).await;
    assert!(revoked.is_err() || revoked.unwrap()["error"].is_object());
    assert!(local(&mut running.owner, GET, key(3)).await["capture"].is_null());
    drop(remote);
    stopped(running).await;
}

#[tokio::test]
async fn paired_human_factual_duplicate_close_keeps_receipt_scope_and_revocation() {
    use crewd_proto::admin_cases::{WORK_DUPLICATE_CLOSE, WORK_DUPLICATE_HISTORY, WORK_DUPLICATES};

    fn close_packet(hello: &Value, crew: &Value, row: &Value) -> Value {
        draft_packet(
            hello,
            WORK_DUPLICATE_CLOSE,
            json!({
                "crewId":crew,"duplicateId":row["duplicateId"],
                "expectedDuplicateRevision":row["duplicateRevision"],
                "survivorId":row["survivorId"],"expectedSurvivorRevision":row["survivorRevision"],
                "witness":row["witness"],"note":"preserve the earlier original plan",
                "evidence":{"kind":"text","value":"fixture requirements and plans match exactly"}
            }),
        )
    }

    let dir = tempfile::tempdir().unwrap();
    let mut running = service(dir.path()).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, accepted) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    assert_eq!(hello["remoteBinding"]["grantId"], accepted.grant.id);
    for method in [
        WORK_DUPLICATES,
        WORK_DUPLICATE_CLOSE,
        WORK_DUPLICATE_HISTORY,
    ] {
        assert!(
            hello["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item == method)
        );
    }
    let crew = remote
        .call(
            "crew.create",
            json!({
                "title":"Paired duplicate archive fixture","workingDirectory":dir.path()
            }),
        )
        .await["id"]
        .clone();
    let foreign = remote
        .call(
            "crew.create",
            json!({
                "title":"Separate duplicate archive scope","workingDirectory":dir.path()
            }),
        )
        .await["id"]
        .clone();
    let source = remote.call("todo.create", json!({
        "requestId":uuid::Uuid::new_v4().to_string(),"crewId":crew,"text":"Original duplicate archive requirement"
    })).await;
    let definition = json!({
        "crewId":crew,"title":"Identical work fixture","goal":"Record the original results",
        "acceptance":"Review results against the original input","sources":[source["requirementId"]]
    });
    let kept = remote.call("work.create", definition.clone()).await;
    let duplicate = remote.call("work.create", definition.clone()).await;
    let next = remote.call("work.create", definition).await;
    let scope = json!({"crewId":crew});
    let page = remote.call(WORK_DUPLICATES, scope.clone()).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let candidate = &page["items"][0];
    assert_eq!(candidate["duplicateId"], duplicate["id"]);
    assert_eq!(candidate["survivorId"], kept["id"]);
    assert_eq!(candidate["sourceIds"], json!([source["requirementId"]]));
    assert_eq!(
        remote
            .call(WORK_DUPLICATES, json!({"crewId":foreign}))
            .await["items"],
        json!([])
    );

    let packet = close_packet(&hello, &crew, candidate);
    let lookup = json!({"dbId":packet["dbId"],"requestId":packet["requestId"]});
    let committed = remote.call("command.submit", packet.clone()).await;
    let receipt = &committed["result"];
    assert_eq!(committed["phase"], "committed");
    assert_eq!(receipt["duplicateId"], duplicate["id"]);
    assert_eq!(receipt["survivorId"], kept["id"]);
    assert_eq!(receipt["survivorRevision"], kept["revision"]);
    assert_eq!(remote.call("command.submit", packet).await, committed);

    drop(remote);
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    assert_eq!(hello["remoteBinding"]["grantId"], accepted.grant.id);
    assert_eq!(
        remote.call("command.lookup", lookup.clone()).await,
        committed
    );
    let history_params = json!({"crewId":crew,"id":duplicate["id"]});
    let history = remote
        .call(WORK_DUPLICATE_HISTORY, history_params.clone())
        .await;
    assert_eq!(history["items"].as_array().unwrap().len(), 1);
    assert_eq!(history["items"][0]["seq"], receipt["eventSeq"]);
    assert_eq!(history["items"][0]["duplicateReview"], *receipt);
    let refused = tokio::time::timeout(
        LIMIT,
        remote.frame(
            WORK_DUPLICATE_HISTORY,
            json!({"crewId":foreign,"id":duplicate["id"]}),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        refused["error"]["code"],
        crewd_proto::RpcError::INVALID_PARAMS
    );
    let ordinary = remote
        .call("work.closureHistory", json!({"id":duplicate["id"]}))
        .await;
    assert!(ordinary["items"][0].get("duplicateReview").is_none());
    assert_eq!(
        remote.call("work.get", json!({"id":kept["id"]})).await,
        kept
    );
    assert_eq!(
        remote.call("work.get", json!({"id":duplicate["id"]})).await["status"],
        "dropped"
    );

    let next_page = remote.call(WORK_DUPLICATES, scope).await;
    assert_eq!(next_page["items"].as_array().unwrap().len(), 1);
    let pending = &next_page["items"][0];
    assert_eq!(pending["duplicateId"], next["id"]);
    let revoked_packet = close_packet(&hello, &crew, pending);
    let revoked_request = revoked_packet["requestId"].as_str().unwrap().to_owned();
    local(&mut running.owner, "remote.revoke", json!({
        "grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"revoke duplicate archive fixture"
    })).await;
    for (method, params) in [
        ("command.submit", revoked_packet),
        (WORK_DUPLICATE_HISTORY, history_params),
        ("command.lookup", lookup),
    ] {
        if let Ok(frame) = tokio::time::timeout(LIMIT, remote.frame(method, params))
            .await
            .unwrap()
        {
            assert_eq!(
                frame["error"]["code"],
                crewd_proto::RpcError::FORBIDDEN,
                "revoked TLS used {method}"
            );
        }
    }
    assert_eq!(
        local(&mut running.owner, "work.get", json!({"id":next["id"]})).await,
        next
    );
    drop(remote);
    stopped(running).await;
    let store = crewd_store::Store::open_data_dir(dir.path()).unwrap();
    let receipts: i64 = store
        .conn()
        .query_row(
            "SELECT count(*) FROM public_command_receipts WHERE request_id=?1",
            [revoked_request],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(receipts, 0);
}

#[tokio::test]
async fn paired_human_requirement_review_recovers_exact_receipts_and_stops_after_revoke() {
    use crewd_collab::{Sender, judgments, requirement_similarity as similarity};
    use crewd_proto::admin_cases::{REQUIREMENT_REVIEW, REQUIREMENT_REVIEWS, SIMILAR_REQUIREMENTS};
    use crewd_proto::judgments::{JudgmentConfig, JudgmentSettingsSet};

    let dir = tempfile::tempdir().unwrap();
    let mut running = service(dir.path()).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, accepted) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    assert_eq!(hello["remoteBinding"]["grantId"], accepted.grant.id);
    let crew = remote
        .call(
            "crew.create",
            json!({
                "title":"Paired requirement fixture","workingDirectory":dir.path()
            }),
        )
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let foreign = remote
        .call(
            "crew.create",
            json!({
                "title":"Other paired requirement fixture","workingDirectory":dir.path()
            }),
        )
        .await["id"]
        .clone();
    let channel = format!("{crew}/lobby");
    let mut originals = Vec::new();
    for text in ["Original paired fixture A", "Original paired fixture B"] {
        let source = remote
            .call(
                "todo.create",
                json!({
                    "requestId":uuid::Uuid::new_v4().to_string(),"crewId":crew,
                    "channelId":channel,"text":text
                }),
            )
            .await;
        originals.push(
            remote
                .call("todo.get", json!({"id":source["requirementId"]}))
                .await,
        );
    }
    drop(remote);
    stopped(running).await;

    // Seed one typed judgment through its actual prepare/dispatch/settle path
    // while the daemon is stopped. Pairing and every review use live TLS.
    {
        let mut store = crewd_store::Store::open_data_dir(dir.path()).unwrap();
        let (snapshot, _) = crewd_collab::run(&mut store, crewd_store::now_ms(), |tx| {
            let previous = judgments::configuration(tx.conn())?;
            judgments::set_configuration(
                tx,
                &Sender::Human,
                JudgmentSettingsSet {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    credential_update: Default::default(),
                    expected_revision: previous.revision,
                    config: Some(JudgmentConfig {
                        enabled: true,
                        provider: "typesafe".into(),
                        base_url: "http://127.0.0.1:1".into(),
                        model: "synthetic".into(),
                        choice_min: 0.5,
                        wake_yes: 0.5,
                        credential_source: "file".into(),
                        similar_requirements: Some(true),
                    }),
                },
            )?;
            similarity::observe_page(tx)?;
            let (pending, next) = similarity::pending_page(tx, None)?;
            assert_eq!(pending.len(), 1);
            assert!(next.is_none());
            Ok(similarity::dispatch(tx, &pending[0])?.unwrap())
        })
        .unwrap();
        crewd_collab::run(&mut store, crewd_store::now_ms(), |tx| {
            similarity::settle(
                tx,
                &snapshot,
                similarity::Outcome::Answered {
                    probability: 0.97,
                    suggested: true,
                },
            )
        })
        .unwrap();
    }

    let mut running = service(dir.path()).await;
    assert_eq!(
        local(&mut running.owner, "remote.status", json!({})).await["listening"],
        true
    );
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    assert_eq!(hello["remoteBinding"]["grantId"], accepted.grant.id);
    let scope = json!({"crewId":crew});
    let candidates = remote.call(SIMILAR_REQUIREMENTS, scope.clone()).await;
    let candidate = &candidates["items"][0];
    assert_eq!(candidate["firstId"], originals[0]["requirementId"]);
    assert_eq!(candidate["secondId"], originals[1]["requirementId"]);
    assert_eq!(candidate["channelId"], channel);
    let params = json!({
        "crewId":crew,"firstId":candidate["firstId"],"secondId":candidate["secondId"],
        "expectedReviewRevision":candidate["reviewRevision"],"candidateRevision":candidate["candidateRevision"],
        "disposition":"group","note":"preserve both paired originals"
    });
    let mut wrong_scope = params.clone();
    wrong_scope["crewId"] = foreign.clone();
    let rejected = draft_packet(&hello, REQUIREMENT_REVIEW, wrong_scope);
    let refused = tokio::time::timeout(LIMIT, remote.frame("command.submit", rejected.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refused["error"]["code"], crewd_proto::RpcError::CONFLICT);
    let rejected_lookup = json!({"dbId":rejected["dbId"],"requestId":rejected["requestId"]});
    assert!(
        remote
            .call("command.lookup", rejected_lookup)
            .await
            .is_null()
    );
    assert_eq!(
        remote
            .call(REQUIREMENT_REVIEWS, json!({"crewId":foreign}))
            .await["items"],
        json!([])
    );

    let packet = draft_packet(&hello, REQUIREMENT_REVIEW, params.clone());
    let receipt = remote.call("command.submit", packet.clone()).await;
    assert_eq!(receipt["phase"], "committed");
    assert_eq!(receipt["result"]["revision"], "1");
    let lookup = json!({"dbId":packet["dbId"],"requestId":packet["requestId"]});
    assert_eq!(remote.call("command.lookup", lookup.clone()).await, receipt);
    assert_eq!(remote.call("command.submit", packet.clone()).await, receipt);
    let mut changed = packet;
    changed["command"]["params"]["note"] = json!("different replay");
    let refused = tokio::time::timeout(LIMIT, remote.frame("command.submit", changed))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refused["error"]["code"], crewd_proto::RpcError::CONFLICT);
    assert_eq!(
        remote.call(REQUIREMENT_REVIEWS, scope.clone()).await["items"],
        json!([{"review":receipt["result"],"effective":true}])
    );
    assert_eq!(
        remote.call(SIMILAR_REQUIREMENTS, scope.clone()).await["items"],
        json!([])
    );

    drop(remote);
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    assert_eq!(remote.call("command.lookup", lookup.clone()).await, receipt);
    let clear = draft_packet(
        &hello,
        REQUIREMENT_REVIEW,
        json!({
            "crewId":crew,"firstId":candidate["firstId"],"secondId":candidate["secondId"],
            "expectedReviewRevision":"1","disposition":"clear","note":"clear only the annotation"
        }),
    );
    let cleared = remote.call("command.submit", clear.clone()).await;
    let clear_lookup = json!({"dbId":clear["dbId"],"requestId":clear["requestId"]});
    assert_eq!(remote.call("command.lookup", clear_lookup).await, cleared);
    assert_eq!(cleared["result"]["revision"], "2");
    assert_eq!(
        remote.call(REQUIREMENT_REVIEWS, scope.clone()).await["items"],
        json!([{"review":cleared["result"],"effective":false}])
    );
    let visible_again = remote.call(SIMILAR_REQUIREMENTS, scope.clone()).await;
    assert_eq!(
        visible_again["items"][0]["candidateRevision"],
        candidate["candidateRevision"]
    );
    assert_eq!(visible_again["items"][0]["reviewRevision"], "2");
    for original in originals {
        assert_eq!(
            remote
                .call("todo.get", json!({"id":original["requirementId"]}))
                .await,
            original
        );
    }

    local(&mut running.owner, "remote.revoke", json!({
        "grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"revoke test grant"
    })).await;
    let mut revoked = params;
    revoked["expectedReviewRevision"] = json!("2");
    let rejected_after_revoke = draft_packet(&hello, REQUIREMENT_REVIEW, revoked);
    let revoked_request = rejected_after_revoke["requestId"]
        .as_str()
        .unwrap()
        .to_owned();
    for (method, payload) in [
        (REQUIREMENT_REVIEWS, scope.clone()),
        ("command.lookup", lookup),
        ("command.submit", rejected_after_revoke),
    ] {
        let denied = tokio::time::timeout(LIMIT, remote.frame(method, payload))
            .await
            .unwrap();
        if let Ok(frame) = denied {
            assert_eq!(
                frame["error"]["code"],
                crewd_proto::RpcError::FORBIDDEN,
                "revoked TLS used {method}"
            );
        }
    }
    assert_eq!(
        local(&mut running.owner, REQUIREMENT_REVIEWS, scope).await["items"],
        json!([{"review":cleared["result"],"effective":false}])
    );
    drop(remote);
    stopped(running).await;
    let store = crewd_store::Store::open_data_dir(dir.path()).unwrap();
    let receipts: i64 = store
        .conn()
        .query_row(
            "SELECT count(*) FROM public_command_receipts WHERE request_id=?1",
            [revoked_request],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receipts, 0);
}

#[tokio::test]
async fn paired_and_local_manual_patrol_refuse_fresh_intent_without_scheduler() {
    let dir = tempfile::tempdir().unwrap();
    let mut running = service(dir.path()).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, _) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let crew = local(&mut running.owner, "crew.create", json!({"title":"scheduler-less patrol fixture","workingDirectory":dir.path()})).await["id"].clone();
    let hello = local(&mut running.owner, "system.hello", json!({})).await;
    assert!(!hello["capabilities"].as_array().unwrap().iter().any(|v| v == "work.patrolStart"));
    let packet = draft_packet(&hello, "work.patrolStart", json!({"crewId":crew}));
    let denied = running.owner.call("command.submit", packet.clone()).await.unwrap_err();
    assert_eq!(denied.0, crewd_proto::RpcError::CONFLICT);
    let lookup = json!({"dbId":packet["dbId"],"requestId":packet["requestId"]});
    assert!(local(&mut running.owner, "command.lookup", lookup).await.is_null());

    let hello = remote.call("system.hello", json!({})).await;
    let packet = draft_packet(&hello, "work.patrolStart", json!({"crewId":crew}));
    let denied = remote.frame("command.submit", packet.clone()).await.unwrap();
    assert_eq!(denied["error"]["code"], crewd_proto::RpcError::CONFLICT);
    assert!(remote.call("command.lookup",json!({"dbId":packet["dbId"],"requestId":packet["requestId"]})).await.is_null());
    assert!(remote.call("work.patrolStatus",json!({"crewId":crew})).await["manualRun"].is_null());
    let raw = remote.frame("work.patrolStart", json!({"crewId":crew})).await.unwrap();
    assert!(raw["error"].is_object());
    drop(remote);
    stopped(running).await;
}

#[tokio::test]
async fn paired_human_recovers_unread_requirement_ruling_after_restart_and_loses_access_on_revoke()
{
    let dir = tempfile::tempdir().unwrap();
    let options = || crewd_server::ServeOptions {
        orchestra: Some(crewd_server::OrchestraServe::default()),
        ..Default::default()
    };
    let mut running = service_with(dir.path(), options()).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, grant) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    let crew = remote
        .call(
            "crew.create",
            json!({"title":"isolated ruling crew","workingDirectory":dir.path()}),
        )
        .await["id"]
        .clone();
    let todo = remote
        .call(
            "todo.create",
            json!({"crewId":crew,"requestId":uuid::Uuid::new_v4().to_string(),"text":"original decision after all plans dropped"}),
        )
        .await;
    let id = &todo["requirementId"];
    let work = remote
        .call(
            "work.create",
            json!({"crewId":crew,"title":"withdrawn plan","sources":[id]}),
        )
        .await;
    remote
        .call(
            "work.update",
            json!({"id":work["id"],"expectedRevision":work["revision"],"status":"dropped","note":"alternative is unavailable"}),
        )
        .await;
    let target = json!({"crewId":crew,"id":id});
    let review = remote.call("todo.rulingCheck", target.clone()).await;
    assert_eq!(review["state"], "ready");
    assert_eq!(review["work"][0]["dropNote"], "alternative is unavailable");
    let packet = draft_packet(
        &hello,
        "todo.ruleClose",
        json!({"crewId":crew,"id":id,"expectedRevision":review["closeRevision"],"note":"recorded Human decision"}),
    );
    let lookup = json!({"dbId":packet["dbId"],"requestId":packet["requestId"]});
    let unread = json!({"id":1000,"method":"command.submit","params":packet});
    remote
        .io
        .get_mut()
        .write_all(format!("{unread}\n").as_bytes())
        .await
        .unwrap();
    // Observe commit through another authenticated connection. The
    // submitting connection never consumes its command response.
    let mut observer = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let original = tokio::time::timeout(LIMIT, async {
        loop {
            let receipt = observer.call("command.lookup", lookup.clone()).await;
            if receipt["phase"] == "committed" {
                break receipt;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(original["result"]["requestId"], packet["requestId"]);
    assert_eq!(original["result"]["note"], "recorded Human decision");
    assert_eq!(
        observer.call("todo.rulingResult", target.clone()).await["receipt"],
        original["result"]
    );
    let (other_cert, _, _) = approved(&mut running.owner, &identity).await;
    let mut other = RemoteRpc::connect(port, &other_cert, &identity.spki_sha256).await;
    assert_eq!(
        other.call("todo.rulingResult", target.clone()).await["receipt"],
        original["result"]
    );
    assert!(other.call("command.lookup", lookup.clone()).await.is_null());
    let copied = tokio::time::timeout(LIMIT, other.frame("command.submit", packet))
        .await
        .unwrap()
        .unwrap();
    assert!(copied["error"].is_object() || copied["result"]["phase"] == "rejected");
    drop(other);
    drop((remote, observer));
    stopped(running).await;

    let mut running = service_with(dir.path(), options()).await;
    tokio::time::timeout(LIMIT, async {
        loop {
            if local(&mut running.owner, "remote.status", json!({})).await["listening"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let reopened = remote.call("command.lookup", lookup.clone()).await;
    assert_eq!(reopened["phase"], "committed");
    assert_eq!(reopened["historical"], true);
    assert_eq!(reopened["result"], original["result"]);
    assert_eq!(
        remote.call("todo.rulingResult", target.clone()).await["receipt"],
        original["result"]
    );
    local(
        &mut running.owner,
        "remote.revoke",
        json!({"grantId":grant.grant.id,"expectedRevision":grant.grant.revision,"note":"ruling fixture complete"}),
    )
    .await;
    for (method, params) in [("todo.rulingResult", target), ("command.lookup", lookup)] {
        let denied = tokio::time::timeout(LIMIT, remote.frame(method, params))
            .await
            .unwrap();
        assert!(denied.is_err() || denied.unwrap()["error"].is_object());
    }
    drop(remote);
    stopped(running).await;
}

#[tokio::test]
async fn paired_human_manual_patrol_has_original_receipt_and_scoped_progress_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let options = || crewd_server::ServeOptions {
        orchestra: Some(crewd_server::OrchestraServe::default()), // Both native executors disabled.
        ..Default::default()
    };
    let mut running = service_with(dir.path(), options()).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, grant) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "work.patrolStart")
    );
    let crew = remote
        .call(
            "crew.create",
            json!({"title":"isolated manual patrol","workingDirectory":dir.path()}),
        )
        .await["id"]
        .clone();
    let foreign = remote
        .call(
            "crew.create",
            json!({"title":"isolated other crew","workingDirectory":dir.path()}),
        )
        .await["id"]
        .clone();
    let w = remote
        .call(
            "work.create",
            json!({"crewId":crew,"title":"local unowned work"}),
        )
        .await["id"]
        .clone();
    remote
        .call(
            "work.create",
            json!({"crewId":foreign,"title":"foreign unowned work"}),
        )
        .await;
    let packet = draft_packet(&hello, "work.patrolStart", json!({"crewId":crew}));
    let receipt = remote.call("command.submit", packet.clone()).await;
    assert_eq!(receipt["phase"], "committed");
    assert_eq!(receipt["result"]["crewId"], crew);
    assert_eq!(receipt["result"]["disposition"], "started");
    let id = receipt["result"]["runId"].clone();
    let joined = remote
        .call(
            "command.submit",
            draft_packet(&hello, "work.patrolStart", json!({"crewId":crew})),
        )
        .await;
    assert_eq!(joined["result"]["disposition"], "joined");
    assert_eq!(joined["result"]["runId"], id);
    let lookup = json!({"dbId":packet["dbId"],"requestId":packet["requestId"]});
    assert_eq!(remote.call("command.lookup", lookup.clone()).await, receipt);
    drop(remote);
    stopped(running).await;

    let mut running = service_with(dir.path(), options()).await;
    tokio::time::timeout(LIMIT, async {
        loop {
            let status = local(&mut running.owner, "remote.status", json!({})).await;
            if status["listening"] == true {
                assert_eq!(status["identity"], serde_json::to_value(&identity).unwrap());
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("persisted loopback endpoint did not reopen");
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let reopened = remote.call("command.lookup", lookup.clone()).await;
    assert_eq!(reopened["result"], receipt["result"]);
    assert_eq!(reopened["phase"], "committed");
    assert_eq!(reopened["historical"], true);
    let result = tokio::time::timeout(LIMIT, async {
        loop {
            let status = remote
                .call("work.patrolStatus", json!({"crewId":crew}))
                .await;
            if status["manualRun"]["completedAtMs"].is_number() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result["manualRun"]["runId"], id);
    assert_eq!(result["manualRunActive"], false);
    assert_eq!(result["manualRun"]["findings"]["unowned"], json!([w]));
    assert!(result["captainBrief"].is_null());
    assert!(
        remote
            .call("work.patrolStatus", json!({"crewId":foreign}))
            .await["manualRun"]
            .is_null()
    );
    assert_eq!(
        remote.call("work.get", json!({"id":w})).await["status"],
        "open"
    );

    let hello = remote.call("system.hello", json!({})).await;
    let next = remote
        .call(
            "command.submit",
            draft_packet(&hello, "work.patrolStart", json!({"crewId":crew})),
        )
        .await;
    assert_ne!(next["result"]["runId"], id);
    assert_eq!(
        remote.call("command.lookup", lookup).await["result"],
        receipt["result"]
    );
    local(&mut running.owner,"remote.revoke",json!({"grantId":grant.grant.id,"expectedRevision":grant.grant.revision,"note":"fixture complete"})).await;
    let denied = tokio::time::timeout(LIMIT, remote
        .frame(
            "command.submit",
            draft_packet(&hello, "work.patrolStart", json!({"crewId":foreign})),
        )).await.unwrap();
    assert!(denied.is_err() || denied.unwrap()["error"].is_object());
    stopped(running).await;
}

#[tokio::test]
async fn paired_message_sources_keep_original_receipts_and_recheck_each_live_grant() {
    use crewd_proto::managed_message_sources::{CAPABILITY, GET, MESSAGE_OBJECTS, RECORD, REQUEST_GET};

    let dir = tempfile::tempdir().unwrap();
    let mut running = service(dir.path()).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, accepted) = approved(&mut running.owner, &identity).await;
    let (other_cert, _, _) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    assert!(hello["capabilities"].as_array().unwrap().iter().any(|cap| cap == CAPABILITY));
    let crew = local(&mut running.owner, "crew.create", json!({
        "title":"Paired message record","workingDirectory":dir.path()
    })).await["id"].clone();
    let source_crew = local(&mut running.owner, "crew.create", json!({
        "title":"Separate source crew","workingDirectory":dir.path()
    })).await["id"].as_str().unwrap().to_owned();
    let channel = crewd_store::channels::human_id(&source_crew);
    let text = "Synthetic message source e\u{301} preserved byte-for-byte";
    let message = local(&mut running.owner, "message.post", json!({
        "channelId":channel,"text":text
    })).await;
    let scope = remote.call("managedObject.scope", json!({})).await;
    let packet = json!({
        "scope":scope,"crewId":crew,"requestId":uuid::Uuid::new_v4().to_string(),
        "sourceMessageId":message["id"],"change":{"kind":"recordDecision","draft":{
            "question":"How to retain provenance?","choice":"Preserve the original",
            "rationale":"Synthetic paired capture","about":[{"kind":"crew","id":crew}]
        }}
    });
    let receipt = remote.call(RECORD, packet.clone()).await;
    let id = receipt["object"]["decision"]["id"].clone();
    assert_eq!(receipt["created"], true);
    assert_eq!(receipt["sourceMessageId"], message["id"]);
    assert_eq!(receipt["scope"], scope);
    assert_eq!(receipt["requestId"], packet["requestId"]);
    assert_eq!(receipt["object"]["decision"]["author"], json!({"kind":"human"}));
    assert!(receipt.get("excerpt").is_none(), "message body belongs only to the source projection");
    assert_eq!(remote.call(RECORD, packet.clone()).await, receipt);
    let lookup = json!({"scope":scope,"crewId":crew,"requestId":packet["requestId"]});
    assert_eq!(remote.call(REQUEST_GET, lookup.clone()).await, json!({"status":"known","receipt":receipt}));
    assert_eq!(local(&mut running.owner, REQUEST_GET, lookup.clone()).await, json!({"status":"notFound"}));

    let expected_source = json!({"status":"available","crewId":source_crew,"messageId":message["id"],"channelId":channel,"excerpt":text});
    assert_eq!(remote.call(GET, json!({"id":id})).await, expected_source);
    assert_eq!(remote.call(MESSAGE_OBJECTS, json!({"messageId":message["id"]})).await,
               json!({"items":[receipt["object"]],"nextAfter":null}));
    let ordinary = remote.call("managedObject.submit", json!({
        "scope":scope,"crewId":crew,"requestId":uuid::Uuid::new_v4().to_string(),
        "change":{"kind":"recordDecision","draft":{"question":"Ordinary record",
            "choice":"Keep no inferred source","rationale":"Explicit source only","about":[{"kind":"crew","id":crew}]}}
    })).await;
    assert_eq!(remote.call(GET, json!({"id":ordinary["object"]["decision"]["id"]})).await,
               json!({"status":"none"}));
    let mut same_source = packet.clone();
    same_source["requestId"] = json!(uuid::Uuid::new_v4().to_string());
    let deduplicated = remote.call(RECORD, same_source.clone()).await;
    assert_eq!(deduplicated["created"], false);
    assert_eq!(deduplicated["object"], receipt["object"]);
    assert_eq!(remote.call(RECORD, same_source).await, deduplicated);
    remote.call("managedObject.submit", json!({
        "scope":scope,"crewId":crew,"requestId":uuid::Uuid::new_v4().to_string(),
        "change":{"kind":"withdrawDecision","decisionId":id,
                  "expectedRevision":receipt["object"]["decision"]["revision"],
                  "reason":"Later explicit withdrawal"}
    })).await;
    assert_eq!(remote.call(GET, json!({"id":id})).await, expected_source);
    drop(remote);
    stopped(running).await;

    let mut running = service(dir.path()).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let restarted = remote.call("system.hello", json!({})).await;
    assert_ne!(restarted["runtimeIdentity"]["runtimeBindingId"], hello["runtimeIdentity"]["runtimeBindingId"]);
    assert_eq!(remote.call(REQUEST_GET, lookup.clone()).await, json!({"status":"known","receipt":receipt}));
    assert_eq!(remote.call("managedObject.get", json!({"id":id})).await["decision"]["status"], "withdrawn");
    assert_eq!(remote.call(GET, json!({"id":id})).await, expected_source);
    let mut other = RemoteRpc::connect(port, &other_cert, &identity.spki_sha256).await;
    other.call("system.hello", json!({})).await;
    assert_eq!(other.call("managedObject.get", json!({"id":id})).await["decision"]["id"], id);
    assert_eq!(other.call(GET, json!({"id":id})).await, expected_source);
    assert_eq!(other.call(REQUEST_GET, lookup.clone()).await, json!({"status":"notFound"}),
               "organization-wide object/source read cannot inherit another grant's original receipt");

    let mut denied_packet = packet.clone();
    denied_packet["requestId"] = json!(uuid::Uuid::new_v4().to_string());
    let denied_request = denied_packet["requestId"].as_str().unwrap().to_owned();
    let mut revoked_calls = Vec::new();
    for (method, params) in [
        (RECORD, denied_packet),
        (GET, json!({"id":id})),
        (MESSAGE_OBJECTS, json!({"messageId":message["id"]})),
        (REQUEST_GET, lookup.clone()),
    ] {
        let mut connection = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
        connection.call("system.hello", json!({})).await;
        revoked_calls.push((method, params, connection));
    }
    local(&mut running.owner, "remote.revoke", json!({
        "grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"End paired message fixture"
    })).await;
    for (method, params, mut connection) in revoked_calls {
        if let Ok(reply) = tokio::time::timeout(LIMIT, connection.frame(method, params)).await.unwrap() {
            assert_eq!(reply["error"]["code"], crewd_proto::RpcError::FORBIDDEN,
                       "revoked grant used {method}: {reply}");
            assert!(reply.get("result").is_none());
        }
    }
    assert_eq!(local(&mut running.owner, "managedObject.get", json!({"id":id})).await["decision"]["status"], "withdrawn");
    assert_eq!(other.call(GET, json!({"id":id})).await, expected_source,
               "revoking one device leaves the separately paired Human authorized");
    drop(remote);
    drop(other);
    stopped(running).await;
    let store = crewd_store::Store::open_data_dir(dir.path()).unwrap();
    let denied_receipts: i64 = store.conn().query_row(
        "SELECT count(*) FROM managed_message_receipts WHERE request_id=?1",
        [&denied_request], |row| row.get(0)
    ).unwrap();
    assert_eq!(denied_receipts, 0, "revoked write must not reserve an original receipt");
}

#[tokio::test]
async fn attached_agent_cannot_use_human_message_source_methods() {
    use crewd_proto::managed_message_sources::{GET, MESSAGE_OBJECTS, RECORD, REQUEST_GET};

    let fixture = crewd_server::agent::testkit::Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let server = fixture.spawn_owned_socket(stopping).await;
    let mut agent = RpcClient::connect(&fixture.data_dir().join("crewd.sock")).await;
    local(&mut agent, "agent.attach", json!({
        "sessionId":fixture.worker,"token":fixture.token(&fixture.worker)
    })).await;
    let before = durable_state(&fixture).await;
    for method in [RECORD, GET, MESSAGE_OBJECTS, REQUEST_GET] {
        let refusal = tokio::time::timeout(LIMIT, agent.call(method,
            json!({"deliberatelyInvalid":"no Human source parsing or read oracle"})))
            .await.unwrap().unwrap_err();
        assert_eq!(refusal.0, crewd_proto::RpcError::FORBIDDEN, "{method}");
    }
    assert_eq!(durable_state(&fixture).await, before);
    drop(agent);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, server).await.unwrap().unwrap();
}

#[tokio::test]
async fn paired_managed_objects_keep_original_principal_and_obey_revocation() {
    let dir = tempfile::tempdir().unwrap();
    let mut running = service(dir.path()).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, grant) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cap| cap == "managedObjects.v1")
    );
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cap| cap == "managedDecisions.v1")
    );
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cap| cap == "managedRiskLessons.v1")
    );
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cap| cap == crewd_proto::managed_objects_api::EVENTS_CAPABILITY)
    );
    let owner = local(
        &mut running.owner,
        "crew.create",
        json!({"title":"managed owner", "workingDirectory":dir.path()}),
    )
    .await["id"]
        .clone();
    let other = local(
        &mut running.owner,
        "crew.create",
        json!({"title":"managed reader", "workingDirectory":dir.path()}),
    )
    .await["id"]
        .clone();
    let scope = remote.call("managedObject.scope", json!({})).await;
    assert_eq!(
        scope,
        local(&mut running.owner, "managedObject.scope", json!({})).await
    );
    let original = json!({"scope":scope,"crewId":owner,"requestId":uuid::Uuid::new_v4().to_string(),
        "change":{"kind":"createTable","title":"Shared fixture","fields":[{"name":"Label","fieldType":"text","nullable":false}]}});
    let receipt = remote.call("managedObject.submit", original.clone()).await;
    let table = receipt["object"]["table"]["id"].clone();
    let lookup = json!({"scope":scope,"crewId":owner,"requestId":original["requestId"]});
    assert_eq!(
        remote
            .call("managedObject.request.get", lookup.clone())
            .await,
        json!({"status":"known","receipt":receipt})
    );
    assert_eq!(
        local(
            &mut running.owner,
            "managedObject.request.get",
            lookup.clone()
        )
        .await,
        json!({"status":"notFound"})
    );
    assert_eq!(
        local(&mut running.owner, "managedObject.get", json!({"id":table})).await,
        receipt["object"]
    );
    let wrong_owner = json!({"scope":scope,"crewId":other,"requestId":uuid::Uuid::new_v4().to_string(),
        "change":{"kind":"addRow","tableId":table,"values":[{"type":"text","value":"denied"}]}});
    assert_eq!(
        remote
            .frame("managedObject.submit", wrong_owner)
            .await
            .unwrap()["error"]["code"],
        crewd_proto::RpcError::FORBIDDEN
    );
    assert_eq!(
        remote
            .call("managedObject.rows", json!({"tableId":table}))
            .await["items"],
        json!([])
    );
    let decision_packet = json!({
        "scope":scope,"crewId":owner,"requestId":uuid::Uuid::new_v4().to_string(),
        "change":{"kind":"recordDecision","draft":{"question":"Which fixture?",
            "choice":"Use isolation","rationale":"Keep the data private",
            "about":[{"kind":"crew","id":owner}]}}
    });
    let decision = remote.call("managedObject.submit", decision_packet).await;
    let decision_id = decision["object"]["decision"]["id"].clone();
    assert_eq!(
        decision["object"]["decision"]["author"],
        json!({"kind":"human"})
    );
    assert_eq!(
        remote.call("managedObject.decisions", json!({})).await["items"],
        json!([decision["object"]["decision"]])
    );
    let decision_lookup = json!({"scope":scope,"crewId":owner,"requestId":decision["requestId"]});
    assert_eq!(
        remote
            .call("managedObject.request.get", decision_lookup.clone())
            .await,
        json!({"status":"known","receipt":decision})
    );
    let risk = remote
        .call(
            "managedObject.submit",
            json!({
                "scope":scope,"crewId":owner,"requestId":uuid::Uuid::new_v4().to_string(),
                "change":{"kind":"recordRisk","draft":{"summary":"Possible delay","impact":"high",
                    "about":[{"kind":"crew","id":owner}],"mitigatedBy":[]}}
            }),
        )
        .await;
    let risk = &risk["object"]["risk"];
    let occurrence = json!({
        "scope":scope,"crewId":owner,"requestId":uuid::Uuid::new_v4().to_string(),
        "change":{"kind":"recordRiskOccurrence","riskId":risk["id"],"expectedRevision":risk["revision"],
            "lessonDraft":{"happened":"The deadline passed","lesson":"Check the dependency",
                "learnedFrom":[],"evidence":[],"appliesTo":[]}}
    });
    let mut updates = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    updates.call("system.hello", json!({})).await;
    assert_eq!(
        updates.call("events.subscribe", json!({"topics":["onto"]})).await,
        json!({"topics":["onto"]})
    );
    let linked = remote
        .call("managedObject.submit", occurrence.clone())
        .await;
    let lesson = &linked["object"]["lesson"];
    assert_eq!(linked["occurrence"]["riskId"], risk["id"]);
    assert_eq!(linked["occurrence"]["lessonId"], lesson["id"]);
    assert_eq!(
        updates.event().await,
        json!({"event":"onto.changed","data":{"crewId":owner,"objectIds":[risk["id"],lesson["id"]]}})
    );
    assert_eq!(
        updates.call("managedObject.get", json!({"id":lesson["id"]})).await,
        linked["object"]
    );
    assert_eq!(
        remote.call("managedObject.risks", json!({})).await["items"][0]["becameLessonId"],
        lesson["id"]
    );
    assert_eq!(
        remote.call("managedObject.lessons", json!({})).await["items"],
        json!([lesson])
    );
    let occurrence_lookup =
        json!({"scope":scope,"crewId":owner,"requestId":occurrence["requestId"]});
    assert_eq!(
        remote
            .call("managedObject.request.get", occurrence_lookup.clone())
            .await,
        json!({"status":"known","receipt":linked})
    );
    assert_eq!(
        local(
            &mut running.owner,
            "managedObject.request.get",
            occurrence_lookup.clone()
        )
        .await,
        json!({"status":"notFound"})
    );
    assert_eq!(remote.call("managedObject.submit", occurrence).await, linked);
    let confirmation = json!({
        "scope":scope,"crewId":owner,"requestId":uuid::Uuid::new_v4().to_string(),
        "change":{"kind":"confirmLesson","lessonId":lesson["id"],"expectedRevision":lesson["revision"]}
    });
    local(&mut running.owner, "managedObject.submit", confirmation).await;
    assert_eq!(
        updates.event().await,
        json!({"event":"onto.changed","data":{"crewId":owner,"objectIds":[lesson["id"]]}})
    );
    local(&mut running.owner, "remote.revoke",
        json!({"grantId":grant.grant.id,"expectedRevision":grant.grant.revision,"note":"managed fixture complete"})).await;
    local(&mut running.owner, "managedObject.submit", json!({
        "scope":scope,"crewId":other,"requestId":uuid::Uuid::new_v4().to_string(),
        "change":{"kind":"recordLesson","draft":{"happened":"The test grant was revoked",
            "lesson":"Recheck authority before notifying","learnedFrom":[],"evidence":[],"appliesTo":[]}}
    })).await;
    let mut after_revoke = String::new();
    assert!(matches!(
        tokio::time::timeout(LIMIT, updates.io.read_line(&mut after_revoke)).await,
        Ok(Ok(0) | Err(_))
    ), "revoked subscriber received {after_revoke}");
    for (method, params) in [
        ("managedObject.get", json!({"id":table})),
        ("managedObject.request.get", lookup),
        ("managedObject.submit", original),
        ("managedObject.get", json!({"id":decision_id})),
        ("managedObject.decisions", json!({})),
        ("managedObject.request.get", decision_lookup),
        ("managedObject.risks", json!({})),
        ("managedObject.lessons", json!({})),
        ("managedObject.request.get", occurrence_lookup),
    ] {
        let response = tokio::time::timeout(LIMIT, remote.frame(method, params))
            .await
            .unwrap();
        assert!(response.is_err() || response.unwrap()["error"].is_object());
    }
    drop(remote);
    stopped(running).await;
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
async fn public_pair_backup_refusal_preserves_revocation_same_server_identity_and_accept_witness() {
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
    let refusal = crewd_store::backup::restore(&root, &backup).unwrap_err();
    assert!(
        refusal
            .to_string()
            .contains("Core-only restore cannot replace a knowledge workspace")
    );
    drop(lock);
    assert_eq!(std::fs::read(&backup).unwrap(), old_bytes);
    assert_eq!(
        std::fs::read(root.join("remote/identity.json")).unwrap(),
        key_bytes
    );
    let mut restored = service(&root).await;
    let status = local(&mut restored.owner, "remote.status", json!({})).await;
    assert_eq!(status["configuration"]["enabled"], true);
    assert_eq!(status["listening"], true);
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
    // Four bounded current messages form the same >kernel-buffer response.
    // This validates transport cancellation without withdrawn document CRUD.
    let channel = format!("{}/lobby", crew["id"].as_str().unwrap());
    for _ in 0..4 {
        local(
            &mut running.owner,
            "message.post",
            json!({"channelId":channel,"text":"x".repeat(250_000)}),
        )
        .await;
    }
    let mut wire = serde_json::to_vec(
        &json!({"id":1000,"method":"message.list","params":{"channelId":channel}}),
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
    // Four bounded current messages form the same >kernel-buffer response.
    // This validates transport cancellation without withdrawn document CRUD.
    let channel = format!("{}/lobby", crew["id"].as_str().unwrap());
    for _ in 0..4 {
        local(
            &mut running.owner,
            "message.post",
            json!({"channelId":channel,"text":"d".repeat(250_000)}),
        )
        .await;
    }
    let mut wire = serde_json::to_vec(
        &json!({"id":9000,"method":"message.list","params":{"channelId":channel}}),
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

fn draft_packet(hello: &Value, operation: &str, params: Value) -> Value {
    json!({"requestId":uuid::Uuid::new_v4().to_string(),"dbId":hello["runtimeIdentity"]["dbId"],"runtimeBindingId":hello["runtimeIdentity"]["runtimeBindingId"],"command":{"operation":operation,"params":params}})
}
fn draft_scope(status: &Value) -> Value {
    json!({"spaceId":status["spaceId"],"spaceEpoch":status["spaceEpoch"],"memberId":status["member"]["memberId"],"memberRevision":status["member"]["revision"]})
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn core_personal_drafts_real_tls_exact_approval_two_writers_retirement_restart_and_revoke() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let mut running = service(&data).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let owner_hello = local(&mut running.owner, "system.hello", json!({"apiVersion":3})).await;
    let owner_status = local(&mut running.owner, "personalDraft.status", json!({})).await;
    let owner=local(&mut running.owner,"command.submit",draft_packet(&owner_hello,"personalDraft.ownerConfigure",json!({"ownerEpoch":owner_status["ownerEpoch"],"expectedSpaceId":null,"expectedSpaceRevision":"0","localEnabled":true,"acknowledgeRestore":false}))).await;
    assert_eq!(owner["phase"], "committed");
    let crew=local(&mut running.owner,"crew.create",json!({"title":"Synthetic personal draft crew","workingDirectory":root.path().join("work").to_str().unwrap()})).await;
    let key = json!({"authorityId":identity.db_id,"dbId":identity.db_id,"crewId":crew["id"],"destination":{"kind":"channel","id":format!("{}/lobby",crew["id"].as_str().unwrap())},"kind":"channelMessage"});
    let (cert_a, _, grant_a) = approved(&mut running.owner, &identity).await;
    let (cert_b, _, grant_b) = approved(&mut running.owner, &identity).await;
    let mut a = RemoteRpc::connect(port, &cert_a, &identity.spki_sha256).await;
    let mut b = RemoteRpc::connect(port, &cert_b, &identity.spki_sha256).await;
    let hello_a = a.call("system.hello", json!({"apiVersion":3})).await;
    let hello_b = b.call("system.hello", json!({"apiVersion":3})).await;
    let outsider = b.call("personalDraft.status", json!({})).await;
    assert_eq!(outsider["available"], false);
    assert!(
        outsider["spaceId"].is_null()
            && outsider["member"].is_null()
            && outsider["ownerEpoch"].is_null()
    );
    let guessed = draft_packet(
        &hello_b,
        "personalDraft.memberSet",
        json!({"spaceId":owner["result"]["spaceId"],"spaceEpoch":owner["result"]["spaceEpoch"],"expectedSpaceRevision":owner["result"]["spaceRevision"],"grantId":grant_b.grant.id,"grantRevision":grant_b.grant.revision,"clientSpkiSha256":cert_b.pin(),"enabled":true}),
    );
    let refused = b.frame("command.submit", guessed).await.unwrap();
    assert_eq!(refused["error"]["code"], crewd_proto::RpcError::FORBIDDEN);
    for (grant, pin) in [
        (&grant_a.grant, cert_a.pin()),
        (&grant_b.grant, cert_b.pin()),
    ] {
        let status = local(&mut running.owner, "personalDraft.status", json!({})).await;
        let approved=local(&mut running.owner,"command.submit",draft_packet(&owner_hello,"personalDraft.memberSet",json!({"spaceId":status["spaceId"],"spaceEpoch":status["spaceEpoch"],"expectedSpaceRevision":status["spaceRevision"],"grantId":grant.id,"grantRevision":grant.revision,"clientSpkiSha256":pin,"enabled":true}))).await;
        assert_eq!(approved["phase"], "committed");
    }
    let pre_a = a.call("personalDraft.status", json!({})).await;
    assert_eq!(pre_a["available"], false);
    assert_eq!(pre_a["member"]["optedIn"], false);
    assert!(
        a.frame(
            "personalDraft.branches",
            json!({"scope":draft_scope(&pre_a),"key":key})
        )
        .await
        .unwrap()
        .get("error")
        .is_some()
    );
    let consent_a = a
        .call(
            "command.submit",
            draft_packet(
                &hello_a,
                "personalDraft.consentSet",
                json!({"scope":draft_scope(&pre_a),"enabled":true}),
            ),
        )
        .await;
    assert_eq!(consent_a["phase"], "committed");
    let status_a = a.call("personalDraft.status", json!({})).await;
    let scope_a = draft_scope(&status_a);
    assert_eq!(pre_a["member"]["memberId"], status_a["member"]["memberId"]);
    assert_eq!(pre_a["member"]["writerId"], status_a["member"]["writerId"]);
    let status_b = b.call("personalDraft.status", json!({})).await;
    b.call(
        "command.submit",
        draft_packet(
            &hello_b,
            "personalDraft.consentSet",
            json!({"scope":draft_scope(&status_b),"enabled":true}),
        ),
    )
    .await;
    let status_b = b.call("personalDraft.status", json!({})).await;
    let scope_b = draft_scope(&status_b);
    assert_ne!(
        status_a["member"]["writerId"],
        status_b["member"]["writerId"]
    );
    a.call("events.subscribe", json!({"topics":["personalDraft"]}))
        .await;
    b.call("events.subscribe", json!({"topics":["personalDraft"]}))
        .await;
    let (cert_c, _, _) = approved(&mut running.owner, &identity).await;
    let mut c = RemoteRpc::connect(port, &cert_c, &identity.spki_sha256).await;
    c.call("system.hello", json!({"apiVersion":3})).await;
    c.call("events.subscribe", json!({"topics":["personalDraft"]}))
        .await;
    assert!(
        c.frame("personalDraft.branches", json!({"scope":scope_a,"key":key}))
            .await
            .unwrap()
            .get("error")
            .is_some()
    );
    let branch_a = uuid::Uuid::new_v4().to_string();
    let branch_b = uuid::Uuid::new_v4().to_string();
    let payload_a = json!({"text":"A exact 原文","stagedMentions":[{"token":"@原样","mention":{"kind":"human","target_id":null}}],"replyTarget":{"mention":null,"replyToId":"synthetic-reply","quotedSender":"quoted","quotedSnippet":"original quote"}});
    let payload_b = json!({"text":"B independent 原文","stagedMentions":[],"replyTarget":null});
    let packet_a = draft_packet(
        &hello_a,
        "personalDraft.checkpoint",
        json!({"scope":scope_a,"branchId":branch_a,"key":key,"expectedRevision":"0","revision":"1","payload":payload_a}),
    );
    let first_a = a.call("command.submit", packet_a.clone()).await;
    assert_eq!(first_a["phase"], "committed");
    let packet_b = draft_packet(
        &hello_b,
        "personalDraft.checkpoint",
        json!({"scope":scope_b,"branchId":branch_b,"key":key,"expectedRevision":"0","revision":"1","payload":payload_b}),
    );
    b.call("command.submit", packet_b).await;
    let first = a
        .call(
            "personalDraft.branches",
            json!({"scope":scope_a,"key":key,"limit":1}),
        )
        .await;
    assert_eq!(first["complete"], false);
    assert_eq!(first["branches"][0]["payload"], payload_a);
    let second = a
        .call(
            "personalDraft.branches",
            json!({"scope":scope_a,"key":key,"limit":1,"cursor":first["nextCursor"]}),
        )
        .await;
    assert_eq!(second["complete"], true);
    assert_eq!(second["branches"][0]["payload"], payload_b);
    let empty = json!({"text":"","stagedMentions":[],"replyTarget":null});
    a.call("command.submit",draft_packet(&hello_a,"personalDraft.checkpoint",json!({"scope":scope_a,"branchId":branch_a,"key":key,"expectedRevision":"1","revision":"2","payload":empty}))).await;
    let cleared = b
        .call("personalDraft.branches", json!({"scope":scope_b,"key":key}))
        .await;
    assert_eq!(cleared["branches"][0]["payload"], empty);
    assert_eq!(
        cleared["branches"][1]["payload"], payload_b,
        "own send-clear must preserve B independent unsent draft"
    );
    let bad=b.frame("command.submit",draft_packet(&hello_b,"personalDraft.checkpoint",json!({"scope":scope_b,"branchId":branch_a,"key":key,"expectedRevision":"2","revision":"99","payload":payload_b}))).await.unwrap();
    assert!(
        bad.get("error").is_some(),
        "other writer cannot raise A revision"
    );
    b.call("command.submit",draft_packet(&hello_b,"personalDraft.checkpoint",json!({"scope":scope_b,"branchId":branch_b,"key":key,"expectedRevision":"1","revision":"3","payload":{"text":"B newer","stagedMentions":[],"replyTarget":null}}))).await;
    let replacement = uuid::Uuid::new_v4().to_string();
    let resolve = |b_revision: &str| {
        draft_packet(
            &hello_a,
            "personalDraft.resolve",
            json!({"scope":scope_a,"key":key,"retiring":[{"branchId":branch_a,"revision":"2"},{"branchId":branch_b,"revision":b_revision}],"replacement":{"branchId":replacement,"revision":"1","payload":{"text":"explicit merged","stagedMentions":[],"replyTarget":null}}}),
        )
    };
    assert!(
        a.frame("command.submit", resolve("1"))
            .await
            .unwrap()
            .get("error")
            .is_some()
    );
    let unchanged = a
        .call("personalDraft.branches", json!({"scope":scope_a,"key":key}))
        .await;
    assert!(
        unchanged["branches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["retired"] == false)
    );
    let resolved = a.call("command.submit", resolve("3")).await;
    assert_eq!(resolved["result"]["retired"][1]["revision"], "3");
    let late=b.frame("command.submit",draft_packet(&hello_b,"personalDraft.checkpoint",json!({"scope":scope_b,"branchId":branch_b,"key":key,"expectedRevision":"3","revision":"100","payload":payload_b}))).await.unwrap();
    assert!(late.get("error").is_some());
    tokio::time::sleep(Duration::from_millis(30)).await;
    c.call("personalDraft.status", json!({})).await;
    assert!(
        c.events
            .iter()
            .all(|e| e["event"] != "personalDraft.changed"),
        "unapproved same-Core/crew grant must receive no personal event"
    );
    drop(c);
    drop(a);
    drop(b);
    stopped(running).await;
    let mut running = service(&data).await;
    let owner_hello2 = local(&mut running.owner, "system.hello", json!({"apiVersion":3})).await;
    assert_ne!(
        owner_hello["runtimeIdentity"]["runtimeBindingId"],
        owner_hello2["runtimeIdentity"]["runtimeBindingId"]
    );
    let mut a = RemoteRpc::connect(port, &cert_a, &identity.spki_sha256).await;
    let mut b = RemoteRpc::connect(port, &cert_b, &identity.spki_sha256).await;
    a.call("system.hello", json!({"apiVersion":3})).await;
    b.call("system.hello", json!({"apiVersion":3})).await;
    let resumed = a.call("personalDraft.status", json!({})).await;
    assert_eq!(resumed["spaceEpoch"], status_a["spaceEpoch"]);
    assert_eq!(
        resumed["member"]["memberId"],
        status_a["member"]["memberId"]
    );
    assert_eq!(
        resumed["member"]["writerId"],
        status_a["member"]["writerId"]
    );
    assert_eq!(resumed["available"], true);
    let original = a
        .call(
            "command.lookup",
            json!({"requestId":packet_a["requestId"],"dbId":identity.db_id}),
        )
        .await;
    assert_eq!(original["command"], packet_a["command"]);
    assert_eq!(original["runtimeBindingId"], packet_a["runtimeBindingId"]);
    assert_eq!(original["historical"], true);
    let old_fence = a
        .call("personalDraft.requestFence", json!({"original":packet_a}))
        .await;
    assert_eq!(old_fence["state"], "existingTerminal");
    let page = a
        .call("personalDraft.branches", json!({"scope":scope_a,"key":key}))
        .await;
    assert_eq!(
        page["branches"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["retired"] == false)
            .count(),
        1
    );
    assert_eq!(page["branches"][2]["payload"]["text"], "explicit merged");
    let current = local(&mut running.owner, "personalDraft.status", json!({})).await;
    local(&mut running.owner,"command.submit",draft_packet(&owner_hello2,"personalDraft.memberSet",json!({"spaceId":current["spaceId"],"spaceEpoch":current["spaceEpoch"],"expectedSpaceRevision":current["spaceRevision"],"grantId":grant_a.grant.id,"grantRevision":grant_a.grant.revision,"clientSpkiSha256":cert_a.pin(),"enabled":false}))).await;
    assert!(
        a.frame("personalDraft.branches", json!({"scope":scope_a,"key":key}))
            .await
            .unwrap()
            .get("error")
            .is_some()
    );
    assert!(
        a.frame(
            "command.lookup",
            json!({"requestId":first_a["requestId"],"dbId":identity.db_id})
        )
        .await
        .unwrap()
        .get("error")
        .is_some()
    );
    let removed = a.call("personalDraft.status", json!({})).await;
    assert_eq!(removed["available"], false);
    assert!(removed["member"].is_null() && removed["spaceId"].is_null());
    let current = local(&mut running.owner, "personalDraft.status", json!({})).await;
    local(&mut running.owner,"command.submit",draft_packet(&owner_hello2,"personalDraft.memberSet",json!({"spaceId":current["spaceId"],"spaceEpoch":current["spaceEpoch"],"expectedSpaceRevision":current["spaceRevision"],"grantId":grant_a.grant.id,"grantRevision":grant_a.grant.revision,"clientSpkiSha256":cert_a.pin(),"enabled":true}))).await;
    let new_member = a.call("personalDraft.status", json!({})).await;
    assert_ne!(
        new_member["member"]["memberId"],
        status_a["member"]["memberId"]
    );
    assert_ne!(
        new_member["member"]["writerId"],
        status_a["member"]["writerId"]
    );
    assert_eq!(new_member["available"], false);
    assert!(
        a.frame("personalDraft.requestFence", json!({"original":packet_a}))
            .await
            .unwrap()
            .get("error")
            .is_some()
    );
    local(&mut running.owner,"remote.revoke",json!({"grantId":grant_b.grant.id,"expectedRevision":grant_b.grant.revision,"note":"Synthetic draft test complete"})).await;
    let revoked = tokio::time::timeout(LIMIT, b.frame("personalDraft.status", json!({}))).await;
    assert!(!matches!(revoked,Ok(Ok(ref v))if v.get("result").is_some()));
    drop(a);
    drop(b);
    stopped(running).await;
    let store = crewd_store::Store::open_data_dir(&data).unwrap();
    let executions: i64 = store
        .conn()
        .query_row("SELECT count(*) FROM native_execution_runtimes", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        executions, 0,
        "draft sync never spawns or restores a session"
    );
}

#[tokio::test]
async fn core_personal_drafts_attached_agent_has_no_read_write_or_subscription_oracle() {
    let fixture = crewd_server::agent::testkit::Fixture::new().await;
    let (stop, rx) = tokio::sync::oneshot::channel();
    let server = fixture.spawn_owned_socket(rx).await;
    let mut client = RpcClient::connect(&fixture.data_dir().join("crewd.sock")).await;
    let hello = local(&mut client, "system.hello", json!({"apiVersion":3})).await;
    local(
        &mut client,
        "agent.attach",
        json!({"sessionId":fixture.worker,"token":fixture.token(&fixture.worker)}),
    )
    .await;
    for method in [
        "personalDraft.status",
        "personalDraft.members",
        "personalDraft.catalog",
        "personalDraft.branches",
        "personalDraft.requestFence",
    ] {
        let refused = client
            .call(
                method,
                json!({"deliberatelyInvalid":"no parsing or body oracle"}),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.0, crewd_proto::RpcError::FORBIDDEN, "{method}");
    }
    let refused = client
        .call("events.subscribe", json!({"topics":["personalDraft"]}))
        .await
        .unwrap_err();
    assert_eq!(refused.0, crewd_proto::RpcError::FORBIDDEN);
    let refused=client.call("command.submit",draft_packet(&hello,"personalDraft.ownerConfigure",json!({"ownerEpoch":"unavailable","expectedSpaceId":null,"expectedSpaceRevision":"0","localEnabled":true,"acknowledgeRestore":false}))).await.unwrap_err();
    assert_eq!(refused.0, crewd_proto::RpcError::FORBIDDEN);
    drop(client);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, server).await.unwrap().unwrap();
    let count = fixture
        .state
        .db
        .call(|s| {
            s.conn()
                .query_row("SELECT COUNT(*)FROM public_command_receipts", [], |r| {
                    r.get::<_, i64>(0)
                })
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn paired_human_reads_original_requirement_create_receipt_and_revocation_denies() {
    let dir = tempfile::tempdir().unwrap();
    let options = crewd_server::ServeOptions {
        orchestra: Some(crewd_server::OrchestraServe::default()),
        ..Default::default()
    };
    let mut running = service_with(dir.path(), options).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, grant) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let crew = remote
        .call(
            "crew.create",
            json!({
                "title":"synthetic original requirement lookup","workingDirectory":dir.path()
            }),
        )
        .await["id"]
        .clone();
    let request = uuid::Uuid::new_v4().to_string();
    let digest = "ecb359a14d8b9313dcd747cf16d7195f1aeffedf799012ae353fcfdfb55f9599";
    let q = json!({"requestId":request,"crewId":crew,"textSha256":digest});
    assert_eq!(
        remote.call("todo.createLookup", q.clone()).await,
        json!({
            "requestId":request,"status":"notFound"
        })
    );
    let original = remote
        .call(
            "todo.create",
            json!({
                "requestId":request,"crewId":crew,"text":"  Cafe\u{301}\n原文 "
            }),
        )
        .await;
    let receipt = remote.call("todo.createLookup", q.clone()).await;
    assert_eq!(
        receipt,
        json!({"requestId":request,"status":"committed","receipt":{
            "crewId":crew,"requirementId":original["requirementId"],"textSha256":digest,
            "createdAt":original["createdAt"]
        }})
    );
    local(&mut running.owner, "remote.revoke", json!({
        "grantId":grant.grant.id,"expectedRevision":grant.grant.revision,"note":"original receipt fixture complete"
    })).await;
    let denied = tokio::time::timeout(LIMIT, remote.frame("todo.createLookup", q))
        .await
        .unwrap();
    assert!(denied.is_err() || denied.unwrap()["error"].is_object());
    drop(remote);
    stopped(running).await;
}

#[tokio::test]
async fn paired_human_reads_exact_channel_responsibility_and_revocation_denies() {
    let dir = tempfile::tempdir().unwrap();
    let options = crewd_server::ServeOptions {
        orchestra: Some(crewd_server::OrchestraServe {
            config: crewd_server::orchestra::OrchestraConfig {
                heartbeat_ms: 20,
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut running = service_with(dir.path(), options).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, grant) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    let crew = remote
        .call(
            "crew.create",
            json!({"title":"synthetic responsibility scope","workingDirectory":dir.path()}),
        )
        .await["id"]
        .clone();
    let other_crew = remote
        .call(
            "crew.create",
            json!({"title":"separate synthetic scope","workingDirectory":dir.path()}),
        )
        .await["id"]
        .clone();
    let topic = remote
        .call(
            "channel.create",
            json!({"crewId":crew,"kind":"topic","name":"Synthetic linked work","members":[]}),
        )
        .await["id"]
        .clone();
    let work = remote
        .call(
            "work.create",
            json!({"crewId":crew,"channelId":topic,"requestId":uuid::Uuid::new_v4().to_string(),"title":"Synthetic unowned work"}),
        )
        .await;
    for index in 0..10 {
        let post = draft_packet(
            &hello,
            "message.post",
            json!({"channelId":topic,"text":format!("synthetic observation {index}")}),
        );
        assert_eq!(remote.call("command.submit", post).await["phase"], "committed");
    }
    let query = json!({"crewId":crew,"channelId":topic});
    let result = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            let result = remote.call("work.channelResponsibility", query.clone()).await;
            if result["items"][0]["phase"] == "open" {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("ordinary patrol did not observe the paired Human's exact linked work");
    let row = &result["items"][0];
    assert_eq!(result["crewId"], crew);
    assert_eq!(row["channelId"], topic);
    assert_eq!(row["workIds"], json!([work["id"]]));
    assert_eq!(row["noticePhase"], "not_queued");
    assert_eq!(
        remote
            .call("work.channelResponsibility", json!({"crewId":other_crew,"channelId":topic}))
            .await,
        json!({"crewId":other_crew,"items":[]})
    );
    assert_eq!(remote.call("work.channelResponsibility", query.clone()).await, result);

    local(&mut running.owner, "remote.revoke", json!({
        "grantId":grant.grant.id,"expectedRevision":grant.grant.revision,"note":"responsibility fixture complete"
    })).await;
    let denied = tokio::time::timeout(LIMIT, remote.frame("work.channelResponsibility", query))
        .await
        .unwrap();
    assert!(denied.is_err() || denied.unwrap()["error"].is_object());
    drop(remote);
    stopped(running).await;
}

#[tokio::test]
async fn paired_human_reads_message_correction_history_until_grant_is_revoked() {
    let dir = tempfile::tempdir().unwrap();
    let mut running = service(dir.path()).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, grant) = approved(&mut running.owner, &identity).await;
    let crew = local(
        &mut running.owner,
        "crew.create",
        json!({"title":"Synthetic correction history","workingDirectory":dir.path()}),
    )
    .await["id"]
        .clone();
    let channel = local(
        &mut running.owner,
        "channel.create",
        json!({"crewId":crew,"kind":"topic","name":"Correction source","members":[]}),
    )
    .await["id"]
        .clone();
    let hello = local(&mut running.owner, "system.hello", json!({})).await;
    let post = draft_packet(
        &hello,
        "message.post",
        json!({"channelId":channel,"text":"Synthetic source remains immutable"}),
    );
    let receipt = local(&mut running.owner, "command.submit", post).await;
    assert_eq!(receipt["phase"], "committed");
    let message = &receipt["result"]["id"];
    let current = local(
        &mut running.owner,
        "message.attribution.get",
        json!({"crewId":crew,"messageId":message}),
    )
    .await;
    let clear = json!({"crewId":crew,"messageId":message,"expectedRevision":current["revision"],"action":"clear"});
    let cleared = local(&mut running.owner, "message.attribution.set", clear).await;
    let query = json!({"crewId":crew,"channelId":channel,"messageId":message});
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let history = remote.call("message.attribution.history", query.clone()).await;
    assert_eq!(history["crewId"], crew);
    assert_eq!(history["channelId"], channel);
    assert_eq!(&history["messageId"], message);
    assert_eq!(history["items"].as_array().unwrap().len(), 1);
    assert_eq!(history["items"][0]["action"], "clear");
    assert_eq!(
        history["items"][0]["after"],
        json!({"action":"clear","targetUnavailable":false})
    );
    let reset = json!({"crewId":crew,"messageId":message,"expectedRevision":cleared["revision"],"action":"reset"});
    let denied = remote.frame("message.attribution.set", reset).await.unwrap();
    assert!(denied["error"].is_object());
    assert_eq!(remote.call("message.attribution.history", query.clone()).await, history);

    local(&mut running.owner, "remote.revoke", json!({
        "grantId":grant.grant.id,"expectedRevision":grant.grant.revision,"note":"correction history fixture complete"
    })).await;
    let denied = tokio::time::timeout(LIMIT, remote.frame("message.attribution.history", query))
        .await
        .unwrap();
    assert!(denied.is_err() || denied.unwrap()["error"].is_object());
    drop(remote);
    stopped(running).await;
}

#[tokio::test]
async fn paired_human_reads_exact_message_delivery_until_revocation() {
    use crewd_collab::{Sender, message_delivery, messages, sessions, wakes};
    use crewd_proto::{
        Mention, MentionKind, Role,
        message_delivery::{EXPLAIN, MessageWakePath, MessageWakeReason},
    };

    let dir = tempfile::tempdir().unwrap();
    let mut first = service(dir.path()).await;
    let (port, identity) = enabled(&mut first.owner).await;
    let (cert, _, grant) = approved(&mut first.owner, &identity).await;
    let crew = local(
        &mut first.owner,
        "crew.create",
        json!({"title":"Synthetic delivery evidence","workingDirectory":dir.path()}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    stopped(first).await;
    let channel = crewd_store::channels::lobby_id(&crew);
    // Persist the actual typed queue/evidence writer with the listener stopped.
    // The separate actor tests exercise how the production planner reaches it.
    let ((message, recipient), _) = {
        let mut store = crewd_store::Store::open_data_dir(dir.path()).unwrap();
        crewd_collab::run(&mut store, crewd_store::now_ms(), |tx| {
            let session = sessions::register(
                tx,
                sessions::NewSession {
                    crew_id: crew.clone(),
                    role: Role::Worker,
                    executor: "codex".into(),
                    title: Some("Delivery target".into()),
                    ..Default::default()
                },
            )?;
            let message = messages::post(
                tx,
                Sender::Human,
                messages::NewPost {
                    channel_id: channel.clone(),
                    text: "Synthetic source body must not be exposed".into(),
                    mentions: vec![Mention {
                        kind: MentionKind::Session,
                        target_id: Some(session.id.clone()),
                    }],
                    ..Default::default()
                },
            )?
            .message;
            let result = message_delivery::enqueue(
                tx,
                wakes::WakeRequest {
                    session: session.id.clone(),
                    kind: wakes::WakeKind::Message,
                    source: wakes::source::message(&message.id),
                    priority: wakes::WakePriority::Human,
                    not_before_ms: None,
                    note: None,
                    origin: None,
                },
                message_delivery::Capture {
                    message_id: message.id.clone(),
                    channel_id: message.channel_id.clone(),
                    seq: message.seq,
                    path: MessageWakePath::Live,
                    reasons: vec![MessageWakeReason::DirectedUnknown],
                },
            )?;
            assert!(matches!(result, wakes::Enqueued::New(_)));
            Ok((message.id, session.id))
        })
        .unwrap()
    };

    let mut reopened = service(dir.path()).await;
    tokio::time::timeout(LIMIT, async {
        loop {
            if local(&mut reopened.owner, "remote.status", json!({})).await["listening"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("persisted pairing listener did not reopen");
    let query = json!({"crewId":crew,"channelId":channel,"messageId":message});
    let expected = json!({
        "crewId":crew, "channelId":channel, "messageId":message, "routing":"targets_recorded",
        "items":[{"sessionId":recipient,"path":"live","priority":"human",
                  "reasons":["explicit_mention"],"state":"queued"}],
        "truncated":false
    });
    assert_eq!(
        local(&mut reopened.owner, EXPLAIN, query.clone()).await,
        expected
    );
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    assert_eq!(remote.call(EXPLAIN, query.clone()).await, expected);
    let wrong = remote
        .frame(
            EXPLAIN,
            json!({"crewId":crew,"channelId":"unrelated-channel","messageId":message}),
        )
        .await
        .unwrap();
    assert!(wrong["error"].is_object() && wrong.get("result").is_none());
    assert_eq!(remote.call(EXPLAIN, query.clone()).await, expected);

    local(&mut reopened.owner, "remote.revoke", json!({
        "grantId":grant.grant.id,"expectedRevision":grant.grant.revision,"note":"delivery evidence fixture complete"
    })).await;
    let denied = tokio::time::timeout(LIMIT, remote.frame(EXPLAIN, query))
        .await
        .unwrap();
    assert!(denied.is_err() || denied.unwrap()["error"].is_object());
    drop(remote);
    stopped(reopened).await;
}

#[tokio::test]
async fn paired_work_close_recovers_unread_original_after_restart_and_obeys_revocation() {
    use crewd_proto::{RpcError, commands::WORK_CLOSE_WITH_RECEIPT};

    let dir = tempfile::tempdir().unwrap();
    let mut running = service(dir.path()).await;
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _, accepted) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let hello = remote.call("system.hello", json!({})).await;
    assert_eq!(hello["remoteBinding"]["grantId"], accepted.grant.id);
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == WORK_CLOSE_WITH_RECEIPT)
    );
    let crew = remote
        .call(
            "crew.create",
            json!({
                "title":"isolated factual close receipt","workingDirectory":dir.path()
            }),
        )
        .await["id"]
        .clone();
    let work = remote
        .call(
            "work.create",
            json!({
                "crewId":crew,"title":"factual close over paired TLS"
            }),
        )
        .await;
    let untouched = remote
        .call(
            "work.create",
            json!({
                "crewId":crew,"title":"never close after revocation"
            }),
        )
        .await;
    let close = |hello: &Value, work: &Value| {
        draft_packet(
            hello,
            WORK_CLOSE_WITH_RECEIPT,
            json!({
                "crewId":crew,"id":work["id"],"expectedRevision":work["revision"],"kind":"done",
                "evidence":{"kind":"text","value":" paired evidence e\u{301} "},"note":" paired original close "
            }),
        )
    };
    let packet = close(&hello, &work);
    let lookup = json!({"dbId":packet["dbId"],"requestId":packet["requestId"]});
    let unread = json!({"id":1000,"method":"command.submit","params":packet});
    remote
        .io
        .get_mut()
        .write_all(format!("{unread}\n").as_bytes())
        .await
        .unwrap();

    let mut observer = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let committed = tokio::time::timeout(LIMIT, async {
        loop {
            let receipt = observer.call("command.lookup", lookup.clone()).await;
            if receipt["phase"] == "committed" {
                break receipt;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(committed["requestId"], packet["requestId"]);
    assert_eq!(committed["command"], packet["command"]);
    assert_eq!(committed["historical"], false);
    assert_eq!(
        committed["result"]["note"],
        packet["command"]["params"]["note"]
    );
    assert_eq!(
        committed["result"]["evidence"],
        packet["command"]["params"]["evidence"]
    );

    let (other_cert, _, _) = approved(&mut running.owner, &identity).await;
    let mut other = RemoteRpc::connect(port, &other_cert, &identity.spki_sha256).await;
    assert!(other.call("command.lookup", lookup.clone()).await.is_null());
    let copied = tokio::time::timeout(LIMIT, other.frame("command.submit", packet.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(copied["error"]["code"], RpcError::CONFLICT);
    assert!(other.call("command.lookup", lookup.clone()).await.is_null());
    drop((other, observer, remote));
    stopped(running).await;

    let mut running = service(dir.path()).await;
    tokio::time::timeout(LIMIT, async {
        loop {
            if local(&mut running.owner, "remote.status", json!({})).await["listening"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let reopened_hello = remote.call("system.hello", json!({})).await;
    let historical = remote.call("command.lookup", lookup.clone()).await;
    assert_eq!(historical["phase"], "committed");
    assert_eq!(historical["historical"], true);
    assert_eq!(historical["result"], committed["result"]);
    assert_eq!(remote.call("command.submit", packet).await, historical);
    let history = remote
        .call("work.closureHistory", json!({"id":work["id"]}))
        .await;
    assert_eq!(history["items"].as_array().unwrap().len(), 1);
    assert_eq!(history["items"][0]["seq"], historical["result"]["eventSeq"]);

    let denied_packet = close(&reopened_hello, &untouched);
    let denied_lookup =
        json!({"dbId":denied_packet["dbId"],"requestId":denied_packet["requestId"]});
    local(&mut running.owner, "remote.revoke", json!({
        "grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"fixture complete"
    })).await;
    for (method, params) in [
        ("command.submit", denied_packet),
        ("command.lookup", lookup),
    ] {
        if let Ok(frame) = tokio::time::timeout(LIMIT, remote.frame(method, params))
            .await
            .unwrap()
        {
            assert_eq!(frame["error"]["code"], RpcError::FORBIDDEN);
        }
    }
    assert_eq!(
        local(
            &mut running.owner,
            "work.get",
            json!({"id":untouched["id"]})
        )
        .await,
        untouched
    );
    assert!(
        local(&mut running.owner, "command.lookup", denied_lookup)
            .await
            .is_null()
    );
    drop(remote);
    stopped(running).await;
}
