use crewd_proto::{Crew, ServerFrame};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::oneshot;

async fn start(dir: &Path) -> oneshot::Sender<()> {
    seed(dir);
    let (tx, rx) = oneshot::channel::<()>();
    let d = dir.to_path_buf();
    tokio::spawn(async move {
        crewd_server::serve(d, async {
            let _ = rx.await;
        })
        .await
        .unwrap();
    });
    for _ in 0..100 {
        if UnixStream::connect(dir.join("crewd.sock")).await.is_ok() {
            return tx;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("crewd 没起来");
}

struct Client {
    rd: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    wr: tokio::net::unix::OwnedWriteHalf,
}

impl Client {
    async fn connect(dir: &Path) -> Self {
        let s = UnixStream::connect(dir.join("crewd.sock")).await.unwrap();
        let (r, w) = s.into_split();
        Self {
            rd: BufReader::new(r).lines(),
            wr: w,
        }
    }
    async fn send_raw(&mut self, line: &str) {
        self.wr
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn next(&mut self) -> ServerFrame {
        let line = tokio::time::timeout(Duration::from_secs(5), self.rd.next_line())
            .await
            .expect("等响应超时")
            .unwrap()
            .expect("连接被关");
        serde_json::from_str(&line).unwrap()
    }
    async fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send_raw(&json!({"id":id,"method":method,"params":params}).to_string())
            .await;
        match self.next().await {
            ServerFrame::Response(r) => {
                assert_eq!(r.id, id);
                if let Some(e) = r.error {
                    panic!("{method} 出错: {}", e.message);
                }
                // 线上是 `"result":null`（如 restart.pending 没有待答询问）；`Option<Value>` 把 JSON null 读成 None。
                r.result.unwrap_or(Value::Null)
            }
            ServerFrame::Event(e) => panic!("意外的事件 {e:?}"),
        }
    }
    async fn call_err(&mut self, id: u64, method: &str, params: Value) -> i32 {
        self.send_raw(&json!({"id":id,"method":method,"params":params}).to_string())
            .await;
        match self.next().await {
            ServerFrame::Response(r) => {
                r.error
                    .unwrap_or_else(|| panic!("{method} 应当失败，却返回 {:?}", r.result))
                    .code
            }
            ServerFrame::Event(e) => panic!("意外的事件 {e:?}"),
        }
    }
}

/// 合成数据：机组 a（1 号，机长 cap-a、worker w-a1）；w-a1 开了一条问、申请了一次审批；有一条重启询问。
fn seed(dir: &Path) {
    use crewd_proto::{Role, Session};
    let mut s = crewd_store::Store::open(&dir.join("crewd.db")).unwrap();
    crewd_store::crews::upsert(
        s.conn(),
        &Crew {
            id: "a".into(),
            title: "A".into(),
            number: Some(1),
            working_directory: Some("/tmp/example".into()),
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    for (id, role, ext) in [
        ("cap-a", Role::Captain, None),
        ("w-a1", Role::Worker, Some(2)),
    ] {
        crewd_store::sessions::upsert(
            s.conn(),
            &Session {
                id: id.into(),
                crew_id: "a".into(),
                role,
                executor: "claude".into(),
                display_name: Some(format!("名字-{id}")),
                ext,
                updated_at: "2026-10-01T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    }
    crewd_store::schema::backfill_v3(s.conn()).unwrap();
    crewd_collab::run(&mut s, crewd_store::now_ms(), |tx| {
        crewd_collab::asks::open(
            tx,
            "w-a1",
            crewd_collab::asks::OpenAsk {
                question: "用哪家支付？".into(),
                ..Default::default()
            },
        )?;
        crewd_collab::approvals::gate(
            tx,
            crewd_collab::approvals::NewApproval {
                session_id: "w-a1".into(),
                tool_name: "computer-use".into(),
                summary: "点击设置".into(),
                tool_use_id: None,
            },
        )?;
        crewd_collab::restart::open(tx, vec!["w-a1".into()])
    })
    .unwrap();
}

#[tokio::test]
async fn hello_crews_channels_and_messages() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    assert_eq!(
        c.call(1, "system.hello", Value::Null).await["apiVersion"],
        2
    );
    assert_eq!(c.call(2, "crew.list", json!({})).await[0]["id"], "a");
    let chans = c.call(3, "channel.list", json!({"crewId":"a"})).await;
    let ids: Vec<&str> = chans
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["channel"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["a/lobby", "a/human"]);
    let text = "发布 0.1.44 🚀，中文没问题";
    let m = c
        .call(
            4,
            "message.post",
            json!({"channelId":"a/lobby","text":text}),
        )
        .await;
    assert_eq!(m["senderKind"], "human");
    assert_eq!(m["channelId"], "a/lobby");
    let list = c
        .call(5, "message.list", json!({"channelId":"a/lobby"}))
        .await;
    assert_eq!(list[0]["text"], text);
    assert_eq!(
        c.call(
            6,
            "message.list",
            json!({"channelId":"a/lobby","afterSeq":m["seq"]})
        )
        .await,
        json!([])
    );
    let found = c.call(7, "message.search", json!({"query":"0.1.44"})).await;
    assert_eq!(found.as_array().unwrap().len(), 1);
    assert_eq!(
        c.call(8, "todo.list", json!({"crewId":"a","ledger":"agent"}))
            .await,
        json!([])
    );
    let dir_rows = c.call(9, "directory.list", Value::Null).await;
    let nums: Vec<&str> = dir_rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["number"].as_str().unwrap())
        .collect();
    assert_eq!(nums, ["1", "1-1", "1-2"]);
    assert_eq!(
        c.call(10, "session.get", json!({"sessionId":"w-a1"})).await["ext"],
        2
    );
}

#[tokio::test]
async fn params_are_strict_and_errors_are_typed() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    // 不是 JSON：-32700，id 0（原 bad_requests 测试里唯一不被别处覆盖的一条，挪到这里）。
    c.send_raw("this is not json").await;
    match c.next().await {
        ServerFrame::Response(r) => {
            assert_eq!(r.id, 0);
            assert_eq!(r.error.unwrap().code, -32700);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        c.call_err(1, "message.list", json!({"channelId":"a/lobby","bogus":1}))
            .await,
        -32602
    );
    assert_eq!(c.call_err(2, "system.hello", json!({"x":1})).await, -32602);
    assert_eq!(
        c.call_err(3, "whiteboard.list", json!({"crewId":"a"}))
            .await,
        -32601,
        "旧方法直接删，不留别名"
    );
    assert_eq!(
        c.call_err(4, "message.post", json!({"channelId":"zz","text":"x"}))
            .await,
        -32004
    );
    assert_eq!(
        c.call_err(
            5,
            "message.post",
            json!({"channelId":"a/lobby","text":"  "})
        )
        .await,
        -32602
    );
    assert_eq!(
        c.call_err(
            6,
            "channel.update",
            json!({"channelId":"a/lobby","archived":true})
        )
        .await,
        -32602
    );
    assert_eq!(
        c.call_err(7, "approval.decide", json!({"id":"ap-none","allow":true}))
            .await,
        -32004
    );
    // 连接还活着
    assert_eq!(
        c.call(8, "system.hello", Value::Null).await["server"],
        "crewd"
    );
    // R11：号码格式对、账本里没有 = 不存在（-32004），不是 -32602；格式不对才是 -32602。
    assert_eq!(
        c.call_err(
            9,
            "ask.answer",
            json!({"crewId":"a","ask":"H99","text":"x"})
        )
        .await,
        -32004
    );
    assert_eq!(
        c.call_err(
            10,
            "ask.answer",
            json!({"crewId":"a","ask":"not-an-h","text":"x"})
        )
        .await,
        -32602
    );
    // R11：parser 收 null 的方法（无参 / 全可选）确实收 null 与 {}。
    for (i, m) in [
        "system.hello",
        "crew.list",
        "approval.list",
        "restart.pending",
        "session.list",
        "ask.list",
        "directory.list",
    ]
    .into_iter()
    .enumerate()
    {
        c.call(100 + 2 * i as u64, m, Value::Null).await;
        c.call(101 + 2 * i as u64, m, json!({})).await;
    }
}

#[tokio::test]
async fn subscribers_receive_message_and_badge_events_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut sub = Client::connect(dir.path()).await;
    assert_eq!(
        sub.call(1, "events.subscribe", json!({"topics":["message","badge"]}))
            .await["topics"],
        json!(["message", "badge"])
    );
    let mut c = Client::connect(dir.path()).await;
    c.call(
        1,
        "message.post",
        json!({"channelId":"a/lobby","text":"有人吗"}),
    )
    .await;
    let names: Vec<String> = [sub.next().await, sub.next().await]
        .into_iter()
        .map(|f| match f {
            ServerFrame::Event(e) => e.event,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(names, ["message.appended", "badge.changed"]);
    assert_eq!(
        sub.call_err(2, "events.subscribe", json!({"topics":["whiteboard"]}))
            .await,
        -32602,
        "whiteboard 主题已删"
    );
    assert_eq!(
        sub.call_err(3, "events.subscribe", json!({"topics":[]}))
            .await,
        -32602
    );
}

#[tokio::test]
async fn asks_approvals_and_restart_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    let waits = c.call(1, "ask.list", Value::Null).await;
    assert_eq!(waits[0]["ask"]["question"], "用哪家支付？");
    let a = c
        .call(
            2,
            "ask.answer",
            json!({"crewId":"a","ask":"H1","text":"用甲"}),
        )
        .await;
    assert_eq!(a["state"], "answered");
    assert_eq!(
        c.call(3, "ask.list", json!({"rootCrewId":"a"})).await,
        json!([])
    );
    assert_eq!(
        c.call_err(
            4,
            "ask.answer",
            json!({"crewId":"a","ask":"H1","text":"再答"})
        )
        .await,
        -32602
    );
    let aps = c.call(5, "approval.list", Value::Null).await;
    // C10：客户端把 ask 与 approval 合并成一张「等你拍板」清单，两边都必须带机组与创建时间。
    assert!(
        aps[0]["crewId"].is_string() && aps[0]["requestedAt"].is_string(),
        "approval 带 crewId / requestedAt"
    );
    assert!(
        waits[0]["ask"]["crewId"].is_string() && waits[0]["ask"]["askedAt"].is_string(),
        "ask 带 crewId / askedAt"
    );
    let id = aps[0]["id"].as_str().unwrap().to_string();
    assert_eq!(
        c.call(6, "approval.decide", json!({"id":id,"allow":true}))
            .await["decision"],
        "allow"
    );
    assert_eq!(
        c.call_err(7, "approval.decide", json!({"id":id,"allow":false}))
            .await,
        -32009
    );
    let p = c.call(8, "restart.pending", Value::Null).await;
    let rid = p["id"].as_i64().unwrap();
    assert_eq!(
        c.call(9, "restart.answer", json!({"id":rid,"answer":"later"}))
            .await["answer"],
        "later"
    );
    assert_eq!(
        c.call(10, "restart.pending", Value::Null).await["id"],
        rid,
        "「先不」之后仍可整批恢复"
    );
    c.call(11, "restart.answer", json!({"id":rid,"answer":"resume"}))
        .await;
    assert_eq!(
        c.call(12, "restart.pending", Value::Null).await,
        Value::Null
    );
}

#[tokio::test]
async fn read_mark_clears_the_human_channel_badge() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    let chans = c.call(1, "channel.list", json!({"crewId":"a"})).await;
    let human = chans
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["channel"]["id"] == "a/human")
        .unwrap()
        .clone();
    assert_eq!(human["unreadForHuman"], 3, "问、审批、重启询问各一条公告");
    assert_eq!(human["humanMentions"], 3);
    let last = c
        .call(2, "message.list", json!({"channelId":"a/human"}))
        .await;
    let seq = last.as_array().unwrap().last().unwrap()["seq"].clone();
    let s = c
        .call(3, "read.mark", json!({"channelId":"a/human","seq":seq}))
        .await;
    assert_eq!(
        (s["unreadForHuman"].clone(), s["humanMentions"].clone()),
        (json!(0), json!(0))
    );
}

#[tokio::test]
async fn channels_can_be_created_watched_and_handed_over() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    let ch = c
        .call(
            1,
            "channel.create",
            json!({"crewId":"a","kind":"topic","name":"发版","members":[{"kind":"session","id":"w-a1"}]}),
        )
        .await;
    assert_eq!(ch["owner"], json!({"kind":"human"}));
    let id = ch["id"].as_str().unwrap().to_string();
    assert_eq!(
        c.call(2, "channel.watch", json!({"channelId":id,"on":true}))
            .await["watched"],
        true
    );
    let ch = c
        .call(
            3,
            "channel.setOwner",
            json!({"channelId":id,"owner":{"kind":"session","id":"w-a1"}}),
        )
        .await;
    assert_eq!(ch["owner"], json!({"kind":"session","id":"w-a1"}));
    assert_eq!(
        c.call_err(
            4,
            "channel.create",
            json!({"crewId":"a","kind":"lobby","name":"x"})
        )
        .await,
        -32602
    );
    let made = c
        .call(
            5,
            "crew.create",
            json!({"title":"新机组","workingDirectory":"/tmp/example","parentId":"a"}),
        )
        .await;
    let detail = c.call(6, "crew.get", json!({"crewId":made["id"]})).await;
    assert_eq!(detail["rootId"], "a");
    c.call(
        7,
        "crew.release",
        json!({"crewId":made["id"],"fromParentId":"a"}),
    )
    .await;
    assert_eq!(
        c.call(8, "crew.delete", json!({"crewId":made["id"]})).await,
        json!({})
    );
}

/// C1：侧栏次序按父边分层。`crew.reorder {parentId}` 只改这个父名下的次序；同一批机组在另一个父下的次序不动；
/// 不带 parentId 只排顶层（`Crew.position`），不动任何父的 `childIds`。
#[tokio::test]
async fn crew_reorder_is_per_parent_edge() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    let p2 = c
        .call(
            1,
            "crew.create",
            json!({"title":"另一个父","workingDirectory":"/tmp/example"}),
        )
        .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut kids = Vec::new();
    for (i, t) in ["子甲", "子乙"].into_iter().enumerate() {
        let k = c
            .call(
                10 + i as u64,
                "crew.create",
                json!({"title":t,"workingDirectory":"/tmp/example","parentId":"a"}),
            )
            .await["id"]
            .as_str()
            .unwrap()
            .to_string();
        c.call(
            20 + i as u64,
            "crew.adopt",
            json!({"crewId":k,"parentId":p2}),
        )
        .await;
        kids.push(k);
    }
    let (k1, k2) = (kids[0].clone(), kids[1].clone());
    let child_ids = |v: &Value, id: &str| -> Vec<String> {
        let crew = v
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == id)
            .unwrap_or_else(|| panic!("crew.list 里没有 {id}"));
        serde_json::from_value(crew["childIds"].clone()).unwrap()
    };
    let before = c.call(30, "crew.list", Value::Null).await;
    assert_eq!(child_ids(&before, "a"), [k1.clone(), k2.clone()]);
    assert_eq!(child_ids(&before, &p2), [k1.clone(), k2.clone()]);

    let r = c
        .call(
            31,
            "crew.reorder",
            json!({"parentId":"a","crewIds":[k2.clone(), k1.clone()]}),
        )
        .await;
    let order: Vec<&str> = r
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(order, [k2.as_str(), k1.as_str()], "回这一层的新顺序");
    let after = c.call(32, "crew.list", Value::Null).await;
    assert_eq!(child_ids(&after, "a"), [k2.clone(), k1.clone()]);
    assert_eq!(
        child_ids(&after, &p2),
        [k1.clone(), k2.clone()],
        "另一个父下的次序不动"
    );

    // 顶层：只有根机组（a、p2）；不带 parentId 改 position，不动任何 childIds。
    c.call(33, "crew.reorder", json!({"crewIds":[p2.clone(), "a"]}))
        .await;
    let top = c.call(34, "crew.list", Value::Null).await;
    let pos = |id: &str| {
        top.as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == id)
            .unwrap()["position"]
            .as_i64()
            .unwrap()
    };
    assert!(pos(&p2) < pos("a"), "顶层按人排的次序");
    assert_eq!(child_ids(&top, "a"), [k2.clone(), k1.clone()]);
    assert_eq!(child_ids(&top, &p2), [k1.clone(), k2.clone()]);
    // 名单必须恰好是这一层：顶层名单里混进子机组 = -32602。
    assert_eq!(
        c.call_err(35, "crew.reorder", json!({"crewIds":[p2, "a", k1]}))
            .await,
        -32602
    );
}

