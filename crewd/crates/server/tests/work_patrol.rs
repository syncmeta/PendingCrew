//! Actual Unix-socket projection, with synthetic data and no executor/model.
use crewd_collab::{patrol_runtime, wakes};
use crewd_proto::{Crew, Desired, Role, ServerFrame, Session, WorkCreateParams};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[tokio::test]
async fn patrol_socket_has_bounded_cursor_and_work_readback() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = crewd_store::Store::open(&dir.path().join("crewd.db")).unwrap();
    for (i, crew) in ["a", "b"].into_iter().enumerate() {
        crewd_store::crews::upsert(
            store.conn(),
            &Crew {
                id: crew.into(),
                title: crew.into(),
                number: Some(i as i64 + 1),
                created_at: "2026-10-01T00:00:00.000Z".into(),
                updated_at: "2026-10-01T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        crewd_store::sessions::upsert(
            store.conn(),
            &Session {
                id: format!("captain-{crew}"),
                crew_id: crew.into(),
                role: Role::Captain,
                desired: Desired::Running,
                executor: "codex".into(),
                updated_at: "2026-10-01T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        crewd_collab::run(&mut store, crewd_store::now_ms(), |tx| {
            crewd_collab::crews::set_captain(tx, crew, &format!("captain-{crew}"))?;
            crewd_collab::work::create(
                tx,
                &crewd_collab::Sender::Human,
                WorkCreateParams {
                    crew_id: crew.into(),
                    title: format!("work {crew}"),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    }
    let (topic, _) = crewd_collab::run(&mut store, crewd_store::now_ms(), |tx| {
        let topic = crewd_collab::channels::create(
            tx,
            &crewd_collab::Sender::Session("captain-a".into()),
            crewd_collab::channels::NewChannel {
                crew_id: "a".into(),
                kind: crewd_proto::ChannelKind::Topic,
                name: "synthetic socket responsibility source".into(),
                purpose: None,
                members: vec![],
            },
        )?;
        crewd_collab::channel_work::link(
            tx,
            &crewd_collab::Sender::Human,
            crewd_proto::ChannelWorkLinkParams {
                channel_id: topic.id.clone(),
                expected_link_revision: "0".into(),
                object: crewd_proto::ChannelWorkObject {
                    kind: crewd_proto::ChannelWorkKind::Work,
                    id: "W1".into(),
                },
            },
            true,
        )?;
        Ok(topic.id)
    })
    .unwrap();
    let now = crewd_store::now_ms();
    for _ in 0..8 {
        crewd_collab::run(&mut store, now, patrol_runtime::tick).unwrap();
    }
    let run = patrol_runtime::status(store.conn(), "a")
        .unwrap()
        .run
        .unwrap();
    assert!(run.completed_at_ms.is_some());
    let run_id = run.run_id;
    // This projection fixture supplies a synthetic executor receipt. Live runner
    // admission and dispatch are exercised separately in orchestra.
    let (wake_id, _) = crewd_collab::run(&mut store, now, |tx| {
        patrol_runtime::enqueue_brief(tx, "a", "captain-a", "fixture-generation")?;
        let wake = patrol_runtime::status(tx.conn(), "a")?
            .captain_brief
            .unwrap()
            .wake_id;
        wakes::claim(tx, &[wake])?;
        wakes::ack_delivered(tx, "captain-a", &[wake], wakes::Riders::Release, &[], &[])?;
        Ok(wake)
    })
    .unwrap();
    drop(store);
    let reopened = crewd_store::Store::open(&dir.path().join("crewd.db")).unwrap();
    assert_eq!(
        crewd_collab::patrol_runtime::status(reopened.conn(), "a")
            .unwrap()
            .run
            .unwrap()
            .run_id,
        run_id
    );
    drop(reopened);
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let path = dir.path().to_owned();
    let server = tokio::spawn(crewd_server::serve(path.clone(), async {
        let _ = stopping.await;
    }));
    let stream = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(stream) = UnixStream::connect(path.join("crewd.sock")).await {
                break stream;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let (rd, mut wr) = stream.into_split();
    let mut lines = BufReader::new(rd).lines();
    async fn call(
        wr: &mut tokio::net::unix::OwnedWriteHalf,
        lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
        id: u64,
        method: &str,
        params: Value,
    ) -> Value {
        wr.write_all(format!("{}\n", json!({"id":id,"method":method,"params":params})).as_bytes())
            .await
            .unwrap();
        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let ServerFrame::Response(response) = serde_json::from_str(&line).unwrap() else {
            panic!("unexpected event");
        };
        assert_eq!(response.id, id);
        response.into_result().unwrap()
    }
    let status = call(
        &mut wr,
        &mut lines,
        99,
        "work.patrolStatus",
        json!({"crewId":"a"}),
    )
    .await;
    assert_eq!(status["run"]["runId"], run_id);
    assert_eq!(status["crewId"], "a");
    assert_eq!(
        status["captainBrief"],
        json!({"runId":run_id,"captainSessionId":"captain-a","wakeId":wake_id,"phase":"delivered"})
    );
    let other = call(
        &mut wr,
        &mut lines,
        100,
        "work.patrolStatus",
        json!({"crewId":"b"}),
    )
    .await;
    assert!(other.get("captainBrief").is_none());
    let responsibility = call(
        &mut wr,
        &mut lines,
        101,
        "work.channelResponsibility",
        json!({"crewId":"a","channelId":topic}),
    )
    .await;
    assert_eq!(responsibility["crewId"], "a");
    assert_eq!(responsibility["items"].as_array().unwrap().len(), 1);
    let row = &responsibility["items"][0];
    assert_eq!(row["channelId"], topic);
    assert_eq!(row["phase"], "open");
    assert_eq!(row["noticePhase"], "not_queued");
    assert_eq!(row["workIds"], json!(["W1"]));
    assert!(row.get("handled").is_none() && responsibility.get("nextAfter").is_none());
    let foreign = call(
        &mut wr,
        &mut lines,
        102,
        "work.channelResponsibility",
        json!({"crewId":"b","channelId":topic}),
    )
    .await;
    assert_eq!(foreign, json!({"crewId":"b","items":[]}));
    let first = call(&mut wr, &mut lines, 1, "work.patrol", json!({"limit":1})).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["unowned"], true);
    assert_eq!(first["liveOnly"], true);
    assert_eq!(first["includeRemoved"], false);
    let id = first["items"][0]["id"].clone();
    assert_eq!(first["nextAfter"], id);
    let detail = call(&mut wr, &mut lines, 2, "work.get", json!({"id":id})).await;
    assert_eq!(detail["crewId"], "a");
    let second = call(
        &mut wr,
        &mut lines,
        3,
        "work.patrol",
        json!({"limit":1,"after":first["nextAfter"]}),
    )
    .await;
    assert_eq!(second["items"][0]["crewId"], "b");
    assert_ne!(second["items"][0]["id"], first["items"][0]["id"]);
    assert!(second["nextAfter"].is_null());
    let empty = call(
        &mut wr,
        &mut lines,
        4,
        "work.patrol",
        json!({"after":second["items"][0]["id"]}),
    )
    .await;
    assert_eq!(empty["items"], json!([]));
    for (id, scan, field) in [(5, "entries", "entries"), (6, "channels", "channels")] {
        let page = call(
            &mut wr,
            &mut lines,
            id,
            "work.patrol",
            json!({"scan":scan,"limit":1,"crewId":"b"}),
        )
        .await;
        assert_eq!(page["scan"], scan);
        assert_eq!(page[field], json!([]));
        assert_eq!(page["items"], json!([]));
    }
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // Reopening uses the same persisted source identities and performs no writes.
    let reopened = crewd_store::Store::open(&path.join("crewd.db")).unwrap();
    let changes = reopened.conn().total_changes();
    let page = crewd_collab::work_patrol::read(
        reopened.conn(),
        crewd_proto::WorkPatrolParams {
            limit: Some(1),
            ..Default::default()
        },
        crewd_store::now_ms(),
    )
    .unwrap();
    assert_eq!(page.items[0].id, first["items"][0]["id"].as_str().unwrap());
    for scan in [
        crewd_proto::WorkPatrolScan::Entries,
        crewd_proto::WorkPatrolScan::Channels,
    ] {
        let page = crewd_collab::work_patrol::read(
            reopened.conn(),
            crewd_proto::WorkPatrolParams {
                scan,
                ..Default::default()
            },
            crewd_store::now_ms(),
        )
        .unwrap();
        assert!(page.entries.is_empty());
        if scan == crewd_proto::WorkPatrolScan::Channels {
            assert_eq!(page.channels.len(), 1);
            assert_eq!(page.channels[0].id, topic);
        } else {
            assert!(page.channels.is_empty());
        }
    }
    let responsibility_reopened = crewd_collab::channel_responsibility::read(
        reopened.conn(),
        crewd_proto::ChannelResponsibilityParams {
            crew_id: "a".into(),
            channel_id: Some(topic),
            after: None,
        },
    )
    .unwrap();
    assert_eq!(serde_json::to_value(responsibility_reopened).unwrap(), responsibility);
    assert_eq!(reopened.conn().total_changes(), changes);
}
