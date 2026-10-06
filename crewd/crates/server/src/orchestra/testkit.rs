//! 测试器材：内存里的 Port 与顶替执行器。只在 `cargo test` 里编译。
#![allow(dead_code)]
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crewd_exec::driver::{Closed, SendError, SessionStatus};
use crewd_exec::resume::{AgentIdLedger, MemoryLedger};
use crewd_exec::session::SessionEvent;
use crewd_term::Snapshot;
use crewd_wake::plan::{ChannelCtx, Listen, MsgFacts, visible_to};
use crewd_wake::queue::is_due;
use crewd_wake::{Latch, LatchScope, MessageRef, Ms, NewWake, QueuedWake, WakeKind, WakeState};
use tokio::sync::mpsc;

use super::exec::{
    BoxFut, Context, ExecHandle, ExecSendError, Executor, LaunchError, LaunchRequest, Launched,
    check_context,
};
use super::port::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemKind {
    Lobby,
    Human,
    Other,
}

#[derive(Clone, Debug)]
pub struct MemChannel {
    pub crew: String,
    pub kind: MemKind,
    /// 只对 `Other` 有意义；大厅 / 人类群的群主 = 机组机长。
    pub owner: Option<String>,
    pub members: Vec<String>,
}

#[derive(Default)]
pub struct MemState {
    pub sessions: BTreeMap<String, SessionRow>,
    pub wakes: Vec<QueuedWake>,
    pub latches: Vec<Latch>,
    pub channels: BTreeMap<String, MemChannel>,
    pub messages: BTreeMap<(String, i64), (MsgFacts, String)>,
    pub cursors: BTreeMap<(String, String), i64>,
    pub listens: BTreeMap<String, Vec<Listen>>,
    pub posts: Vec<(Audience, String)>,
    pub events: Vec<OrchestraEvent>,
    pub max_live: Option<usize>,
    pub restarts: Vec<Vec<String>>,
    /// 重启询问 id → 答复（`true` = 恢复）；测试里直接写，模拟「人答了但 `RestartAnswered` 事件丢了」。
    pub restart_answers: BTreeMap<i64, bool>,
    pub defaults_applied: usize,
    pub prunes: usize,
    pub ledgers: BTreeMap<String, Arc<MemoryLedger>>,
    /// 机组 → 工作目录（`Port::crew_workdir`）。
    pub crew_dirs: BTreeMap<String, PathBuf>,
    pub next_wake: i64,
    pub next_session: u64,
    /// 测试里的「现在」（入队时间）。
    pub now: Ms,
    /// 模拟 M3a R6 的窗口：列在这里的唤醒 id 由 `render_wake` 报成 deferred（不进正文）。
    pub defer: BTreeSet<i64>,
    /// 模拟窗口里比被叫消息更旧的积压：session → [(群, seq)]，进正文、并入 `cursor_targets`。
    pub backlog: BTreeMap<String, Vec<(String, i64)>>,
    /// `revoke_token` 调过的 session（按调用顺序）。
    pub revoked: Vec<String>,
}

pub struct MemPort {
    pub st: Mutex<MemState>,
}

impl MemPort {
    pub fn new() -> Self {
        Self {
            st: Mutex::new(MemState::default()),
        }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, MemState> {
        self.st.lock().expect("测试器材锁中毒")
    }

    fn captain_of(st: &MemState, crew: &str) -> Option<String> {
        st.sessions
            .values()
            .find(|s| s.crew_id == crew && s.captain)
            .map(|s| s.id.clone())
    }

    /// 填法同控制者裁定 C22 对 DbPort 的要求：`live_sessions` = 库里全部 session（有 session 就不会空）、
    /// `crew_captain` = 这个群所属机组的机长；MemPort 没有 dm 群，`dm_peer` 恒为 None。
    fn ctx_of(st: &MemState, channel: &str) -> Option<ChannelCtx> {
        let ch = st.channels.get(channel)?;
        let captain = Self::captain_of(st, &ch.crew);
        let (members, owner) = match ch.kind {
            MemKind::Lobby => (
                st.sessions
                    .values()
                    .filter(|s| s.crew_id == ch.crew)
                    .map(|s| s.id.clone())
                    .collect(),
                captain.clone(),
            ),
            MemKind::Human => {
                // 机长 + 显式成员，去重（机长同时是显式成员时只算一次，同 M3a `channels::members`）。
                let mut m: Vec<String> = captain.iter().cloned().collect();
                for s in &ch.members {
                    if !m.contains(s) {
                        m.push(s.clone());
                    }
                }
                (m, captain.clone())
            }
            MemKind::Other => (ch.members.clone(), ch.owner.clone()),
        };
        Some(ChannelCtx {
            session_members: members,
            live_sessions: st.sessions.keys().cloned().collect(),
            owner,
            dm_peer: None,
            crew_captain: captain,
            listens: st.listens.get(channel).cloned().unwrap_or_default(),
        })
    }

    /// 群里最大的 seq（空群 = 0）。
    fn end_of(st: &MemState, channel: &str) -> i64 {
        st.messages
            .range((channel.to_string(), i64::MIN)..=(channel.to_string(), i64::MAX))
            .map(|((_, seq), _)| *seq)
            .max()
            .unwrap_or(0)
    }

    /// 入群点（M3a 裁定 9）：没有游标行就把它钉到群的当前末尾；已有的不动。
    pub fn join(&self, session: &str, channel: &str) {
        let mut st = self.lock();
        let end = Self::end_of(&st, channel);
        st.cursors
            .entry((session.to_string(), channel.to_string()))
            .or_insert(end);
    }

    pub fn wakes_of(&self, session: &str) -> Vec<QueuedWake> {
        self.lock()
            .wakes
            .iter()
            .filter(|w| w.session == session)
            .cloned()
            .collect()
    }

    pub fn posts(&self) -> Vec<(Audience, String)> {
        self.lock().posts.clone()
    }

    pub fn row(&self, id: &str) -> SessionRow {
        self.lock()
            .sessions
            .get(id)
            .cloned()
            .expect("测试里先建 session")
    }
}

impl Port for MemPort {
    async fn session(&self, id: &str) -> Result<Option<SessionRow>> {
        Ok(self.lock().sessions.get(id).cloned())
    }

