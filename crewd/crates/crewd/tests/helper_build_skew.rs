//! S6 caller-only test-first candidate: actual crewd helpers, owned recording daemon.
//! Does not exercise daemon comparison or authorize any Human RPC. No provider process.
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, ensure};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};

const SESSION: &str = "s6-owned-session";
const TOKEN: &str = "s6-synthetic-token";
const INSTRUCTIONS: &str = "Synthetic S6 tools";
const SURFACE: &str = "s6-synthetic-surface";
const WARNING: &str = "S6 SYNTHETIC BUILD SKEW: choose an update time; no automatic restart";
const HOOK_STDOUT: &str = "{\"hookSpecificOutput\":{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"S6 synthetic context\"}}";
const BOUND: Duration = Duration::from_secs(12);
const OUTPUT_CAP: u64 = 256 * 1024;

#[derive(Clone, Copy)]
enum ReplyMode {
    Legacy,
    Skew,
    Same,
    Unknown,
    Future,
    OldStrict,
}

#[derive(Clone, Default, Debug)]
struct Trace {
    connections: usize,
    requests: Vec<Value>,
    errors: Vec<String>,
}

struct RecordingDaemon {
    dir: tempfile::TempDir,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    trace: Arc<Mutex<Trace>>,
}

impl Drop for RecordingDaemon {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(task) = self.task.take() {
            task.abort(); // Setup/cleanup failure fallback only; never normal-join proof.
        }
    }
}

