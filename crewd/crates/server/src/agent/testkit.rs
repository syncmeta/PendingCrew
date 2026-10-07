//! 测试夹具：临时数据目录里的真库（M3a 的 Db + 当前 schema）+ FakeSessionControl。
//! `crewd` 的集成测试经 feature `testkit` 用它；仓库公开，只造合成数据。
//! 不进产品代码（字面量扫描器不扫它）。
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crewd_collab::Change;
use crewd_proto::{Mention, Message};
use serde_json::json;

use crate::AppState;
use crate::agent::port::{self, PostReq, PostSender};
use crate::agent::runtime::{AgentConfig, AgentRuntime};
use crate::agent::{Identity, Role};
use crate::control::FakeSessionControl;

pub struct Fixture {
    pub state: Arc<AppState>,
    pub crew: String,
    pub captain: String,
    pub worker: String,
    dir: tempfile::TempDir,
    control: Arc<FakeSessionControl>,
}

impl Fixture {
    pub async fn new() -> Self {
        Self::with_config(AgentConfig::default()).await
    }

    pub async fn with_config(config: AgentConfig) -> Self {
        Self::with_runtime(AgentRuntime::new(config)).await
    }

    /// A fixture with a prepared runtime (e.g. a post gate on a clock the test moves).
    pub async fn with_runtime(runtime: AgentRuntime) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let control = Arc::new(FakeSessionControl::default());
        let store = crewd_store::Store::open(&dir.path().join("crewd.db")).unwrap();
        let db = crewd_store::Db::spawn(store).unwrap();
        let mut state = AppState::new(db).with_sessions(control.clone());
        state.agent = runtime;
        let mut f = Self {
            state: Arc::new(state),
            crew: String::new(),
            captain: "s-cap".into(),
            worker: "s-w1".into(),
            dir,
            control,
        };
        f.crew = f.add_crew("示例机组", None).await;
        let crew = f.crew.clone();
        f.add_session(&crew, "s-cap", Role::Captain, "示例机长")
            .await;
        f.add_session(&crew, "s-w1", Role::Worker, "示例 worker")
            .await;
        f
    }

    /// 经客户端 RPC 建机组（与客户端同一条路，顺带建大厅）。
    pub async fn add_crew(&self, title: &str, parent: Option<&str>) -> String {
        let v = self
            .rpc(
                "crew.create",
                json!({"title": title, "workingDirectory": self.dir.path(), "parentId": parent}),
            )
            .await;
        v["id"].as_str().unwrap().to_string()
    }

    /// 固定 id 的 session。**这样加的 session 没有分机号**（`ext` 空，通讯录里没有它的号，机长除外）；
    /// 测试要用号时改用 `Fixture::register`。与 `sessions::register` 同样的效果（行、机长指针、大厅游标钉在末尾），
    /// 只是 id 由调用方定、没有分机号：测试在登记**之后**发的消息才是它的未读，与生产一致。
    pub async fn add_session(&self, crew: &str, id: &str, role: Role, label: &str) {
        let row = crewd_proto::Session {
            id: id.into(),
            crew_id: crew.into(),
            role,
            executor: "claude".into(),
            display_name: Some(label.into()),
            updated_at: "2026-10-06T00:00:00.000Z".into(),
            ..Default::default()
        };
        let (c, i) = (crew.to_string(), id.to_string());
        self.state
            .collab(move |tx| {
                crewd_store::sessions::upsert(tx.conn(), &row, None)?;
                if role == Role::Captain {
                    crewd_collab::crews::set_captain(tx, &c, &i)?;
                }
                let lobby = crewd_store::channels::lobby_id(&c);
                crewd_collab::cursors::pin_to_end(tx, &i, &lobby)?;
                // 大厅的成员是隐式的（= 本机组全体 session）：多了一个人，大厅就变了。
                tx.note(Change::ChannelChanged { channel_id: lobby });
                tx.note(Change::SessionChanged { session_id: i });
                Ok(())
            })
            .await
            .unwrap();
    }

    /// 走 `sessions::register`（分配分机号）；返回生成的 id。`label` 同时当标题（N1：`register` 要标题或 brief，
    /// 标题 ≤ 18 个码点）。
    pub async fn register(&self, crew: &str, role: Role, label: &str) -> String {
        let n = crewd_collab::sessions::NewSession {
            crew_id: crew.into(),
            role,
            executor: "claude".into(),
            title: Some(label.into()),
            display_name: Some(label.into()),
            ..Default::default()
        };
        self.state
            .collab(move |tx| crewd_collab::sessions::register(tx, n))
            .await
            .unwrap()
            .id
    }

    /// 换机长（M3a `crews::set_captain`，经 `AppState::collab` 发 CrewChanged / SessionChanged）。
    /// `Role::Worker` = 把机长还给夹具原来的机长。
    pub async fn set_role(&self, session: &str, role: Role) {
        let crew = self.me(session).await.crew_id;
        let to = if role == Role::Captain {
            session.to_string()
        } else {
            self.captain.clone()
        };
        self.state
            .collab(move |tx| crewd_collab::crews::set_captain(tx, &crew, &to))
            .await
            .unwrap();
    }

    pub fn token(&self, session: &str) -> String {
        self.state.tokens.mint(session)
    }

    pub async fn me(&self, session: &str) -> Identity {
        port::identity(&self.state, session).await.unwrap().unwrap()
    }

    /// 进程内调客户端 RPC，出错 panic。
    pub async fn rpc(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let resp = crate::dispatch::handle(
            &self.state,
            crewd_proto::Request {
                id: 1,
                method: method.into(),
                params,
            },
        )
        .await;
        match resp.into_result() {
            Ok(v) => v,
            Err(e) => panic!("{method}: {}", e.message),
        }
    }

    /// 人在某个群里发言（客户端 `message.post`）。
    pub async fn human_post(&self, channel: &str, text: &str, mentions: Vec<Mention>) -> Message {
        let v = self
            .rpc(
                "message.post",
                json!({"channelId": channel, "text": text, "mentions": mentions}),
            )
            .await;
        serde_json::from_value(v).unwrap()
    }

    /// session 以自己身份发言（绕过工具，造场景用）。
    pub async fn session_post(
        &self,
        session: &str,
        channel: &str,
        text: &str,
        mentions: Vec<Mention>,
    ) -> Message {
        port::post(
            &self.state,
            PostReq {
                channel_id: channel.into(),
                sender: PostSender::Session(session.into()),
                text: text.into(),
                mentions,
                in_reply_to: None,
                category: None,
                attachments: vec![],
            },
        )
        .await
        .unwrap()
        .message
    }

    /// 在 `<dir>/crewd.sock` 上起 accept 循环（与 serve 同一个 `conn::run`），返回 socket 路径。
    /// 一个夹具一个 socket：重复调用返回同一个。
    pub async fn spawn_socket(&self) -> PathBuf {
        let sock = self.dir.path().join("crewd.sock");
        if sock.exists() {
            return sock;
        }
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        let state = self.state.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let st = state.clone();
                tokio::spawn(async move {
                    let _ = crate::conn::run(st, stream).await;
                });
            }
        });
        sock
    }

    pub fn data_dir(&self) -> &Path {
        self.dir.path()
    }

    /// 本机组 Agent 那本 Todo 加一条（M1 的 store 函数；M3 没有客户端建 Todo 的入口）。
    pub async fn add_agent_todo(&self, number: i64, text: &str) {
        let todo = crewd_proto::Todo {
            id: format!("todo-{number}"),
            crew_id: self.crew.clone(),
            ledger: crewd_proto::Ledger::Agent,
            number,
            text: text.into(),
            status: "pending".into(),
            created_at: "2026-10-06T00:00:00.000Z".into(),
            updated_at: None,
            deleted_at: None,
            attachments: vec![],
            responses: vec![],
        };
        self.state
            .db
            .call(move |s| crewd_store::todos::insert(s.conn(), &todo, None))
            .await
            .unwrap()
            .unwrap();
    }

    /// 这个 session 的唤醒（kind, not_before 毫秒, note），新的在前（M3a `wakes::list`）。
    pub async fn wakes_of(&self, session: &str) -> Vec<(String, i64, String)> {
        let session = session.to_string();
        let rows = self
            .state
            .db
            .call(move |s| crewd_collab::wakes::list(s.conn(), Some(&session), 100))
            .await
            .unwrap()
            .unwrap();
        rows.into_iter()
            .map(|w| {
                (
                    w.kind.as_str().to_string(),
                    w.not_before_ms,
                    w.note.unwrap_or_default(),
                )
            })
            .collect()
    }

    /// The fake's next call (any method) fails with `e`.
    pub fn fail_next(&self, e: crate::control::ControlError) {
        self.control.fail_next(e);
    }

    /// The fake's next call of `method` fails with `e` (other methods go through).
    pub fn fail_method(&self, method: crate::control::FakeMethod, e: crate::control::ControlError) {
        self.control.fail_method(method, e);
    }

    /// What the fake's `live` / `inspect` report for `l.session_id` from now on.
    pub fn set_live(&self, l: crewd_proto::SessionLive) {
        self.control.set_live(l);
    }

    /// Apply the last `handoff_captain` the fake recorded the way M3c does (collab
    /// `crews::set_captain` through `AppState::collab`), so a test sees the data after a handoff.
    pub async fn apply_last_handoff(&self) {
        let (crew, to) = self
            .control
            .calls()
            .into_iter()
            .rev()
            .find_map(|c| match c {
                crate::control::FakeCall::Handoff(crew, to) => Some((crew, to)),
                _ => None,
            })
            .expect("a recorded handoff");
        self.state
            .collab(move |tx| crewd_collab::crews::set_captain(tx, &crew, &to))
            .await
            .unwrap();
    }

    /// FakeSessionControl 记下的调用，逐条 `{:?}`（如 `Start(StartRequest { crew_id: "…", role: Worker, … })`）。
    pub fn control_calls(&self) -> Vec<String> {
        self.control
            .calls()
            .iter()
            .map(|c| format!("{c:?}"))
            .collect()
    }
}