    async fn sessions_desired_running(&self) -> Result<Vec<SessionRow>> {
        Ok(self
            .lock()
            .sessions
            .values()
            .filter(|s| s.desired == Desired::Running)
            .cloned()
            .collect())
    }

    async fn all_session_ids(&self) -> Result<Vec<String>> {
        Ok(self.lock().sessions.keys().cloned().collect())
    }

    async fn crew_workdir(&self, crew: &str) -> Result<Option<PathBuf>> {
        Ok(self.lock().crew_dirs.get(crew).cloned())
    }

    async fn insert_session(&self, s: NewSession) -> Result<SessionRow> {
        let mut st = self.lock();
        st.next_session += 1;
        let shared = st.crew_dirs.get(&s.crew_id).cloned();
        let row = SessionRow {
            id: format!("new-{}", st.next_session),
            crew_id: s.crew_id,
            captain: s.captain,
            executor: s.executor,
            display_name: s.title,
            agent_session_id: None,
            // worktree（isolation=true 建好的）→ 机组共享目录。
            workdir: Some(
                s.worktree
                    .or(shared)
                    .unwrap_or_else(|| PathBuf::from("/nonexistent/wd")),
            ),
            model: s.model,
            effort: s.effort,
            brief: Some(s.brief),
            desired: Desired::Stopped,
            was_working: false,
            last_active_at: None,
            resume_failed_at: None,
            awaiting_question: None,
            last_progress_at: None,
        };
        // 同 M3a `sessions::register`：新 session 的大厅游标钉在当前末尾（机长另加人类群，同 `point_captain_at`）。
        let joined: Vec<String> = st
            .channels
            .iter()
            .filter(|(_, c)| {
                c.crew == row.crew_id
                    && (c.kind == MemKind::Lobby || (row.captain && c.kind == MemKind::Human))
            })
            .map(|(id, _)| id.clone())
            .collect();
        for ch in joined {
            let end = Self::end_of(&st, &ch);
            st.cursors.insert((row.id.clone(), ch), end);
        }
        st.sessions.insert(row.id.clone(), row.clone());
        Ok(row)
    }

    async fn patch_session(&self, id: &str, p: SessionPatch) -> Result<()> {
        let mut st = self.lock();
        let row = st
            .sessions
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("没有 session {id}"))?;
        if let Some(v) = p.desired {
            row.desired = v;
        }
        if let Some(v) = p.was_working {
            row.was_working = v;
        }
        if let Some(v) = p.last_active_at {
            row.last_active_at = Some(v);
        }
        if let Some(v) = p.resume_failed_at {
            row.resume_failed_at = v;
        }
        if let Some(v) = p.model {
            row.model = Some(v);
        }
        if let Some(v) = p.effort {
            row.effort = Some(v);
        }
        Ok(())
    }

    async fn set_captain(&self, crew: &str, session: &str) -> Result<Option<String>> {
        let mut st = self.lock();
        let prev = Self::captain_of(&st, crew).filter(|p| p != session);
        for s in st.sessions.values_mut().filter(|s| s.crew_id == crew) {
            s.captain = s.id == session;
        }
        Ok(prev)
    }

