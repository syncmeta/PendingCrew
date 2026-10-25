//! `crewd mcp` / `crewd hook` 对着一个假的 crewd.sock：验转发、推送、失败时的保守动作。
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::process::Command;
use tokio::sync::mpsc;

/// A generous wall-clock ceiling for "the hook must not hang": below claude's own hook timeout (15 s),
/// far above the helper's 5 s deadline, so a cold exec under load cannot trip it. Which path a hook
/// took is asserted from its stderr, never from how long it took.
const CEILING: Duration = Duration::from_secs(15);

const SESSION: &str = "s-1";
const TOKEN: &str = "tok-示例";

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    /// attach 之后什么都不回（crewd 卡住）。
    Hang,
    /// Closes the whole connection as soon as `agent.hook` arrives (serve stopping mid-request).
    DropOnHook,
}

struct FakeServe {
    dir: tempfile::TempDir,
    push: mpsc::UnboundedSender<Value>,
}

impl FakeServe {
    async fn start(mode: Mode) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("token"), format!("{TOKEN}\n")).unwrap();
        let listener = UnixListener::bind(dir.path().join("crewd.sock")).unwrap();
        let (push, mut push_rx) = mpsc::unbounded_channel::<Value>();
        tokio::spawn(async move {
            // 只服务一条「推送用」的连接：最先 attach 的那条。
            let mut push_tx: Option<mpsc::UnboundedSender<Value>> = None;
            loop {
                tokio::select! {
                    Some(ev) = push_rx.recv() => {
                        if let Some(t) = &push_tx { let _ = t.send(ev); }
                    }
                    Ok((stream, _)) = listener.accept() => {
                        let (rd, mut wr) = stream.into_split();
                        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
                        // DropOnHook: the push registry must not keep the write half alive, or the "drop" never closes the socket.
                        if push_tx.is_none() && mode != Mode::DropOnHook { push_tx = Some(tx.clone()); }
                        tokio::spawn(async move {
                            while let Some(v) = rx.recv().await {
                                let mut l = serde_json::to_vec(&v).unwrap();
                                l.push(b'\n');
                                if wr.write_all(&l).await.is_err() { break; }
                            }
                        });
                        tokio::spawn(async move {
                            let mut lines = BufReader::new(rd).lines();
                            while let Ok(Some(line)) = lines.next_line().await {
                                let req: Value = serde_json::from_str(&line).unwrap();
                                let id = req["id"].clone();
                                let p = &req["params"];
                                let reply = match req["method"].as_str().unwrap() {
                                    "agent.attach" if p["sessionId"] == SESSION && p["token"] == TOKEN =>
                                        json!({"id": id, "result": {"role": "worker", "crewId": "crew-a", "label": "示例", "instructions": "Synthetic crew tools", "surfaceHash": "synthetic"}}),
                                    "agent.attach" => json!({"id": id, "error": {"code": -32602, "message": "invalid session or token"}}),
                                    _ if mode == Mode::Hang => continue,
                                    "agent.hook" if mode == Mode::DropOnHook => break,
                                    "agent.tools" => json!({"id": id, "result": [
                                        {"name": "post", "description": "发群", "inputSchema": {"type": "object", "properties": {"message": {"type": "string"}}},
                                         "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false}},
                                        {"name": "ask", "description": "问人", "inputSchema": {"type": "object"}}
                                    ]}),
                                    "agent.call" => json!({"id": id, "result": {
                                        "text": format!("called {} with {}", p["tool"].as_str().unwrap(), p["args"]),
                                        "isError": p["tool"] == "ask"
                                    }}),
                                    "agent.hook" => match p["event"].as_str().unwrap() {
                                        "post-tool-use" => json!({"id": id, "result": {"stdout": "{\"hookSpecificOutput\":{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"示例未读\"}}"}}),
                                        "stop" => json!({"id": id, "result": {}}),
                                        e => json!({"id": id, "error": {"code": -32602, "message": format!("unexpected event {e}")}}),
                                    },
                                    m => json!({"id": id, "error": {"code": -32601, "message": m}}),
                                };
                                if tx.send(reply).is_err() { break; }
                            }
                        });
                    }
                }
            }
        });
        Self { dir, push }
    }

    fn data_dir(&self) -> &Path {
        self.dir.path()
    }

    fn token_file(&self) -> PathBuf {
        self.dir.path().join("token")
    }
}

fn crewd(data_dir: &Path, sub: &[&str], token_file: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_crewd"));
    c.args(sub)
        .arg("--data-dir")
        .arg(data_dir)
        .args(["--session", SESSION, "--token-file"])
        .arg(token_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    c
}

async fn run_hook(
    data_dir: &Path,
    event: &str,
    extra: &[&str],
    token_file: &Path,
    stdin: &str,
) -> (String, String, Duration) {
    let started = Instant::now();
    let mut args = vec!["hook", event];
    args.extend_from_slice(extra);
    let mut child = crewd(data_dir, &args, token_file).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .await
        .unwrap();
    let out = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .expect("hook 不该挂住")
        .unwrap();
    assert!(out.status.success(), "hook 永远 exit 0");
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
        started.elapsed(),
    )
}