impl RecordingDaemon {
    async fn start(mode: ReplyMode) -> Result<Self> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir_all(dir.path().join("home"))?;
        std::fs::write(dir.path().join("token"), format!("{TOKEN}\n"))?;
        let listener = UnixListener::bind(dir.path().join("crewd.sock"))?;
        let trace = Arc::new(Mutex::new(Trace::default()));
        let observed = trace.clone();
        let (stop, mut stop_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    result = stop_rx.changed() => {
                        if result.is_err() || *stop_rx.borrow() { break; }
                    }
                    result = listener.accept() => match result {
                        Ok((stream, _)) => {
                            observed.lock().unwrap().connections += 1;
                            let trace = observed.clone();
                            let stop = stop_rx.clone();
                            connections.spawn(async move {
                                if let Err(error) = serve_connection(stream, mode, stop, trace.clone()).await {
                                    trace.lock().unwrap().errors.push(error.to_string());
                                }
                            });
                        }
                        Err(error) => {
                            observed.lock().unwrap().errors.push(error.to_string());
                            break;
                        }
                    },
                    Some(result) = connections.join_next(), if !connections.is_empty() => {
                        if let Err(error) = result { observed.lock().unwrap().errors.push(error.to_string()); }
                    }
                }
            }
            // Stop signal releases still-open read loops; every accepted task is joined.
            drop(listener);
            while let Some(result) = connections.join_next().await {
                if let Err(error) = result {
                    observed.lock().unwrap().errors.push(error.to_string());
                }
            }
        });
        Ok(Self {
            dir,
            stop,
            task: Some(task),
            trace,
        })
    }

    fn command(&self, kind: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_crewd"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", self.dir.path().join("home"))
            .env("XDG_CACHE_HOME", self.dir.path().join("home/cache"))
            .env(
                "CLAUDE_CONFIG_DIR",
                self.dir.path().join("home/claude-config"),
            )
            .env("SHELL", "/usr/bin/false")
            .arg("--data-dir")
            .arg(self.dir.path())
            .arg(kind);
        if kind == "hook" {
            command.arg("post-tool-use");
        }
        command
            .arg("--session")
            .arg(SESSION)
            .arg("--token-file")
            .arg(self.dir.path().join("token"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    async fn close(mut self) -> Result<Trace> {
        let _ = self.stop.send(true);
        let joined =
            tokio::time::timeout(BOUND, self.task.as_mut().context("missing listener task")?).await;
        match joined {
            Ok(result) => {
                result.context("recording listener task panicked")?;
                self.task.take();
            }
            Err(_) => return Err(anyhow!("recording listener did not normally join")),
        }
        let trace = self.trace.lock().unwrap().clone();
        ensure!(
            trace.errors.is_empty(),
            "recording listener errors: {:?}",
            trace.errors
        );
        Ok(trace)
    }
}

async fn serve_connection(
    stream: UnixStream,
    mode: ReplyMode,
    mut stop: watch::Receiver<bool>,
    trace: Arc<Mutex<Trace>>,
) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    loop {
        let next = tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() { return Ok(()); }
                continue;
            }
            line = lines.next_line() => line?,
        };
        let Some(line) = next else {
            return Ok(());
        };
        ensure!(
            line.len() <= OUTPUT_CAP as usize,
            "oversized helper request"
        );
        let request: Value = serde_json::from_str(&line)?;
        trace.lock().unwrap().requests.push(request.clone());
        let id = &request["id"];
        let method = request["method"].as_str().context("request lacks method")?;
        let params = &request["params"];
        let reply = match method {
            "agent.attach" if params["sessionId"] != SESSION || params["token"] != TOKEN => {
                json!({"id": id, "error": {"code": -32003, "message": "invalid session or token"}})
            }
            "agent.attach"
                if matches!(mode, ReplyMode::OldStrict)
                    && params.as_object().is_some_and(|p| {
                        p.keys().any(|key| key != "sessionId" && key != "token")
                    }) =>
            {
                json!({"id": id, "error": {"code": -32602, "message": "unknown field helperBuild"}})
            }
            "agent.attach" => {
                let mut result = json!({"role": "worker", "crewId": "s6-crew", "label": "S6 worker", "instructions": INSTRUCTIONS, "surfaceHash": SURFACE});
                match mode {
                    ReplyMode::Skew => {
                        result["buildCheck"] = json!({"status": "skew", "warning": WARNING})
                    }
                    ReplyMode::Same => result["buildCheck"] = json!({"status": "same"}),
                    ReplyMode::Unknown => result["buildCheck"] = json!({"status": "unknown"}),
                    ReplyMode::Future => {
                        result["buildCheck"] =
                            json!({"status": "future-s6-status", "warning": WARNING})
                    }
                    ReplyMode::Legacy | ReplyMode::OldStrict => {}
                }
                json!({"id": id, "result": result})
            }
            "agent.tools" => json!({"id": id, "result": [{
                "name": "post", "description": "Synthetic post", "inputSchema": {"type": "object"},
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false}
            }]}),
            "agent.call" => {
                json!({"id": id, "result": {"text": "S6 synthetic accepted call", "isError": false}})
            }
            "agent.hook" => json!({"id": id, "result": {"stdout": HOOK_STDOUT}}),
            _ => {
                json!({"id": id, "error": {"code": -32601, "message": "unknown synthetic method"}})
            }
        };
        let mut bytes = serde_json::to_vec(&reply)?;
        bytes.push(b'\n');
        tokio::time::timeout(BOUND, write.write_all(&bytes))
            .await
            .context("listener write deadline")??;
    }
}

struct Helper {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<JoinHandle<Result<Vec<u8>>>>,
    stderr: Option<JoinHandle<Result<Vec<u8>>>>,
    frames: mpsc::UnboundedReceiver<Value>,
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        if let Some(task) = self.stdout.take() {
            task.abort();
        }
        if let Some(task) = self.stderr.take() {
            task.abort();
        }
    }
}

#[derive(Debug)]
struct ExitReceipt {
    status: ExitStatus,
    forced_kill: bool,
    stdout: String,
    stderr: String,
}

fn collect_pipe(pipe: impl AsyncRead + Unpin + Send + 'static) -> JoinHandle<Result<Vec<u8>>> {
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        pipe.take(OUTPUT_CAP + 1).read_to_end(&mut bytes).await?;
        ensure!(
            bytes.len() <= OUTPUT_CAP as usize,
            "helper output exceeded cap"
        );
        Ok(bytes)
    })
}