    async fn captain(&self, crew: &str) -> Result<Option<String>> {
        Ok(Self::captain_of(&self.lock(), crew))
    }

    fn ledger(&self, session: &str) -> Arc<dyn AgentIdLedger> {
        self.lock()
            .ledgers
            .entry(session.to_string())
            .or_default()
            .clone()
    }

    async fn worldview(&self, session: &str) -> Result<String> {
        Ok(format!("WORLD {session}"))
    }

    fn mint_token(&self, session: &str) -> String {
        format!("tok-{session}")
    }

    fn revoke_token(&self, session: &str) {
        self.lock().revoked.push(session.to_string());
    }

    async fn enqueue(&self, w: NewWake) -> Result<bool> {
        let mut st = self.lock();
        if st
            .wakes
            .iter()
            .any(|q| q.session == w.session && q.kind == w.kind && q.source == w.source)
        {
            return Ok(false);
        }
        st.next_wake += 1;
        let (id, now) = (st.next_wake, st.now);
        // `msg:<id>` → 那条消息的位置（同 M3a：消息 id 唯一）。
        let msg = w.source.strip_prefix("msg:").and_then(|mid| {
            st.messages
                .values()
                .find(|(m, _)| m.id == mid)
                .map(|(m, _)| m.at.clone())
        });
        st.wakes.push(QueuedWake {
            id,
            session: w.session,
            kind: w.kind,
            source: w.source,
            priority: w.priority,
            not_before: w.not_before,
            note: w.note,
            state: WakeState::Pending,
            attempts: 0,
            created_at: now,
            origin: w.origin,
            msg,
        });
        Ok(true)
    }

    async fn pending(&self, session: &str) -> Result<Vec<QueuedWake>> {
        Ok(self
            .lock()
            .wakes
            .iter()
            .filter(|w| w.session == session && w.state == WakeState::Pending)
            .cloned()
            .collect())
    }

    async fn settle(&self, ids: &[i64], s: Settle, _now: Ms) -> Result<()> {
        let mut st = self.lock();
        for w in st.wakes.iter_mut().filter(|w| ids.contains(&w.id)) {
            match &s {
                Settle::Inflight => w.state = WakeState::Inflight,
                Settle::Delivered => w.state = WakeState::Delivered,
                Settle::Retry {
                    attempts, retry_at, ..
                } => {
                    w.state = WakeState::Pending;
                    w.attempts = *attempts;
                    w.not_before = *retry_at;
                }
                Settle::Dropped { .. } => w.state = WakeState::Dropped,
                Settle::Requeue => {
                    if w.state == WakeState::Inflight {
                        w.state = WakeState::Pending;
                    }
                }
            }
        }
        Ok(())
    }

    async fn requeue_inflight(&self) -> Result<usize> {
        let mut st = self.lock();
        let mut n = 0;
        for w in st
            .wakes
            .iter_mut()
            .filter(|w| w.state == WakeState::Inflight)
        {
            w.state = WakeState::Pending;
            n += 1;
        }
        Ok(n)
    }

    async fn due_sessions(&self, now: Ms) -> Result<Vec<String>> {
        let st = self.lock();
        let mut best: BTreeMap<String, (crewd_wake::Priority, i64)> = BTreeMap::new();
        for w in st.wakes.iter().filter(|w| is_due(w, now)) {
            let e = best.entry(w.session.clone()).or_insert((w.priority, w.id));
            e.0 = e.0.max(w.priority);
            e.1 = e.1.min(w.id);
        }
        let mut v: Vec<(String, (crewd_wake::Priority, i64))> = best.into_iter().collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
        Ok(v.into_iter().map(|(s, _)| s).collect())
    }

    async fn next_not_before_after(&self, now: Ms) -> Result<Option<Ms>> {
        Ok(self
            .lock()
            .wakes
            .iter()
            .filter(|w| w.state == WakeState::Pending && w.not_before > now)
            .map(|w| w.not_before)
            .min())
    }

    async fn list_wakes(&self, session: Option<&str>) -> Result<Vec<QueuedWake>> {
        Ok(self
            .lock()
            .wakes
            .iter()
            .filter(|w| session.is_none_or(|s| w.session == s))
            .cloned()
            .collect())
    }

    async fn latches(&self) -> Result<Vec<Latch>> {
        Ok(self.lock().latches.clone())
    }

    async fn put_latch(&self, l: &Latch) -> Result<()> {
        let mut st = self.lock();
        st.latches.retain(|x| (x.scope, &x.id) != (l.scope, &l.id));
        st.latches.push(l.clone());
        Ok(())
    }

    async fn clear_latch(&self, scope: LatchScope, id: &str) -> Result<()> {
        self.lock()
            .latches
            .retain(|x| !(x.scope == scope && x.id == id));
        Ok(())
    }