struct Mcp {
    _child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
}

impl Mcp {
    async fn start(f: &FakeServe) -> Self {
        let mut child = crewd(f.data_dir(), &["mcp"], &f.token_file())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            _child: child,
            stdin,
            lines,
        }
    }
    async fn send(&mut self, v: Value) {
        let mut l = serde_json::to_vec(&v).unwrap();
        l.push(b'\n');
        self.stdin.write_all(&l).await.unwrap();
    }
    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
            .await
            .expect("crewd mcp 没回")
            .unwrap()
            .expect("crewd mcp 关了 stdout");
        serde_json::from_str(&line).unwrap()
    }
    async fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let v = self.recv().await;
            if v["id"] == id {
                return v;
            }
        }
    }
    async fn handshake(&mut self) -> Value {
        let init = self
            .request(
                1,
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "helper-cli-test", "version": "0"}
                }),
            )
            .await;
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        init
    }
}

#[tokio::test]
async fn mcp_bridges_list_and_call_and_errors() {
    let f = FakeServe::start(Mode::Normal).await;
    let mut m = Mcp::start(&f).await;
    let init = m.handshake().await;
    assert_eq!(init["result"]["serverInfo"]["name"], "crewd");
    assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], true);
    let list = m.request(2, "tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["name"], "post");
    assert_eq!(tools[0]["description"], "发群");
    assert_eq!(
        tools[0]["inputSchema"]["properties"]["message"]["type"],
        "string"
    );
    // crewd's hints reach the agent's client under the MCP names; a tool without them (a serve
    // that predates them) is listed without annotations.
    assert_eq!(
        tools[0]["annotations"],
        json!({"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false})
    );
    assert!(tools[1].get("annotations").is_none(), "{}", tools[1]);
    let call = m
        .request(
            3,
            "tools/call",
            json!({"name": "post", "arguments": {"message": "你好"}}),
        )
        .await;
    assert_eq!(
        call["result"]["content"][0]["text"],
        "called post with {\"message\":\"你好\"}"
    );
    assert_ne!(call["result"]["isError"], true);
    let err = m
        .request(4, "tools/call", json!({"name": "ask", "arguments": {}}))
        .await;
    assert_eq!(err["result"]["isError"], true, "serve 的 isError 原样带出");
}

#[tokio::test]
async fn tools_changed_push_becomes_list_changed_notification() {
    let f = FakeServe::start(Mode::Normal).await;
    let mut m = Mcp::start(&f).await;
    m.handshake().await;
    f.push
        .send(json!({"event": "agent.tools_changed", "data": {"role": "captain"}}))
        .unwrap();
    loop {
        let v = m.recv().await;
        if v["method"] == "notifications/tools/list_changed" {
            break;
        }
    }
}

#[tokio::test]
async fn mcp_refuses_to_start_with_a_wrong_token() {
    let f = FakeServe::start(Mode::Normal).await;
    std::fs::write(f.token_file(), "forged").unwrap();
    let out = tokio::time::timeout(
        Duration::from_secs(15),
        crewd(f.data_dir(), &["mcp"], &f.token_file()).output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid session or token"));
}

#[tokio::test]
async fn hook_prints_serve_stdout_verbatim() {
    let f = FakeServe::start(Mode::Normal).await;
    let (out, _, _) = run_hook(
        f.data_dir(),
        "post-tool-use",
        &[],
        &f.token_file(),
        "{\"tool_name\":\"Bash\"}",
    )
    .await;
    assert_eq!(
        out.trim_end(),
        "{\"hookSpecificOutput\":{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"示例未读\"}}"
    );
    let (out, _, _) = run_hook(f.data_dir(), "stop", &[], &f.token_file(), "{}").await;
    assert_eq!(out, "", "serve 没给 stdout 就什么都不打");
}

#[tokio::test]
async fn hook_without_crewd_is_silent() {
    let empty = tempfile::tempdir().unwrap();
    let tf = empty.path().join("token");
    std::fs::write(&tf, TOKEN).unwrap();
    for ev in ["post-tool-use", "stop"] {
        let (out, err, took) = run_hook(empty.path(), ev, &[], &tf, "{}").await;
        assert_eq!(out, "", "{ev}");
        assert!(
            err.contains("cannot connect"),
            "{ev}: stderr names the failed connect: {err}"
        );
        assert!(
            !err.contains("no answer within"),
            "{ev}: it failed fast, not by deadline: {err}"
        );
        assert!(took < CEILING, "{ev}: {took:?}");
    }
}

#[tokio::test]
async fn crewd_gone_during_a_hook_is_silent() {
    // serve goes away in the middle of the request -> the helper notices the closed connection
    // (stderr says so) and stays silent; it does not sit on the deadline timer.
    let f = FakeServe::start(Mode::DropOnHook).await;
    let (out, err, took) = run_hook(
        f.data_dir(),
        "post-tool-use",
        &[],
        &f.token_file(),
        "{\"tool_name\":\"x\"}",
    )
    .await;
    assert_eq!(out, "");
    assert!(
        err.contains("connection to crewd closed before it answered"),
        "stderr names the closed connection: {err}"
    );
    assert!(!err.contains("no answer within"), "{err}");
    assert!(took < CEILING, "{took:?}");
}

#[tokio::test]
async fn hook_gives_up_when_crewd_hangs() {
    // Review Focus 1: the helper's own deadline ends it (stderr says so); claude's 15 s never matters.
    let f = FakeServe::start(Mode::Hang).await;
    for ev in ["post-tool-use", "stop"] {
        let (out, err, took) = run_hook(f.data_dir(), ev, &[], &f.token_file(), "{}").await;
        assert_eq!(out, "", "{ev}");
        assert!(err.contains("no answer within"), "{ev}: {err}");
        assert!(took < CEILING, "{ev}: {took:?}");
    }
}

#[tokio::test]
async fn bad_stdin_or_unknown_event_never_prints_garbage() {
    let f = FakeServe::start(Mode::Normal).await;
    let (out, err, _) = run_hook(
        f.data_dir(),
        "post-tool-use",
        &[],
        &f.token_file(),
        "not json",
    )
    .await;
    assert_eq!(out, "");
    assert!(!err.is_empty());
    let (out, err, _) = run_hook(
        f.data_dir(),
        "future-unknown-hook",
        &[],
        &f.token_file(),
        "{}",
    )
    .await;
    assert_eq!(out, "");
    assert!(err.contains("future-unknown-hook"));
}

#[tokio::test]
async fn obsolete_pre_tool_use_event_is_silent_and_exits_zero() {
    // B16: PendingCrew no longer registers a PreToolUse hook. A settings file written by an older
    // build may still call `crewd hook pre-tool-use`; that must be a silent no-op (exit 0, no
    // stdout), never an error, never a call to serve, never a deny.
    let f = FakeServe::start(Mode::Normal).await;
    let (out, err, _) = run_hook(
        f.data_dir(),
        "pre-tool-use",
        &[],
        &f.token_file(),
        "{\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"mcp__computer-use__click\"}",
    )
    .await;
    assert_eq!(out, "");
    assert!(err.contains("unknown event"), "{err}");
    assert!(
        !err.contains("unexpected event"),
        "it must not reach serve: {err}"
    );
    // Not even a reachable serve is needed.
    let empty = tempfile::tempdir().unwrap();
    let (out, err, _) = run_hook(
        empty.path(),
        "pre-tool-use",
        &[],
        &empty.path().join("none"),
        "{}",
    )
    .await;
    assert_eq!(out, "");
    assert!(
        err.contains("unknown event") && !err.contains("cannot connect"),
        "{err}"
    );
}

#[tokio::test]
async fn hook_with_stdin_held_open_still_ends_by_its_deadline() {
    // claude did not close stdin (or is stuck): reading stdin is inside the 5 s deadline, so the
    // hook ends, silent, and says why on stderr.
    let f = FakeServe::start(Mode::Normal).await;
    for ev in ["post-tool-use", "stop"] {
        let mut child = crewd(f.data_dir(), &["hook", ev], &f.token_file())
            .spawn()
            .unwrap();
        let _held = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let status = tokio::time::timeout(CEILING, child.wait())
            .await
            .expect("stdin not closed must not hang the hook")
            .unwrap();
        assert!(status.success(), "{ev}: hook 永远 exit 0");
        let (mut out, mut err) = (String::new(), String::new());
        stdout.read_to_string(&mut out).await.unwrap();
        stderr.read_to_string(&mut err).await.unwrap();
        assert_eq!(out, "", "{ev}");
        assert!(err.contains("stdin not closed within"), "{ev}: {err}");
    }
}

#[tokio::test]
async fn events_that_arrive_before_the_attach_answer_are_kept() {
    // serve 可能先推 tools_changed、再回 attach（订阅先于读身份）：helper 不能丢掉这条。
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("token"), TOKEN).unwrap();
    let listener = UnixListener::bind(dir.path().join("crewd.sock")).unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (rd, mut wr) = stream.into_split();
        let mut lines = BufReader::new(rd).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let req: Value = serde_json::from_str(&line).unwrap();
            let id = req["id"].clone();
            let frames = match req["method"].as_str().unwrap() {
                "agent.attach" => vec![
                    json!({"event": "agent.tools_changed", "data": {"role": "captain"}}),
                    json!({"id": id, "result": {"role": "worker", "crewId": "crew-a", "label": "示例", "instructions": "Synthetic crew tools", "surfaceHash": "synthetic"}}),
                ],
                "agent.tools" => vec![json!({"id": id, "result": []})],
                m => vec![json!({"id": id, "error": {"code": -32601, "message": m}})],
            };
            for v in frames {
                let mut l = serde_json::to_vec(&v).unwrap();
                l.push(b'\n');
                wr.write_all(&l).await.unwrap();
            }
        }
    });
    let mut child = crewd(dir.path(), &["mcp"], &dir.path().join("token"))
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    for v in [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "helper-cli-test", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    ] {
        let mut l = serde_json::to_vec(&v).unwrap();
        l.push(b'\n');
        stdin.write_all(&l).await.unwrap();
    }
    loop {
        let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await
            .expect("attach 之前到的 tools_changed 丢了")
            .unwrap()
            .expect("crewd mcp 关了 stdout");
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["method"] == "notifications/tools/list_changed" {
            break;
        }
    }
}

