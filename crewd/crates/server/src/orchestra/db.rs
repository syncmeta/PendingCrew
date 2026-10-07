//! `Port` 的生产实现。写一律经 M3a 冻结的 `crewd_collab` 函数（`AppState::collab` 在库线程上提交、按序发布变更），
//! 读经 `Db::call`；M3c 不建表、不写表、不写 SQL。M3a 的名字只在这个文件里出现（Task 10 Step 1 对齐；
//! 映射表见 T10a 补充 §2.2）。一个 Port 调用 = 一个 `Db::call` 或一个 collab 事务（除非下面另说）。
//!
//! 也是协作事件进编排的桥（[`fact_of`] / [`pump_collab`] / [`forward_collab`]）。
//!
//! **时间**：DbPort 自己需要「现在」的地方（`render_wake`、`message`、`unread`、`facts_of` 的兜底）读墙上时钟
//! （`crewd_store::now_ms`），与 collab 事务里写的时刻同源——不读 Core 注入的 `Clock`。所以接 DbPort 的测试即使给 Core
//! 一个 `ManualClock`，收听过期、陈旧闸、`settled_at` 也按真实时间算（T10a 复审）；要控制这些时刻的测试用 MemPort。
use std::path::PathBuf;
use std::sync::Arc;

use crewd_collab as collab;
use crewd_collab::Change;
use crewd_collab::wakes::{self as cw, Enqueued, WakePriority, WakeRequest, WakeRow};
use crewd_exec::resume::AgentIdLedger;
use crewd_proto::{ChannelKind, MemberRef, MentionKind, Message, RestartAnswer, SenderKind};
use crewd_wake::breaker::{HUMAN, SYSTEM};
use crewd_wake::plan::{ChannelCtx, Listen, Mention, MsgFacts, Sender};
use crewd_wake::text::Lang;
use crewd_wake::{Latch, LatchScope, MessageRef, Ms, NewWake, Priority, QueuedWake, WakeKind};
use tokio::sync::broadcast::{self, error::RecvError};

use super::Handle;
use super::core::CollabFact;
use super::port::*;
use crate::AppState;
use crate::hub::CollabEvent;
use crate::ledger::StoreLedger;

pub struct DbPort {
    state: Arc<AppState>,
    /// `<数据目录>/crewd.db`：claude 会话号账本（`StoreLedger`）每次开自己的连接。
    db_path: PathBuf,
}

impl DbPort {
    pub fn new(state: Arc<AppState>, db_path: PathBuf) -> Self {
        Self { state, db_path }
    }

    async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> collab::Result<T> + Send + 'static,
    ) -> Result<T> {
        Ok(self.state.db.call(move |s| f(s.conn())).await??)
    }

    async fn write<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut collab::Tx<'_>) -> collab::Result<T> + Send + 'static,
    ) -> Result<T> {
        Ok(self.state.collab(f).await?)
    }
}

/// M3a 的读函数「不存在 = NotFound」，Port 要的是 `Option`。
fn opt<T>(r: collab::Result<T>) -> collab::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(collab::CollabError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

fn ms_of(ts: &str) -> Option<Ms> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|t| Ms(t.timestamp_millis()))
}

fn kind_to(k: WakeKind) -> cw::WakeKind {
    match k {
        WakeKind::Message => cw::WakeKind::Message,
        WakeKind::Timer => cw::WakeKind::Timer,
        WakeKind::AskAnswer => cw::WakeKind::AskAnswer,
        WakeKind::Restart => cw::WakeKind::Restart,
        WakeKind::System => cw::WakeKind::System,
        WakeKind::Checkpoint => cw::WakeKind::Checkpoint,
        WakeKind::StopCheck => cw::WakeKind::StopCheck,
    }
}

fn kind_from(k: cw::WakeKind) -> WakeKind {
    match k {
        cw::WakeKind::Message => WakeKind::Message,
        cw::WakeKind::Timer => WakeKind::Timer,
        cw::WakeKind::AskAnswer => WakeKind::AskAnswer,
        cw::WakeKind::Restart => WakeKind::Restart,
        cw::WakeKind::System => WakeKind::System,
        cw::WakeKind::Checkpoint => WakeKind::Checkpoint,
        cw::WakeKind::StopCheck => WakeKind::StopCheck,
    }
}

fn priority_to(p: Priority) -> WakePriority {
    match p {
        Priority::Auto => WakePriority::Auto,
        Priority::System => WakePriority::System,
        Priority::Human => WakePriority::Human,
    }
}

fn priority_from(p: WakePriority) -> Priority {
    match p {
        WakePriority::Auto => Priority::Auto,
        WakePriority::System => Priority::System,
        WakePriority::Human => Priority::Human,
    }
}

/// 熔断来源键 → `wakes.origin`（M3a：触发它的 session，人 / 系统 = NULL；C21）。
fn origin_to(origin: &str) -> Option<String> {
    (origin != HUMAN && origin != SYSTEM).then(|| origin.to_string())
}

/// `wakes.origin` → 熔断来源键（[`origin_to`] 的反方向）。NULL 有两种：人与系统。分法：
/// - 消息类：按那条消息的发送者（人发的 = `human`，哪怕它只是叫醒了一个收听者、优先级是 Auto——`breaker::origin_of`
///   的同一条规则；否则人叫醒收听者的唤醒会被读回成 `system`，被计数、被挡）；消息已不在 → 按下一条。
/// - 优先级 Human、问被回答（M3a 入队的 `ask_answer`，origin 恒为 NULL，ledger NOTE T8）、重启恢复（`origin_of` 定为人）
///   → `human`：问的答复永不计数、永不被挡。
/// - 其余 → `system`。
fn origin_from(row: &WakeRow, msg_sender: Option<SenderKind>) -> String {
    match (&row.origin, msg_sender) {
        (Some(s), _) => s.clone(),
        (None, Some(SenderKind::Human)) => HUMAN.into(),
        (None, Some(_)) => SYSTEM.into(),
        (None, None)
            if row.priority == WakePriority::Human
                || matches!(row.kind, cw::WakeKind::AskAnswer | cw::WakeKind::Restart) =>
        {
            HUMAN.into()
        }
        (None, None) => SYSTEM.into(),
    }
}

fn queued(conn: &rusqlite::Connection, row: &WakeRow) -> collab::Result<QueuedWake> {
    let m = match row.source.strip_prefix("msg:") {
        Some(id) if row.kind == cw::WakeKind::Message => opt(collab::messages::get(conn, id))?,
        _ => None,
    };
    Ok(QueuedWake {
        id: row.id,
        session: row.session.clone(),
        kind: kind_from(row.kind),
        source: row.source.clone(),
        priority: priority_from(row.priority),
        not_before: Ms(row.not_before_ms),
        note: row.note.clone(),
        state: super::port::wake_state(row.state),
        attempts: row.attempts,
        // 读不出入队时刻：不能当成 0（那会让它看起来最老）；照 id 排就行，这个字段只给排障看。
        created_at: ms_of(&row.created_at).unwrap_or(Ms(crewd_store::now_ms())),
        origin: origin_from(row, m.as_ref().map(|m| m.sender_kind)),
        msg: m.map(|m| MessageRef {
            channel: m.channel_id,
            seq: m.seq,
        }),
        last_error: row.last_error.clone(),
    })
}

