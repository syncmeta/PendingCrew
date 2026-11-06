//! V3 integration candidate: actual owned Unix connection/attached token.
//! Prepared against frozen v0.4.1; no backend bypass, provider or fake-ready data.
#![cfg(feature = "testkit")]
use crewd_server::agent::testkit::Fixture;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
const LIMIT: Duration = Duration::from_secs(15);
const SIX: [&str; 6] = [
    "managed_table_v2_get",
    "managed_tables_v2_list",
    "managed_row_v2_get",
    "managed_rows_v2_list",
    "managed_table_v2_submit",
    "managed_table_v2_request_get",
];
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
  let names=s.conn().prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
  let mut output=Vec::new();for name in names {let quoted=name.replace('"',"\"\"");let mut q=s.conn().prepare(&format!("SELECT * FROM \"{quoted}\""))?;let columns=q.column_count();let mut rows=q.query_map([],|r|{let mut cols=Vec::new();for index in 0..columns {cols.push(format!("{:?}",r.get_ref(index)?));}Ok(cols.join("|"))})?.collect::<rusqlite::Result<Vec<_>>>()?;rows.sort();output.push(name);output.extend(rows);}Ok::<_,crewd_store::StoreError>(output)
 }).await.unwrap().unwrap()
}
async fn checked(
    p: &mut Peer,
    scope: &Value,
    crew: &str,
    title: &str,
    purpose: Option<&str>,
    excluding: Option<&str>,
) -> Value {
    let mut args = json!({"contractVersion":3,"action":"defineCheck","scope":scope,"crewId":crew,"title":title,"purpose":purpose,"excludingTableId":excluding,"cursor":null});
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..128 {
        let v = p.tool_ok("managed_table_v2_get", args.clone()).await;
        if v["state"] == "complete" {
            assert_eq!(v["exactNameTaken"], false);
            assert!(v["definitionCheckToken"].is_string());
            return v;
        }
        assert_eq!(v["state"], "checking");
        assert!(v["definitionCheckToken"].is_null());
        let next = v["nextCursor"].as_str().unwrap().to_owned();
        assert!(seen.insert(next.clone()));
        args["cursor"] = next.into();
    }
    panic!("bounded fixture definition scan failed to terminate")
}
fn new_table(
    scope: &Value,
    crew: &str,
    id: &str,
    title: &str,
    purpose: &str,
    proof: &Value,
    extra: bool,
) -> Value {
    let mut fields = vec![json!({"name":"Title","kind":{"kind":"text"},"nullable":false})];
    if extra {
        fields.push(json!({"name":"Flag","kind":{"kind":"boolean"},"nullable":true}));
    }
    json!({"contractVersion":3,"scope":scope,"crewId":crew,"requestId":id,"operation":{"kind":"createTable","title":title,"purpose":purpose,"fields":fields,"titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters","definitionCheckToken":proof["definitionCheckToken"],"confirmNew":true}})
}
#[tokio::test]
async fn six_tools_preserve_v2_defaults_reject_selector_downgrades_and_never_reinterpret_original_uuid()
 {
    let f = Fixture::new().await;
    f.enable_public_command_identity().await;
    let (stop, closing) = tokio::sync::oneshot::channel();
    let server = f.spawn_owned_socket(closing).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = Peer::connect(&socket).await;
    let hello = human.ok("system.hello", json!({})).await;
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "managedTables.v3")
    );
    let scope = human.ok("managedObject.scope", json!({})).await;
    let mut a = attached(&f, &socket).await;
    let original_id = uuid::Uuid::new_v4().to_string();
    let create = json!({"scope":scope,"crewId":f.crew,"requestId":original_id,"operation":{"kind":"createTable","title":"Original V2 table","purpose":"The first and only initial fixture table","fields":[{"name":"Title","kind":{"kind":"text"},"nullable":false}],"titleFieldIndex":0,"rowWritePolicy":"ownerCrewWriters"}});
    let original = a.tool_ok("managed_table_v2_submit", create.clone()).await;
    let table = original["effect"]["tableId"].as_str().unwrap().to_owned();
    let mut explicit = create.clone();
    explicit["contractVersion"] = 2.into();
    assert_eq!(
        a.tool_ok("managed_table_v2_submit", explicit).await,
        original
    );
    let row_request = json!({"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"addRow","tableId":table,"expectedSchemaRevision":"1","patches":[{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Old row"}}}]}});
    let row = a.tool_ok("managed_table_v2_submit", row_request).await["effect"]["rowId"]
        .as_str()
        .unwrap()
        .to_owned();
    for (name, args) in [
        ("managed_table_v2_get", json!({"id":table})),
        ("managed_tables_v2_list", json!({"after":null})),
        ("managed_row_v2_get", json!({"id":row})),
        (
            "managed_rows_v2_list",
            json!({"tableId":table,"after":null}),
        ),
        (
            "managed_table_v2_request_get",
            json!({"scope":scope,"crewId":f.crew,"requestId":original_id}),
        ),
    ] {
        let normal = a.tool_ok(name, args.clone()).await;
        let mut explicit = args;
        explicit["contractVersion"] = 2.into();
        assert_eq!(a.tool_ok(name, explicit).await, normal);
    }
    let before = snapshot(&f).await;
    for tool in SIX {
        for version in [Value::Null, json!("3"), json!(3.0), json!(true), json!(4)] {
            let r = a.tool(tool, json!({"contractVersion":version})).await;
            assert_eq!(r["isError"], true, "{tool}: {r}");
        }
    }
    assert_eq!(snapshot(&f).await, before);
    let current = a
        .tool_ok(
            "managed_table_v2_get",
            json!({"contractVersion":3,"action":"table","id":table}),
        )
        .await;
    assert_eq!(current["id"], table);
    let title = "Fresh explicit V3 table";
    let purpose = "Independent original request namespace proof";
    let proof = checked(&mut a, &scope, &f.crew, title, Some(purpose), None).await;
    let conflict = new_table(&scope, &f.crew, &original_id, title, purpose, &proof, false);
    let before = snapshot(&f).await;
    assert_eq!(
        a.tool("managed_table_v2_submit", conflict.clone()).await["isError"],
        true
    );
    assert_eq!(
        a.tool(
            "managed_table_v2_request_get",
            json!({"contractVersion":3,"scope":scope,"crewId":f.crew,"requestId":original_id})
        )
        .await["isError"],
        true
    );
    assert_eq!(snapshot(&f).await, before);
    let mut fresh = conflict;
    fresh["requestId"] = uuid::Uuid::new_v4().to_string().into();
    let committed = a.tool_ok("managed_table_v2_submit", fresh.clone()).await;
    assert_ne!(committed["effect"]["tableId"], table);
    let known=a.tool_ok("managed_table_v2_request_get",json!({"contractVersion":3,"scope":scope,"crewId":f.crew,"requestId":fresh["requestId"]})).await;
    assert_eq!(known["status"], "known");
    assert_eq!(known["receipt"], committed);
    let row3 = a
        .tool_ok(
            "managed_row_v2_get",
            json!({"contractVersion":3,"action":"row","id":row}),
        )
        .await;
    assert_eq!(row3["id"], row);
    assert_eq!(row3["linkFieldIds"], json!([]));
    let rows3 = a
        .tool_ok(
            "managed_rows_v2_list",
            json!({"contractVersion":3,"tableId":table,"after":null}),
        )
        .await;
    assert_eq!(rows3["items"][0]["id"], row);
    let tables3 = a
        .tool_ok(
            "managed_tables_v2_list",
            json!({"contractVersion":3,"after":null}),
        )
        .await;
    assert!(
        tables3["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == table)
    );
    a.tool_ok("managed_table_v2_submit",json!({"contractVersion":3,"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"alterTable","tableId":table,"expectedSchemaRevision":"1","reason":"Evolve the live schema after the old immutable receipt","change":{"kind":"addField","field":{"name":"New boolean","kind":{"kind":"boolean"},"nullable":true}}}})).await;
    assert_eq!(
        a.tool("managed_table_v2_get", json!({"id":table})).await["isError"],
        true
    );
    let before_replay = snapshot(&f).await;
    assert_eq!(a.tool_ok("managed_table_v2_submit", create).await, original);
    assert_eq!(snapshot(&f).await, before_replay);
    let original_lookup = a
        .tool_ok(
            "managed_table_v2_request_get",
            json!({"scope":scope,"crewId":f.crew,"requestId":original_id}),
        )
        .await;
    assert_eq!(original_lookup["receipt"], original);
    for method in [
        "managedTable.v3.get",
        "managedTable.v3.list",
        "managedTable.v3.row.get",
        "managedTable.v3.rows",
        "managedTable.v3.row.links",
        "managedTable.v3.submit",
        "managedTable.v3.request.get",
        "managedTable.v3.defineCheck",
        "managedTable.v3.history",
        "managedTable.v3.history.value",
    ] {
        let r = a.frame(method, json!([])).await;
        assert_eq!(r["error"]["code"], -32003, "{method}: {r}");
    }
    drop(a);
    drop(human);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, server).await.unwrap().unwrap();
}
#[tokio::test]
async fn v2_directory_filters_unrepresentable_tables_before_limit_and_legacy_none_purpose_renames_explicitly()
 {
    let f = Fixture::new().await;
    f.enable_public_command_identity().await;
    let (stop, closing) = tokio::sync::oneshot::channel();
    let server = f.spawn_owned_socket(closing).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = Peer::connect(&socket).await;
    let scope = human.ok("managedObject.scope", json!({})).await;
    let mut a = Peer::connect(&socket).await;
    a.ok(
        "agent.attach",
        json!({"sessionId":f.captain,"token":f.token(&f.captain)}),
    )
    .await;
    let legacy=human.ok("managedObject.submit",json!({"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"createTable","title":"Legacy no purpose","fields":[{"name":"Title","fieldType":"text","nullable":false}]}})).await;
    let old = legacy["object"]["table"]["id"].as_str().unwrap().to_owned();
    assert!(
        a.tool_ok(
            "managed_table_v2_get",
            json!({"contractVersion":3,"action":"table","id":old})
        )
        .await["purpose"]
            .is_null()
    );
    let rename_title = "Renamed legacy no purpose";
    let proof = checked(&mut a, &scope, &f.crew, rename_title, None, Some(&old)).await;
    // Same authoritative current title removes a second possible refusal cause:
    // this certificate has None purpose and belongs to Rename, not SetPurpose.
    let none_proof = checked(
        &mut a,
        &scope,
        &f.crew,
        "Legacy no purpose",
        None,
        Some(&old),
    )
    .await;
    let wrong = json!({"contractVersion":3,"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"alterTable","tableId":old,"expectedSchemaRevision":"1","reason":"Test wrong consumer refusal","change":{"kind":"setPurpose","purpose":"New actual purpose","definitionCheckToken":none_proof["definitionCheckToken"],"confirmNew":true}}});
    let before = snapshot(&f).await;
    assert_eq!(
        a.tool("managed_table_v2_submit", wrong).await["isError"],
        true
    );
    assert_eq!(snapshot(&f).await, before);
    a.tool_ok("managed_table_v2_submit",json!({"contractVersion":3,"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"alterTable","tableId":old,"expectedSchemaRevision":"1","reason":"Preserve the original missing purpose","change":{"kind":"renameTable","title":rename_title,"definitionCheckToken":proof["definitionCheckToken"],"confirmNew":true}}})).await;
    let renamed = a
        .tool_ok(
            "managed_table_v2_get",
            json!({"contractVersion":3,"action":"table","id":old}),
        )
        .await;
    assert_eq!(renamed["title"], rename_title);
    assert!(renamed["purpose"].is_null());
    for index in 0..20 {
        let title = format!("V3-only boolean table {index}");
        let purpose = format!("Unsupported field definition {index}");
        let proof = checked(&mut a, &scope, &f.crew, &title, Some(&purpose), None).await;
        let r = a
            .tool_ok(
                "managed_table_v2_submit",
                new_table(
                    &scope,
                    &f.crew,
                    &uuid::Uuid::new_v4().to_string(),
                    &title,
                    &purpose,
                    &proof,
                    true,
                ),
            )
            .await;
        let id = r["effect"]["tableId"].as_str().unwrap();
        let denied = human.frame("managedTable.v2.get", json!({"id":id})).await;
        assert_eq!(denied["error"]["code"], -32009);
        assert_eq!(
            denied["error"]["data"]["refusal"],
            "managedTable.codec_unsupported"
        );
    }
    let title = "Compatible text tail";
    let purpose = "This table must remain visible after unsupported predecessors";
    let proof = checked(&mut a, &scope, &f.crew, title, Some(purpose), None).await;
    let tail = a
        .tool_ok(
            "managed_table_v2_submit",
            new_table(
                &scope,
                &f.crew,
                &uuid::Uuid::new_v4().to_string(),
                title,
                purpose,
                &proof,
                false,
            ),
        )
        .await["effect"]["tableId"]
        .as_str()
        .unwrap()
        .to_owned();
    let page = human.ok("managedTable.v2.list", json!({"after":old})).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["id"], tail);
    assert!(page["nextAfter"].is_null());
    let old_again = a.tool_ok("managed_table_v2_get", json!({"id":old})).await;
    assert_eq!(old_again["title"], rename_title);
    drop(a);
    drop(human);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, server).await.unwrap().unwrap();
}

#[tokio::test]
async fn history_rechecks_target_membership_and_original_token_at_the_actual_fifo_boundary() {
    use crewd_server::agent::testkit::CallerStage;
    let f = Fixture::new().await;
    f.enable_public_command_identity().await;
    let third = f.add_crew("Independent history target", None).await;
    let (stop, closing) = tokio::sync::oneshot::channel();
    let server = f.spawn_owned_socket(closing).await;
    let socket = f.data_dir().join("crewd.sock");
    let mut human = Peer::connect(&socket).await;
    let scope = human.ok("managedObject.scope", json!({})).await;
    let channel=human.ok("channel.create",json!({"crewId":third,"kind":"topic","name":"Actual history membership","members":[{"kind":"session","id":f.worker}]})).await["id"].as_str().unwrap().to_owned();
    let body = "PRIVATE_HISTORY_TARGET_BODY";
    let message = human
        .ok("message.post", json!({"channelId":channel,"text":body}))
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut a = attached(&f, &socket).await;
    let title = "History target source";
    let purpose = "Exercise current source and historical target grants";
    let proof = checked(&mut a, &scope, &f.crew, title, Some(purpose), None).await;
    let mut create = new_table(
        &scope,
        &f.crew,
        &uuid::Uuid::new_v4().to_string(),
        title,
        purpose,
        &proof,
        false,
    );
    create["operation"]["fields"].as_array_mut().unwrap().push(json!({"name":"Evidence","kind":{"kind":"link","target":{"kind":"message"},"many":true},"nullable":true}));
    let table = a.tool_ok("managed_table_v2_submit", create).await["effect"]["tableId"]
        .as_str()
        .unwrap()
        .to_owned();
    let added=a.tool_ok("managed_table_v2_submit",json!({"contractVersion":3,"scope":scope,"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"operation":{"kind":"addRow","tableId":table,"expectedSchemaRevision":"1","patches":[{"fieldId":"p1","change":{"kind":"setScalar","value":{"type":"text","value":"Readable original row"}}},{"fieldId":"p2","change":{"kind":"replaceLinks","values":[{"kind":"reference","target":{"kind":"message","id":message}}],"keepHiddenSetToken":null}}]}})).await;
    let row = added["effect"]["rowId"].as_str().unwrap().to_owned();
    let args = json!({"contractVersion":3,"action":"historyValue","source":{"kind":"row","id":row},"key":{"kind":"event","eventId":added["eventId"]},"path":{"kind":"links","fieldId":"p2","side":"after"},"cursor":null});
    let initial = a.tool_ok("managed_table_v2_get", args.clone()).await;
    assert_eq!(initial["status"], "links");
    assert!(
        initial["visible"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["target"]["id"] == message)
    );
    assert_eq!(initial["restrictedDetails"], false);
    let mut gate = f.caller_gate(&f.worker, CallerStage::ManagedReadCaptured);
    let captured = args.clone();
    let pending = tokio::spawn(async move {
        let value = a.tool("managed_table_v2_get", captured).await;
        (a, value)
    });
    tokio::time::timeout(LIMIT, gate.entered()).await.unwrap();
    human
        .ok(
            "channel.removeMember",
            json!({"channelId":channel,"member":{"kind":"session","id":f.worker}}),
        )
        .await;
    let after_revoke = snapshot(&f).await;
    gate.release();
    let (mut a, result) = tokio::time::timeout(LIMIT, pending).await.unwrap().unwrap();
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(
        !encoded.contains(&message) && !encoded.contains(body) && !encoded.contains("hiddenCount")
    );
    if result["isError"] == false {
        let value: Value = serde_json::from_str(result["text"].as_str().unwrap()).unwrap();
        assert!(
            value["status"] == "restricted"
                || value["status"] == "links"
                    && value["restrictedDetails"] == true
                    && value["visible"] == json!([])
        );
    }
    assert_eq!(snapshot(&f).await, after_revoke);
    let mut gate = f.caller_gate(&f.worker, CallerStage::ManagedReadCaptured);
    let pending = tokio::spawn(async move {
        let value = a.tool("managed_table_v2_get", args).await;
        (a, value)
    });
    tokio::time::timeout(LIMIT, gate.entered()).await.unwrap();
    f.state.tokens.revoke(&f.worker);
    let after_token = snapshot(&f).await;
    gate.release();
    let (_a, denied) = tokio::time::timeout(LIMIT, pending).await.unwrap().unwrap();
    assert_eq!(denied["isError"], true);
    let encoded = serde_json::to_string(&denied).unwrap();
    assert!(!encoded.contains(&message) && !encoded.contains(body));
    assert_eq!(snapshot(&f).await, after_token);
    drop(human);
    stop.send(()).unwrap();
    tokio::time::timeout(LIMIT, server).await.unwrap().unwrap();
}