// ---- 控制者裁定（Task 8 review）：关掉 hook 的放行路径；mcp 失败文字分清「被拒」与「断在半路」。

/// 起 `crewd <args>`（不加任何默认参数），写完 stdin 就关；返回 (退出成功?, stdout, stderr, 用时)。
async fn run_raw(
    args: &[&std::ffi::OsStr],
    envs: &[(&str, &str)],
    stdin: &str,
) -> (bool, String, String, Duration) {
    let started = Instant::now();
    let mut c = Command::new(env!("CARGO_BIN_EXE_crewd"));
    c.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in envs {
        c.env(k, v);
    }
    let mut child = c.spawn().unwrap();
    // 子进程可能不读 stdin 就退了：写失败不算测试失败。
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .await;
    let out = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .expect("hook 不该挂住")
        .unwrap();
    (
        out.status.success(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
        started.elapsed(),
    )
}

#[tokio::test]
async fn hook_argv_errors_exit_zero_and_print_nothing() {
    // exit 2 对 Stop 是「挡住」、对 PostToolUse 是把 stderr 喂回给模型；命令行错一律 exit 0、不打 stdout。
    let f = FakeServe::start(Mode::Normal).await;
    let d = f.data_dir().as_os_str().to_owned();
    let t = f.token_file().into_os_string();
    let o = |s: &'static str| std::ffi::OsStr::new(s);
    let cases: Vec<Vec<&std::ffi::OsStr>> = vec![
        vec![
            o("hook"),
            o("pre-tool-use"),
            o("--data-dir"),
            &d,
            o("--session"),
            o(SESSION),
            o("--token-file"),
            &t,
            o("--bogus"),
        ],
        vec![
            o("hook"),
            o("pre-tool-use"),
            o("--data-dir"),
            &d,
            o("--session"),
            o(SESSION),
        ],
        vec![
            o("--data-dir"),
            &d,
            o("hook"),
            o("--bogus"),
            o("pre-tool-use"),
            o("--session"),
            o(SESSION),
        ],
        vec![
            o("hook"),
            o("post-tool-use"),
            o("--data-dir"),
            &d,
            o("--session"),
            o(SESSION),
            o("--token-file"),
            &t,
            o("--bogus"),
        ],
        vec![
            o("hook"),
            o("stop"),
            o("--data-dir"),
            &d,
            o("--session"),
            o(SESSION),
        ],
        vec![o("hook")],
    ];
    for args in cases {
        let (ok, out, err, _) = run_raw(&args, &[], "{\"tool_name\":\"x\"}").await;
        assert!(ok, "{args:?}: hook 永远 exit 0 ({err})");
        assert_eq!(out, "", "{args:?}");
    }
}

#[tokio::test]
async fn token_file_that_never_opens_is_inside_the_deadline() {
    // FIFO 没人写：打开就卡住。读 token 也算在 5s 里。
    let f = FakeServe::start(Mode::Normal).await;
    let fifo = f.data_dir().join("token-fifo");
    let st = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(st.success());
    for ev in ["post-tool-use", "stop"] {
        let (out, err, took) =
            run_hook(f.data_dir(), ev, &[], &fifo, "{\"tool_name\":\"x\"}").await;
        assert_eq!(out, "", "{ev}");
        assert!(err.contains("no answer within"), "{ev}: {err}");
        assert!(took < CEILING, "{ev}: {took:?}");
    }
}

#[tokio::test]
async fn a_panic_exits_zero_and_stays_silent() {
    // 注入的 panic 只在 debug 构建里存在（`CREWD_TEST_HOOK_PANIC`，见 hook_cmd）。
    let f = FakeServe::start(Mode::Normal).await;
    let env = [("CREWD_TEST_HOOK_PANIC", "1")];
    for ev in ["post-tool-use", "stop"] {
        let d = f.data_dir().as_os_str().to_owned();
        let t = f.token_file().into_os_string();
        let o = std::ffi::OsStr::new;
        let args = [
            o("hook"),
            o(ev),
            o("--data-dir"),
            &d,
            o("--session"),
            o(SESSION),
            o("--token-file"),
            &t,
        ];
        let (ok, out, err, _) = run_raw(&args, &env, "{\"tool_name\":\"x\"}").await;
        assert!(ok, "{ev}: panic 也 exit 0 ({err})");
        assert!(err.contains("panic"), "{ev}: panic 写 stderr：{err}");
        assert_eq!(out, "", "{ev}");
    }
}

#[tokio::test]
async fn closed_stdout_is_a_real_panic_path_and_still_exits_zero() {
    // 不靠注入：claude 那头关了 stdout，println! 写断管会 panic。
    let f = FakeServe::start(Mode::Normal).await;
    let mut child = crewd(f.data_dir(), &["hook", "post-tool-use"], &f.token_file())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    child.stdin.take().unwrap().write_all(b"{}").await.unwrap();
    let mut err = String::new();
    let mut stderr = child.stderr.take().unwrap();
    let status = tokio::time::timeout(Duration::from_secs(20), child.wait())
        .await
        .unwrap()
        .unwrap();
    stderr.read_to_string(&mut err).await.unwrap();
    assert!(status.success(), "{status:?}: {err}");
    assert!(err.contains("panic"), "这条路确实 panic 了：{err}");
}

/// 一条连接的脚本化假 serve：`reply` 返回 None = 收到这个请求后直接断开。
fn scripted_serve(dir: &Path, reply: fn(&str, &Value, Value) -> Option<Value>) {
    std::fs::write(dir.join("token"), TOKEN).unwrap();
    let listener = UnixListener::bind(dir.join("crewd.sock")).unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (rd, mut wr) = stream.into_split();
        let mut lines = BufReader::new(rd).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let req: Value = serde_json::from_str(&line).unwrap();
            let id = req["id"].clone();
            let v = match req["method"].as_str().unwrap() {
                "agent.attach" => {
                    json!({"id": id, "result": {"role": "worker", "crewId": "crew-a", "label": "示例", "instructions": "Synthetic crew tools", "surfaceHash": "synthetic"}})
                }
                m => match reply(m, &req["params"], id) {
                    Some(v) => v,
                    None => return,
                },
            };
            let mut l = serde_json::to_vec(&v).unwrap();
            l.push(b'\n');
            wr.write_all(&l).await.unwrap();
        }
    });
}