    async fn max_live(&self) -> Result<Option<usize>> {
        Ok(self.lock().max_live)
    }

    async fn set_max_live(&self, max: Option<usize>) -> Result<()> {
        self.lock().max_live = max;
        Ok(())
    }

    async fn locale(&self) -> Result<crewd_wake::text::Lang> {
        Ok(crewd_wake::text::Lang::Zh)
    }

    async fn open_restart(&self, session_ids: Vec<String>) -> Result<Option<i64>> {
        let mut st = self.lock();
        if session_ids.is_empty() {
            return Ok(None);
        }
        st.restarts.push(session_ids);
        Ok(Some(st.restarts.len() as i64))
    }

    async fn restart_answer(&self, id: i64) -> Result<Option<bool>> {
        Ok(self.lock().restart_answers.get(&id).copied())
    }

    async fn apply_due_defaults(&self) -> Result<()> {
        self.lock().defaults_applied += 1;
        Ok(())
    }

    async fn prune_wakes(&self, _before: Ms) -> Result<usize> {
        // MemPort 不记 settled 时刻：只数调用次数（测试只核「定时拍会调」）。
        self.lock().prunes += 1;
        Ok(0)
    }

    async fn message(&self, r: &MessageRef) -> Result<Option<(MsgFacts, String)>> {
        Ok(self
            .lock()
            .messages
            .get(&(r.channel.clone(), r.seq))
            .cloned())
    }

    async fn channel_ctx(&self, channel: &str, _now: Ms) -> Result<Option<ChannelCtx>> {
        // MemPort 原样给全部收听；过期的由 `plan` 按 `until > now` 再滤一次（与 DbPort 结果一致）。
        Ok(Self::ctx_of(&self.lock(), channel))
    }

    /// 同 M3a `cursors::unread_for`：所在的群 + 此刻还在收听的群；游标之后、对它可见（`plan::visible_to`）、
    /// 不是它自己发的。没有游标行的群 = 入群点没记：先钉到群末尾、本次不返回（M3a 在入群时钉，`Rig` 也是）。
    /// 不模拟的：`UNREAD_CAP` 窗口与「开头一段不可投递的消息让游标跳过」（只影响游标，不影响返回什么）。
    async fn unread(&self, session: &str) -> Result<Vec<MsgFacts>> {
        let mut st = self.lock();
        let now = st.now;
        let mut chans: Vec<String> = st
            .channels
            .keys()
            .filter(|ch| {
                Self::ctx_of(&st, ch)
                    .is_some_and(|c| c.session_members.iter().any(|s| s == session))
            })
            .cloned()
            .collect();
        for (ch, ls) in &st.listens {
            if ls.iter().any(|l| l.session == session && l.until > now) && !chans.contains(ch) {
                chans.push(ch.clone());
            }
        }
        let mut out = Vec::new();
        for ch in chans {
            let key = (session.to_string(), ch.clone());
            let Some(after) = st.cursors.get(&key).copied() else {
                let end = Self::end_of(&st, &ch);
                st.cursors.insert(key, end);
                continue;
            };
            out.extend(
                st.messages
                    .range((ch.clone(), after + 1)..=(ch.clone(), i64::MAX))
                    .map(|(_, (m, _))| m)
                    .filter(|m| m.sender.session_id() != Some(session) && visible_to(m, session))
                    .cloned(),
            );
        }
        Ok(out)
    }

    async fn cursor(&self, session: &str, channel: &str) -> Result<i64> {
        Ok(self
            .lock()
            .cursors
            .get(&(session.to_string(), channel.to_string()))
            .copied()
            .unwrap_or(0))
    }

    async fn advance(&self, session: &str, channel: &str, seq: i64) -> Result<()> {
        let mut st = self.lock();
        let c = st
            .cursors
            .entry((session.to_string(), channel.to_string()))
            .or_insert(0);
        *c = (*c).max(seq);
        Ok(())
    }

    async fn post_system(&self, to: Audience, text: String) -> Result<()> {
        self.lock().posts.push((to, text));
        Ok(())
    }

