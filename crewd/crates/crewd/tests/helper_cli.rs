//! `crewd mcp` / `crewd hook` 对着一个假的 crewd.sock：验转发、推送、失败时的保守动作。
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::process::Command;
use tokio::sync::mpsc;

const SESSION: &str = "s-1";
const TOKEN: &str = "tok-示例";

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    /// attach 之后什么都不回（crewd 卡住）。
    Hang,
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
                        if push_tx.is_none() { push_tx = Some(tx.clone()); }
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
                                        json!({"id": id, "result": {"role": "worker", "crewId": "crew-a", "label": "示例"}}),
                                    "agent.attach" => json!({"id": id, "error": {"code": -32602, "message": "invalid session or token"}}),
                                    _ if mode == Mode::Hang => continue,
                                    "agent.tools" => json!({"id": id, "result": [
                                        {"name": "post", "description": "发群", "inputSchema": {"type": "object", "properties": {"message": {"type": "string"}}}},
                                        {"name": "ask", "description": "问人", "inputSchema": {"type": "object"}}
                                    ]}),
                                    "agent.call" => json!({"id": id, "result": {
                                        "text": format!("called {} with {}", p["tool"].as_str().unwrap(), p["args"]),
                                        "isError": p["tool"] == "ask"
                                    }}),
                                    "agent.hook" => match p["event"].as_str().unwrap() {
                                        "post-tool-use" => json!({"id": id, "result": {"stdout": "{\"hookSpecificOutput\":{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"示例未读\"}}"}}),
                                        "pre-tool-use" => json!({"id": id, "result": {"stdout": format!("{{\"echo\":{}}}", p["payload"]["tool_name"])}}),
                                        _ => json!({"id": id, "result": {}}),
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
    let (out, _, _) = run_hook(
        f.data_dir(),
        "pre-tool-use",
        &[],
        &f.token_file(),
        "{\"tool_name\":\"mcp__computer-use__click\"}",
    )
    .await;
    assert_eq!(out.trim_end(), "{\"echo\":\"mcp__computer-use__click\"}");
}

#[tokio::test]
async fn hook_without_crewd_is_silent_except_pre_tool_use_which_denies() {
    let empty = tempfile::tempdir().unwrap();
    let tf = empty.path().join("token");
    std::fs::write(&tf, TOKEN).unwrap();
    for ev in ["post-tool-use", "stop"] {
        let (out, err, took) = run_hook(empty.path(), ev, &[], &tf, "{}").await;
        assert_eq!(out, "", "{ev}");
        assert!(!err.is_empty(), "{ev}: 失败要写 stderr");
        assert!(took < Duration::from_secs(6), "{ev}: {took:?}");
    }
    let (out, _, _) = run_hook(
        empty.path(),
        "pre-tool-use",
        &[],
        &tf,
        "{\"tool_name\":\"x\"}",
    )
    .await;
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
}

#[tokio::test]
async fn hook_gives_up_quickly_when_crewd_hangs() {
    // Review Focus 1
    let f = FakeServe::start(Mode::Hang).await;
    let (out, _, took) = run_hook(f.data_dir(), "post-tool-use", &[], &f.token_file(), "{}").await;
    assert_eq!(out, "");
    assert!(took < Duration::from_secs(7), "{took:?}");
    let (out, _, took) = run_hook(f.data_dir(), "pre-tool-use", &[], &f.token_file(), "{}").await;
    assert!(
        out.contains("\"deny\""),
        "到 helper 自己的期限（5s）就 deny：{out}"
    );
    assert!(took < Duration::from_secs(7), "{took:?}");
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
    let (out, _, _) = run_hook(
        f.data_dir(),
        "pre-tool-use",
        &[],
        &f.token_file(),
        "not json",
    )
    .await;
    assert!(
        out.contains("\"deny\""),
        "看不懂的 PreToolUse 输入：保守 deny"
    );
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
async fn hook_with_stdin_held_open_still_answers_within_its_deadline() {
    // claude 没关 stdin（或卡住）：读 stdin 也算在 5s 期限里，命中闸的 pre-tool-use 照样 deny、进程照样退出。
    let f = FakeServe::start(Mode::Normal).await;
    for (ev, denies) in [("pre-tool-use", true), ("post-tool-use", false)] {
        let started = Instant::now();
        let mut child = crewd(f.data_dir(), &["hook", ev], &f.token_file())
            .spawn()
            .unwrap();
        let _held = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let status = tokio::time::timeout(Duration::from_secs(20), child.wait())
            .await
            .expect("stdin 不关也不该挂住")
            .unwrap();
        assert!(status.success(), "{ev}: hook 永远 exit 0");
        let mut out = String::new();
        stdout.read_to_string(&mut out).await.unwrap();
        assert_eq!(out.contains("\"deny\""), denies, "{ev}: {out}");
        if !denies {
            assert_eq!(out, "", "{ev}");
        }
        assert!(
            started.elapsed() < Duration::from_secs(7),
            "{ev}: {:?}",
            started.elapsed()
        );
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
                    json!({"id": id, "result": {"role": "worker", "crewId": "crew-a", "label": "示例"}}),
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