#[tokio::test]
async fn second_instance_on_same_dir_is_refused_and_stale_socket_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("crewd.sock"), b"stale").unwrap();
    let _stop = start(dir.path()).await;
    let err = crewd_server::serve(dir.path().to_path_buf(), async {})
        .await
        .unwrap_err();
    assert!(err.to_string().contains("另一个 crewd"), "{err}");
}

#[tokio::test]
async fn invalid_utf8_gets_parse_error_and_connection_survives() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    c.wr.write_all(&[0xff, 0xfe, b'\n']).await.unwrap();
    match c.next().await {
        ServerFrame::Response(r) => {
            assert_eq!(r.id, 0);
            assert_eq!(r.error.unwrap().code, -32700);
        }
        other => panic!("{other:?}"),
    }
    let h = c.call(1, "system.hello", Value::Null).await;
    assert_eq!(h["apiVersion"], 2);
}

// R4：以下两条测的是连接层的订阅语义（与白板无关），从 whiteboard 主题移植到 message 主题。

#[tokio::test]
async fn repeated_subscribe_does_not_duplicate_events() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut sub = Client::connect(dir.path()).await;
    sub.call(1, "events.subscribe", json!({"topics":["message"]}))
        .await;
    sub.call(2, "events.subscribe", json!({"topics":["message"]}))
        .await;

    let mut poster = Client::connect(dir.path()).await;
    poster
        .call(
            1,
            "message.post",
            json!({"channelId":"a/lobby","text":"一次"}),
        )
        .await;

    match sub.next().await {
        ServerFrame::Event(e) => {
            assert_eq!(e.event, "message.appended");
            assert_eq!(e.data["text"], "一次");
        }
        other => panic!("{other:?}"),
    }
    // 给可能存在的重复事件留出时间，再确认下一帧就是 hello 的响应。
    tokio::time::sleep(Duration::from_millis(100)).await;
    let h = sub.call(3, "system.hello", Value::Null).await;
    assert_eq!(h["apiVersion"], 2);
}

#[tokio::test]
async fn subscribe_with_unknown_topic_is_invalid_params_and_subscribes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut sub = Client::connect(dir.path()).await;
    for (id, topics) in [(1, json!(["todo"])), (2, json!(["message", "nope"]))] {
        sub.send_raw(
            &json!({"id":id,"method":"events.subscribe","params":{"topics":topics}}).to_string(),
        )
        .await;
        match sub.next().await {
            ServerFrame::Response(r) => {
                assert_eq!(r.id, id);
                let e = r.error.expect("未知主题必须报错");
                assert_eq!(e.code, -32602);
                assert!(e.message.contains("message"), "{}", e.message);
            }
            other => panic!("{other:?}"),
        }
    }

    // 被拒的订阅（哪怕夹带了已知主题）不生效：之后的消息事件不会推过来。
    let mut poster = Client::connect(dir.path()).await;
    poster
        .call(
            1,
            "message.post",
            json!({"channelId":"a/lobby","text":"不该收到"}),
        )
        .await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    let h = sub.call(2, "system.hello", Value::Null).await;
    assert_eq!(h["apiVersion"], 2);
}