async fn mcp_call_text(dir: &Path) -> Value {
    let child = crewd(dir, &["mcp"], &dir.join("token")).spawn().unwrap();
    let mut child = child;
    let stdin = child.stdin.take().unwrap();
    let lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut m = Mcp {
        _child: child,
        stdin,
        lines,
    };
    m.handshake().await;
    let r = m
        .request(
            2,
            "tools/call",
            json!({"name": "post", "arguments": {"message": "示例"}}),
        )
        .await;
    assert_eq!(r["result"]["isError"], true, "{r}");
    r["result"]["content"][0]["text"].clone()
}

#[tokio::test]
async fn call_rejected_by_serve_says_what_serve_said() {
    let dir = tempfile::tempdir().unwrap();
    scripted_serve(dir.path(), |_, _, id| {
        Some(json!({"id": id, "error": {"code": -32602, "message": "unknown tool: post"}}))
    });
    let text = mcp_call_text(dir.path()).await;
    let text = text.as_str().unwrap();
    assert!(text.contains("unknown tool: post"), "{text}");
    assert!(text.contains("rejected"), "{text}");
    assert!(!text.contains("may or may not"), "{text}");
}

#[tokio::test]
async fn call_lost_after_it_was_sent_says_check_before_retrying() {
    let dir = tempfile::tempdir().unwrap();
    scripted_serve(dir.path(), |_, _, _| None);
    let text = mcp_call_text(dir.path()).await;
    let text = text.as_str().unwrap();
    assert!(text.contains("may or may not"), "{text}");
    assert!(text.contains("check"), "{text}");
    assert!(!text.contains("rejected"), "{text}");
}

