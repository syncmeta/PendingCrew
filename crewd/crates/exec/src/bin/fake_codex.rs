//! 测试器材：假的 `codex app-server`（只给 `tests/fake_codex.rs` 用）。
//!
//! 讲真 codex 的 stdio 协议形状（一行一个 JSON、没有 `jsonrpc` 字段、服务端请求的 id 有整数也有
//! 字符串），行为由环境变量控制——执行器只放行调用方显式给的变量，所以测试经
//! `CodexSpec::additional_env` 传：
//!
//! - `FAKE_CODEX_LOG`：收到的每一行原样追加到这个文件。
//! - `FAKE_CODEX_ENV_LOG`：启动时把自己环境变量的**名字**（排序）写到这个文件。
//! - `FAKE_CODEX_PID`：启动时把自己的 pid 写到这个文件。
//! - `FAKE_CODEX_CHILD_PID`：`spawn_child` 模式下把起的 `sleep` 的 pid 写到这个文件。
//! - `FAKE_CODEX_TERM_LOG`：`count_term` 模式下每收到一次 SIGTERM 往这个文件追加一个 `T`。
//! - `FAKE_CODEX_MODE`：逗号分隔的开关：
//!   `die_on_init`（收到 initialize 就往 stderr 写一句、exit 3）、`init_silent`（不回 initialize）、
//!   `stderr_flood`（回 initialize 之前往 stderr 灌 1 MiB）、`resume_fails`、`turn_hangs`（不回
//!   turn/start）、`slow_turn`（回合一直跑到被 turn/interrupt）、`server_requests`（回合里依次发 6 种
//!   服务端请求，每条都等到回应才继续）、`usage_limit`、`big_context`（回合后上下文 190k/258k，
//!   等压缩）、`spawn_child`（回合里起一个 `sleep 600`）、`spawn_child_inherit`（同上，但它继承
//!   codex 的 stdout / stderr，codex 退出后管道仍被握着）、`deaf`（thread/start 之后再也不读 stdin）、
//!   `huge_item`（回合里发一条 3 MB 的 agent 正文）、`exit_after_turn`（第一个回合结束就自己
//!   退出）、`ignore_term`（忽略 SIGTERM）、`count_term`（不退，每次 SIGTERM 记一笔）。
//!
//! 缺省回合：回 `turn/start`、`turn/started`、userMessage 条目、`GOT: <正文>` 的 agentMessage
//! 条目、小额 token 读数、`turn/completed`。
use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::os::fd::IntoRawFd;
use std::sync::atomic::{AtomicI32, Ordering};

use serde_json::{Value, json};

struct Fake {
    modes: Vec<String>,
    log: Option<std::fs::File>,
    lines: std::io::Lines<std::io::StdinLock<'static>>,
    backlog: VecDeque<Value>,
    turns: u32,
    next_server_id: i64,
    thread: String,
    active_turn: Option<String>,
}

impl Fake {
    fn has(&self, mode: &str) -> bool {
        self.modes.iter().any(|m| m == mode)
    }

    fn emit(&self, v: Value) {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{v}").unwrap();
        out.flush().unwrap();
    }

    fn notify(&self, method: &str, params: Value) {
        self.emit(json!({ "method": method, "params": params }));
    }

    /// 下一条客户端消息（先取积压的）。EOF = 退出。
    fn next(&mut self) -> Value {
        match self.backlog.pop_front() {
            Some(v) => v,
            None => self.next_raw(),
        }
    }

    /// 发一条服务端请求并等它的回应（期间收到的别的消息放进积压）。
    fn ask(&mut self, id: Value, method: &str, params: Value) -> Value {
        self.emit(json!({ "id": id, "method": method, "params": params }));
        loop {
            let v = self.next_raw();
            if v.get("method").is_none() && v.get("id") == Some(&id) {
                return v;
            }
            self.backlog.push_back(v);
        }
    }

    fn next_raw(&mut self) -> Value {
        loop {
            let Some(Ok(line)) = self.lines.next() else {
                std::process::exit(0);
            };
            if let Some(f) = &mut self.log {
                writeln!(f, "{line}").unwrap();
            }
            if !line.trim().is_empty() {
                return serde_json::from_str(&line).expect("客户端发来的不是 JSON");
            }
        }
    }

    fn item(&self, turn: &str, item: Value) {
        self.notify(
            "item/completed",
            json!({ "threadId": self.thread, "turnId": turn, "completedAtMs": 0, "item": item }),
        );
    }

    fn usage(&self, turn: &str, context: i64, total: i64) {
        self.notify(
            "thread/tokenUsage/updated",
            json!({ "threadId": self.thread, "turnId": turn, "tokenUsage": {
                "last": { "totalTokens": context, "inputTokens": 0, "outputTokens": 0,
                          "cachedInputTokens": 0, "reasoningOutputTokens": 0 },
                "total": { "totalTokens": total, "inputTokens": 0, "outputTokens": 0,
                           "cachedInputTokens": 0, "reasoningOutputTokens": 0 },
                "modelContextWindow": 258_400 } }),
        );
    }