impl Helper {
    fn start(daemon: &RecordingDaemon, kind: &str) -> Result<Self> {
        let mut child = daemon.command(kind).spawn()?;
        let stdin = child.stdin.take();
        let pipe = child.stdout.take().context("missing child stdout")?;
        let (frames_tx, frames) = mpsc::unbounded_channel();
        let stdout = Some(tokio::spawn(async move {
            let mut lines = BufReader::new(pipe).lines();
            let mut bytes = Vec::new();
            while let Some(line) = lines.next_line().await? {
                ensure!(
                    bytes.len() + line.len() < OUTPUT_CAP as usize,
                    "helper stdout exceeded cap"
                );
                bytes.extend_from_slice(line.as_bytes());
                bytes.push(b'\n');
                if let Ok(frame) = serde_json::from_str::<Value>(&line) {
                    let _ = frames_tx.send(frame);
                }
            }
            Ok(bytes)
        }));
        let stderr = Some(collect_pipe(
            child.stderr.take().context("missing child stderr")?,
        ));
        Ok(Self {
            child,
            stdin,
            stdout,
            stderr,
            frames,
        })
    }

    async fn send(&mut self, input: &[u8]) -> Result<()> {
        tokio::time::timeout(
            BOUND,
            self.stdin
                .as_mut()
                .context("missing helper stdin")?
                .write_all(input),
        )
        .await??;
        Ok(())
    }

    async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<()> {
        let line = format!(
            "{}\n",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        );
        self.send(line.as_bytes()).await?;
        tokio::time::timeout(BOUND, async {
            while let Some(frame) = self.frames.recv().await {
                if frame["id"] == id {
                    ensure!(
                        frame.get("error").is_none(),
                        "MCP protocol rejection: {frame}"
                    );
                    return Ok(());
                }
            }
            Err(anyhow!("MCP closed before response id {id}"))
        })
        .await
        .context("MCP response deadline")?
    }

    async fn observe_protocol(&mut self, kind: &str) -> Result<()> {
        if kind == "hook" {
            return self
                .send(
                    json!({"tool_name":"Read","tool_response":"S6 synthetic tool result"})
                        .to_string()
                        .as_bytes(),
                )
                .await;
        }
        self.request(1, "initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"s6-owned-test","version":"0"}})).await?;
        self.send(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","method":"notifications/initialized"})
            )
            .as_bytes(),
        )
        .await?;
        self.request(2, "tools/list", json!({})).await?;
        self.request(
            3,
            "tools/call",
            json!({"name":"post","arguments":{"message":"S6 synthetic body"}}),
        )
        .await
    }

    async fn close(mut self) -> Result<ExitReceipt> {
        self.stdin.take(); // EOF is the normal MCP shutdown and hook payload completion.
        let (status, forced_kill) = match tokio::time::timeout(BOUND, self.child.wait()).await {
            Ok(result) => (result?, false),
            Err(_) => {
                self.child.kill().await?;
                (self.child.wait().await?, true)
            }
        };
        let stdout =
            tokio::time::timeout(BOUND, self.stdout.as_mut().context("missing stdout owner")?)
                .await???;
        self.stdout.take();
        let stderr =
            tokio::time::timeout(BOUND, self.stderr.as_mut().context("missing stderr owner")?)
                .await???;
        self.stderr.take();
        Ok(ExitReceipt {
            status,
            forced_kill,
            stdout: String::from_utf8(stdout)?,
            stderr: String::from_utf8(stderr)?,
        })
    }
}

struct Observation {
    exit: ExitReceipt,
    trace: Trace,
}

