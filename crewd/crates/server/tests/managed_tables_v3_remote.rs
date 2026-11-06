//! V3 through actual temporary TLS pairing, original grant and clean service reopen.
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
            // Ignore unrelated invalidations while awaiting this exact response.
        }
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
            device_label: "V3 receipt loopback device".into(),
        })
        .unwrap();
    let input = json!({"requestId":uuid::Uuid::new_v4().to_string(),"responseEnvelope":response});
    let accepted =
        serde_json::from_value(local(owner, "remote.accept", input.clone()).await).unwrap();
    (client, input, accepted)
}

async fn checked(peer: &mut RemoteRpc, scope: &Value, crew: &str) -> Value {
    let mut params = json!({"scope":scope,"crewId":crew,"title":"Paired V3 original table","purpose":"Typed cells and the same original UUID across a real service restart","excludingTableId":null,"cursor":null});
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..128 {
        let value = peer
            .call("managedTable.v3.defineCheck", params.clone())
            .await;
        if value["state"] == "complete" {
            assert_eq!(value["exactNameTaken"], false);
            assert!(value["definitionCheckToken"].is_string());
            return value;
        }
        assert_eq!(value["state"], "checking");
        assert!(value["definitionCheckToken"].is_null());
        let next = value["nextCursor"].as_str().unwrap().to_owned();
        assert!(seen.insert(next.clone()));
        params["cursor"] = next.into();
    }
    panic!("bounded synthetic definition traversal did not terminate")
}
#[tokio::test]
async fn paired_v3_all_methods_keep_original_codec_and_recheck_the_live_grant_after_restart() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("work")).unwrap();
    let mut running = service_with(root.path(), Default::default()).await;
    let hello = local(&mut running.owner, "system.hello", json!({})).await;
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "managedTables.v3")
    );
    let (port, identity) = enabled(&mut running.owner).await;
    let (cert, _input, accepted) = approved(&mut running.owner, &identity).await;
    let mut paired = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let binding = paired.call("system.hello", json!({})).await;
    assert_eq!(binding["remoteBinding"]["grantId"], accepted.grant.id);
    let crew = local(
        &mut running.owner,
        "crew.create",
        json!({"title":"V3 paired owning crew","workingDirectory":root.path().join("work")}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let scope = paired.call("managedObject.scope", json!({})).await;
    let proof = checked(&mut paired, &scope, &crew).await;
    let create_id = uuid::Uuid::new_v4().to_string();
    let created=paired.call("managedTable.v3.submit",json!({"scope":scope,"crewId":crew,"requestId":create_id,"operation":{"kind":"createTable","title":"Paired V3 original table","purpose":"Typed cells and the same original UUID across a real service restart","fields":[{"name":"Title","kind":{"kind":"text"},"nullable":false},{"name":"Boolean","kind":{"kind":"boolean"},"nullable":true},{"name":"Related crew","kind":{"kind":"link","target":{"kind":"crew"},"many":true},"nullable":true}],"titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters","definitionCheckToken":proof["definitionCheckToken"],"confirmNew":true}})).await;
    let table = created["effect"]["tableId"].as_str().unwrap().to_owned();
    let row_id = uuid::Uuid::new_v4().to_string();
    let add = json!({"scope":scope,"crewId":crew,"requestId":row_id,"operation":{"kind":"addRow","tableId":table,"expectedSchemaRevision":"1","patches":[{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Original paired row"}}},{"fieldId":"p2","change":{"kind":"setScalar","value":{"type":"boolean","value":false}}},{"fieldId":"p3","change":{"kind":"replaceLinks","values":[{"kind":"reference","target":{"kind":"crew","id":crew}}],"keepHiddenSetToken":null}}]}});
    // Send one request and deliberately never read its submit response.
    paired.next += 1;
    let mut raw = serde_json::to_vec(
        &json!({"id":paired.next,"method":"managedTable.v3.submit","params":add}),
    )
    .unwrap();
    raw.push(b'\n');
    paired.io.get_mut().write_all(&raw).await.unwrap();
    drop(paired);
    let mut recovered = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    recovered.call("system.hello", json!({})).await;
    let request = json!({"scope":scope,"crewId":crew,"requestId":row_id});
    let original = tokio::time::timeout(LIMIT, async {
        loop {
            let lookup = recovered
                .call("managedTable.v3.request.get", request.clone())
                .await;
            if lookup["status"] == "known" {
                break lookup["receipt"].clone();
            }
            assert_eq!(lookup["status"], "notFound");
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("read-only observation of the one original request");
    assert_eq!(original["requestId"], row_id);
    let row = original["effect"]["rowId"].as_str().unwrap().to_owned();
    drop(recovered);
    stopped(running).await;
    // Actual service instance is gone; reload persisted authority and grants.
    let mut running = service_with(root.path(), Default::default()).await;
    let status = local(&mut running.owner, "remote.status", json!({})).await;
    assert_eq!(status["listening"], true);
    assert_eq!(status["configuration"]["port"], port);
    assert_eq!(status["endpoint"]["port"], port);
    assert!(status["failure"].is_null());
    assert_eq!(status["identity"], serde_json::to_value(&identity).unwrap());
    let mut paired = RemoteRpc::connect(port, &cert, &identity.spki_sha256).await;
    let binding = paired.call("system.hello", json!({})).await;
    assert_eq!(binding["remoteBinding"]["grantId"], accepted.grant.id);
    let found = paired
        .call("managedTable.v3.request.get", request.clone())
        .await;
    assert_eq!(found["status"], "known");
    assert_eq!(found["receipt"], original);
    let wrong = tokio::time::timeout(
        LIMIT,
        paired.frame("managedTable.v2.request.get", request.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(wrong["error"]["code"], -32009);
    assert_eq!(
        wrong["error"]["data"]["refusal"],
        "resource.request_conflict"
    );
    let table_view = paired
        .call("managedTable.v3.get", json!({"id":table}))
        .await;
    assert_eq!(table_view["id"], table);
    assert_eq!(table_view["fields"][1]["kind"]["kind"], "boolean");
    assert!(
        paired
            .call("managedTable.v3.list", json!({"after":null}))
            .await["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == table)
    );
    let row_view = paired
        .call("managedTable.v3.row.get", json!({"id":row}))
        .await;
    assert_eq!(
        row_view["scalarCells"][1]["value"],
        json!({"type":"boolean","value":false})
    );
    assert_eq!(row_view["linkFieldIds"], json!(["p3"]));
    assert!(
        paired
            .call(
                "managedTable.v3.rows",
                json!({"tableId":table,"after":null})
            )
            .await["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == row)
    );
    let links_params =
        json!({"rowId":row,"fieldId":"p3","expectedSchemaRevision":"1","expectedRowRevision":"1"});
    let links = paired
        .call("managedTable.v3.row.links", links_params.clone())
        .await;
    assert_eq!(
        links["cell"]["visible"][0]["target"],
        json!({"kind":"crew","id":crew})
    );
    let history_params = json!({"source":{"kind":"row","id":row},"cursor":null});
    let history = paired
        .call("managedTable.v3.history", history_params.clone())
        .await;
    assert!(
        history["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["key"] == json!({"kind":"event","eventId":original["eventId"]}))
    );
    let value_params = json!({"source":{"kind":"row","id":row},"key":{"kind":"event","eventId":original["eventId"]},"path":{"kind":"links","fieldId":"p3","side":"after"},"cursor":null});
    let value = paired
        .call("managedTable.v3.history.value", value_params.clone())
        .await;
    assert_eq!(value["status"], "links");
    assert_eq!(
        value["visible"][0]["target"],
        json!({"kind":"crew","id":crew})
    );
    local(&mut running.owner,"remote.revoke",json!({"grantId":accepted.grant.id,"expectedRevision":accepted.grant.revision,"note":"Finish the original V3 paired grant"})).await;
    for (method, params) in [
        ("managedTable.v3.get", json!({"id":table})),
        ("managedTable.v3.list", json!({"after":null})),
        ("managedTable.v3.row.get", json!({"id":row})),
        (
            "managedTable.v3.rows",
            json!({"tableId":table,"after":null}),
        ),
        ("managedTable.v3.row.links", links_params),
        (
            "managedTable.v3.submit",
            json!({"scope":scope,"crewId":crew,"requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"archiveRow","rowId":row,"expectedSchemaRevision":"1","expectedRowRevision":"1","reason":"This revoked write must never commit"}}),
        ),
        ("managedTable.v3.request.get", request),
        (
            "managedTable.v3.defineCheck",
            json!({"scope":scope,"crewId":crew,"title":"Forbidden after revoke","purpose":"No new grant","excludingTableId":null,"cursor":null}),
        ),
        ("managedTable.v3.history", history_params),
        ("managedTable.v3.history.value", value_params),
    ] {
        let denied = tokio::time::timeout(LIMIT, paired.frame(method, params))
            .await
            .unwrap();
        assert!(
            denied.is_err() || denied.unwrap().get("error").is_some(),
            "{method}: revoked pairing yielded a result"
        );
    }
    let current = local(
        &mut running.owner,
        "managedTable.v3.row.get",
        json!({"id":row}),
    )
    .await;
    assert_eq!(
        current, row_view,
        "revoked lifecycle submit must not alter the original row"
    );
    let original_owner = local(
        &mut running.owner,
        "managedTable.v3.get",
        json!({"id":table}),
    )
    .await;
    assert_eq!(
        original_owner, table_view,
        "the real service stays alive for a lawful current Human"
    );
    drop(paired);
    stopped(running).await;
}