    async fn render_wake(&self, session: &str, batch: &[QueuedWake]) -> Result<Rendered> {
        let st = self.lock();
        let deferred: Vec<i64> = batch
            .iter()
            .filter(|w| st.defer.contains(&w.id))
            .map(|w| w.id)
            .collect();
        let shown: Vec<&QueuedWake> = batch.iter().filter(|w| !st.defer.contains(&w.id)).collect();
        let backlog = st.backlog.get(session).cloned().unwrap_or_default();
        let mut parts: Vec<String> = backlog
            .iter()
            .map(|(ch, seq)| format!("backlog {ch}#{seq}"))
            .collect();
        parts.extend(shown.iter().filter_map(|w| {
            match w.kind {
                WakeKind::Message => w
                    .msg
                    .as_ref()
                    .and_then(|r| st.messages.get(&(r.channel.clone(), r.seq)))
                    .map(|(_, text)| text.clone()),
                _ => Some(
                    w.note
                        .clone()
                        .unwrap_or_else(|| format!("{}:{}", w.kind, w.source)),
                ),
            }
        }));
        // 与 M3a `cursor_targets()` 同义：每群推到正文里实际带上的最大 seq。
        let mut tops: BTreeMap<String, i64> = BTreeMap::new();
        let shown_msgs = shown
            .iter()
            .filter(|w| w.kind == WakeKind::Message)
            .filter_map(|w| w.msg.as_ref());
        for (ch, seq) in backlog
            .into_iter()
            .chain(shown_msgs.map(|r| (r.channel.clone(), r.seq)))
        {
            let top = tops.entry(ch).or_insert(seq);
            *top = (*top).max(seq);
        }
        Ok(Rendered {
            text: parts.join("\n---\n"),
            dropped: vec![],
            deferred,
            cursor_targets: tops.into_iter().collect(),
        })
    }

    fn publish(&self, e: OrchestraEvent) {
        self.lock().events.push(e);
    }
}

#[derive(Clone, Debug)]
pub struct LaunchRecord {
    pub session: String,
    pub workdir: PathBuf,
    pub initial: Option<String>,
    pub system_prompt: Option<String>,
    pub token: String,
}

#[derive(Default)]
pub struct StandInState {
    pub launches: Vec<LaunchRecord>,
    /// 依次让下几次 launch 失败（原因）。
    pub fail_next: Vec<String>,
    pub sends: BTreeMap<String, Vec<(u64, String)>>,
    pub raws: BTreeMap<String, Vec<Vec<u8>>>,
    pub stops: Vec<String>,
    pub cleared_quota: Vec<String>,
    pub events: BTreeMap<String, mpsc::UnboundedSender<SessionEvent>>,
    pub transcripts: BTreeMap<String, PathBuf>,
    pub next_id: u64,
}

/// 顶替执行器：记下收到的一切；事件由测试经 [`StandIn::emit`] 推。launch 成功时自带一条 `Spawned`。
#[derive(Clone, Default)]
pub struct StandIn(pub Arc<Mutex<StandInState>>);

impl StandIn {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, StandInState> {
        self.0.lock().expect("测试器材锁中毒")
    }

    pub fn emit(&self, session: &str, e: SessionEvent) {
        let tx = self
            .lock()
            .events
            .get(session)
            .cloned()
            .expect("先 launch 再推事件");
        tx.send(e).expect("事件泵还在");
    }

    pub fn sent(&self, session: &str) -> Vec<String> {
        self.lock()
            .sends
            .get(session)
            .map(|v| v.iter().map(|(_, t)| t.clone()).collect())
            .unwrap_or_default()
    }

    pub fn last_send_id(&self, session: &str) -> u64 {
        self.lock()
            .sends
            .get(session)
            .and_then(|v| v.last())
            .map(|(id, _)| *id)
            .expect("还没 send 过")
    }

    pub fn launches(&self, session: &str) -> Vec<LaunchRecord> {
        self.lock()
            .launches
            .iter()
            .filter(|l| l.session == session)
            .cloned()
            .collect()
    }
}

/// 顶替 `Worktrees`：不碰 git，记下调用；可让下一次失败。
#[derive(Clone, Default)]
pub struct StandInWorktrees(pub Arc<Mutex<StandInWtState>>);

#[derive(Default)]
pub struct StandInWtState {
    pub calls: Vec<(PathBuf, String)>,
    pub fail_next: Option<String>,
}

impl StandInWorktrees {
    pub fn calls(&self) -> Vec<(PathBuf, String)> {
        self.0.lock().expect("测试器材锁中毒").calls.clone()
    }

    pub fn fail_next(&self, why: &str) {
        self.0.lock().expect("测试器材锁中毒").fail_next = Some(why.into());
    }
}

impl super::worktree::Worktrees for StandInWorktrees {
    fn create<'a>(
        &'a self,
        crew_dir: &'a std::path::Path,
        hint: &'a str,
    ) -> BoxFut<'a, Result<PathBuf, String>> {
        let mut st = self.0.lock().expect("测试器材锁中毒");
        st.calls.push((crew_dir.to_path_buf(), hint.to_string()));
        let n = st.calls.len();
        let r = match st.fail_next.take() {
            Some(why) => Err(why),
            None => Ok(PathBuf::from(format!("/nonexistent/worktrees/w-{n}"))),
        };
        Box::pin(async move { r })
    }
}

