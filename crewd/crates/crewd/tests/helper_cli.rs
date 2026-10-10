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
        "user-prompt-submit",
        &[],
        &f.token_file(),
        "{}",
    )
    .await;
    assert_eq!(out, "");
    assert!(err.contains("user-prompt-submit"));
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
