//! Independent actual socket coverage for manual message attribution.
#![cfg(feature = "testkit")]
use crewd_server::agent::testkit::{Fixture, RpcClient};
use serde_json::{Value, json};
use std::time::Duration;
const LIMIT: Duration = Duration::from_secs(10);
async fn call(c: &mut RpcClient, m: &str, p: Value) -> Result<Value, (i32, String)> {
    tokio::time::timeout(LIMIT, c.call(m, p)).await.unwrap()
}
async fn rpc(c: &mut RpcClient, m: &str, p: Value) -> Value {
    call(c, m, p).await.unwrap()
}
async fn attach(f: &Fixture, socket: &std::path::Path, id: &str) -> RpcClient {
    let mut c = RpcClient::connect(socket).await;
    rpc(
        &mut c,
        "agent.attach",
        json!({"sessionId":id,"token":f.token(id)}),
    )
    .await;
    c
}
// Every durable fixture table is compared, including allocator and request-key records.
async fn snapshot(f: &Fixture) -> Vec<String> {
    f.state
        .db
        .call(|s| {
            let mut q = s
                .conn()
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
            let tables = q
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut out = Vec::new();
            for name in tables {
                assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
                let mut stmt = s.conn().prepare(&format!("SELECT * FROM \"{name}\""))?;
                let count = stmt.column_count();
                let rows = stmt
                    .query_map([], |r| {
                        let mut v = Vec::new();
                        for i in 0..count {
                            v.push(format!("{:?}", r.get_ref(i)?));
                        }
                        Ok(v.join("|"))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let mut rows = rows;
                rows.sort();
                out.push(name);
                out.extend(rows);
            }
            Ok::<_, crewd_store::StoreError>(out)
        })
        .await
        .unwrap()
        .unwrap()
}
async fn close_socket(task: tokio::task::JoinHandle<()>, stop: tokio::sync::oneshot::Sender<()>) {
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
}

async fn topic(c: &mut RpcClient, f: &Fixture, name: &str) -> String {
    rpc(c,"channel.create",json!({"crewId":f.crew,"kind":"topic","name":name,"members":[{"kind":"session","id":f.worker}]})).await["id"].as_str().unwrap().to_owned()
}
async fn inspect(c: &mut RpcClient, crew: &str, id: &str) -> Value {
    rpc(
        c,
        "message.attribution.get",
        json!({"crewId":crew,"messageId":id}),
    )
    .await
}
async fn change(c: &mut RpcClient, v: &Value, action: &str, link: Option<Value>) -> Value {
    let mut p = json!({"crewId":v["crewId"],"messageId":v["messageId"],"expectedRevision":v["revision"],"action":action});
    if let Some(link) = link {
        p["link"] = link;
    }
    rpc(c, "message.attribution.set", p).await
}
#[tokio::test]
async fn human_override_clear_reset_cas_and_attached_denial_preserve_original_message() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let ch = topic(&mut human, &f, "attribution scope").await;
    let prior = f.human_post(&ch, "original topic", vec![]).await;
    let source = f
        .session_post(
            &f.worker,
            &ch,
            "answer whose independent attribution may be corrected",
            vec![],
        )
        .await;
    let start = inspect(&mut human, &f.crew, &source.id).await;
    let chosen = start["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["link"]["id"] == prior.id)
        .unwrap()["link"]
        .clone();
    let linked = change(&mut human, &start, "set", Some(chosen.clone())).await;
    assert_eq!(linked["source"], "human");
    assert_eq!(linked["link"], chosen);
    assert_ne!(linked["revision"], start["revision"]);
    let before = snapshot(&f).await;
    let stale=call(&mut human,"message.attribution.set",json!({"crewId":f.crew,"messageId":source.id,"expectedRevision":start["revision"],"action":"clear"})).await;
    assert_eq!(stale.unwrap_err().0, crewd_proto::RpcError::CONFLICT);
    assert_eq!(snapshot(&f).await, before);
    let mut worker = attach(&f, &socket, &f.worker).await;
    let denied=call(&mut worker,"message.attribution.set",json!({"crewId":f.crew,"messageId":source.id,"expectedRevision":linked["revision"],"action":"clear"})).await;
    assert_eq!(denied.unwrap_err().0, crewd_proto::RpcError::FORBIDDEN);
    assert_eq!(snapshot(&f).await, before);
    let cleared = change(&mut human, &linked, "clear", None).await;
    assert_eq!(cleared["source"], "human");
    assert_eq!(cleared["override"]["action"], "clear");
    assert!(cleared["link"].is_null());
    let reset = change(&mut human, &cleared, "reset", None).await;
    assert!(reset["override"].is_null());
    assert_ne!(
        reset["revision"], start["revision"],
        "reset must not revive an old CAS token"
    );
    assert_eq!(reset["source"], "none");
    let after_corrections = snapshot(&f).await;
    let query = json!({"crewId":f.crew,"channelId":ch,"messageId":source.id});
    let history = rpc(&mut human, "message.attribution.history", query.clone()).await;
    let rows = history["items"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["action"], "reset");
    assert!(rows[0].get("after").is_none());
    assert_eq!(rows[0]["before"]["action"], "clear");
    assert_eq!(
        rows[1]["before"]["target"],
        json!({"kind":"message","id":prior.id})
    );
    assert_eq!(rows[2]["after"]["target"], rows[1]["before"]["target"]);
    assert!(rows[2].get("before").is_none());
    assert!(history.get("nextCursor").is_none() && history["olderUnavailable"] == false);
    let delivery = rpc(&mut human, "message.deliveryExplain", query.clone()).await;
    assert_eq!(
        delivery,
        json!({
            "crewId":f.crew,"channelId":ch,"messageId":source.id,
            "routing":"unknown","items":[],"truncated":false
        })
    );
    let denied = call(&mut worker, "message.deliveryExplain", query.clone()).await;
    assert_eq!(denied.unwrap_err().0, crewd_proto::RpcError::FORBIDDEN);
    let denied = call(&mut worker, "message.attribution.history", query).await;
    assert_eq!(denied.unwrap_err().0, crewd_proto::RpcError::FORBIDDEN);
    let wrong_channel = call(
        &mut human,
        "message.attribution.history",
        json!({"crewId":f.crew,"channelId":"different-channel","messageId":source.id}),
    )
    .await;
    assert!(wrong_channel.is_err());
    assert_eq!(snapshot(&f).await, after_corrections);
    let id = source.id.clone();
    let reread = f
        .state
        .db
        .call_result(move |s| crewd_collab::messages::get(s.conn(), &id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(reread).unwrap(),
        serde_json::to_value(source).unwrap(),
        "attribution never edits original message semantics"
    );
    drop((human, worker));
    close_socket(task, stop).await;
}

#[tokio::test]
async fn explicit_resolution_reaches_old_targets_and_rejects_foreign_or_future_messages() {
    let f = Fixture::new().await;
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let task = f.spawn_owned_socket(stopping).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = RpcClient::connect(&socket).await;
    let ch = topic(&mut human, &f, "old attribution target").await;
    let other = topic(&mut human, &f, "unrelated private target").await;
    let old = f.human_post(&ch, "older than bounded sample", vec![]).await;
    for n in 0..24 {
        f.human_post(&ch, &format!("intervening message {n}"), vec![])
            .await;
    }
    let source = f.human_post(&ch, "needs old context", vec![]).await;
    let foreign = f.human_post(&other, "foreign context", vec![]).await;
    let future = f.human_post(&ch, "future context", vec![]).await;
    let sampled = inspect(&mut human, &f.crew, &source.id).await;
    assert_eq!(sampled["candidatesTruncated"], true);
    assert!(
        !sampled["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["link"]["id"] == old.id)
    );
    let resolved = rpc(
        &mut human,
        "message.attribution.get",
        json!({"crewId":f.crew,"messageId":source.id,"target":{"kind":"message","id":old.id}}),
    )
    .await;
    assert_eq!(resolved["resolvedCandidate"]["link"]["id"], old.id);
    let selected = change(
        &mut human,
        &resolved,
        "set",
        Some(resolved["resolvedCandidate"]["link"].clone()),
    )
    .await;
    assert_eq!(selected["source"], "human");
    let before = snapshot(&f).await;
    for bad in [foreign.id, future.id, source.id.clone()] {
        assert!(
            call(
                &mut human,
                "message.attribution.get",
                json!({"crewId":f.crew,"messageId":source.id,"target":{"kind":"message","id":bad}})
            )
            .await
            .is_err()
        );
    }
    assert_eq!(snapshot(&f).await, before);
    let mut worker = attach(&f, &socket, &f.worker).await;
    // The read surface must retain source-channel authority even when writes are Human-only.
    let hidden=rpc(&mut human,"channel.create",json!({"crewId":f.crew,"kind":"topic","name":"hidden source","members":[{"kind":"session","id":f.captain}]})).await;
    let secret = f
        .human_post(hidden["id"].as_str().unwrap(), "private source", vec![])
        .await;
    assert!(
        call(
            &mut worker,
            "message.attribution.get",
            json!({"crewId":f.crew,"messageId":secret.id})
        )
        .await
        .is_err()
    );
    drop((human, worker));
    close_socket(task, stop).await;
}

#[test]
fn file_database_reopen_preserves_override_reset_tombstone_and_audit() {
    use crewd_collab::{Sender, crews, message_attribution as attribution, messages, run};
    use crewd_proto::{judgments::JudgmentObjectKind, message_attribution::*};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("attribution.sqlite3");
    let mut s = crewd_store::Store::open(&path).unwrap();
    let now = crewd_store::now_ms();
    let crew = run(&mut s, now, |tx| {
        crews::create(
            tx,
            crews::NewCrew {
                title: "durable attribution".into(),
                working_directory: dir.path().to_string_lossy().into(),
                ..Default::default()
            },
        )
    })
    .unwrap()
    .0;
    let ch = crewd_store::channels::lobby_id(&crew.id);
    let prior = run(&mut s, now, |tx| {
        messages::post(
            tx,
            Sender::Human,
            messages::NewPost {
                channel_id: ch.clone(),
                text: "older".into(),
                ..Default::default()
            },
        )
    })
    .unwrap()
    .0
    .message;
    let source = run(&mut s, now, |tx| {
        messages::post(
            tx,
            Sender::Human,
            messages::NewPost {
                channel_id: ch.clone(),
                text: "source".into(),
                ..Default::default()
            },
        )
    })
    .unwrap()
    .0
    .message;
    let p = MessageAttributionGet {
        crew_id: crew.id,
        message_id: source.id.clone(),
        target: Some(AttributionTarget {
            kind: JudgmentObjectKind::Message,
            id: prior.id,
        }),
    };
    let first = attribution::get(s.conn(), &Sender::Human, p.clone()).unwrap();
    let chosen = run(&mut s, now, |tx| {
        attribution::set(
            tx,
            &Sender::Human,
            MessageAttributionSet {
                crew_id: p.crew_id.clone(),
                message_id: p.message_id.clone(),
                expected_revision: first.revision.clone(),
                action: AttributionAction::Set,
                link: Some(first.resolved_candidate.clone().unwrap().link),
            },
        )
    })
    .unwrap()
    .0;
    drop(s);
    let mut s = crewd_store::Store::open(&path).unwrap();
    let query = MessageAttributionGet { target: None, ..p };
    let reopened = attribution::get(s.conn(), &Sender::Human, query.clone()).unwrap();
    assert_eq!(reopened.revision, chosen.revision);
    assert_eq!(reopened.source, AttributionSource::Human);
    assert_eq!(reopened.link, chosen.link);
    let reset = run(&mut s, now + 1, |tx| {
        attribution::set(
            tx,
            &Sender::Human,
            MessageAttributionSet {
                crew_id: query.crew_id.clone(),
                message_id: query.message_id.clone(),
                expected_revision: reopened.revision,
                action: AttributionAction::Reset,
                link: None,
            },
        )
    })
    .unwrap()
    .0;
    drop(s);
    let mut s = crewd_store::Store::open(&path).unwrap();
    let again = attribution::get(s.conn(), &Sender::Human, query.clone()).unwrap();
    assert_eq!(again.revision, reset.revision);
    assert!(again.r#override.is_none());
    assert_ne!(again.revision, first.revision);
    let stale = run(&mut s, now + 2, |tx| {
        attribution::set(
            tx,
            &Sender::Human,
            MessageAttributionSet {
                crew_id: query.crew_id,
                message_id: query.message_id,
                expected_revision: first.revision,
                action: AttributionAction::Clear,
                link: None,
            },
        )
    });
    assert!(matches!(stale, Err(crewd_collab::CollabError::Conflict(_))));
    let count: i64 = s
        .conn()
        .query_row(
            "SELECT count(*) FROM meta WHERE key LIKE 'message.attribution.audit.v1:%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(messages::get(s.conn(), &source.id).unwrap(), source);
}