impl Executor for StandIn {
    fn launch(&self, req: LaunchRequest) -> BoxFut<'_, Result<Launched, LaunchError>> {
        let st = self.0.clone();
        Box::pin(async move {
            let mut s = st.lock().expect("测试器材锁中毒");
            if !s.fail_next.is_empty() {
                let why = s.fail_next.remove(0);
                return Err(LaunchError::Failed(why));
            }
            s.launches.push(LaunchRecord {
                session: req.session_id.clone(),
                workdir: req.workdir.clone(),
                initial: req.initial_prompt.clone(),
                system_prompt: req.system_prompt.clone(),
                token: req.token.clone(),
            });
            let (tx, rx) = mpsc::unbounded_channel();
            tx.send(SessionEvent::Spawned {
                pid: 4242,
                agent_session_id: format!("agent-{}", req.session_id),
                resumed: false,
            })
            .expect("刚建的通道");
            s.events.insert(req.session_id.clone(), tx);
            drop(s);
            let handle = StandInHandle {
                session: req.session_id,
                st,
            };
            Ok((Arc::new(handle) as Arc<dyn ExecHandle>, rx))
        })
    }

    fn locate_transcript(&self, id: &str) -> Option<PathBuf> {
        self.lock().transcripts.get(id).cloned()
    }
}

pub struct StandInHandle {
    session: String,
    st: Arc<Mutex<StandInState>>,
}

impl StandInHandle {
    fn lock(&self) -> std::sync::MutexGuard<'_, StandInState> {
        self.st.lock().expect("测试器材锁中毒")
    }
}

impl ExecHandle for StandInHandle {
    fn send(&self, text: String, context: &Context) -> Result<u64, ExecSendError> {
        check_context(self.accepts_context(), context)?;
        if text.trim().is_empty() {
            return Err(SendError::EmptyText.into());
        }
        let mut s = self.lock();
        s.next_id += 1;
        let id = s.next_id;
        s.sends
            .entry(self.session.clone())
            .or_default()
            .push((id, text));
        Ok(id)
    }

    fn raw(&self, bytes: Vec<u8>) -> Result<(), Closed> {
        self.lock()
            .raws
            .entry(self.session.clone())
            .or_default()
            .push(bytes);
        Ok(())
    }

    /// 像真驱动层一样：叫停之后发一条 `Exited { stopped_by_user: true }`。
    fn stop(&self) -> Result<(), Closed> {
        let mut s = self.lock();
        s.stops.push(self.session.clone());
        if let Some(tx) = s.events.get(&self.session) {
            let info = crewd_exec::session::ExitInfo {
                status: crewd_exec::session::ExitStatusKind::Signal(15),
                stopped_by_user: true,
                hit_limit: false,
            };
            let _ = tx.send(SessionEvent::Exited(info));
        }
        Ok(())
    }

    fn clear_quota_health(&self) -> Result<(), Closed> {
        self.lock().cleared_quota.push(self.session.clone());
        Ok(())
    }

    fn screen_text(&self, _max_lines: usize) -> BoxFut<'_, Result<String, Closed>> {
        Box::pin(async { Ok("合成屏幕".to_string()) })
    }

    fn snapshot(&self) -> BoxFut<'_, Result<Option<Snapshot>, Closed>> {
        Box::pin(async { Ok(None) })
    }

    fn status(&self) -> BoxFut<'_, Result<SessionStatus, Closed>> {
        Box::pin(async { Err(Closed) })
    }
}

#[cfg(test)]
mod self_tests {
    use super::*;
    use crewd_wake::plan::{Mention, Sender};
    use crewd_wake::{Priority, WakeKind};

    fn wake(session: &str, source: &str, priority: Priority) -> NewWake {
        NewWake {
            session: session.into(),
            kind: WakeKind::Message,
            source: source.into(),
            priority,
            not_before: Ms(0),
            note: None,
            origin: "cap".into(),
        }
    }

    #[tokio::test]
    async fn mem_port_dedupes_and_orders_due_sessions_by_priority_then_age() {
        let p = MemPort::new();
        assert!(p.enqueue(wake("a", "x#1", Priority::Auto)).await.unwrap());
        assert!(
            !p.enqueue(wake("a", "x#1", Priority::Auto)).await.unwrap(),
            "UNIQUE(session, kind, source)"
        );
        assert!(p.enqueue(wake("b", "x#2", Priority::Human)).await.unwrap());
        assert!(p.enqueue(wake("c", "x#3", Priority::Auto)).await.unwrap());
        assert_eq!(p.due_sessions(Ms(0)).await.unwrap(), ["b", "a", "c"]);
        let ids: Vec<i64> = p.pending("a").await.unwrap().iter().map(|w| w.id).collect();
        p.settle(&ids, Settle::Inflight, Ms(0)).await.unwrap();
        assert!(p.pending("a").await.unwrap().is_empty());
        assert_eq!(p.requeue_inflight().await.unwrap(), 1);
        assert_eq!(p.pending("a").await.unwrap().len(), 1);
    }

