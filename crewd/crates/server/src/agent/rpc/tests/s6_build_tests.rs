//! Proposed S6 actual AgentConn comparison caller; not literal serve or kernel authority QA.
use crate::agent::rpc::{AgentConn, handle};
use crate::agent::testkit::{Fixture, RelayStandIn};
use crate::orchestra::{
    self, Actor, Handle, OrchestraConfig,
    clock::ManualClock,
    db::{DbPort, pump_collab},
    exec::Executors,
    hooks::Hooks,
};
use crewd_proto::{Request, Response, RpcError, ServerFrame};
use crewd_wake::Ms;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{
        UnixListener, UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};

const LIMIT: Duration = Duration::from_secs(8);
// Extra qualification proofs use existing official compile-time overrides; default tests
// remain meaningful under the ordinary artifact's actual metadata.
const SYNTHETIC_SHA: &str = "6a6000000001";
type Checked<T> = Result<T, String>;

#[derive(Debug, PartialEq)]
struct Surface {
    rows: Vec<(String, Vec<String>)>,
    tokens: usize,
    hub: usize,
    launches: usize,
    sends: usize,
}
#[derive(Debug)]
struct SocketReceipt {
    attached: bool,
    watcher_started: bool,
    watcher_cancelled_join: bool,
    responses: usize,
}

