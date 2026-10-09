//! Test-owned bounded Claude CLI stream. No transcript reader and no real application data.
#![allow(dead_code)] // Shared by smoke and eval binaries, which use different observations.
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use crewd_collab::{Locale, worldview};
use crewd_exec::env::{AgentPaths, build_env_from_process};
use crewd_exec::launch::{
    AgentSessionArg, ClaudeSpec, CommsSpec, LaunchFiles, new_agent_session_id, prepare_claude,
};
use crewd_server::agent::testkit::Fixture;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

pub fn authorized() {
    assert_eq!(
        std::env::var("CREWD_LIVE_CLAUDE").as_deref(),
        Ok("1"),
        "real Claude requires both --ignored --exact and CREWD_LIVE_CLAUDE=1"
    );
}

#[derive(Default, Debug)]
pub struct Trace {
    pub calls: Vec<(String, String)>, // id, name, in emitted assistant order
    pub results: Vec<Value>,
    pub init: Option<Value>,
    tool_results: Vec<Value>,
    seen: HashSet<String>,
}

impl Trace {
    pub fn observe(&mut self, v: &Value) -> Result<(), String> {
        match v["type"].as_str() {
            Some("system") if v["subtype"] == "init" => self.init = Some(v.clone()),
            Some("system")
                if v["subtype"] == "permission_denied" || v["subtype"] == "api_retry" =>
            {
                return Err(format!(
                    "stop at {}: {}",
                    v["subtype"],
                    v.get("error").unwrap_or(&Value::Null)
                ));
            }
            Some("assistant") | Some("user") => {
                if v["parent_tool_use_id"].is_string() {
                    return Err("unexpected child agent in authorized test".into());
                }
                if let Some(blocks) = v["message"]["content"].as_array() {
                    for b in blocks {
                        match b["type"].as_str() {
                            Some("tool_use") => {
                                let id = b["id"].as_str().ok_or("tool_use has no id")?;
                                let name = b["name"].as_str().ok_or("tool_use has no name")?;
                                if self.seen.insert(id.into()) {
                                    self.calls.push((id.into(), name.into()));
                                }
                            }
                            Some("tool_result") => self.tool_results.push(b.clone()),
                            _ => {}
                        }
                    }
                }
            }
            Some("result") => {
                self.results.push(v.clone());
                if v["is_error"] == true {
                    return Err(format!("CLI result failure: {}", v["subtype"]));
                }
                if v["permission_denials"]
                    .as_array()
                    .is_some_and(|a| !a.is_empty())
                {
                    return Err("CLI result contains permission denials; do not retry".into());
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Independent complete assistant/tool stream, not a zero hook counter. A successful terminal
    /// result and every emitted call's matching result establish that the captured turn completed.
    pub fn no_search_before_first_crew_call(&self) -> Result<(), String> {
        if self.init.is_none() || self.results.is_empty() {
            return Err("incomplete structured trace".into());
        }
        for (id, _) in &self.calls {
            if !self.tool_results.iter().any(|r| r["tool_use_id"] == *id) {
                return Err(format!("call {id} lacks its tool_result"));
            }
        }
        let first = self
            .calls
            .iter()
            .position(|(_, n)| n.starts_with("mcp__crew__"))
            .ok_or("no crew tool call")?;
        if self.calls[..first].iter().any(|(_, n)| n == "ToolSearch") {
            return Err("ToolSearch preceded the first crew call".into());
        }
        Ok(())
    }

    pub fn succeeded(&self, name: &str) -> bool {
        self.calls.iter().filter(|(_, n)| n == name).any(|(id, _)| {
            self.tool_results
                .iter()
                .any(|r| r["tool_use_id"] == *id && r["is_error"] != true)
        })
    }

    pub fn ask_user_question_absent(&self) -> Option<bool> {
        self.init
            .as_ref()?
            .get("tools")?
            .as_array()
            .map(|a| !a.iter().any(|x| x == "AskUserQuestion"))
    }
}

pub struct Claude {
    child: Child,
    input: Option<ChildStdin>,
    lines: tokio::io::Lines<BufReader<ChildStdout>>,
    log: std::fs::File,
    _workdir: tempfile::TempDir,
    _runtime: tempfile::TempDir,
    _files: LaunchFiles,
    pub trace: Trace,
    started: Instant,
    pub version: String,
    pub install_type: &'static str,
}

impl Claude {
    pub async fn start(f: &Fixture, label: &str) -> Result<Self, String> {
        authorized();
        // Logs and runtime are confined to this task's own worktree; no HOME fixture/transcript.
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()
            .map_err(|e| e.to_string())?
            .join(".superpowers/sdd/2026-10-06-crewd-m3b-agent-surface/scratch-t16");
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let workdir = tempfile::Builder::new()
            .prefix("synthetic-live-")
            .tempdir_in(&root)
            .map_err(|e| e.to_string())?;
        let runtime = tempfile::Builder::new()
            .prefix("live-launch-")
            .tempdir_in(&root)
            .map_err(|e| e.to_string())?;
        let paths = AgentPaths::discover().await;
        let program = paths
            .resolve("claude")
            .ok_or("no Claude on discovered PATH")?;
        let canonical = program.canonicalize().unwrap_or_else(|_| program.clone());
        let spelling = canonical.to_string_lossy();
        let install_type = if spelling.contains("Caskroom") {
            "Homebrew"
        } else if spelling.contains("node_modules") {
            "npm"
        } else if spelling.contains("claude/versions") {
            "native"
        } else {
            "unknown"
        };
        let env = build_env_from_process(&paths.child_path, &Default::default());
        let version_out = Command::new(&program)
            .arg("--version")
            .env_clear()
            .envs(&env)
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| e.to_string())?;
        if !version_out.status.success() {
            return Err("Claude --version failed".into());
        }
        let version = String::from_utf8_lossy(&version_out.stdout)
            .trim()
            .to_string();
        let worker = f.worker.clone();
        let world = f
            .state
            .db
            .call(move |s| worldview::context_for(s.conn(), &worker, Locale::Zh))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let spec = ClaudeSpec {
            workdir: workdir.path().into(),
            system_prompt: Some(worldview::render(&world)),
            comms: Some(CommsSpec {
                helper: env!("CARGO_BIN_EXE_crewd").into(),
                data_dir: f.data_dir().into(),
                session_id: f.worker.clone(),
                token: f.token(&f.worker),
                hooks: true,
            }),
            ..ClaudeSpec::default()
        };
        let (args, files) = prepare_claude(
            &spec,
            AgentSessionArg::New(new_agent_session_id()),
            runtime.path(),
        )
        .map_err(|e| e.to_string())?;
        let mut child = Command::new(&program)
            .args(args.argv())
            .args([
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--no-session-persistence",
                "--strict-mcp-config",
                "--restricted",
                "--tools",
                "Bash,ToolSearch,AskUserQuestion",
                "--disable-slash-commands",
                "--max-turns",
                "8",
                "--max-budget-usd",
                "2",
            ])
            .current_dir(workdir.path())
            .env_clear()
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(
                std::fs::File::create(root.join(format!("{label}-stderr.log")))
                    .map_err(|e| e.to_string())?,
            ))
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| e.to_string())?;
        let input = child.stdin.take();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let log = std::fs::File::create(root.join(format!("{label}-stream.jsonl")))
            .map_err(|e| e.to_string())?;
        eprintln!(
            "LIVE {label}: {version}, install={install_type}, max_turns=8 per prompt, deadline=240s per prompt, no retries, quota not measured; stable is not a version lock"
        );
        Ok(Self {
            child,
            input,
            lines,
            log,
            _workdir: workdir,
            _runtime: runtime,
            _files: files,
            trace: Trace::default(),
            started: Instant::now(),
            version,
            install_type,
        })
    }

    pub async fn prompt(&mut self, text: &str) -> Result<Value, String> {
        let message = json!({"type": "user", "message": {"role": "user", "content": text}, "parent_tool_use_id": null});
        self.input
            .as_mut()
            .ok_or("stdin closed")?
            .write_all(format!("{message}\n").as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let until = tokio::time::Instant::now() + Duration::from_secs(240);
        loop {
            let line = tokio::time::timeout_at(until, self.lines.next_line())
                .await
                .map_err(|_| "authorized prompt hit 240s deadline")?
                .map_err(|e| e.to_string())?
                .ok_or("Claude exited before a result; see test-owned stderr")?;
            writeln!(self.log, "{line}").map_err(|e| e.to_string())?;
            let event: Value = serde_json::from_str(&line)
                .map_err(|e| format!("incomplete/non-JSON tool stream: {e}"))?;
            self.trace.observe(&event)?;
            if event["type"] == "result" {
                return Ok(event);
            }
        }
    }

    /// Cleanup is performed before assertions, including denied/error/timeout paths. Only this
    /// exact Child is signalled; helpers receive EOF when their test-owned parent closes.
    pub async fn finish(mut self) -> Trace {
        self.input.take();
        if tokio::time::timeout(Duration::from_secs(15), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
        eprintln!(
            "LIVE finished: wall_ms={}, version={}, install={}, calls={:?}, results={}",
            self.started.elapsed().as_millis(),
            self.version,
            self.install_type,
            self.trace.calls,
            self.trace.results.len()
        );
        self.trace
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn completed(names: &[&str]) -> Trace {
        let mut t = Trace::default();
        t.observe(&json!({"type": "system", "subtype": "init", "tools": ["ToolSearch", "mcp__crew__post"]})).unwrap();
        for (i, name) in names.iter().enumerate() {
            let id = format!("synthetic-{i}");
            t.observe(&json!({"type": "assistant", "parent_tool_use_id": null, "message": {"content": [{"type": "tool_use", "id": id, "name": name, "input": {}}]}})).unwrap();
            t.observe(&json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": id, "content": "ok"}]}})).unwrap();
        }
        t.observe(&json!({"type": "result", "is_error": false, "permission_denials": []}))
            .unwrap();
        t
    }
    #[test]
    fn structured_trace_distinguishes_search_order_and_incomplete_capture() {
        assert!(
            completed(&["ToolSearch", "mcp__crew__post"])
                .no_search_before_first_crew_call()
                .is_err()
        );
        assert!(
            completed(&["mcp__crew__post", "ToolSearch"])
                .no_search_before_first_crew_call()
                .is_ok()
        );
        let mut t = completed(&["mcp__crew__post"]);
        t.tool_results.clear();
        assert!(t.no_search_before_first_crew_call().is_err());
        assert!(Trace::default().no_search_before_first_crew_call().is_err());
    }
    #[test]
    fn structured_trace_stops_at_policy_or_auth_failure() {
        for v in [
            json!({"type": "system", "subtype": "permission_denied"}),
            json!({"type": "system", "subtype": "api_retry", "error": "authentication_failed"}),
            json!({"type": "result", "is_error": false, "permission_denials": [{}]}),
        ] {
            assert!(Trace::default().observe(&v).is_err());
        }
    }
}