/// 消息 → 规划要素。`now` 只在读不出发送时刻时用：读不出 = 当作刚发的（绝不当成陈旧而不叫，ledger NOTE T2）。
pub(crate) fn facts_of(m: &Message, now: Ms) -> MsgFacts {
    let sender = match (m.sender_kind, &m.sender_session_id) {
        (SenderKind::Human, _) => Sender::Human,
        (SenderKind::Session | SenderKind::Captain, Some(id)) => Sender::Session {
            id: id.clone(),
            captain: m.sender_kind == SenderKind::Captain,
        },
        _ => Sender::System,
    };
    // @ 只决定叫醒谁（§24.2）。没有目标的 session / 机长 @（旧数据：机组没有机长时没落定的 @机长）叫不醒任何人，略过；
    // 人发的消息因此没指向任何成员时照兜底规则走（Option A）。
    let mentions = m
        .mentions
        .iter()
        .filter_map(|x| match (x.kind, &x.target_id) {
            (MentionKind::Session | MentionKind::Captain, Some(t)) => {
                Some(Mention::Session(t.clone()))
            }
            (MentionKind::Session | MentionKind::Captain, None) => None,
            (MentionKind::Human, _) => Some(Mention::Human),
        })
        .collect();
    let created_at = ms_of(&m.created_at).unwrap_or_else(|| {
        eprintln!(
            "crewd: 编排：消息 {} 的发送时刻读不出（{:?}），按刚发的算",
            m.id, m.created_at
        );
        now
    });
    MsgFacts {
        id: m.id.clone(),
        at: MessageRef {
            channel: m.channel_id.clone(),
            seq: m.seq,
        },
        sender,
        mentions,
        created_at,
    }
}

fn row_of(conn: &rusqlite::Connection, s: crewd_proto::Session) -> collab::Result<SessionRow> {
    let crew = collab::crews::get(conn, &s.crew_id)?;
    let captain = collab::crews::captain_of(conn, &s.crew_id)?;
    let crew_removed = collab::crews::is_archived(conn, &s.crew_id)?;
    let workdir = s
        .worktree
        .clone()
        .or(s.working_directory.clone())
        .or(crew.working_directory.clone())
        .filter(|w| !w.is_empty())
        .map(PathBuf::from);
    Ok(SessionRow {
        captain: captain.as_deref() == Some(s.id.as_str()),
        crew_id: s.crew_id,
        executor: s.executor,
        // 显示名 → 标题（N1：标题必有，显示名可选；同 collab `sessions::name_of` 的回退）。
        display_name: s.display_name.or(s.title),
        agent_session_id: s.agent_session_id,
        workdir,
        model: s.model,
        effort: s.effort,
        brief: s.brief,
        desired: if s.desired == crewd_proto::Desired::Running {
            Desired::Running
        } else {
            Desired::Stopped
        },
        was_working: s.was_working,
        last_active_at: s.last_active_at.as_deref().and_then(ms_of),
        resume_failed_at: s.resume_failed_at.as_deref().and_then(ms_of),
        awaiting_question: s.awaiting_question,
        last_progress_at: s.last_progress_at.as_deref().and_then(ms_of),
        crew_removed,
        id: s.id,
    })
}

/// 熔断 id ↔ 台账的 `Scope`：只经 collab 的 `Scope::parse` / `Scope::id`（C21），这里不拆、不拼分隔符。
fn latch_scope(scope: LatchScope, id: &str) -> Result<cw::latches::Scope> {
    cw::latches::Scope::parse(scope.as_str(), id)
        .ok_or_else(|| anyhow::anyhow!("熔断 id 认不出：{scope} {id:?}"))
}

fn session_ids(ms: Vec<MemberRef>) -> Vec<String> {
    ms.into_iter()
        .filter_map(|m| match m {
            MemberRef::Session { id } => Some(id),
            MemberRef::Human => None,
        })
        .collect()
}

fn acked_origins(b: &Batch) -> Vec<String> {
    let mut origins: Vec<String> = Vec::new();
    for w in &b.wakes {
        if !origins.contains(&w.origin) {
            origins.push(w.origin.clone());
        }
    }
    origins
}

impl Port for DbPort {
    async fn session(&self, id: &str) -> Result<Option<SessionRow>> {
        let id = id.to_string();
        self.read(move |c| {
            opt(collab::sessions::get(c, &id))?
                .map(|s| row_of(c, s))
                .transpose()
        })
        .await
    }

    async fn sessions_desired_running(&self) -> Result<Vec<SessionRow>> {
        self.read(|c| {
            collab::sessions::running_desired(c)?
                .into_iter()
                .map(|s| row_of(c, s))
                .collect()
        })
        .await
    }

    async fn all_session_ids(&self) -> Result<Vec<String>> {
        self.read(|c| {
            Ok(collab::sessions::list(c, None)?
                .into_iter()
                .map(|s| s.id)
                .collect())
        })
        .await
    }

    async fn insert_session(&self, s: NewSession) -> Result<SessionRow> {
        let session = self
            .write(move |tx| {
                // 标题与 brief 原样交给 collab（§26.7：标题必填或从 brief 派生，拒绝原样上抛）。
                collab::sessions::register(
                    tx,
                    collab::sessions::NewSession {
                        crew_id: s.crew_id,
                        role: if s.captain {
                            crewd_proto::Role::Captain
                        } else {
                            crewd_proto::Role::Worker
                        },
                        executor: s.executor,
                        brief: Some(s.brief),
                        title: s.title,
                        model: s.model,
                        effort: s.effort,
                        worktree: s.worktree.map(|p| p.display().to_string()),
                        ..Default::default()
                    },
                )
            })
            .await?;
        self.read(move |c| row_of(c, session)).await
    }

    async fn crew_workdir(&self, crew: &str) -> Result<Option<PathBuf>> {
        let crew = crew.to_string();
        self.read(move |c| {
            Ok(collab::crews::get(c, &crew)?
                .working_directory
                .filter(|w| !w.is_empty())
                .map(PathBuf::from))
        })
        .await
    }

    async fn patch_session(&self, id: &str, p: SessionPatch) -> Result<()> {
        let id = id.to_string();
        self.write(move |tx| {
            if let Some(d) = p.desired {
                let d = if d == Desired::Running {
                    crewd_proto::Desired::Running
                } else {
                    crewd_proto::Desired::Stopped
                };
                collab::sessions::set_desired(tx, &id, d)?;
            }
            // `set_working` 同时写 last_active_at。**只给 last_active_at 的（退出时）M3a 没有写函数**：这一次的时间丢了，
            // 库里留的是最后一个 Working 边沿的时间（`restart_order` 的「最近活跃」因此可能偏早）。不静默：打一行日志，
            // 缺口记在交付记录的遗留（要 M3a 补一个只写 last_active_at 的函数）；MemPort 照写，Core 的单元测试看不出这个差异。
            if let Some(w) = p.was_working {
                collab::sessions::set_working(tx, &id, w)?;
            } else if p.last_active_at.is_some() {
                eprintln!("crewd: 编排：{id} 的 last_active_at 没落库（M3a 没有只写它的函数）");
            }
            if let Some(f) = p.resume_failed_at {
                collab::sessions::set_resume_failed(tx, &id, f.is_some())?;
            }
            if p.model.is_some() || p.effort.is_some() {
                // M3a：COALESCE，None = 保留旧值。
                collab::sessions::set_profile(tx, &id, p.model, p.effort)?;
            }
            Ok(())
        })
        .await
    }

    async fn set_captain(&self, crew: &str, session: &str) -> Result<Option<String>> {
        let (crew, session) = (crew.to_string(), session.to_string());
        self.write(move |tx| {
            let prev = collab::crews::captain_of(tx.conn(), &crew)?;
            collab::crews::set_captain(tx, &crew, &session)?;
            Ok(prev.filter(|p| *p != session))
        })
        .await
    }