    fn msg(st: &mut MemState, ch: &str, seq: i64, sender: Sender, mentions: Vec<Mention>) {
        let facts = MsgFacts {
            id: format!("{ch}#{seq}"),
            at: MessageRef {
                channel: ch.into(),
                seq,
            },
            sender,
            mentions,
            created_at: Ms(0),
        };
        st.messages
            .insert((ch.into(), seq), (facts, format!("合成 {ch}#{seq}")));
    }

    fn ids_of(v: &[MsgFacts]) -> Vec<String> {
        v.iter().map(|m| m.id.clone()).collect()
    }

    /// Task 6 评审 (5)：MemPort 的 `unread` 与 M3a `cursors::unread_for` 同义——所在群 + 收听中的群、
    /// 游标之后、对它可见、不是它自己发的；没有游标行的群先钉到末尾、本次不返回。
    #[tokio::test]
    async fn mem_port_unread_mirrors_collab_unread_for() {
        let p = MemPort::new();
        let w1 = Sender::Session {
            id: "w1".into(),
            captain: false,
        };
        let cap = Sender::Session {
            id: "cap".into(),
            captain: true,
        };
        {
            let mut st = p.lock();
            st.now = Ms(100);
            for (id, captain) in [("cap", true), ("w1", false)] {
                st.sessions.insert(
                    id.into(),
                    SessionRow {
                        id: id.into(),
                        crew_id: "c1".into(),
                        captain,
                        executor: "claude".into(),
                        display_name: None,
                        agent_session_id: None,
                        workdir: None,
                        model: None,
                        effort: None,
                        brief: None,
                        desired: Desired::Stopped,
                        was_working: false,
                        last_active_at: None,
                        resume_failed_at: None,
                        awaiting_question: None,
                        last_progress_at: None,
                    },
                );
            }
            let other = |members: Vec<String>| MemChannel {
                crew: "c1".into(),
                kind: MemKind::Other,
                owner: None,
                members,
            };
            st.channels.insert(
                "c1/lobby".into(),
                MemChannel {
                    crew: "c1".into(),
                    kind: MemKind::Lobby,
                    owner: None,
                    members: vec![],
                },
            );
            st.channels.insert("c1/heard".into(), other(vec![]));
            st.channels.insert("c1/expired".into(), other(vec![]));
            st.channels
                .insert("c1/fresh".into(), other(vec!["w1".into()]));
            for ch in ["c1/lobby", "c1/heard", "c1/expired"] {
                st.cursors.insert(("w1".into(), ch.into()), 1);
            }
            let listen = |until| Listen {
                session: "w1".into(),
                until: Ms(until),
                senders: None,
            };
            st.listens.insert("c1/heard".into(), vec![listen(200)]);
            st.listens.insert("c1/expired".into(), vec![listen(100)]);
            msg(&mut st, "c1/lobby", 1, cap.clone(), vec![]); // 游标上（已读）
            msg(&mut st, "c1/lobby", 2, cap.clone(), vec![]); // 看得见
            msg(&mut st, "c1/lobby", 3, w1.clone(), vec![]); // 自己发的
            let to_cap = vec![Mention::Session("cap".into())];
            msg(&mut st, "c1/lobby", 4, Sender::Human, to_cap); // 收窄给别人
            let all = vec![Mention::Session("cap".into()), Mention::Broadcast];
            msg(&mut st, "c1/lobby", 5, Sender::Human, all); // broadcast 人人可见
            msg(&mut st, "c1/heard", 2, cap.clone(), vec![]); // 不在群里、但在收听
            msg(&mut st, "c1/expired", 2, cap.clone(), vec![]); // 收听已过期
            msg(&mut st, "c1/fresh", 1, cap.clone(), vec![]); // 没有游标行
        }
        let got = p.unread("w1").await.unwrap();
        // 顺序同 M3a：先所在的群（按 id），再收听中的群。
        assert_eq!(ids_of(&got), ["c1/lobby#2", "c1/lobby#5", "c1/heard#2"]);
        assert_eq!(
            p.lock().cursors.get(&("w1".into(), "c1/fresh".into())),
            Some(&1),
            "没有游标行 = 入群点：钉到末尾"
        );
        msg(&mut p.lock(), "c1/fresh", 2, cap, vec![]);
        let got = p.unread("w1").await.unwrap();
        assert!(ids_of(&got).contains(&"c1/fresh#2".to_string()), "{got:?}");
    }

