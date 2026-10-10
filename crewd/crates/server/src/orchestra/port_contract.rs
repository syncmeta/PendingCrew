//! Port 的契约（S13 的 Port 一半，T10a 补充 §2.7）：同一套用例跑 `MemPort` 与 `DbPort`（真库，经 collab 布置、
//! 固定 id，C17）。MemPort 比库松的地方修 MemPort，不在用例里迁就它。
//! 另有注入面 ↔ 规划候选的契约（§26.7 第 2 行）：对每个 session S 与每条消息 m，
//! `m ∈ unread(S)` ⇔ `S ∈ plan::candidates(m, channel_ctx(m 的群, now))`。
//!
//! 两边同一份布置（全是编的名字）：
//! ```text
//! chief（总机组）  chief-1
//! c-1（顶层）      机长 cap-1；w-1、w-2
//!   └ c-2          机长 cap-2
//! c-9（顶层）      机长 cap-9；w-9        ← 有的用例里被移除
//! ```
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use crewd_collab as collab;
use crewd_proto::MemberRef;
use crewd_wake::breaker::{self, HUMAN, SYSTEM};
use crewd_wake::plan::{self, Listen, Mention, MsgFacts, Sender};
use crewd_wake::queue::MAX_ATTEMPTS;
use crewd_wake::{
    Latch, LatchScope, MessageRef, Ms, NewWake, Priority, QueuedWake, WakeKind, WakeState, source,
};

use super::db::{self, DbPort, fixture};
use super::port::*;
use super::testkit::{MemChannel, MemKind, MemPort};
use crate::AppState;

const ALL: &[&str] = &["cap-1", "w-1", "w-2", "cap-2", "cap-9", "w-9", "chief-1"];

/// 谁发的消息 / 谁开的 dm。
#[derive(Clone, Copy, Debug)]
enum Who<'a> {
    Human,
    System,
    S(&'a str),
}

/// 一边的世界：一个 Port + 往它背后的数据里布置东西的手段（MemPort 直接写内存，DbPort 经 collab）。
trait World: Sized {
    type P: Port;
    async fn new() -> Self;
    fn port(&self) -> &Self::P;
    /// 这一边的「现在」：`channel_ctx` 用它（与 `unread` 内部判收听过期用的是同一个时刻）。
    fn now(&self) -> Ms;
    async fn topic(&mut self, crew: &str, owner: &str, members: &[&str]) -> String;
    /// `a` 与 session `b` 的 dm。
    async fn dm(&mut self, a: Who<'_>, b: &str) -> String;
    async fn human_channel(&mut self, crew: &str) -> String;
    /// `until` 早于现在 = 一条已经过期的收听（collab 的 `listens::set` 不收，DbPort 直接写行模拟）。
    async fn listen(&mut self, session: &str, channel: &str, until: Option<Ms>);
    async fn expire_listen(&mut self, session: &str, channel: &str);
    async fn post(&mut self, from: Who<'_>, channel: &str, at: &[&str]) -> MsgFacts;
    async fn remove_crew(&mut self, crew: &str);
    /// 人归档一个群（topic / dm）。
    async fn archive_channel(&mut self, channel: &str);
    /// M3a 入队的「问被回答」唤醒（origin 为空、优先级不一定是人）。
    async fn ask_answer_wake(&mut self, session: &str);
    /// 系统发到这个群里的正文（`post_system` 落在哪）。
    async fn system_posts(&self, channel: &str) -> Vec<String>;
    /// 系统发到这个群里的每一条 @ 了谁（session id；人 = `"human"`），按先后。
    async fn system_mentions(&self, channel: &str) -> Vec<Vec<String>>;
}

// —— DbPort 一边

struct DbWorld {
    state: Arc<AppState>,
    port: DbPort,
    _dir: tempfile::TempDir,
}

impl DbWorld {
    async fn collab<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut collab::Tx<'_>) -> collab::Result<T> + Send + 'static,
    ) -> T {
        self.state.collab(f).await.expect("布置失败")
    }
}

fn member(who: Who<'_>) -> MemberRef {
    match who {
        Who::S(s) => MemberRef::session(s.to_string()),
        _ => MemberRef::Human,
    }
}

impl World for DbWorld {
    type P = DbPort;

    async fn new() -> Self {
        let (state, port, dir) = fixture::seeded().await;
        Self {
            state,
            port,
            _dir: dir,
        }
    }

    fn port(&self) -> &DbPort {
        &self.port
    }

    fn now(&self) -> Ms {
        Ms(crewd_store::now_ms())
    }

    async fn topic(&mut self, crew: &str, owner: &str, members: &[&str]) -> String {
        let (crew, owner) = (crew.to_string(), owner.to_string());
        let members: Vec<MemberRef> = members
            .iter()
            .map(|s| MemberRef::session(s.to_string()))
            .collect();
        self.collab(move |tx| {
            collab::channels::create(
                tx,
                &collab::Sender::Session(owner),
                collab::channels::NewChannel {
                    crew_id: crew,
                    kind: crewd_proto::ChannelKind::Topic,
                    name: "合成话题".into(),
                    purpose: None,
                    members,
                },
            )
        })
        .await
        .id
    }

    async fn dm(&mut self, a: Who<'_>, b: &str) -> String {
        let anchor = match a {
            Who::S(s) => s.to_string(),
            _ => b.to_string(),
        };
        let (a, b) = (member(a), MemberRef::session(b.to_string()));
        self.collab(move |tx| {
            let crew = collab::sessions::crew_of(tx.conn(), &anchor)?;
            collab::channels::ensure_dm(tx, &crew, &a, &b)
        })
        .await
        .0
        .id
    }

    async fn human_channel(&mut self, crew: &str) -> String {
        let crew = crew.to_string();
        self.collab(move |tx| collab::channels::ensure_human_channel(tx, &crew))
            .await
            .id
    }

    async fn listen(&mut self, session: &str, channel: &str, until: Option<Ms>) {
        let (session, channel) = (session.to_string(), channel.to_string());
        match until {
            Some(t) if t <= self.now() => {
                self.state
                    .db
                    .call(move |s| {
                        s.conn().execute(
                            "INSERT INTO listens(session_id,channel_id,until,senders,created_at)
                             VALUES(?1,?2,?3,'[]','2026-09-01T00:00:00.000Z')",
                            rusqlite::params![session, channel, t.0],
                        )
                    })
                    .await
                    .unwrap()
                    .unwrap();
            }
            _ => {
                self.collab(move |tx| {
                    collab::listens::set(tx, &session, &channel, until.map(|t| t.0), vec![])
                })
                .await
            }
        }
    }

    async fn expire_listen(&mut self, session: &str, channel: &str) {
        let (session, channel, until) =
            (session.to_string(), channel.to_string(), self.now().0 - 1);
        self.state
            .db
            .call(move |s| {
                s.conn().execute(
                    "UPDATE listens SET until=?3 WHERE session_id=?1 AND channel_id=?2",
                    rusqlite::params![session, channel, until],
                )
            })
            .await
            .unwrap()
            .unwrap();
    }

    async fn post(&mut self, from: Who<'_>, channel: &str, at: &[&str]) -> MsgFacts {
        let sender = match from {
            Who::Human => collab::Sender::Human,
            Who::System => collab::Sender::System,
            Who::S(s) => collab::Sender::Session(s.to_string()),
        };
        let m = fixture::post_as(&self.state, sender, channel, "合成消息", at).await;
        db::facts_of(&m, self.now())
    }

    async fn remove_crew(&mut self, crew: &str) {
        let crew = crew.to_string();
        self.collab(move |tx| collab::crews::archive(tx, &crew))
            .await;
    }

