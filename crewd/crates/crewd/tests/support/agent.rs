//! Consumer-side harness: execute helper commands read from generated launch files.
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crewd_exec::launch::{AgentSessionArg, ClaudeSpec, CommsSpec, LaunchFiles, prepare_claude};
use crewd_server::agent::testkit::Fixture;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

pub struct Launch {
    pub _files: LaunchFiles,
    pub mcp: Value,
    pub settings: Value,
    pub world: Option<String>,
}

pub fn launch(f: &Fixture, session: &str, runtime: &Path) -> Launch {
    launch_with_world(f, session, runtime, None)
}

pub fn launch_with_world(
    f: &Fixture,
    session: &str,
    runtime: &Path,
    world: Option<String>,
) -> Launch {
    let spec = ClaudeSpec {
        system_prompt: world,
        comms: Some(CommsSpec {
            helper: env!("CARGO_BIN_EXE_crewd").into(),
            data_dir: f.data_dir().into(),
            session_id: session.into(),
            token: f.token(session),
            hooks: true,
        }),
        ..ClaudeSpec::default()
    };
    let (args, files) = prepare_claude(
        &spec,
        AgentSessionArg::New(format!("e2e-{session}")),
        runtime,
    )
    .unwrap();
    let read = |p| serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
    Launch {
        _files: files,
        mcp: read(args.mcp_config_file.unwrap()),
        settings: read(args.settings_file.unwrap()),
        world: args
            .system_prompt_file
            .map(|p| std::fs::read_to_string(p).unwrap()),
    }
}

// Only the documented exact-value list is emitted by this product. Refuse other shapes instead
// of silently approximating regexes and hiding an incorrect matcher.
pub fn matches(entry: &Value, value: &str) -> bool {
    let Some(m) = entry["matcher"].as_str() else {
        return true;
    };
    assert!(
        m.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_|,-".contains(c)),
        "unsupported matcher {m}"
    );
    m.split(['|', ',']).any(|part| part == value)
}

pub async fn fire(settings: &Value, event: &str, payload: Value) -> Option<Value> {
    fire_with_home(settings, event, payload, None).await
}

/// Exact generated helper command, with a test-owned environment (no login shell/profile).
// This shared support module is also compiled by suites using only the original helpers.
#[allow(dead_code)]
pub async fn fire_isolated(
    settings: &Value,
    event: &str,
    payload: Value,
    home: &Path,
) -> Option<Value> {
    fire_with_home(settings, event, payload, Some(home)).await
}

async fn fire_with_home(
    settings: &Value,
    event: &str,
    payload: Value,
    home: Option<&Path>,
) -> Option<Value> {
    let entries = settings["hooks"][event].as_array()?;
    let key = if event == "SessionStart" {
        "source"
    } else {
        "tool_name"
    };
    let mut result = None;
    for entry in entries {
        if !matches(entry, payload[key].as_str().unwrap_or("")) {
            continue;
        }
        for hook in entry["hooks"].as_array().unwrap() {
            assert_eq!(hook["type"], "command");
            let mut command = Command::new(if home.is_some() { "/bin/sh" } else { "sh" });
            if let Some(home) = home {
                command
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                    .env("HOME", home)
                    .env("XDG_CACHE_HOME", home.join("cache"))
                    .env("CLAUDE_CONFIG_DIR", home.join("claude-config"))
                    .env("SHELL", "/usr/bin/false");
            }
            let mut child = command
                .args(["-c", hook["command"].as_str().unwrap()])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let mut input = child.stdin.take().unwrap();
            input
                .write_all(payload.to_string().as_bytes())
                .await
                .unwrap();
            drop(input);
            let out = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
                .await
                .unwrap()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let text = String::from_utf8(out.stdout).unwrap();
            let text = text.trim();
            if text.starts_with('{') {
                result = Some(serde_json::from_str(text).unwrap());
            } else {
                assert!(text.is_empty(), "helper stdout was not JSON: {text}");
            }
        }
    }
    result
}

