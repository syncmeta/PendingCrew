//! Actual captured V3 write identity must remain valid at the FIFO boundary.
//! No provider, direct dispatcher call, artificial sleep, or replay of an unknown write.
#![cfg(feature = "testkit")]
use crewd_server::agent::testkit::Fixture;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
const LIMIT: Duration = Duration::from_secs(15);
struct Peer {
    rd: BufReader<tokio::net::unix::OwnedReadHalf>,
    wr: tokio::net::unix::OwnedWriteHalf,
    next: u64,
}
impl Peer {
    async fn connect(path: &Path) -> Self {
        let (rd, wr) = tokio::net::UnixStream::connect(path)
            .await
            .unwrap()
            .into_split();
        Self {
            rd: BufReader::new(rd),
            wr,
            next: 0,
        }
    }
    async fn frame(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let mut bytes =
            serde_json::to_vec(&json!({"id":id,"method":method,"params":params})).unwrap();
        bytes.push(b'\n');
        self.wr.write_all(&bytes).await.unwrap();
        tokio::time::timeout(LIMIT, async {
            loop {
                let mut line = String::new();
                assert!(
                    self.rd.read_line(&mut line).await.unwrap() > 0,
                    "owned Core closed connection"
                );
                let v: Value = serde_json::from_str(&line).unwrap();
                if v["id"] == id {
                    return v;
                }
            }
        })
        .await
        .unwrap()
    }
    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.frame(method, params).await;
        assert!(r.get("error").is_none(), "{method}: {r}");
        r["result"].clone()
    }
    async fn tool(&mut self, name: &str, args: Value) -> Value {
        self.ok("agent.call", json!({"tool":name,"args":args}))
            .await
    }
    async fn tool_ok(&mut self, name: &str, args: Value) -> Value {
        let r = self.tool(name, args).await;
        assert_eq!(r["isError"], false, "{name}: {r}");
        serde_json::from_str(r["text"].as_str().unwrap()).unwrap()
    }
}
async fn attached(f: &Fixture, path: &Path) -> Peer {
    let mut p = Peer::connect(path).await;
    p.ok(
        "agent.attach",
        json!({"sessionId":f.worker,"token":f.token(&f.worker)}),
    )
    .await;
    p
}
async fn snapshot(f: &Fixture) -> Vec<String> {
    f.state.db.call(|s| {
  let names=s.conn().prepare("SELECT name FROM sqlite_master WHERE type='table' AND (name NOT LIKE 'sqlite_%' OR name='sqlite_sequence') ORDER BY name")?.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
  let mut output=Vec::new();for name in names {let quoted=name.replace('"',"\"\"");let mut q=s.conn().prepare(&format!("SELECT * FROM \"{quoted}\""))?;let columns=q.column_count();let mut rows=q.query_map([],|r|{let mut cols=Vec::new();for index in 0..columns {cols.push(format!("{:?}",r.get_ref(index)?));}Ok(cols.join("|"))})?.collect::<rusqlite::Result<Vec<_>>>()?;rows.sort();output.push(name);output.extend(rows);}Ok::<_,crewd_store::StoreError>(output)
 }).await.unwrap().unwrap()
}

#[derive(Clone, Copy)]
enum Invalidation {
    Token,
    Role,
}