    async fn archive_channel(&mut self, channel: &str) {
        let channel = channel.to_string();
        self.collab(move |tx| {
            collab::channels::update(
                tx,
                &collab::Sender::Human,
                &channel,
                collab::channels::ChannelPatch {
                    archived: Some(true),
                    ..Default::default()
                },
            )
        })
        .await;
    }

    async fn ask_answer_wake(&mut self, session: &str) {
        let session = session.to_string();
        self.collab(move |tx| {
            collab::wakes::enqueue(
                tx,
                collab::wakes::WakeRequest {
                    session,
                    kind: collab::wakes::WakeKind::AskAnswer,
                    source: collab::wakes::source::ask("c-1", "H1"),
                    priority: collab::wakes::WakePriority::System,
                    not_before_ms: Some(0),
                    note: None,
                    origin: None,
                },
            )
        })
        .await;
    }

    async fn system_posts(&self, channel: &str) -> Vec<String> {
        let channel = channel.to_string();
        self.state
            .db
            .call(move |s| collab::messages::list(s.conn(), &channel, None, None, 100))
            .await
            .unwrap()
            .unwrap()
            .into_iter()
            .filter(|m| m.sender_kind == crewd_proto::SenderKind::System)
            .map(|m| m.text)
            .collect()
    }

    async fn system_mentions(&self, channel: &str) -> Vec<Vec<String>> {
        let channel = channel.to_string();
        self.state
            .db
            .call(move |s| collab::messages::list(s.conn(), &channel, None, None, 100))
            .await
            .unwrap()
            .unwrap()
            .into_iter()
            .filter(|m| m.sender_kind == crewd_proto::SenderKind::System)
            .map(|m| {
                m.mentions
                    .into_iter()
                    .map(|x| match x.kind {
                        crewd_proto::MentionKind::Human => "human".to_string(),
                        _ => x.target_id.unwrap_or_default(),
                    })
                    .collect()
            })
            .collect()
    }
}

// —— MemPort 一边

struct MemWorld {
    port: MemPort,
    topics: usize,
    messages: usize,
}

fn row(id: &str, crew: &str, captain: bool) -> SessionRow {
    SessionRow {
        id: id.into(),
        crew_id: crew.into(),
        captain,
        executor: "claude".into(),
        display_name: Some(format!("名字 {id}")),
        agent_session_id: None,
        workdir: Some(PathBuf::from("/nonexistent/wd")),
        model: None,
        effort: None,
        brief: None,
        desired: Desired::Stopped,
        was_working: false,
        last_active_at: None,
        resume_failed_at: None,
        awaiting_question: None,
        last_progress_at: None,
        crew_removed: false,
    }
}

impl World for MemWorld {
    type P = MemPort;

    async fn new() -> Self {
        let port = MemPort::new();
        {
            let mut st = port.lock();
            st.now = Ms(1_000_000_000);
            for (id, crew, captain) in [
                ("cap-1", "c-1", true),
                ("w-1", "c-1", false),
                ("w-2", "c-1", false),
                ("cap-2", "c-2", true),
                ("cap-9", "c-9", true),
                ("w-9", "c-9", false),
                ("chief-1", "chief", false),
            ] {
                st.sessions.insert(id.into(), row(id, crew, captain));
            }
            for crew in ["c-1", "c-2", "c-9", "chief"] {
                st.crew_dirs
                    .insert(crew.into(), PathBuf::from("/nonexistent/wd"));
                st.channels.insert(
                    format!("{crew}/lobby"),
                    MemChannel {
                        crew: crew.into(),
                        kind: MemKind::Lobby,
                        owner: None,
                        members: vec![],
                    },
                );
            }
            st.crew_parents.insert("c-2".into(), "c-1".into());
        }
        Self {
            port,
            topics: 0,
            messages: 0,
        }
    }

    fn port(&self) -> &MemPort {
        &self.port
    }

    fn now(&self) -> Ms {
        self.port.lock().now
    }

    async fn topic(&mut self, crew: &str, owner: &str, members: &[&str]) -> String {
        self.topics += 1;
        let id = format!("{crew}/topic-{}", self.topics);
        let mut all = vec![owner.to_string()];
        all.extend(members.iter().map(|s| s.to_string()));
        self.port.lock().channels.insert(
            id.clone(),
            MemChannel {
                crew: crew.into(),
                kind: MemKind::Other,
                owner: Some(owner.into()),
                members: all.clone(),
            },
        );
        for s in all {
            self.port.join(&s, &id);
        }
        id
    }

    async fn dm(&mut self, a: Who<'_>, b: &str) -> String {
        self.topics += 1;
        let anchor = match a {
            Who::S(s) => s,
            _ => b,
        };
        let crew = self.port.row(anchor).crew_id;
        let id = format!("{crew}/dm-{}", self.topics);
        let mut members = Vec::new();
        if let Who::S(s) = a {
            members.push(s.to_string());
        }
        members.push(b.to_string());
        let owner = match a {
            Who::S(s) => Some(s.to_string()),
            _ => None,
        };
        self.port.lock().channels.insert(
            id.clone(),
            MemChannel {
                crew,
                kind: MemKind::Dm,
                owner,
                members: members.clone(),
            },
        );
        for s in members {
            self.port.join(&s, &id);
        }
        id
    }

    async fn human_channel(&mut self, crew: &str) -> String {
        let id = format!("{crew}/human");
        self.port.lock().channels.insert(
            id.clone(),
            MemChannel {
                crew: crew.into(),
                kind: MemKind::Human,
                owner: None,
                members: vec![],
            },
        );
        id
    }

    async fn listen(&mut self, session: &str, channel: &str, until: Option<Ms>) {
        self.port
            .lock()
            .listens
            .entry(channel.to_string())
            .or_default()
            .push(Listen {
                session: session.into(),
                until: until.unwrap_or(Ms(i64::MAX)),
                senders: None,
            });
        self.port.join(session, channel);
    }

    async fn expire_listen(&mut self, session: &str, channel: &str) {
        let until = self.now().add(-1);
        for l in self
            .port
            .lock()
            .listens
            .get_mut(channel)
            .into_iter()
            .flatten()
        {
            if l.session == session {
                l.until = until;
            }
        }
    }

    async fn post(&mut self, from: Who<'_>, channel: &str, at: &[&str]) -> MsgFacts {
        self.messages += 1;
        let mut st = self.port.lock();
        // 同库：seq 全库唯一（M3a.1），不是每个群各数各的——`message()` 的「群对不上」才测得出来。
        let seq = st.messages.keys().map(|(_, s)| *s).max().unwrap_or(0) + 1;
        let sender = match from {
            Who::Human => Sender::Human,
            Who::System => Sender::System,
            Who::S(s) => Sender::Session {
                id: s.into(),
                captain: st.sessions.get(s).is_some_and(|r| r.captain),
            },
        };
        let facts = MsgFacts {
            provenance: crewd_wake::plan::MessageProvenance::Ordinary,
            id: format!("m-{}", self.messages),
            at: MessageRef {
                channel: channel.into(),
                seq,
            },
            sender,
            mentions: at.iter().map(|s| Mention::Session(s.to_string())).collect(),
            created_at: st.now,
        };
        st.messages
            .insert((channel.into(), seq), (facts.clone(), "合成消息".into()));
        facts
    }

    async fn remove_crew(&mut self, crew: &str) {
        self.port.remove_crew(crew);
    }

    async fn archive_channel(&mut self, channel: &str) {
        self.port
            .lock()
            .archived_channels
            .insert(channel.to_string());
    }