/// Setup/protocol observation errors are retained until both owned parties close.
/// No functional assertion or intentional panic is permitted before this return.
async fn observe(kind: &str, mode: ReplyMode) -> Result<Observation> {
    let daemon = RecordingDaemon::start(mode).await?;
    let mut helper = match Helper::start(&daemon, kind) {
        Ok(helper) => helper,
        Err(error) => {
            let cleanup = daemon.close().await;
            return Err(anyhow!(
                "helper spawn failed: {error:#}; listener cleanup: {cleanup:?}"
            ));
        }
    };
    // Normal MCP closes stdin only after initialize/list/call actual replies, not an EOF race.
    let protocol = helper.observe_protocol(kind).await;
    let child_cleanup = helper.close().await;
    let listener_cleanup = daemon.close().await;
    // A strict old daemon may reject attach before the helper starts its MCP service.
    // Only that test accepts a missing MCP response; its final oracle still requires the exact
    // rejection, no request/reconnect/retry, normal reap, and a nonzero MCP exit (hook stays0).
    if protocol.is_err() && !matches!(mode, ReplyMode::OldStrict) {
        return Err(anyhow!(
            "protocol failed after cleanup: {protocol:?}; child cleanup: {child_cleanup:?}; listener cleanup: {listener_cleanup:?}"
        ));
    }
    let (exit, trace) = match (child_cleanup, listener_cleanup) {
        (Ok(exit), Ok(trace)) => (exit, trace),
        (child, listener) => {
            return Err(anyhow!(
                "protocol: {protocol:?}; child cleanup: {child:?}; listener cleanup: {listener:?}"
            ));
        }
    };
    ensure!(
        !exit.forced_kill,
        "owned helper required forced kill: {exit:?}"
    );
    // Record status rather than enforcing success here: strict-daemon MCP refusal exits nonzero.
    Ok(Observation { exit, trace })
}

fn methods(observation: &Observation, method: &str) -> Vec<Value> {
    observation
        .trace
        .requests
        .iter()
        .filter(|request| request["method"] == method)
        .cloned()
        .collect()
}

fn mcp_reply(observation: &Observation, id: u64) -> Result<Value> {
    for line in observation.exit.stdout.lines() {
        let value: Value = serde_json::from_str(line)?;
        if value["id"] == id {
            return Ok(value);
        }
    }
    Err(anyhow!(
        "no MCP response id {id}; exit={:?}",
        observation.exit
    ))
}

fn require_forwarded(observation: &Observation, kind: &str) {
    assert_eq!(
        observation.exit.status.code(),
        Some(0),
        "normal helper exit"
    );
    assert_eq!(observation.trace.connections, 1, "one connection; no retry");
    assert_eq!(
        methods(observation, "agent.attach").len(),
        1,
        "one attach; no retry"
    );
    if kind == "mcp" {
        assert_eq!(methods(observation, "agent.tools").len(), 1);
        assert_eq!(methods(observation, "agent.call").len(), 1);
        let init = mcp_reply(observation, 1).unwrap();
        assert_eq!(
            init["result"]["serverInfo"]["version"],
            format!("{}+{SURFACE}", env!("CARGO_PKG_VERSION"))
        );
        let list = mcp_reply(observation, 2).unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1, "unchanged catalogue cardinality");
        let tool = &tools[0];
        assert_eq!(tool["name"], "post", "unchanged actual tool name");
        assert_eq!(tool["description"], "Synthetic post");
        assert_eq!(
            tool["inputSchema"],
            json!({"type": "object"}),
            "unchanged actual input schema"
        );
        for (hint, expected) in [
            ("readOnlyHint", false),
            ("destructiveHint", true),
            ("idempotentHint", false),
            ("openWorldHint", false),
        ] {
            assert_eq!(
                tool["annotations"][hint], expected,
                "unchanged actual annotation {hint}"
            );
        }
        let call = mcp_reply(observation, 3).unwrap();
        assert_eq!(
            call["result"]["content"][0]["text"],
            "S6 synthetic accepted call"
        );
        assert_ne!(call["result"]["isError"], true);
    } else {
        assert_eq!(methods(observation, "agent.hook").len(), 1);
        assert_eq!(
            observation.exit.stdout.trim(),
            HOOK_STDOUT,
            "unchanged JSON-only hook stdout"
        );
    }
}

fn require_emitted_build(observation: &Observation) {
    let attach = methods(observation, "agent.attach");
    let build = &attach[0]["params"]["helperBuild"];
    assert!(
        build.is_object(),
        "S6 real helper emits compiled helperBuild after normal reap/listener join"
    );
    assert_eq!(build["gitSha"], crewd_server::build_info::GIT_SHA);
    assert_eq!(build["dirty"], crewd_server::build_info::git_dirty());
    assert_eq!(build["exeStillOnDisk"], true);
    assert_eq!(
        Path::new(build["exe"].as_str().unwrap()),
        Path::new(env!("CARGO_BIN_EXE_crewd"))
    );
}

