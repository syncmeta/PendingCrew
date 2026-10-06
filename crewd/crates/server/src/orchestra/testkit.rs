//! 测试器材：内存里的 Port 与顶替执行器。只在 `cargo test` 里编译。
#![allow(dead_code)]
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crewd_exec::delivery::Subject;
use crewd_exec::driver::{Closed, SendError, SessionStatus};
use crewd_exec::resume::{AgentIdLedger, MemoryLedger};
use crewd_exec::session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent};
use crewd_term::{DecisionEvent, Snapshot};
use crewd_wake::plan::{ChannelCtx, Listen, Mention, MsgFacts, Sender, visible_to};
use crewd_wake::queue::is_due;
use crewd_wake::{Latch, LatchScope, MessageRef, Ms, NewWake, QueuedWake, WakeKind, WakeState};
use tokio::sync::{mpsc, oneshot};

use super::OrchestraConfig;
use super::clock::{Clock, ManualClock};
use super::core::{Cmd, CollabFact, Core, Input, Live, Origin};
use super::exec::{
    BoxFut, Context, ExecHandle, ExecSendError, Executor, Executors, LaunchError, LaunchRequest,
    Launched, check_context,
};
use super::hooks::Hooks;
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
    /// 设了就让 `render_wake` 报错（模拟 M3a `build_batch` 遇到坏账本 fail-loud）。
    pub render_error: Option<String>,
    /// `render_wake` 被调了几次（量「出错不原地紧循环重试」）。
    pub renders: usize,
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
        let mut st = self.lock();
        st.renders += 1;
        if let Some(e) = &st.render_error {
            anyhow::bail!("{e}");
        }
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
    /// 每次 `raw` 的时刻（与 `raws` 一一对应）：量 nudge 键之间的间隔。
    pub raw_at: BTreeMap<String, Vec<std::time::Instant>>,
    /// 写进会话的先后（`raw:<字节>` / `send:<正文>`），按 session：量 nudge / 投递有没有交错。
    pub writes: BTreeMap<String, Vec<String>>,
    pub stops: Vec<String>,
    pub cleared_quota: Vec<String>,
    pub events: BTreeMap<String, mpsc::UnboundedSender<SessionEvent>>,
    pub transcripts: BTreeMap<String, PathBuf>,
    pub next_id: u64,
    /// 驱动层的影子状态（同 M2b `session.rs` 的 tracker）：还没终局的投递号（含开场正文 `BRIEF_DELIVERY_ID`）、
    /// 在不在干活、有没有待决策框、进程在不在。停掉 / 退出时据它补发 `abandon()` 的那串事件。
    pub outstanding: BTreeMap<String, BTreeSet<u64>>,
    pub working: BTreeSet<String>,
    pub deciding: BTreeSet<String>,
    pub running: BTreeSet<String>,
}

impl StandInState {
    /// 推一条事件给 Core，顺带更新影子状态。`Exited` 之前先补 `abandon("进程退出")`（同 M2b `on_exit`）。
    fn push(&mut self, session: &str, e: SessionEvent) {
        match &e {
            SessionEvent::Working(on) => {
                if *on {
                    self.working.insert(session.to_string());
                } else {
                    self.working.remove(session);
                }
            }
            SessionEvent::Decision(DecisionEvent::Appeared(_)) => {
                self.deciding.insert(session.to_string());
            }
            SessionEvent::Decision(DecisionEvent::Cleared) => {
                self.deciding.remove(session);
            }
            SessionEvent::Delivery { id, outcome, .. } if outcome.is_final() => {
                if let Some(o) = self.outstanding.get_mut(session) {
                    o.remove(id);
                }
            }
            SessionEvent::Exited(_) => {
                if self.running.remove(session) {
                    self.abandon(session, "进程退出");
                }
            }
            _ => {}
        }
        if let Some(tx) = self.events.get(session) {
            let _ = tx.send(e);
        }
    }

