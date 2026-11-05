//! Search through actual temporary TLS pairing and live owner grants; no provider.
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
    async fn call_frame(&mut self, method: &str, params: Value) -> Value {
        tokio::time::timeout(LIMIT, self.frame(method, params))
            .await
            .unwrap()
            .unwrap()
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

struct Service {
    owner: RpcClient,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
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
    tokio::time::timeout(LIMIT, async {
        while !socket.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned Unix socket must bind before the first actual hello");
    let mut owner = RpcClient::connect(&socket).await;
    // The kernel socket can accept before persisted remote startup completes.
    // A real dispatcher response proves initialization reached the accept loop.
    local(&mut owner, "system.hello", json!({})).await;
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
    assert_eq!(status["configuration"]["enabled"], true);
    assert_eq!(status["configuration"]["port"], port);
    assert_eq!(status["endpoint"]["port"], port);
    assert!(status["failure"].is_null());
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

async fn ready(peer: &mut RemoteRpc, params: Value) -> Value {
    tokio::time::timeout(LIMIT, async {
        loop {
            let page = peer.call("organization.search", params.clone()).await;
            if page["indexState"] == "ready" {
                return page;
            }
            assert_eq!(page["indexState"], "building");
            assert_eq!(page["items"], json!([]));
            assert!(page["snapshot"].is_null() && page["nextCursor"].is_null());
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("real provider-free maintenance must publish the complete source cut")
}

#[tokio::test]
async fn paired_search_and_expand_keep_real_grant_principals_and_stop_after_revoke() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("work")).unwrap();
    let mut running = service_with(root.path(), Default::default()).await;
    let hello = local(&mut running.owner, "system.hello", json!({})).await;
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "organizationSearch.v1")
    );
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _input, accepted) = approved(&mut running.owner, &identity).await;
    let mut remote = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let binding = remote.call("system.hello", json!({})).await;
    assert_eq!(binding["remoteBinding"]["grantId"], accepted.grant.id);
    assert_eq!(
        binding["remoteBinding"]["grantRevision"],
        accepted.grant.revision
    );
    let crew = local(
        &mut running.owner,
        "crew.create",
        json!({"title":"Paired search source","workingDirectory":root.path().join("work")}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let scope = local(&mut running.owner, "managedObject.scope", json!({})).await;
    let created=remote.call("managedTable.v2.submit",json!({"scope":scope,"crewId":crew,"requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"createTable","title":"paired-search-boundary table","purpose":"Live grant revalidation","titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters","fields":[{"name":"Title","kind":{"kind":"text"},"nullable":false}]}})).await;
    let table = created["effect"]["tableId"].as_str().unwrap().to_owned();
    let decision=remote.call("managedObject.submit",json!({"scope":scope,"crewId":crew,"requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"recordDecision","draft":{"question":"paired-search-boundary decision","choice":"Keep original relation","rationale":"Real TLS and current grant","about":[{"kind":"crew","id":crew}]}}})).await["object"]["decision"]["id"].as_str().unwrap().to_owned();
    let source = json!({"kind":"table","id":table});
    let links=remote.call("managedLink.list",json!({"query":{"direction":"outgoing","source":source,"includeAdapters":false},"after":null,"limit":20})).await;
    remote.call("managedLink.submit",json!({"scope":scope,"crewId":crew,"requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"link","source":source,"target":{"kind":"reference","target":{"kind":"decision","id":decision}},"expectedManualLinkRevision":links["manualLinkRevision"]}})).await;
    let query = json!({"query":"paired-search-boundary","in":"all","limit":1});
    let first = ready(&mut remote, query.clone()).await;
    assert_eq!(first["scope"], scope);
    let cursor = first["nextCursor"]
        .as_str()
        .expect("real table and decision need two pages")
        .to_owned();
    let mut local_query = query.clone();
    local_query["cursor"] = cursor.clone().into();
    assert!(
        tokio::time::timeout(
            LIMIT,
            running.owner.call("organization.search", local_query)
        )
        .await
        .unwrap()
        .is_err(),
        "local Human cannot borrow a paired Human's search cursor"
    );
    let (cert2, _input2, accepted2) = approved(&mut running.owner, &identity).await;
    assert_ne!(accepted.grant.id, accepted2.grant.id);
    let status = local(&mut running.owner, "remote.status", json!({})).await;
    assert_eq!(status["listening"], true);
    assert_eq!(status["endpoint"]["port"], port);
    assert_eq!(status["identity"], serde_json::to_value(&identity).unwrap());
    let mut other = RemoteRpc::connect(port, &cert2, &identity.spki_sha256).await;
    other.call("system.hello", json!({})).await;
    let mut borrowed = query.clone();
    borrowed["cursor"] = cursor.into();
    assert!(
        other
            .call_frame("organization.search", borrowed)
            .await
            .get("error")
            .is_some(),
        "another live grant is a different cursor principal"
    );
    let expanded = remote
        .call(
            "organization.search.expand",
            json!({"target":{"kind":"table","id":table},"limit":20}),
        )
        .await;
    assert_eq!(expanded["indexState"], "ready");
    assert!(
        expanded["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["direction"] == "outgoing"
                && x["neighbor"]["target"] == json!({"kind":"decision","id":decision}))
    );
    remote
        .call("events.subscribe", json!({"topics":["onto"]}))
        .await;
    let visible=local(&mut running.owner,"managedObject.submit",json!({"scope":scope,"crewId":crew,"requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"recordDecision","draft":{"question":"Visible before grant revoke","choice":"Observe one real event","rationale":"The paired stream is genuinely live","about":[{"kind":"crew","id":crew}]}}})).await["object"]["decision"]["id"].as_str().unwrap().to_owned();
    let event = remote.event().await;
    assert_eq!(event["event"], "onto.changed");
    assert!(
        event["data"]["objectIds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == &visible)
    );
    local(&mut running.owner,"remote.revoke",json!({"grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"Finish original paired search scope"})).await;
    let hidden=local(&mut running.owner,"managedObject.submit",json!({"scope":scope,"crewId":crew,"requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"recordDecision","draft":{"question":"HIDDEN_AFTER_SEARCH_REVOKE","choice":"No revoked output","rationale":"New source is visible only to current principals","about":[{"kind":"crew","id":crew}]}}})).await["object"]["decision"]["id"].as_str().unwrap().to_owned();
    for (method, params) in [
        (
            "organization.search",
            json!({"query":"HIDDEN_AFTER_SEARCH_REVOKE","limit":20}),
        ),
        (
            "organization.search.expand",
            json!({"target":{"kind":"table","id":table},"limit":20}),
        ),
    ] {
        let rejected = tokio::time::timeout(LIMIT, remote.frame(method, params))
            .await
            .unwrap();
        assert!(
            rejected.is_err() || rejected.unwrap().get("error").is_some(),
            "revoked live grant cannot receive new search/expand results"
        );
    }
    assert!(
        !serde_json::to_string(&remote.events)
            .unwrap()
            .contains(&hidden),
        "the revoked stream must not leak the later committed object"
    );
    let still_live = ready(
        &mut other,
        json!({"query":"HIDDEN_AFTER_SEARCH_REVOKE","limit":20}),
    )
    .await;
    assert!(
        still_live["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["target"]["id"] == hidden),
        "revocation denial cannot be explained by a dead service or stopped index"
    );
    drop(remote);
    drop(other);
    stopped(running).await;
}