pub struct Mcp {
    child: Child,
    input: ChildStdin,
    lines: tokio::io::Lines<BufReader<ChildStdout>>,
    next: u64,
    pub init: Value,
    pending: Vec<Value>,
}

/// Owned MCP helper cleanup observation; no process identifiers or content.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Shared fixture: only callers requiring observed normal EOF use this receipt.
pub struct McpCloseReceipt {
    pub normal: bool,
    pub exit_code: Option<i32>,
    pub forced_kill: bool,
}

impl Mcp {
    pub async fn start(cfg: &Value) -> Self {
        Self::start_with_home(cfg, None).await
    }
    // Other integration suites include this module without using the isolated entry point.
    #[allow(dead_code)]
    pub async fn start_isolated(cfg: &Value, home: &Path) -> Self {
        Self::start_with_home(cfg, Some(home)).await
    }
    async fn start_with_home(cfg: &Value, home: Option<&Path>) -> Self {
        let crew = &cfg["mcpServers"]["crew"];
        assert_eq!(crew["type"], "stdio");
        let mut command = Command::new(crew["command"].as_str().unwrap());
        if let Some(home) = home {
            command
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("HOME", home)
                .env("XDG_CACHE_HOME", home.join("cache"))
                .env("CLAUDE_CONFIG_DIR", home.join("claude-config"))
                .env("SHELL", "/usr/bin/false");
        }
        let mut child = command
            .args(
                crew["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| a.as_str().unwrap()),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut m = Self {
            child,
            input,
            lines,
            next: 1,
            init: Value::Null,
            pending: vec![],
        };
        m.init = m.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "synthetic-e2e", "version": "0"}})).await;
        m.write(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        m
    }
    async fn write(&mut self, v: Value) {
        self.input
            .write_all(format!("{v}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("MCP exited before replying");
        serde_json::from_str(&line).unwrap()
    }
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.write(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let v = self.recv().await;
            if v["id"] == id {
                assert!(v.get("error").is_none(), "{v}");
                return v;
            }
            self.pending.push(v);
        }
    }
    pub async fn changed(&mut self) {
        loop {
            let v = if self.pending.is_empty() {
                self.recv().await
            } else {
                self.pending.remove(0)
            };
            if v["method"] == "notifications/tools/list_changed" {
                return;
            }
        }
    }
    pub async fn tools(&mut self) -> Vec<Value> {
        self.request("tools/list", json!({})).await["result"]["tools"]
            .as_array()
            .unwrap()
            .clone()
    }
    pub async fn call(&mut self, name: &str, args: Value) -> (String, bool) {
        let v = self
            .request("tools/call", json!({"name": name, "arguments": args}))
            .await;
        (
            v["result"]["content"][0]["text"].as_str().unwrap().into(),
            v["result"]["isError"].as_bool().unwrap_or(false),
        )
    }
    /// Close stdin, observe the actual exit, and reap only this owned child on timeout.
    #[allow(dead_code)] // Other integration binaries retain the existing close helper.
    pub async fn close_observed(mut self) -> McpCloseReceipt {
        drop(self.input);
        let (status, forced_kill) =
            match tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await {
                Ok(status) => (status.expect("owned MCP helper wait"), false),
                Err(_) => {
                    self.child.kill().await.expect("kill only owned MCP helper");
                    (
                        self.child.wait().await.expect("reap owned MCP helper"),
                        true,
                    )
                }
            };
        McpCloseReceipt {
            normal: !forced_kill && status.success(),
            exit_code: status.code(),
            forced_kill,
        }
    }
    pub async fn close(mut self) {
        drop(self.input);
        if tokio::time::timeout(Duration::from_secs(3), self.child.wait())
            .await
            .is_err()
        {
            self.child.kill().await.unwrap();
            self.child.wait().await.unwrap();
        }
    }
}