    /// MemPort 没有 NULL origin（`NewWake.origin` 是 String）：这里直接存 `human`，所以「问的答复读回成人」这条规则
    /// 只在 DbPort 那一边是真测（`origin_from` 把 NULL 解成 human）；MemPort 这边只是同形。
    async fn ask_answer_wake(&mut self, session: &str) {
        let w = NewWake {
            session: session.into(),
            kind: WakeKind::AskAnswer,
            source: "ask:c-1/H1".into(),
            priority: Priority::System,
            not_before: Ms(0),
            note: None,
            origin: HUMAN.into(),
        };
        self.port.enqueue(w).await.unwrap();
    }

    async fn system_posts(&self, channel: &str) -> Vec<String> {
        self.port
            .lock()
            .posted
            .iter()
            .filter(|(c, _)| c == channel)
            .map(|(_, t)| t.clone())
            .collect()
    }

    async fn system_mentions(&self, channel: &str) -> Vec<Vec<String>> {
        let st = self.port.lock();
        st.posted
            .iter()
            .zip(&st.posted_mentions)
            .filter(|((c, _), _)| c == channel)
            .map(|(_, m)| m.clone())
            .collect()
    }
}

// —— 用例（每条两边各跑一次，见文末的宏）

fn timer(session: &str, token: &str) -> NewWake {
    NewWake {
        session: session.into(),
        kind: WakeKind::Timer,
        source: source::timer(token),
        priority: Priority::Auto,
        not_before: Ms(0),
        note: Some("合成提醒".into()),
        origin: session.into(),
    }
}

fn batch(session: &str, wakes: Vec<QueuedWake>) -> Batch {
    Batch {
        session: session.into(),
        wakes,
        ..Default::default()
    }
}

fn sources(v: &[QueuedWake]) -> Vec<String> {
    v.iter().map(|w| w.source.clone()).collect()
}

async fn enqueue_dedupes<W: World>() {
    let w = W::new().await;
    let id = w.port().enqueue(timer("w-1", "t")).await.unwrap();
    assert!(id.is_some_and(|id| id > 0), "新行回它的 id");
    assert!(
        w.port().enqueue(timer("w-1", "t")).await.unwrap().is_none(),
        "撞了 UNIQUE"
    );
    assert_eq!(w.port().pending("w-1").await.unwrap().len(), 1);
}

async fn enqueue_for_a_removed_crew_is_nothing<W: World>() {
    let mut w = W::new().await;
    w.remove_crew("c-9").await;
    assert!(
        w.port().enqueue(timer("w-9", "t")).await.unwrap().is_none(),
        "Skipped"
    );
    assert!(w.port().pending("w-9").await.unwrap().is_empty());
    assert!(
        w.port().list_wakes(Some("w-9")).await.unwrap().is_empty(),
        "没写行"
    );
    assert!(w.port().crew_removed("c-9").await.unwrap());
    assert!(!w.port().crew_removed("c-1").await.unwrap());
    let r = w.port().session("w-9").await.unwrap().unwrap();
    assert!(r.crew_removed);
}

async fn pending_is_by_priority_then_age<W: World>() {
    let w = W::new().await;
    let p = w.port();
    p.enqueue(timer("w-1", "a1")).await.unwrap();
    let human = NewWake {
        kind: WakeKind::System,
        source: source::system("h"),
        priority: Priority::Human,
        origin: HUMAN.into(),
        ..timer("w-1", "")
    };
    p.enqueue(human).await.unwrap();
    let system = NewWake {
        kind: WakeKind::System,
        source: source::system("s"),
        priority: Priority::System,
        origin: SYSTEM.into(),
        ..timer("w-1", "")
    };
    p.enqueue(system).await.unwrap();
    p.enqueue(timer("w-1", "a2")).await.unwrap();
    assert_eq!(
        sources(&p.pending("w-1").await.unwrap()),
        ["system:h", "system:s", "timer:a1", "timer:a2"]
    );
}

async fn claim_is_all_or_nothing_and_a_row_dropped_meanwhile_conflicts<W: World>() {
    let w = W::new().await;
    let p = w.port();
    p.enqueue(timer("w-1", "a")).await.unwrap();
    p.enqueue(timer("w-1", "b")).await.unwrap();
    let wakes = p.pending("w-1").await.unwrap();
    let s = Settle::Dropped {
        error: "stopped".into(),
    };
    p.settle(&[wakes[1].id], s, Ms(0)).await.unwrap();
    assert!(p.claim(&batch("w-1", wakes.clone())).await.is_err());
    let after = p.list_wakes(Some("w-1")).await.unwrap();
    let a = after.iter().find(|x| x.id == wakes[0].id).unwrap();
    assert_eq!((a.state, a.attempts), (WakeState::Pending, 0), "一行都没拿");
}

async fn ack_delivered_moves_cursors_in_the_same_transaction<W: World>() {
    let mut w = W::new().await;
    let m = w.post(Who::S("cap-1"), "c-1/lobby", &["w-1"]).await;
    let wake = NewWake {
        kind: WakeKind::Message,
        source: source::message(&m.id),
        origin: "cap-1".into(),
        ..timer("w-1", "")
    };
    let p = w.port();
    p.enqueue(wake).await.unwrap();
    let cursor0 = p.cursor("w-1", "c-1/lobby").await.unwrap();
    let mut b = batch("w-1", p.pending("w-1").await.unwrap());
    p.claim(&b).await.unwrap();
    // 第二个游标推不了（没有这个群）：整批回滚——行还是 inflight，第一个群的游标也没动。
    b.cursor_targets = vec![("c-1/lobby".into(), m.at.seq), ("nope/lobby".into(), 1)];
    let delivered = || Ack::Delivered {
        riders: Riders::Release,
    };
    assert!(p.ack(&b, delivered()).await.is_err());
    assert_eq!(p.cursor("w-1", "c-1/lobby").await.unwrap(), cursor0);
    assert_eq!(
        p.list_wakes(Some("w-1")).await.unwrap()[0].state,
        WakeState::Inflight
    );
    b.cursor_targets = vec![("c-1/lobby".into(), m.at.seq)];
    let acked = p.ack(&b, delivered()).await.unwrap();
    assert_eq!(acked.origins, ["cap-1"]);
    assert_eq!(p.cursor("w-1", "c-1/lobby").await.unwrap(), m.at.seq);
    assert_eq!(
        p.list_wakes(Some("w-1")).await.unwrap()[0].state,
        WakeState::Delivered
    );
}

async fn ack_failed_counts_by_the_stored_attempts<W: World>() {
    let w = W::new().await;
    let p = w.port();
    p.enqueue(timer("w-1", "f")).await.unwrap();
    let b = batch("w-1", p.pending("w-1").await.unwrap());
    for n in 1..=MAX_ATTEMPTS {
        p.claim(&b).await.unwrap();
        // 手里那份的次数一直是 0：判据只看库里（claim 记的）。
        let failed = Ack::Failed {
            error: "e".into(),
            now: Ms(0),
            jitter: 0.0,
        };
        let a = p.ack(&b, failed).await.unwrap();
        if n < MAX_ATTEMPTS {
            assert_eq!(a.retried.len(), 1, "第 {n} 次");
        } else {
            assert_eq!(a.gave_up, [b.wakes[0].id], "到上限放弃");
        }
    }
}

async fn fail_inflight_hands_back_old_given_up_rows<W: World>() {
    let w = W::new().await;
    let p = w.port();
    p.enqueue(timer("w-1", "crash")).await.unwrap();
    let b = batch("w-1", p.pending("w-1").await.unwrap());
    for i in 0..205 {
        p.enqueue(timer("w-2", &format!("n{i}"))).await.unwrap();
    }
    let mut r = Reclaimed::default();
    for _ in 0..MAX_ATTEMPTS {
        p.claim(&b).await.unwrap();
        r = p.fail_inflight("crewd restarted", Ms(0)).await.unwrap();
    }
    assert_eq!(r.gave_up.len(), 1);
    assert_eq!(
        (r.gave_up[0].id, r.gave_up[0].session.as_str()),
        (b.wakes[0].id, "w-1")
    );
    assert_eq!(r.gave_up[0].state, WakeState::Dropped);
}

async fn due_sessions_are_by_top_priority_then_oldest<W: World>() {
    let w = W::new().await;
    let p = w.port();
    p.enqueue(timer("cap-1", "a")).await.unwrap();
    p.enqueue(timer("w-1", "b")).await.unwrap();
    let human = NewWake {
        kind: WakeKind::System,
        source: source::system("h"),
        priority: Priority::Human,
        origin: HUMAN.into(),
        ..timer("w-2", "")
    };
    p.enqueue(human).await.unwrap();
    let later = NewWake {
        not_before: Ms(i64::MAX / 2),
        ..timer("cap-2", "later")
    };
    p.enqueue(later).await.unwrap();
    assert_eq!(
        p.due_sessions(w.now()).await.unwrap(),
        ["w-2", "cap-1", "w-1"],
        "没到点的不算"
    );
}

async fn next_not_before_skips_ones_already_due<W: World>() {
    let w = W::new().await;
    let p = w.port();
    let at = |t: &str, ms: i64| NewWake {
        not_before: Ms(ms),
        ..timer("w-1", t)
    };
    p.enqueue(at("past", 10)).await.unwrap();
    p.enqueue(at("future", 500)).await.unwrap();
    assert_eq!(
        p.next_not_before_after(Ms(100)).await.unwrap(),
        Some(Ms(500))
    );
    assert_eq!(p.next_not_before_after(Ms(600)).await.unwrap(), None);
}

async fn list_wakes_is_the_newest_capped_rows<W: World>() {
    let w = W::new().await;
    let p = w.port();
    for i in 0..(WAKE_LIST_LIMIT + 5) {
        p.enqueue(timer("w-1", &format!("t{i}"))).await.unwrap();
    }
    p.enqueue(timer("w-2", "other")).await.unwrap();
    let v = p.list_wakes(Some("w-1")).await.unwrap();
    assert_eq!(v.len(), WAKE_LIST_LIMIT);
    assert!(v.windows(2).all(|x| x[0].id > x[1].id), "新的在前");
    assert_eq!(v[0].source, format!("timer:t{}", WAKE_LIST_LIMIT + 4));
    assert!(v.iter().all(|x| x.session == "w-1"));
}

async fn latches_round_trip_both_scopes<W: World>() {
    let w = W::new().await;
    let p = w.port();
    let edge = Latch {
        scope: LatchScope::Edge,
        id: breaker::edge_id("cap-1", "w-1"),
        reason: "no-progress:9/10min".into(),
        at: Ms(0),
    };
    let target = Latch {
        scope: LatchScope::Session,
        id: "w-1".into(),
        ..edge.clone()
    };
    p.put_latch(&edge).await.unwrap();
    p.put_latch(&target).await.unwrap();
    let mut got: Vec<(LatchScope, String, String)> = p
        .latches()
        .await
        .unwrap()
        .into_iter()
        .map(|l| (l.scope, l.id, l.reason))
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            (LatchScope::Session, "w-1".into(), edge.reason.clone()),
            (LatchScope::Edge, edge.id.clone(), edge.reason.clone()),
        ]
    );
    p.clear_latch(LatchScope::Edge, &edge.id).await.unwrap();
    let left: Vec<LatchScope> = p.latches().await.unwrap().iter().map(|l| l.scope).collect();
    assert_eq!(left, [LatchScope::Session]);
}