/// 测试用的 NDJSON 客户端：一条连接，回应按 id 取，事件先攒着。
pub struct RpcClient {
    rd: tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    wr: tokio::net::unix::OwnedWriteHalf,
    next: u64,
    events: std::collections::VecDeque<crewd_proto::Event>,
}

impl RpcClient {
    pub async fn connect(sock: &Path) -> Self {
        let (rd, wr) = tokio::net::UnixStream::connect(sock)
            .await
            .unwrap()
            .into_split();
        Self {
            rd: tokio::io::BufReader::new(rd),
            wr,
            next: 1,
            events: Default::default(),
        }
    }

    async fn frame(&mut self) -> crewd_proto::ServerFrame {
        use tokio::io::AsyncBufReadExt;
        let mut line = String::new();
        let n = self.rd.read_line(&mut line).await.unwrap();
        assert!(n > 0, "crewd closed the connection");
        serde_json::from_str(&line).unwrap()
    }

    pub async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, (i32, String)> {
        use tokio::io::AsyncWriteExt;
        let id = self.next;
        self.next += 1;
        let mut line = serde_json::to_vec(&crewd_proto::Request {
            id,
            method: method.into(),
            params,
        })
        .unwrap();
        line.push(b'\n');
        self.wr.write_all(&line).await.unwrap();
        loop {
            match self.frame().await {
                crewd_proto::ServerFrame::Event(e) => self.events.push_back(e),
                crewd_proto::ServerFrame::Response(r) if r.id == id => {
                    // `"result": null` 是成功、值为 null（M3a 的 `Outcome::Result(Value::Null)`）。
                    return r.into_result().map_err(|e| (e.code, e.message));
                }
                crewd_proto::ServerFrame::Response(_) => {}
            }
        }
    }

    pub async fn next_event(&mut self, within: std::time::Duration) -> Option<crewd_proto::Event> {
        if let Some(e) = self.events.pop_front() {
            return Some(e);
        }
        match tokio::time::timeout(within, self.frame()).await {
            Ok(crewd_proto::ServerFrame::Event(e)) => Some(e),
            _ => None,
        }
    }
}