    fn complete(&mut self, turn: &str, status: &str, error: Value) {
        self.active_turn = None;
        self.notify(
            "turn/completed",
            json!({ "threadId": self.thread,
                    "turn": { "id": turn, "status": status, "items": [], "error": error } }),
        );
    }

    fn turn_start(&mut self, id: &Value, params: &Value) {
        if self.has("turn_hangs") {
            return;
        }
        self.turns += 1;
        let turn = format!("turn-{}", self.turns);
        let text = params
            .pointer("/input/0/text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.emit(json!({ "id": id, "result": { "turn": {
            "id": turn, "status": "inProgress", "items": [] } } }));
        self.notify(
            "turn/started",
            json!({ "threadId": self.thread, "turn": { "id": turn, "status": "inProgress", "items": [] } }),
        );
        self.active_turn = Some(turn.clone());
        self.item(
            &turn,
            json!({ "id": format!("{turn}-u"), "type": "userMessage",
                    "content": [{ "type": "text", "text": text, "text_elements": [] }] }),
        );
        if self.has("spawn_child") || self.has("spawn_child_inherit") {
            // `spawn_child_inherit`：工具子命令继承 codex 的 stdout / stderr（真 codex 的子命令就是这样），
            // codex 退出后管道仍被它握着、读不到 EOF。
            let sink = || {
                if self.has("spawn_child_inherit") {
                    std::process::Stdio::inherit()
                } else {
                    std::process::Stdio::null()
                }
            };
            let child = std::process::Command::new("/bin/sleep")
                .arg("600")
                .stdin(std::process::Stdio::null())
                .stdout(sink())
                .stderr(sink())
                .spawn()
                .unwrap();
            if let Ok(path) = std::env::var("FAKE_CODEX_CHILD_PID") {
                std::fs::write(path, child.id().to_string()).unwrap();
            }
            std::mem::forget(child);
        }
        if self.has("server_requests") {
            self.server_requests(&turn);
        }
        if self.has("huge_item") {
            self.item(
                &turn,
                json!({ "id": format!("{turn}-big"), "type": "agentMessage",
                        "text": "x".repeat(3 * 1024 * 1024) }),
            );
        }
        if self.has("slow_turn") {
            return;
        }
        self.item(
            &turn,
            json!({ "id": format!("{turn}-a"), "type": "agentMessage", "text": format!("GOT: {text}") }),
        );
        if self.has("usage_limit") {
            self.notify(
                "account/rateLimits/updated",
                json!({ "rateLimits": { "rateLimitReachedType": "rate_limit_reached" } }),
            );
            self.complete(
                &turn,
                "failed",
                json!({ "message": "You've hit your usage limit.", "codexErrorInfo": "usageLimitExceeded" }),
            );
            return;
        }
        let big = self.has("big_context") && self.turns == 1;
        self.complete(&turn, "completed", Value::Null);
        if big {
            self.usage(&turn, 190_000, 500_000);
        } else {
            self.usage(&turn, 20_000, 40_000);
        }
        if self.has("exit_after_turn") {
            std::process::exit(0);
        }
    }

    fn server_requests(&mut self, turn: &str) {
        let thread = self.thread.clone();
        let asks = [
            (
                json!(self.next_server_id),
                "item/commandExecution/requestApproval",
                json!({ "threadId": thread, "turnId": turn, "itemId": "c1", "command": "rm -rf /" }),
            ),
            (
                json!("elicit-tool"),
                "mcpServer/elicitation/request",
                json!({ "threadId": thread, "serverName": "crew", "mode": "form", "message": "Allow post_to_crew?",
                     "requestedSchema": { "type": "object", "properties": {} },
                     "_meta": { "codex_approval_kind": "mcp_tool_call" } }),
            ),
            (
                json!("elicit-form"),
                "mcpServer/elicitation/request",
                json!({ "threadId": thread, "serverName": "crew", "mode": "form", "message": "你叫什么？",
                     "requestedSchema": { "type": "object", "properties": {} } }),
            ),
            (
                json!(self.next_server_id + 1),
                "account/chatgptAuthTokens/refresh",
                json!({ "reason": "unauthorized" }),
            ),
            (
                json!(self.next_server_id + 2),
                "item/tool/requestUserInput",
                json!({ "threadId": thread, "prompt": "选一个" }),
            ),
            (
                json!(self.next_server_id + 3),
                "currentTime/read",
                json!({ "threadId": thread }),
            ),
        ];
        self.next_server_id += 4;
        let mut answered = 0;
        for (id, method, params) in asks {
            let reply = self.ask(id, method, params);
            if reply.get("result").is_some() || reply.get("error").is_some() {
                answered += 1;
            }
        }
        self.item(
            turn,
            json!({ "id": format!("{turn}-sr"), "type": "agentMessage", "text": format!("answered {answered}") }),
        );
    }

    fn serve(&mut self) {
        loop {
            let msg = self.next();
            let Some(method) = msg
                .get("method")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let id = msg.get("id").cloned();
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let Some(id) = id else {
                continue; // 通知（initialized）
            };
            match method.as_str() {
                "initialize" => {
                    if self.has("die_on_init") {
                        eprintln!("fake codex: not logged in");
                        std::process::exit(3);
                    }
                    if self.has("init_silent") {
                        continue;
                    }
                    if self.has("stderr_flood") {
                        let chunk = "x".repeat(64 * 1024);
                        let mut err = std::io::stderr().lock();
                        for _ in 0..16 {
                            err.write_all(chunk.as_bytes()).unwrap();
                        }
                        err.flush().unwrap();
                    }
                    self.emit(json!({ "id": id, "result": {
                        "userAgent": "fake_codex/0.0.0", "codexHome": "/nonexistent",
                        "platformFamily": "unix", "platformOs": "macos" } }));
                }
                "thread/start" => {
                    self.thread = format!("thr-{}", std::process::id());
                    self.emit(json!({ "id": id, "result": {
                        "thread": { "id": self.thread }, "model": "fake-model",
                        "reasoningEffort": "medium",
                        "serviceTier": params.get("serviceTier").cloned().unwrap_or(Value::Null),
                    } }));
                    if self.has("deaf") {
                        loop {
                            std::thread::sleep(std::time::Duration::from_secs(60));
                        }
                    }
                }
                "thread/resume" => {
                    let wanted = params
                        .get("threadId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if self.has("resume_fails") {
                        self.emit(json!({ "id": id, "error": {
                            "code": -32600, "message": format!("no rollout found for thread id {wanted}") } }));
                    } else {
                        self.thread = wanted;
                        self.emit(json!({ "id": id, "result": {
                            "thread": { "id": self.thread }, "model": "fake-model" } }));
                    }
                }
                "turn/start" => self.turn_start(&id, &params),
                "turn/interrupt" => {
                    self.emit(json!({ "id": id, "result": {} }));
                    if let Some(turn) = self.active_turn.clone() {
                        self.complete(&turn, "interrupted", Value::Null);
                    }
                }
                "thread/settings/update" => {
                    self.emit(json!({ "id": id, "result": {} }));
                    let tier = params
                        .get("serviceTier")
                        .cloned()
                        .unwrap_or(json!("default"));
                    self.notify(
                        "thread/settings/updated",
                        json!({ "threadId": self.thread, "threadSettings": {
                            "serviceTier": tier, "model": params.get("model").cloned().unwrap_or(json!("fake-model")) } }),
                    );
                }
                "thread/compact/start" => {
                    self.emit(json!({ "id": id, "result": {} }));
                    self.notify(
                        "turn/started",
                        json!({ "threadId": self.thread, "turn": { "id": "compact", "status": "inProgress", "items": [] } }),
                    );
                    self.item(
                        "compact",
                        json!({ "id": "compact-item", "type": "contextCompaction" }),
                    );
                    self.usage("compact", 28_000, 525_000);
                    // 压缩要一点时间：这期间投进来的正文必须排队（端到端测试靠它）。
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    self.complete("compact", "completed", Value::Null);
                }
                other => {
                    self.emit(json!({ "id": id, "error": {
                        "code": -32601, "message": format!("fake codex does not implement {other}") } }));
                }
            }
        }
    }
}