async fn origins_round_trip<W: World>() {
    let mut w = W::new().await;
    let from_human = w.post(Who::Human, "c-1/lobby", &[]).await;
    let from_system = w.post(Who::System, "c-1/lobby", &[]).await;
    let msg = |m: &MsgFacts, origin: &str| NewWake {
        kind: WakeKind::Message,
        source: source::message(&m.id),
        origin: origin.into(),
        ..timer("w-1", "")
    };
    let p = w.port();
    // 人发的叫醒收听者：Auto 档，来源仍是人。
    p.enqueue(msg(&from_human, HUMAN)).await.unwrap();
    p.enqueue(msg(&from_system, SYSTEM)).await.unwrap();
    let sys = NewWake {
        kind: WakeKind::System,
        source: source::system("x"),
        priority: Priority::System,
        origin: SYSTEM.into(),
        ..timer("w-1", "")
    };
    p.enqueue(sys).await.unwrap();
    p.enqueue(timer("w-1", "own")).await.unwrap();
    w.ask_answer_wake("w-1").await;
    let mut got: Vec<(String, String)> = w
        .port()
        .pending("w-1")
        .await
        .unwrap()
        .into_iter()
        .map(|q| (q.source, q.origin))
        .collect();
    got.sort();
    let mut want = vec![
        (source::message(&from_human.id), HUMAN.to_string()),
        (source::message(&from_system.id), SYSTEM.to_string()),
        (source::system("x"), SYSTEM.to_string()),
        (source::timer("own"), "w-1".to_string()),
        ("ask:c-1/H1".to_string(), HUMAN.to_string()),
    ];
    want.sort();
    assert_eq!(got, want);
}

fn sorted(v: &[String]) -> Vec<String> {
    let mut v = v.to_vec();
    v.sort();
    v
}

/// C22 的填法：六个字段两边一致；群里有 session 成员或收听者时 `live_sessions` 不空。
async fn channel_ctx_fills_all_six_fields<W: World>() {
    let mut w = W::new().await;
    let topic = w.topic("c-1", "cap-1", &["w-2"]).await;
    let dm_ss = w.dm(Who::S("w-1"), "w-2").await;
    let dm_hs = w.dm(Who::Human, "w-1").await;
    let human = w.human_channel("c-1").await;
    w.listen("chief-1", "c-1/lobby", None).await;
    let past = Ms(w.now().0 - 1_000);
    w.listen("cap-2", &human, Some(past)).await;
    let now = w.now();
    let ctx = |ch: String| {
        let p = w.port();
        async move { p.channel_ctx(&ch, now).await.unwrap().expect("群在") }
    };
    let lobby = ctx("c-1/lobby".into()).await;
    assert_eq!(sorted(&lobby.session_members), ["cap-1", "w-1", "w-2"]);
    assert_eq!(lobby.owner.as_deref(), Some("cap-1"));
    assert_eq!(lobby.crew_captain.as_deref(), Some("cap-1"));
    assert!(lobby.dm_sessions.is_empty());
    let listeners: Vec<&str> = lobby.listens.iter().map(|l| l.session.as_str()).collect();
    assert_eq!(listeners, ["chief-1"]);
    assert_eq!(
        sorted(&lobby.live_sessions),
        ["cap-1", "chief-1", "w-1", "w-2"],
        "成员 ∪ 收听者"
    );
    let t = ctx(topic).await;
    assert_eq!(sorted(&t.session_members), ["cap-1", "w-2"]);
    assert_eq!(t.owner.as_deref(), Some("cap-1"));
    assert!(t.dm_sessions.is_empty());
    let d = ctx(dm_ss).await;
    assert_eq!(sorted(&d.dm_sessions), ["w-1", "w-2"]);
    assert_eq!(sorted(&d.session_members), ["w-1", "w-2"]);
    assert_eq!(d.owner.as_deref(), Some("w-1"));
    assert_eq!(d.crew_captain.as_deref(), Some("cap-1"));
    let d = ctx(dm_hs).await;
    assert_eq!(d.dm_sessions, ["w-1"]);
    assert_eq!(d.owner, None, "人开的 dm：群主不是 session");
    let h = ctx(human).await;
    assert_eq!(h.session_members, ["cap-1"], "人类群 = home 的机长");
    assert_eq!(h.owner.as_deref(), Some("cap-1"));
    assert!(h.listens.is_empty(), "过期的收听不给");
    assert_eq!(h.live_sessions, ["cap-1"]);
    assert!(w.port().channel_ctx("nope", now).await.unwrap().is_none());
}