#[tokio::test]
async fn managed_prompt_guard_fails_closed_on_errors_without_changing_observation_hooks() {
    for mode in [Mode::Normal, Mode::DropOnHook] {
        let serve = FakeServe::start(mode).await;
        for (input, extra) in [
            ("{}", vec![]),
            ("not-json", vec![]),
            ("[]", vec![]),
            ("{}", vec!["--invalid-argument"]),
        ] {
            let mut args = vec!["hook", "user-prompt-submit"];
            args.extend(extra);
            let mut child = crewd(serve.data_dir(), &args, &serve.token_file())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .await
                .unwrap();
            let out = tokio::time::timeout(CEILING, child.wait_with_output())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(out.status.code(), Some(2));
            assert!(out.stdout.is_empty());
        }
    }
    let panic_serve = FakeServe::start(Mode::Normal).await;
    let mut child = crewd(
        panic_serve.data_dir(),
        &["hook", "user-prompt-submit"],
        &panic_serve.token_file(),
    )
    .env("CREWD_TEST_HOOK_PANIC", "1")
    .spawn()
    .unwrap();
    child.stdin.take().unwrap().write_all(b"{}").await.unwrap();
    let panic = tokio::time::timeout(CEILING, child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(panic.status.code(), Some(2));
    assert!(panic.stdout.is_empty());
    let serve = FakeServe::start(Mode::Normal).await;
    let (stdout, _, _) = run_hook(serve.data_dir(), "stop", &[], &serve.token_file(), "{}").await;
    assert!(stdout.is_empty(), "legacy Stop remains observational");
}

const NOTIFICATION_REQUEST: &str = "10000000-0000-4000-8000-000000000001";
const SYNTHETIC_PUSH_KEY: &str = "U1lOVEhFVElDLVBSSVZBVEUtUEtDUzg=";
fn notification_input() -> Value {
    json!({"requestId":NOTIFICATION_REQUEST,"expectedRevision":"1","enabled":true,
        "provider":{"teamId":"ABCDEFGHIJ","keyId":"1234567890","environment":"sandbox",
                    "topic":"com.example.synthetic","privateKeyPkcs8Base64":SYNTHETIC_PUSH_KEY}})
}
fn notification_known() -> Value {
    json!({"kind":"known","operation":"notification.configure","result":{
        "available":true,"enabled":true,"reason":"credentialsNotLoaded","revision":"2",
        "environment":"sandbox","topic":"com.example.synthetic"}})
}
async fn notification_cli(data:&Path, args:&[&str], input:Option<Vec<u8>>) -> std::process::Output {
    let mut child=Command::new(env!("CARGO_BIN_EXE_crewd"))
        .env_clear().env("PATH","/usr/bin:/bin").arg("--data-dir").arg(data).arg("notification").args(args)
        .stdin(if input.is_some(){Stdio::piped()}else{Stdio::null()}).stdout(Stdio::piped()).stderr(Stdio::piped())
        .kill_on_drop(true).spawn().unwrap();
    if let Some(input)=input {child.stdin.take().unwrap().write_all(&input).await.unwrap();}
    tokio::time::timeout(Duration::from_secs(15),child.wait_with_output()).await.unwrap().unwrap()
}
async fn notification_request(listener:&UnixListener, result:Option<Value>) -> Value {
    let(stream,_)=tokio::time::timeout(Duration::from_secs(10),listener.accept()).await.unwrap().unwrap();
    let(mut rd,mut wr)=stream.into_split();let mut line=String::new();
    tokio::time::timeout(Duration::from_secs(10),BufReader::new(&mut rd).read_line(&mut line)).await.unwrap().unwrap();
    let request:Value=serde_json::from_str(&line).unwrap();
    if let Some(result)=result {wr.write_all((json!({"id":request["id"],"result":result}).to_string()+"\n").as_bytes()).await.unwrap();}
    request
}
fn assert_notification_secret_absent(output:&std::process::Output) {
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SYNTHETIC_PUSH_KEY));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(SYNTHETIC_PUSH_KEY));
}
#[tokio::test]
async fn notification_cli_typed_configuration_reaches_original_socket_once() {
    use std::os::unix::fs::PermissionsExt;
    let root=tempfile::Builder::new().prefix("pcwn-").tempdir_in("/private/tmp").unwrap();
    crewd_server::ensure_data_dir(root.path()).unwrap();
    let listener=UnixListener::bind(root.path().join("crewd.sock")).unwrap();
    let server=tokio::spawn(async move {notification_request(&listener,Some(notification_known())).await});
    let output=notification_cli(root.path(),&["configure"],Some(serde_json::to_vec(&notification_input()).unwrap())).await;
    assert!(output.status.success(),"safe notification configuration command missing or failed: {}",String::from_utf8_lossy(&output.stderr));
    assert_notification_secret_absent(&output);
    let request=tokio::time::timeout(Duration::from_secs(12),server).await.unwrap().unwrap();assert_eq!(request["method"],"notification.configure");
    assert_eq!(request["params"]["requestId"],NOTIFICATION_REQUEST);
    assert_eq!(request["params"]["provider"]["privateKeyPkcs8Base64"],SYNTHETIC_PUSH_KEY);
    let witness=root.path().join("notification-cli").join(format!("notification-{NOTIFICATION_REQUEST}.json"));
    let bytes=std::fs::read(&witness).unwrap();assert!(!String::from_utf8_lossy(&bytes).contains(SYNTHETIC_PUSH_KEY));
    let record:Value=serde_json::from_slice(&bytes).unwrap();assert_eq!(record["requestId"],NOTIFICATION_REQUEST);
    assert_eq!(record["kind"],"notificationConfiguration");assert_eq!(record.as_object().unwrap().len(),4);
    assert_eq!(std::fs::metadata(witness).unwrap().permissions().mode()&0o777,0o600);
    assert_eq!(std::fs::metadata(root.path().join("notification-cli")).unwrap().permissions().mode()&0o777,0o700);
}
#[tokio::test]
async fn notification_cli_lost_reply_retains_uuid_and_only_explicit_lookup_sends() {
    let root=tempfile::Builder::new().prefix("pcwn-").tempdir_in("/private/tmp").unwrap();
    crewd_server::ensure_data_dir(root.path()).unwrap();
    let listener=UnixListener::bind(root.path().join("crewd.sock")).unwrap();
    let server=tokio::spawn(async move {
        let first=notification_request(&listener,None).await;
        let second=notification_request(&listener,Some(json!({"kind":"unknown","operation":"notification.configure"}))).await;
        (first,second)
    });
    let input=serde_json::to_vec(&notification_input()).unwrap();
    let first=notification_cli(root.path(),&["configure"],Some(input.clone())).await;
    assert!(!first.status.success());assert_notification_secret_absent(&first);
    assert!(String::from_utf8_lossy(&first.stderr).contains(NOTIFICATION_REQUEST));
    let retry=notification_cli(root.path(),&["configure"],Some(input)).await;
    assert!(!retry.status.success());assert_notification_secret_absent(&retry);
    let lookup=notification_cli(root.path(),&["lookup","--request-id",NOTIFICATION_REQUEST],None).await;
    assert!(lookup.status.success(),"{}",String::from_utf8_lossy(&lookup.stderr));assert_notification_secret_absent(&lookup);
    let(first,second)=tokio::time::timeout(Duration::from_secs(12),server).await.unwrap().unwrap();assert_eq!(first["method"],"notification.configure");
    assert_eq!(second["method"],"notification.requestLookup");
    assert_eq!(second["params"],json!({"requestId":NOTIFICATION_REQUEST}));
    assert_eq!(serde_json::from_slice::<Value>(&lookup.stdout).unwrap()["result"]["kind"],"unknown");
}