    async fn captain(&self, crew: &str) -> Result<Option<String>> {
        let crew = crew.to_string();
        self.read(move |c| collab::crews::captain_of(c, &crew))
            .await
    }

    async fn crew_removed(&self, crew: &str) -> Result<bool> {
        let crew = crew.to_string();
        self.read(move |c| {
            collab::crews::get(c, &crew)?;
            collab::crews::is_archived(c, &crew)
        })
        .await
    }

    fn ledger(&self, session: &str) -> Arc<dyn AgentIdLedger> {
        Arc::new(StoreLedger::new(self.db_path.clone(), session))
    }

    async fn worldview(&self, session: &str) -> Result<String> {
        let session = session.to_string();
        self.read(move |c| {
            let locale = collab::settings::locale(c)?;
            Ok(collab::worldview::render(&collab::worldview::context_for(
                c, &session, locale,
            )?))
        })
        .await
    }

    fn mint_token(&self, session: &str) -> String {
        self.state.tokens.mint(session)
    }

    fn revoke_token(&self, session: &str) {
        self.state.tokens.revoke(session);
    }

    async fn enqueue(&self, w: NewWake) -> Result<Option<i64>> {
        let r = self
            .write(move |tx| {
                cw::enqueue(
                    tx,
                    WakeRequest {
                        session: w.session,
                        kind: kind_to(w.kind),
                        source: w.source,
                        priority: priority_to(w.priority),
                        not_before_ms: Some(w.not_before.0),
                        note: w.note,
                        origin: origin_to(&w.origin),
                    },
                )
            })
            .await?;
        // `Skipped` = 目标的机组已移除（不写行、不发变更）：当作无事发生，不记日志。
        Ok(match r {
            Enqueued::New(id) => Some(id),
            Enqueued::Duplicate(_) | Enqueued::Skipped => None,
        })
    }

    async fn pending(&self, session: &str) -> Result<Vec<QueuedWake>> {
        let session = session.to_string();
        // `due_for(…, i64::MAX)` = 全部 pending（不论到没到点），按优先级、入队先后。
        self.read(move |c| {
            cw::due_for(c, &session, i64::MAX)?
                .iter()
                .map(|r| queued(c, r))
                .collect()
        })
        .await
    }

    async fn settle(&self, ids: &[i64], s: Settle, _now: Ms) -> Result<()> {
        let ids = ids.to_vec();
        self.write(move |tx| match s {
            Settle::Delivered => cw::mark_delivered(tx, &ids),
            Settle::Dropped { error } => {
                for id in &ids {
                    cw::mark_dropped(tx, *id, &error)?;
                }
                Ok(())
            }
            Settle::Requeue => cw::requeue(tx, &ids),
        })
        .await
    }

    /// 全有或全无：在 `pending()` 与这里之间被作废的行（机组移除、人停）让 collab 回 `Conflict wake.bad_transition`，
    /// 一行都不拿；Core 把这批留在 pending、下一拍不带那一行再来（spec §26.5）。
    async fn claim(&self, b: &Batch) -> Result<()> {
        let ids = b.ids();
        self.write(move |tx| cw::claim(tx, &ids).map(|_| ())).await
    }

    async fn ack(&self, b: &Batch, a: Ack) -> Result<Acked> {
        let session = b.session.clone();
        let wake_ids: Vec<i64> = b.wakes.iter().map(|w| w.id).collect();
        let rider_ids: Vec<i64> = b.riders.iter().map(|w| w.id).collect();
        let all = b.ids();
        let cursor_targets = b.cursor_targets.clone();
        match a {
            Ack::Delivered { riders } => {
                self.write(move |tx| {
                    let fate = match &riders {
                        Riders::Release => cw::Riders::Release,
                        Riders::Drop(why) => cw::Riders::Drop(why),
                    };
                    cw::ack_delivered(tx, &session, &wake_ids, fate, &rider_ids, &cursor_targets)
                })
                .await?;
                // 只在提交之后交回来源（环路熔断据此记账）。
                Ok(Acked {
                    origins: acked_origins(b),
                    ..Default::default()
                })
            }
            Ack::Failed { error, now, jitter } => {
                let failed = self
                    .write(move |tx| {
                        cw::ack_failed(tx, &all, &error, |n| {
                            crewd_wake::queue::after_failure(n, now, jitter).map(|t| t.0)
                        })
                    })
                    .await?;
                let mut out = Acked::default();
                for f in failed {
                    match f.retry_at_ms {
                        Some(t) => out.retried.push((f.id, Ms(t))),
                        None => out.gave_up.push(f.id),
                    }
                }
                Ok(out)
            }
        }
    }

    async fn fail_inflight(&self, error: &str, now: Ms) -> Result<Reclaimed> {
        let error = error.to_string();
        self.write(move |tx| {
            let failed = cw::reset_inflight(tx, &error, |n| {
                crewd_wake::queue::after_restart(n, now).map(|t| t.0)
            })?;
            let mut out = Reclaimed::default();
            let mut gave_up = Vec::new();
            for f in failed {
                match f.retry_at_ms {
                    Some(t) => out.retried.push((f.id, Ms(t))),
                    None => gave_up.push(f.id),
                }
            }
            // 放弃的行按 id 原样读回（不封顶：一条崩了好几轮的唤醒 id 很旧，9a 修复 1）。同一事务里读。
            out.gave_up = cw::by_ids(tx.conn(), &gave_up)?
                .iter()
                .map(|r| queued(tx.conn(), r))
                .collect::<collab::Result<_>>()?;
            Ok(out)
        })
        .await
    }

    async fn due_sessions(&self, now: Ms) -> Result<Vec<String>> {
        self.read(move |c| cw::sessions_with_due(c, now.0)).await
    }

    async fn next_not_before_after(&self, now: Ms) -> Result<Option<Ms>> {
        self.read(move |c| Ok(cw::next_due_after(c, now.0)?.map(Ms)))
            .await
    }

    async fn list_wakes(&self, session: Option<&str>) -> Result<Vec<QueuedWake>> {
        let session = session.map(str::to_string);
        self.read(move |c| {
            cw::list(c, session.as_deref(), WAKE_LIST_LIMIT as u32)?
                .iter()
                .map(|r| queued(c, r))
                .collect()
        })
        .await
    }

    async fn wakes_for_message(&self, message_id: &str) -> Result<Vec<QueuedWake>> {
        let source = cw::source::message(message_id);
        self.read(move |c| {
            cw::for_source(c, cw::WakeKind::Message, &source)?
                .iter()
                .map(|r| queued(c, r))
                .collect()
        })
        .await
    }

    async fn latches(&self) -> Result<Vec<Latch>> {
        self.read(|c| {
            Ok(cw::latches::list(c)?
                .into_iter()
                .map(|l| {
                    let scope = match &l.scope {
                        cw::latches::Scope::Edge { .. } => LatchScope::Edge,
                        cw::latches::Scope::Session(_) => LatchScope::Session,
                    };
                    Latch {
                        scope,
                        id: l.scope.id(),
                        reason: l.reason.clone(),
                        at: ms_of(&l.at).unwrap_or(Ms(crewd_store::now_ms())),
                    }
                })
                .collect())
        })
        .await
    }

    async fn put_latch(&self, l: &Latch) -> Result<()> {
        let (scope, reason) = (latch_scope(l.scope, &l.id)?, l.reason.clone());
        self.write(move |tx| cw::latches::set(tx, &scope, &reason))
            .await
    }

    async fn clear_latch(&self, scope: LatchScope, id: &str) -> Result<()> {
        let scope = latch_scope(scope, id)?;
        self.write(move |tx| cw::latches::clear(tx, &scope).map(|_| ()))
            .await
    }