async fn a_child_crews_human_notice_lands_in_its_homes_human_channel<W: World>() {
    let mut w = W::new().await;
    let home = w.human_channel("c-1").await;
    let to = Audience::HumanChannel {
        crew: "c-2".into(),
        mention_human: true,
    };
    w.port().post_system(to, "合成公告".into()).await.unwrap();
    assert_eq!(w.system_posts(&home).await, ["合成公告"]);
}

/// §26.7 第 2 行：注入面（`unread`）与规划的候选（`plan::candidates`）在每一条消息上一致——都是成员（含收听者），
/// 都只排除发送者。布置里有大厅、显式成员的 topic、session 之间的 dm、人与 session 的 dm、home 人类群（机长）、
/// 群外的收听者、过期的收听者、以非成员身份发言的总机组 session、被移除的机组。
async fn injection_and_planner_agree_on_every_message<W: World>() {
    let mut w = W::new().await;
    let topic = w.topic("c-1", "cap-1", &["w-2"]).await;
    let old_topic = w.topic("c-1", "cap-1", &["w-1"]).await;
    let dm_ss = w.dm(Who::S("w-1"), "w-2").await;
    let dm_hs = w.dm(Who::Human, "w-1").await;
    let human = w.human_channel("c-1").await;
    w.listen("chief-1", "c-1/lobby", None).await;
    w.listen("chief-1", &old_topic, None).await;
    let past = Ms(w.now().0 - 1_000);
    w.listen("cap-2", &human, Some(past)).await;
    // 入群点：各自注入面上的群先钉一次游标（之后发的才算未读）。
    for s in ALL {
        w.port().unread(s).await.unwrap();
    }
    let mut posted = Vec::new();
    for (from, ch, at) in [
        (Who::Human, "c-1/lobby".to_string(), vec![]),
        (Who::S("cap-1"), "c-1/lobby".into(), vec!["w-1"]),
        (Who::S("chief-1"), "c-1/lobby".into(), vec!["w-1"]),
        (Who::S("w-1"), "c-1/lobby".into(), vec!["cap-1"]),
        (Who::System, "c-1/lobby".into(), vec![]),
        (Who::S("w-2"), topic.clone(), vec![]),
        (Who::S("w-1"), dm_ss.clone(), vec![]),
        (Who::Human, dm_hs.clone(), vec![]),
        (Who::Human, human.clone(), vec![]),
        (Who::S("cap-2"), "c-2/lobby".into(), vec![]),
        (Who::S("cap-9"), "c-9/lobby".into(), vec![]),
        (Who::S("cap-1"), old_topic.clone(), vec!["w-1"]),
    ] {
        posted.push(w.post(from, &ch, &at).await);
    }
    w.remove_crew("c-9").await;
    // 发完之后才归档的 topic：它里面的消息谁都不再被注入 / 叫醒（成员、收听者都不算）。
    w.archive_channel(&old_topic).await;
    let now = w.now();
    let mut mismatches = Vec::new();
    let mut seen_by: std::collections::BTreeMap<&str, BTreeSet<String>> = Default::default();
    for s in ALL {
        let seen: BTreeSet<String> = w
            .port()
            .unread(s)
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        for m in &posted {
            let ctx = w
                .port()
                .channel_ctx(&m.at.channel, now)
                .await
                .unwrap()
                .unwrap_or_default();
            let cand = plan::candidates(m, &ctx).iter().any(|c| c == s);
            if seen.contains(&m.id) != cand {
                mismatches.push(format!(
                    "{s} / {} 在 {}（{:?}）：注入 {} 候选 {cand}",
                    m.id,
                    m.at.channel,
                    m.sender,
                    seen.contains(&m.id)
                ));
            }
        }
        seen_by.insert(s, seen);
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
    // 布置真的覆盖到了（不是两边一起空着）：几条有代表性的读数。
    let saw = |s: &str, i: usize| seen_by[s].contains(&posted[i].id);
    assert!(
        saw("chief-1", 0) && saw("chief-1", 3),
        "群外的收听者：人发的、@ 别人的都看得到"
    );
    assert!(!saw("chief-1", 2), "自己发的不算");
    assert!(saw("w-2", 6) && saw("w-1", 7), "两种 dm");
    assert!(saw("cap-1", 8), "home 人类群 = 机长");
    assert!(!saw("cap-2", 8), "过期的收听");
    assert!(saw("w-9", 10), "被移除的机组：自己大厅里的照样在注入面上");
    assert!(!saw("w-1", 5), "topic 只给成员");
    assert!(!saw("w-1", 11) && !saw("chief-1", 11), "归档的群");
}

/// `message.receipts` 的来源（S54）：一条消息给每个 session 入队的唤醒，任何状态、按入队先后；作废的原因在 `last_error`。
async fn wakes_for_message_lists_each_target_in_order<W: World>() {
    let mut w = W::new().await;
    let lobby = crewd_store::channels::lobby_id("c-1");
    let m = w.post(Who::S("cap-1"), &lobby, &["w-1", "w-2"]).await;
    let msg = |session: &str| NewWake {
        session: session.into(),
        kind: WakeKind::Message,
        source: source::message(&m.id),
        priority: Priority::Auto,
        not_before: Ms(0),
        note: None,
        origin: "cap-1".into(),
    };
    let p = w.port();
    let a = p.enqueue(msg("w-2")).await.unwrap().unwrap();
    let b = p.enqueue(msg("w-1")).await.unwrap().unwrap();
    p.enqueue(timer("w-1", &m.id)).await.unwrap();
    p.settle(
        &[a],
        Settle::Dropped {
            error: "合成：停了".into(),
        },
        Ms(0),
    )
    .await
    .unwrap();
    let got = p.wakes_for_message(&m.id).await.unwrap();
    let rows: Vec<(i64, &str, WakeState, Option<&str>)> = got
        .iter()
        .map(|w| (w.id, w.session.as_str(), w.state, w.last_error.as_deref()))
        .collect();
    assert_eq!(
        rows,
        [
            (a, "w-2", WakeState::Dropped, Some("合成：停了")),
            (b, "w-1", WakeState::Pending, None)
        ],
        "按入队先后；定时不算；作废的原因读得回来"
    );
    assert_eq!(
        got[1].msg.as_ref().map(|r| r.channel.as_str()),
        Some(lobby.as_str())
    );
    assert!(p.wakes_for_message("nope").await.unwrap().is_empty());
}

/// S49：三个设置一起写、读回；null 清掉（回到默认）；没给的不动；被拒的一次什么都不改（一个事务，修复轮 M4）。
async fn settings_round_trip<W: World>() {
    use crewd_wake::text::Lang;
    let w = W::new().await;
    let p = w.port();
    assert_eq!(p.locale().await.unwrap(), Lang::Zh);
    let all = SettingsUpdate {
        lang: Some(Lang::En),
        max_live: Some(Some(3)),
        hibernate_deadline_secs: Some(Some(0)),
    };
    p.set_settings(&all).await.unwrap();
    assert_eq!(p.locale().await.unwrap(), Lang::En);
    assert_eq!(p.hibernate_deadline_secs().await.unwrap(), Some(0));
    assert_eq!(p.max_live().await.unwrap(), Some(3));
    let bad = SettingsUpdate {
        lang: Some(Lang::Zh),
        hibernate_deadline_secs: Some(Some(9)),
        max_live: Some(Some(0)),
    };
    assert!(p.set_settings(&bad).await.is_err(), "上限 0 被拒");
    assert_eq!(p.locale().await.unwrap(), Lang::En, "一个事务：语言也没改");
    assert_eq!(p.hibernate_deadline_secs().await.unwrap(), Some(0));
    let clear = SettingsUpdate {
        max_live: Some(None),
        hibernate_deadline_secs: Some(None),
        ..Default::default()
    };
    p.set_settings(&clear).await.unwrap();
    assert_eq!(p.max_live().await.unwrap(), None);
    assert_eq!(p.hibernate_deadline_secs().await.unwrap(), None);
    assert_eq!(p.locale().await.unwrap(), Lang::En, "没给的不动");
}

/// T10a 复审：归档的 dm 里没有人「在」——没有成员、也没有 dm 各方（两边一致；MemPort 曾照旧给 dm 各方）。
async fn an_archived_dm_has_no_members_and_no_parties<W: World>() {
    let mut w = W::new().await;
    let dm = w.dm(Who::S("w-1"), "w-2").await;
    let now = w.now();
    let live = w.port().channel_ctx(&dm, now).await.unwrap().unwrap();
    let mut parties = live.dm_sessions.clone();
    parties.sort();
    assert_eq!(parties, ["w-1", "w-2"]);
    w.archive_channel(&dm).await;
    let gone = w.port().channel_ctx(&dm, now).await.unwrap().unwrap();
    assert!(gone.session_members.is_empty(), "{gone:?}");
    assert!(gone.dm_sessions.is_empty(), "{gone:?}");
}

/// 不属于一批的结算（`settle`）：Delivered 从 pending；Requeue 从 inflight 回 pending、退回 claim 记的那一次；
/// 表不认的（已结的再 Delivered、pending 的 Requeue）= 错误、什么都不改。
async fn settle_delivered_and_requeue_follow_the_table<W: World>() {
    let w = W::new().await;
    let p = w.port();
    p.enqueue(timer("w-1", "a")).await.unwrap();
    p.enqueue(timer("w-1", "b")).await.unwrap();
    let rows = p.pending("w-1").await.unwrap();
    let (a, b) = (rows[0].clone(), rows[1].clone());
    p.claim(&batch("w-1", vec![b.clone()])).await.unwrap();
    p.settle(&[a.id], Settle::Delivered, Ms(0)).await.unwrap();
    p.settle(&[b.id], Settle::Requeue, Ms(0)).await.unwrap();
    let state = |id: i64, v: &[QueuedWake]| {
        let r = v.iter().find(|w| w.id == id).unwrap();
        (r.state, r.attempts)
    };
    let all = p.list_wakes(Some("w-1")).await.unwrap();
    assert_eq!(state(a.id, &all), (WakeState::Delivered, 0));
    assert_eq!(
        state(b.id, &all),
        (WakeState::Pending, 0),
        "退回 claim 那一次"
    );
    assert!(
        p.settle(&[a.id], Settle::Delivered, Ms(0)).await.is_err(),
        "已结的不能再结"
    );
    assert!(
        p.settle(&[b.id, a.id], Settle::Requeue, Ms(0))
            .await
            .is_err(),
        "pending 的不能 Requeue；整批不改"
    );
    let all = p.list_wakes(Some("w-1")).await.unwrap();
    assert_eq!(state(b.id, &all), (WakeState::Pending, 0));
}

/// `ack(Delivered)`：随批行按 `Riders` 办——Release 回 pending（退回那一次）、Drop 作废并记原因；批里有别的 session
/// 的行 = 错误，一行都不改（两边一致）。
async fn ack_delivered_releases_or_drops_riders_and_checks_ownership<W: World>() {
    let w = W::new().await;
    let p = w.port();
    for t in ["a", "b", "c", "d"] {
        p.enqueue(timer("w-1", t)).await.unwrap();
    }
    p.enqueue(timer("w-2", "x")).await.unwrap();
    let rows = p.pending("w-1").await.unwrap();
    let other = p.pending("w-2").await.unwrap();
    let mut b1 = batch("w-1", vec![rows[0].clone()]);
    b1.riders = vec![rows[1].clone()];
    p.claim(&b1).await.unwrap();
    p.ack(
        &b1,
        Ack::Delivered {
            riders: Riders::Release,
        },
    )
    .await
    .unwrap();
    let mut b2 = batch("w-1", vec![rows[2].clone()]);
    b2.riders = vec![rows[3].clone()];
    p.claim(&b2).await.unwrap();
    p.ack(
        &b2,
        Ack::Delivered {
            riders: Riders::Drop("合成：停了".into()),
        },
    )
    .await
    .unwrap();
    let all = p.list_wakes(Some("w-1")).await.unwrap();
    let got = |id: i64| {
        let r = all.iter().find(|w| w.id == id).unwrap();
        (r.state, r.attempts, r.last_error.clone())
    };
    assert_eq!(got(rows[0].id), (WakeState::Delivered, 1, None));
    assert_eq!(
        got(rows[1].id),
        (WakeState::Pending, 0, None),
        "Release：回 pending"
    );
    assert_eq!(got(rows[2].id), (WakeState::Delivered, 1, None));
    assert_eq!(
        got(rows[3].id),
        (WakeState::Dropped, 1, Some("合成：停了".into())),
        "Drop：作废、记原因"
    );
    // 批是 w-1 的，里面夹了 w-2 的一行：整批不动。
    let mut bad = batch("w-1", vec![rows[1].clone()]);
    bad.riders = vec![other[0].clone()];
    p.claim(&bad).await.unwrap();
    assert!(
        p.ack(
            &bad,
            Ack::Delivered {
                riders: Riders::Release
            }
        )
        .await
        .is_err(),
        "别的 session 的行"
    );
    let all = p.list_wakes(None).await.unwrap();
    for id in [rows[1].id, other[0].id] {
        let r = all.iter().find(|w| w.id == id).unwrap();
        assert_eq!(r.state, WakeState::Inflight, "一行都没改");
    }
}

/// `prune_wakes`：只清早于 `before` 结算的 delivered / dropped，按结算先后、至多 `limit` 条；pending 的不碰。
async fn prune_wakes_only_clears_rows_settled_before_the_cutoff<W: World>() {
    let w = W::new().await;
    let p = w.port();
    for t in ["a", "b", "c"] {
        p.enqueue(timer("w-1", t)).await.unwrap();
    }
    let rows = p.pending("w-1").await.unwrap();
    p.settle(&[rows[0].id], Settle::Delivered, Ms(0))
        .await
        .unwrap();
    p.settle(
        &[rows[1].id],
        Settle::Dropped {
            error: "合成".into(),
        },
        Ms(0),
    )
    .await
    .unwrap();
    assert_eq!(p.prune_wakes(Ms(0), 10).await.unwrap(), 0, "都是之后结算的");
    let later = w.now().add(crewd_wake::HOUR);
    assert_eq!(p.prune_wakes(later, 1).await.unwrap(), 1, "至多 limit 条");
    assert_eq!(p.prune_wakes(later, 10).await.unwrap(), 1);
    assert_eq!(p.prune_wakes(later, 10).await.unwrap(), 0);
    let left: Vec<i64> = p
        .list_wakes(Some("w-1"))
        .await
        .unwrap()
        .iter()
        .map(|w| w.id)
        .collect();
    assert_eq!(left, [rows[2].id], "pending 的留着");
}

/// `post_system`：大厅 / 群 `mention_owner` 时 @ 群主（大厅 = 机长，topic = 它的群主），不要就不 @；人类群 @人。
async fn post_system_mentions_the_owner_only_when_asked<W: World>() {
    let mut w = W::new().await;
    let topic = w.topic("c-1", "w-1", &["w-2"]).await;
    let p = w.port();
    for mention_human in [true, false] {
        p.post_system(
            Audience::HumanChannel {
                crew: "c-1".into(),
                mention_human,
            },
            "synthetic".into(),
        )
        .await
        .unwrap();
    }
    let human = w.human_channel("c-1").await;
    assert!(w.system_mentions("c-1/lobby").await.is_empty());
    assert!(w.system_mentions(&topic).await.is_empty());
    assert_eq!(
        w.system_mentions(&human).await,
        [vec!["human".to_string()], vec![]]
    );
}

/// `message()`：seq 全库唯一，群对不上 = 不是这条（两边一致）。
async fn message_with_the_wrong_channel_is_none<W: World>() {
    let mut w = W::new().await;
    let topic = w.topic("c-1", "w-1", &["w-2"]).await;
    let m = w.post(Who::S("cap-1"), "c-1/lobby", &[]).await;
    let m2 = w.post(Who::S("w-1"), &topic, &[]).await;
    let p = w.port();
    let at = |channel: &str, seq| MessageRef {
        channel: channel.into(),
        seq,
    };
    let got = p.message(&at("c-1/lobby", m.at.seq)).await.unwrap();
    assert_eq!(got.map(|(f, _)| f.id), Some(m.id.clone()));
    assert!(p.message(&at(&topic, m.at.seq)).await.unwrap().is_none());
    assert!(
        p.message(&at("c-1/lobby", m2.at.seq))
            .await
            .unwrap()
            .is_none()
    );
    assert!(p.message(&at(&topic, m2.at.seq)).await.unwrap().is_some());
}

/// 没有这个机组：`crew_removed` 是 `crew.not_found`（两边一致），编排据此回 -32004（不是内部错误）。
async fn crew_removed_of_an_unknown_crew_is_not_found<W: World>() {
    let w = W::new().await;
    let e = w.port().crew_removed("没有这个").await.unwrap_err();
    match super::core::ControlError::from_port(e) {
        super::core::ControlError::Refused { kind, refusal } => {
            assert_eq!(kind, super::core::RefusedKind::NotFound);
            assert_eq!(refusal.code, collab::refusal::Code::CrewNotFound);
        }
        other => panic!("{other:?}"),
    }
}

/// 一个持久人停边界，跨后来的 desired=Running：旧行不退回，新行仍退回，次数跟库共用 C24。
async fn human_stop_boundary_survives_new_work_and_mixed_requeue<W: World>() {
    let w = W::new().await;
    let p = w.port();
    let old = p.enqueue(timer("w-1", "old")).await.unwrap().unwrap();
    let pending = p.enqueue(timer("w-1", "pending")).await.unwrap().unwrap();
    let old_row = p
        .pending("w-1")
        .await
        .unwrap()
        .into_iter()
        .find(|w| w.id == old)
        .unwrap();
    p.claim(&batch("w-1", vec![old_row])).await.unwrap();
    let dropped = p.human_stop("w-1").await.unwrap();
    assert_eq!(dropped.iter().map(|w| w.id).collect::<Vec<_>>(), [pending]);
    let fresh = p.enqueue(timer("w-1", "fresh")).await.unwrap().unwrap();
    p.claim(&batch("w-1", p.pending("w-1").await.unwrap()))
        .await
        .unwrap();
    p.patch_session(
        "w-1",
        SessionPatch {
            desired: Some(Desired::Running),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    p.settle(&[old, fresh], Settle::Requeue, Ms(10))
        .await
        .unwrap();
    let rows = p.list_wakes(Some("w-1")).await.unwrap();
    let old = rows.iter().find(|w| w.id == old).unwrap();
    let fresh = rows.iter().find(|w| w.id == fresh).unwrap();
    assert_eq!(
        (old.state, old.attempts, old.last_error.as_deref()),
        (WakeState::Dropped, 1, Some("stopped"))
    );
    assert_eq!((fresh.state, fresh.attempts), (WakeState::Pending, 0));
    assert_eq!(
        rows.iter().find(|w| w.id == pending).unwrap().state,
        WakeState::Dropped
    );
    assert_eq!(
        p.session("w-1").await.unwrap().unwrap().desired,
        Desired::Running
    );
}

async fn human_stop_boundary_separates_cancelled_and_fresh_failure_outcomes<W: World>() {
    let w = W::new().await;
    let p = w.port();
    let old = p.enqueue(timer("w-1", "old")).await.unwrap().unwrap();
    p.claim(&batch("w-1", p.pending("w-1").await.unwrap()))
        .await
        .unwrap();
    p.human_stop("w-1").await.unwrap();
    let fresh = p.enqueue(timer("w-1", "fresh")).await.unwrap().unwrap();
    p.claim(&batch("w-1", p.pending("w-1").await.unwrap()))
        .await
        .unwrap();
    let rows = p.list_wakes(Some("w-1")).await.unwrap();
    let out = p
        .ack(
            &batch("w-1", rows),
            Ack::Failed {
                error: "failure".into(),
                now: Ms(10),
                jitter: 0.0,
            },
        )
        .await
        .unwrap();
    assert_eq!(out.retried, [(fresh, Ms(30_010))]);
    assert!(out.gave_up.is_empty());
    let rows = p.list_wakes(Some("w-1")).await.unwrap();
    let old = rows.iter().find(|w| w.id == old).unwrap();
    assert_eq!(
        (old.state, old.attempts, old.last_error.as_deref()),
        (WakeState::Dropped, 1, Some("stopped"))
    );
    assert_eq!(
        rows.iter().find(|w| w.id == fresh).unwrap().state,
        WakeState::Pending
    );
}

/// Both adapters own a real timer batch, reject an old receipt after the next
/// incarnation is claimed, and keep the next attempt intact until its own receipt.
async fn reservation_receipt_is_exact_and_old_receipt_cannot_ack_next<W: World>() {
    let w = W::new().await;
    w.port()
        .enqueue(timer("w-1", "reserved-old"))
        .await
        .unwrap();
    let wakes = w.port().pending("w-1").await.unwrap();
    let rendered = w.port().render_wake("w-1", &wakes).await.unwrap();
    let mut old = Batch {
        session: "w-1".into(),
        wakes,
        cursor_targets: rendered.cursor_targets,
        prepared: rendered.prepared,
        ..Default::default()
    };
    old.lease = match w.port().claim_reserved(&old, 1).await.unwrap() {
        Decision::Applied(lease) => Some(lease),
        other => panic!("first claim: {other:?}"),
    };
    assert!(matches!(
        w.port().claim_reserved(&old, 1).await.unwrap(),
        Decision::Stale | Decision::Busy
    ));
    assert!(matches!(
        w.port()
            .settle_batch(&old, Settle::Requeue, w.now())
            .await
            .unwrap(),
        Decision::Applied(())
    ));
    let wakes = w.port().pending("w-1").await.unwrap();
    let rendered = w.port().render_wake("w-1", &wakes).await.unwrap();
    let mut next = Batch {
        session: "w-1".into(),
        wakes,
        cursor_targets: rendered.cursor_targets,
        prepared: rendered.prepared,
        ..Default::default()
    };
    next.lease = match w.port().claim_reserved(&next, 2).await.unwrap() {
        Decision::Applied(lease) => Some(lease),
        other => panic!("second claim: {other:?}"),
    };
    assert!(
        w.port()
            .ack(
                &old,
                Ack::Delivered {
                    riders: Riders::Release
                }
            )
            .await
            .unwrap()
            .stale
    );
    assert!(w.port().pending("w-1").await.unwrap().is_empty());
    let rows = w.port().list_wakes(Some("w-1")).await.unwrap();
    assert_eq!(rows[0].state, WakeState::Inflight);
    assert_eq!(rows[0].attempts, 1);
    assert!(
        !w.port()
            .ack(
                &next,
                Ack::Delivered {
                    riders: Riders::Release
                }
            )
            .await
            .unwrap()
            .stale
    );
    assert_eq!(
        w.port().list_wakes(Some("w-1")).await.unwrap()[0].state,
        WakeState::Delivered
    );
}

/// Pure backlog still owns the cursor even though no wake ID exists; boot reset
/// invalidates its receipt and permits a new actual rendered body to claim.
async fn zero_id_body_reservation_is_released_by_boot_and_old_ack_is_stale<W: World>() {
    let mut w = W::new().await;
    let channel = w.topic("c-1", "cap-1", &["w-1"]).await;
    w.port().unread("w-1").await.unwrap();
    let m = w.post(Who::Human, &channel, &[]).await;
    let queued = QueuedWake {
        id: 0,
        session: "w-1".into(),
        kind: WakeKind::Message,
        source: source::message(&m.id),
        priority: Priority::Human,
        not_before: Ms(0),
        note: None,
        origin: HUMAN.into(),
        state: WakeState::Pending,
        attempts: 0,
        created_at: w.now(),
        last_error: None,
        msg: Some(m.at.clone()),
    };
    // Renderer needs an actual queued row in DbPort, so enqueue normally, render,
    // then select the true cursor-only prefix rather than manufacturing a snapshot.
    w.port()
        .enqueue(NewWake {
            session: queued.session.clone(),
            kind: queued.kind,
            source: queued.source.clone(),
            priority: queued.priority,
            not_before: queued.not_before,
            note: None,
            origin: queued.origin.clone(),
        })
        .await
        .unwrap();
    let pending = w.port().pending("w-1").await.unwrap();
    let r = w.port().render_wake("w-1", &pending).await.unwrap();
    assert!(r.cursor_targets.contains(&(channel.clone(), m.at.seq)));
    let mut body = Batch {
        session: "w-1".into(),
        cursor_targets: r.cursor_targets,
        prepared: r.prepared,
        ..Default::default()
    };
    body.lease = match w.port().claim_reserved(&body, 3).await.unwrap() {
        Decision::Applied(lease) => Some(lease),
        other => panic!("zero ID claim: {other:?}"),
    };
    // Same-channel suffix is held behind the unconfirmed prefix.
    w.post(Who::Human, &channel, &[]).await;
    assert!(
        w.port()
            .unread("w-1")
            .await
            .unwrap()
            .iter()
            .all(|m| m.at.channel != channel)
    );
    w.port()
        .fail_inflight("synthetic boot", w.now())
        .await
        .unwrap();
    assert!(
        w.port()
            .ack(
                &body,
                Ack::Delivered {
                    riders: Riders::Release
                }
            )
            .await
            .unwrap()
            .stale
    );
    assert!(w.port().cursor("w-1", &channel).await.unwrap() < m.at.seq);
    let r = w.port().render_wake("w-1", &pending).await.unwrap();
    let retry = Batch {
        session: "w-1".into(),
        cursor_targets: r.cursor_targets,
        prepared: r.prepared,
        ..Default::default()
    };
    assert!(matches!(
        w.port().claim_reserved(&retry, 4).await.unwrap(),
        Decision::Applied(_)
    ));
}

async fn prepared_body_rechecks_archived_or_expired_injection_surface<W: World>() {
    for expired in [false, true] {
        let mut w = W::new().await;
        let ch = w
            .topic("c-1", "cap-1", if expired { &[] } else { &["w-1"] })
            .await;
        if expired {
            w.listen("w-1", &ch, None).await;
        }
        w.port().unread("w-1").await.unwrap();
        let m = w.post(Who::Human, &ch, &[]).await;
        w.port()
            .enqueue(NewWake {
                session: "w-1".into(),
                kind: WakeKind::Message,
                source: source::message(&m.id),
                priority: Priority::Human,
                not_before: Ms(0),
                note: None,
                origin: HUMAN.into(),
            })
            .await
            .unwrap();
        let pending = w.port().pending("w-1").await.unwrap();
        let r = w.port().render_wake("w-1", &pending).await.unwrap();
        assert!(
            r.cursor_targets.contains(&(ch.clone(), m.at.seq)),
            "must prepare a real readable prefix"
        );
        let b = Batch {
            session: "w-1".into(),
            wakes: pending,
            cursor_targets: r.cursor_targets,
            prepared: r.prepared,
            ..Default::default()
        };
        let cursor = w.port().cursor("w-1", &ch).await.unwrap();
        if expired {
            w.expire_listen("w-1", &ch).await;
        } else {
            w.archive_channel(&ch).await;
        }
        assert!(
            matches!(
                w.port().claim_reserved(&b, 1).await.unwrap(),
                Decision::Stale
            ),
            "expired={expired}"
        );
        assert_eq!(w.port().cursor("w-1", &ch).await.unwrap(), cursor);
        assert!(
            w.port()
                .list_wakes(Some("w-1"))
                .await
                .unwrap()
                .iter()
                .all(|r| r.attempts == 0)
        );
    }
}

macro_rules! both {
    ($($case:ident),* $(,)?) => {
        mod mem {
            use super::*;
            $(#[tokio::test] async fn $case() { super::$case::<MemWorld>().await })*
        }
        mod store {
            use super::*;
            $(#[tokio::test] async fn $case() { super::$case::<DbWorld>().await })*
        }
    };
}

both!(
    prepared_body_rechecks_archived_or_expired_injection_surface,
    reservation_receipt_is_exact_and_old_receipt_cannot_ack_next,
    zero_id_body_reservation_is_released_by_boot_and_old_ack_is_stale,
    human_stop_boundary_survives_new_work_and_mixed_requeue,
    human_stop_boundary_separates_cancelled_and_fresh_failure_outcomes,
    enqueue_dedupes,
    enqueue_for_a_removed_crew_is_nothing,
    pending_is_by_priority_then_age,
    claim_is_all_or_nothing_and_a_row_dropped_meanwhile_conflicts,
    ack_delivered_moves_cursors_in_the_same_transaction,
    ack_failed_counts_by_the_stored_attempts,
    fail_inflight_hands_back_old_given_up_rows,
    due_sessions_are_by_top_priority_then_oldest,
    next_not_before_skips_ones_already_due,
    list_wakes_is_the_newest_capped_rows,
    latches_round_trip_both_scopes,
    origins_round_trip,
    channel_ctx_fills_all_six_fields,
    a_child_crews_human_notice_lands_in_its_homes_human_channel,
    injection_and_planner_agree_on_every_message,
    wakes_for_message_lists_each_target_in_order,
    settings_round_trip,
    an_archived_dm_has_no_members_and_no_parties,
    settle_delivered_and_requeue_follow_the_table,
    ack_delivered_releases_or_drops_riders_and_checks_ownership,
    prune_wakes_only_clears_rows_settled_before_the_cutoff,
    post_system_mentions_the_owner_only_when_asked,
    message_with_the_wrong_channel_is_none,
    crew_removed_of_an_unknown_crew_is_not_found,
);