    /// 同 M3a `sessions::register`：新 session 的大厅游标钉在当前末尾——入群之前的消息不算它的未读。
    #[tokio::test]
    async fn mem_port_insert_session_pins_its_lobby_like_register() {
        let p = MemPort::new();
        {
            let mut st = p.lock();
            st.channels.insert(
                "c1/lobby".into(),
                MemChannel {
                    crew: "c1".into(),
                    kind: MemKind::Lobby,
                    owner: None,
                    members: vec![],
                },
            );
            msg(&mut st, "c1/lobby", 1, Sender::Human, vec![]);
        }
        let row = p
            .insert_session(NewSession {
                crew_id: "c1".into(),
                captain: false,
                executor: "claude".into(),
                brief: "合成任务甲".into(),
                title: None,
                model: None,
                effort: None,
                worktree: None,
            })
            .await
            .unwrap();
        assert_eq!(p.cursor(&row.id, "c1/lobby").await.unwrap(), 1);
        msg(&mut p.lock(), "c1/lobby", 2, Sender::Human, vec![]);
        assert_eq!(ids_of(&p.unread(&row.id).await.unwrap()), ["c1/lobby#2"]);
    }

    /// 人类群的成员 = 机长 + 显式成员；机长同时是显式成员时只算一次（同 M3a `channels::members`）。
    #[tokio::test]
    async fn mem_port_human_channel_members_are_deduped() {
        let p = MemPort::new();
        let new = |captain: bool, title: &str| NewSession {
            crew_id: "c1".into(),
            captain,
            executor: "claude".into(),
            brief: "合成任务甲".into(),
            title: Some(title.into()),
            model: None,
            effort: None,
            worktree: None,
        };
        let cap = p.insert_session(new(true, "cap")).await.unwrap();
        let w1 = p.insert_session(new(false, "w1")).await.unwrap();
        p.lock().channels.insert(
            "c1/human".into(),
            MemChannel {
                crew: "c1".into(),
                kind: MemKind::Human,
                owner: None,
                members: vec![cap.id.clone(), w1.id.clone()],
            },
        );
        let ctx = p.channel_ctx("c1/human", Ms(0)).await.unwrap().unwrap();
        assert_eq!(ctx.session_members, [cap.id.clone(), w1.id.clone()]);
    }

    /// 顶替句柄与 claude 句柄同一条规矩：不收 context，给了就回错误（不 panic）。
    #[tokio::test]
    async fn stand_in_handle_refuses_context_with_an_error() {
        let exec = StandIn::default();
        let (h, _rx) = exec
            .launch(LaunchRequest {
                session_id: "s1".into(),
                crew_id: "c1".into(),
                workdir: PathBuf::from("/nonexistent/wd"),
                model: None,
                effort: None,
                system_prompt: None,
                initial_prompt: None,
                token: "tok".into(),
                ledger: Arc::new(MemoryLedger::default()),
            })
            .await
            .unwrap();
        let mut ctx = Context::new();
        ctx.insert("k".into(), "v".into());
        assert_eq!(
            h.send("合成".into(), &ctx),
            Err(ExecSendError::ContextUnsupported)
        );
        assert!(exec.sent("s1").is_empty());
    }

    /// 控制者裁定 C22 的填法在 MemPort 上同样成立：有 session 时 `live_sessions` 不空（含不在群里的），
    /// `crew_captain` 是这个群所属机组的机长（人建的群群主可能不是它）。
    #[tokio::test]
    async fn mem_port_channel_ctx_fills_live_sessions_and_crew_captain() {
        let p = MemPort::new();
        let new = |captain: bool, title: &str| NewSession {
            crew_id: "c1".into(),
            captain,
            executor: "claude".into(),
            brief: "合成任务甲".into(),
            title: Some(title.into()),
            model: None,
            effort: None,
            worktree: None,
        };
        let cap = p.insert_session(new(true, "cap")).await.unwrap();
        let w1 = p.insert_session(new(false, "w1")).await.unwrap();
        p.lock().channels.insert(
            "c1/side".into(),
            MemChannel {
                crew: "c1".into(),
                kind: MemKind::Other,
                owner: Some(w1.id.clone()),
                members: vec![w1.id.clone()],
            },
        );
        let ctx = p.channel_ctx("c1/side", Ms(0)).await.unwrap().unwrap();
        assert_eq!(ctx.session_members, std::slice::from_ref(&w1.id));
        assert_eq!(ctx.owner.as_deref(), Some(w1.id.as_str()));
        assert_eq!(ctx.crew_captain.as_deref(), Some(cap.id.as_str()));
        assert_eq!(
            ctx.live_sessions,
            [cap.id.clone(), w1.id.clone()],
            "不在群里的机长也是活 session"
        );
        assert_eq!(ctx.dm_peer, None);
    }
}