    async fn max_live(&self) -> Result<Option<usize>> {
        self.read(|c| Ok(collab::settings::max_live_sessions(c)?.map(|n| n as usize)))
            .await
    }

    async fn set_max_live(&self, max: Option<usize>) -> Result<()> {
        let max = max.map(u32::try_from).transpose()?;
        self.write(move |tx| collab::settings::set_max_live_sessions(tx, max))
            .await
    }

    async fn locale(&self) -> Result<Lang> {
        self.read(|c| {
            Ok(match collab::settings::locale(c)? {
                collab::Locale::Zh => Lang::Zh,
                collab::Locale::En => Lang::En,
            })
        })
        .await
    }

    async fn set_locale(&self, l: Lang) -> Result<()> {
        let l = match l {
            Lang::Zh => collab::Locale::Zh,
            Lang::En => collab::Locale::En,
        };
        self.write(move |tx| collab::settings::set_locale(tx, l))
            .await
    }

    async fn open_restart(
        &self,
        session_ids: Vec<String>,
        reason: RestartReason,
    ) -> Result<Option<i64>> {
        self.write(move |tx| Ok(collab::restart::open_with(tx, session_ids, reason)?.map(|p| p.id)))
            .await
    }

    async fn restart_answer(&self, id: i64) -> Result<Option<bool>> {
        self.read(move |c| {
            Ok(opt(collab::restart::get(c, id))?
                .and_then(|p| p.answer)
                .map(|a| a == RestartAnswer::Resume))
        })
        .await
    }

    async fn apply_due_defaults(&self) -> Result<()> {
        // 已移除的机组不在列表里（只读，不按默认办）。
        let crews: Vec<String> = self
            .read(|c| {
                Ok(collab::crews::list(c, false)?
                    .into_iter()
                    .map(|c| c.id)
                    .collect())
            })
            .await?;
        for crew in crews {
            // 每个机组一个事务：一本账出错不连累别的机组。
            let r = self
                .write({
                    let crew = crew.clone();
                    move |tx| collab::asks::apply_due_defaults(tx, &crew).map(|_| ())
                })
                .await;
            if let Err(e) = r {
                eprintln!("crewd: 编排：机组 {crew} 的默认方案到点处理失败：{e:#}");
            }
        }
        Ok(())
    }

    async fn prune_wakes(&self, before: Ms, limit: usize) -> Result<usize> {
        let limit = u32::try_from(limit).unwrap_or(u32::MAX);
        self.write(move |tx| cw::prune(tx, before.0, limit)).await
    }

    async fn hibernate_deadline_secs(&self) -> Result<Option<u32>> {
        self.read(collab::settings::hibernate_deadline_secs).await
    }

    async fn set_hibernate_deadline_secs(&self, secs: Option<u32>) -> Result<()> {
        self.write(move |tx| collab::settings::set_hibernate_deadline_secs(tx, secs))
            .await
    }

    async fn hibernate_record(&self) -> Result<Option<HibernateRecord>> {
        self.read(collab::hibernate::get).await
    }

    async fn put_hibernate(&self, r: &HibernateRecord) -> Result<()> {
        let r = r.clone();
        self.write(move |tx| collab::hibernate::put(tx, &r)).await
    }

    async fn message(&self, r: &MessageRef) -> Result<Option<(MsgFacts, String)>> {
        let r = r.clone();
        let now = Ms(crewd_store::now_ms());
        self.read(move |c| {
            // seq 全库唯一（M3a.1）；群对不上 = 不是这条。
            Ok(opt(collab::messages::get_by_seq(c, r.seq))?
                .filter(|m| m.channel_id == r.channel)
                .map(|m| (facts_of(&m, now), m.text.clone())))
        })
        .await
    }

    async fn channel_ctx(&self, channel: &str, now: Ms) -> Result<Option<ChannelCtx>> {
        let channel = channel.to_string();
        self.read(move |c| {
            let Some(ch) = opt(collab::channels::get(c, &channel))? else {
                return Ok(None);
            };
            // 成员 = M3a 的有效成员（`channels::members`），且与注入面（`cursors::unread_for` 用的
            // `channels_of_session`）同一口径：归档的 topic / dm / 人类群里没有人「在」（大厅照算：它的机组
            // 被移除时大厅归档，大厅仍在它的 session 的注入面上）。
            let archived_out = ch.archived_at.is_some() && ch.kind != ChannelKind::Lobby;
            let session_members = if archived_out {
                Vec::new()
            } else {
                session_ids(collab::channels::members(c, &channel)?)
            };
            let owner = match ch.owner {
                Some(MemberRef::Session { id }) => Some(id),
                _ => None,
            };
            let crew_captain = collab::crews::captain_of(c, &ch.crew_id)?;
            let dm_sessions = if ch.kind == ChannelKind::Dm {
                session_members.clone()
            } else {
                Vec::new()
            };
            // 传真实的 now（不是 0）：0 会把已过期的收听也读回来。归档的群、读不到的收听 collab 已滤掉。
            let active = collab::listens::active_for_channel(c, &channel, now.0)?;
            // 活着 = 存在且机组没被移除（不是「进程在跑」）：群成员 ∪ 机组没被移除的收听者。
            let mut live_sessions = session_members.clone();
            for l in &active {
                let Some(s) = crewd_store::sessions::get(c, &l.session)? else {
                    continue;
                };
                if !collab::crews::is_archived(c, &s.crew_id)? && !live_sessions.contains(&s.id) {
                    live_sessions.push(s.id);
                }
            }
            let listens = active
                .into_iter()
                .map(|l| Listen {
                    session: l.session,
                    until: l.until_ms.map_or(Ms(i64::MAX), Ms),
                    senders: (!l.senders.is_empty()).then_some(l.senders),
                })
                .collect();
            Ok(Some(ChannelCtx {
                session_members,
                live_sessions,
                owner,
                dm_sessions,
                crew_captain,
                listens,
            }))
        })
        .await
    }

    async fn unread(&self, session: &str) -> Result<Vec<MsgFacts>> {
        let session = session.to_string();
        let now = Ms(crewd_store::now_ms());
        // 写事务：`unread_for` 会把缺的游标钉到群末尾（入群点）。
        let blocks = self
            .write(move |tx| collab::cursors::unread_for(tx, &session))
            .await?;
        Ok(blocks
            .iter()
            .flat_map(|(_, msgs)| msgs.iter().map(|m| facts_of(m, now)))
            .collect())
    }

    async fn cursor(&self, session: &str, channel: &str) -> Result<i64> {
        let (session, channel) = (session.to_string(), channel.to_string());
        self.read(move |c| Ok(collab::cursors::cursor(c, &session, &channel)?.unwrap_or(0)))
            .await
    }

    async fn post_system(&self, to: Audience, text: String) -> Result<()> {
        self.write(move |tx| {
            let (channel, mention_human, mention_owner) = match to {
                // 人类群归 home（顶层机组 / 总机组自己，§24.1 H2）。
                Audience::HumanChannel { crew } => {
                    let home = collab::crews::home_of(tx.conn(), &crew)?;
                    (
                        collab::channels::ensure_human_channel(tx, &home)?.id,
                        true,
                        false,
                    )
                }
                Audience::Lobby {
                    crew,
                    mention_owner,
                } => (crewd_store::channels::lobby_id(&crew), false, mention_owner),
                Audience::Channel {
                    channel,
                    mention_owner,
                } => (channel, false, mention_owner),
            };
            let mut mentions = Vec::new();
            if mention_human {
                mentions.push(crewd_proto::Mention {
                    kind: MentionKind::Human,
                    target_id: None,
                });
            }
            if mention_owner
                && let Some(MemberRef::Session { id }) =
                    collab::channels::owner_of(tx.conn(), &channel)?
            {
                mentions.push(crewd_proto::Mention {
                    kind: MentionKind::Session,
                    target_id: Some(id),
                });
            }
            let post = collab::messages::NewPost {
                channel_id: channel,
                text,
                mentions,
                ..Default::default()
            };
            collab::messages::post(tx, collab::Sender::System, post).map(|_| ())
        })
        .await
    }