#[tokio::test]
async fn s6_real_mcp_emits_its_compiled_build_on_attach() {
    let observation = observe("mcp", ReplyMode::Legacy).await.unwrap();
    require_forwarded(&observation, "mcp");
    require_emitted_build(&observation);
}

#[tokio::test]
async fn s6_real_hook_emits_its_compiled_build_on_attach() {
    let observation = observe("hook", ReplyMode::Legacy).await.unwrap();
    require_forwarded(&observation, "hook");
    require_emitted_build(&observation);
}

#[tokio::test]
async fn s6_real_mcp_consumes_skew_without_changing_tool_surface() {
    let observation = observe("mcp", ReplyMode::Skew).await.unwrap();
    require_forwarded(&observation, "mcp");
    let init = mcp_reply(&observation, 1).unwrap();
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains(WARNING),
        "S6 real MCP consumes skew warning after normal reap/listener join"
    );
    let instructions = init["result"]["instructions"].as_str().unwrap();
    assert_eq!(instructions.matches(WARNING).count(), 1);
    assert!(
        instructions.starts_with(INSTRUCTIONS),
        "original instructions remain the prefix"
    );
    assert_eq!(
        instructions.matches(INSTRUCTIONS).count(),
        1,
        "original instructions appear exactly once"
    );
    assert_eq!(
        instructions.strip_prefix(INSTRUCTIONS).unwrap(),
        format!("\n\n{WARNING}"),
        "skew appends one separated warning without discarding normal instructions"
    );
}

#[tokio::test]
async fn s6_real_hook_warns_on_stderr_and_preserves_stdout() {
    let observation = observe("hook", ReplyMode::Skew).await.unwrap();
    require_forwarded(&observation, "hook");
    assert_eq!(
        observation.exit.stderr.matches(WARNING).count(),
        1,
        "S6 real hook consumes skew on stderr after normal reap/listener join"
    );
}

#[tokio::test]
async fn s6_legacy_same_unknown_results_preserve_forwarding_without_skew_warning() {
    for mode in [
        ReplyMode::Legacy,
        ReplyMode::Same,
        ReplyMode::Unknown,
        ReplyMode::Future,
    ] {
        for kind in ["mcp", "hook"] {
            let observation = observe(kind, mode).await.unwrap();
            require_forwarded(&observation, kind);
            assert!(!observation.exit.stderr.contains(WARNING));
            if kind == "mcp" {
                assert_eq!(
                    mcp_reply(&observation, 1).unwrap()["result"]["instructions"],
                    INSTRUCTIONS
                );
            }
        }
    }
}

async fn require_strict_rejection(kind: &str) {
    let observation = observe(kind, ReplyMode::OldStrict).await.unwrap();
    assert_eq!(
        observation.trace.connections, 1,
        "no new connection on strict refusal"
    );
    assert_eq!(
        methods(&observation, "agent.attach").len(),
        1,
        "no attach retry without build"
    );
    assert!(
        methods(&observation, "agent.call").is_empty(),
        "strict daemon never executed the tool"
    );
    assert!(
        methods(&observation, "agent.hook").is_empty(),
        "strict daemon never executed the hook"
    );
    assert!(
        methods(&observation, "agent.tools").is_empty(),
        "no catalogue after failed attach"
    );
    assert!(
        observation
            .exit
            .stderr
            .contains("unknown field helperBuild"),
        "strict rejection remains observable after normal reap/listener join"
    );
    if kind == "hook" {
        assert_eq!(observation.exit.status.code(), Some(0));
        assert!(observation.exit.stdout.is_empty());
    } else {
        assert!(
            observation.exit.status.code().is_some_and(|code| code != 0),
            "strict MCP refusal has a normal nonzero exit code, not signal termination"
        );
    }
}

#[tokio::test]
async fn s6_old_strict_daemon_rejects_new_mcp_without_call_or_retry() {
    require_strict_rejection("mcp").await;
}

#[tokio::test]
async fn s6_old_strict_daemon_rejects_new_hook_without_call_or_retry() {
    require_strict_rejection("hook").await;
}