    /// 同 M2b `abandon()`：每笔没终局的投递一条 `Delivery{Abandoned}`，待决策框清掉，在干活的发 `Working(false)`。
    fn abandon(&mut self, session: &str, reason: &str) {
        let lost = self.outstanding.remove(session).unwrap_or_default();
        for id in lost {
            let subject = if id == BRIEF_DELIVERY_ID {
                Subject::Brief
            } else {
                Subject::Steer
            };
            let outcome = DeliveryOutcome::Abandoned(reason.into());
            self.push(
                session,
                SessionEvent::Delivery {
                    id,
                    subject,
                    outcome,
                },
            );
        }
        if self.deciding.contains(session) {
            self.push(session, SessionEvent::Decision(DecisionEvent::Cleared));
        }
        if self.working.contains(session) {
            self.push(session, SessionEvent::Working(false));
        }
    }
}

/// 顶替执行器：记下收到的一切；事件由测试经 [`StandIn::emit`] 推。launch 成功时自带一条 `Spawned`。
#[derive(Clone, Default)]
pub struct StandIn(pub Arc<Mutex<StandInState>>);

impl StandIn {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, StandInState> {
        self.0.lock().expect("测试器材锁中毒")
    }

    /// 以驱动层的身份推一条事件（`Exited` 之前自动补 `abandon` 的那串事件，同真驱动）。
    pub fn emit(&self, session: &str, e: SessionEvent) {
        let mut st = self.lock();
        assert!(st.events.contains_key(session), "先 launch 再推事件");
        st.push(session, e);
    }

    /// 事件流断了、没有 `Exited`（驱动任务没了）：丢掉发送端。
    pub fn end_stream(&self, session: &str) {
        let mut st = self.lock();
        st.events.remove(session);
        st.running.remove(session);
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
    /// 下一次 `create` 等这个信号才返回（模拟慢的 `git worktree add`）。
    pub hold: Option<oneshot::Receiver<()>>,
}

impl StandInWorktrees {
    pub fn calls(&self) -> Vec<(PathBuf, String)> {
        self.0.lock().expect("测试器材锁中毒").calls.clone()
    }

    pub fn fail_next(&self, why: &str) {
        self.0.lock().expect("测试器材锁中毒").fail_next = Some(why.into());
    }

    /// 让下一次 `create` 卡住，直到返回的发送端被触发（或丢掉）。
    pub fn hold_next(&self) -> oneshot::Sender<()> {
        let (tx, rx) = oneshot::channel();
        self.0.lock().expect("测试器材锁中毒").hold = Some(rx);
        tx
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
        let hold = st.hold.take();
        Box::pin(async move {
            if let Some(h) = hold {
                let _ = h.await;
            }
            r
        })
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
            s.running.insert(req.session_id.clone());
            let outstanding = s.outstanding.entry(req.session_id.clone()).or_default();
            outstanding.clear();
            if req.initial_prompt.is_some() {
                outstanding.insert(BRIEF_DELIVERY_ID);
            }
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
        s.outstanding
            .entry(self.session.clone())
            .or_default()
            .insert(id);
        let last = s.sends[&self.session].last().map(|(_, t)| t.clone());
        s.writes
            .entry(self.session.clone())
            .or_default()
            .push(format!("send:{}", last.unwrap_or_default()));
        Ok(id)
    }

    fn raw(&self, bytes: Vec<u8>) -> Result<(), Closed> {
        let mut s = self.lock();
        s.raws.entry(self.session.clone()).or_default().push(bytes);
        s.raw_at
            .entry(self.session.clone())
            .or_default()
            .push(std::time::Instant::now());
        let shown = String::from_utf8_lossy(s.raws[&self.session].last().expect("刚写的"))
            .escape_default()
            .to_string();
        s.writes
            .entry(self.session.clone())
            .or_default()
            .push(format!("raw:{shown}"));
        Ok(())
    }