    async fn render_wake(&self, session: &str, batch: &[QueuedWake]) -> Result<Rendered> {
        let session = session.to_string();
        let ids: Vec<i64> = batch.iter().map(|w| w.id).collect();
        let now_ms = crewd_store::now_ms();
        let (text, dropped, deferred, cursor_targets) = self
            .read(move |c| {
                // `by_ids` 按 id 排；正文的顺序要跟这一批（按优先级选好的）一致。
                let mut rows: Vec<WakeRow> = cw::by_ids(c, &ids)?;
                rows.sort_by_key(|r| ids.iter().position(|i| *i == r.id));
                let locale = collab::settings::locale(c)?;
                // 报错（如账本坏了）是 fail-loud：原样上抛，Core 记日志、这批留在 pending，不在这里重试。
                let b = collab::render::build_batch(c, &session, locale, &rows, now_ms)?;
                let text = collab::render::wake_text(locale, &b);
                // R6 / S45：`deferred` 随这批走（Core 一起 claim）；`cursor_targets()` 是送到后游标推到哪。
                Ok((
                    text,
                    b.unusable.clone(),
                    b.deferred.clone(),
                    b.cursor_targets(),
                ))
            })
            .await?;
        let dropped_ids: Vec<i64> = dropped.iter().map(|(id, _)| *id).collect();
        if !dropped.is_empty() {
            self.write(move |tx| {
                for (id, why) in &dropped {
                    cw::mark_dropped(tx, *id, why)?;
                }
                Ok(())
            })
            .await?;
        }
        Ok(Rendered {
            text,
            dropped: dropped_ids,
            deferred,
            cursor_targets,
        })
    }

    fn publish(&self, e: OrchestraEvent) {
        let event = match e {
            OrchestraEvent::SessionChanged(v) => crewd_proto::Event {
                event: crewd_proto::events::SESSION_CHANGED.into(),
                data: serde_json::to_value(super::control::live_to_proto(&v)).unwrap_or_default(),
            },
            OrchestraEvent::WakeChanged {
                session,
                wake_ids,
                message_ids,
            } => crewd_proto::Event {
                event: crewd_proto::orchestra::events::WAKE_CHANGED.into(),
                data: serde_json::to_value(crewd_proto::orchestra::WakeChanged {
                    session_id: session,
                    wake_ids,
                    message_ids,
                })
                .unwrap_or_default(),
            },
        };
        self.state.hub.publish(event);
    }
}

/// M3a 的协作变更 → 编排关心的事实；其余的不管。
pub fn fact_of(c: &Change) -> Option<CollabFact> {
    match c {
        Change::MessageAppended {
            channel_id, seq, ..
        } => Some(CollabFact::MessageAppended {
            channel: channel_id.clone(),
            seq: *seq,
        }),
        Change::WakeEnqueued { session_id, .. } => Some(CollabFact::WakeEnqueued {
            session: session_id.clone(),
        }),
        Change::AskAnswered {
            crew_id,
            ask,
            cleared,
            ..
        } => Some(CollabFact::AskAnswered {
            crew: crew_id.clone(),
            ask: ask.clone(),
            cleared: cleared.clone(),
        }),
        Change::RestartAnswered {
            id,
            answer,
            session_ids,
        } => Some(CollabFact::RestartAnswered {
            prompt: *id,
            resume: *answer == RestartAnswer::Resume,
            sessions: session_ids.clone(),
        }),
        // 控制者裁定 C3：工具调用 / 工作项变化（M3b 的 post-tool-use、M4b 的工作项命令经 M3a `note_progress` 发）。
        Change::Progress { session_id, .. } => Some(CollabFact::Progress {
            session: session_id.clone(),
        }),
        _ => None,
    }
}

/// 把协作变更一条条变成事实交给 `sink`，直到通道关了；订阅落后 → `Lagged`（积压补救，裁定 16）。
pub async fn pump_collab(
    mut rx: broadcast::Receiver<CollabEvent>,
    mut sink: impl FnMut(CollabFact),
) {
    loop {
        match rx.recv().await {
            Ok(c) => {
                if let Some(f) = fact_of(&c) {
                    sink(f);
                }
            }
            Err(RecvError::Lagged(_)) => sink(CollabFact::Lagged),
            Err(RecvError::Closed) => return,
        }
    }
}

/// 把协作变更转给编排 actor（[`pump_collab`] 跑在一个派生任务里）。
/// `rx` 要在 `orchestra::spawn`（里面跑 `boot`）**之前**订阅：boot 期间产生的变更不能漏（T11 接线）。
pub fn forward_collab(
    rx: broadcast::Receiver<CollabEvent>,
    handle: Handle,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(pump_collab(rx, move |f| handle.collab(f)))
}