async fn captured_write_is_not_committed(invalidation: Invalidation) {
    use crewd_proto::Role;
    use crewd_server::agent::testkit::CallerStage;

    let f = Fixture::new().await;
    f.enable_public_command_identity().await;
    let (stop, closing) = tokio::sync::oneshot::channel();
    let server = f.spawn_owned_socket(closing).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = Peer::connect(&socket).await;
    let scope = human.ok("managedObject.scope", json!({})).await;
    // A real V2-created compatible table isolates the V3 write authorization gate
    // from definition-review pagination; AddRow is lawful for either current role.
    let created = human
        .ok(
            "managedTable.v2.submit",
            json!({
                "scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),
                "operation":{"kind":"createTable","title":"Captured V3 write",
                    "purpose":"A queued write cannot borrow a changed identity",
                    "titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters",
                    "fields":[{"name":"Title","kind":{"kind":"text"},"nullable":false}]}
            }),
        )
        .await;
    let table = created["effect"]["tableId"].as_str().unwrap().to_owned();
    let request = uuid::Uuid::new_v4().to_string();
    let params = json!({"contractVersion":3,"scope":scope,"crewId":f.crew,
        "requestId":request,"operation":{"kind":"addRow","tableId":table,
            "expectedSchemaRevision":"1","patches":[{"fieldId":"p1",
                "change":{"kind":"setScalar","value":{"type":"text","value":"Exactly once after a valid capture"}}}]}});
    let lookup = json!({"contractVersion":3,"scope":scope,"crewId":f.crew,"requestId":request});
    let mut worker = attached(&f, &socket).await;
    let initial = worker
        .tool_ok("managed_table_v2_request_get", lookup.clone())
        .await;
    assert_eq!(initial, json!({"status":"notFound"}));
    let mut gate = f.caller_gate(&f.worker, CallerStage::ManagedWriteCaptured);
    let pending_params = params.clone();
    let pending = tokio::spawn(async move {
        let reply = worker.tool("managed_table_v2_submit", pending_params).await;
        (worker, reply)
    });
    let evidence = tokio::time::timeout(LIMIT, gate.entered()).await.unwrap();
    assert!(evidence.targets.is_empty() && evidence.text.is_empty());
    match invalidation {
        Invalidation::Token => f.state.tokens.revoke(&f.worker),
        Invalidation::Role => f.set_role(&f.worker, Role::Captain).await,
    }
    // Snapshot AFTER the deliberate authority mutation. Any receipt, audit entry,
    // allocator/clock advance, row, search dirty fact or sequence change from the
    // now-forbidden write would make this full persisted-state comparison fail.
    let after_invalidation = snapshot(&f).await;
    gate.release();
    let (old_worker, denied) = tokio::time::timeout(LIMIT, pending).await.unwrap().unwrap();
    assert_eq!(
        denied["isError"], true,
        "captured write escaped revocation: {denied}"
    );
    // Agent refusals are localized explanations, not raw internal refusal identifiers.
    assert!(!denied["text"].as_str().unwrap().is_empty());
    assert_eq!(
        snapshot(&f).await,
        after_invalidation,
        "a refused captured write changed persisted business/receipt/audit/clock state"
    );
    drop(old_worker);

    // New attachment captures the newly valid token and current real role.
    // Read the original UUID first; the refusal must not occupy that namespace.
    let mut current = attached(&f, &socket).await;
    let before_lookup = snapshot(&f).await;
    assert_eq!(
        current
            .tool_ok("managed_table_v2_request_get", lookup.clone())
            .await,
        json!({"status":"notFound"})
    );
    assert_eq!(snapshot(&f).await, before_lookup);
    let accepted = current.tool_ok("managed_table_v2_submit", params).await;
    assert_eq!(accepted["requestId"], request);
    assert_eq!(accepted["effect"]["kind"], "rowAdded");
    assert_eq!(accepted["effect"]["tableId"], table);
    assert_eq!(accepted["effect"]["rowRevision"], "1");
    assert!(accepted["eventId"].is_string());
    let after_commit = snapshot(&f).await;
    let observed = current
        .tool_ok("managed_table_v2_request_get", lookup)
        .await;
    assert_eq!(observed, json!({"status":"known","receipt":accepted}));
    assert_eq!(snapshot(&f).await, after_commit);
    let rows = human
        .ok(
            "managedTable.v3.rows",
            json!({"tableId":table,"after":null,"includeArchived":false}),
        )
        .await;
    assert_eq!(rows["items"].as_array().unwrap().len(), 1);
    assert_eq!(rows["items"][0]["id"], accepted["effect"]["rowId"]);
    assert!(rows["nextAfter"].is_null());
    drop(current);
    drop(human);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, server).await.unwrap().unwrap();
}

#[tokio::test]
async fn v3_write_rechecks_revoked_original_token_after_capture_before_fifo() {
    captured_write_is_not_committed(Invalidation::Token).await;
}

#[tokio::test]
async fn v3_write_rechecks_actual_role_change_after_capture_before_fifo() {
    captured_write_is_not_committed(Invalidation::Role).await;
}
