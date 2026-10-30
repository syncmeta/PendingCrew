//! Real Unix transport with an intentionally unread submit response. No models.
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::oneshot,
};

async fn start(dir: &Path) -> (oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = oneshot::channel();
    let dir = dir.to_owned();
    let socket = dir.join("crewd.sock");
    let task = tokio::spawn(async move {
        crewd_server::serve(dir, async {
            let _ = rx.await;
        })
        .await
        .unwrap();
    });
    for _ in 0..100 {
        if UnixStream::connect(&socket).await.is_ok() {
            return (tx, task);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server did not become ready");
}
async fn call(stream: &mut BufReader<UnixStream>, method: &str, params: Value) -> Value {
    let request = json!({"id":1,"method":method,"params":params});
    stream
        .get_mut()
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let frame: Value = serde_json::from_str(&line).unwrap();
    assert!(frame.get("error").is_none(), "{frame}");
    frame["result"].clone()
}
async fn connect(dir: &Path) -> BufReader<UnixStream> {
    BufReader::new(UnixStream::connect(dir.join("crewd.sock")).await.unwrap())
}
#[tokio::test]
async fn unread_committed_response_survives_disconnect_and_service_restart() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let (stop, server) = start(dir.path()).await;
    let mut submitter = connect(dir.path()).await;
    let hello = call(&mut submitter, "system.hello", json!({})).await;
    let identity = &hello["runtimeIdentity"];
    let request = uuid::Uuid::new_v4().to_string();
    let command = json!({"operation":"crew.createFirstMessage","params":{"crew":{"title":"Unread response","workingDirectory":work,"parentId":null},"text":"Exactly one original message"}});
    let packet = json!({"id":2,"method":"command.submit","params":{"requestId":request,"dbId":identity["dbId"],"runtimeBindingId":identity["runtimeBindingId"],"command":command}});
    submitter
        .get_mut()
        .write_all(format!("{packet}\n").as_bytes())
        .await
        .unwrap();
    // Never read from submitter again. An independent observer proves commit,
    // avoiding timing-based guesses about whether the write reached the server.
    let lookup = json!({"requestId":request,"dbId":identity["dbId"]});
    let mut observer = connect(dir.path()).await;
    let receipt = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let receipt = call(&mut observer, "command.lookup", lookup.clone()).await;
            if receipt["phase"] == "committed" {
                break receipt;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    drop(submitter);
    drop(observer);
    let mut reconnected = connect(dir.path()).await;
    assert_eq!(
        call(&mut reconnected, "command.lookup", lookup.clone()).await,
        receipt
    );
    drop(reconnected);
    stop.send(()).unwrap();
    server.await.unwrap();
    let (stop, server) = start(dir.path()).await;
    let mut reopened = connect(dir.path()).await;
    let recovered = call(&mut reopened, "command.lookup", lookup).await;
    assert_eq!(recovered["requestId"], request);
    assert_eq!(recovered["command"], command);
    assert_eq!(recovered["phase"], "committed");
    assert_eq!(recovered["historical"], true);
    assert_eq!(recovered["runtimeBindingId"], identity["runtimeBindingId"]);
    assert_eq!(recovered["result"], receipt["result"]);
    drop(reopened);
    stop.send(()).unwrap();
    server.await.unwrap();
    let store = crewd_store::Store::open(&dir.path().join("crewd.db")).unwrap();
    for (table, expected) in [
        ("crews", 2),
        ("channels", 3),
        ("messages", 1),
        ("public_command_receipts", 1),
    ] {
        let count: i64 = store
            .conn()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, expected, "{table}");
    }
}