/// 测试夹具：真库（磁盘上的临时文件，账本要开自己的连接）+ `AppState` + `DbPort`，全用固定 id（C17）。
/// ```text
/// chief（总机组 = 管理机组）  chief-1（worker）
/// c-1（顶层，工作目录 = 临时目录） 机长 cap-1；w-1（分机 2）、w-2（分机 3）
///   └ c-2                         机长 cap-2
/// c-9（顶层）                     机长 cap-9；w-9（分机 2）
/// ```
/// session id 都带 `-`（熔断台账的 id 要经得起）。
#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use crewd_proto::{Crew, Role, Session};

    pub(crate) async fn seeded() -> (Arc<AppState>, DbPort, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crewd.db");
        let store = crewd_store::Store::open(&path).unwrap();
        let c = store.conn();
        let wd = dir.path().display().to_string();
        for (id, title, number, management, at) in [
            ("c-1", "机组甲", Some(1), false, "2026-09-01T00:00:00.000Z"),
            ("c-2", "机组乙", Some(2), false, "2026-09-02T00:00:00.000Z"),
            ("c-9", "机组丙", Some(3), false, "2026-09-03T00:00:00.000Z"),
            ("chief", "总机组", None, true, "2026-08-01T00:00:00.000Z"),
        ] {
            crewd_store::crews::upsert(
                c,
                &Crew {
                    id: id.into(),
                    title: title.into(),
                    number,
                    management,
                    working_directory: Some(wd.clone()),
                    created_at: at.into(),
                    updated_at: at.into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        }
        crewd_store::crews::set_parents(c, "c-2", &["c-1".to_string()]).unwrap();
        for (id, crew, role, ext) in [
            ("cap-1", "c-1", Role::Captain, None),
            ("w-1", "c-1", Role::Worker, Some(2)),
            ("w-2", "c-1", Role::Worker, Some(3)),
            ("cap-2", "c-2", Role::Captain, None),
            ("cap-9", "c-9", Role::Captain, None),
            ("w-9", "c-9", Role::Worker, Some(2)),
            ("chief-1", "chief", Role::Worker, Some(2)),
        ] {
            crewd_store::sessions::upsert(
                c,
                &Session {
                    id: id.into(),
                    crew_id: crew.into(),
                    role,
                    executor: "claude".into(),
                    title: Some(format!("名字 {id}")),
                    ext,
                    updated_at: "2026-09-01T00:00:00.000Z".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        }
        crewd_store::schema::backfill_v3(c).unwrap();
        crewd_store::schema::backfill_v4(c).unwrap();
        let state = Arc::new(AppState::new(crewd_store::Db::spawn(store).unwrap()));
        let port = DbPort::new(state.clone(), path);
        (state, port, dir)
    }

    /// `sender` 在 `channel` 里发一条，@ 这些 session（经 collab 的 `messages::post`，同生产路径）。
    pub(crate) async fn post_as(
        state: &AppState,
        sender: collab::Sender,
        channel: &str,
        text: &str,
        at: &[&str],
    ) -> Message {
        let (channel, text) = (channel.to_string(), text.to_string());
        let mentions: Vec<crewd_proto::Mention> = at
            .iter()
            .map(|s| crewd_proto::Mention {
                kind: MentionKind::Session,
                target_id: Some(s.to_string()),
            })
            .collect();
        state
            .collab(move |tx| {
                collab::messages::post(
                    tx,
                    sender,
                    collab::messages::NewPost {
                        channel_id: channel,
                        text,
                        mentions,
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap()
            .message
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{post_as, seeded};
    use super::*;
    use crewd_wake::breaker;
    use crewd_wake::source;

    fn new_wake(
        session: &str,
        kind: WakeKind,
        source: String,
        p: Priority,
        origin: &str,
    ) -> NewWake {
        NewWake {
            session: session.into(),
            kind,
            source,
            priority: p,
            not_before: Ms(0),
            note: Some("x".into()),
            origin: origin.into(),
        }
    }

    #[test]
    fn source_formats_are_byte_identical_to_collab() {
        assert_eq!(source::message("m-1"), cw::source::message("m-1"));
        assert_eq!(source::quota(5), cw::source::quota(5));
        assert_eq!(source::restart("7"), cw::source::restart(7));
        assert_eq!(source::system("x"), cw::source::system("x"));
        assert_eq!(source::timer("t"), cw::source::timer("t"));
    }

    /// ledger NOTE T2：读不出发送时刻 = 当作刚发的（不是 Ms(0)，那会被 6h 陈旧闸挡掉、谁都不叫）。
    /// §2.2：没有目标的 session / 机长 @ 略过（叫不醒任何人）；@人类 原样；机长发的带 `captain`。
    #[test]
    fn facts_of_treats_an_unreadable_time_as_now_and_skips_targetless_mentions() {
        let m = Message {
            id: "m-1".into(),
            channel_id: "c-1/lobby".into(),
            seq: 3,
            sender_kind: SenderKind::Captain,
            sender_session_id: Some("cap-1".into()),
            text: "合成".into(),
            mentions: vec![
                crewd_proto::Mention {
                    kind: MentionKind::Captain,
                    target_id: None,
                },
                crewd_proto::Mention {
                    kind: MentionKind::Session,
                    target_id: Some("w-1".into()),
                },
                crewd_proto::Mention {
                    kind: MentionKind::Human,
                    target_id: None,
                },
            ],
            created_at: "不是时间".into(),
            ..Default::default()
        };
        let now = Ms(10 * crewd_wake::HOUR);
        let f = facts_of(&m, now);
        assert_eq!(f.created_at, now);
        assert!(!crewd_wake::plan::is_stale(&f, now));
        assert_eq!(f.mentions, [Mention::Session("w-1".into()), Mention::Human]);
        assert_eq!(
            f.sender,
            Sender::Session {
                id: "cap-1".into(),
                captain: true
            }
        );
        let ok = Message {
            created_at: "2026-09-01T00:00:00.000Z".into(),
            ..m
        };
        assert_eq!(
            facts_of(&ok, now).created_at,
            ms_of("2026-09-01T00:00:00.000Z").unwrap()
        );
    }

    #[tokio::test]
    async fn enqueue_dedupes_and_pending_resolves_the_message_position() {
        let (state, port, _d) = seeded().await;
        let posted = post_as(
            &state,
            collab::Sender::Session("cap-1".into()),
            "c-1/lobby",
            "合成派活甲",
            &["w-1"],
        )
        .await;
        let w = NewWake {
            note: None,
            ..new_wake(
                "w-1",
                WakeKind::Message,
                source::message(&posted.id),
                Priority::Auto,
                "cap-1",
            )
        };
        assert!(port.enqueue(w.clone()).await.unwrap().is_some());
        assert!(port.enqueue(w).await.unwrap().is_none());
        let p = port.pending("w-1").await.unwrap();
        assert_eq!(
            p[0].msg,
            Some(MessageRef {
                channel: posted.channel_id.clone(),
                seq: posted.seq
            })
        );
        assert_eq!(p[0].origin, "cap-1");
    }

    /// NOTE T8 / §2.2：NULL 的来源读回来——人发的消息（哪怕叫醒的是收听者、优先级 Auto）= human；问的答复 = human；
    /// 系统 = system；session 原样。问的答复因此永不计数、永不被挡。
    #[tokio::test]
    async fn origins_round_trip_and_ask_answers_and_human_messages_read_back_as_human() {
        let (state, port, _d) = seeded().await;
        let from_human = post_as(&state, collab::Sender::Human, "c-1/lobby", "合成甲", &[]).await;
        let from_system = post_as(&state, collab::Sender::System, "c-1/lobby", "合成乙", &[]).await;
        for w in [
            new_wake(
                "w-1",
                WakeKind::Message,
                source::message(&from_human.id),
                Priority::Auto,
                HUMAN,
            ),
            new_wake(
                "w-1",
                WakeKind::Message,
                source::message(&from_system.id),
                Priority::Auto,
                SYSTEM,
            ),
            new_wake(
                "w-1",
                WakeKind::System,
                source::system("a"),
                Priority::System,
                SYSTEM,
            ),
            new_wake(
                "w-1",
                WakeKind::Timer,
                source::timer("t"),
                Priority::Auto,
                "w-1",
            ),
        ] {
            assert!(port.enqueue(w).await.unwrap().is_some());
        }
        // M3a 自己入队的问的答复：origin 恒为 NULL，优先级不一定是 Human。
        state
            .collab(|tx| {
                cw::enqueue(
                    tx,
                    WakeRequest {
                        session: "w-1".into(),
                        kind: cw::WakeKind::AskAnswer,
                        source: cw::source::ask("c-1", "H1"),
                        priority: WakePriority::System,
                        not_before_ms: Some(0),
                        note: None,
                        origin: None,
                    },
                )
            })
            .await
            .unwrap();
        let got: Vec<(WakeKind, String)> = port
            .pending("w-1")
            .await
            .unwrap()
            .into_iter()
            .map(|w| (w.kind, w.origin))
            .collect();
        let origin = |k: WakeKind, src_human: bool| {
            got.iter()
                .filter(|(kind, _)| *kind == k)
                .map(|(_, o)| o.as_str())
                .collect::<Vec<_>>()
                .get(usize::from(!src_human))
                .copied()
        };
        assert_eq!(origin(WakeKind::Message, true), Some(HUMAN), "人发的消息");
        assert_eq!(
            origin(WakeKind::Message, false),
            Some(SYSTEM),
            "系统发的消息"
        );
        assert_eq!(origin(WakeKind::System, true), Some(SYSTEM));
        assert_eq!(origin(WakeKind::Timer, true), Some("w-1"));
        assert_eq!(origin(WakeKind::AskAnswer, true), Some(HUMAN), "问的答复");
    }

    #[tokio::test]
    async fn next_not_before_after_ignores_ones_already_due() {
        let (_s, port, _d) = seeded().await;
        let at = |src: &str, ms: i64| NewWake {
            not_before: Ms(ms),
            ..new_wake(
                "w-1",
                WakeKind::Timer,
                source::timer(src),
                Priority::Auto,
                "w-1",
            )
        };
        port.enqueue(at("past", 10)).await.unwrap();
        port.enqueue(at("future", 500)).await.unwrap();
        assert_eq!(
            port.next_not_before_after(Ms(100)).await.unwrap(),
            Some(Ms(500))
        );
        assert_eq!(port.next_not_before_after(Ms(600)).await.unwrap(), None);
    }

    /// C21：熔断 id 只经 collab 的 `Scope` 互转；session id 里带 `-` 也一样往返。
    #[tokio::test]
    async fn latches_round_trip_both_scopes() {
        let (_s, port, _d) = seeded().await;
        let edge_id = breaker::edge_id("cap-1", "w-1");
        let edge = Latch {
            scope: LatchScope::Edge,
            id: edge_id.clone(),
            reason: "no-progress:9/10min".into(),
            at: Ms(0),
        };
        let target = Latch {
            scope: LatchScope::Session,
            id: "w-1".into(),
            reason: "no-progress:9/10min".into(),
            at: Ms(0),
        };
        port.put_latch(&edge).await.unwrap();
        port.put_latch(&target).await.unwrap();
        let mut got: Vec<(LatchScope, String)> = port
            .latches()
            .await
            .unwrap()
            .into_iter()
            .map(|l| (l.scope, l.id))
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                (LatchScope::Session, "w-1".to_string()),
                (LatchScope::Edge, edge_id.clone())
            ]
        );
        port.clear_latch(LatchScope::Edge, &edge_id).await.unwrap();
        assert_eq!(port.latches().await.unwrap().len(), 1);
        assert!(
            port.put_latch(&Latch {
                id: "没有箭头".into(),
                ..edge
            })
            .await
            .is_err(),
            "认不出的 edge id 不写"
        );
    }

    #[tokio::test]
    async fn patching_runtime_columns_never_erases_the_agent_session_id() {
        let (_s, port, _d) = seeded().await;
        port.ledger("w-1").record("agent-1").unwrap();
        let patch = SessionPatch {
            desired: Some(Desired::Running),
            was_working: Some(true),
            resume_failed_at: Some(Some(Ms(1))),
            ..Default::default()
        };
        port.patch_session("w-1", patch).await.unwrap();
        let row = port.session("w-1").await.unwrap().unwrap();
        assert_eq!(row.agent_session_id.as_deref(), Some("agent-1"), "§18 遗留");
        assert_eq!((row.desired, row.was_working), (Desired::Running, true));
        assert!(row.resume_failed_at.is_some());
        assert_eq!(
            row.display_name.as_deref(),
            Some("名字 w-1"),
            "没有显示名 = 标题"
        );
        assert!(!row.crew_removed);
    }

    #[tokio::test]
    async fn lobby_context_has_every_crew_session_and_the_captain_as_owner() {
        let (_s, port, _d) = seeded().await;
        let ctx = port
            .channel_ctx("c-1/lobby", Ms(crewd_store::now_ms()))
            .await
            .unwrap()
            .unwrap();
        let mut m = ctx.session_members.clone();
        m.sort();
        assert_eq!(m, ["cap-1", "w-1", "w-2"]);
        assert_eq!(ctx.owner.as_deref(), Some("cap-1"));
        assert_eq!(ctx.crew_captain.as_deref(), Some("cap-1"));
        assert!(ctx.dm_sessions.is_empty());
        assert_eq!(port.captain("c-1").await.unwrap().as_deref(), Some("cap-1"));
        assert!(port.channel_ctx("nope", Ms(0)).await.unwrap().is_none());
    }

    /// spec §26.5 / 合并报告的遗留：`pending()` 与 `claim` 之间被作废的行（人停 / 机组移除）→ collab 回
    /// Conflict `wake.bad_transition {from: dropped, to: inflight}`，一行都不拿（全有或全无）。
    #[tokio::test]
    async fn claim_of_a_row_dropped_meanwhile_is_a_conflict_and_claims_nothing() {
        let (_s, port, _d) = seeded().await;
        for t in ["a", "b"] {
            port.enqueue(new_wake(
                "w-1",
                WakeKind::Timer,
                source::timer(t),
                Priority::Auto,
                "w-1",
            ))
            .await
            .unwrap();
        }
        let wakes = port.pending("w-1").await.unwrap();
        port.settle(
            &[wakes[1].id],
            Settle::Dropped {
                error: "stopped".into(),
            },
            Ms(0),
        )
        .await
        .unwrap();
        let b = Batch {
            session: "w-1".into(),
            wakes: wakes.clone(),
            ..Default::default()
        };
        let e = port.claim(&b).await.unwrap_err();
        let refusal = e
            .downcast_ref::<collab::CollabError>()
            .and_then(|c| c.refusal().cloned())
            .expect("规则拒绝");
        assert_eq!(refusal.code.as_str(), "wake.bad_transition", "{e:#}");
        let after = port.pending("w-1").await.unwrap();
        assert_eq!(after.len(), 1, "没被作废的那一行还是 pending，没被拿走");
        assert_eq!(after[0].id, wakes[0].id);
    }

    /// `render_wake`：正文按这一批的顺序（不是 id 顺序）；渲染不了的行在同一调用里记 dropped 并报回；游标推进点是带上的消息。
    #[tokio::test]
    async fn render_wake_keeps_batch_order_drops_unusable_rows_and_reports_cursor_targets() {
        let (state, port, _d) = seeded().await;
        let m = post_as(
            &state,
            collab::Sender::Session("cap-1".into()),
            "c-1/lobby",
            "合成派活甲",
            &["w-1"],
        )
        .await;
        let msg = NewWake {
            session: "w-1".into(),
            kind: WakeKind::Message,
            source: source::message(&m.id),
            priority: Priority::Auto,
            not_before: Ms(0),
            note: None,
            origin: "cap-1".into(),
        };
        let gone = NewWake {
            source: source::message("没有这条"),
            ..msg.clone()
        };
        let timer = NewWake {
            kind: WakeKind::Timer,
            source: source::timer("合成"),
            note: Some("合成提醒乙".into()),
            origin: "w-1".into(),
            ..msg.clone()
        };
        let a = port.enqueue(msg).await.unwrap().unwrap();
        let b = port.enqueue(gone).await.unwrap().unwrap();
        let c = port.enqueue(timer).await.unwrap().unwrap();
        let rows = port.pending("w-1").await.unwrap();
        let by = |id: i64| rows.iter().find(|w| w.id == id).unwrap().clone();
        // 定时（id 最新）排在消息前面：正文跟着这个次序，不是 `by_ids` 的 id 次序。
        let r = port
            .render_wake("w-1", &[by(c), by(a), by(b)])
            .await
            .unwrap();
        let (t, mt) = (
            r.text.find("合成提醒乙").expect("定时的附言"),
            r.text.find("合成派活甲").expect("消息正文"),
        );
        assert!(t < mt, "{}", r.text);
        assert_eq!(r.dropped, [b], "消息没了 = 渲染不了");
        assert!(r.deferred.is_empty());
        assert_eq!(r.cursor_targets, [("c-1/lobby".to_string(), m.seq)]);
        let all = port.list_wakes(Some("w-1")).await.unwrap();
        let dropped = all.iter().find(|w| w.id == b).unwrap();
        assert_eq!(
            dropped.state,
            crewd_wake::WakeState::Dropped,
            "同一调用里记了 dropped"
        );
        assert!(dropped.last_error.is_some());
    }

    /// Port::ack（Delivered）在同一个事务里推游标，交回这批的来源；Failed 按库里的次数退避或放弃。
    #[tokio::test]
    async fn ack_delivers_with_cursors_and_fails_by_the_stored_attempts() {
        let (state, port, _d) = seeded().await;
        let m = post_as(
            &state,
            collab::Sender::Session("cap-1".into()),
            "c-1/lobby",
            "合成派活乙",
            &["w-1"],
        )
        .await;
        port.enqueue(new_wake(
            "w-1",
            WakeKind::Message,
            source::message(&m.id),
            Priority::Auto,
            "cap-1",
        ))
        .await
        .unwrap();
        let b = Batch {
            session: "w-1".into(),
            wakes: port.pending("w-1").await.unwrap(),
            riders: vec![],
            cursor_targets: vec![("c-1/lobby".into(), m.seq)],
        };
        port.claim(&b).await.unwrap();
        let acked = port
            .ack(
                &b,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await
            .unwrap();
        assert_eq!(acked.origins, ["cap-1"]);
        assert_eq!(port.cursor("w-1", "c-1/lobby").await.unwrap(), m.seq);
        assert!(port.pending("w-1").await.unwrap().is_empty());

        port.enqueue(new_wake(
            "w-1",
            WakeKind::Timer,
            source::timer("f"),
            Priority::Auto,
            "w-1",
        ))
        .await
        .unwrap();
        let b = Batch {
            session: "w-1".into(),
            wakes: port.pending("w-1").await.unwrap(),
            ..Default::default()
        };
        let mut gave_up = false;
        for _ in 0..crewd_wake::queue::MAX_ATTEMPTS {
            port.claim(&b).await.unwrap();
            let a = port
                .ack(
                    &b,
                    Ack::Failed {
                        error: "e".into(),
                        now: Ms(0),
                        jitter: 0.0,
                    },
                )
                .await
                .unwrap();
            gave_up = !a.gave_up.is_empty();
            if !gave_up {
                assert_eq!(a.retried.len(), 1);
            }
        }
        assert!(gave_up, "到上限放弃（次数是库里 claim 记的）");
    }

    /// 9a 修复 1：放弃的行带回来，哪怕后面又有 205 条更新的行（不靠封顶的 `list_wakes` 找）。
    #[tokio::test]
    async fn fail_inflight_hands_back_given_up_rows_however_many_newer_rows_there_are() {
        let (_s, port, _d) = seeded().await;
        port.enqueue(new_wake(
            "w-1",
            WakeKind::Timer,
            source::timer("crash"),
            Priority::Auto,
            "w-1",
        ))
        .await
        .unwrap();
        let first = port.pending("w-1").await.unwrap();
        let b = Batch {
            session: "w-1".into(),
            wakes: first.clone(),
            ..Default::default()
        };
        for i in 0..205 {
            let w = new_wake(
                "w-2",
                WakeKind::Timer,
                source::timer(&format!("n{i}")),
                Priority::Auto,
                "w-2",
            );
            port.enqueue(w).await.unwrap();
        }
        let mut reclaimed = Reclaimed::default();
        for _ in 0..crewd_wake::queue::MAX_ATTEMPTS {
            port.claim(&b).await.unwrap();
            reclaimed = port.fail_inflight("crewd restarted", Ms(0)).await.unwrap();
        }
        assert_eq!(reclaimed.gave_up.len(), 1);
        assert_eq!(reclaimed.gave_up[0].id, first[0].id);
        assert_eq!(reclaimed.gave_up[0].session, "w-1");
    }

    #[tokio::test]
    async fn collab_changes_become_orchestra_facts() {
        let ch = Change::RestartAnswered {
            id: 3,
            answer: RestartAnswer::Resume,
            session_ids: vec!["w-1".into()],
        };
        assert_eq!(
            fact_of(&ch),
            Some(CollabFact::RestartAnswered {
                prompt: 3,
                resume: true,
                sessions: vec!["w-1".into()]
            })
        );
        assert_eq!(
            fact_of(&Change::WakeEnqueued {
                wake_id: 1,
                session_id: "w-1".into()
            }),
            Some(CollabFact::WakeEnqueued {
                session: "w-1".into()
            })
        );
        assert_eq!(
            fact_of(&Change::Progress {
                session_id: "w-1".into(),
                kind: crewd_collab::ProgressKind::ToolCall
            }),
            Some(CollabFact::Progress {
                session: "w-1".into()
            }),
            "C3：产出信号进熔断"
        );
        assert_eq!(
            fact_of(&Change::MessageAppended {
                channel_id: "c-1/lobby".into(),
                seq: 7,
                message_id: "m".into(),
                sender_session_id: None,
            }),
            Some(CollabFact::MessageAppended {
                channel: "c-1/lobby".into(),
                seq: 7
            })
        );
        let cleared = vec![("W1".to_string(), Some("w-1".to_string()))];
        assert_eq!(
            fact_of(&Change::AskAnswered {
                crew_id: "c-1".into(),
                ask: "H1".into(),
                asked_by: None,
                cleared: cleared.clone(),
            }),
            Some(CollabFact::AskAnswered {
                crew: "c-1".into(),
                ask: "H1".into(),
                cleared
            })
        );
        assert_eq!(
            fact_of(&Change::CrewChanged {
                crew_id: "c-1".into()
            }),
            None
        );
    }

    /// 订阅落后（容量 2 的通道里塞了 5 条）→ sink 见到 `Lagged`；通道关了 → 泵退出。
    #[tokio::test]
    async fn a_lagging_pump_reports_lagged_and_ends_when_the_channel_closes() {
        let (tx, rx) = broadcast::channel(2);
        for i in 0..5 {
            tx.send(Change::WakeEnqueued {
                wake_id: i,
                session_id: "w-1".into(),
            })
            .unwrap();
        }
        drop(tx);
        let mut seen = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            pump_collab(rx, |f| seen.push(f)),
        )
        .await
        .expect("通道关了泵就退出");
        assert_eq!(seen[0], CollabFact::Lagged, "{seen:?}");
        assert_eq!(seen.len(), 3, "落后一次 + 还在通道里的两条");
    }

    /// 9a 复审遗留（真 Hub）：别人入了一条 5 秒后到点的唤醒 → Hub 的 `WakeEnqueued` 经桥进 actor → 下一拍缓存作废，
    /// actor 的下一个定时拍正好是那条的 not_before。
    #[tokio::test]
    async fn an_outside_enqueue_through_the_real_hub_moves_the_actors_next_deadline() {
        use crate::orchestra::clock::ManualClock;
        use crate::orchestra::exec::Executors;
        use crate::orchestra::hooks::Hooks;
        use crate::orchestra::{OrchestraConfig, spawn};
        const T0: i64 = 1_000_000_000;
        let (state, port, _d) = seeded().await;
        let rx = state.hub.subscribe_collab();
        let (h, _actor) = spawn(
            Arc::new(port),
            Executors::new(),
            Hooks::default(),
            Arc::new(ManualClock::new(Ms(T0))),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        let _pump = forward_collab(rx, h.clone());
        let hb = OrchestraConfig::for_tests().heartbeat_ms;
        assert_eq!(h.next_deadline().await, Some(Ms(T0 + hb)), "先是心跳");
        state
            .collab(|tx| {
                cw::enqueue(
                    tx,
                    WakeRequest {
                        session: "w-1".into(),
                        kind: cw::WakeKind::Timer,
                        source: cw::source::timer("outside"),
                        priority: WakePriority::Auto,
                        not_before_ms: Some(T0 + 5_000),
                        note: None,
                        origin: Some("w-1".into()),
                    },
                )
            })
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let got = h.next_deadline().await;
            if got == Some(Ms(T0 + 5_000)) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "5 秒内没等到桥把入队交给 actor：{got:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