struct Rig {
    fixture: Option<Fixture>,
    h: Handle,
    actor: Option<Actor>,
    bridge: Option<JoinHandle<()>>,
    socket: Option<JoinHandle<Checked<SocketReceipt>>>,
    reader: Option<BufReader<OwnedReadHalf>>,
    writer: Option<OwnedWriteHalf>,
    exec: RelayStandIn,
    facts: Arc<AtomicUsize>,
    token: String,
}
impl Drop for Rig {
    fn drop(&mut self) {
        // Emergency fallback is not normal lifecycle evidence.
        if let Some(actor) = &self.actor {
            actor.abort_handle().abort();
        }
        if let Some(task) = &self.bridge {
            task.abort();
        }
        if let Some(task) = &self.socket {
            task.abort();
        }
    }
}
impl Rig {
    async fn new() -> Self {
        let cell = Arc::new(OnceLock::new());
        let fixture = Fixture::with_orchestra(cell.clone()).await;
        let rx = fixture.state.hub.subscribe_collab();
        let exec = RelayStandIn::default();
        let (h, actor) = orchestra::spawn(
            Arc::new(DbPort::new(
                fixture.state.clone(),
                fixture.data_dir().join("crewd.db"),
            )),
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(Ms(crewd_store::now_ms()))),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        assert!(cell.set(h.clone()).is_ok());
        assert!(fixture.state.orchestra.set(h.clone()).is_ok());
        let facts = Arc::new(AtomicUsize::new(0));
        let count = facts.clone();
        let sink = h.clone();
        let bridge = tokio::spawn(pump_collab(rx, move |fact| {
            count.fetch_add(1, Ordering::SeqCst);
            sink.collab(fact.clone());
        }));
        let token = fixture.token(&fixture.worker);
        let path = fixture.data_dir().join("s6-actual-agent.sock");
        let listener = UnixListener::bind(path).unwrap();
        let state = fixture.state.clone();
        // This owned transport delegates every request to actual production handle. It does
        // not synthesize BuildCheck, relax strict parsing, or implement peer authority.
        let socket = tokio::spawn(async move {
            let (stream, _) = timeout(LIMIT, listener.accept())
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            let (read, mut write) = stream.into_split();
            let (output, mut frames) = mpsc::channel::<ServerFrame>(32);
            let mut writer_task = tokio::spawn(async move {
                while let Some(frame) = frames.recv().await {
                    let mut bytes = serde_json::to_vec(&frame).map_err(|e| e.to_string())?;
                    bytes.push(b'\n');
                    write.write_all(&bytes).await.map_err(|e| e.to_string())?;
                }
                write.shutdown().await.map_err(|e| e.to_string())
            });
            let mut reader = BufReader::new(read);
            let mut conn = AgentConn::default();
            let mut responses = 0;
            let observation: Checked<()> = async {
                loop {
                    let mut line = String::new();
                    let n = timeout(LIMIT, reader.read_line(&mut line))
                        .await
                        .map_err(|e| e.to_string())?
                        .map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    let request: Request =
                        serde_json::from_str(&line).map_err(|e| e.to_string())?;
                    let response = handle(&state, &mut conn, request, &output).await;
                    output
                        .send(ServerFrame::Response(response))
                        .await
                        .map_err(|e| e.to_string())?;
                    responses += 1;
                }
                Ok(())
            }
            .await;
            let attached = conn.session.as_deref() == Some("s-w1");
            let watcher_started = conn.watcher.is_some();
            let mut cleanup = Vec::new();
            let mut watcher_cancelled_join = false;
            if let Some(watcher) = conn.watcher.take() {
                // Actual production Drop semantics: explicit cancel followed by observed join.
                watcher.abort();
                match timeout(LIMIT, watcher).await {
                    Ok(Err(e)) if e.is_cancelled() => watcher_cancelled_join = true,
                    Ok(Ok(())) => {}
                    other => cleanup.push(format!("watcher cleanup: {other:?}")),
                }
            }
            drop(conn);
            drop(output);
            match timeout(LIMIT, &mut writer_task).await {
                Ok(Ok(Ok(()))) => {}
                Err(e) => {
                    cleanup.push(format!("socket writer deadline: {e}"));
                    writer_task.abort();
                    let _ = timeout(LIMIT, &mut writer_task).await;
                }
                other => cleanup.push(format!("socket writer cleanup: {other:?}")),
            }
            drop(state);
            drop(listener);
            if !cleanup.is_empty() {
                return Err(format!("observation={observation:?}; cleanup={cleanup:?}"));
            }
            observation?;
            Ok(SocketReceipt {
                attached,
                watcher_started,
                watcher_cancelled_join,
                responses,
            })
        });
        let stream = timeout(
            LIMIT,
            UnixStream::connect(fixture.data_dir().join("s6-actual-agent.sock")),
        )
        .await
        .unwrap()
        .unwrap();
        let (read, write) = stream.into_split();
        Self {
            fixture: Some(fixture),
            h,
            actor: Some(actor),
            bridge: Some(bridge),
            socket: Some(socket),
            reader: Some(BufReader::new(read)),
            writer: Some(write),
            exec,
            facts,
            token,
        }
    }
    fn fixture(&self) -> &Fixture {
        self.fixture.as_ref().unwrap()
    }
    async fn surface(&self) -> Checked<Surface> {
        timeout(LIMIT, self.h.latches())
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?; // FIFO actor barrier, no invented fact.
        let rows = self.fixture().state.db.call(|store| -> Result<_, rusqlite::Error> {
            let conn = store.conn();
            let mut table_stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
            let tables = table_stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            let mut snapshot = Vec::new();
            for name in tables {
                let quoted = name.replace('"', "\"\"");
                let mut stmt = conn.prepare(&format!("SELECT * FROM \"{quoted}\""))?;
                let columns = stmt.column_count();
                let mut result = stmt.query([])?;
                let mut rows = Vec::new();
                while let Some(row) = result.next()? {
                    let mut values = Vec::new();
                    for i in 0..columns { values.push(format!("{:?}", row.get_ref(i)?)); }
                    rows.push(values.join("|"));
                }
                rows.sort(); snapshot.push((name, rows));
            }
            Ok(snapshot)
        }).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        Ok(Surface {
            rows,
            tokens: self.fixture().state.tokens.test_len(),
            hub: self.facts.load(Ordering::SeqCst),
            launches: self.exec.launches("s-cap").len() + self.exec.launches("s-w1").len(),
            sends: self.exec.sent("s-cap").len() + self.exec.sent("s-w1").len(),
        })
    }
    async fn request(&mut self, params: Value) -> Checked<Response> {
        let req = Request {
            id: 601,
            method: "agent.attach".into(),
            params,
        };
        let mut bytes = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        timeout(LIMIT, self.writer.as_mut().unwrap().write_all(&bytes))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        timeout(LIMIT, async {
            loop {
                let mut line = String::new();
                if self
                    .reader
                    .as_mut()
                    .unwrap()
                    .read_line(&mut line)
                    .await
                    .map_err(|e| e.to_string())?
                    == 0
                {
                    return Err("socket EOF before response".into());
                }
                match serde_json::from_str::<ServerFrame>(&line).map_err(|e| e.to_string())? {
                    ServerFrame::Response(r) if r.id == 601 => return Ok(r),
                    _ => {}
                }
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }
    async fn close(mut self) -> Checked<SocketReceipt> {
        let mut errors = Vec::new();
        if let Some(mut writer) = self.writer.take()
            && let Err(e) = writer.shutdown().await
        {
            errors.push(e.to_string());
        }
        let socket = match timeout(LIMIT, self.socket.as_mut().unwrap()).await {
            Ok(Ok(Ok(receipt))) => Some(receipt),
            other => {
                errors.push(format!("owned socket cleanup: {other:?}"));
                None
            }
        };
        if socket.is_some() {
            self.socket.take();
        } else {
            // Timed-out/error transport cannot be counted normal. End its owned task before
            // trying remaining independent Core/Hub/database cleanup.
            if let Some(mut task) = self.socket.take() {
                task.abort();
                let _ = timeout(LIMIT, &mut task).await;
            }
        }
        self.reader.take();
        let shutdown = timeout(LIMIT, self.h.shutdown(Duration::from_secs(2))).await;
        let shutdown_clean = matches!(&shutdown, Ok(true));
        if !shutdown_clean {
            errors.push(format!("actor shutdown: {shutdown:?}"));
        }
        let actor = self.actor.take().unwrap();
        let abort = actor.abort_handle();
        // Keep the same future (and therefore its actual JoinHandle) across timeout.
        let mut finished = Box::pin(actor.finished());
        if !shutdown_clean {
            abort.abort();
        }
        match timeout(LIMIT, &mut finished).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                // A returned ActorDied is already an observed completed join. Abort is
                // idempotent; polling the completed finished future again would be invalid.
                abort.abort();
                errors.push(format!("actor join: {error:?}"));
            }
            Err(error) => {
                errors.push(format!("actor join deadline: {error}"));
                abort.abort();
                let joined = timeout(LIMIT, &mut finished).await;
                // Actor::finished maps cancellation to Ok; fallback must still be nonnormal.
                errors.push(format!("actor abort fallback observed join: {joined:?}"));
            }
        }
        let fixture = self.fixture.take().unwrap();
        let db = fixture.state.db.clone();
        drop(fixture.state);
        match timeout(LIMIT, self.bridge.as_mut().unwrap()).await {
            Ok(Ok(())) => {
                self.bridge.take();
            }
            other => errors.push(format!("Hub bridge join: {other:?}")),
        }
        match timeout(LIMIT, tokio::task::spawn_blocking(move || db.shutdown())).await {
            Ok(Ok(())) => {}
            other => errors.push(format!("DB shutdown: {other:?}")),
        }
        // Fixture's remaining TempDir stays alive through the database join above.
        if errors.is_empty() {
            socket.ok_or_else(|| "missing socket receipt".into())
        } else {
            Err(errors.join("; "))
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Case {
    Different,
    Same,
    Legacy,
    Unknown,
    Dirty,
    InvalidToken,
    UnknownField,
    Qualification,
}
async fn observe(case: Case) {
    let compiled = crate::build_info::build_info();
    let daemon_known_clean =
        !compiled.dirty && !compiled.git_sha.trim().is_empty() && compiled.git_sha != "unknown";
    // Expected state comes from actual immutable compile metadata, never a runtime override.
    // No branch skips a request, cleanup, or assertion under the normal workspace build.
    let expected = match case {
        Case::Different if daemon_known_clean => "skew",
        Case::Same | Case::Qualification if daemon_known_clean => "same",
        _ => "unknown",
    };
    let mut rig = Rig::new().await;
    let observation: Checked<(Response, Surface, Surface, Value)> = async {
        let before = rig.surface().await?;
        let me = crate::agent::port::identity(&rig.fixture().state, &rig.fixture().worker)
            .await.map_err(|e| e.to_string())?.ok_or_else(|| "fixture worker missing".to_string())?;
        let original = json!({"role":me.role, "crewId":me.crew_id, "label":me.label,
            "instructions":crate::agent::text::get(crate::agent::text::Locale::default(), "instructions"),
            "surfaceHash":crate::agent::surface::hash(crate::agent::text::Locale::default())});
        let mut params = json!({"sessionId":rig.fixture().worker, "token":rig.token});
        let mut helper = serde_json::to_value(&compiled).map_err(|e| e.to_string())?;
        // Path and inode equality are deliberately not part of this build comparison.
        helper["exe"] = json!("/isolated/copied-helper"); helper["exeStillOnDisk"] = json!(false);
        match case {
            Case::Different => {
                helper["gitSha"] = json!(if compiled.git_sha == "6a6000000002" { SYNTHETIC_SHA } else { "6a6000000002" });
                helper["dirty"] = json!(false);
            },
            Case::Unknown => { helper["gitSha"] = json!("unknown"); helper["dirty"] = json!(false); },
            Case::Dirty => {
                helper["gitSha"] = json!(if compiled.git_sha.is_empty() || compiled.git_sha == "unknown" { SYNTHETIC_SHA } else { compiled.git_sha.as_str() });
                helper["dirty"] = json!(true);
            },
            Case::Qualification => {
                helper["gitSha"] = json!(if compiled.git_sha.is_empty() || compiled.git_sha == "unknown" { SYNTHETIC_SHA } else { compiled.git_sha.as_str() });
                helper["dirty"] = json!(false);
            },
            _ => {},
        }
        if !matches!(case, Case::Legacy) { params["helperBuild"] = helper; }
        if matches!(case, Case::InvalidToken) { params["token"] = json!("invalid-s6-token"); }
        if matches!(case, Case::UnknownField) {
            // Independent of the future accepted helperBuild key: strict parsing remains strict.
            params.as_object_mut().unwrap().remove("helperBuild"); params["callerAuthority"] = json!("human");
        }
        let response = rig.request(params).await?;
        let after = rig.surface().await?;
        Ok((response, before, after, original))
    }.await;
    let cleanup = rig.close().await;
    match &observation {
        Ok((response, before, after, _)) => eprintln!(
            "S6 actual AgentConn case={case:?} daemon_known_clean={daemon_known_clean} expected={expected} compiled={compiled:?} response={response:?} effects_equal={} table_count={} tokens={} hub={} launches={} sends={} cleanup={cleanup:?}",
            before == after,
            before.rows.len(),
            before.tokens,
            before.hub,
            before.launches,
            before.sends
        ),
        Err(error) => eprintln!(
            "S6 actual AgentConn case={case:?} observation_error={error} cleanup={cleanup:?}"
        ),
    }
    let receipt = cleanup.expect("normal socket/watcher/Core/Hub/DB cleanup must complete");
    let (response, before, after, original) =
        observation.expect("actual wire observation, after cleanup");
    assert_eq!(receipt.responses, 1, "no attach retry/downgrade");
    assert_eq!(
        before, after,
        "S6 attach comparison has no DB/meta/token/Hub/executor effects"
    );
    let result = response.into_result();
    if matches!(case, Case::InvalidToken | Case::UnknownField) {
        let expected = if matches!(case, Case::InvalidToken) {
            RpcError::FORBIDDEN
        } else {
            RpcError::INVALID_PARAMS
        };
        assert_eq!(result.unwrap_err().code, expected);
        assert!(!receipt.attached && !receipt.watcher_started && !receipt.watcher_cancelled_join);
    } else {
        let result = result.expect("S6 actual daemon attach succeeds without build-skew refusal");
        assert!(receipt.attached && receipt.watcher_started && receipt.watcher_cancelled_join);
        for field in ["role", "crewId", "label", "instructions", "surfaceHash"] {
            assert_eq!(
                result[field], original[field],
                "existing attach field {field} remains exact"
            );
        }
        assert_eq!(
            result["buildCheck"]["status"], expected,
            "S6 actual daemon comparison result"
        );
        if expected == "skew" {
            assert!(
                !result["buildCheck"]["warning"]
                    .as_str()
                    .unwrap_or("")
                    .is_empty()
            );
        } else {
            assert!(
                result["buildCheck"]
                    .get("warning")
                    .is_none_or(|w| w.is_null() || w == "")
            );
        }
    }
}

#[tokio::test]
async fn s6_actual_daemon_different_known_helper_attaches_with_conservative_status() {
    observe(Case::Different).await;
}
#[tokio::test]
async fn s6_actual_daemon_equal_declared_build_attaches_with_conservative_status() {
    observe(Case::Same).await;
}
#[tokio::test]
async fn s6_actual_daemon_legacy_absence_is_unknown_and_usable() {
    observe(Case::Legacy).await;
}
#[tokio::test]
async fn s6_actual_daemon_unknown_build_is_not_claimed_same() {
    observe(Case::Unknown).await;
}
#[tokio::test]
async fn s6_actual_daemon_dirty_build_is_not_claimed_same() {
    observe(Case::Dirty).await;
}
#[tokio::test]
async fn s6_actual_daemon_invalid_token_never_binds_or_watches() {
    observe(Case::InvalidToken).await;
}
#[tokio::test]
async fn s6_actual_daemon_unrelated_unknown_field_is_still_rejected() {
    observe(Case::UnknownField).await;
}

// This always executes. Extra official clean/unknown/dirty artifacts prove its distinct branches.
#[tokio::test]
async fn s6_actual_daemon_actual_qualification_controls_clean_helper_comparison() {
    observe(Case::Qualification).await;
}