    /// 像真驱动层一样（M2b `stop` + `on_exit`）：先 `abandon("session 被停掉")`（没终局的投递 → Abandoned、
    /// 待决策框清掉、`Working(false)`），再 `Exited { stopped_by_user: true }`。
    fn stop(&self) -> Result<(), Closed> {
        let mut s = self.lock();
        s.stops.push(self.session.clone());
        if s.running.remove(&self.session) {
            s.abandon(&self.session, "session 被停掉");
            let info = crewd_exec::session::ExitInfo {
                status: crewd_exec::session::ExitStatusKind::Signal(15),
                stopped_by_user: true,
                hit_limit: false,
            };
            s.push(&self.session, SessionEvent::Exited(info));
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

pub const T0: Ms = Ms(1_000_000_000);

/// Core 的测试台：内存 Port、顶替执行器、手动时钟、顶替 worktree；不跑 actor 循环，测试直接 `core.handle(..)`。
pub struct Rig {
    pub core: Core<MemPort>,
    pub port: Arc<MemPort>,
    pub exec: StandIn,
    pub clock: Arc<ManualClock>,
    pub worktrees: StandInWorktrees,
    pub rx: mpsc::UnboundedReceiver<Input>,
}

impl Rig {
    pub async fn new() -> Self {
        Self::with(Hooks::default()).await
    }

    pub async fn with(hooks: Hooks) -> Self {
        Self::with_cfg(hooks, OrchestraConfig::for_tests()).await
    }

    /// 控制者裁定 C7：不管调用方给什么 `Hooks`，worktree 一律换成顶替的——单元测试永不调真 git。
    pub async fn with_cfg(mut hooks: Hooks, cfg: OrchestraConfig) -> Self {
        let port = Arc::new(MemPort::new());
        let exec = StandIn::default();
        let worktrees = StandInWorktrees::default();
        hooks.worktrees = Arc::new(worktrees.clone());
        let clock = Arc::new(ManualClock::new(T0));
        port.lock().now = T0;
        let (tx, rx) = mpsc::unbounded_channel();
        let execs = Executors::new().with("claude", Arc::new(exec.clone()));
        let core = Core::new(port.clone(), execs, hooks, clock.clone(), cfg, tx)
            .await
            .expect("内存 Port 不会失败");
        Self {
            core,
            port,
            exec,
            clock,
            worktrees,
            rx,
        }
    }

    fn row(id: &str, crew: &str, captain: bool) -> SessionRow {
        SessionRow {
            id: id.into(),
            crew_id: crew.into(),
            captain,
            executor: "claude".into(),
            display_name: Some(id.into()),
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
        }
    }

    /// 机组 + 机长 + 大厅 `<crew>/lobby` + 人类群 `<crew>/human`。session 都没在跑、desired=stopped。
    /// 入群点照 M3a：机长的大厅 / 人类群游标钉在当前末尾。
    pub fn crew(&self, crew: &str, captain: &str) {
        {
            let mut st = self.port.lock();
            st.sessions
                .insert(captain.into(), Self::row(captain, crew, true));
            st.crew_dirs
                .insert(crew.into(), PathBuf::from("/nonexistent/wd"));
            for (suffix, kind) in [("lobby", MemKind::Lobby), ("human", MemKind::Human)] {
                st.channels.insert(
                    format!("{crew}/{suffix}"),
                    MemChannel {
                        crew: crew.into(),
                        kind,
                        owner: None,
                        members: vec![],
                    },
                );
            }
        }
        self.port.join(captain, &format!("{crew}/lobby"));
        self.port.join(captain, &format!("{crew}/human"));
    }

    pub fn worker(&self, crew: &str, id: &str) {
        self.port
            .lock()
            .sessions
            .insert(id.into(), Self::row(id, crew, false));
        self.port.join(id, &format!("{crew}/lobby"));
    }

    pub fn channel(&self, id: &str, crew: &str, owner: Option<&str>, members: &[&str]) {
        self.port.lock().channels.insert(
            id.into(),
            MemChannel {
                crew: crew.into(),
                kind: MemKind::Other,
                owner: owner.map(str::to_string),
                members: members.iter().map(|s| s.to_string()).collect(),
            },
        );
        for m in members {
            self.port.join(m, id);
        }
    }

    /// 推进派生任务（拉起、事件泵、Arbiter、建 worktree）并处理它们发回来的输入，直到连续 50 次让步都没有新输入。
    pub async fn settle(&mut self) {
        let mut quiet = 0;
        while quiet < 50 {
            tokio::task::yield_now().await;
            match self.rx.try_recv() {
                Ok(input) => {
                    self.core.handle(input).await;
                    quiet = 0;
                }
                Err(_) => quiet += 1,
            }
        }
    }

    pub async fn cmd<T>(&mut self, make: impl FnOnce(oneshot::Sender<T>) -> Cmd) -> T {
        let (tx, rx) = oneshot::channel();
        self.core.handle(Input::Cmd(make(tx))).await;
        self.settle().await;
        rx.await.expect("命令要有回执")
    }

    /// 人从客户端把它起起来（不错开、不受上限），并等到 `Spawned` 处理完。
    pub async fn run(&mut self, session: &str) {
        let s = session.to_string();
        self.cmd(|reply| Cmd::Resume {
            session: s,
            origin: Origin::Human,
            reply,
        })
        .await
        .expect("起得来");
    }

    pub async fn emit(&mut self, session: &str, e: SessionEvent) {
        self.exec.emit(session, e);
        self.settle().await;
    }

    pub async fn tick(&mut self, advance_ms: i64) {
        self.clock.advance(advance_ms);
        self.port.lock().now = self.clock.now();
        self.core.handle(Input::Tick).await;
        self.settle().await;
    }

    pub fn live(&self, session: &str) -> Option<&Live> {
        self.core.live.get(session)
    }

    /// 以 session `id` 的身份发言（发帖时不是机长；要「以机长身份」的测试自己构造 `Sender::Session`）。
    pub fn sender(id: &str) -> Sender {
        Sender::Session {
            id: id.into(),
            captain: false,
        }
    }
}

impl Rig {
    /// 往群里落一条消息（时间 = 现在），再把 `MessageAppended` 交给 actor。
    pub async fn post(
        &mut self,
        channel: &str,
        sender: Sender,
        mentions: Vec<Mention>,
        text: &str,
    ) -> MessageRef {
        let r = {
            let mut st = self.port.lock();
            let seq = st
                .messages
                .keys()
                .filter(|(c, _)| c == channel)
                .map(|(_, s)| *s)
                .max()
                .unwrap_or(0)
                + 1;
            let r = MessageRef {
                channel: channel.into(),
                seq,
            };
            let facts = MsgFacts {
                id: format!("{channel}#{seq}"),
                at: r.clone(),
                sender,
                mentions,
                created_at: st.now,
            };
            st.messages
                .insert((channel.into(), seq), (facts, text.into()));
            r
        };
        let fact = CollabFact::MessageAppended {
            channel: r.channel.clone(),
            seq: r.seq,
        };
        self.core.handle(Input::Collab(fact)).await;
        self.settle().await;
        r
    }

    /// 最后一笔 send 投进去了。
    pub async fn ack(&mut self, session: &str) {
        let id = self.exec.last_send_id(session);
        let e = SessionEvent::Delivery {
            id,
            subject: Subject::Steer,
            outcome: DeliveryOutcome::Delivered,
        };
        self.emit(session, e).await;
    }

    /// 拉起时带的开场正文投进去了。
    pub async fn ack_brief(&mut self, session: &str) {
        let e = SessionEvent::Delivery {
            id: BRIEF_DELIVERY_ID,
            subject: Subject::Brief,
            outcome: DeliveryOutcome::Delivered,
        };
        self.emit(session, e).await;
    }

    /// 一边处理派生任务发回来的输入（写队列的 `WriteDone`、拉起、事件），一边等这个回执。
    /// 一串 nudge 键要真等 `NUDGE_KEY_GAP_MS`，`settle` 的几十次让步等不到它写完。
    pub async fn settle_until<T>(&mut self, mut rx: oneshot::Receiver<T>) -> T {
        loop {
            tokio::select! {
                r = &mut rx => return r.expect("要有回执"),
                Some(input) = self.rx.recv() => self.core.handle(input).await,
            }
        }
    }
}

#[cfg(test)]
mod self_tests {
    use super::*;
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

    fn launch_req(session: &str, initial: Option<&str>) -> LaunchRequest {
        LaunchRequest {
            session_id: session.into(),
            crew_id: "c1".into(),
            workdir: PathBuf::from("/nonexistent/wd"),
            model: None,
            effort: None,
            system_prompt: None,
            initial_prompt: initial.map(str::to_string),
            token: "tok".into(),
            ledger: Arc::new(MemoryLedger::default()),
        }
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<SessionEvent>) -> Vec<SessionEvent> {
        let mut v = Vec::new();
        while let Ok(e) = rx.try_recv() {
            v.push(e);
        }
        v
    }

    fn abandoned(id: u64, subject: Subject, reason: &str) -> SessionEvent {
        SessionEvent::Delivery {
            id,
            subject,
            outcome: DeliveryOutcome::Abandoned(reason.into()),
        }
    }

    /// 顶替执行器照 M2b `abandon()` 的次序：停掉 / 退出时，没终局的投递（含开场正文）各一条 Abandoned、
    /// 待决策框清掉、在干活的 `Working(false)`，最后才是 `Exited`。
    #[tokio::test]
    async fn stand_in_abandons_outstanding_work_before_exited_like_the_driver() {
        let exec = StandIn::default();
        let (h, mut rx) = exec.launch(launch_req("s1", Some("开场"))).await.unwrap();
        let id = h.send("合成".into(), &Context::new()).unwrap();
        exec.emit("s1", SessionEvent::Working(true));
        exec.emit(
            "s1",
            SessionEvent::Decision(DecisionEvent::Appeared(crewd_term::PendingDecision {
                prompt: "p".into(),
                options: vec![],
                numbered: false,
            })),
        );
        drain(&mut rx);
        h.stop().unwrap();
        let got = drain(&mut rx);
        let reason = "session 被停掉";
        assert_eq!(got.len(), 5, "{got:?}");
        assert_eq!(got[0], abandoned(BRIEF_DELIVERY_ID, Subject::Brief, reason));
        assert_eq!(got[1], abandoned(id, Subject::Steer, reason));
        assert_eq!(got[2], SessionEvent::Decision(DecisionEvent::Cleared));
        assert_eq!(got[3], SessionEvent::Working(false));
        assert!(
            matches!(&got[4], SessionEvent::Exited(i) if i.stopped_by_user),
            "{got:?}"
        );
        h.stop().unwrap();
        assert!(drain(&mut rx).is_empty(), "停过一次就不再有事件");

        let (_h, mut rx) = exec.launch(launch_req("s2", Some("开场"))).await.unwrap();
        exec.emit(
            "s2",
            SessionEvent::Delivery {
                id: BRIEF_DELIVERY_ID,
                subject: Subject::Brief,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        exec.emit("s2", SessionEvent::Working(true));
        drain(&mut rx);
        let info = crewd_exec::session::ExitInfo {
            status: crewd_exec::session::ExitStatusKind::Code(1),
            stopped_by_user: false,
            hit_limit: false,
        };
        exec.emit("s2", SessionEvent::Exited(info.clone()));
        let got = drain(&mut rx);
        assert_eq!(
            got,
            [SessionEvent::Working(false), SessionEvent::Exited(info)],
            "投进去了的不再 Abandoned"
        );
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
