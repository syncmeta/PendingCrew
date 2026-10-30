//! C13 real binary/library fixture. No independently minted configs or live provider.
#![allow(dead_code)]
use crate::{agent, t11};
use crewd_exec::registry::{ProbeResult, ProcessIdentity, probe};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    process::{Child, Command},
    sync::oneshot,
    task::JoinHandle,
};
const LIMIT: Duration = Duration::from_secs(15);
pub struct Rig {
    pub f: t11::Fixture,
    pub client: t11::Client,
    child: Option<Child>,
    join: Option<JoinHandle<anyhow::Result<()>>>,
    shutdown: Option<oneshot::Sender<()>>,
    library: bool,
    sessions: Vec<String>,
    closed: bool,
}
impl Drop for Rig {
    fn drop(&mut self) {
        if !self.closed {
            eprintln!("C13 cleanup FALLBACK after incomplete caller; never normal proof");
            if let Some(j) = &self.join {
                j.abort();
            }
        }
    }
}
impl Rig {
    pub async fn literal() -> Self {
        Self::new(None).await
    }
    pub async fn library(hooks: crewd_server::orchestra::hooks::Hooks) -> Self {
        Self::new(Some(hooks)).await
    }
    async fn new(hooks: Option<crewd_server::orchestra::hooks::Hooks>) -> Self {
        let f = t11::Fixture::new();
        let gate = f.root.path().join("busy");
        std::fs::create_dir_all(&gate).unwrap();
        let witness = f.root.path().join("witness");
        let (child, join, shutdown, library) = if let Some(hooks) = hooks {
            let mut opts = f.options("ready");
            let o = opts.orchestra.as_mut().unwrap();
            o.claude_env
                .insert("FAKE_CLAUDE_FULL_INPUT".into(), "1".into());
            o.claude_env
                .insert("FAKE_CLAUDE_C13_GATE".into(), gate.display().to_string());
            o.claude_env.insert(
                "FAKE_CLAUDE_C13_WITNESS".into(),
                witness.display().to_string(),
            );
            let data = f.data.clone();
            let (tx, rx) = oneshot::channel();
            let j = tokio::spawn(crewd_server::serve_with_test_hooks(
                data,
                opts,
                hooks,
                async {
                    let _ = rx.await;
                },
            ));
            (None, Some(j), Some(tx), true)
        } else {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_crewd"));
            cmd.env_clear()
                .env("PATH", t11::CHILD_PATH)
                .env("HOME", &f.home)
                .env("XDG_CACHE_HOME", &f.cache)
                .env("CLAUDE_CONFIG_DIR", &f.config)
                .env("TMPDIR", f.root.path())
                .env("SHELL", "/usr/bin/false")
                .env("CREWD_DATA_DIR", &f.data)
                .env("CREWD_CLAUDE", t11::artifact("fake_claude"))
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .arg("serve")
                .arg("--claude-env")
                .arg("FAKE_CLAUDE_FULL_INPUT=1")
                .arg("--claude-env")
                .arg("FAKE_CLAUDE_VERSION=2.1.301 (Claude Code)");
            for (key, value) in [
                ("FAKE_CLAUDE_LOG", f.log.clone()),
                ("FAKE_CLAUDE_C13_GATE", gate),
                ("FAKE_CLAUDE_C13_WITNESS", witness),
            ] {
                cmd.arg("--claude-env")
                    .arg(format!("{key}={}", value.display()));
            }
            cmd.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(f.root.path().join("serve.log")).unwrap())
                .kill_on_drop(true);
            (Some(cmd.spawn().unwrap()), None, None, false)
        };
        let mut client = t11::Client::connect(&f.data).await;
        client.call("system.hello", Value::Null).await.unwrap();
        Self {
            f,
            client,
            child,
            join,
            shutdown,
            library,
            sessions: vec![],
            closed: false,
        }
    }
    pub fn records(&self) -> Vec<Value> {
        let dir = self.f.root.path().join("witness");
        let mut records = vec![];
        let Ok(entries) = std::fs::read_dir(dir) else {
            return records;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let bytes = std::fs::read(path).unwrap();
            // Per-generation writer, and only completed records: no cross-session or partial parse.
            let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            records.extend(
                bytes[..end]
                    .split(|b| *b == b'\n')
                    .filter(|l| !l.is_empty())
                    .map(|l| serde_json::from_slice::<Value>(l).expect("complete fixture JSONL")),
            );
        }
        records
    }
    pub fn count(&self, sid: &str, marker: &str) -> usize {
        self.records()
            .iter()
            .filter(|v| {
                v["event"] == "submit"
                    && v["session_id"] == sid
                    && v["body"].as_str().is_some_and(|s| s.contains(marker))
            })
            .count()
    }
    /// Mandatory semantic body observation: same 15s bound; absence is data until cleanup.
    pub async fn body_observed(&mut self, sid: &str, marker: &str) -> bool {
        let seen = tokio::time::timeout(LIMIT, async {
            while self.count(sid, marker) == 0 {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .is_ok();
        if seen {
            self.working(sid, false).await;
        }
        seen
    }
    pub async fn body(&mut self, sid: &str, marker: &str) {
        let stage = format!(
            "actual per-session submitted body library={} session={sid} marker={marker}",
            self.library
        );
        eprintln!("C13 body observation start: {stage}");
        t11::wait(&stage, || self.count(sid, marker) > 0).await;
        eprintln!(
            "C13 body observed: {stage} count={}",
            self.count(sid, marker)
        );
        self.working(sid, false).await;
    }
    pub async fn pair(&mut self) -> (String, String, String) {
        let v = self
            .client
            .call(
                "crew.create",
                json!({"title":"C13 synthetic","workingDirectory":self.f.work}),
            )
            .await
            .unwrap();
        let cid = v["id"].as_str().unwrap().to_owned();
        let mut ids = vec![];
        for (role, brief) in [("captain", "C13_INIT_CAP"), ("worker", "C13_INIT_WORKER")] {
            let s=self.client.call("session.start",json!({"crewId":cid,"role":role,"executor":"claude","brief":brief,"isolation":false})).await.unwrap();
            let id = s["id"].as_str().unwrap().to_owned();
            self.sessions.push(id.clone());
            self.body(&id, brief).await;
            ids.push(id);
        }
        // Empty initial briefs carry no message cause. Establish the captain's basis
        // through actual Human RPC -> Hub/actor -> PTY submit -> real Delivered receipt.
        // This setup marker is distinct from every six-case semantic marker below.
        let cap = ids[0].clone();
        let lobby = format!("{cid}/lobby");
        let marker = "C13_SETUP_HUMAN_CAUSAL_CAPTAIN";
        let input = self
            .client
            .call(
                "message.post",
                json!({"channelId":lobby,"text":marker,
   "mentions":[{"kind":"session","targetId":cap}]}),
            )
            .await
            .unwrap();
        assert_eq!(
            input["senderKind"], "human",
            "actual Human entry, never Session/free-brief surrogate"
        );
        assert_eq!(input["channelId"], lobby);
        assert_eq!(input["text"], marker);
        self.body(&cap, marker).await;
        let confirmed = self.delivered(input["id"].as_str().unwrap(), &cap).await;
        assert!(
            confirmed
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["sessionId"] == cap && v["state"] == "delivered"),
            "S42 C13 setup exact Human input has actual successful output receipt"
        );
        assert_eq!(
            self.count(&cap, marker),
            1,
            "actual Human setup submitted once before Session posts"
        );
        (cid, ids.remove(0), ids.remove(0))
    }
    pub async fn working(&mut self, sid: &str, want: bool) {
        tokio::time::timeout(LIMIT, async {
            loop {
                let v = self
                    .client
                    .call("session.live", json!({"sessionId":sid}))
                    .await
                    .unwrap();
                if v["running"] == true && v["working"] == want {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("actual driver working transition");
    }
    pub fn runtime(&self) -> PathBuf {
        if self.library {
            return self.f.runtime.clone();
        }
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.f.data.canonicalize().unwrap().hash(&mut h);
        let cache = if cfg!(target_os = "macos") {
            self.f.home.join("Library/Caches")
        } else {
            self.f.cache.clone()
        };
        cache
            .join("PendingCrew/run")
            .join(format!("{:016x}", h.finish()))
    }
    pub fn generated(&self, sid: &str) -> Generated {
        let root = self.runtime().canonicalize().unwrap();
        let own = self.f.root.path().canonicalize().unwrap();
        assert!(root.starts_with(&own) && !root.starts_with(self.f.work.canonicalize().unwrap()));
        assert_eq!(root.metadata().unwrap().permissions().mode() & 0o777, 0o700);
        let identities: Vec<_> = t11::registry(&self.f.data)
            .into_iter()
            .filter(|(s, _)| s == sid)
            .collect();
        assert_eq!(identities.len(), 1);
        let identity = identities[0].1.clone();
        assert!(matches!(probe(identity.pid),ProbeResult::Found(ref actual)if actual==&identity));
        let launches: Vec<_> = self
            .records()
            .into_iter()
            .filter(|v| {
                v["event"] == "launch" && v["session_id"] == sid && v["pid"] == identity.pid
            })
            .collect();
        assert_eq!(launches.len(), 1, "actual live generation");
        let launch = &launches[0];
        let path = PathBuf::from(launch["mcp"].as_str().unwrap())
            .canonicalize()
            .unwrap();
        let dir = path.parent().unwrap().to_owned();
        assert!(dir.starts_with(&root));
        assert_eq!(dir.metadata().unwrap().permissions().mode() & 0o777, 0o700);
        let mcp: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let crew = &mcp["mcpServers"]["crew"];
        let args: Vec<_> = crew["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(args.windows(2).any(|w| w == ["--session", sid]));
        assert!(args.windows(2).any(|w| w[0] == "--data-dir"
            && Path::new(w[1]).canonicalize().unwrap() == self.f.data.canonicalize().unwrap()));
        assert_eq!(
            Path::new(crew["command"].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            t11::artifact("crewd").canonicalize().unwrap()
        );
        let token = PathBuf::from(args.windows(2).find(|w| w[0] == "--token-file").unwrap()[1])
            .canonicalize()
            .unwrap();
        assert!(token.starts_with(&dir));
        for p in [&path, &dir.join("settings.json"), &token] {
            assert_eq!(
                p.metadata().unwrap().permissions().mode() & 0o777,
                0o600,
                "generated private launch file"
            );
        }
        assert!(!std::fs::read(&token).unwrap().is_empty()); // never print or fabricate token
        let settings =
            serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
        let gate = PathBuf::from(launch["gate"].as_str().unwrap())
            .canonicalize()
            .unwrap();
        assert!(gate.starts_with(own.join("busy")));
        Generated {
            dir,
            mcp,
            settings,
            gate,
            identity,
            generation: launch["generation"].as_str().unwrap().to_owned(),
            agent_id: launch["agent_id"].as_str().unwrap().to_owned(),
            resumed: launch["resumed"].as_bool().unwrap(),
        }
    }
    pub async fn message(&mut self, cid: &str, marker: &str) -> Value {
        tokio::time::timeout(LIMIT, async {
            loop {
                let v = self
                    .client
                    .call("message.list", json!({"channelId":cid,"limit":1000}))
                    .await
                    .unwrap();
                if let Some(m) = v
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|m| m["text"].as_str().is_some_and(|s| s.contains(marker)))
                {
                    return m.clone();
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("durable actual message")
    }
    pub async fn receipts(&mut self, id: &str) -> Value {
        self.client
            .call("message.receipts", json!({"messageId":id}))
            .await
            .unwrap()
    }
    pub async fn delivered(&mut self, id: &str, sid: &str) -> Value {
        tokio::time::timeout(LIMIT, async {
            loop {
                let v = self.receipts(id).await;
                if v.as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["sessionId"] == sid && r["state"] == "delivered")
                {
                    return v;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("actual durable delivered receipt")
    }
    pub async fn topic(&mut self, cid: &str, cap: &str, worker: &str) -> String {
        self.client
            .call(
                "channel.create",
                json!({"crewId":cid,"kind":"topic","name":"c13_other",
   "members":[{"kind":"session","id":cap},{"kind":"session","id":worker}]}),
            )
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    pub async fn close(&mut self) -> bool {
        // Release every current owned generation before stopping; never a guessed PID/group.
        for v in self
            .records()
            .into_iter()
            .filter(|v| v["event"] == "launch")
        {
            let gate = PathBuf::from(v["gate"].as_str().unwrap());
            if gate.is_dir() {
                std::fs::write(gate.join("release"), []).unwrap();
            }
        }
        for sid in self.sessions.clone() {
            self.client
                .call(
                    "session.stop",
                    json!({"sessionId":sid,"reason":"C13 synthetic normal cleanup"}),
                )
                .await
                .unwrap();
            self.client.stopped(&sid).await;
        }
        let normal = if let Some(child) = self.child.as_mut() {
            let pid = child.id().unwrap();
            let signaled = Command::new("/bin/kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .await
                .unwrap()
                .success();
            let status = tokio::time::timeout(LIMIT, child.wait())
                .await
                .expect("normal owned serve exit")
                .unwrap();
            signaled && status.success()
        } else {
            self.shutdown.take().unwrap().send(()).unwrap();
            tokio::time::timeout(LIMIT, self.join.as_mut().unwrap())
                .await
                .expect("normal library serve join")
                .unwrap()
                .is_ok()
        };
        self.closed = true;
        normal && !self.f.data.join("crewd.sock").exists() && t11::registry(&self.f.data).is_empty()
    }
    pub fn store(&self) -> crewd_store::Store {
        assert!(self.closed);
        crewd_store::Store::open_data_dir(&self.f.data).unwrap()
    }
    pub fn cursor(&self, sid: &str, channel: &str) -> Option<i64> {
        let s = self.store();
        crewd_collab::cursors::cursor(s.conn(), sid, channel).unwrap()
    }
}
pub struct Generated {
    pub dir: PathBuf,
    pub mcp: Value,
    pub settings: Value,
    pub gate: PathBuf,
    pub identity: ProcessIdentity,
    pub generation: String,
    pub agent_id: String,
    pub resumed: bool,
}
impl Generated {
    pub async fn mcp(&self, f: &t11::Fixture) -> agent::Mcp {
        agent::Mcp::start_isolated(&self.mcp, &f.home).await
    }
    pub async fn arm(&self, r: &mut Rig, sid: &str) {
        std::fs::write(self.gate.join("arm"), []).unwrap();
        let session = r
            .client
            .call("session.get", json!({"sessionId":sid}))
            .await
            .unwrap();
        r.client.call("message.post", json!({"channelId":format!("{}/lobby", session["crewId"].as_str().unwrap()),"text":"C13_CURRENT_HUMAN_BUSY_TASK","mentions":[{"kind":"session","targetId":sid}]})).await.unwrap();
        t11::wait("actual managed task callback receipt", || {
            self.gate.join("busy-written").is_file()
        })
        .await;
        r.working(sid, true).await;
    }
    pub async fn release(&self, r: &mut Rig, sid: &str) {
        std::fs::write(self.gate.join("release"), []).unwrap();
        t11::wait("actual release frame receipt", || {
            self.gate.join("released").is_file()
        })
        .await;
        r.working(sid, false).await;
    }
    pub async fn hook(&self, f: &t11::Fixture, event: &str) -> Value {
        agent::fire_isolated(
            &self.settings,
            event,
            json!({"hook_event_name":event,"session_id":self.agent_id,
   "prompt_id":std::fs::read_to_string(self.gate.join("managed-prompt")).unwrap(),
   "tool_name":"mcp__crew__read_channel","tool_input":{},
   "tool_response":{"synthetic":true},"error":"synthetic failure"}),
            &f.home,
        )
        .await
        .unwrap_or(Value::Null)
    }
}
pub async fn post(m: &mut agent::Mcp, channel: &str, target: Option<&str>, marker: &str) {
    let mentions = target.map_or(
        json!([{"kind":"human"}]),
        |id| json!([{"kind":"session","target_id":id}]),
    );
    let (_, error) = m
        .call(
            "post",
            json!({"channel":channel,"message":marker,"category":"question","mentions":mentions}),
        )
        .await;
    assert!(!error, "actual generated MCP attached tool success");
}
pub fn context(v: &Value) -> &str {
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or("")
}
pub fn normal_mcp(close: agent::McpCloseReceipt) -> bool {
    close.normal && !close.forced_kill && close.exit_code == Some(0)
}