#[tokio::test]
async fn notification_cli_invalid_input_or_private_paths_send_nothing() {
    use std::os::unix::fs::PermissionsExt;
    for mode in ["bad-json","oversize","bad-uuid","bad-revision","unknown-field","file-mode","file-symlink","file-owner","root-mode","root-symlink","witness-symlink"] {
        let root=tempfile::Builder::new().prefix("pcwn-").tempdir_in("/private/tmp").unwrap();
    crewd_server::ensure_data_dir(root.path()).unwrap();
        let listener=UnixListener::bind(root.path().join("crewd.sock")).unwrap();
        let mut input=notification_input();let mut payload=None;let mut file=None;let mut data=root.path().to_owned();
        match mode {
            "bad-json"=>payload=Some(format!("{{{SYNTHETIC_PUSH_KEY}").into_bytes()),
            "oversize"=>payload=Some(vec![b'x';16385]),
            "bad-uuid"=>input["requestId"]=json!("not-a-uuid"),
            "bad-revision"=>input["expectedRevision"]=json!("01"),
            "unknown-field"=>input["privateKey"]=json!(SYNTHETIC_PUSH_KEY),
            "file-owner"=>file=Some(PathBuf::from("/etc/passwd")),
            "root-mode"=>std::fs::set_permissions(root.path(),std::fs::Permissions::from_mode(0o755)).unwrap(),
            "root-symlink"=>{data=root.path().join("data-link");std::os::unix::fs::symlink(root.path(),&data).unwrap();},
            "witness-symlink"=>std::os::unix::fs::symlink(root.path(),root.path().join("notification-cli")).unwrap(),
            "file-mode"|"file-symlink"=>{
                let path=root.path().join("input.json");std::fs::write(&path,serde_json::to_vec(&input).unwrap()).unwrap();
                std::fs::set_permissions(&path,std::fs::Permissions::from_mode(if mode=="file-mode"{0o644}else{0o600})).unwrap();
                if mode=="file-symlink" {let alias=root.path().join("input-link");std::os::unix::fs::symlink(path,&alias).unwrap();file=Some(alias);}else{file=Some(path);}
            },
            _=>{},
        }
        if file.is_none() && payload.is_none(){payload=Some(serde_json::to_vec(&input).unwrap());}
        let file_text=file.as_ref().map(|f|f.to_str().unwrap());
        let args=if let Some(file)=file_text{vec!["configure","--input-file",file]}else{vec!["configure"]};
        let output=notification_cli(&data,&args,payload).await;
        assert!(!output.status.success(),"{mode} unexpectedly accepted");assert_notification_secret_absent(&output);
        assert!(tokio::time::timeout(Duration::from_millis(30),listener.accept()).await.is_err(),"{mode} contacted Core before validation");
        std::fs::set_permissions(root.path(),std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[tokio::test]
async fn notification_cli_protected_file_and_lookup_not_found_never_replay() {
    use std::os::unix::fs::PermissionsExt;
    let root=tempfile::Builder::new().prefix("pcwn-").tempdir_in("/private/tmp").unwrap();
    crewd_server::ensure_data_dir(root.path()).unwrap();
    // The real Core creator supplies its normal private data root beneath
    // ordinary0755 application-support parents; ancestors need not be0700.
    let parent=root.path().join("Library/Application Support");std::fs::create_dir_all(&parent).unwrap();
    std::fs::set_permissions(&parent,std::fs::Permissions::from_mode(0o755)).unwrap();
    let data=parent.join("PendingCrewLocal");crewd_server::ensure_data_dir(&data).unwrap();
    let file=root.path().join("input.json");let input=serde_json::to_vec(&notification_input()).unwrap();
    std::fs::write(&file,&input).unwrap();std::fs::set_permissions(&file,std::fs::Permissions::from_mode(0o600)).unwrap();
    let listener=UnixListener::bind(data.join("crewd.sock")).unwrap();
    let server=tokio::spawn(async move {
        let a=notification_request(&listener,Some(json!({"kind":"unknown","operation":"notification.configure"}))).await;
        let b=notification_request(&listener,Some(json!({"kind":"notFound"}))).await;(a,b)
    });
    let configured=notification_cli(&data,&["configure","--input-file",file.to_str().unwrap()],None).await;
    assert!(configured.status.success());assert_notification_secret_absent(&configured);
    assert_eq!(std::fs::read(&file).unwrap(),input);
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode()&0o777,0o600);
    let looked=notification_cli(&data,&["lookup","--request-id",NOTIFICATION_REQUEST],None).await;
    assert!(looked.status.success());assert_notification_secret_absent(&looked);
    assert_eq!(serde_json::from_slice::<Value>(&looked.stdout).unwrap()["result"]["kind"],"notFound");
    let(a,b)=tokio::time::timeout(Duration::from_secs(12),server).await.unwrap().unwrap();assert_eq!(a["method"],"notification.configure");assert_eq!(b["method"],"notification.requestLookup");
}

#[tokio::test]
async fn notification_cli_never_echoes_secret_in_invalid_reply_or_rpc_error() {
    for mode in ["unknown-fields","status-topic","operation","rpc-error","malformed","oversize"] {
        let root=tempfile::Builder::new().prefix("pcwn-").tempdir_in("/private/tmp").unwrap();
    crewd_server::ensure_data_dir(root.path()).unwrap();
        let listener=UnixListener::bind(root.path().join("crewd.sock")).unwrap();
        let server=tokio::spawn(async move {
            let(mut stream,_)=tokio::time::timeout(Duration::from_secs(10),listener.accept()).await.unwrap().unwrap();let mut line=String::new();
            tokio::time::timeout(Duration::from_secs(10),BufReader::new(&mut stream).read_line(&mut line)).await.unwrap().unwrap();
            let mut value=notification_known();
            match mode {
                "unknown-fields"=>value["result"]["privateKeyPkcs8Base64"]=json!(SYNTHETIC_PUSH_KEY),
                "status-topic"=>value["result"]["topic"]=json!(SYNTHETIC_PUSH_KEY),
                "operation"=>value["operation"]=json!(SYNTHETIC_PUSH_KEY),
                _=>{},
            }
            let reply=match mode {
                "rpc-error"=>json!({"id":1,"error":{"code":-32009,"message":SYNTHETIC_PUSH_KEY}}).to_string()+"\n",
                "malformed"=>SYNTHETIC_PUSH_KEY.to_owned()+"\n",
                "oversize"=>format!("{}{}\n","x".repeat(16385),SYNTHETIC_PUSH_KEY),
                _=>json!({"id":1,"result":value}).to_string()+"\n",
            };
            let _=stream.write_all(reply.as_bytes()).await;
        });
        let output=notification_cli(root.path(),&["configure"],Some(serde_json::to_vec(&notification_input()).unwrap())).await;
        if mode!="status-topic"{assert!(!output.status.success(),"{mode}");}
        assert_notification_secret_absent(&output);tokio::time::timeout(Duration::from_secs(12),server).await.unwrap().unwrap();
        assert!(root.path().join("notification-cli").join(format!("notification-{NOTIFICATION_REQUEST}.json")).is_file());
    }
}
