use crewd_proto::{Crew, ServerFrame};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::oneshot;

async fn start(dir: &Path) -> oneshot::Sender<()> {
    // 先建一个机组，post 需要它存在
    {
        let s = crewd_store::Store::open(&dir.join("crewd.db")).unwrap();
        crewd_store::crews::upsert(
            s.conn(),
            &Crew {
                id: "a".into(),
                title: "A".into(),
                number: Some(1),
                builtin: false,
                parent_ids: vec![],
                working_directory: None,
                captain_executor: None,
                hidden_at: None,
                created_at: "2026-10-01T00:00:00.000Z".into(),
                updated_at: "2026-10-01T00:00:00.000Z".into(),
            },
            None,
        )
        .unwrap();
    }
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
                r.result.unwrap()
            }
            ServerFrame::Event(e) => panic!("意外的事件 {e:?}"),
        }
    }
}

#[tokio::test]
async fn hello_post_list_and_todo_list() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    let h = c.call(1, "system.hello", Value::Null).await;
    assert_eq!(h["apiVersion"], 1);
    assert_eq!(h["server"], "crewd");

    let crews = c.call(2, "crew.list", Value::Null).await;
    assert_eq!(crews[0]["id"], "a");

    let text = "发布 0.1.44 🚀，中文没问题";
    let m = c
        .call(
            3,
            "whiteboard.post",
            json!({"crewId":"a","text":text,"senderKind":"user"}),
        )
        .await;
    assert!(m["seq"].as_i64().unwrap() > 0);
    let list = c.call(4, "whiteboard.list", json!({"crewId":"a"})).await;
    assert_eq!(list[0]["text"], text);
    let none = c
        .call(
            5,
            "whiteboard.list",
            json!({"crewId":"a","afterSeq":m["seq"]}),
        )
        .await;
    assert_eq!(none, json!([]));

    let todos = c
        .call(6, "todo.list", json!({"crewId":"a","ledger":"agent"}))
        .await;
    assert_eq!(todos, json!([]));
}

#[tokio::test]
async fn bad_requests_get_errors_and_connection_survives() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;

    c.send_raw("this is not json").await;
    match c.next().await {
        ServerFrame::Response(r) => {
            assert_eq!(r.id, 0);
            assert_eq!(r.error.unwrap().code, -32700);
        }
        other => panic!("{other:?}"),
    }
    c.send_raw(r#"{"id":2,"method":"no.such"}"#).await;
    match c.next().await {
        ServerFrame::Response(r) => assert_eq!(r.error.unwrap().code, -32601),
        other => panic!("{other:?}"),
    }
    c.send_raw(
        r#"{"id":3,"method":"whiteboard.post","params":{"crewId":"zz","text":"x","senderKind":"user"}}"#,
    )
    .await;
    match c.next().await {
        ServerFrame::Response(r) => assert_eq!(r.error.unwrap().code, -32602),
        other => panic!("{other:?}"),
    }
    c.send_raw(
        r#"{"id":4,"method":"whiteboard.post","params":{"crewId":"a","text":"  ","senderKind":"user"}}"#,
    )
    .await;
    match c.next().await {
        ServerFrame::Response(r) => assert_eq!(r.error.unwrap().code, -32602),
        other => panic!("{other:?}"),
    }
    let h = c.call(5, "system.hello", Value::Null).await;
    assert_eq!(h["apiVersion"], 1);
}

#[tokio::test]
async fn subscribers_receive_whiteboard_events() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut sub = Client::connect(dir.path()).await;
    let ok = sub
        .call(1, "events.subscribe", json!({"topics":["whiteboard"]}))
        .await;
    assert_eq!(ok["topics"], json!(["whiteboard"]));

    let mut poster = Client::connect(dir.path()).await;
    poster
        .call(
            1,
            "whiteboard.post",
            json!({"crewId":"a","text":"通知我","senderKind":"user"}),
        )
        .await;

    match sub.next().await {
        ServerFrame::Event(e) => {
            assert_eq!(e.event, "whiteboard.appended");
            assert_eq!(e.data["text"], "通知我");
        }
        other => panic!("{other:?}"),
    }
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
    assert_eq!(h["apiVersion"], 1);
}

#[tokio::test]
async fn repeated_subscribe_does_not_duplicate_events() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut sub = Client::connect(dir.path()).await;
    sub.call(1, "events.subscribe", json!({"topics":["whiteboard"]}))
        .await;
    sub.call(2, "events.subscribe", json!({"topics":["whiteboard"]}))
        .await;

    let mut poster = Client::connect(dir.path()).await;
    poster
        .call(
            1,
            "whiteboard.post",
            json!({"crewId":"a","text":"一次","senderKind":"user"}),
        )
        .await;

    match sub.next().await {
        ServerFrame::Event(e) => assert_eq!(e.data["text"], "一次"),
        other => panic!("{other:?}"),
    }
    // 给可能存在的重复事件留出时间，再确认下一帧就是 hello 的响应。
    tokio::time::sleep(Duration::from_millis(100)).await;
    let h = sub.call(3, "system.hello", Value::Null).await;
    assert_eq!(h["apiVersion"], 1);
}

#[tokio::test]
async fn subscribe_with_unknown_topic_is_invalid_params_and_subscribes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut sub = Client::connect(dir.path()).await;
    for (id, topics) in [(1, json!(["todo"])), (2, json!(["whiteboard", "nope"]))] {
        sub.send_raw(
            &json!({"id":id,"method":"events.subscribe","params":{"topics":topics}}).to_string(),
        )
        .await;
        match sub.next().await {
            ServerFrame::Response(r) => {
                assert_eq!(r.id, id);
                let e = r.error.expect("未知主题必须报错");
                assert_eq!(e.code, -32602);
                assert!(e.message.contains("whiteboard"), "{}", e.message);
            }
            other => panic!("{other:?}"),
        }
    }

    // 被拒的订阅（哪怕夹带了已知主题）不生效：之后的白板事件不会推过来。
    let mut poster = Client::connect(dir.path()).await;
    poster
        .call(
            1,
            "whiteboard.post",
            json!({"crewId":"a","text":"不该收到","senderKind":"user"}),
        )
        .await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    let h = sub.call(2, "system.hello", Value::Null).await;
    assert_eq!(h["apiVersion"], 1);
}

#[tokio::test]
async fn subscribe_with_empty_topics_is_invalid_params() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    c.send_raw(r#"{"id":1,"method":"events.subscribe","params":{"topics":[]}}"#)
        .await;
    match c.next().await {
        ServerFrame::Response(r) => {
            assert_eq!(r.id, 1);
            assert_eq!(r.error.unwrap().code, -32602);
        }
        other => panic!("{other:?}"),
    }
}