/// `count_term`：SIGTERM 处理函数往这个 fd 写一个字节（-1 = 没开）。
static TERM_LOG_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_term(_: libc::c_int) {
    let fd = TERM_LOG_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: write(2) 是 async-signal-safe 的；fd 在装处理函数之前就打开、从不关闭。
        unsafe {
            libc::write(fd, b"T".as_ptr().cast(), 1);
        }
    }
}

fn main() {
    let modes: Vec<String> = std::env::var("FAKE_CODEX_MODE")
        .unwrap_or_default()
        .split(',')
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .collect();
    if modes.iter().any(|m| m == "ignore_term") {
        // SAFETY: 进程启动时、还没有别的线程；只把 SIGTERM 设成忽略。
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
    }
    if modes.iter().any(|m| m == "count_term") {
        let path = std::env::var("FAKE_CODEX_TERM_LOG").expect("count_term 要 FAKE_CODEX_TERM_LOG");
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        TERM_LOG_FD.store(f.into_raw_fd(), Ordering::Relaxed);
        // SAFETY: 进程启动时、还没有别的线程；处理函数只调 async-signal-safe 的 write(2)。
        unsafe {
            libc::signal(libc::SIGTERM, on_term as *const () as libc::sighandler_t);
        }
    }
    if let Ok(path) = std::env::var("FAKE_CODEX_PID") {
        std::fs::write(path, std::process::id().to_string()).unwrap();
    }
    if let Ok(path) = std::env::var("FAKE_CODEX_ENV_LOG") {
        let mut names: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
        names.sort();
        std::fs::write(path, names.join("\n")).unwrap();
    }
    let log = std::env::var("FAKE_CODEX_LOG").ok().map(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .unwrap()
    });
    let mut fake = Fake {
        modes,
        log,
        lines: std::io::stdin().lock().lines(),
        backlog: VecDeque::new(),
        turns: 0,
        next_server_id: 100,
        thread: String::new(),
        active_turn: None,
    };
    fake.serve();
}
