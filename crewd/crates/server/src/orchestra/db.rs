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
use crewd_collab::cursor_reservations as cr;
use crewd_collab::wakes::{self as cw, Enqueued, WakePriority, WakeRequest, WakeRow};
use crewd_exec::resume::AgentIdLedger;
#[cfg(test)]
use crewd_proto::Message;
use crewd_proto::{MentionKind, RestartAnswer, SenderKind};
use crewd_wake::breaker::{HUMAN, SYSTEM};
use crewd_wake::plan::{ChannelCtx, MsgFacts};
#[cfg(test)]
use crewd_wake::plan::{Mention, Sender};
use crewd_wake::text::Lang;
use crewd_wake::{Latch, LatchScope, MessageRef, Ms, NewWake, Priority, QueuedWake, WakeKind};
use tokio::sync::broadcast::{self, error::RecvError};

use super::Handle;
use super::core::CollabFact;
use super::port::*;
use crate::AppState;
use crate::hub::CollabEvent;
use crate::ledger::DbLedger;

pub struct DbPort {
    state: Arc<AppState>,
}

impl DbPort {
    pub fn new(state: Arc<AppState>, _db_path: PathBuf) -> Self {
        Self { state }
    }

    async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> collab::Result<T> + Send + 'static,
    ) -> Result<T> {
        Ok(self.state.db.call_result(move |s| f(s.conn())).await??)
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

pub(super) fn queued(conn: &rusqlite::Connection, row: &WakeRow) -> collab::Result<QueuedWake> {
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

#[cfg(test)]
pub(crate) use crate::wake_adapter::facts_of;
use crate::wake_adapter::message_facts as facts_with_provenance;

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
        Arc::new(DbLedger::new(self.state.db.clone(), session))
    }

    async fn worldview(&self, session: &str) -> Result<String> {
        // Shared launch/SessionStart source: DB read is released before the bounded quota read.
        // OrchestraControl's source is outside the actor mailbox; provider feeding is unavailable.
        let ctx = crate::agent::port::world_context(&self.state, session).await?;
        Ok(collab::worldview::render(&ctx))
    }

    fn causal_fence(&self) -> Option<Arc<crate::causal_fence::CausalFence>> {
        Some(self.state.causal_fence.clone())
    }

    fn mint_token(&self, session: &str) -> String {
        self.state.tokens.mint(session)
    }

    fn revoke_token(&self, session: &str) {
        self.state.tokens.revoke(session);
    }

    async fn enqueue(&self, w: NewWake) -> Result<Option<i64>> {
        if w.kind == WakeKind::StopCheck && w.source == super::work_idle::CANDIDATE_SOURCE {
            let candidate: crewd_collab::stop_checks::Candidate =
                serde_json::from_str(w.note.as_deref().unwrap_or(""))?;
            anyhow::ensure!(
                candidate.session() == w.session,
                "idle candidate session mismatch"
            );
            let result = self
                .write(move |tx| crewd_collab::stop_checks::accept(tx, &candidate))
                .await?;
            return Ok(match result {
                crewd_collab::stop_checks::Accepted::Rewake(id) => Some(id),
                _ => None,
            });
        }
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

    async fn human_stop(&self, session: &str) -> Result<Vec<QueuedWake>> {
        let session = session.to_string();
        self.write(move |tx| {
            cw::human_stop(tx, &session)?
                .iter()
                .map(|w| queued(tx.conn(), w))
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

    async fn claim_reserved(&self, b: &Batch, generation: u64) -> Result<Decision<BatchLease>> {
        if let Some(PreparedSnapshot::CausalDatabase {
            reservation,
            causes,
        }) = &b.prepared
        {
            let (reservation, causes) = (reservation.clone(), causes.clone());
            let windows = reservation.windows_for(&b.cursor_targets)?;
            let ids = b.wakes.iter().map(|w| w.id).collect::<Vec<_>>();
            let riders = b.riders.iter().map(|w| w.id).collect::<Vec<_>>();
            let owner = cr::Owner::new(generation)?;
            return Ok(
                match self
                    .write(move |tx| {
                        cr::claim_output(tx, &reservation, &causes, &windows, &ids, &riders, owner)
                    })
                    .await?
                {
                    Decision::Applied(claimed) => Decision::Applied(BatchLease::CausalDatabase {
                        claimed: Box::new(claimed),
                        observed: None,
                    }),
                    Decision::Stale => Decision::Stale,
                    Decision::Busy => Decision::Busy,
                },
            );
        }
        let Some(PreparedSnapshot::Database(prepared)) = &b.prepared else {
            anyhow::bail!("actual body has no database preparation snapshot");
        };
        let prepared = prepared.clone();
        let windows = prepared.windows_for(&b.cursor_targets)?;
        let wake_ids: Vec<_> = b.wakes.iter().map(|w| w.id).collect();
        let rider_ids: Vec<_> = b.riders.iter().map(|w| w.id).collect();
        let owner = cr::Owner::new(generation)?;
        let decision = self
            .write(move |tx| {
                cr::claim_reserved(tx, &prepared, &windows, &wake_ids, &rider_ids, owner)
            })
            .await?;
        Ok(match decision {
            Decision::Applied(lease) => Decision::Applied(BatchLease::Database(lease)),
            Decision::Stale => Decision::Stale,
            Decision::Busy => Decision::Busy,
        })
    }

    async fn settle_batch(&self, b: &Batch, s: Settle, now: Ms) -> Result<Decision<()>> {
        if let Some(BatchLease::CausalDatabase { claimed, .. }) = &b.lease {
            let claimed = claimed.clone();
            return self
                .write(move |tx| match s {
                    Settle::Requeue => cr::settle_output(tx, &claimed, cr::Terminal::Requeue),
                    Settle::Dropped { error } => {
                        cr::settle_output(tx, &claimed, cr::Terminal::Dropped(&error))
                    }
                    Settle::Delivered => Err(crewd_store::StoreError::Invalid(
                        "reserved Delivered requires success evidence".into(),
                    )
                    .into()),
                })
                .await;
        }
        if let Some(BatchLease::Database(lease)) = &b.lease {
            let lease = lease.clone();
            return self
                .write(move |tx| {
                    let fate = match &s {
                        Settle::Requeue => cr::Terminal::Requeue,
                        Settle::Dropped { error } => cr::Terminal::Dropped(error),
                        Settle::Delivered => {
                            return Err(crewd_store::StoreError::Invalid(
                                "reserved Delivered requires actual delivery ack".into(),
                            )
                            .into());
                        }
                    };
                    cr::settle_reserved(tx, &lease, fate)
                })
                .await;
        }
        if b.prepared.is_some() {
            anyhow::bail!("prepared body has no claimed lease");
        }
        self.settle(&b.ids(), s, now).await?;
        Ok(Decision::Applied(()))
    }

    async fn ack(&self, b: &Batch, a: Ack) -> Result<Acked> {
        if let Some(BatchLease::CausalDatabase { claimed, observed }) = &b.lease {
            let claimed = claimed.clone();
            return match a {
                Ack::Delivered { riders } => {
                    let observed = observed.clone();
                    let result = self
                        .write(move |tx| {
                            let observed = match observed {
                                Some(success) => success,
                                None => {
                                    if cr::output_receipt_retired(tx, &claimed)? {
                                        return Ok(Decision::Stale);
                                    }
                                    return Err(crewd_store::StoreError::Invalid(
                                        "missing original emitted success".into(),
                                    )
                                    .into());
                                }
                            };
                            let riders = match &riders {
                                Riders::Release => cw::Riders::Release,
                                Riders::Drop(why) => cw::Riders::Drop(why),
                            };
                            cr::ack_output_delivered(tx, &claimed, &observed, riders)
                        })
                        .await?;
                    match result {
                        Decision::Applied(_) => Ok(Acked {
                            origins: acked_origins(b),
                            ..Default::default()
                        }),
                        Decision::Stale => Ok(Acked {
                            stale: true,
                            ..Default::default()
                        }),
                        Decision::Busy => anyhow::bail!(
                            "causal Delivered is busy; exact successful fate retained"
                        ),
                    }
                }
                Ack::Failed { error, now, jitter } => {
                    let result = self
                        .write(move |tx| {
                            cr::ack_output_failed(tx, &claimed, &error, |n| {
                                crewd_wake::queue::after_failure(n, now, jitter).map(|t| t.0)
                            })
                        })
                        .await?;
                    match result {
                        Decision::Stale => Ok(Acked {
                            stale: true,
                            ..Default::default()
                        }),
                        Decision::Busy => {
                            anyhow::bail!("causal Failed is busy; exact fate retained")
                        }
                        Decision::Applied(failed) => {
                            let mut out = Acked::default();
                            for f in failed.into_iter().filter(|f| !f.cancelled) {
                                if let Some(t) = f.retry_at_ms {
                                    out.retried.push((f.id, Ms(t)));
                                } else {
                                    out.gave_up.push(f.id);
                                }
                            }
                            Ok(out)
                        }
                    }
                }
            };
        }
        if let Some(BatchLease::Database(lease)) = &b.lease {
            let lease = lease.clone();
            return match a {
                Ack::Delivered { riders } => {
                    let decision = self
                        .write(move |tx| {
                            let riders = match &riders {
                                Riders::Release => cw::Riders::Release,
                                Riders::Drop(why) => cw::Riders::Drop(why),
                            };
                            cr::ack_reserved_delivered(tx, &lease, riders)
                        })
                        .await?;
                    Ok(match decision {
                        Decision::Applied(()) => Acked {
                            origins: acked_origins(b),
                            ..Default::default()
                        },
                        _ => Acked {
                            stale: true,
                            ..Default::default()
                        },
                    })
                }
                Ack::Failed { error, now, jitter } => {
                    let decision = self
                        .write(move |tx| {
                            cr::ack_reserved_failed(tx, &lease, &error, |n| {
                                crewd_wake::queue::after_failure(n, now, jitter).map(|t| t.0)
                            })
                        })
                        .await?;
                    let Decision::Applied(failed) = decision else {
                        return Ok(Acked {
                            stale: true,
                            ..Default::default()
                        });
                    };
                    let mut out = Acked::default();
                    for f in failed.into_iter().filter(|f| !f.cancelled) {
                        match f.retry_at_ms {
                            Some(t) => out.retried.push((f.id, Ms(t))),
                            None => out.gave_up.push(f.id),
                        }
                    }
                    Ok(out)
                }
            };
        }
        if b.prepared.is_some() {
            anyhow::bail!("prepared delivery ack has no claimed lease");
        }
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
                for f in failed.into_iter().filter(|f| !f.cancelled) {
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
            let failed = cr::reset_inflight_and_reservations(tx, &error, |n| {
                crewd_wake::queue::after_restart(n, now).map(|t| t.0)
            })?;
            let mut out = Reclaimed::default();
            let mut gave_up = Vec::new();
            for f in failed.into_iter().filter(|f| !f.cancelled) {
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

    async fn prepare_causal_admission(&self, session: &str, _now: Ms) -> Result<()> {
        let session = session.to_owned();
        self.write(move |tx| {
            let captured = cr::prepare_causal_unread(tx, &session)?;
            crate::causal_fence::blocked_channels(tx, &captured.causes)?;
            Ok(())
        })
        .await
    }
    async fn causal_diagnostics(
        &self,
        session: &str,
        now: Ms,
    ) -> Result<Vec<(String, crewd_wake::explain::CausalBlockage)>> {
        let session = session.to_owned();
        self.read(move |conn| {
            Ok(crate::causal_fence::diagnostic_values(
                collab::causal::diagnostics(conn, &session, now.0)?,
            ))
        })
        .await
    }
    async fn causal_holds(
        &self,
        target: Option<&str>,
    ) -> Result<Vec<crewd_collab::causal::ChainHold>> {
        let target = target.map(str::to_owned);
        self.read(move |conn| crewd_collab::causal::holds(conn, target.as_deref()))
            .await
    }
    async fn recover_chain(
        &self,
        chain: &str,
        from: &str,
        target: &str,
        action: &crewd_collab::causal::HumanRecoveryAction,
    ) -> Result<bool> {
        let (chain, from, target, action) = (
            chain.to_owned(),
            from.to_owned(),
            target.to_owned(),
            action.clone(),
        );
        self.write(move |tx| crewd_collab::causal::recover(tx, &chain, &from, &target, &action))
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

    async fn locale(&self) -> Result<Lang> {
        self.read(|c| {
            Ok(match collab::settings::locale(c)? {
                collab::Locale::Zh => Lang::Zh,
                collab::Locale::En => Lang::En,
            })
        })
        .await
    }

    /// 一个 collab 事务：任何一步被拒（collab 的规则，如上限 0）整个回滚。
    async fn set_settings(&self, u: &SettingsUpdate) -> Result<()> {
        let max = match u.max_live {
            Some(m) => Some(m.map(u32::try_from).transpose()?),
            None => None,
        };
        let (lang, secs) = (u.lang, u.hibernate_deadline_secs);
        self.write(move |tx| {
            if let Some(l) = lang {
                let l = match l {
                    Lang::Zh => collab::Locale::Zh,
                    Lang::En => collab::Locale::En,
                };
                collab::settings::set_locale(tx, l)?;
            }
            if let Some(secs) = secs {
                collab::settings::set_hibernate_deadline_secs(tx, secs)?;
            }
            if let Some(max) = max {
                collab::settings::set_max_live_sessions(tx, max)?;
            }
            Ok(())
        })
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

    async fn enqueue_work_checkpoints(&self) -> Result<()> {
        self.write(|tx| collab::checkpoints::enqueue_due(tx).map(|_| ()))
            .await
    }

    async fn apply_due_defaults(&self) -> Result<()> {
        self.write(|tx| collab::asks::apply_all_due_defaults(tx).map(|_| ()))
            .await
    }

    async fn prune_wakes(&self, before: Ms, limit: usize) -> Result<usize> {
        let limit = u32::try_from(limit).unwrap_or(u32::MAX);
        self.write(move |tx| cw::prune(tx, before.0, limit)).await
    }

    async fn hibernate_deadline_secs(&self) -> Result<Option<u32>> {
        self.read(collab::settings::hibernate_deadline_secs).await
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
            opt(collab::messages::get_by_seq(c, r.seq))?
                .filter(|m| m.channel_id == r.channel)
                .map(|m| facts_with_provenance(c, &m, now).map(|f| (f, m.text.clone())))
                .transpose()
        })
        .await
    }

    async fn channel_ctx(&self, channel: &str, now: Ms) -> Result<Option<ChannelCtx>> {
        let channel = channel.to_string();
        self.read(move |c| crate::wake_adapter::channel_context(c, &channel, now))
            .await
    }

    async fn unread(&self, session: &str) -> Result<Vec<MsgFacts>> {
        let session = session.to_string();
        let now = Ms(crewd_store::now_ms());
        // 写事务：`unread_for` 会把缺的游标钉到群末尾（入群点）。
        self.write(move |tx| {
            let blocks = collab::cursors::unread_for(tx, &session)?;
            blocks
                .iter()
                .flat_map(|(_, messages)| messages.iter())
                .map(|m| facts_with_provenance(tx.conn(), m, now))
                .collect()
        })
        .await
    }

    async fn cursor(&self, session: &str, channel: &str) -> Result<i64> {
        let (session, channel) = (session.to_string(), channel.to_string());
        self.read(move |c| Ok(collab::cursors::cursor(c, &session, &channel)?.unwrap_or(0)))
            .await
    }

    async fn post_system(&self, to: Audience, text: String) -> Result<()> {
        self.write(move |tx| {
            let Audience::HumanChannel {
                crew,
                mention_human,
            } = to;
            let home = collab::crews::home_of(tx.conn(), &crew)?;
            let channel = collab::channels::ensure_human_channel(tx, &home)?.id;
            let mut mentions = Vec::new();
            if mention_human {
                mentions.push(crewd_proto::Mention {
                    kind: MentionKind::Human,
                    target_id: None,
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
        #[cfg(feature = "testkit")]
        let gate_session = session.clone();
        let ids: Vec<i64> = batch.iter().map(|w| w.id).collect();
        let now_ms = crewd_store::now_ms();
        let (text, dropped, deferred, cursor_targets, prepared, already_injected) = self
            .write(move |tx| {
                // Normalize absent cursors and leading-own prefixes only with the shared
                // reservation-aware scanner. Rendering and snapshot stay in this same job.
                let captured = cr::prepare_causal_unread(tx, &session)?;
                let blocked = crate::causal_fence::blocked_channels(tx, &captured.causes)?;
                let c = tx.conn();
                // `by_ids` 按 id 排；正文的顺序要跟这一批（按优先级选好的）一致。
                let mut rows: Vec<WakeRow> = cw::by_ids(c, &ids)?;
                rows.sort_by_key(|r| ids.iter().position(|i| *i == r.id));
                // Unsafe channel bodies are indivisible; safe channels still render normally.
                let mut allowed = Vec::new();
                for row in rows {
                    let blocked_message = match row.source.strip_prefix("msg:") {
                        Some(id) => crewd_store::messages::get(c, id)?
                            .is_some_and(|m| blocked.contains(&m.channel_id)),
                        None => false,
                    };
                    if !blocked_message {
                        allowed.push(row);
                    }
                }
                let mut rows = allowed;

                let locale = collab::settings::locale(c)?;
                // 报错（如账本坏了）是 fail-loud：原样上抛，Core 记日志、这批留在 pending，不在这里重试。
                let original = collab::render::build_batch(c, &session, locale, &rows, now_ms)?;
                let mut already = Vec::new();
                for row in &rows {
                    if row.state != cw::WakeState::Pending
                        || row.kind != cw::WakeKind::Message
                        || original.unusable.iter().any(|(id, _)| *id == row.id)
                    {
                        continue;
                    }
                    if let Some(id) = row.source.strip_prefix("msg:")
                        && let Some(m) = crewd_store::messages::get(c, id)?
                        && collab::cursors::cursor(c, &session, &m.channel_id)?
                            .is_some_and(|base| base >= m.seq)
                    {
                        already.push(row.id);
                    }
                }
                // The renderer may lower to min queued seq. Remove only actually announced,
                // eligible pending rows atomically, then render afresh; never claim the old body.
                if !already.is_empty() {
                    cw::mark_delivered(tx, &already)?;
                }
                rows.retain(|r| !already.contains(&r.id));
                let c = tx.conn();
                let b = collab::render::build_batch(c, &session, locale, &rows, now_ms)?;
                let text = collab::render::wake_text(locale, &b);
                let targets = b.cursor_targets();
                let channels: Vec<_> = targets.iter().map(|(ch, _)| ch.clone()).collect();
                let mut reservation = cr::snapshot(tx.conn(), &session, &channels, now_ms)?;
                reservation.capture_work_followups(tx.conn(), &rows, now_ms)?;
                let causes = captured.causes.select(&targets)?;
                let prepared = PreparedSnapshot::CausalDatabase {
                    reservation,
                    causes,
                };
                // R6 / S45：`deferred` 随这批走（Core 一起 claim）；`cursor_targets()` 是送到后游标推到哪。
                Ok((
                    text,
                    b.unusable.clone(),
                    b.deferred.clone(),
                    targets,
                    prepared,
                    already,
                ))
            })
            .await?;
        #[cfg(feature = "testkit")]
        crate::agent::testkit::pause_caller(
            &self.state,
            crate::agent::testkit::CallerStage::WakeRendered,
            &gate_session,
            cursor_targets.clone(),
            text.clone(),
        )
        .await;
        let mut dropped_ids: Vec<i64> = dropped.iter().map(|(id, _)| *id).collect();
        dropped_ids.extend(already_injected);
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
            prepared: Some(prepared),
        })
    }

    async fn runner_turn_end(&self, session: &str, turn_id: &str, text: &str) -> Result<()> {
        let (session, turn_id, text) = (session.to_string(), turn_id.to_string(), text.to_string());
        self.write(move |tx| {
            crewd_collab::turn_trace::on_turn_end(tx, &session, &turn_id, &text)?;
            Ok(())
        })
        .await
    }

    async fn record_usage(
        &self,
        session: &str,
        snapshot: crewd_exec::session::RunnerUsageSnapshot,
        observed_at: Ms,
    ) -> Result<()> {
        let session = session.to_owned();
        self.write(move |tx| {
            crewd_store::runner_usage::append(
                tx.conn(),
                &session,
                crewd_store::runner_usage::Snapshot {
                    provider: &snapshot.provider,
                    thread_id: &snapshot.thread_id,
                    turn_id: snapshot.turn_id.as_deref(),
                    token_usage_json: &snapshot.token_usage_json,
                },
                observed_at.0,
            )?;
            Ok(())
        })
        .await
    }

    fn publish(&self, e: OrchestraEvent) {
        let event = match e {
            OrchestraEvent::RunnerItem {
                session,
                turn_id,
                item,
            } => crewd_proto::Event {
                event: crewd_proto::orchestra::events::SESSION_RUNNER_ITEM.into(),
                data: serde_json::json!({"sessionId":session,"turnId":turn_id,"item":item}),
            },
            OrchestraEvent::RunnerProfile { session, profile } => crewd_proto::Event {
                event: crewd_proto::orchestra::events::SESSION_RUNNER_PROFILE.into(),
                data: serde_json::json!({"sessionId":session,"model":profile.model,"effort":profile.effort,"fast":profile.fast}),
            },
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

/// Test-only single-use failure on the same spawned pump future; no alternate dispatch.
#[cfg(test)]
pub(crate) fn forward_collab_with_fault(
    rx: broadcast::Receiver<CollabEvent>,
    handle: Handle,
    fault: Option<tokio::sync::oneshot::Receiver<()>>,
) -> tokio::task::JoinHandle<()> {
    match fault {
        None => forward_collab(rx, handle),
        Some(fault) => tokio::spawn(async move {
            tokio::select! {
                _ = pump_collab(rx, move |f| handle.collab(f)) => {},
                result = fault => { if result.is_ok() { panic!("S5 owned Hub bridge fault"); } }
            }
        }),
    }
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

    #[tokio::test]
    async fn codex_turn_end_uses_real_collab_question_and_replay_rules() {
        let (state, port, _dir) = seeded().await;
        let count = || {
            state.db.call(|store| {
                store.conn().query_row(
                    "SELECT count(*) FROM messages WHERE channel_id='c-1/lobby'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
        };
        let before = count().await.unwrap().unwrap();
        port.runner_turn_end("w-1", "codex-turn-1", "Please choose the next step?")
            .await
            .unwrap();
        assert_eq!(
            port.session("w-1")
                .await
                .unwrap()
                .unwrap()
                .awaiting_question
                .as_deref(),
            Some("Please choose the next step?")
        );
        let after = count().await.unwrap().unwrap();
        assert_eq!(after, before + 1, "real Stop relay reaches the crew");
        port.runner_turn_end("w-1", "codex-turn-1", "Please choose the next step?")
            .await
            .unwrap();
        assert_eq!(
            count().await.unwrap().unwrap(),
            after,
            "same turn is not another question"
        );
        port.runner_turn_end("w-1", "codex-turn-2", "Completed the task.")
            .await
            .unwrap();
        assert_eq!(
            port.session("w-1")
                .await
                .unwrap()
                .unwrap()
                .awaiting_question,
            None
        );
        assert_eq!(count().await.unwrap().unwrap(), after);
        let mut rx = state.hub.subscribe();
        port.publish(OrchestraEvent::RunnerItem {
            session: "w-1".into(),
            turn_id: "original-turn".into(),
            item: serde_json::json!({"id":"original-item","type":"agentMessage","text":"result"}),
        });
        let event = rx.recv().await.unwrap();
        assert_eq!(
            event.event,
            crewd_proto::orchestra::events::SESSION_RUNNER_ITEM
        );
        assert_eq!(event.data["sessionId"], "w-1");
        assert_eq!(event.data["turnId"], "original-turn");
        assert_eq!(event.data["item"]["id"], "original-item");
    }

    #[tokio::test]
    async fn s4_read_owner_orchestra_real_sql_records_once_before_mapping() {
        let (state, port, dir) = seeded().await;
        let db = state.db.clone();
        let health = db.health();
        let control = db
            .call(|s| {
                s.conn()
                    .query_row("SELECT v FROM s4_read_owner_missing_table", [], |row| {
                        row.get::<_, i64>(0)
                    })
            })
            .await;
        let before = health.snapshot();
        let failed = port
            .read(|conn| {
                conn.query_row("SELECT v FROM s4_read_owner_missing_table", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(Into::into)
            })
            .await;
        let observed = health.snapshot();
        let recovered = port
            .read(|conn| {
                conn.query_row("SELECT 42", [], |row| row.get::<_, i64>(0))
                    .map_err(Into::into)
            })
            .await;
        let later = health.snapshot();
        drop(port);
        drop(state);
        let closed = tokio::task::spawn_blocking(move || db.shutdown()).await;
        drop(dir);
        assert!(
            closed.is_ok(),
            "original owned DB shutdown task completed before semantics"
        );
        assert!(
            matches!(&control, Ok(Err(rusqlite::Error::SqliteFailure(f, _))) if f.extended_code == rusqlite::ffi::SQLITE_ERROR),
            "real raw SQLite missing-table control"
        );
        assert!(
            before.is_none(),
            "plain value-return Db call does not observe nested SQL result"
        );
        assert!(
            matches!(failed.as_ref().err().and_then(|e| e.downcast_ref::<collab::CollabError>()), Some(collab::CollabError::Store(crewd_store::StoreError::Sql(rusqlite::Error::SqliteFailure(f, _)))) if f.extended_code == rusqlite::ffi::SQLITE_ERROR),
            "known owning Collab SQL error preserved in original anyhow result"
        );
        assert_eq!(recovered.unwrap(), 42);
        assert_eq!(
            observed, later,
            "successful FIFO read retains history without another incident"
        );
        // First intended functional target; kind/stage are NOTREACHED if this fails.
        assert_eq!(
            observed.as_ref().map(|f| f.sequence),
            Some(1),
            "one actual owning read SQL failure observed before error mapping"
        );
        let fault = observed.unwrap();
        assert_eq!(fault.stage, crewd_store::db_health::DbFaultStage::Job);
        assert_eq!(fault.kind, crewd_store::db_health::DbFaultKind::Sql);
    }

    #[tokio::test]
    async fn s4_read_owner_orchestra_business_not_found_adds_no_fault() {
        let (state, port, dir) = seeded().await;
        let db = state.db.clone();
        let health = db.health();
        let before = health.snapshot();
        let refused = port
            .read(|conn| crewd_collab::crews::get(conn, "s4-read-owner-no-such-crew"))
            .await;
        let after = health.snapshot();
        let recovered = port
            .read(|conn| {
                conn.query_row("SELECT 42", [], |row| row.get::<_, i64>(0))
                    .map_err(Into::into)
            })
            .await;
        let later = health.snapshot();
        drop(port);
        drop(state);
        let closed = tokio::task::spawn_blocking(move || db.shutdown()).await;
        drop(dir);
        assert!(
            closed.is_ok(),
            "original owned DB shutdown task completed before semantics"
        );
        assert!(
            matches!(
                refused
                    .as_ref()
                    .err()
                    .and_then(|e| e.downcast_ref::<collab::CollabError>()),
                Some(collab::CollabError::NotFound(_))
            ),
            "known owning business NotFound preserved in original anyhow result"
        );
        assert_eq!(recovered.unwrap(), 42);
        assert!(
            before.is_none(),
            "fresh owned fixture starts without fault history"
        );
        assert_eq!(
            before, after,
            "business refusal does not record a database incident"
        );
        assert_eq!(
            after, later,
            "successful FIFO read does not add a database incident"
        );
    }
    use super::fixture::{post_as, seeded};
    use super::*;
    use crewd_wake::breaker;
    use crewd_wake::source;

    /// The real Hub bridge and spawned actor; only the executor/worktrees are synthetic.
    struct S46ActorDb {
        state: Arc<AppState>,
        port: Arc<DbPort>,
        handle: super::super::Handle,
        actor: super::super::Actor,
        bridge: tokio::task::JoinHandle<()>,
        exec: super::super::testkit::StandIn,
        now: Ms,
        clock: Arc<super::super::clock::ManualClock>,
        _dir: tempfile::TempDir,
    }

    impl S46ActorDb {
        async fn new(hooks: super::super::hooks::Hooks) -> Self {
            Self::new_with_work_idle(hooks, false).await
        }
        async fn new_with_work_idle(
            mut hooks: super::super::hooks::Hooks,
            work_idle: bool,
        ) -> Self {
            use super::super::{
                clock::ManualClock,
                exec::Executors,
                testkit::{StandIn, StandInWorktrees},
            };
            let (state, port, dir) = seeded().await;
            if work_idle {
                hooks.idle = Arc::new(super::super::work_idle::WorkIdle(state.db.clone()));
            }
            let rx = state.hub.subscribe_collab();
            let port = Arc::new(port);
            let exec = StandIn::default();
            let now = Ms(crewd_store::now_ms());
            let clock = Arc::new(ManualClock::new(now));
            hooks.worktrees = Arc::new(StandInWorktrees::default());
            let (handle, actor) = super::super::spawn(
                port.clone(),
                Executors::new().with("claude", Arc::new(exec.clone())),
                hooks,
                clock.clone(),
                super::super::OrchestraConfig::for_tests(),
            )
            .await
            .unwrap();
            let bridge = forward_collab(rx, handle.clone());
            Self {
                state,
                port,
                handle,
                actor,
                bridge,
                exec,
                now,
                clock,
                _dir: dir,
            }
        }

        async fn sent_containing(&self, marker: &str) -> bool {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    // FIFO actor command barrier; no fabricated collab fact or fixed sleep.
                    self.handle.latches().await.unwrap();
                    if self.exec.sent("w-1").iter().any(|s| s.contains(marker)) {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_ok()
        }

        /// DbPort uses transaction wall time, while the actor uses its injected clock.
        /// Wait for the actual persisted source, then tick at that row's real due time.
        async fn make_source_due(&self, source: String) -> Option<i64> {
            use super::super::{clock::Clock, core::Input};
            let at = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    self.handle.latches().await.unwrap();
                    let sought = source.clone();
                    let row = self
                        .port
                        .read(move |c| {
                            Ok(cw::list(c, Some("w-1"), 100)?
                                .into_iter()
                                .find(|w| w.source == sought))
                        })
                        .await
                        .unwrap();
                    if let Some(row) = row {
                        break row.not_before_ms;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .ok()?;
            self.clock.set(Ms(at.max(self.clock.now().0)));
            self.handle.tx.send(Input::Tick).unwrap();
            self.handle.latches().await.unwrap();
            Some(at)
        }

        async fn close(self) {
            let Self {
                state,
                port,
                handle,
                actor,
                bridge,
                exec: _,
                now: _,
                clock: _,
                _dir,
            } = self;
            let db = state.db.clone();
            assert!(handle.shutdown(std::time::Duration::from_secs(3)).await);
            tokio::time::timeout(std::time::Duration::from_secs(3), actor.finished())
                .await
                .unwrap()
                .unwrap();
            drop(handle);
            drop(port);
            // Dropping the final AppState closes Hub; the existing bridge exits naturally.
            drop(state);
            tokio::time::timeout(std::time::Duration::from_secs(3), bridge)
                .await
                .unwrap()
                .unwrap();
            tokio::task::spawn_blocking(move || db.shutdown())
                .await
                .unwrap();
            drop(_dir);
        }
    }

    #[tokio::test]
    async fn checkpoint_tick_reaches_actual_actor_and_success_receipt_counts_once() {
        use super::super::{clock::Clock, core::Input, hooks::Hooks};
        use crewd_exec::{
            delivery::Subject,
            session::{DeliveryOutcome, SessionEvent},
        };
        use crewd_proto::{WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};
        let rig = S46ActorDb::new(Hooks::default()).await;
        rig.handle
            .resume("w-1", super::super::core::Origin::Human)
            .await
            .unwrap();
        let ready = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if rig.handle.live("w-1").await.unwrap().running {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        let p = rig
            .state
            .collab(|tx| {
                let p = collab::work::create(
                    tx,
                    &collab::Sender::Human,
                    WorkCreateParams {
                        crew_id: "c-1".into(),
                        owner: Some("w-1".into()),
                        title: "synthetic actor checkpoint".into(),
                        ..Default::default()
                    },
                )?;
                collab::work::update(
                    tx,
                    &collab::Sender::Session("w-1".into()),
                    WorkUpdateParams {
                        id: p.id,
                        expected_revision: p.revision,
                        status: Some(WorkUpdateStatus::Active),
                        note: Some("next actual synthetic step".into()),
                        next_check_at: Some(crewd_store::ts_from_ms(tx.now_ms() + 1)),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap();
        let receipt = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                rig.clock
                    .set(Ms(crewd_store::now_ms().max(rig.clock.now().0)));
                rig.handle.tx.send(Input::Tick).unwrap();
                rig.handle.latches().await.unwrap();
                let row = rig
                    .port
                    .read(|c| {
                        Ok(cw::list(c, Some("w-1"), 100)?
                            .into_iter()
                            .find(|r| r.kind == cw::WakeKind::Checkpoint))
                    })
                    .await
                    .unwrap();
                if let Some(row) = row {
                    break row;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .ok();
        let sent = if let Some(row) = &receipt {
            rig.make_source_due(row.source.clone()).await;
            rig.sent_containing(&p.id).await
        } else {
            false
        };
        let work_number = p.id.strip_prefix('W').unwrap().parse::<i64>().unwrap();
        let missed_before = rig
            .port
            .read(move |c| {
                Ok(c.query_row("SELECT json_extract(i.value,'$.missed_checks') FROM organization_work_book b,json_each(b.state,'$.items') i JOIN object_refs r ON r.core_id=i.key AND r.kind='plan' WHERE r.number=?1",[work_number],|r|r.get::<_,u32>(0))?)
            })
            .await
            .unwrap();
        let id = if sent {
            rig.exec.last_send_id("w-1")
        } else {
            0
        };
        let counted = if sent {
            rig.exec.emit(
                "w-1",
                SessionEvent::Delivery {
                    id,
                    subject: Subject::Steer,
                    outcome: DeliveryOutcome::Delivered,
                },
            );
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    rig.handle.latches().await.unwrap();
                    let n = rig
                        .port
                        .read(move |c| {
                            Ok(c.query_row("SELECT json_extract(i.value,'$.missed_checks') FROM organization_work_book b,json_each(b.state,'$.items') i JOIN object_refs r ON r.core_id=i.key AND r.kind='plan' WHERE r.number=?1",[work_number],|r|r.get::<_,u32>(0))?)
                        })
                        .await
                        .unwrap();
                    if n == 1 {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_ok()
        } else {
            false
        };
        if counted {
            rig.exec.emit(
                "w-1",
                SessionEvent::Delivery {
                    id,
                    subject: Subject::Steer,
                    outcome: DeliveryOutcome::Delivered,
                },
            );
            rig.handle.latches().await.unwrap();
        }
        let final_misses = rig
            .port
            .read(move |c| {
                Ok(c.query_row("SELECT json_extract(i.value,'$.missed_checks') FROM organization_work_book b,json_each(b.state,'$.items') i JOIN object_refs r ON r.core_id=i.key AND r.kind='plan' WHERE r.number=?1",[work_number],|r|r.get::<_,u32>(0))?)
            })
            .await
            .unwrap();
        let sends = rig.exec.sent("w-1");
        rig.close().await;
        assert!(ready);
        assert!(
            receipt.is_some(),
            "actual actor tick did not persist a checkpoint"
        );
        assert!(sent, "checkpoint did not reach actual dispatch");
        assert_eq!(
            missed_before, 0,
            "queue and dispatch acceptance are not success receipts"
        );
        assert!(
            counted,
            "original successful delivery did not record the checkpoint"
        );
        assert_eq!(final_misses, 1);
        assert_eq!(
            sends.iter().filter(|s| s.contains(&p.id)).count(),
            1,
            "duplicate success must not schedule another model turn"
        );
    }

    #[tokio::test]
    async fn work_idle_real_actor_accepts_budget_and_delivers_compact_stop_reminder() {
        use super::super::{core::Origin, hooks::Hooks};
        use crewd_exec::session::SessionEvent;
        let rig = S46ActorDb::new_with_work_idle(Hooks::default(), true).await;
        let observation = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            rig.handle.resume("w-1", Origin::Human).await.unwrap();
            while !rig.handle.live("w-1").await.unwrap().running {
                tokio::task::yield_now().await;
            }
            let p = rig
                .state
                .collab(|tx| {
                    collab::work::create(
                        tx,
                        &collab::Sender::Human,
                        crewd_proto::WorkCreateParams {
                            crew_id: "c-1".into(),
                            owner: Some("w-1".into()),
                            title: "synthetic real idle edge".into(),
                            ..Default::default()
                        },
                    )
                })
                .await
                .unwrap();
            rig.exec.emit("w-1", SessionEvent::Working(true));
            while !rig.handle.live("w-1").await.unwrap().working {
                tokio::task::yield_now().await;
            }
            rig.exec.emit("w-1", SessionEvent::Working(false));
            let row = loop {
                rig.handle.latches().await.unwrap();
                if let Some(r) = rig
                    .port
                    .read(|c| {
                        Ok(cw::list(c, Some("w-1"), 100)?
                            .into_iter()
                            .find(|r| r.kind == cw::WakeKind::StopCheck))
                    })
                    .await
                    .unwrap()
                {
                    break r;
                }
                tokio::task::yield_now().await;
            };
            rig.make_source_due(row.source.clone()).await;
            let sent = rig.sent_containing(&p.id).await;
            let budget = rig
                .port
                .read(|c| Ok(crewd_store::work_stop_checks::get(c, "w-1")?))
                .await
                .unwrap()
                .unwrap();
            let sends = rig.exec.sent("w-1");
            (p, row, sent, budget, sends)
        })
        .await;
        rig.close().await;
        let (p, row, sent, budget, sends) = observation.expect("actual idle actor chain timed out");
        assert!(sent, "accepted stop reminder did not reach dispatch");
        assert_eq!(budget.epoch, 1);
        assert_eq!(budget.streak.rewakes, 1);
        assert!(row.source.starts_with("work-stop:"));
        assert!(
            !row.source
                .starts_with(super::super::work_idle::CANDIDATE_SOURCE)
        );
        let sent: Vec<_> = sends.iter().filter(|s| s.contains(&p.id)).collect();
        assert_eq!(sent.len(), 1);
        assert!(!sent[0].contains("stopTargets"));
        assert!(!sent[0].contains("expected_epoch"));
    }

    // Test-first: current wire/Port shape only. No proposed cursorUnknown field is needed.
    // This uses the actual DB facade, spawned actor and public orchestration RPC dispatcher.
    type S42ReadonlyTables = Vec<(String, Vec<Vec<rusqlite::types::Value>>)>;

    fn s42_readonly_tables(c: &rusqlite::Connection) -> rusqlite::Result<S42ReadonlyTables> {
        let mut names = c.prepare("SELECT name FROM sqlite_master WHERE type='table' AND (name NOT LIKE 'sqlite_%' OR name='sqlite_sequence') ORDER BY name")?;
        let names = names
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        names
            .into_iter()
            .map(|name| {
                let quoted = name.replace('"', "\"\"");
                let mut query = c.prepare(&format!("SELECT * FROM \"{quoted}\""))?;
                let width = query.column_count();
                let mut rows = query
                    .query_map([], |r| {
                        (0..width)
                            .map(|i| r.get(i))
                            .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows.sort_by_key(|r| format!("{r:?}"));
                Ok((name, rows))
            })
            .collect()
    }

    struct S42DiagnosticCatch<F: std::future::Future>(std::pin::Pin<Box<F>>);
    impl<F: std::future::Future> std::future::Future for S42DiagnosticCatch<F> {
        type Output = std::result::Result<F::Output, Box<dyn std::any::Any + Send>>;
        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.0.as_mut().poll(cx)
            })) {
                Ok(std::task::Poll::Ready(v)) => std::task::Poll::Ready(Ok(v)),
                Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
                Err(e) => std::task::Poll::Ready(Err(e)),
            }
        }
    }

    #[tokio::test]
    async fn s42_db_readonly_missing_cursor_survives_port_and_wake_list_without_writes() {
        let rig = S46ActorDb::new(super::super::hooks::Hooks::default()).await;
        // Boot/actor creation already completed. There is no pending work or live child,
        // so ticks cannot legitimately prepare this channel while inspecting it.
        let collected = S42DiagnosticCatch(Box::pin(async {
            let deleted =
                rig.state
                    .db
                    .call(|s| {
                        s.conn().execute(
                "DELETE FROM read_cursors WHERE reader='w-1' AND channel_id='c-1/lobby'", []
            )
                    })
                    .await??;
            rig.handle.latches().await?; // real FIFO actor barrier, not a synthetic collab fact
            let mut hub = rig.state.hub.subscribe_collab();
            let before = rig
                .state
                .db
                .call(|s| s42_readonly_tables(s.conn()))
                .await??;
            let raw = rig
                .state
                .db
                .call(|s| collab::causal::diagnostics(s.conn(), "w-1", crewd_store::now_ms()))
                .await??;
            let mut ports = Vec::new();
            let mut wires = Vec::new();
            for _ in 0..2 {
                ports.push(rig.port.causal_diagnostics("w-1", rig.now).await?);
                wires.push(
                    super::super::rpc::call(
                        &rig.handle,
                        &rig.state,
                        crewd_proto::orchestra::methods::WAKE_LIST,
                        serde_json::json!({"sessionId":"w-1"}),
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("actual wake.list RPC error: {e:?}"))?,
                );
            }
            rig.handle.latches().await?;
            let after = rig
                .state
                .db
                .call(|s| s42_readonly_tables(s.conn()))
                .await??;
            let missing = rig
                .state
                .db
                .call(|s| collab::cursors::cursor(s.conn(), "w-1", "c-1/lobby"))
                .await??;
            let mut events = Vec::new();
            loop {
                match hub.try_recv() {
                    Ok(event) => events.push(format!("{event:?}")),
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                    Err(e) => anyhow::bail!("readonly Hub observer failed: {e}"),
                }
            }
            Ok::<_, anyhow::Error>((deleted, raw, ports, wires, before, after, missing, events))
        }))
        .await;
        // This handles setup errors and unexpected panics before the normal cleanup.
        // Functional oracles run only after normal actor/bridge/DB joins.
        let cleanup = S42DiagnosticCatch(Box::pin(rig.close())).await;
        let result = match (collected, cleanup) {
            (Ok(result), Ok(())) => result,
            (Err(panic), Ok(())) => std::panic::resume_unwind(panic),
            (original, Err(_)) => panic!(
                "S42 readonly normal cleanup failed; original stage failed={}",
                original.as_ref().map_or(true, |v| v.is_err())
            ),
        };
        let (deleted, raw, ports, wires, before, after, missing, events) = result.expect(
            "readonly fixture/dispatcher stage failed after normal cleanup; not causal RED",
        );
        assert!(deleted <= 1);
        assert!(
            raw.unknown_channels.iter().any(|ch| ch == "c-1/lobby"),
            "fixture must expose actual missing eligible cursor"
        );
        assert_eq!(missing, None);
        assert_eq!(
            before, after,
            "readonly Port/actor/RPC must not pin, hold, alter receipts or any durable table"
        );
        assert!(
            events.is_empty(),
            "readonly diagnosis must not publish Hub effects"
        );
        for wire in &wires {
            assert!(
                wire["wakes"]
                    .as_array()
                    .expect("actual wake.list shape")
                    .is_empty(),
                "no fabricated wake row for unknown cursor"
            );
        }
        eprintln!(
            "S42 missing-cursor observed normal_cleanup=true raw_unknown={:?} port_channels={:?} rpc_targets={:?} hub_events={}",
            raw.unknown_channels,
            ports
                .iter()
                .map(|v| v.iter().map(|(ch, _)| ch).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            wires.iter().map(|v| &v["targets"]).collect::<Vec<_>>(),
            events.len()
        );
        // Expected first old-source behavioral RED: raw storage is honest, adapter loses it.
        for diagnostic in &ports {
            assert!(
                diagnostic.iter().any(|(ch, _)| ch == "c-1/lobby"),
                "S42 missing cursor must survive actual Port diagnostic conversion"
            );
        }
        for wire in &wires {
            let target = wire["targets"]
                .as_array()
                .expect("actual wake.list targets")
                .iter()
                .find(|v| v["sessionId"] == "w-1")
                .expect("explicit actual target");
            assert!(
                target["causalDiagnostics"]
                    .as_array()
                    .is_some_and(|channels| channels.iter().any(|v| v["channelId"] == "c-1/lobby")),
                "S42 missing cursor must survive actual actor and RPC target diagnostics"
            );
        }
    }

    #[tokio::test]
    async fn s42_missing_cursor_actual_prepare_then_claim_has_no_diagnostic_latch() {
        use crewd_exec::{
            delivery::Subject,
            session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
        };
        let rig = S46ActorDb::new(super::super::hooks::Hooks::default()).await;
        let collected = S42DiagnosticCatch(Box::pin(async {
            rig.state
                .db
                .call(|s| {
                    s.conn().execute(
                        "DELETE FROM read_cursors WHERE reader='w-1' AND channel_id='c-1/lobby'",
                        [],
                    )
                })
                .await??;
            rig.handle.latches().await?;
            let before = rig.port.causal_diagnostics("w-1", rig.now).await?;
            let cursor_before = rig
                .state
                .db
                .call(|s| collab::cursors::cursor(s.conn(), "w-1", "c-1/lobby"))
                .await??;
            // Real ordinary preparation, not a diagnostic side effect or fixture pin.
            rig.port.prepare_causal_admission("w-1", rig.now).await?;
            let prepared_cursor = rig
                .state
                .db
                .call(|s| collab::cursors::cursor(s.conn(), "w-1", "c-1/lobby"))
                .await??;
            let after_prepare = rig.port.causal_diagnostics("w-1", rig.now).await?;
            let message = rig
                .state
                .collab(|tx| {
                    collab::messages::post(
                        tx,
                        collab::Sender::Human,
                        collab::messages::NewPost {
                            channel_id: "c-1/lobby".into(),
                            text: "S42_CURSOR_PREP_REAL_BODY".into(),
                            mentions: vec![crewd_proto::Mention {
                                kind: crewd_proto::MentionKind::Session,
                                target_id: Some("w-1".into()),
                            }],
                            ..Default::default()
                        },
                    )
                    .map(|p| p.message)
                })
                .await?;
            let due = rig.make_source_due(source::message(&message.id)).await;
            // Actual Hub->actor->planner->render/claim->controlled executor launch.
            let body_seen = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    rig.handle.latches().await?;
                    if rig.exec.launches("w-1").iter().any(|l| {
                        l.initial
                            .as_deref()
                            .is_some_and(|t| t.contains(&message.text))
                    }) {
                        break Ok::<_, anyhow::Error>(());
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await;
            if matches!(&body_seen, Ok(Ok(()))) {
                rig.exec.emit(
                    "w-1",
                    SessionEvent::Delivery {
                        id: BRIEF_DELIVERY_ID,
                        subject: Subject::Brief,
                        outcome: DeliveryOutcome::Delivered,
                    },
                );
            }
            let expected_source = source::message(&message.id);
            let settled = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    rig.handle.latches().await?;
                    let expected_source = expected_source.clone();
                    let row = rig
                        .port
                        .read(move |c| {
                            Ok(cw::list(c, Some("w-1"), 100)?
                                .into_iter()
                                .find(|w| w.source == expected_source))
                        })
                        .await?;
                    if row
                        .as_ref()
                        .is_some_and(|w| w.state == cw::WakeState::Delivered)
                    {
                        break Ok::<_, anyhow::Error>(row);
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await;
            let final_cursor = rig
                .state
                .db
                .call(|s| collab::cursors::cursor(s.conn(), "w-1", "c-1/lobby"))
                .await??;
            let final_diag = rig.port.causal_diagnostics("w-1", rig.now).await?;
            let holds = rig.port.causal_holds(Some("w-1")).await?;
            let wire = super::super::rpc::call(
                &rig.handle,
                &rig.state,
                crewd_proto::orchestra::methods::WAKE_LIST,
                serde_json::json!({"sessionId":"w-1"}),
            )
            .await
            .map_err(|e| anyhow::anyhow!("actual RPC: {e:?}"))?;
            Ok::<_, anyhow::Error>((
                before,
                cursor_before,
                prepared_cursor,
                after_prepare,
                message,
                due,
                body_seen,
                settled,
                final_cursor,
                final_diag,
                holds,
                wire,
            ))
        }))
        .await;
        let cleanup = S42DiagnosticCatch(Box::pin(rig.close())).await;
        let result = match (collected, cleanup) {
            (Ok(result), Ok(())) => result,
            (Err(p), Ok(())) => std::panic::resume_unwind(p),
            (original, Err(_)) => panic!(
                "S42 preparation cleanup failed; original failed={}",
                original.as_ref().map_or(true, |v| v.is_err())
            ),
        };
        let (
            before,
            cursor_before,
            prepared_cursor,
            after_prepare,
            message,
            due,
            body_seen,
            settled,
            final_cursor,
            final_diag,
            holds,
            wire,
        ) = result.expect("preparation fixture failed after cleanup, not behavior proof");
        assert_eq!(cursor_before, None);
        assert!(
            before
                .iter()
                .any(|(ch, b)| ch == "c-1/lobby" && b.cursor_unknown),
            "S42 original eligible surface is honestly unknown"
        );
        assert!(
            prepared_cursor.is_some(),
            "S42 actual preparation establishes a safe real cursor"
        );
        assert!(
            !after_prepare
                .iter()
                .any(|(ch, b)| ch == "c-1/lobby" && b.cursor_unknown),
            "S42 preparation removes lookup uncertainty"
        );
        assert!(
            due.is_some(),
            "actual committed Hub message creates real planner row"
        );
        assert!(
            matches!(body_seen, Ok(Ok(()))),
            "S42 prepared surface actual rendered body must launch"
        );
        let row = settled
            .expect("actual Delivered row missing after normal cleanup")
            .expect("actual receipt storage failure")
            .unwrap();
        assert_eq!(row.source, source::message(&message.id));
        assert_eq!(row.state, cw::WakeState::Delivered);
        assert_eq!(final_cursor, Some(message.seq));
        assert!(
            holds.is_empty(),
            "S42 cursor uncertainty is not a durable causal latch"
        );
        assert!(
            !final_diag
                .iter()
                .any(|(ch, b)| ch == "c-1/lobby" && b.cursor_unknown)
        );
        let target = wire["targets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["sessionId"] == "w-1")
            .unwrap();
        assert!(
            !target["causalDiagnostics"]
                .as_array()
                .is_some_and(|v| v
                    .iter()
                    .any(|ch| ch["channelId"] == "c-1/lobby"
                        && ch["blockage"]["cursorUnknown"] == true)),
            "actual RPC no longer reports unknown prepared channel"
        );
    }

    type S46AskCall = (String, String, Vec<(String, Option<String>)>, Ms);

    #[derive(Default)]
    struct S46Ask {
        calls: std::sync::Mutex<Vec<S46AskCall>>,
        receipt: tokio::sync::Notify,
    }

    impl super::super::hooks::AskAnsweredHook for S46Ask {
        fn on_answered<'a>(
            &'a self,
            crew: &'a str,
            ask: &'a str,
            cleared: &'a [(String, Option<String>)],
            now: Ms,
        ) -> super::super::exec::BoxFut<'a, ()> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push((crew.into(), ask.into(), cleared.to_vec(), now));
                self.receipt.notify_one();
            })
        }
    }

    #[tokio::test]
    async fn s46_real_ask_answer_hub_polls_hook_and_delivers_resume_note() {
        use super::super::{clock::Clock, core::Origin, hooks::Hooks};
        let hook = Arc::new(S46Ask::default());
        let rig = S46ActorDb::new(Hooks {
            ask: hook.clone(),
            ..Hooks::default()
        })
        .await;
        rig.handle.resume("w-1", Origin::Human).await.unwrap();
        let live_before_ask = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if rig.handle.live("w-1").await.unwrap().running {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        let ask = rig
            .state
            .collab(|tx| {
                collab::asks::open(
                    tx,
                    "w-1",
                    collab::asks::OpenAsk {
                        question: "S46 actual standalone question".into(),
                        resume_note: Some("S46_UNIQUE_RESUME_NOTE".into()),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap();
        let ask_id = ask.ask.clone();
        let answer_id = ask_id.clone();
        let answered = rig
            .state
            .collab(move |tx| collab::asks::answer(tx, "c-1", &answer_id, "S46 confirmed answer"))
            .await
            .unwrap();
        // Missing-hook mutation still reaches normal cleanup and the exact counter assertion.
        let receipt =
            tokio::time::timeout(std::time::Duration::from_secs(3), hook.receipt.notified())
                .await
                .is_ok();
        // Keep clock fixed until the hook's exact-now witness was captured (or diagnosed
        // absent); only then move it to the actual answer transaction's wake timestamp.
        let answer_due = rig.make_source_due(cw::source::ask("c-1", &ask.ask)).await;
        let answer_clock_after = rig.clock.now();
        let delivered = rig.sent_containing("S46_UNIQUE_RESUME_NOTE").await;
        let ordinary = "S46_ORDINARY_POST_NEGATIVE";
        let ordinary_message = post_as(
            &rig.state,
            collab::Sender::Human,
            "c-1/lobby",
            ordinary,
            &["w-1"],
        )
        .await;
        // Finish the first delivery so the ordinary post is not hidden behind its fence.
        let id = rig
            .exec
            .lock()
            .sends
            .get("w-1")
            .and_then(|s| s.last())
            .map(|(id, _)| *id);
        if let Some(id) = id {
            rig.exec.emit(
                "w-1",
                crewd_exec::session::SessionEvent::Delivery {
                    id,
                    subject: crewd_exec::delivery::Subject::Steer,
                    outcome: crewd_exec::session::DeliveryOutcome::Delivered,
                },
            );
        }
        let ordinary_due = rig
            .make_source_due(cw::source::message(&ordinary_message.id))
            .await;
        let ordinary_clock_after = rig.clock.now();
        let ordinary_delivered = rig.sent_containing(ordinary).await;
        let now = rig.now;
        let calls = hook.calls.lock().unwrap().clone();
        let sent = rig.exec.sent("w-1");
        let durable = rig
            .port
            .read(move |c| {
                Ok((
                    collab::asks::get(c, "c-1", &ask_id)?,
                    cw::list(c, Some("w-1"), 100)?,
                ))
            })
            .await
            .unwrap();
        rig.close().await;
        eprintln!(
            "S46 Ask due alignment: initial={now:?} answer_not_before={answer_due:?} answer_clock={answer_clock_after:?} ordinary_not_before={ordinary_due:?} ordinary_clock={ordinary_clock_after:?}"
        );
        assert_eq!(calls.len(), 1, "S46 AskAnsweredHook polled call");
        assert!(
            receipt,
            "missing receipt is diagnostic; counter is the primary oracle"
        );
        assert_eq!(calls, [("c-1".into(), ask.ask.clone(), vec![], now)]);
        assert!(
            live_before_ask,
            "ask starts only after public actor reports an actual live handle"
        );
        assert_eq!(answered.state, crewd_proto::AskStateView::Answered);
        assert_eq!(durable.0.answer.as_deref(), Some("S46 confirmed answer"));
        assert_eq!(durable.0.state, crewd_proto::AskStateView::Answered);
        assert_eq!(
            durable.0.resume_note.as_deref(),
            Some("S46_UNIQUE_RESUME_NOTE")
        );
        let answer_wakes: Vec<_> = durable
            .1
            .iter()
            .filter(|w| w.kind == cw::WakeKind::AskAnswer)
            .collect();
        assert_eq!(answer_wakes.len(), 1);
        assert_eq!(answer_wakes[0].source, cw::source::ask("c-1", &ask.ask));
        assert_eq!(answer_wakes[0].priority, WakePriority::Human);
        assert_eq!(answer_wakes[0].state.as_str(), "delivered");
        assert!(answer_due.is_some() && ordinary_due.is_some());
        assert_eq!(answer_clock_after, Ms(answer_due.unwrap().max(now.0)));
        assert_eq!(
            ordinary_clock_after,
            Ms(ordinary_due.unwrap().max(answer_clock_after.0))
        );
        assert!(delivered && ordinary_delivered);
        assert_eq!(
            sent.iter()
                .filter(|s| s.contains("S46_UNIQUE_RESUME_NOTE"))
                .count(),
            1
        );
        assert_eq!(sent.iter().filter(|s| s.contains(ordinary)).count(), 1);
    }

    #[derive(Default)]
    struct S46DbQuota(std::sync::Mutex<Vec<(String, Ms)>>);
    impl super::super::hooks::QuotaSource for S46DbQuota {
        fn reset_at<'a>(
            &'a self,
            executor: &'a str,
            now: Ms,
        ) -> super::super::exec::BoxFut<'a, Option<Ms>> {
            Box::pin(async move {
                self.0.lock().unwrap().push((executor.into(), now));
                Some(now.add(17 * crewd_wake::MIN))
            })
        }
    }

    #[tokio::test]
    async fn s46_real_usage_limit_persists_one_quota_wake() {
        use super::super::{core::Origin, hooks::Hooks};
        let quota = Arc::new(S46DbQuota::default());
        let rig = S46ActorDb::new(Hooks {
            quota: quota.clone(),
            ..Hooks::default()
        })
        .await;
        rig.handle.resume("w-1", Origin::Human).await.unwrap();
        for _ in 0..2 {
            rig.exec.emit(
                "w-1",
                crewd_exec::session::SessionEvent::Health(Some(crewd_exec::health::Health::new(
                    crewd_exec::health::HealthKind::UsageLimit,
                    "S46 durable",
                ))),
            );
        }
        // Observe the durable pending episode before shutdown legitimately requeues work.
        rig.handle.latches().await.unwrap();
        let reached = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                rig.handle.latches().await.unwrap();
                if !rig.port.pending("w-1").await.unwrap().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        // StandIn event forwarding is FIFO; let the actor consume both before capturing the row.
        rig.exec
            .emit("w-1", crewd_exec::session::SessionEvent::Working(true));
        let working = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if rig.handle.live("w-1").await.unwrap().working {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        let wakes = rig
            .port
            .read(|c| cw::list(c, Some("w-1"), 100))
            .await
            .unwrap();
        let decoded = rig.port.pending("w-1").await.unwrap();
        let calls = quota.0.lock().unwrap().clone();
        let now = rig.now;
        rig.close().await;
        assert_eq!(
            calls,
            [("claude".into(), now)],
            "S46 QuotaSource polled call"
        );
        assert!(reached && working);
        assert_eq!(wakes.len(), 1);
        let w = &wakes[0];
        assert_eq!(w.session, "w-1");
        assert_eq!(w.kind, cw::WakeKind::System);
        assert_eq!(w.priority, WakePriority::System);
        assert_eq!(w.state.as_str(), "pending");
        assert_eq!(w.not_before_ms, now.add(18 * crewd_wake::MIN).0);
        assert_eq!(w.source, format!("quota:{}", w.not_before_ms));
        assert_eq!(w.origin, None, "C21 stores system origin as SQL NULL");
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].id, w.id);
        assert_eq!(
            decoded[0].origin, SYSTEM,
            "actual Port decode preserves system identity"
        );
        assert_eq!(
            w.note.as_deref(),
            Some(crewd_wake::text::get(Lang::Zh, "quota.wake"))
        );
    }

    struct T11PendingQuota {
        entered: Arc<tokio::sync::Semaphore>,
        fallback: crate::control::NoSessions,
    }

    impl crate::control::SessionControl for T11PendingQuota {
        fn start(
            &self,
            req: crate::control::StartRequest,
        ) -> crate::control::BoxFuture<
            '_,
            std::result::Result<crewd_proto::Session, crate::control::ControlError>,
        > {
            crate::control::SessionControl::start(&self.fallback, req)
        }
        fn resume(
            &self,
            s: String,
        ) -> crate::control::BoxFuture<'_, std::result::Result<(), crate::control::ControlError>>
        {
            crate::control::SessionControl::resume(&self.fallback, s)
        }
        fn stop(
            &self,
            s: String,
            reason: String,
        ) -> crate::control::BoxFuture<'_, std::result::Result<(), crate::control::ControlError>>
        {
            crate::control::SessionControl::stop(&self.fallback, s, reason)
        }
        fn nudge(
            &self,
            s: String,
            text: String,
        ) -> crate::control::BoxFuture<'_, std::result::Result<(), crate::control::ControlError>>
        {
            crate::control::SessionControl::nudge(&self.fallback, s, text)
        }
        fn inspect(
            &self,
            s: String,
            n: usize,
        ) -> crate::control::BoxFuture<
            '_,
            std::result::Result<crate::control::Inspection, crate::control::ControlError>,
        > {
            crate::control::SessionControl::inspect(&self.fallback, s, n)
        }
        fn set_profile(
            &self,
            s: String,
            model: Option<String>,
            effort: Option<String>,
        ) -> crate::control::BoxFuture<'_, std::result::Result<(), crate::control::ControlError>>
        {
            crate::control::SessionControl::set_profile(&self.fallback, s, model, effort)
        }
        fn handoff_captain(
            &self,
            c: String,
            s: String,
        ) -> crate::control::BoxFuture<'_, std::result::Result<(), crate::control::ControlError>>
        {
            crate::control::SessionControl::handoff_captain(&self.fallback, c, s)
        }
        fn quota(
            &self,
        ) -> crate::control::BoxFuture<
            '_,
            std::result::Result<crate::control::QuotaReport, crate::control::ControlError>,
        > {
            Box::pin(async move {
                self.entered.add_permits(1);
                std::future::pending().await
            })
        }
        fn live(
            &self,
            s: String,
        ) -> crate::control::BoxFuture<
            '_,
            std::result::Result<crewd_proto::SessionLive, crate::control::ControlError>,
        > {
            crate::control::SessionControl::live(&self.fallback, s)
        }
    }

    #[tokio::test]
    async fn t11_launch_quota_deadline_releases_db_and_preserves_identity() {
        use collab::worldview::QuotaTier;
        use std::time::Duration;
        let (base, _old_port, _dir) = seeded().await;
        let entered = Arc::new(tokio::sync::Semaphore::new(0));
        let state = Arc::new(AppState::new(base.db.clone()).with_sessions(Arc::new(
            T11PendingQuota {
                entered: entered.clone(),
                fallback: crate::control::NoSessions,
            },
        )));
        let port = DbPort::new(state.clone(), PathBuf::from("/synthetic/not-opened"));
        let captured_locale = state
            .db
            .call(|s| collab::settings::locale(s.conn()))
            .await
            .unwrap()
            .unwrap();
        let reader_state = state.clone();
        let callers = tokio::spawn(async move {
            // Two independent real callers, both bound by the public 1s quota deadline.
            tokio::time::timeout(Duration::from_millis(1500), async move {
                tokio::join!(
                    port.worldview("w-1"),
                    crate::agent::port::world_context(&reader_state, "w-1")
                )
            })
            .await
            .expect("pending quota must not stall actual launch caller past 1.5s")
        });
        let permits = tokio::time::timeout(Duration::from_millis(400), entered.acquire_many(2))
            .await
            .expect("both actual callers must reach quota source")
            .unwrap();
        permits.forget();
        // The sources are now pending, yet the same DB can commit another collab
        // transaction and read it back. This catches quota awaits held under Db::call.
        tokio::time::timeout(Duration::from_millis(300), async {
            state
                .collab(|tx| collab::settings::set_locale(tx, collab::Locale::En))
                .await
                .unwrap();
            let loc = state
                .db
                .call(|s| collab::settings::locale(s.conn()))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(loc, collab::Locale::En);
        })
        .await
        .expect("quota wait must release the DB actor before awaiting source");
        assert!(
            !callers.is_finished(),
            "quota source is still pending before its deadline"
        );
        let (launch, ctx) = callers.await.unwrap();
        let ctx = ctx.unwrap();
        assert_eq!(ctx.quota_claude, QuotaTier::Failed);
        assert_eq!(ctx.quota_codex, QuotaTier::Failed);
        assert_eq!(ctx.session_id, "w-1");
        assert_eq!(ctx.crew_id, "c-1");
        assert_eq!(ctx.role, crewd_proto::Role::Worker);
        assert_eq!(ctx.executor, "claude");
        assert_eq!(
            ctx.locale, captured_locale,
            "identity context was captured before the quota wait"
        );
        assert_eq!(launch.unwrap(), collab::worldview::render(&ctx));
        tokio::task::spawn_blocking(move || state.db.clone().shutdown())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn t11_ledger_write_serializes_with_collab_read_then_write() {
        let (state, port, _dir) = seeded().await;
        let ledger = port.ledger("w-1");
        let writer = Arc::new(std::sync::Mutex::new(None));
        let writer_slot = writer.clone();
        let result = state
            .collab(move |tx| {
                // Pin the WAL snapshot before the concurrently launched driver's ledger write.
                crewd_store::sessions::agent_session_id(tx.conn(), "w-1")?;
                let (done, wait) = std::sync::mpsc::channel();
                *writer_slot.lock().unwrap() = Some(std::thread::spawn(move || {
                    let r = ledger.record("synthetic-uuid");
                    let _ = done.send(());
                    r
                }));
                // Old StoreLedger finishes on another connection here. Shared Db queues it
                // until this transaction commits, so this bounded wait then times out.
                let _ = wait.recv_timeout(std::time::Duration::from_millis(100));
                tx.conn().execute(
                    "UPDATE sessions SET title='synthetic title' WHERE id='w-1'",
                    [],
                )?;
                Ok(())
            })
            .await;
        let write = writer.lock().unwrap().take().unwrap().join().unwrap();
        assert!(write.is_ok(), "ledger write: {write:?}");
        assert!(result.is_ok(), "collab read-to-write upgrade: {result:?}");
        let ledger = port.ledger("w-1");
        assert_eq!(
            tokio::task::spawn_blocking(move || ledger.recorded())
                .await
                .unwrap()
                .unwrap()
                .as_deref(),
            Some("synthetic-uuid")
        );
        tokio::task::spawn_blocking(move || state.db.clone().shutdown())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn t11_launch_world_uses_the_shared_executor_tier_source() {
        use crate::control::{QuotaLine, QuotaReport};
        let f = crate::agent::testkit::Fixture::new().await;
        f.set_quota(QuotaReport {
            lines: vec![
                QuotaLine {
                    executor: "codex".into(),
                    plan: Some("Synthetic Codex".into()),
                    ..Default::default()
                },
                QuotaLine {
                    executor: "claude".into(),
                    plan: Some("Synthetic Claude".into()),
                    ..Default::default()
                },
            ],
        });
        let p = DbPort::new(f.state.clone(), PathBuf::from("/synthetic/not-opened"));
        let ctx = crate::agent::port::world_context(&f.state, &f.worker)
            .await
            .unwrap();
        let launch = p.worldview(&f.worker).await.unwrap();
        assert_eq!(launch, collab::worldview::render(&ctx));
        assert!(launch.contains("Synthetic Claude") && launch.contains("Synthetic Codex"));
        assert_eq!(
            crate::agent::port::live_world(&f.state, &f.worker)
                .await
                .unwrap(),
            collab::worldview::render_live(&ctx)
        );
    }

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
            ..Default::default()
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

    // Actual DbPort/SQLite + Core; only the executor is controlled. These never start an agent CLI.
    struct StopDbRig {
        core: super::super::core::Core<DbPort>,
        state: Arc<AppState>,
        port: Arc<DbPort>,
        exec: super::super::testkit::StandIn,
        rx: tokio::sync::mpsc::UnboundedReceiver<super::super::core::Input>,
        dir: tempfile::TempDir,
    }

    impl StopDbRig {
        async fn new() -> Self {
            let (state, port, dir) = seeded().await;
            Self::on_db(state, Arc::new(port), dir).await
        }

        async fn on_db(state: Arc<AppState>, port: Arc<DbPort>, dir: tempfile::TempDir) -> Self {
            use super::super::{clock::ManualClock, core::Core, exec::Executors, hooks::Hooks};
            let exec = super::super::testkit::StandIn::default();
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let core = Core::new(
                port.clone(),
                Executors::new().with("claude", Arc::new(exec.clone())),
                Hooks {
                    worktrees: Arc::new(super::super::testkit::StandInWorktrees::default()),
                    ..Default::default()
                },
                Arc::new(ManualClock::new(Ms(crewd_store::now_ms()))),
                super::super::OrchestraConfig::for_tests(),
                tx,
            )
            .await
            .unwrap();
            Self {
                core,
                state,
                port,
                exec,
                rx,
                dir,
            }
        }

        async fn drain(&mut self) {
            while let Ok(Some(i)) =
                tokio::time::timeout(std::time::Duration::from_millis(25), self.rx.recv()).await
            {
                self.core.handle(i).await;
            }
        }

        async fn wake(&mut self, tag: &str) -> i64 {
            use super::super::core::{CollabFact, Input};
            let id = self
                .port
                .enqueue(new_wake(
                    "w-1",
                    WakeKind::System,
                    source::system(tag),
                    Priority::System,
                    SYSTEM,
                ))
                .await
                .unwrap()
                .unwrap();
            self.core
                .handle(Input::Collab(CollabFact::WakeEnqueued {
                    session: "w-1".into(),
                }))
                .await;
            self.drain().await;
            assert!(
                self.core.live.contains_key("w-1"),
                "actual Core launches the controlled process"
            );
            id
        }

        async fn stop(&mut self) -> Result<(), super::super::core::ControlError> {
            use super::super::core::{Cmd, Input};
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.core
                .handle(Input::Cmd(Cmd::Stop {
                    session: "w-1".into(),
                    reply: tx,
                }))
                .await;
            let r = rx.await.unwrap();
            self.drain().await;
            r
        }

        async fn enqueue_only(&self, tag: &str) -> i64 {
            self.port
                .enqueue(new_wake(
                    "w-1",
                    WakeKind::System,
                    source::system(tag),
                    Priority::System,
                    SYSTEM,
                ))
                .await
                .unwrap()
                .unwrap()
        }

        async fn floor(&self) -> i64 {
            self.state
                .db
                .call(|s| cw::human_stop_floor(s.conn(), "w-1").unwrap())
                .await
                .unwrap()
        }

        async fn batch(&self, ids: &[i64]) -> Batch {
            let ids = ids.to_vec();
            let wakes = self
                .state
                .db
                .call(move |s| {
                    cw::by_ids(s.conn(), &ids)
                        .unwrap()
                        .iter()
                        .map(|w| queued(s.conn(), w).unwrap())
                        .collect()
                })
                .await
                .unwrap();
            Batch {
                session: "w-1".into(),
                wakes,
                ..Default::default()
            }
        }

        async fn row(&self, id: i64) -> cw::WakeRow {
            self.state
                .db
                .call(move |s| cw::by_ids(s.conn(), &[id]).unwrap().remove(0))
                .await
                .unwrap()
        }

        // Known settlement error, not a fake successful cancellation. Dropping the trigger permits boot recovery.
        async fn block_drop(&self, id: i64) {
            self.state
                .db
                .call(move |s| {
                    s.conn()
                        .execute_batch(&format!(
                            "CREATE TRIGGER stop_settle_fault BEFORE UPDATE OF state ON wakes
                 WHEN OLD.id={id} AND NEW.state='dropped'
                 BEGIN SELECT RAISE(ABORT, 'synthetic stop settlement fault'); END;"
                        ))
                        .unwrap()
                })
                .await
                .unwrap();
        }

        async fn unblock_drop(&self) {
            self.state
                .db
                .call(|s| {
                    s.conn()
                        .execute_batch("DROP TRIGGER stop_settle_fault")
                        .unwrap()
                })
                .await
                .unwrap();
        }

        async fn reboot(self) -> Self {
            let Self {
                core,
                state,
                port,
                exec,
                rx,
                dir,
            } = self;
            drop(core); // Crash: no halt/hibernate/shutdown settlement.
            exec.lock().running.clear();
            exec.lock().events.clear();
            drop(rx);
            drop(port);
            let db = state.db.clone();
            drop(state);
            tokio::task::spawn_blocking(move || db.shutdown())
                .await
                .unwrap();
            let path = dir.path().join("crewd.db");
            let state = Arc::new(AppState::new(
                crewd_store::Db::spawn(crewd_store::Store::open(&path).unwrap()).unwrap(),
            ));
            let port = Arc::new(DbPort::new(state.clone(), path));
            let mut r = Self::on_db(state, port, dir).await;
            r.core.boot().await.unwrap(); // Actual ordinary boot route, freshly opened SQLite.
            r.drain().await;
            r
        }
    }

    // White-box Core/inbox + real DbPort proof, not a public helper E2E:
    // real rendering supplies a cursor-only body; its synthetic unowned trigger is
    // disposed before admission, so the claimed batch has genuinely zero wake IDs.
    impl StopDbRig {
        async fn cursor_body(&self) -> (Batch, String, i64) {
            let m = post_as(
                &self.state,
                collab::Sender::Human,
                "c-1/lobby",
                "合成零行积压",
                &[],
            )
            .await;
            let id = self
                .port
                .enqueue(new_wake(
                    "w-1",
                    WakeKind::Message,
                    source::message(&m.id),
                    Priority::Human,
                    HUMAN,
                ))
                .await
                .unwrap()
                .unwrap();
            let wakes = self.port.pending("w-1").await.unwrap();
            let rendered = self.port.render_wake("w-1", &wakes).await.unwrap();
            assert!(
                rendered
                    .cursor_targets
                    .contains(&("c-1/lobby".into(), m.seq))
            );
            self.port
                .settle(
                    &[id],
                    Settle::Dropped {
                        error: "synthetic trigger disposed".into(),
                    },
                    self.core.now(),
                )
                .await
                .unwrap();
            let b = Batch {
                session: "w-1".into(),
                cursor_targets: rendered.cursor_targets,
                prepared: rendered.prepared,
                ..Default::default()
            };
            assert!(b.ids().is_empty() && b.has_work());
            (b, rendered.text, m.seq)
        }

        async fn held_cursor(&self) -> bool {
            self.state
                .db
                .call(|s| {
                    cr::snapshot(
                        s.conn(),
                        "w-1",
                        &["c-1/lobby".into()],
                        crewd_store::now_ms(),
                    )
                    .map(|p| p.blocks_channel("c-1/lobby"))
                })
                .await
                .unwrap()
                .unwrap()
        }

        async fn close(mut self) {
            use super::super::core::Input;
            self.exec.lock().hang_on_stop.clear();
            let (tx, closed) = tokio::sync::oneshot::channel();
            self.core.handle(Input::Shutdown(tx)).await;
            self.drain().await;
            assert!(
                self.core.live.is_empty() && self.core.starting.is_empty(),
                "normal controlled terminal drain"
            );
            assert!(self.core.shutdown_done());
            tokio::time::timeout(std::time::Duration::from_secs(2), closed)
                .await
                .unwrap()
                .unwrap();
            self.exec.lock().events.clear();
            self.drain().await; // executor streams finish; no real child was spawned
            let db = self.state.db.clone();
            drop(self.core);
            drop(self.port);
            drop(self.state);
            tokio::task::spawn_blocking(move || db.shutdown())
                .await
                .unwrap();
        }
    }

    // Provider notification -> actual Codex Core -> SessionEvent -> real Core/DbPort/SQLite.
    fn usage_from_provider_core(
        thread: &str,
        values: &[serde_json::Value],
    ) -> Vec<crewd_exec::session::SessionEvent> {
        use crewd_exec::codex::{
            proto::ThreadOptions,
            session::{CodexAction, CodexCore, CodexCoreConfig, Opening},
        };
        let now = std::time::Instant::now();
        let mut core = CodexCore::new(
            Opening {
                pid: 42,
                cwd: std::path::PathBuf::from("/tmp/synthetic-usage"),
                options: ThreadOptions::default(),
                recorded: None,
                client_version: "fixture".into(),
                brief: None,
                unix_now: 0,
            },
            CodexCoreConfig::default(),
            now,
        );
        for (method, result) in [
            ("initialize", serde_json::json!({"userAgent":"fixture"})),
            ("thread/start", serde_json::json!({"thread":{"id":thread}})),
        ] {
            let id = core
                .take_actions()
                .into_iter()
                .find_map(|a| {
                    let CodexAction::Write(line) = a else {
                        return None;
                    };
                    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
                    (v["method"] == method).then(|| v["id"].clone())
                })
                .unwrap();
            core.on_line(
                &serde_json::json!({"id":id,"result":result}).to_string(),
                now,
            );
        }
        core.on_recorded(Ok(()), now);
        core.take_actions();
        let mut out = Vec::new();
        for params in values {
            core.on_line(
                &serde_json::json!({"method":"thread/tokenUsage/updated","params":params})
                    .to_string(),
                now,
            );
            out.extend(core.take_actions().into_iter().filter_map(|a| match a {
                CodexAction::Event(e @ crewd_exec::session::SessionEvent::Usage(_)) => Some(e),
                _ => None,
            }));
        }
        out
    }

    #[tokio::test]
    async fn usage_evidence_real_core_db_retains_provider_snapshots_without_wakes_or_progress() {
        use super::super::core::Input;
        use crewd_exec::session::SessionEvent;
        let mut rig = StopDbRig::new().await;
        rig.wake("usage-fixture-start").await;
        let generation = rig.core.live["w-1"].generator;
        let thread = rig.core.live["w-1"].agent_session_id.clone();
        let before_progress = rig
            .port
            .session("w-1")
            .await
            .unwrap()
            .unwrap()
            .last_progress_at;
        let before = rig
            .port
            .read(|c| {
                Ok((
                    c.query_row("SELECT count(*) FROM wakes", [], |r| r.get::<_, i64>(0))?,
                    c.query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))?,
                ))
            })
            .await
            .unwrap();
        let raw = serde_json::json!({"last":{"inputTokens":10,"cachedInputTokens":3,"outputTokens":5,"reasoningOutputTokens":2,"totalTokens":15},
            "total":{"totalTokens":9007199254740993u64},"future":{"kept":"yes"}});
        let one = serde_json::json!({"threadId":thread,"tokenUsage":raw});
        let two = serde_json::json!({"threadId":thread,"turnId":"provider-turn","tokenUsage":{"total":{"totalTokens":9007199254740994u64},"last":{"cacheWriteInputTokens":0}}});
        let events = usage_from_provider_core(
            &thread,
            &[
                serde_json::json!({"threadId":"foreign","tokenUsage":raw}),
                one.clone(),
                one.clone(),
                two,
            ],
        );
        assert_eq!(events.len(), 3, "foreign thread must not reach persistence");
        let mut observer = rig.state.hub.subscribe();
        let stale = events[0].clone();
        rig.core
            .handle(Input::Exec {
                session: "w-1".into(),
                generator: generation + 1,
                event: stale,
            })
            .await;
        for event in events {
            rig.core
                .handle(Input::Exec {
                    session: "w-1".into(),
                    generator: generation,
                    event,
                })
                .await;
        }
        let page = rig
            .port
            .read(|c| Ok(crewd_store::runner_usage::list(c, "w-1", 0, 200)?))
            .await
            .unwrap();
        assert_eq!(
            page.items.len(),
            2,
            "exact repeats do not create more evidence"
        );
        assert_eq!(page.items[0].turn_id, None);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&page.items[0].token_usage_json).unwrap(),
            raw
        );
        assert!(
            !page.items[0]
                .token_usage_json
                .contains("cacheWriteInputTokens")
        );
        assert!(
            page.items[1]
                .token_usage_json
                .contains("\"cacheWriteInputTokens\":0")
        );
        assert_eq!(page.items[1].turn_id.as_deref(), Some("provider-turn"));
        let after = rig
            .port
            .read(|c| {
                Ok((
                    c.query_row("SELECT count(*) FROM wakes", [], |r| r.get::<_, i64>(0))?,
                    c.query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(
            before, after,
            "usage must not schedule, notify, or fabricate progress"
        );
        assert!(
            matches!(
                observer.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "telemetry must not publish live-refresh events"
        );
        assert_eq!(
            rig.port
                .session("w-1")
                .await
                .unwrap()
                .unwrap()
                .last_progress_at,
            before_progress
        );
        assert_eq!(rig.exec.launches("w-1").len(), 1);
        // First provider observation remains a snapshot; no derived aggregate is stored.
        assert_eq!(page.items[0].provider, "codex");
        assert!(rig.state.db.health().snapshot().is_none());
        assert!(matches!(
            usage_from_provider_core(&thread, &[one])[0],
            SessionEvent::Usage(_)
        ));
        let durable_dir = std::mem::replace(&mut rig.dir, tempfile::tempdir().unwrap());
        let durable_path = durable_dir.path().join("crewd.db");
        rig.close().await;
        let reopened = crewd_store::Store::open(&durable_path).unwrap();
        let after_restart =
            crewd_store::runner_usage::list(reopened.conn(), "w-1", 0, 200).unwrap();
        assert_eq!(
            after_restart, page,
            "actual Core/DbPort evidence survives orderly backend shutdown"
        );
    }

    #[tokio::test]
    async fn usage_evidence_persist_failure_is_visible_without_runner_restart_or_fake_receipt() {
        use super::super::core::Input;
        let mut rig = StopDbRig::new().await;
        rig.wake("usage-storage-refusal").await;
        let generation = rig.core.live["w-1"].generator;
        let thread = rig.core.live["w-1"].agent_session_id.clone();
        rig.state.db.call(|s| s.conn().execute_batch("CREATE TRIGGER usage_refusal BEFORE INSERT ON runner_usage BEGIN SELECT RAISE(ABORT,'synthetic usage storage refusal'); END;")).await.unwrap().unwrap();
        let event = usage_from_provider_core(
            &thread,
            &[serde_json::json!({"threadId":thread,"tokenUsage":{"total":{"totalTokens":17}}})],
        )
        .remove(0);
        let before = rig
            .port
            .session("w-1")
            .await
            .unwrap()
            .unwrap()
            .last_progress_at;
        rig.core
            .handle(Input::Exec {
                session: "w-1".into(),
                generator: generation,
                event,
            })
            .await;
        let fault = rig
            .state
            .db
            .health()
            .snapshot()
            .expect("real persistence failure is recorded");
        assert_eq!(fault.kind, crewd_store::db_health::DbFaultKind::Sql);
        assert!(
            rig.port
                .read(|c| Ok(crewd_store::runner_usage::list(c, "w-1", 0, 50)?))
                .await
                .unwrap()
                .items
                .is_empty()
        );
        assert_eq!(
            rig.port
                .session("w-1")
                .await
                .unwrap()
                .unwrap()
                .last_progress_at,
            before
        );
        assert_eq!(rig.exec.launches("w-1").len(), 1);
        assert!(rig.core.live.contains_key("w-1"));
        rig.close().await;
    }

    // All cases below use real SQLite reservations and actual Core dispatch, with
    // controlled executor futures/events. They are not public actor/helper E2E.
    async fn launch_zero_id_body(r: &mut StopDbRig) -> (Batch, i64) {
        let (body, text, seq) = r.cursor_body().await;
        let row = r.port.session("w-1").await.unwrap().unwrap();
        let executor = r.core.execs.get("claude").unwrap();
        assert!(
            r.core
                .launch_carrying(row, executor, Some(text), body)
                .await
                .unwrap()
        );
        (r.core.starting["w-1"].batch.clone(), seq)
    }

    #[derive(Clone, Copy)]
    enum ZeroStarting {
        Failed,
        BinaryReplaced,
        StopFailed,
        HaltFailed,
    }

    async fn zero_id_starting_terminal_case(kind: ZeroStarting) {
        use super::super::{core::Input, exec::LaunchError};
        let mut r = StopDbRig::new().await;
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        r.exec
            .lock()
            .held_launches
            .insert("w-1".into(), (entered_tx, release_rx));
        let (batch, _) = launch_zero_id_body(&mut r).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), entered_rx)
            .await
            .unwrap()
            .unwrap();
        let held_before = r.held_cursor().await;
        let no_process_before = r.exec.launches("w-1").is_empty();
        let mut shutdown = None;
        match kind {
            ZeroStarting::StopFailed => {
                r.stop().await.unwrap();
            }
            ZeroStarting::HaltFailed => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                r.core.handle(Input::Shutdown(tx)).await;
                shutdown = Some(rx);
            }
            _ => {}
        }
        let held_while_terminal_unresolved = r.held_cursor().await;
        let error = if matches!(kind, ZeroStarting::BinaryReplaced) {
            LaunchError::BinaryReplaced
        } else {
            LaunchError::Failed("synthetic held zero-ID launch failure".into())
        };
        release_tx.send(Err(error)).unwrap();
        r.drain().await;
        if let Some(rx) = shutdown {
            assert!(r.core.shutdown_done());
            rx.await.unwrap();
        }
        let held_after = r.held_cursor().await;
        let cursor_after = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let no_process_after = r.core.live.is_empty()
            && r.core.starting.is_empty()
            && r.exec.launches("w-1").is_empty();
        let receipt = r
            .port
            .ack(
                &batch,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await
            .unwrap();
        r.close().await;
        assert!(held_before && held_while_terminal_unresolved && no_process_before);
        assert!(!held_after && no_process_after);
        assert_eq!(
            cursor_after, 0,
            "failed/refused/cancelled body was never delivered"
        );
        assert!(
            receipt.stale,
            "old exact lease retired after actual future terminal"
        );
    }

    // S42 original-file/Core-boot composition; controlled executor, no public helper/process.
    async fn s42_boot_cause(r: &StopDbRig, m: &Message) -> Option<Vec<(String, u32)>> {
        let m = m.clone();
        r.state
            .db
            .call(
                move |s| match collab::causal::read_message(s.conn(), &m).unwrap() {
                    collab::causal::MessageCause::Resolved(f) => Some(
                        f.components()
                            .iter()
                            .map(|v| (v.chain_id(), v.hop()))
                            .collect(),
                    ),
                    collab::causal::MessageCause::Unresolved { .. } => None,
                },
            )
            .await
            .unwrap()
    }
    async fn s42_boot_stamp(r: &StopDbRig, m: &Message) -> (Message, String) {
        let id = m.id.clone();
        r.state
            .db
            .call(move |s| {
                (
                    collab::messages::get(s.conn(), &id).unwrap(),
                    crewd_store::meta_get(s.conn(), &format!("causal.message.v1.{id}"))
                        .unwrap()
                        .unwrap(),
                )
            })
            .await
            .unwrap()
    }
    async fn s42_boot_state(r: &StopDbRig) -> serde_json::Value {
        r.state
            .db
            .call(|s| {
                let raw = crewd_store::meta_get(s.conn(), "causal.session.v1.w-1")
                    .unwrap()
                    .unwrap();
                serde_json::from_str::<serde_json::Value>(&raw).unwrap()["value"].clone()
            })
            .await
            .unwrap()
    }
    async fn s42_boot_meta(r: &StopDbRig) -> Vec<(String, String)> {
        r.state
            .db
            .call(|s| {
                let mut q = s
                    .conn()
                    .prepare("SELECT key,value FROM meta ORDER BY key")
                    .unwrap();
                q.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<(String, String)>>>()
                    .unwrap()
            })
            .await
            .unwrap()
    }
    async fn s42_boot_actual_brief_success(r: &mut StopDbRig) {
        use crewd_exec::{
            delivery::Subject,
            session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
        };
        r.exec.emit(
            "w-1",
            SessionEvent::Delivery {
                id: BRIEF_DELIVERY_ID,
                subject: Subject::Brief,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        r.drain().await;
    }

    #[tokio::test]
    async fn s42_original_file_boot_preserves_committed_basis_hold_and_immutable_tuple() {
        let mut r = StopDbRig::new().await;
        r.state
            .collab(|tx| collab::cursors::pin_to_end(tx, "w-2", "c-1/lobby"))
            .await
            .unwrap();
        let (initial_batch, seq) = launch_zero_id_body(&mut r).await;
        r.drain().await;
        let initial_cursor = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let initial_rows = r
            .state
            .db
            .call(|s| crewd_store::messages::list(s.conn(), "c-1/lobby", 0, None, 100))
            .await
            .unwrap()
            .unwrap();
        eprintln!(
            "S42 original boot before actual success human_seq={seq} cursor={initial_cursor} targets={:?} rows={initial_rows:?}",
            initial_batch.cursor_targets
        );
        s42_boot_actual_brief_success(&mut r).await;
        let cursor_after_initial_ack = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        eprintln!("S42 original boot after actual success cursor={cursor_after_initial_ack}");
        let child = post_as(
            &r.state,
            collab::Sender::Session("w-1".into()),
            "c-1/lobby",
            "S42 committed before original-file boot",
            &[],
        )
        .await;
        let before_cause = s42_boot_cause(&r, &child).await;
        let before_stamp = s42_boot_stamp(&r, &child).await;
        let before_state = s42_boot_state(&r).await;
        let chain = before_cause
            .as_ref()
            .expect("actual committed brief produced basis")[0]
            .0
            .clone();
        // Actual typed durable hold API. This is retention proof, not a fabricated
        // over-budget caller oracle; live hop17 admission has its separate caller case.
        let hold = collab::causal::ChainHold {
            chain_id: chain,
            from: "w-1".into(),
            target: "w-2".into(),
            hop: 1,
            reason: "explicit typed retention fixture".into(),
            at_ms: crewd_store::now_ms(),
        };
        let installed = r
            .state
            .collab({
                let hold = hold.clone();
                move |tx| collab::causal::hold(tx, &hold)
            })
            .await
            .unwrap();
        let cursor_before = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let foreign_cursor_before = r.port.cursor("w-2", "c-1/lobby").await.unwrap();
        let mut r = r.reboot().await; // original path Db.shutdown / Store::open / actual Core.boot
        let cause_after = s42_boot_cause(&r, &child).await;
        let stamp_after = s42_boot_stamp(&r, &child).await;
        let state_after = s42_boot_state(&r).await;
        let holds_after = r.port.causal_holds(Some("w-2")).await.unwrap();
        let cursor_after = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let foreign_cursor_after = r.port.cursor("w-2", "c-1/lobby").await.unwrap();
        let diagnostics = r
            .port
            .causal_diagnostics("w-2", r.core.now())
            .await
            .unwrap();
        r.drain().await;
        let repeated_cause = s42_boot_cause(&r, &child).await;
        let repeated_holds = r.port.causal_holds(Some("w-2")).await.unwrap();
        let live_after_boot = r.core.live.keys().cloned().collect::<Vec<_>>();
        let original_held = r.core.held.contains("w-1");
        let restart_prompt = r.core.restart_prompt;
        eprintln!(
            "S42 original boot live={live_after_boot:?} original_held={original_held} restart_prompt={restart_prompt:?}"
        );
        r.close().await;
        assert!(installed, "S42 actual typed hold installed before shutdown");
        assert_eq!(
            cursor_after_initial_ack, seq,
            "actual brief DB ack committed original prefix before subsequent own post"
        );
        assert_eq!(
            cause_after, before_cause,
            "S42 boot cannot reset committed cause or add hop"
        );
        assert_eq!(
            stamp_after, before_stamp,
            "S42 original-file reopen retains immutable actual tuple/stamp"
        );
        assert_eq!(
            state_after["basis"], before_state["basis"],
            "S42 boot preserves canonical committed basis"
        );
        assert_eq!(
            state_after["epoch"], before_state["epoch"],
            "S42 boot must not manufacture Work epoch"
        );
        assert_eq!(
            holds_after,
            vec![hold.clone()],
            "S42 boot cannot clear exact persisted hold episode"
        );
        assert_eq!(
            cursor_before, seq,
            "own child was not delivered as an incoming body before boot"
        );
        // Ordinary boot replan skips leading own rows using the shared scanner. A
        // foreign receiver's held prefix must remain untouched by that cleanup.
        assert_eq!(
            cursor_after, child.seq,
            "boot may skip exactly the leading own child, never a foreign prefix"
        );
        assert_eq!(
            foreign_cursor_after, foreign_cursor_before,
            "boot cannot consume a foreign held input"
        );
        assert!(
            diagnostics
                .iter()
                .any(|(channel, blockage)| channel == "c-1/lobby"
                    && blockage.holds.iter().any(|h| h.chain_id == hold.chain_id
                        && h.from == hold.from
                        && h.target == hold.target)),
            "S42 reopened exact held actual tuple remains observable, not merely missing-cursor diagnostic"
        );
        assert_eq!(repeated_cause, cause_after);
        assert_eq!(repeated_holds, holds_after);
        assert!(
            !live_after_boot.iter().any(|id| id == "w-1")
                && original_held
                && restart_prompt.is_some(),
            "boot must hold the original crashed session for its actual restart decision; other legitimate backlog remains independent"
        );
    }

    #[tokio::test]
    async fn s42_original_file_boot_retires_pending_eligible_old_ticket_without_overwriting_new_owner()
     {
        use crewd_exec::{
            delivery::Subject,
            session::{BRIEF_DELIVERY_ID, DeliveryOutcome, ExitInfo, ExitStatusKind, SessionEvent},
        };
        let mut r = StopDbRig::new().await;
        let (_, _) = launch_zero_id_body(&mut r).await;
        r.drain().await;
        s42_boot_actual_brief_success(&mut r).await;
        let previous = post_as(
            &r.state,
            collab::Sender::Session("w-1".into()),
            "c-1/lobby",
            "S42 previous committed cause before pending restart",
            &[],
        )
        .await;
        let previous_cause = s42_boot_cause(&r, &previous).await;
        r.exec.emit(
            "w-1",
            SessionEvent::Exited(ExitInfo {
                status: ExitStatusKind::Code(0),
                stopped_by_user: false,
                hit_limit: false,
            }),
        );
        r.drain().await;
        let (mut old, _) = launch_zero_id_body(&mut r).await;
        r.drain().await;
        // Genuine StandIn producer publishes via registered original observer before
        // Core receives the queued event. No fabricated observe_success factory.
        r.exec.emit(
            "w-1",
            SessionEvent::Delivery {
                id: BRIEF_DELIVERY_ID,
                subject: Subject::Brief,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        match old.lease.as_mut().expect("actual old claim lease") {
            BatchLease::CausalDatabase { claimed, observed } => {
                let p = &claimed.pending;
                *observed = Some(Box::new(
                    r.state
                        .causal_fence
                        .success(p, p.generation(), BRIEF_DELIVERY_ID)
                        .unwrap(),
                ));
            }
            other => panic!("actual causal lease required: {other:?}"),
        }
        let eligible = post_as(
            &r.state,
            collab::Sender::Session("w-1".into()),
            "c-1/lobby",
            "S42 final emission before original DB ack and boot",
            &[],
        )
        .await;
        let early_cause = s42_boot_cause(&r, &eligible).await;
        let early_stamp = s42_boot_stamp(&r, &eligible).await;
        let state_before = s42_boot_state(&r).await;
        let mut r = r.reboot().await; // drop queued old receipt; boot conservatively terminals pending
        let boot_cause = s42_boot_cause(&r, &eligible).await;
        let boot_stamp = s42_boot_stamp(&r, &eligible).await;
        let boot_state = s42_boot_state(&r).await;
        let previous_after = s42_boot_cause(&r, &previous).await;
        let after_boot_receipt = r
            .port
            .ack(
                &old,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await;
        // Existing launch helper is a white-box caller of actual Core launch_carrying;
        // it does NOT pretend that restart held intent was answered through public RPC.
        let (new, newseq) = launch_zero_id_body(&mut r).await;
        r.drain().await;
        let new_held = r.held_cursor().await;
        let different_owner = match (old.lease.as_ref(), new.lease.as_ref()) {
            (
                Some(BatchLease::CausalDatabase { claimed: old, .. }),
                Some(BatchLease::CausalDatabase { claimed: new, .. }),
            ) => old.pending != new.pending,
            _ => false,
        }; // compare immutable exact receipts, not observed Some/None or local generation integers
        let before_late = s42_boot_meta(&r).await;
        let cursor_before_late = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let late = r
            .port
            .ack(
                &old,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await;
        let after_late = s42_boot_meta(&r).await;
        let cursor_after_late = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let held_after_late = r.held_cursor().await;
        s42_boot_actual_brief_success(&mut r).await;
        let new_cursor = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let new_basis = s42_boot_state(&r).await;
        let late_again = r
            .port
            .ack(
                &old,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await;
        let after_final_late = s42_boot_state(&r).await;
        let final_eligible = s42_boot_cause(&r, &eligible).await;
        let final_stamp = s42_boot_stamp(&r, &eligible).await;
        r.close().await;
        assert!(
            early_cause.is_none(),
            "S42 real emitted-before-DB-ack child remains conservative"
        );
        assert!(
            boot_cause.is_none() && final_eligible.is_none(),
            "S42 boot/old receipt cannot retroactively activate terminal Eligible child"
        );
        assert_eq!(boot_stamp, early_stamp);
        assert_eq!(
            final_stamp, early_stamp,
            "immutable candidate stamp remains historical"
        );
        assert_eq!(
            previous_after, previous_cause,
            "S42 pending reset preserves prior committed ancestry"
        );
        assert_eq!(boot_state["basis"], state_before["basis"]);
        assert_eq!(boot_state["epoch"], state_before["epoch"]);
        assert!(
            after_boot_receipt.unwrap().stale,
            "S42 old exact ticket stale after actual boot reset"
        );
        assert!(
            different_owner && new_held && held_after_late,
            "actual new durable lease survives late old ticket"
        );
        assert!(late.unwrap().stale);
        assert_eq!(
            before_late, after_late,
            "old exact observed success cannot mutate new owner metadata"
        );
        assert_eq!(
            cursor_after_late, cursor_before_late,
            "old ticket cannot advance new claimed prefix"
        );
        assert_eq!(
            new_cursor, newseq,
            "actual new original success still commits its prefix"
        );
        assert!(late_again.unwrap().stale);
        assert_eq!(
            after_final_late["basis"], new_basis["basis"],
            "late old success cannot replace new canonical basis"
        );
        assert_eq!(after_final_late["epoch"], new_basis["epoch"]);
    }

    // Real Core callback -> original observer -> real SQL abort -> same-fate retry.
    // Work here is real note_progress Tx, not a replacement for actual hook/Tool caller coverage.
    async fn s42_core_known_delivered_sql_retry_case(new_work: bool) {
        use crewd_exec::{
            delivery::Subject,
            session::{BRIEF_DELIVERY_ID, DeliveryOutcome, ExitInfo, ExitStatusKind, SessionEvent},
        };
        let mut r = StopDbRig::new().await;
        r.state
            .collab(|tx| collab::cursors::pin_to_end(tx, "w-1", "c-1/lobby"))
            .await
            .unwrap();
        let input = post_as(
            &r.state,
            collab::Sender::Human,
            "c-1/lobby",
            "S42 original successful debt input",
            &[],
        )
        .await;
        let input_cause = s42_boot_cause(&r, &input).await;
        // Real Message row, shared rendering and actual Core dispatch; no System-only surrogate.
        let id = r
            .port
            .enqueue(new_wake(
                "w-1",
                WakeKind::Message,
                source::message(&input.id),
                Priority::Human,
                HUMAN,
            ))
            .await
            .unwrap()
            .unwrap();
        r.core
            .handle(super::super::core::Input::Collab(
                super::super::core::CollabFact::WakeEnqueued {
                    session: "w-1".into(),
                },
            ))
            .await;
        r.drain().await;
        let mut original = r.core.live["w-1"]
            .inflight
            .as_ref()
            .expect("actual launched body inflight")
            .batch
            .clone();
        let actual_ids = original.ids();
        let actual_targets = original.cursor_targets.clone();
        let actual_payload = r
            .exec
            .launches("w-1")
            .last()
            .and_then(|l| l.initial.clone());
        let output_key = match original.lease.as_ref().unwrap() {
            BatchLease::CausalDatabase { claimed, .. } => {
                format!("causal.output.v1.{}", claimed.pending.stable_key())
            }
            other => panic!("actual causal lease required: {other:?}"),
        };
        let capture=r.state.db.call(move|s| {
            let raw=crewd_store::meta_get(s.conn(),&output_key).unwrap().unwrap();
            serde_json::from_str::<serde_json::Value>(&raw).unwrap()["value"]["receipt"]["capture"].clone()
        }).await.unwrap();
        let actual_tuple = capture["tuples"].as_array().unwrap().iter().any(|t| {
            t["id"] == input.id && t["channel"] == input.channel_id && t["seq"] == input.seq
        });
        let row_before = r.row(id).await;
        let cursor_before = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let queued = post_as(
            &r.state,
            collab::Sender::Session("w-1".into()),
            "c-1/lobby",
            "S42 before original final success must stay unknown",
            &[],
        )
        .await;
        r.state.db.call(move |s|s.conn().execute_batch(&format!(
            "CREATE TRIGGER s42_original_delivered_abort BEFORE UPDATE OF state ON wakes WHEN OLD.id={id} AND NEW.state='delivered' BEGIN SELECT RAISE(ABORT,'s42 original Delivered SQL abort'); END;"
        )).unwrap()).await.unwrap();
        r.exec.emit(
            "w-1",
            SessionEvent::Delivery {
                id: BRIEF_DELIVERY_ID,
                subject: Subject::Brief,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        let original_success = match original
            .lease
            .as_mut()
            .expect("actual original causal lease")
        {
            BatchLease::CausalDatabase { claimed, observed } => {
                let p = &claimed.pending;
                let ticket = r
                    .state
                    .causal_fence
                    .success(p, p.generation(), BRIEF_DELIVERY_ID)
                    .unwrap();
                *observed = Some(Box::new(ticket.clone()));
                ticket
            }
            other => panic!("actual causal lease required: {other:?}"),
        };
        let eligible = post_as(
            &r.state,
            collab::Sender::Session("w-1".into()),
            "c-1/lobby",
            "S42 emitted original success before SQL acknowledgement",
            &[],
        )
        .await;
        let before_failure = s42_boot_meta(&r).await;
        let eligible_before = s42_boot_cause(&r, &eligible).await;
        r.drain().await; // real Input::Session callback stores KnownDelivered, then actual SQL error
        let debt_after_error = r.core.terminal_sessions();
        let after_failure = s42_boot_meta(&r).await;
        let row_after_error = r.row(id).await;
        let cursor_after_error = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let deferred = r.core.try_deliver("w-1").await;
        let debt_after_defer = r.core.terminal_sessions();
        let exact_retry_error = r.core.retry_terminal("w-1").await;
        let basis_before_work = s42_boot_state(&r).await;
        let applied_work = if new_work {
            r.state
                .collab(|tx| {
                    collab::sessions::note_progress(tx, "w-1", collab::ProgressKind::ToolCall)
                })
                .await
                .unwrap();
            let message = post_as(
                &r.state,
                collab::Sender::Session("w-1".into()),
                "c-1/lobby",
                "S42 newer real work applied before older same-fate retry",
                &[],
            )
            .await;
            let stamp = s42_boot_stamp(&r, &message).await;
            Some((
                message.clone(),
                s42_boot_cause(&r, &message).await,
                s42_boot_state(&r).await,
                stamp,
            ))
        } else {
            None
        };
        // A subsequent actual process terminal must not convert known Delivered to Failed.
        r.exec.emit(
            "w-1",
            SessionEvent::Exited(ExitInfo {
                status: ExitStatusKind::Code(0),
                stopped_by_user: false,
                hit_limit: false,
            }),
        );
        r.drain().await;
        let debt_after_exit = r.core.terminal_sessions();
        let row_after_exit = r.row(id).await;
        r.state
            .db
            .call(|s| {
                s.conn()
                    .execute_batch("DROP TRIGGER s42_original_delivered_abort")
                    .unwrap()
            })
            .await
            .unwrap();
        let repaired_retry = r.core.retry_terminal("w-1").await;
        let debt_after_retry = r.core.terminal_sessions();
        let row_after_retry = r.row(id).await;
        let cursor_after_retry = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let eligible_after = s42_boot_cause(&r, &eligible).await;
        let queued_after = s42_boot_cause(&r, &queued).await;
        let basis_after_retry = s42_boot_state(&r).await;
        let work_after_retry = if let Some((message, _, _, _)) = &applied_work {
            let m = message.clone();
            let frontier = r
                .state
                .db
                .call(
                    move |s| match collab::causal::read_message(s.conn(), &m).unwrap() {
                        collab::causal::MessageCause::Resolved(frontier) => {
                            Some(serde_json::to_value(frontier).unwrap())
                        }
                        collab::causal::MessageCause::Unresolved { .. } => None,
                    },
                )
                .await
                .unwrap();
            Some((frontier, s42_boot_stamp(&r, message).await))
        } else {
            None
        };
        // Repeated original exact capability must be idempotent, not another SQL fate/hop.
        let before_duplicate = s42_boot_meta(&r).await;
        let duplicate = r
            .port
            .ack(
                &original,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await;
        let after_duplicate = s42_boot_meta(&r).await;
        let row_after_duplicate = r.row(id).await;
        let same_ticket = match original.lease.as_ref().unwrap() {
            BatchLease::CausalDatabase {
                observed: Some(ticket),
                ..
            } => ticket.as_ref() == &original_success,
            _ => false,
        };
        // Fresh actual owner and original final event after old debt commits; no hand-made generation.
        let (new, newseq) = launch_zero_id_body(&mut r).await;
        r.drain().await;
        let distinct = match (original.lease.as_ref(), new.lease.as_ref()) {
            (
                Some(BatchLease::CausalDatabase { claimed: a, .. }),
                Some(BatchLease::CausalDatabase { claimed: b, .. }),
            ) => a.pending != b.pending,
            _ => false,
        };
        // Old exact committed capability while the NEW owner is active, before its success.
        let active_meta_before = s42_boot_meta(&r).await;
        let active_cursor_before = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let active_basis_before = s42_boot_state(&r).await;
        let active_lease_before = r.core.live["w-1"]
            .inflight
            .as_ref()
            .unwrap()
            .batch
            .lease
            .clone();
        let active_held_before = r.held_cursor().await;
        let old_during_active = r
            .port
            .ack(
                &original,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await;
        let active_meta_after = s42_boot_meta(&r).await;
        let active_cursor_after = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let active_basis_after = s42_boot_state(&r).await;
        let active_lease_after = r.core.live["w-1"]
            .inflight
            .as_ref()
            .unwrap()
            .batch
            .lease
            .clone();
        let active_held_after = r.held_cursor().await;
        s42_boot_actual_brief_success(&mut r).await;
        let new_cursor = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let new_basis = s42_boot_state(&r).await;
        let before_old_late = s42_boot_meta(&r).await;
        let old_late = r
            .port
            .ack(
                &original,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await;
        let after_old_late = s42_boot_meta(&r).await;
        let after_old_basis = s42_boot_state(&r).await;
        let eligible_final = s42_boot_cause(&r, &eligible).await;
        r.close().await; // normal controlled executor/Core/Db terminal before primary oracles
        assert_eq!(
            actual_ids,
            vec![id],
            "S42 exact actual Message trigger is claimed without riders"
        );
        assert_eq!(row_before.source, source::message(&input.id));
        assert!(
            actual_targets.contains(&(input.channel_id.clone(), input.seq)),
            "S42 actual Message target conveys input prefix"
        );
        assert!(
            actual_payload
                .as_deref()
                .is_some_and(|text| text.contains(&input.text)),
            "S42 actual launched body contains exact Human input"
        );
        assert!(
            actual_tuple,
            "S42 actual persisted claimed receipt captures exact input tuple"
        );
        assert_eq!(
            debt_after_error,
            vec!["w-1"],
            "S42 actual known Delivered SQL error retains original Core debt"
        );
        assert_eq!(
            before_failure, after_failure,
            "S42 failed actual ack must roll back all causal/reservation metadata"
        );
        assert_eq!(row_before.state, cw::WakeState::Inflight);
        assert_eq!(row_after_error, row_before);
        assert_eq!(cursor_after_error, cursor_before);
        assert!(
            deferred
                .unwrap_err()
                .to_string()
                .contains("s42 original Delivered SQL abort"),
            "new attempt retries exact debt before claim"
        );
        assert_eq!(debt_after_defer, vec!["w-1"]);
        assert!(
            exact_retry_error
                .unwrap_err()
                .to_string()
                .contains("s42 original Delivered SQL abort"),
            "actual SQL trigger reached on same-fate retry"
        );
        assert_eq!(
            debt_after_exit,
            vec!["w-1"],
            "Exit cannot convert known original Delivered into Failed"
        );
        assert_eq!(
            row_after_exit, row_before,
            "known successful output cannot requeue/drop/refund after Exit"
        );
        repaired_retry.unwrap();
        assert!(debt_after_retry.is_empty());
        assert_eq!(row_after_retry.state, cw::WakeState::Delivered);
        assert_eq!(
            row_after_retry.attempts, row_before.attempts,
            "same-fate retry neither reclains nor refunds attempts"
        );
        assert_eq!(
            cursor_after_retry, input.seq,
            "one exact original confirmed prefix"
        );
        assert!(
            eligible_before.is_none() && queued_after.is_none(),
            "pre-success never retroactively gains parent; pre-ack is conservative"
        );
        let root = input_cause.as_ref().unwrap()[0].0.clone();
        assert_eq!(
            eligible_after,
            Some(vec![(root, 1)]),
            "original trusted success resolves exact old child once"
        );
        if let Some((_, preack_cause, work_state, preack_stamp)) = applied_work {
            assert!(
                preack_cause.is_none(),
                "S42 actual Eligible Work post remains conservative before original DB ack"
            );
            let stamp: serde_json::Value = serde_json::from_str(&preack_stamp.1).unwrap();
            let resolution = &stamp["value"]["resolution"];
            assert_eq!(
                resolution["kind"], "eligible",
                "S42 actual original success ticket stamps Eligible, not a free root"
            );
            let epoch = work_state["epoch"].as_u64().unwrap();
            assert_eq!(
                epoch,
                basis_before_work["epoch"].as_u64().unwrap() + 1,
                "S42 one actual ToolCall increments exactly once"
            );
            assert_eq!(work_state["applied"], work_state["epoch"]);
            assert_eq!(
                work_state["basis"], basis_before_work["basis"],
                "S42 pending original ack preserves previous canonical basis"
            );
            let mut expected = capture["frontier"].clone();
            assert_eq!(
                expected.as_array().unwrap().len(),
                1,
                "S42 original actual input is a single exact cause"
            );
            for component in expected.as_array_mut().unwrap() {
                component["key"]["reset"] =
                    serde_json::json!({"kind":"work","session":"w-1","epoch":epoch});
                component["hop"] = serde_json::json!(0);
            }
            assert_eq!(
                resolution["frontier"], expected,
                "S42 actual preack stamp retains original roots with qualified Work zero"
            );
            assert_eq!(work_state["provisional"]["basis"], expected);
            assert_eq!(work_state["provisional"]["epoch"], epoch);
            assert_eq!(work_state["provisional"]["output"], resolution["output"]);
            let (resolved, after_stamp) = work_after_retry.unwrap();
            assert_eq!(
                preack_stamp, after_stamp,
                "S42 same-fate retry does not rewrite original Eligible message history"
            );
            assert_eq!(
                resolved,
                Some(expected.clone()),
                "S42 original ack resolves exact qualified Work cause once"
            );
            assert_eq!(
                basis_after_retry["basis"], expected,
                "S42 same-fate retry installs current Work provisional instead of old Initial hop1"
            );
            assert_eq!(basis_after_retry["epoch"], epoch);
            assert_eq!(basis_after_retry["applied"], epoch);
            assert!(basis_after_retry["provisional"].is_null());
        }
        duplicate.unwrap();
        assert!(same_ticket);
        assert_eq!(after_duplicate, before_duplicate);
        assert_eq!(
            row_after_duplicate, row_after_retry,
            "duplicate exact original receipt is not another row fate"
        );
        old_during_active.unwrap();
        assert_eq!(
            active_meta_before, active_meta_after,
            "S42 old receipt cannot mutate active new owner metadata"
        );
        assert_eq!(active_cursor_before, active_cursor_after);
        assert_eq!(active_basis_before, active_basis_after);
        assert_eq!(
            active_lease_before, active_lease_after,
            "S42 exact active new lease remains held"
        );
        assert!(
            active_held_before && active_held_after,
            "S42 older committed duplicate cannot release current reservation"
        );
        assert!(distinct);
        assert_eq!(
            new_cursor, newseq,
            "new actual owner still commits original success"
        );
        old_late.unwrap();
        assert_eq!(
            before_old_late, after_old_late,
            "late older committed success cannot mutate new owner"
        );
        assert_eq!(after_old_basis["basis"], new_basis["basis"]);
        assert_eq!(after_old_basis["epoch"], new_basis["epoch"]);
        assert_eq!(eligible_final, eligible_after);
    }
    #[tokio::test]
    async fn s42_core_original_delivered_sql_failure_retains_debt_and_same_fate_retry_once() {
        s42_core_known_delivered_sql_retry_case(false).await;
    }
    #[tokio::test]
    async fn s42_core_original_delivered_sql_retry_preserves_new_work_and_new_owner_basis() {
        s42_core_known_delivered_sql_retry_case(true).await;
    }

    #[tokio::test]
    async fn s42_active_unwitnessed_ack_keeps_owner_and_retired_replay_is_stale() {
        use crewd_exec::{
            delivery::Subject,
            session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
        };
        let mut r = StopDbRig::new().await;
        let (batch, seq) = launch_zero_id_body(&mut r).await;
        r.drain().await;
        let before = r
            .state
            .db
            .call(|s| {
                let mut q = s
                    .conn()
                    .prepare("SELECT key,value FROM meta ORDER BY key")
                    .unwrap();
                q.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
            })
            .await
            .unwrap();
        let held_before = r.held_cursor().await;
        let refusal = r
            .port
            .ack(
                &batch,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await;
        let after = r
            .state
            .db
            .call(|s| {
                let mut q = s
                    .conn()
                    .prepare("SELECT key,value FROM meta ORDER BY key")
                    .unwrap();
                q.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
            })
            .await
            .unwrap();
        let held_after = r.held_cursor().await;
        let cursor_after = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        r.exec.emit(
            "w-1",
            SessionEvent::Delivery {
                id: BRIEF_DELIVERY_ID,
                subject: Subject::Brief,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        r.drain().await;
        let delivered_cursor = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let retired = r
            .port
            .ack(
                &batch,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await;
        let final_cursor = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        r.close().await;
        assert!(held_before && held_after);
        assert_eq!(
            before, after,
            "unwitnessed active ack cannot mutate causal or reservation state"
        );
        assert_eq!(cursor_after, 0);
        assert!(
            refusal
                .unwrap_err()
                .to_string()
                .contains("missing original emitted success")
        );
        assert_eq!(
            delivered_cursor, seq,
            "actual observed successful delivery still commits"
        );
        assert!(
            retired.unwrap().stale,
            "unwitnessed retired replay is not new Applied"
        );
        assert_eq!(final_cursor, seq);
    }

    // S42 family6 actual DbPort/shared renderer/Core. Only executor emission is
    // controlled. Planner inputs are real committed messages; no private stamp is
    // inserted, no Frontier is manufactured, no live/provider process is started.
    type S42Family6Frontier = std::collections::BTreeMap<String, u32>;

    // No new futures dependency or unsafe pinning. Catch each observation/cleanup
    // poll while preserving the owned Rig outside the observed future.
    struct S42Family6Catch<F: std::future::Future> {
        inner: std::pin::Pin<Box<F>>,
    }
    impl<F: std::future::Future> std::future::Future for S42Family6Catch<F> {
        type Output = Result<F::Output, Box<dyn std::any::Any + Send>>;
        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            let this = self.get_mut();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                this.inner.as_mut().poll(cx)
            })) {
                Ok(std::task::Poll::Ready(value)) => std::task::Poll::Ready(Ok(value)),
                Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
                Err(panic) => std::task::Poll::Ready(Err(panic)),
            }
        }
    }
    fn s42_family6_catch<F: std::future::Future>(future: F) -> S42Family6Catch<F> {
        S42Family6Catch {
            inner: Box::pin(future),
        }
    }
    fn s42_family6_panic_note(payload: &(dyn std::any::Any + Send)) -> &str {
        if let Some(note) = payload.downcast_ref::<String>() {
            note.as_str()
        } else if let Some(note) = payload.downcast_ref::<&'static str>() {
            note
        } else {
            "non-string panic payload"
        }
    }
    async fn s42_family6_finish<T>(
        r: StopDbRig,
        observation: Result<anyhow::Result<T>, Box<dyn std::any::Any + Send>>,
    ) -> anyhow::Result<T> {
        // Even inherited setup helpers' unwrap/index panics first attempt the
        // normal Core terminal drain and actual DB shutdown. Never make them None.
        let cleanup = s42_family6_catch(r.close()).await;
        match (observation, cleanup) {
            (Err(original), Err(cleanup)) => {
                eprintln!(
                    "S42 family6 original panic: {}; cleanup panic: {}",
                    s42_family6_panic_note(original.as_ref()),
                    s42_family6_panic_note(cleanup.as_ref())
                );
                std::panic::resume_unwind(original)
            }
            (Err(original), Ok(())) => std::panic::resume_unwind(original),
            (Ok(Err(original)), Err(cleanup)) => {
                eprintln!(
                    "S42 family6 original error: {original:#}; cleanup panic: {}",
                    s42_family6_panic_note(cleanup.as_ref())
                );
                std::panic::resume_unwind(cleanup)
            }
            (Ok(Ok(_)), Err(cleanup)) => std::panic::resume_unwind(cleanup),
            (Ok(result), Ok(())) => result,
        }
    }

    async fn s42_family6_frontier(
        r: &StopDbRig,
        m: &Message,
    ) -> anyhow::Result<Option<S42Family6Frontier>> {
        let m = m.clone();
        Ok(r.state
            .db
            .call(move |s| -> collab::Result<_> {
                Ok(match collab::causal::read_message(s.conn(), &m)? {
                    collab::causal::MessageCause::Resolved(f) => Some(
                        f.components()
                            .iter()
                            .map(|c| (c.chain_id(), c.hop()))
                            .collect(),
                    ),
                    collab::causal::MessageCause::Unresolved { .. } => None,
                })
            })
            .await??)
    }

    async fn s42_family6_enqueue(r: &StopDbRig, target: &str, m: &Message) -> anyhow::Result<i64> {
        let at = MessageRef {
            channel: m.channel_id.clone(),
            seq: m.seq,
        };
        let (facts, _) = r
            .port
            .message(&at)
            .await?
            .ok_or_else(|| anyhow::anyhow!("actual committed message missing"))?;
        let now = r.core.now();
        let ctx = r
            .port
            .channel_ctx(&at.channel, now)
            .await?
            .ok_or_else(|| anyhow::anyhow!("actual channel missing"))?;
        let chosen = crewd_wake::plan::plan(&facts, &ctx, None, now)
            .into_iter()
            .find(|t| t.session == target)
            .ok_or_else(|| anyhow::anyhow!("actual planner omitted explicit target"))?;
        let origin = crewd_wake::breaker::origin_of(WakeKind::Message, Some(&facts.sender), target);
        r.port
            .enqueue(NewWake {
                session: chosen.session,
                kind: WakeKind::Message,
                source: source::message(&facts.id),
                priority: chosen.priority,
                not_before: now,
                note: None,
                origin,
            })
            .await?
            .ok_or_else(|| anyhow::anyhow!("new planner row deduplicated"))
    }

    async fn s42_family6_dispatch(
        r: &mut StopDbRig,
        target: &str,
    ) -> anyhow::Result<(Batch, u64, String)> {
        // Actual Core classification determines primary/riders and exact windows.
        r.core.try_deliver(target).await?;
        r.drain().await;
        let inflight = r
            .core
            .live
            .get(target)
            .and_then(|l| l.inflight.as_ref())
            .ok_or_else(|| anyhow::anyhow!("actual Core did not expose an inflight body"))?;
        let id = inflight
            .delivery
            .ok_or_else(|| anyhow::anyhow!("actual command has not been sent"))?;
        let body = if id == crewd_exec::session::BRIEF_DELIVERY_ID {
            r.exec
                .launches(target)
                .last()
                .and_then(|l| l.initial.clone())
        } else {
            r.exec.sent(target).last().cloned()
        }
        .ok_or_else(|| anyhow::anyhow!("actual executor body missing"))?;
        Ok((inflight.batch.clone(), id, body))
    }

    async fn s42_family6_delivered(
        r: &mut StopDbRig,
        target: &str,
        mut batch: Batch,
        id: u64,
    ) -> anyhow::Result<Batch> {
        use crewd_exec::{
            delivery::Subject,
            session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
        };
        r.exec.emit(
            target,
            SessionEvent::Delivery {
                id,
                subject: if id == BRIEF_DELIVERY_ID {
                    Subject::Brief
                } else {
                    Subject::Steer
                },
                outcome: DeliveryOutcome::Delivered,
            },
        );
        // StandIn emits through the real configured observer BEFORE broadcasting.
        // Retain that original typed ticket for later duplicate-ack observation.
        let lease = batch
            .lease
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("actual original claim lease missing"))?;
        match lease {
            BatchLease::CausalDatabase { claimed, observed } => {
                let p = &claimed.pending;
                *observed = Some(Box::new(r.state.causal_fence.success(
                    p,
                    p.generation(),
                    id,
                )?));
            }
            _ => anyhow::bail!("actual causal Db lease required"),
        }
        r.drain().await;
        Ok(batch)
    }

    async fn s42_family6_post(
        r: &StopDbRig,
        sender: collab::Sender,
        text: &str,
        target: &str,
    ) -> Message {
        post_as(&r.state, sender, "c-1/lobby", text, &[target]).await
    }

    #[tokio::test]
    async fn s42_actual_core_fanout_reconvergence_preserves_per_key_max_not_global_or_sum() {
        let mut r = StopDbRig::new().await;
        let observed = s42_family6_catch(async {
            let x = s42_family6_post(&r, collab::Sender::Human, "S42_F6_X", "w-1").await;
            let y = s42_family6_post(&r, collab::Sender::Human, "S42_F6_Y", "w-1").await;
            let fx = s42_family6_frontier(&r, &x).await?;
            let fy = s42_family6_frontier(&r, &y).await?;
            s42_family6_enqueue(&r, "w-1", &y).await?;
            let (b, id, initial_body) = s42_family6_dispatch(&mut r, "w-1").await?;
            s42_family6_delivered(&mut r, "w-1", b, id).await?;
            let a1 = post_as(
                &r.state,
                collab::Sender::Session("w-1".into()),
                "c-1/lobby",
                "S42_F6_A1",
                &["w-2", "cap-1"],
            )
            .await;
            // The same immutable A1 is actually rendered to both branch owners.
            let mut branches = Vec::new();
            for target in ["w-2", "cap-1"] {
                // Persisted mentions include both branches. Use untouched actual
                // committed facts/context and the production planner for each row.
                s42_family6_enqueue(&r, target, &a1).await?;
                let (b, id, body) = s42_family6_dispatch(&mut r, target).await?;
                branches.push((target.to_string(), body));
                s42_family6_delivered(&mut r, target, b, id).await?;
            }
            let b2 = s42_family6_post(
                &r,
                collab::Sender::Session("w-2".into()),
                "S42_F6_B2",
                "w-1",
            )
            .await;
            let c2 = s42_family6_post(
                &r,
                collab::Sender::Session("cap-1".into()),
                "S42_F6_C2",
                "w-2",
            )
            .await;
            s42_family6_enqueue(&r, "w-2", &c2).await?;
            let (b, id, _) = s42_family6_dispatch(&mut r, "w-2").await?;
            s42_family6_delivered(&mut r, "w-2", b, id).await?;
            let b3 = s42_family6_post(
                &r,
                collab::Sender::Session("w-2".into()),
                "S42_F6_B3",
                "w-1",
            )
            .await;
            let z =
                s42_family6_post(&r, collab::Sender::Human, "S42_F6_Z_INDEPENDENT", "w-1").await;
            let fz = s42_family6_frontier(&r, &z).await?;
            s42_family6_enqueue(&r, "w-1", &b3).await?;
            s42_family6_enqueue(&r, "w-1", &z).await?;
            let (b, id, reconvergence_body) = s42_family6_dispatch(&mut r, "w-1").await?;
            let targets = b.cursor_targets.clone();
            s42_family6_delivered(&mut r, "w-1", b, id).await?;
            let out = s42_family6_post(
                &r,
                collab::Sender::Session("w-1".into()),
                "S42_F6_RECONVERGED",
                "cap-1",
            )
            .await;
            Ok((
                fx,
                fy,
                fz,
                s42_family6_frontier(&r, &a1).await?,
                s42_family6_frontier(&r, &b2).await?,
                s42_family6_frontier(&r, &c2).await?,
                s42_family6_frontier(&r, &b3).await?,
                s42_family6_frontier(&r, &out).await?,
                initial_body,
                branches,
                reconvergence_body,
                targets,
                z.seq,
            ))
        })
        .await;
        let observed = s42_family6_finish(r, observed).await;
        let (fx, fy, fz, a1, b2, c2, b3, out, initial, branches, body, targets, zseq) =
            observed.expect("closed fixture setup");
        let mut expected = S42Family6Frontier::new();
        for f in [fx, fy] {
            for (key, hop) in f.expect("actual Human root") {
                assert_eq!(hop, 0);
                expected.insert(key, 1);
            }
        }
        assert_eq!(
            expected.len(),
            2,
            "two exact independent actual Human roots"
        );
        assert_eq!(a1, Some(expected.clone()));
        for hop in expected.values_mut() {
            *hop = 2;
        }
        assert_eq!(b2, Some(expected.clone()));
        assert_eq!(c2, Some(expected.clone()));
        for hop in expected.values_mut() {
            *hop = 3;
        }
        assert_eq!(b3, Some(expected.clone()));
        for hop in expected.values_mut() {
            *hop = 4;
        }
        for (key, hop) in fz.expect("actual independent Z root") {
            assert_eq!(hop, 0);
            expected.insert(key, 1);
        }
        assert_eq!(
            out,
            Some(expected),
            "actual reconvergence is component-wise max+1, never sum or globalmax"
        );
        assert!(initial.contains("S42_F6_X") && initial.contains("S42_F6_Y"));
        assert_eq!(branches.len(), 2);
        assert!(branches.iter().all(|(_, t)| t.contains("S42_F6_A1")));
        assert!(
            body.contains("S42_F6_B2")
                && body.contains("S42_F6_C2")
                && body.contains("S42_F6_B3")
                && body.contains("S42_F6_Z_INDEPENDENT")
        );
        assert!(targets.contains(&("c-1/lobby".into(), zseq)));
    }

    #[tokio::test]
    async fn s42_actual_core_released_rider_activates_only_its_later_own_output() {
        let mut r = StopDbRig::new().await;
        let observed = s42_family6_catch(async {
            let mut fillers = Vec::new();
            // Real production cap, not a lowered test renderer/window. Byte budget
            // may make the first actual window shorter; observations use its exact end.
            for index in 0..collab::cursors::UNREAD_CAP {
                fillers.push(
                    s42_family6_post(
                        &r,
                        collab::Sender::Human,
                        &format!("S42_F6_FILL_{index:03}"),
                        "w-1",
                    )
                    .await,
                );
            }
            let rider =
                s42_family6_post(&r, collab::Sender::Human, "S42_F6_DISTINCT_RIDER", "w-1").await;
            let rider_root = s42_family6_frontier(&r, &rider).await?;
            let primary_id = s42_family6_enqueue(&r, "w-1", &fillers[0]).await?;
            let rider_id = s42_family6_enqueue(&r, "w-1", &rider).await?;
            let (first, id, first_body) = s42_family6_dispatch(&mut r, "w-1").await?;
            let first_primary = first.wakes.iter().map(|w| w.id).collect::<Vec<_>>();
            let first_riders = first.riders.iter().map(|w| w.id).collect::<Vec<_>>();
            let first_targets = first.cursor_targets.clone();
            let first = s42_family6_delivered(&mut r, "w-1", first, id).await?;
            let primary_after = r.row(primary_id).await;
            let rider_after = r.row(rider_id).await;
            let first_post = s42_family6_post(
                &r,
                collab::Sender::Session("w-1".into()),
                "S42_F6_AFTER_PRIMARY_ONLY",
                "cap-1",
            )
            .await;
            let first_cause = s42_family6_frontier(&r, &first_post).await?;
            let mut windows = Vec::new();
            let mut early = None;
            let mut replay = None;
            let mut delivered_rider = false;
            for _ in 0..(collab::cursors::UNREAD_CAP + 4) {
                let row = r.row(rider_id).await;
                if row.state == cw::WakeState::Delivered {
                    delivered_rider = true;
                    break;
                }
                // A genuine ordinary post before the new candidate is queued
                // witnesses the previous successful nonempty canonical basis.
                let prior_message = s42_family6_post(
                    &r,
                    collab::Sender::Session("w-1".into()),
                    "S42_F6_REAL_PRIOR_BASIS_CONTROL",
                    "cap-1",
                )
                .await;
                let prior_basis = s42_family6_frontier(&r, &prior_message).await?;
                let (next, next_id, text) = s42_family6_dispatch(&mut r, "w-1").await?;
                let ids = next.wakes.iter().map(|w| w.id).collect::<Vec<_>>();
                let riders = next.riders.iter().map(|w| w.id).collect::<Vec<_>>();
                let targets = next.cursor_targets.clone();
                if ids.contains(&rider_id) {
                    // This post occurs before the new output's accepted producer.
                    // Existing nonempty basis is legal and must remain exactly
                    // unchanged; the new rider parent must never be retroactive.
                    let message = s42_family6_post(
                        &r,
                        collab::Sender::Session("w-1".into()),
                        "S42_F6_QUEUED_RIDER_PARENT",
                        "cap-1",
                    )
                    .await;
                    let before = s42_family6_frontier(&r, &message).await?;
                    let stamp_before = s42_boot_stamp(&r, &message).await;
                    let owner_before = r.core.live["w-1"].inflight.as_ref().unwrap().batch.clone();
                    let meta_before = s42_boot_meta(&r).await;
                    let cursor_before = r.port.cursor("w-1", "c-1/lobby").await?;
                    let ack = r
                        .port
                        .ack(
                            &first,
                            Ack::Delivered {
                                riders: Riders::Release,
                            },
                        )
                        .await?;
                    let meta_after = s42_boot_meta(&r).await;
                    let cursor_after = r.port.cursor("w-1", "c-1/lobby").await?;
                    let owner_after = r.core.live["w-1"].inflight.as_ref().unwrap().batch.clone();
                    replay = Some((
                        ack.stale,
                        meta_before == meta_after,
                        cursor_before == cursor_after,
                        owner_before.lease == owner_after.lease,
                        owner_before.ids() == owner_after.ids(),
                    ));
                    early = Some((message, before, prior_basis, stamp_before));
                }
                windows.push((ids, riders, targets, text));
                s42_family6_delivered(&mut r, "w-1", next, next_id).await?;
            }
            // The bounded loop's last iteration can commit the row; re-read actual
            // terminal state rather than infer success from timeout/loop exhaustion.
            let rider_final = r.row(rider_id).await;
            delivered_rider |= rider_final.state == cw::WakeState::Delivered;
            let early_after = match &early {
                Some((m, _, _, _)) => s42_family6_frontier(&r, m).await?,
                None => None,
            };
            let early_stamp_after = match &early {
                Some((m, _, _, _)) => Some(s42_boot_stamp(&r, m).await),
                None => None,
            };
            let after = s42_family6_post(
                &r,
                collab::Sender::Session("w-1".into()),
                "S42_F6_AFTER_RIDER_OWN_OUTPUT",
                "cap-1",
            )
            .await;
            let after_cause = s42_family6_frontier(&r, &after).await?;
            let repeat = s42_family6_post(
                &r,
                collab::Sender::Session("w-1".into()),
                "S42_F6_AFTER_RIDER_REPEAT",
                "cap-1",
            )
            .await;
            let repeat_cause = s42_family6_frontier(&r, &repeat).await?;
            Ok((
                rider_root,
                primary_id,
                rider_id,
                rider.seq,
                first_primary,
                first_riders,
                first_targets,
                first_body,
                primary_after,
                rider_after,
                first_cause,
                windows,
                early,
                replay,
                early_after,
                early_stamp_after,
                delivered_rider,
                rider_final,
                after_cause,
                repeat_cause,
            ))
        })
        .await;
        let observed = s42_family6_finish(r, observed).await;
        let (
            root,
            primary_id,
            rider_id,
            rider_seq,
            first_primary,
            first_riders,
            targets,
            body,
            primary_after,
            rider_after,
            first_cause,
            windows,
            early,
            replay,
            early_after,
            early_stamp_after,
            delivered_rider,
            rider_final,
            after_cause,
            repeat_cause,
        ) = observed.expect("closed fixture setup");
        let root = root.expect("actual distinct Human rider root");
        assert_eq!(root.len(), 1);
        let (key, hop) = root.into_iter().next().unwrap();
        assert_eq!(hop, 0);
        assert_eq!(first_primary, vec![primary_id]);
        assert_eq!(
            first_riders,
            vec![rider_id],
            "actual Core.render_fitted classifies the unshown row as rider"
        );
        let end = targets.iter().find(|(ch, _)| ch == "c-1/lobby").unwrap().1;
        assert!(end < rider_seq);
        assert!(!body.contains("S42_F6_DISTINCT_RIDER"));
        assert_eq!(primary_after.state, cw::WakeState::Delivered);
        assert_eq!(
            rider_after.state,
            cw::WakeState::Pending,
            "original ack releases, does not deliver, rider"
        );
        assert!(
            !first_cause
                .expect("primary actual output basis")
                .contains_key(&key),
            "unshown released rider cannot activate its cause"
        );
        assert!(delivered_rider);
        assert_eq!(rider_final.state, cw::WakeState::Delivered);
        assert_eq!(
            rider_after.attempts, 0,
            "released rider refunds its unshown claim attempt"
        );
        assert_eq!(
            rider_final.attempts, 1,
            "only its actual delivered own claim remains counted"
        );
        let shown = windows
            .iter()
            .filter(|(ids, _, _, _)| ids.contains(&rider_id))
            .collect::<Vec<_>>();
        assert_eq!(
            shown.len(),
            1,
            "one actual owned output finally represents rider"
        );
        assert!(shown[0].3.contains("S42_F6_DISTINCT_RIDER"));
        assert!(
            shown[0]
                .2
                .iter()
                .any(|(ch, end)| ch == "c-1/lobby" && *end >= rider_seq)
        );
        assert!(!shown[0].1.contains(&rider_id));
        let (_, early_before, prior_basis, stamp_before) =
            early.expect("actual queued-own-output post observed");
        assert!(
            prior_basis.as_ref().is_some_and(|f| !f.is_empty()),
            "the real previous successful output has a nonempty basis"
        );
        assert_eq!(
            early_before, prior_basis,
            "queued output preserves exact actual old basis"
        );
        assert!(
            !early_before.as_ref().unwrap().contains_key(&key),
            "unwritten/unconfirmed new rider cannot enter previous basis"
        );
        assert_eq!(
            early_after, prior_basis,
            "later ack cannot retroactively assign a new parent"
        );
        assert_eq!(
            early_stamp_after,
            Some(stamp_before),
            "immutable original post tuple/candidate/resolution remains unchanged"
        );
        let (_stale, meta, cursor, lease, ids) =
            replay.expect("original receipt retried during newer exact owner");
        assert!(
            meta && cursor && lease && ids,
            "duplicate old receipt cannot disturb new owner or frontier"
        );
        assert_eq!(
            after_cause.as_ref().and_then(|f| f.get(&key)),
            Some(&1),
            "rider activates only after its own actual receipt, exactly hop1"
        );
        assert_eq!(
            repeat_cause, after_cause,
            "same receipt does not repeatedly advance ordinary output"
        );
    }

    #[tokio::test]
    async fn s44_zero_id_starting_actual_failed_future_releases_exact_owner() {
        zero_id_starting_terminal_case(ZeroStarting::Failed).await;
    }
    #[tokio::test]
    async fn s44_zero_id_starting_actual_binary_refusal_releases_exact_owner() {
        zero_id_starting_terminal_case(ZeroStarting::BinaryReplaced).await;
    }
    #[tokio::test]
    async fn s44_zero_id_starting_stop_retains_owner_until_actual_failed_future() {
        zero_id_starting_terminal_case(ZeroStarting::StopFailed).await;
    }
    #[tokio::test]
    async fn s44_zero_id_starting_halt_retains_owner_until_actual_failed_future() {
        zero_id_starting_terminal_case(ZeroStarting::HaltFailed).await;
    }

    #[derive(Clone, Copy)]
    enum ZeroStop {
        Delivered,
        Failed,
        NoReceipt,
    }

    async fn zero_id_stop_terminal_case(kind: ZeroStop) {
        use crewd_exec::{
            delivery::Subject,
            session::{BRIEF_DELIVERY_ID, DeliveryOutcome, ExitInfo, ExitStatusKind, SessionEvent},
        };
        let mut r = StopDbRig::new().await;
        let (batch, seq) = launch_zero_id_body(&mut r).await;
        r.drain().await;
        r.exec.lock().hang_on_stop.insert("w-1".into());
        r.stop().await.unwrap();
        let held_after_stop = r.held_cursor().await;
        let still_physical = r.core.live.get("w-1").is_some_and(|l| l.stopping);
        match kind {
            ZeroStop::Delivered | ZeroStop::Failed => {
                r.exec.emit(
                    "w-1",
                    SessionEvent::Delivery {
                        id: BRIEF_DELIVERY_ID,
                        subject: Subject::Brief,
                        outcome: if matches!(kind, ZeroStop::Delivered) {
                            DeliveryOutcome::Delivered
                        } else {
                            DeliveryOutcome::Unsubmitted("synthetic stopped body failure".into())
                        },
                    },
                );
                r.drain().await;
            }
            ZeroStop::NoReceipt => {}
        }
        let held_after_receipt = r.held_cursor().await;
        let cursor_after_receipt = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        r.exec.emit(
            "w-1",
            SessionEvent::Exited(ExitInfo {
                status: ExitStatusKind::Code(0),
                stopped_by_user: true,
                hit_limit: false,
            }),
        );
        r.drain().await;
        let held_after_exit = r.held_cursor().await;
        let cursor_after_exit = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let no_live = r.core.live.is_empty() && r.core.starting.is_empty();
        let receipt = r
            .port
            .ack(
                &batch,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await
            .unwrap();
        r.close().await;
        assert!(
            held_after_stop && still_physical,
            "Stop alone cannot release a physical owner"
        );
        assert_eq!(held_after_receipt, !matches!(kind, ZeroStop::Delivered));
        let expected_cursor = if matches!(kind, ZeroStop::Delivered) {
            seq
        } else {
            0
        };
        assert_eq!(cursor_after_receipt, expected_cursor);
        assert!(!held_after_exit && no_live && receipt.stale);
        assert_eq!(
            cursor_after_exit, expected_cursor,
            "Exit must not reinterpret confirmed Delivered"
        );
    }

    #[tokio::test]
    async fn s44_zero_id_stop_late_actual_delivered_commits_before_exit() {
        zero_id_stop_terminal_case(ZeroStop::Delivered).await;
    }
    #[tokio::test]
    async fn s44_zero_id_stop_failed_receipt_keeps_owner_until_actual_exit() {
        zero_id_stop_terminal_case(ZeroStop::Failed).await;
    }
    #[tokio::test]
    async fn s44_zero_id_stop_without_receipt_releases_only_at_actual_exit() {
        zero_id_stop_terminal_case(ZeroStop::NoReceipt).await;
    }

    #[derive(Clone, Copy)]
    enum ZeroExit {
        Halt,
        Quota,
        Natural,
        StreamClosed,
    }

    async fn zero_id_exit_terminal_case(kind: ZeroExit) {
        use crewd_exec::session::{ExitInfo, ExitStatusKind, SessionEvent};
        let mut r = StopDbRig::new().await;
        let (batch, _) = launch_zero_id_body(&mut r).await;
        r.drain().await;
        let mut held_while_halting = None;
        if matches!(kind, ZeroExit::Halt) {
            r.exec.lock().hang_on_stop.insert("w-1".into());
            r.core.halt("w-1");
            held_while_halting = Some(r.held_cursor().await);
        }
        if matches!(kind, ZeroExit::StreamClosed) {
            r.exec.end_stream("w-1"); // actual sender close, no fabricated StreamEnded input
        } else {
            r.exec.emit(
                "w-1",
                SessionEvent::Exited(ExitInfo {
                    status: ExitStatusKind::Code(1),
                    stopped_by_user: false,
                    hit_limit: matches!(kind, ZeroExit::Quota),
                }),
            );
        }
        r.drain().await;
        let held_after = r.held_cursor().await;
        let cursor_after = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        let no_live = r.core.live.is_empty() && r.core.starting.is_empty();
        let receipt = r
            .port
            .ack(
                &batch,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await
            .unwrap();
        r.close().await;
        assert!(held_while_halting.is_none_or(|held| held));
        assert!(!held_after && no_live && receipt.stale);
        assert_eq!(
            cursor_after, 0,
            "no successful body receipt advances the cursor"
        );
    }

    #[tokio::test]
    async fn s44_zero_id_halt_releases_only_after_actual_exit() {
        zero_id_exit_terminal_case(ZeroExit::Halt).await;
    }
    #[tokio::test]
    async fn s44_zero_id_quota_exit_releases_exact_owner() {
        zero_id_exit_terminal_case(ZeroExit::Quota).await;
    }
    #[tokio::test]
    async fn s44_zero_id_natural_exit_releases_exact_owner() {
        zero_id_exit_terminal_case(ZeroExit::Natural).await;
    }
    #[tokio::test]
    async fn s44_zero_id_actual_stream_sender_close_releases_exact_owner() {
        zero_id_exit_terminal_case(ZeroExit::StreamClosed).await;
    }

    #[tokio::test]
    async fn s44_zero_id_reopened_boot_fences_old_lease_from_new_owner() {
        use crewd_exec::{
            delivery::Subject,
            session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
        };
        let mut r = StopDbRig::new().await;
        let (old, _) = launch_zero_id_body(&mut r).await;
        r.drain().await;
        let old_held = r.held_cursor().await;
        let mut r = r.reboot().await; // real Db close/reopen and ordinary Core boot reset
        let held_after_boot = r.held_cursor().await;
        let (new, seq) = launch_zero_id_body(&mut r).await;
        r.drain().await;
        let held_new = r.held_cursor().await;
        let different_exact_owner = old.lease != new.lease;
        // Actor generations are local to a Core incarnation, so do not fabricate
        // a cross-restart generation mismatch. The durable exact lease is the fence.
        let old_receipt = r
            .port
            .ack(
                &old,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await
            .unwrap();
        let held_after_old = r.held_cursor().await;
        let cursor_after_old = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        r.exec.emit(
            "w-1",
            SessionEvent::Delivery {
                id: BRIEF_DELIVERY_ID,
                subject: Subject::Brief,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        r.drain().await;
        let held_after_new = r.held_cursor().await;
        let cursor_after_new = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        r.close().await;
        assert!(old_held && !held_after_boot && held_new && different_exact_owner);
        assert!(old_receipt.stale && held_after_old);
        assert_eq!(cursor_after_old, 0);
        assert!(!held_after_new);
        assert_eq!(cursor_after_new, seq);
    }

    #[tokio::test]
    async fn s44_zero_id_actual_stop_exit_sql_failure_retains_owner_until_exact_retry() {
        use super::super::core::{CollabFact, Input};
        use crewd_exec::session::{ExitInfo, ExitStatusKind, SessionEvent};
        let mut r = StopDbRig::new().await;
        let (body, text, seq) = r.cursor_body().await;
        let row = r.port.session("w-1").await.unwrap().unwrap();
        let exec = r.core.execs.get("claude").unwrap();
        assert!(
            r.core
                .launch_carrying(row, exec, Some(text), body)
                .await
                .unwrap()
        );
        r.drain().await;
        let old = r.core.live["w-1"].inflight.as_ref().unwrap().batch.clone();
        r.exec.lock().hang_on_stop.insert("w-1".into());
        r.stop().await.unwrap(); // actual durable cancellation, but process is unresolved
        let held_before_exit = r.held_cursor().await;
        r.state
            .db
            .call(|s| {
                s.conn().execute_batch(
                    "CREATE TRIGGER s44_terminal_meta_fault BEFORE UPDATE ON meta
             WHEN OLD.key='cursor.reservation.v1.w-1'
             BEGIN SELECT RAISE(ABORT, 'synthetic exact terminal release fault'); END;",
                )
            })
            .await
            .unwrap()
            .unwrap();
        r.exec.emit(
            "w-1",
            SessionEvent::Exited(ExitInfo {
                status: ExitStatusKind::Code(0),
                stopped_by_user: true,
                hit_limit: false,
            }),
        );
        r.drain().await; // actual matching Exited; original atomic fate fails
        let dead = !r.core.live.contains_key("w-1");
        let held_after_failure = r.held_cursor().await;
        let fresh = r.enqueue_only("fresh-after-zero-terminal").await;
        for _ in 0..2 {
            r.core
                .handle(Input::Collab(CollabFact::WakeEnqueued {
                    session: "w-1".into(),
                }))
                .await;
            r.drain().await;
        }
        let launches_while_failed = r.exec.launches("w-1").len();
        let fresh_before = r.row(fresh).await;
        let cursor_before = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        r.state
            .db
            .call(|s| {
                s.conn()
                    .execute_batch("DROP TRIGGER s44_terminal_meta_fault")
            })
            .await
            .unwrap()
            .unwrap();
        r.core
            .handle(Input::Collab(CollabFact::WakeEnqueued {
                session: "w-1".into(),
            }))
            .await;
        r.drain().await;
        let new_running = r.core.live.contains_key("w-1");
        let launches_after = r.exec.launches("w-1").len();
        let fresh_after = r.row(fresh).await;
        let stale_old = r
            .port
            .ack(
                &old,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await
            .unwrap()
            .stale;
        let untouched_after_old = r.row(fresh).await;
        let cursor_after = r.port.cursor("w-1", "c-1/lobby").await.unwrap();
        r.close().await;
        assert!(held_before_exit && dead && held_after_failure);
        assert_eq!(
            launches_while_failed, 1,
            "persistent SQL error cannot admit a new process"
        );
        assert_eq!(
            (fresh_before.state, fresh_before.attempts),
            (cw::WakeState::Pending, 0)
        );
        assert!(
            cursor_before < seq && cursor_after == cursor_before,
            "no unconfirmed cursor advance"
        );
        assert!(
            new_running && launches_after == 2,
            "exact same-fate retry admits fresh work only after commit"
        );
        assert_eq!(
            (fresh_after.state, fresh_after.attempts),
            (cw::WakeState::Inflight, 1)
        );
        assert!(stale_old);
        assert_eq!(
            (untouched_after_old.state, untouched_after_old.attempts),
            (fresh_after.state, fresh_after.attempts)
        );
    }

    #[tokio::test]
    async fn db_human_stop_failed_settlement_boot_does_not_relaunch_old_wake() {
        let mut r = StopDbRig::new().await;
        let old = r.wake("old").await;
        assert_eq!(r.row(old).await.state, cw::WakeState::Inflight);
        assert_eq!(r.row(old).await.attempts, 1);
        r.block_drop(old).await;
        r.stop().await.unwrap(); // Successful public Stop, settlement itself fails on actual SQLite.
        assert_eq!(r.floor().await, old);
        assert!(!r.core.live.contains_key("w-1"));
        assert_eq!(
            r.row(old).await.state,
            cw::WakeState::Inflight,
            "failed atomic settlement changed no row"
        );
        assert_eq!(
            r.port.session("w-1").await.unwrap().unwrap().desired,
            Desired::Stopped
        );
        r.unblock_drop().await;
        let r = r.reboot().await;
        assert_eq!(
            r.row(old).await.state,
            cw::WakeState::Dropped,
            "I5: boot must honor successful human Stop, not resurrect old unconfirmed work"
        );
        assert_eq!(r.floor().await, old, "marker survives SQLite reopen");
        assert_eq!(
            r.row(old).await.attempts,
            1,
            "human cancellation is not retry/give-up"
        );
        assert!(!r.core.live.contains_key("w-1"));
        assert!(
            r.exec.launches("w-1").is_empty(),
            "no wake enqueued after Stop"
        );
    }

    #[tokio::test]
    async fn db_human_stop_without_fault_boot_does_not_relaunch_old_wake() {
        let mut r = StopDbRig::new().await;
        let old = r.wake("old").await;
        r.stop().await.unwrap();
        assert_eq!(r.row(old).await.state, cw::WakeState::Dropped);
        let r = r.reboot().await;
        assert_eq!(r.row(old).await.state, cw::WakeState::Dropped);
        assert!(r.exec.launches("w-1").is_empty());
    }

    #[tokio::test]
    async fn db_human_stop_new_work_later_crash_does_not_rescue_old_cancelled_wake() {
        let mut r = StopDbRig::new().await;
        let old = r.wake("old").await;
        r.block_drop(old).await;
        r.stop().await.unwrap();
        r.unblock_drop().await;
        let fresh = r.wake("after-stop").await;
        assert!(fresh > old);
        assert_eq!(
            r.floor().await,
            old,
            "new wake does not erase/advance the stop marker"
        );
        assert_eq!(
            r.exec.launches("w-1").len(),
            2,
            "new wake after Stop can launch"
        );
        assert_eq!(
            r.port.session("w-1").await.unwrap().unwrap().desired,
            Desired::Running
        );
        assert_eq!(r.row(fresh).await.state, cw::WakeState::Inflight);
        let r = r.reboot().await;
        assert_eq!(
            r.row(old).await.state,
            cw::WakeState::Dropped,
            "old cancel intent survives mutable desired=Running and later crash"
        );
        assert_eq!(r.row(old).await.attempts, 1);
        assert_eq!(
            r.row(fresh).await.state,
            cw::WakeState::Pending,
            "ordinary C25 rescue of fresh work"
        );
        assert_eq!(r.row(fresh).await.attempts, 1, "boot doesn't refund claim");
        assert!(
            r.core.held.contains("w-1"),
            "ordinary restart asks before resuming fresh work"
        );
    }

    #[tokio::test]
    async fn db_human_stop_pending_drop_fault_rolls_back_intent_and_reports_error() {
        let mut r = StopDbRig::new().await;
        let old = r.wake("inflight").await;
        let first = r.enqueue_only("pending-first").await;
        let second = r.enqueue_only("pending-fault").await;
        r.block_drop(second).await;
        let before_first = r.row(first).await;
        let before_second = r.row(second).await;
        assert!(
            r.stop().await.is_err(),
            "pending settlement failure is NOT successful Stop"
        );
        assert_eq!(
            r.floor().await,
            0,
            "intent rollback, not a false durable success"
        );
        assert_eq!(
            r.row(first).await,
            before_first,
            "even the earlier drop is rolled back"
        );
        assert_eq!(r.row(second).await, before_second);
        assert_eq!(r.row(old).await.state, cw::WakeState::Inflight);
        let row = r.port.session("w-1").await.unwrap().unwrap();
        assert_eq!(row.desired, Desired::Running);
        assert!(
            r.exec.lock().stops.is_empty(),
            "no process stop on failed durable transaction"
        );
        assert!(!r.core.live["w-1"].stopping);
        r.unblock_drop().await;
        r.stop().await.unwrap();
        assert_eq!(r.floor().await, second);
        for id in [old, first, second] {
            assert_eq!(r.row(id).await.state, cw::WakeState::Dropped);
        }
    }

    #[tokio::test]
    async fn db_human_stop_meta_write_failure_is_not_successful_stop() {
        let mut r = StopDbRig::new().await;
        let old = r.wake("old").await;
        let pending = r.enqueue_only("pending").await;
        r.state
            .db
            .call(|s| {
                s.conn()
                    .execute_batch(
                        "CREATE TRIGGER stop_intent_fault BEFORE INSERT ON meta
             WHEN NEW.key='human_stop_wake_floor:w-1'
             BEGIN SELECT RAISE(ABORT, 'synthetic cancellation intent failure'); END;",
                    )
                    .unwrap()
            })
            .await
            .unwrap();
        let error = r.stop().await.unwrap_err();
        assert!(format!("{error:?}").contains("synthetic cancellation intent failure"));
        assert_eq!(r.floor().await, 0);
        assert_eq!(r.row(old).await.state, cw::WakeState::Inflight);
        assert_eq!(r.row(pending).await.state, cw::WakeState::Pending);
        assert_eq!(
            r.port.session("w-1").await.unwrap().unwrap().desired,
            Desired::Running
        );
        assert!(r.exec.lock().stops.is_empty());
    }

    #[tokio::test]
    async fn db_human_stop_corrupt_marker_fails_closed_on_stop_claim_requeue_and_boot() {
        for corrupt in ["not-an-id", "-1", "9223372036854775808"] {
            let mut r = StopDbRig::new().await;
            let old = r.wake("old").await;
            let fresh = r.enqueue_only("fresh").await;
            r.state
                .db
                .call(move |s| {
                    crewd_store::meta_set(s.conn(), "human_stop_wake_floor:w-1", corrupt).unwrap()
                })
                .await
                .unwrap();
            let before = r.row(old).await;
            assert!(
                r.stop().await.is_err(),
                "{corrupt}: corrupted marker must not become zero"
            );
            assert!(r.port.claim(&r.batch(&[fresh]).await).await.is_err());
            assert!(
                r.port
                    .settle(&[old], Settle::Requeue, Ms(10))
                    .await
                    .is_err()
            );
            assert!(r.core.boot().await.is_err());
            assert_eq!(r.row(old).await, before, "no writes on failed marker read");
            assert_eq!(r.row(fresh).await.state, cw::WakeState::Pending);
            assert_eq!(
                r.port.session("w-1").await.unwrap().unwrap().desired,
                Desired::Running
            );
            assert!(r.exec.lock().stops.is_empty());
        }
    }

    #[tokio::test]
    async fn db_human_stop_marker_read_sql_failure_rolls_back_and_does_not_stop_process() {
        let mut r = StopDbRig::new().await;
        let old = r.wake("old").await;
        r.state
            .db
            .call(|s| {
                s.conn()
                    .execute_batch("ALTER TABLE meta RENAME TO meta_unavailable")
                    .unwrap()
            })
            .await
            .unwrap();
        let error = r.stop().await.unwrap_err();
        assert!(format!("{error:?}").contains("no such table"));
        r.state
            .db
            .call(|s| {
                s.conn()
                    .execute_batch("ALTER TABLE meta_unavailable RENAME TO meta")
                    .unwrap()
            })
            .await
            .unwrap();
        assert_eq!(r.floor().await, 0);
        assert_eq!(r.row(old).await.state, cw::WakeState::Inflight);
        assert_eq!(
            r.port.session("w-1").await.unwrap().unwrap().desired,
            Desired::Running
        );
        assert!(r.exec.lock().stops.is_empty());
    }

    #[tokio::test]
    async fn db_human_stop_late_actual_delivered_retains_row_and_cursor_through_reboot() {
        use super::super::core::{CollabFact, Input};
        use crewd_exec::{
            delivery::Subject,
            session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
        };
        let mut r = StopDbRig::new().await;
        let m = post_as(
            &r.state,
            collab::Sender::Human,
            "c-1/lobby",
            "合成迟到送达",
            &["w-1"],
        )
        .await;
        let old = r
            .port
            .enqueue(new_wake(
                "w-1",
                WakeKind::Message,
                source::message(&m.id),
                Priority::Human,
                HUMAN,
            ))
            .await
            .unwrap()
            .unwrap();
        r.core
            .handle(Input::Collab(CollabFact::WakeEnqueued {
                session: "w-1".into(),
            }))
            .await;
        r.drain().await;
        assert_eq!(r.row(old).await.state, cw::WakeState::Inflight);
        r.exec.lock().hang_on_stop.insert("w-1".into());
        r.stop().await.unwrap();
        assert_eq!(r.floor().await, old);
        r.exec.emit(
            "w-1",
            SessionEvent::Delivery {
                id: BRIEF_DELIVERY_ID,
                subject: Subject::Brief,
                outcome: DeliveryOutcome::Delivered,
            },
        );
        r.drain().await;
        assert_eq!(r.row(old).await.state, cw::WakeState::Delivered);
        assert!(r.port.cursor("w-1", "c-1/lobby").await.unwrap() >= m.seq);
        let r = r.reboot().await;
        assert_eq!(
            r.row(old).await.state,
            cw::WakeState::Delivered,
            "not Delivered -> Dropped"
        );
        assert!(r.port.cursor("w-1", "c-1/lobby").await.unwrap() >= m.seq);
        assert!(r.exec.launches("w-1").is_empty());
    }

    #[tokio::test]
    async fn db_human_stop_mixed_old_and_fresh_retry_is_atomic_and_not_giveup() {
        use crewd_wake::queue::MAX_ATTEMPTS;
        let mut r = StopDbRig::new().await;
        let old = r.wake("old").await;
        r.exec.lock().hang_on_stop.insert("w-1".into());
        r.stop().await.unwrap();
        let fresh = r.enqueue_only("fresh").await;
        r.port.claim(&r.batch(&[fresh]).await).await.unwrap();
        r.state
            .db
            .call(move |s| {
                s.conn()
                    .execute(
                        "UPDATE wakes SET attempts=?1 WHERE id=?2",
                        rusqlite::params![MAX_ATTEMPTS, old],
                    )
                    .unwrap()
            })
            .await
            .unwrap();
        let batch = r.batch(&[old, fresh]).await;
        r.block_drop(old).await;
        let before_old = r.row(old).await;
        let before_fresh = r.row(fresh).await;
        assert!(
            r.port
                .ack(
                    &batch,
                    Ack::Failed {
                        error: "synthetic failure".into(),
                        now: Ms(5_000),
                        jitter: 0.0
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(r.row(old).await, before_old);
        assert_eq!(
            r.row(fresh).await,
            before_fresh,
            "failed cancellation rolls back fresh retry too"
        );
        r.unblock_drop().await;
        let acked = r
            .port
            .ack(
                &batch,
                Ack::Failed {
                    error: "synthetic failure".into(),
                    now: Ms(5_000),
                    jitter: 0.0,
                },
            )
            .await
            .unwrap();
        assert!(
            acked.gave_up.is_empty(),
            "human stop at MAX is cancellation, not give-up"
        );
        assert_eq!(acked.retried, [(fresh, Ms(35_000))]);
        assert_eq!(r.row(old).await.state, cw::WakeState::Dropped);
        assert_eq!(r.row(old).await.attempts, MAX_ATTEMPTS);
        assert_eq!(r.row(old).await.last_error.as_deref(), Some("stopped"));
        assert_eq!(r.row(fresh).await.state, cw::WakeState::Pending);
        assert_eq!(r.row(fresh).await.attempts, 1);
        assert_eq!(r.row(fresh).await.not_before_ms, 35_000);
    }

    #[tokio::test]
    async fn db_human_stop_release_old_rider_drops_it_but_requeues_fresh_rider_atomically() {
        let r = StopDbRig::new().await;
        let old = r.enqueue_only("old-rider").await;
        r.port.claim(&r.batch(&[old]).await).await.unwrap();
        r.port.human_stop("w-1").await.unwrap();
        let delivered = r.enqueue_only("fresh-body").await;
        let rider = r.enqueue_only("fresh-rider").await;
        r.port
            .claim(&r.batch(&[delivered, rider]).await)
            .await
            .unwrap();
        let mut b = r.batch(&[delivered]).await;
        b.riders = r.batch(&[old, rider]).await.wakes;
        let m = post_as(
            &r.state,
            collab::Sender::Human,
            "c-1/lobby",
            "合成游标",
            &[],
        )
        .await;
        b.cursor_targets = vec![("c-1/lobby".into(), m.seq)];
        r.state
            .db
            .call(|s| {
                s.conn()
                    .execute_batch(
                        "CREATE TRIGGER stop_cursor_fault BEFORE INSERT ON read_cursors
             WHEN NEW.reader='w-1' AND NEW.channel_id='c-1/lobby'
             BEGIN SELECT RAISE(ABORT, 'synthetic cursor failure'); END;",
                    )
                    .unwrap()
            })
            .await
            .unwrap();
        assert!(
            r.port
                .ack(
                    &b,
                    Ack::Delivered {
                        riders: Riders::Release
                    }
                )
                .await
                .is_err()
        );
        for id in [old, delivered, rider] {
            assert_eq!(r.row(id).await.state, cw::WakeState::Inflight);
        }
        assert_eq!(r.port.cursor("w-1", "c-1/lobby").await.unwrap(), 0);
        r.state
            .db
            .call(|s| {
                s.conn()
                    .execute_batch("DROP TRIGGER stop_cursor_fault")
                    .unwrap()
            })
            .await
            .unwrap();
        let acked = r
            .port
            .ack(
                &b,
                Ack::Delivered {
                    riders: Riders::Release,
                },
            )
            .await
            .unwrap();
        assert_eq!(acked.origins, [SYSTEM]);
        assert_eq!(r.row(delivered).await.state, cw::WakeState::Delivered);
        assert_eq!(
            (r.row(old).await.state, r.row(old).await.attempts),
            (cw::WakeState::Dropped, 1)
        );
        assert_eq!(r.row(old).await.last_error.as_deref(), Some("stopped"));
        assert_eq!(
            (r.row(rider).await.state, r.row(rider).await.attempts),
            (cw::WakeState::Pending, 0)
        );
        assert_eq!(r.port.cursor("w-1", "c-1/lobby").await.unwrap(), m.seq);
    }

    #[tokio::test]
    async fn db_human_stop_old_and_fresh_claim_is_rejected_without_partial_new_claim() {
        let r = StopDbRig::new().await;
        let old = r.enqueue_only("old-pending").await;
        let stale_batch = r.batch(&[old]).await;
        r.port.human_stop("w-1").await.unwrap();
        let fresh = r.enqueue_only("fresh").await;
        let mut b = r.batch(&[fresh]).await;
        b.riders = stale_batch.wakes;
        assert!(
            r.port.claim(&b).await.is_err(),
            "cannot claim/send a stale cancelled batch"
        );
        assert_eq!(
            (r.row(fresh).await.state, r.row(fresh).await.attempts),
            (cw::WakeState::Pending, 0)
        );
        assert_eq!(r.row(old).await.state, cw::WakeState::Dropped);
        assert!(r.exec.launches("w-1").is_empty());
        assert!(
            r.port
                .settle(&[old], Settle::Delivered, Ms(1))
                .await
                .is_err()
        );
        assert!(r.port.settle(&[old], Settle::Requeue, Ms(1)).await.is_err());
    }

    #[tokio::test]
    async fn db_human_stop_repeated_stop_after_prune_never_regresses_marker_or_reuses_ids() {
        let r = StopDbRig::new().await;
        let old = r.enqueue_only("old").await;
        r.port.human_stop("w-1").await.unwrap();
        assert_eq!(r.floor().await, old);
        assert_eq!(
            r.port
                .prune_wakes(Ms(crewd_store::now_ms() + 60_000), 100)
                .await
                .unwrap(),
            1
        );
        r.port.human_stop("w-1").await.unwrap();
        assert_eq!(
            r.floor().await,
            old,
            "empty MAX(id) cannot roll back a persisted stop"
        );
        let fresh = r.enqueue_only("old").await;
        assert!(fresh > old, "real AUTOINCREMENT, not a time/desire proxy");
        r.port.claim(&r.batch(&[fresh]).await).await.unwrap();
        assert_eq!(r.floor().await, old);
        r.port.human_stop("w-1").await.unwrap();
        assert_eq!(
            r.floor().await,
            fresh,
            "another successful Stop cancels the newer work"
        );
        let out = r.port.fail_inflight("restart", Ms(1)).await.unwrap();
        assert!(out.retried.is_empty() && out.gave_up.is_empty());
        assert_eq!(r.row(fresh).await.state, cw::WakeState::Dropped);
    }

    #[tokio::test]
    async fn db_human_stop_boot_rescue_distinguishes_cancelled_at_max_from_ordinary_c25_limit() {
        use crewd_wake::queue::MAX_ATTEMPTS;
        let r = StopDbRig::new().await;
        let old = r.enqueue_only("old").await;
        r.port.claim(&r.batch(&[old]).await).await.unwrap();
        r.port.human_stop("w-1").await.unwrap();
        let fresh = r.enqueue_only("fresh-crash").await;
        r.port.claim(&r.batch(&[fresh]).await).await.unwrap();
        r.state
            .db
            .call(move |s| {
                s.conn()
                    .execute(
                        "UPDATE wakes SET attempts=?1 WHERE id IN (?2,?3)",
                        rusqlite::params![MAX_ATTEMPTS, old, fresh],
                    )
                    .unwrap()
            })
            .await
            .unwrap();
        let out = r.port.fail_inflight("restart", Ms(2)).await.unwrap();
        assert!(out.retried.is_empty());
        assert_eq!(
            out.gave_up.iter().map(|w| w.id).collect::<Vec<_>>(),
            [fresh]
        );
        assert_eq!(r.row(old).await.last_error.as_deref(), Some("stopped"));
        assert_eq!(r.row(fresh).await.last_error.as_deref(), Some("restart"));
        for id in [old, fresh] {
            assert_eq!(r.row(id).await.state, cw::WakeState::Dropped);
            assert_eq!(r.row(id).await.attempts, MAX_ATTEMPTS);
        }
        let out = r.port.fail_inflight("duplicate boot", Ms(3)).await.unwrap();
        assert!(out.retried.is_empty() && out.gave_up.is_empty());
    }

    #[tokio::test]
    async fn db_human_stop_starting_claimed_batch_failed_settle_is_cancelled_on_boot() {
        use super::super::core::{CollabFact, Input};
        let mut r = StopDbRig::new().await;
        let old = r.enqueue_only("starting-old").await;
        r.core
            .handle(Input::Collab(CollabFact::WakeEnqueued {
                session: "w-1".into(),
            }))
            .await;
        assert!(
            r.core.starting.contains_key("w-1"),
            "Launched input not yet consumed"
        );
        assert_eq!(r.row(old).await.state, cw::WakeState::Inflight);
        r.block_drop(old).await;
        r.stop().await.unwrap();
        assert!(!r.core.live.contains_key("w-1") && !r.core.starting.contains_key("w-1"));
        assert_eq!(r.row(old).await.state, cw::WakeState::Inflight);
        assert_eq!(r.floor().await, old);
        r.unblock_drop().await;
        let r = r.reboot().await;
        assert_eq!(r.row(old).await.state, cw::WakeState::Dropped);
        assert!(r.exec.launches("w-1").is_empty());
    }

    #[tokio::test]
    async fn db_human_stop_explicit_resume_changes_desired_not_old_cancel_intent() {
        use super::super::core::{Cmd, Input, Origin};
        let mut r = StopDbRig::new().await;
        let old = r.wake("old").await;
        r.block_drop(old).await;
        r.stop().await.unwrap();
        r.unblock_drop().await;
        let (tx, rx) = tokio::sync::oneshot::channel();
        r.core
            .handle(Input::Cmd(Cmd::Resume {
                session: "w-1".into(),
                origin: Origin::Human,
                reply: tx,
            }))
            .await;
        rx.await.unwrap().unwrap();
        r.drain().await;
        assert_eq!(
            r.exec.launches("w-1").len(),
            2,
            "explicit human resume is still legal"
        );
        assert_eq!(
            r.port.session("w-1").await.unwrap().unwrap().desired,
            Desired::Running
        );
        assert_eq!(r.floor().await, old);
        let r = r.reboot().await;
        assert_eq!(r.row(old).await.state, cw::WakeState::Dropped);
        assert_eq!(r.row(old).await.attempts, 1);
        assert!(r.core.held.contains("w-1"));
        assert!(r.exec.launches("w-1").is_empty());
    }

    #[tokio::test]
    async fn db_human_stop_newwake_while_stopping_and_stale_exit_do_not_cancel_future_work() {
        use super::super::core::{CollabFact, Input};
        use crewd_exec::session::{ExitInfo, ExitStatusKind, SessionEvent};
        let mut r = StopDbRig::new().await;
        let old = r.wake("old").await;
        let oldgen = r.core.live["w-1"].generator;
        r.exec.lock().hang_on_stop.insert("w-1".into());
        r.stop().await.unwrap();
        let fresh = r.enqueue_only("future").await;
        r.core
            .handle(Input::Collab(CollabFact::WakeEnqueued {
                session: "w-1".into(),
            }))
            .await;
        r.drain().await;
        assert_eq!(
            r.row(fresh).await.state,
            cw::WakeState::Pending,
            "stopping blocks writes, not new intent"
        );
        assert_eq!(r.exec.launches("w-1").len(), 1);
        let info = ExitInfo {
            status: ExitStatusKind::Code(0),
            stopped_by_user: true,
            hit_limit: false,
        };
        r.exec.emit("w-1", SessionEvent::Exited(info.clone()));
        r.drain().await;
        assert_eq!(r.row(old).await.state, cw::WakeState::Dropped);
        assert_eq!(r.row(fresh).await.state, cw::WakeState::Inflight);
        assert_eq!(r.exec.launches("w-1").len(), 2);
        assert_ne!(r.core.live["w-1"].generator, oldgen);
        let before = r.row(fresh).await;
        r.core
            .handle(Input::Exec {
                session: "w-1".into(),
                generator: oldgen,
                event: SessionEvent::Exited(info),
            })
            .await;
        r.core
            .handle(Input::StreamEnded {
                session: "w-1".into(),
                generator: oldgen,
            })
            .await;
        assert_eq!(
            r.row(fresh).await,
            before,
            "duplicate old callbacks cannot touch the new generation"
        );
        assert_eq!(r.floor().await, old);
        assert!(!r.core.live["w-1"].stopping);
    }

    #[tokio::test]
    async fn db_human_stop_later_hibernate_preserves_non_cancelled_new_work_and_attempt_refund() {
        use super::super::core::{Cmd, Input};
        use crewd_exec::session::SessionEvent;
        let mut r = StopDbRig::new().await;
        let old = r.wake("old").await;
        r.stop().await.unwrap();
        let fresh = r.wake("fresh").await;
        r.exec.emit("w-1", SessionEvent::Working(true));
        r.drain().await;
        let (tx, rx) = tokio::sync::oneshot::channel();
        r.core
            .handle(Input::Cmd(Cmd::Hibernate {
                deadline_ms: Some(0),
                reply: tx,
            }))
            .await;
        r.drain().await;
        let report = rx.await.unwrap().unwrap();
        assert!(report.completed);
        assert_eq!(report.interrupted, ["w-1"]);
        assert_eq!(r.floor().await, old, "hibernate is not a new human Stop");
        assert_eq!(
            (r.row(fresh).await.state, r.row(fresh).await.attempts),
            (cw::WakeState::Pending, 0)
        );
        assert_eq!(
            r.port.session("w-1").await.unwrap().unwrap().desired,
            Desired::Running
        );
        let r = r.reboot().await;
        assert_eq!(r.row(old).await.state, cw::WakeState::Dropped);
        assert_eq!(r.row(fresh).await.state, cw::WakeState::Pending);
        assert_eq!(r.floor().await, old);
        assert!(r.core.held.contains("w-1"));
        assert_eq!(
            r.core.hibernated.as_ref().unwrap().reason,
            RestartReason::Hibernate
        );
        assert!(r.exec.launches("w-1").is_empty());
    }

    #[tokio::test]
    async fn db_human_stop_existing_n4_snapshot_preserves_marker_identity_sequence_and_schema() {
        let mut r = StopDbRig::new().await;
        let old = r.wake("old").await;
        r.exec.lock().hang_on_stop.insert("w-1".into());
        r.stop().await.unwrap();
        let db_id = r
            .state
            .db
            .call(|s| crewd_store::meta_get(s.conn(), "dbId").unwrap())
            .await
            .unwrap();
        let path = r.dir.path().join("crewd.db");
        let snapshot = r.dir.path().join("synthetic-snapshot.db");
        let dest = snapshot.clone();
        tokio::task::spawn_blocking(move || crewd_store::backup::backup_db(&path, &dest))
            .await
            .unwrap()
            .unwrap();
        let store = crewd_store::Store::open(&snapshot).unwrap();
        assert_eq!(crewd_store::meta_get(store.conn(), "dbId").unwrap(), db_id);
        assert_eq!(cw::human_stop_floor(store.conn(), "w-1").unwrap(), old);
        let version: i64 = store
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            version,
            crewd_store::schema::SCHEMA_VERSION,
            "no schema migration added"
        );
        assert_eq!(
            cw::by_ids(store.conn(), &[old]).unwrap()[0].state,
            cw::WakeState::Inflight
        );
        let state = Arc::new(AppState::new(crewd_store::Db::spawn(store).unwrap()));
        let port = DbPort::new(state, snapshot);
        let out = port.fail_inflight("snapshot reboot", Ms(1)).await.unwrap();
        assert!(out.retried.is_empty() && out.gave_up.is_empty());
        assert_eq!(
            port.list_wakes(Some("w-1")).await.unwrap()[0].state,
            crewd_wake::WakeState::Dropped
        );
        let fresh = port
            .enqueue(new_wake(
                "w-1",
                WakeKind::System,
                source::system("fresh"),
                Priority::System,
                SYSTEM,
            ))
            .await
            .unwrap()
            .unwrap();
        assert!(fresh > old, "snapshot carries sqlite_sequence too");
        assert_eq!(
            r.row(old).await.state,
            cw::WakeState::Inflight,
            "snapshot recovery does not modify source"
        );
    }
    // Minimal actual Core debt observer: never attempts a new owner after the SQL fault.
    // Original broad case stays unchanged; this keeps its first debt oracle reachable under K1.
    #[tokio::test]
    async fn s42_actual_core_known_delivered_sql_fault_keeps_debt_through_normal_cleanup() {
        use crewd_exec::{
            delivery::Subject,
            session::{BRIEF_DELIVERY_ID, DeliveryOutcome, SessionEvent},
        };
        let mut r = StopDbRig::new().await;
        let collected=s42_family6_catch(async {
            r.state.collab(|tx|collab::cursors::pin_to_end(tx,"w-1","c-1/lobby")).await?;
            let input=post_as(&r.state,collab::Sender::Human,"c-1/lobby","S42_K1_ORIGINAL_ACTUAL_MESSAGE",&[]).await;
            let id=r.port.enqueue(new_wake("w-1",WakeKind::Message,source::message(&input.id),Priority::Human,HUMAN)).await?.unwrap();
            r.core.handle(super::super::core::Input::Collab(super::super::core::CollabFact::WakeEnqueued{session:"w-1".into()})).await;
            r.drain().await;
            let original=r.core.live["w-1"].inflight.as_ref().expect("original real body before fault").batch.clone();
            let (pending,generation)=match original.lease.as_ref().unwrap() {
                BatchLease::CausalDatabase{claimed,..}=>(claimed.pending.clone(),claimed.pending.generation()),
                other=>panic!("actual shared causal receipt required: {other:?}"),
            };
            let capture=s42_boot_meta(&r).await;
            let before=r.row(id).await;let cursor_before=r.port.cursor("w-1","c-1/lobby").await?;
            r.state.db.call(move|s|s.conn().execute_batch(&format!("CREATE TRIGGER s42_k1_small_delivered_abort BEFORE UPDATE OF state ON wakes WHEN OLD.id={id} AND NEW.state='delivered' BEGIN SELECT RAISE(ABORT,'S42 K1 small exact Delivered SQL abort'); END;"))).await??;
            r.exec.emit("w-1",SessionEvent::Delivery{id:BRIEF_DELIVERY_ID,subject:Subject::Brief,outcome:DeliveryOutcome::Delivered});
            let actual_ticket=r.state.causal_fence.success(&pending,generation,BRIEF_DELIVERY_ID)?;
            let before_ack=s42_boot_meta(&r).await;
            r.drain().await;
            let debt_after_error=r.core.terminal_sessions();
            let after_ack=s42_boot_meta(&r).await;let failed_row=r.row(id).await;
            let failed_cursor=r.port.cursor("w-1","c-1/lobby").await?;
            // Save the result instead of unwrap. Missing debt under K1 must reach final oracle.
            let repeat_fault=r.core.retry_terminal("w-1").await.map_err(|e|e.to_string());
            let debt_after_repeat=r.core.terminal_sessions();
            let repeat_row=r.row(id).await;
            r.state.db.call(|s|s.conn().execute_batch("DROP TRIGGER s42_k1_small_delivered_abort")).await??;
            let repaired=r.core.retry_terminal("w-1").await.map_err(|e|e.to_string());
            let debt_after_repair=r.core.terminal_sessions();let committed_row=r.row(id).await;
            let committed_cursor=r.port.cursor("w-1","c-1/lobby").await?;
            let basis=s42_boot_state(&r).await;let after_repair=s42_boot_meta(&r).await;
            let mut exact=original.clone();
            if let Some(BatchLease::CausalDatabase{observed,..})=exact.lease.as_mut(){*observed=Some(Box::new(actual_ticket));}
            // Repeated exact original ack has no new owner / binding / launch helper.
            let duplicate=r.port.ack(&exact,Ack::Delivered{riders:Riders::Release}).await.map_err(|e|e.to_string());
            let after_duplicate=s42_boot_meta(&r).await;let duplicate_row=r.row(id).await;
            Ok::<_,anyhow::Error>((input,id,original.ids(),capture,before,cursor_before,before_ack,debt_after_error,
                after_ack,failed_row,failed_cursor,repeat_fault,debt_after_repeat,repeat_row,repaired,
                debt_after_repair,committed_row,committed_cursor,basis,after_repair,duplicate,after_duplicate,duplicate_row))
        }).await;
        let observed = s42_family6_finish(r, collected)
            .await
            .expect("normal owned Core and Db shutdown before debt oracles");
        let (
            input,
            id,
            ids,
            _capture,
            before,
            cursor_before,
            before_ack,
            debt_after_error,
            after_ack,
            failed_row,
            failed_cursor,
            repeat_fault,
            debt_after_repeat,
            repeat_row,
            repaired,
            debt_after_repair,
            committed_row,
            committed_cursor,
            basis,
            after_repair,
            duplicate,
            after_duplicate,
            duplicate_row,
        ) = observed;
        assert_eq!(
            debt_after_error,
            vec!["w-1"],
            "S42 actual known Delivered SQL fault must retain exact Core debt before any new owner; after normal cleanup"
        );
        assert_eq!(ids, vec![id]);
        assert_eq!(before.source, source::message(&input.id));
        assert_eq!(before.state, cw::WakeState::Inflight);
        assert_eq!(before_ack, after_ack);
        assert_eq!(failed_row, before);
        assert_eq!(failed_cursor, cursor_before);
        assert!(
            repeat_fault
                .as_ref()
                .err()
                .is_some_and(|e| e.contains("S42 K1 small exact Delivered SQL abort")),
            "real exact retry must still hit original SQL fault: {repeat_fault:?}"
        );
        assert_eq!(debt_after_repeat, vec!["w-1"]);
        assert_eq!(repeat_row, before);
        assert!(repaired.is_ok());
        assert!(debt_after_repair.is_empty());
        assert_eq!(committed_row.state, cw::WakeState::Delivered);
        assert_eq!(committed_row.attempts, before.attempts);
        assert_eq!(committed_cursor, input.seq);
        assert!(!basis["basis"].as_array().unwrap().is_empty());
        assert!(duplicate.is_ok());
        assert_eq!(after_repair, after_duplicate);
        assert_eq!(duplicate_row, committed_row);
    }
}

#[cfg(test)]
mod stop_provenance_tests {
    use super::fixture::{post_as, seeded};
    use super::*;
    use crewd_wake::plan::MessageProvenance;

    fn marker(message: &Message) -> (String, String) {
        (
            format!("message.provenance.v1.{}", message.id),
            serde_json::json!({
                "version": 1, "kind": "stop_relay", "messageId": message.id,
                "seq": message.seq, "sourceSession": message.sender_session_id,
            })
            .to_string(),
        )
    }

    #[tokio::test]
    async fn malformed_stop_provenance_fails_message_and_rolls_back_unread_lazy_cursor() {
        let (state, port, _dir) = seeded().await;
        let message = post_as(
            &state,
            collab::Sender::Session("w-1".into()),
            "c-1/lobby",
            "合成问题？",
            &["cap-1"],
        )
        .await;
        let human = state
            .collab(|tx| collab::channels::ensure_human_channel(tx, "c-1"))
            .await
            .unwrap()
            .id;
        let human_for_delete = human.clone();
        let (key, _) = marker(&message);
        state
            .db
            .call(move |s| {
                s.conn().execute(
                    "DELETE FROM read_cursors WHERE reader='cap-1' AND channel_id=?1",
                    [&human_for_delete],
                )?;
                crewd_store::meta_set(s.conn(), &key, "{broken")
            })
            .await
            .unwrap()
            .unwrap();
        let read_error = port
            .message(&MessageRef {
                channel: message.channel_id,
                seq: message.seq,
            })
            .await
            .unwrap_err();
        assert!(matches!(
            read_error.downcast_ref::<collab::CollabError>(),
            Some(collab::CollabError::Store(
                crewd_store::StoreError::Invalid(_)
            ))
        ));
        let unread_error = port.unread("cap-1").await.unwrap_err();
        assert!(matches!(
            unread_error.downcast_ref::<collab::CollabError>(),
            Some(collab::CollabError::Store(
                crewd_store::StoreError::Invalid(_)
            ))
        ));
        // unread_for DID visit every channel and lazily insert this cursor. Mapping must fail
        // inside that same write transaction, rolling the insert back instead of committing it.
        let cursor = state
            .db
            .call(move |s| collab::cursors::cursor(s.conn(), "cap-1", &human))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cursor, None);
        let db = state.db.clone();
        tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn retained_foreign_dm_provenance_is_read_after_source_crew_purge() {
        let (state, port, _dir) = seeded().await;
        let message = state
            .collab(|tx| {
                let (dm, _) = collab::channels::ensure_dm(
                    tx,
                    "c-1",
                    &crewd_proto::MemberRef::session("w-1"),
                    &crewd_proto::MemberRef::session("w-9"),
                )?;
                let posted = collab::messages::post(
                    tx,
                    collab::Sender::Session("w-9".into()),
                    collab::messages::NewPost {
                        channel_id: dm.id,
                        text: "合成保留历史？".into(),
                        ..Default::default()
                    },
                )?;
                // Storage fixture models an existing valid durable marker; this is not a claim
                // that worker Stop currently relays into a foreign DM (S67 step2 is deferred).
                let (key, value) = marker(&posted.message);
                crewd_store::meta_set(tx.conn(), &key, &value)?;
                Ok(posted.message)
            })
            .await
            .unwrap();
        state
            .collab(|tx| {
                collab::crews::archive(tx, "c-9")?;
                collab::crews::purge(tx, "c-9")
            })
            .await
            .unwrap();
        assert!(
            state
                .db
                .call(|s| crewd_store::sessions::get(s.conn(), "w-9"))
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        let facts = port
            .message(&MessageRef {
                channel: message.channel_id.clone(),
                seq: message.seq,
            })
            .await
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(facts.provenance, MessageProvenance::StopRelay);
        assert!(matches!(facts.sender, Sender::Session { id, .. } if id == "w-9"));
        let unread = port.unread("w-1").await.unwrap();
        assert!(
            unread
                .iter()
                .any(|f| f.id == message.id && f.provenance == MessageProvenance::StopRelay)
        );
        let db = state.db.clone();
        tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .unwrap();
    }
}

// Production Task + real fence + real DB post. Controlled owned child only; not actor/serve.
#[cfg(test)]
mod s42_task_fence_db_windows {
    use super::fixture::{post_as, seeded};
    use super::*;
    use crewd_exec::{
        driver::{
            ClaudeStart, DeliverySuccessObserver, DriverConfig, SessionHandle, UntrustedContext,
            start_claude,
        },
        launch::ClaudeSpec,
        registry::ProcessRegistry,
        session::{DeliveryOutcome, ExitStatusKind, SessionEvent, sanitize_text},
    };
    use serde_json::{Value, json};
    use std::{
        any::Any,
        collections::BTreeMap,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::{
            Condvar, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };
    use tokio::{sync::mpsc, time::timeout};
    const LIMIT: Duration = Duration::from_secs(5);

    struct EmissionGate {
        released: Mutex<bool>,
        release: Condvar,
        expired: AtomicBool,
        inner: Arc<dyn DeliverySuccessObserver>,
        entered: mpsc::UnboundedSender<u64>,
    }
    impl EmissionGate {
        fn release(&self) {
            *self.released.lock().unwrap() = true;
            self.release.notify_all();
        }
    }
    impl DeliverySuccessObserver for EmissionGate {
        fn emitted(&self, id: u64) {
            // Only production Task invokes this. Real fence publishes before fixture pause.
            self.inner.emitted(id);
            // Hand the Tokio worker core back before waking the receiver and blocking.
            // The true fence publication remains above and owns no lock across this wait.
            tokio::task::block_in_place(|| {
                let _ = self.entered.send(id);
                let released = self.released.lock().unwrap();
                let (released, _) = self
                    .release
                    .wait_timeout_while(released, Duration::from_secs(3), |released| !*released)
                    .unwrap();
                if !*released {
                    self.expired.store(true, Ordering::SeqCst);
                }
            });
        }
    }
    struct TaskOwner {
        handle: SessionHandle,
        events: mpsc::UnboundedReceiver<SessionEvent>,
        gate: Arc<EmissionGate>,
        registry: Arc<ProcessRegistry>,
        runtime: PathBuf,
        agent_id: String,
        normal: bool,
    }
    impl Drop for TaskOwner {
        fn drop(&mut self) {
            self.gate.release();
            if !self.normal {
                eprintln!("S42 Task fixture fallback stop; not normal accepted cleanup");
                let _ = self.handle.stop();
            }
        }
    }
    struct ChildProtocol {
        dir: PathBuf,
        marker: Vec<u8>,
        frame: u64,
    }
    impl ChildProtocol {
        fn release_input(&self) {
            // The child polls existence before parsing the command. Publish only
            // complete bytes: create/truncate of the visible file is not atomic.
            let pending = self.dir.join("release-input.pending");
            std::fs::write(&pending, "0").unwrap();
            std::fs::rename(pending, self.dir.join("release-input")).unwrap();
        }
        fn diagnostic(&self, name: &str) -> String {
            use std::io::Read;
            const CAP: u64 = 64 * 1024;
            let file = match std::fs::File::open(self.dir.join(name)) {
                Ok(file) => file,
                Err(error) => return format!("read {name}: {error}"),
            };
            let mut bytes = Vec::new();
            if let Err(error) = file.take(CAP + 1).read_to_end(&mut bytes) {
                return format!("read {name}: {error}");
            }
            let truncated = bytes.len() > CAP as usize;
            bytes.truncate(CAP as usize);
            let mut text = String::from_utf8_lossy(&bytes).into_owned();
            if truncated {
                text.push_str("\n[diagnostic truncated at 64KiB]");
            }
            text
        }
        async fn receipt(&self, name: &str) {
            timeout(LIMIT, async {
                loop {
                    if self.dir.join("fixture.error").exists() {
                        let error = self.diagnostic("fixture.error");
                        let protocol = self.diagnostic("protocol.log");
                        panic!(
                            "fixture error: {name}; actual error={error:?}; protocol={protocol:?}"
                        );
                    }
                    if self.dir.join(name).exists() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .expect("owned child protocol receipt");
        }
        async fn screen(&mut self, text: &str) {
            let n = self.frame;
            self.frame += 1;
            let text = text.replace('\n', "\r\n");
            let bytes = format!(
                "\x1b[2J\x1b[HClaude Code v9.9.9\r\n\r\n────────────────────\r\n❯ {text}\x1b[7m \x1b[27m\r\n────────────────────\r\n\x1b[2m ? for shortcuts\x1b[22m"
            );
            std::fs::write(self.dir.join(format!("screen-{n}.bin")), bytes).unwrap();
            std::fs::write(self.dir.join(format!("screen-{n}.request")), []).unwrap();
            self.receipt(&format!("screen-{n}.written")).await;
            // Task alone reads and drains. File receipt is output-written, not parser proof.
        }
        async fn capture(&self, wanted: &[u8]) -> Vec<u8> {
            timeout(LIMIT, async {
                loop {
                    let got = std::fs::read(self.dir.join("capture.bin")).unwrap_or_default();
                    if got == wanted {
                        return got;
                    }
                    assert!(
                        wanted.starts_with(&got),
                        "unexpected actual child capture: {got:?}"
                    );
                    assert!(!self.dir.join("fixture.error").exists());
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .expect("exact real PTY body / submit capture")
        }
    }
    fn quote(path: &Path) -> String {
        format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
    }
    async fn start_owned(
        state: &AppState,
        port: &DbPort,
        dir: &Path,
        generation: u64,
    ) -> (TaskOwner, ChildProtocol, mpsc::UnboundedReceiver<u64>) {
        let child_dir = dir.join("physical-child");
        std::fs::create_dir(&child_dir).unwrap();
        std::fs::set_permissions(&child_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let marker = format!("S42_CLOSE_{}", uuid::Uuid::new_v4().simple()).into_bytes();
        std::fs::write(child_dir.join("marker.bin"), &marker).unwrap();
        let debug = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let fake = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .map(|p| p.join("debug/fake_claude"))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| debug.join("fake_claude"));
        assert!(
            fake.is_file(),
            "root must build fresh fake_claude: {fake:?}"
        );
        let wrapper = dir.join("owned-fake-guarded.sh");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexec {} --guarded-write-fixture {} \"$@\"\n",
                quote(&fake),
                quote(&child_dir)
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let home = dir.join("physical-home");
        for name in ["", "cache", "config", "tmp"] {
            std::fs::create_dir_all(home.join(name)).unwrap();
        }
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = dir.join("physical-runtime");
        let registry =
            Arc::new(ProcessRegistry::load(&dir.join("physical-registry.json")).unwrap());
        let (entered, ready) = mpsc::unbounded_channel();
        let gate = Arc::new(EmissionGate {
            released: Mutex::new(false),
            release: Condvar::new(),
            expired: AtomicBool::new(false),
            inner: state.causal_fence.observer("w-1", generation),
            entered,
        });
        let mut config = DriverConfig::new(runtime.clone());
        config.registry = Some(registry.clone());
        config.claude_projects_dir = None;
        config.delivery_observer = Some(gate.clone());
        let env: BTreeMap<String, String> = [
            ("HOME", home.clone()),
            ("XDG_CACHE_HOME", home.join("cache")),
            ("CLAUDE_CONFIG_DIR", home.join("config")),
            ("TMPDIR", home.join("tmp")),
        ]
        .into_iter()
        .map(|(k, p)| (k.into(), p.to_str().unwrap().into()))
        .collect();
        let (handle, events) = start_claude(
            ClaudeStart {
                session_id: "w-1".into(),
                program: wrapper,
                child_path: "/usr/bin:/bin".into(),
                spec: ClaudeSpec {
                    workdir: dir.into(),
                    additional_env: env,
                    ..Default::default()
                },
                ledger: port.ledger("w-1"),
            },
            config,
        )
        .await
        .unwrap();
        let mut task = TaskOwner {
            handle,
            events,
            gate,
            registry,
            runtime,
            agent_id: String::new(),
            normal: false,
        };
        let spawned = timeout(LIMIT, task.events.recv()).await.unwrap().unwrap();
        let SessionEvent::Spawned {
            pid,
            agent_session_id,
            resumed,
        } = spawned
        else {
            panic!("actual Spawned required")
        };
        assert!(pid > 0 && !resumed);
        task.agent_id = agent_session_id;
        let protocol = ChildProtocol {
            dir: child_dir,
            marker,
            frame: 0,
        };
        protocol.receipt("slave-ready").await;
        let ready_text = std::fs::read_to_string(protocol.dir.join("slave-ready")).unwrap();
        assert!(
            ready_text.contains("raw_no_echo=1") && ready_text.contains("stdin_reader_started=0")
        );
        (task, protocol, ready)
    }
    async fn parser_row(task: &TaskOwner, wanted: &str) {
        timeout(LIMIT, async {
            loop {
                if task
                    .handle
                    .screen_text(25)
                    .await
                    .unwrap()
                    .contains(&format!("❯ {wanted}"))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("real Task parsed requested screen");
    }
    fn drain_delivered(task: &mut TaskOwner, id: u64, retained: &mut Vec<u64>) -> usize {
        let mut delivered = 0;
        while let Ok(event) = task.events.try_recv() {
            eprintln!("S42 observed pre-release event: {event:?}");
            if let SessionEvent::Delivery {
                id: actual,
                outcome: DeliveryOutcome::Delivered,
                ..
            } = event
            {
                retained.push(actual);
                if actual == id {
                    delivered += 1;
                }
            }
        }
        delivered
    }
    #[derive(Debug, PartialEq)]
    struct DbSnapshot {
        wake: WakeRow,
        cursor: i64,
        meta: Vec<(String, String)>,
    }
    async fn db_snapshot(state: &AppState, wake: i64) -> DbSnapshot {
        state.db.call(move |s| {
            let wake = cw::by_ids(s.conn(), &[wake]).unwrap().remove(0);
            let cursor = collab::cursors::cursor(s.conn(), "w-1", "c-1/lobby").unwrap().unwrap_or(0);
            let mut q = s.conn().prepare("SELECT key,value FROM meta WHERE substr(key,1,7)='causal.' OR substr(key,1,22)='cursor.reservation.v1.' ORDER BY key").unwrap();
            let meta = q.query_map([], |r| Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            DbSnapshot { wake, cursor, meta }
        }).await.unwrap()
    }
    #[derive(Debug, PartialEq)]
    struct PostSnapshot {
        actual: Message,
        stamp: String,
        resolved: Option<Value>,
    }
    async fn post_snapshot(state: &AppState, message: &Message) -> PostSnapshot {
        let id = message.id.clone();
        state.db.call(move |s| {
            let actual = collab::messages::get(s.conn(), &id).unwrap();
            let stamp = crewd_store::meta_get(s.conn(), &format!("causal.message.v1.{id}")).unwrap().unwrap();
            let resolved = match collab::causal::read_message(s.conn(), &actual).unwrap() {
                collab::causal::MessageCause::Resolved(f) => Some(serde_json::to_value(f).unwrap()),
                collab::causal::MessageCause::Unresolved { .. } => None,
            };
            eprintln!("S42 actual post snapshot: message={actual:?} stamp={stamp} resolved={resolved:?}");
            PostSnapshot { actual, stamp, resolved }
        }).await.unwrap()
    }
    async fn output_value(state: &AppState, key: &str) -> Value {
        let key = format!("causal.output.v1.{key}");
        state
            .db
            .call(move |s| {
                serde_json::from_str::<Value>(
                    &crewd_store::meta_get(s.conn(), &key).unwrap().unwrap(),
                )
                .unwrap()["value"]
                    .clone()
            })
            .await
            .unwrap()
    }
    async fn catch<F: std::future::Future>(future: F) -> Result<F::Output, Box<dyn Any + Send>> {
        let mut future = Box::pin(future);
        std::future::poll_fn(move |cx| {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                future.as_mut().poll(cx)
            })) {
                Ok(std::task::Poll::Ready(v)) => std::task::Poll::Ready(Ok(v)),
                Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
                Err(e) => std::task::Poll::Ready(Err(e)),
            }
        })
        .await
    }
    async fn close_owned(task: &mut TaskOwner, protocol: &ChildProtocol) -> (Vec<u8>, String) {
        task.gate.release();
        task.handle.raw(protocol.marker.clone()).unwrap();
        protocol.receipt("capture.closed").await;
        let capture = std::fs::read(protocol.dir.join("capture.bin")).unwrap();
        let receipt = std::fs::read_to_string(protocol.dir.join("capture.closed")).unwrap();
        let log = std::fs::read_to_string(protocol.dir.join("protocol.log")).unwrap();
        eprintln!("S42 full protocol:\n{log}\nclosed: {receipt}\nactual capture: {capture:?}");
        let mut exit = None;
        timeout(LIMIT, async {
            while let Some(event) = task.events.recv().await {
                eprintln!("S42 cleanup event: {event:?}");
                if let SessionEvent::Exited(info) = event {
                    exit = Some(info);
                }
            }
        })
        .await
        .expect("actual Task Exited and event stream EOF");
        assert!(
            matches!(exit, Some(info) if info.status == ExitStatusKind::Code(0) && !info.stopped_by_user)
        );
        assert!(task.handle.is_closed());
        assert!(
            !task
                .runtime
                .join(format!("claude-{}", task.agent_id))
                .exists()
        );
        assert!(task.registry.entries().is_empty());
        task.normal = true;
        (capture, receipt)
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn s42_actual_task_real_fence_three_posts_resolve_only_emitted_success() {
        let (state, port, dir) = seeded().await;
        let root = post_as(
            &state,
            collab::Sender::Human,
            "c-1/lobby",
            "S42_PHYSICAL_EXACT_HUMAN",
            &[],
        )
        .await;
        let wake = port
            .enqueue(NewWake {
                session: "w-1".into(),
                kind: WakeKind::Message,
                source: cw::source::message(&root.id),
                priority: Priority::Human,
                not_before: Ms(0),
                note: None,
                origin: HUMAN.into(),
            })
            .await
            .unwrap()
            .unwrap();
        let wakes = port.pending("w-1").await.unwrap();
        let rendered = port.render_wake("w-1", &wakes).await.unwrap();
        let body = sanitize_text(&rendered.text);
        let mut batch = Batch {
            session: "w-1".into(),
            wakes,
            cursor_targets: rendered.cursor_targets,
            prepared: rendered.prepared,
            ..Default::default()
        };
        let generation = 1;
        let cr::Decision::Applied(lease) = port.claim_reserved(&batch, generation).await.unwrap()
        else {
            panic!("real causal claim required")
        };
        batch.lease = Some(lease);
        let pending = match batch.lease.as_ref().unwrap() {
            BatchLease::CausalDatabase { claimed, .. } => claimed.pending.clone(),
            _ => panic!("real causal output required"),
        };
        let before_output = output_value(&state, pending.stable_key()).await;
        let (mut task, mut protocol, mut emitted) =
            start_owned(&state, &port, dir.path(), generation).await;
        let observation = catch(async {
            let binding = state.causal_fence.binder(&pending, generation).unwrap();
            let id = task
                .handle
                .send_bound(
                    rendered.text.clone(),
                    UntrustedContext::default(),
                    binding.as_ref(),
                )
                .unwrap();
            protocol.release_input();
            protocol.receipt("filler-drained").await;
            let queued_capture = std::fs::read(protocol.dir.join("capture.bin")).unwrap();
            // Preserve every actual success, including a counterfactual premature broadcast.
            let mut retained_delivered = Vec::new();
            let queued_events = drain_delivered(&mut task, id, &mut retained_delivered);
            let queued_success = state.causal_fence.success(&pending, generation, id).is_ok();
            let q = post_as(
                &state,
                collab::Sender::Session("w-1".into()),
                "c-1/lobby",
                "S42_PHYSICAL_QUEUED",
                &[],
            )
            .await;
            let q_before = post_snapshot(&state, &q).await;
            protocol.screen("").await;
            let body_capture = protocol.capture(body.as_bytes()).await;
            let body_events = drain_delivered(&mut task, id, &mut retained_delivered);
            let body_success = state.causal_fence.success(&pending, generation, id).is_ok();
            let b = post_as(
                &state,
                collab::Sender::Session("w-1".into()),
                "c-1/lobby",
                "S42_PHYSICAL_BODY_ONLY",
                &[],
            )
            .await;
            let b_before = post_snapshot(&state, &b).await;
            protocol.screen(&body).await;
            parser_row(&task, body.lines().next().unwrap_or("")).await;
            let mut submitted = body.as_bytes().to_vec();
            submitted.push(b' ');
            protocol.capture(&submitted).await;
            protocol.screen(&format!("{body} ")).await;
            submitted.push(0x7f);
            protocol.capture(&submitted).await;
            protocol.screen(&body).await;
            submitted.push(b'\r');
            protocol.capture(&submitted).await;
            protocol.screen("").await;
            let emission = timeout(LIMIT, emitted.recv()).await.unwrap().unwrap();
            let premature_broadcast = drain_delivered(&mut task, id, &mut retained_delivered);
            let e = post_as(
                &state,
                collab::Sender::Session("w-1".into()),
                "c-1/lobby",
                "S42_PHYSICAL_EMITTED_PRE_ACK",
                &[],
            )
            .await;
            let e_before = post_snapshot(&state, &e).await;
            let before_ack = db_snapshot(&state, wake).await;
            let success = state
                .causal_fence
                .success(&pending, generation, id)
                .unwrap();
            task.gate.release();
            // Reuse the real already-received success. A wrong producer ordering must
            // reach the post-cleanup premature-broadcast oracle, not wait for a second event.
            let delivered = if let Some(actual) = retained_delivered.first().copied() {
                actual
            } else {
                timeout(LIMIT, async {
                    while let Some(event) = task.events.recv().await {
                        eprintln!("S42 post-release actual event: {event:?}");
                        if let SessionEvent::Delivery {
                            id: actual,
                            outcome: DeliveryOutcome::Delivered,
                            ..
                        } = event
                        {
                            retained_delivered.push(actual);
                            return actual;
                        }
                    }
                    panic!("actual Delivered missing")
                })
                .await
                .unwrap()
            };
            if let Some(BatchLease::CausalDatabase { observed, .. }) = batch.lease.as_mut() {
                *observed = Some(Box::new(success.clone()));
            }
            let ack = port
                .ack(
                    &batch,
                    Ack::Delivered {
                        riders: Riders::Release,
                    },
                )
                .await
                .unwrap();
            let after_ack = db_snapshot(&state, wake).await;
            let q_after = post_snapshot(&state, &q).await;
            let b_after = post_snapshot(&state, &b).await;
            let e_after = post_snapshot(&state, &e).await;
            let after_output = output_value(&state, pending.stable_key()).await;
            let duplicate = port
                .ack(
                    &batch,
                    Ack::Delivered {
                        riders: Riders::Release,
                    },
                )
                .await
                .unwrap();
            let after_duplicate = db_snapshot(&state, wake).await;
            let ticket_same = state
                .causal_fence
                .success(&pending, generation, id)
                .unwrap()
                == success;
            (
                id,
                emission,
                delivered,
                retained_delivered,
                queued_capture,
                queued_events,
                queued_success,
                body_capture,
                body_events,
                body_success,
                premature_broadcast,
                q_before,
                q_after,
                b_before,
                b_after,
                e_before,
                e_after,
                before_ack,
                after_ack,
                after_duplicate,
                after_output,
                ack.stale,
                duplicate.stale,
                ticket_same,
            )
        })
        .await;
        let cleanup = catch(close_owned(&mut task, &protocol)).await;
        if cleanup.is_err() {
            task.gate.release();
            let _ = task.handle.stop();
            let stopped = timeout(LIMIT, async {
                while let Some(event) = task.events.recv().await {
                    eprintln!("S42 fallback stop drain: {event:?}");
                }
            })
            .await;
            eprintln!(
                "S42 failed normal cleanup, fallback owned stop EOF={}",
                stopped.is_ok()
            );
        }
        let expired = task.gate.expired.load(Ordering::SeqCst);
        drop(task);
        drop(emitted);
        drop(port);
        let db = state.db.clone();
        drop(state);
        tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .unwrap();
        let (capture, receipt) = match cleanup {
            Ok(v) => v,
            Err(e) => {
                eprintln!("S42 normal cleanup failed; no accepted semantic tail");
                if let Err(original) = observation {
                    std::panic::resume_unwind(original);
                }
                std::panic::resume_unwind(e)
            }
        };
        let (
            id,
            emission,
            delivered,
            retained_delivered,
            queued_capture,
            queued_events,
            queued_success,
            body_capture,
            body_events,
            body_success,
            premature_broadcast,
            q_before,
            q_after,
            b_before,
            b_after,
            e_before,
            e_after,
            before_ack,
            after_ack,
            after_duplicate,
            after_output,
            stale,
            duplicate_stale,
            ticket_same,
        ) = match observation {
            Ok(v) => v,
            Err(e) => std::panic::resume_unwind(e),
        };
        // Main semantic oracles only after real child/Task EOF, files/registry and DB cleanup.
        assert!(!expired, "test gate deadline is not producer-order proof");
        assert_eq!(queued_capture, b"");
        assert_eq!((queued_events, queued_success), (0, false));
        assert_eq!(body_capture, body.as_bytes());
        assert_eq!((body_events, body_success), (0, false));
        assert_eq!(
            premature_broadcast, 0,
            "real fence publication must precede broadcast; gate still held"
        );
        assert_eq!((emission, delivered), (id, id));
        assert_eq!(
            retained_delivered,
            vec![id],
            "exact one original actual success event, including retained early arrivals"
        );
        assert_eq!(
            q_before, q_after,
            "queued child history must never retroactively activate"
        );
        assert_eq!(
            b_before, b_after,
            "body-only child history must never retroactively activate"
        );
        for saved in [&q_before, &b_before] {
            assert!(saved.resolved.is_none());
            let stamp: Value = serde_json::from_str(&saved.stamp).unwrap();
            assert_eq!(stamp["value"]["candidate"], pending.stable_key());
            assert_eq!(stamp["value"]["resolution"]["kind"], "unresolved");
            assert_eq!(stamp["value"]["resolution"]["previous"], json!([]));
        }
        let eligible: Value = serde_json::from_str(&e_before.stamp).unwrap();
        assert!(e_before.resolved.is_none());
        assert_eq!(eligible["value"]["candidate"], pending.stable_key());
        assert_eq!(eligible["value"]["resolution"]["kind"], "eligible");
        assert_eq!(
            eligible["value"]["resolution"]["output"],
            pending.stable_key()
        );
        assert_eq!(e_before.actual, e_after.actual);
        assert_eq!(
            e_before.stamp, e_after.stamp,
            "eligible history immutable, receipt state resolves read"
        );
        let expected = json!([{"key":{"root":{"kind":"message","id":root.id,"seq":root.seq},"reset":{"kind":"initial"}},"hop":1}]);
        assert_eq!(e_after.resolved, Some(expected.clone()));
        assert_eq!(eligible["value"]["resolution"]["frontier"], expected);
        assert_eq!(
            eligible["value"]["resolution"]["success"]["receipt"], before_output["receipt"],
            "exact original receipt/generation/capture, not any success"
        );
        assert_eq!(before_ack.wake.state, cw::WakeState::Inflight);
        assert_eq!(
            before_ack.cursor, 0,
            "success publication and actual post do not acknowledge DB prefix"
        );
        assert_eq!(after_ack.wake.state, cw::WakeState::Delivered);
        assert_eq!((before_ack.wake.attempts, after_ack.wake.attempts), (1, 1));
        assert_eq!(
            after_ack.cursor, root.seq,
            "later posts not included in original captured prefix"
        );
        assert_eq!(
            after_ack, after_duplicate,
            "same original same-fate duplicate is inert"
        );
        assert!(!stale && !duplicate_stale && ticket_same);
        assert_eq!(before_output["status"], "pending");
        assert_eq!(after_output["status"], "committed");
        assert_eq!(before_output["receipt"], after_output["receipt"]);
        assert_eq!(
            after_output["success"],
            eligible["value"]["resolution"]["success"]
        );
        assert_eq!(
            before_output["receipt"]["capture"]["tuples"],
            json!([{"id":root.id,"channel":root.channel_id,"seq":root.seq,"kind":root.sender_kind,"sender":root.sender_session_id}])
        );
        let mut expected_capture = body.as_bytes().to_vec();
        expected_capture.extend_from_slice(b" \x7f\r");
        assert_eq!(
            capture, expected_capture,
            "exact complete actual rendered body and one submit"
        );
        assert!(receipt.lines().any(|line| line == "trailing_bytes=0"));
    }

    // Supplemental actual Task/fence/DbPort callers. Reuse the accepted owned PTY protocol.
    async fn gdl_claim_human(
        state: &AppState,
        port: &DbPort,
        text: &str,
    ) -> (Message, Batch, String, collab::causal::PendingOutput) {
        let m = post_as(state, collab::Sender::Human, "c-1/lobby", text, &[]).await;
        port.enqueue(NewWake {
            session: "w-1".into(),
            kind: WakeKind::Message,
            source: cw::source::message(&m.id),
            priority: Priority::Human,
            not_before: Ms(0),
            note: None,
            origin: HUMAN.into(),
        })
        .await
        .unwrap()
        .unwrap();
        let wakes = port.pending("w-1").await.unwrap();
        let rendered = port.render_wake("w-1", &wakes).await.unwrap();
        let mut batch = Batch {
            session: "w-1".into(),
            wakes,
            cursor_targets: rendered.cursor_targets,
            prepared: rendered.prepared,
            ..Default::default()
        };
        let cr::Decision::Applied(lease) = port.claim_reserved(&batch, 1).await.unwrap() else {
            panic!("gdl real actual claim required")
        };
        batch.lease = Some(lease);
        let pending = match batch.lease.as_ref().unwrap() {
            BatchLease::CausalDatabase { claimed, .. } => claimed.pending.clone(),
            _ => panic!("gdl actual causal receipt required"),
        };
        (m, batch, rendered.text, pending)
    }
    async fn gdl_physical_emit(
        task: &TaskOwner,
        protocol: &mut ChildProtocol,
        emitted: &mut mpsc::UnboundedReceiver<u64>,
        text: &str,
        prefix: &[u8],
    ) -> (u64, Vec<u8>) {
        let body = sanitize_text(text);
        protocol.screen("").await;
        let mut capture = prefix.to_vec();
        capture.extend_from_slice(body.as_bytes());
        protocol.capture(&capture).await;
        protocol.screen(&body).await;
        parser_row(task, body.lines().next().unwrap_or("")).await;
        capture.push(b' ');
        protocol.capture(&capture).await;
        protocol.screen(&format!("{body} ")).await;
        capture.push(0x7f);
        protocol.capture(&capture).await;
        protocol.screen(&body).await;
        capture.push(b'\r');
        protocol.capture(&capture).await;
        protocol.screen("").await;
        let id = timeout(LIMIT, emitted.recv()).await.unwrap().unwrap();
        (id, capture)
    }
    async fn gdl_receive_success(task: &mut TaskOwner, retained: &mut Vec<u64>) -> u64 {
        if let Some(id) = retained.first().copied() {
            return id;
        }
        timeout(LIMIT, async {
            while let Some(event) = task.events.recv().await {
                eprintln!("GDL actual event: {event:?}");
                if let SessionEvent::Delivery {
                    id,
                    outcome: DeliveryOutcome::Delivered,
                    ..
                } = event
                {
                    retained.push(id);
                    return id;
                }
            }
            panic!("gdl actual Delivered missing")
        })
        .await
        .unwrap()
    }
    fn gdl_witness(batch: &mut Batch, success: collab::causal::ObservedSuccess) {
        if let Some(BatchLease::CausalDatabase { observed, .. }) = batch.lease.as_mut() {
            *observed = Some(Box::new(success));
        } else {
            panic!("gdl original claimed batch required");
        }
    }
    async fn gdl_session_value(state: &AppState) -> Value {
        state
            .db
            .call(|s| {
                serde_json::from_str::<Value>(
                    &crewd_store::meta_get(s.conn(), "causal.session.v1.w-1")
                        .unwrap()
                        .unwrap(),
                )
                .unwrap()["value"]
                    .clone()
            })
            .await
            .unwrap()
    }
    async fn gdl_cleanup(
        task: &mut TaskOwner,
        protocol: &ChildProtocol,
    ) -> Result<(Vec<u8>, String), Box<dyn Any + Send>> {
        let result = catch(close_owned(task, protocol)).await;
        if result.is_err() {
            task.gate.release();
            let _ = task.handle.stop();
            let stopped = timeout(LIMIT, async {
                while let Some(e) = task.events.recv().await {
                    eprintln!("GDL failed normal cleanup fallback: {e:?}");
                }
            })
            .await;
            eprintln!(
                "GDL normal cleanup failed, owned fallback EOF={}",
                stopped.is_ok()
            );
        }
        result
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn s42_actual_task_exact_success_queries_reject_wrong_generation_receipt_and_unpublished_candidate()
     {
        let (state, port, dir) = seeded().await;
        let (root1, mut first, text1, p1) =
            gdl_claim_human(&state, &port, "GDL_FIRST_ACTUAL_ROOT").await;
        let (mut task, mut protocol, mut emitted) = start_owned(&state, &port, dir.path(), 1).await;
        let observation = catch(async {
            let b1 = state.causal_fence.binder(&p1, 1).unwrap();
            let id1 = task
                .handle
                .send_bound(text1.clone(), UntrustedContext::default(), b1.as_ref())
                .unwrap();
            protocol.release_input();
            protocol.receipt("filler-drained").await;
            let (e1, capture1) =
                gdl_physical_emit(&task, &mut protocol, &mut emitted, &text1, &[]).await;
            let s1 = state.causal_fence.success(&p1, 1, id1).unwrap();
            let mut early1 = Vec::new();
            let premature1 = drain_delivered(&mut task, id1, &mut early1);
            task.gate.release();
            let received1 = gdl_receive_success(&mut task, &mut early1).await;
            gdl_witness(&mut first, s1);
            let first_ack = port
                .ack(
                    &first,
                    Ack::Delivered {
                        riders: Riders::Release,
                    },
                )
                .await
                .unwrap();
            // Real original output is now committed; retirement is the direct owner protocol,
            // not a release while unresolved. No Core/actor lifecycle claim is made.
            state.causal_fence.retire(&p1).unwrap();
            *task.gate.released.lock().unwrap() = false;
            let (root2, mut second, text2, p2) =
                gdl_claim_human(&state, &port, "GDL_SECOND_ACTUAL_ROOT").await;
            let b2 = state.causal_fence.binder(&p2, 1).unwrap();
            let id2 = task
                .handle
                .send_bound(text2.clone(), UntrustedContext::default(), b2.as_ref())
                .unwrap();
            let unpublished_accepted = state.causal_fence.success(&p2, 1, id2).is_ok();
            let queued = post_as(
                &state,
                collab::Sender::Session("w-1".into()),
                "c-1/lobby",
                "GDL_QUEUED_SECOND_STILL_FIRST_BASIS",
                &[],
            )
            .await;
            let queued_before = post_snapshot(&state, &queued).await;
            let (e2, capture2) =
                gdl_physical_emit(&task, &mut protocol, &mut emitted, &text2, &capture1).await;
            // Save Results; invalid capabilities must reach after-cleanup functional oracles.
            let wrong_generation = state.causal_fence.success(&p2, 2, id2).is_ok();
            let wrong_delivery = state.causal_fence.success(&p2, 1, id2 + 1).is_ok();
            let wrong_receipt = state.causal_fence.success(&p1, 1, id2).is_ok();
            let s2 = state.causal_fence.success(&p2, 1, id2).unwrap();
            let mut early2 = Vec::new();
            let premature2 = drain_delivered(&mut task, id2, &mut early2);
            task.gate.release();
            let received2 = gdl_receive_success(&mut task, &mut early2).await;
            gdl_witness(&mut second, s2);
            let second_ack = port
                .ack(
                    &second,
                    Ack::Delivered {
                        riders: Riders::Release,
                    },
                )
                .await
                .unwrap();
            let queued_after = post_snapshot(&state, &queued).await;
            let new_child = post_as(
                &state,
                collab::Sender::Session("w-1".into()),
                "c-1/lobby",
                "GDL_TRUE_SECOND_OUTPUT_CHILD",
                &[],
            )
            .await;
            let child = post_snapshot(&state, &new_child).await;
            (
                root2,
                id1,
                id2,
                e1,
                e2,
                received1,
                received2,
                premature1,
                premature2,
                early1,
                early2,
                unpublished_accepted,
                wrong_generation,
                wrong_delivery,
                wrong_receipt,
                queued_before,
                queued_after,
                child,
                capture2,
                first_ack.stale,
                second_ack.stale,
            )
        })
        .await;
        let cleanup = gdl_cleanup(&mut task, &protocol).await;
        let expired = task.gate.expired.load(Ordering::SeqCst);
        drop(task);
        drop(emitted);
        drop(port);
        let db = state.db.clone();
        drop(state);
        tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .unwrap();
        let (capture, receipt) = cleanup.unwrap_or_else(|e| std::panic::resume_unwind(e));
        let (
            root2,
            id1,
            id2,
            e1,
            e2,
            r1,
            r2,
            pre1,
            pre2,
            events1,
            events2,
            unpublished,
            wrong_generation,
            wrong_delivery,
            wrong_receipt,
            q_before,
            q_after,
            child,
            expected_capture,
            stale1,
            stale2,
        ) = observation.unwrap_or_else(|e| std::panic::resume_unwind(e));
        assert!(!expired);
        assert_eq!((pre1, pre2), (0, 0));
        assert_eq!((e1, r1, e2, r2), (id1, id1, id2, id2));
        assert_eq!(events1, vec![id1]);
        assert_eq!(events2, vec![id2]);
        assert!(
            !wrong_generation,
            "G exact success query must reject wrong positive generation"
        );
        assert!(
            !wrong_delivery,
            "G exact success query must reject wrong delivery ID"
        );
        assert!(
            !wrong_receipt,
            "G exact success query must reject old actual receipt against new published owner"
        );
        assert!(
            !unpublished,
            "G new registered candidate must not borrow previous committed success"
        );
        assert_eq!(
            q_before, q_after,
            "G registered but unconfirmed second candidate cannot retroactivate old post"
        );
        assert_eq!(
            q_before.resolved,
            Some(
                json!([{"key":{"root":{"kind":"message","id":root1.id,"seq":root1.seq},"reset":{"kind":"initial"}},"hop":1}])
            )
        );
        assert_eq!(
            child.resolved,
            Some(
                json!([{"key":{"root":{"kind":"message","id":root2.id,"seq":root2.seq},"reset":{"kind":"initial"}},"hop":1}])
            )
        );
        assert!(!stale1 && !stale2);
        assert_eq!(capture, expected_capture);
        assert!(receipt.contains("trailing_bytes=0"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn s42_actual_task_duplicate_success_callback_and_ack_keep_one_ticket_and_one_hop() {
        let (state, port, dir) = seeded().await;
        let (root, mut batch, text, pending) =
            gdl_claim_human(&state, &port, "GDL_DUP_ACTUAL_ROOT").await;
        let (mut task, mut protocol, mut emitted) = start_owned(&state, &port, dir.path(), 1).await;
        let observation = catch(async {
            let binding = state.causal_fence.binder(&pending, 1).unwrap();
            let id = task
                .handle
                .send_bound(text.clone(), UntrustedContext::default(), binding.as_ref())
                .unwrap();
            protocol.release_input();
            protocol.receipt("filler-drained").await;
            let (emission, wanted) =
                gdl_physical_emit(&task, &mut protocol, &mut emitted, &text, &[]).await;
            let original = state.causal_fence.success(&pending, 1, id).unwrap();
            // First emission was genuine production Task. Repeat the exact callback on
            // its original trusted adapter capability; this is not a second PTY submit.
            task.gate.inner.emitted(id);
            let duplicate_ticket = state.causal_fence.success(&pending, 1, id).unwrap();
            let mut early = Vec::new();
            let premature = drain_delivered(&mut task, id, &mut early);
            task.gate.release();
            let delivered = gdl_receive_success(&mut task, &mut early).await;
            gdl_witness(&mut batch, original.clone());
            let first = port
                .ack(
                    &batch,
                    Ack::Delivered {
                        riders: Riders::Release,
                    },
                )
                .await
                .unwrap();
            let before = db_snapshot(&state, batch.wakes[0].id).await;
            let first_post = post_as(
                &state,
                collab::Sender::Session("w-1".into()),
                "c-1/lobby",
                "GDL_DUP_FIRST_CHILD",
                &[],
            )
            .await;
            let first_cause = post_snapshot(&state, &first_post).await;
            let pre_duplicate = db_snapshot(&state, batch.wakes[0].id).await;
            let second = port
                .ack(
                    &batch,
                    Ack::Delivered {
                        riders: Riders::Release,
                    },
                )
                .await
                .unwrap();
            let after = db_snapshot(&state, batch.wakes[0].id).await;
            let repeat_post = post_as(
                &state,
                collab::Sender::Session("w-1".into()),
                "c-1/lobby",
                "GDL_DUP_REPEAT_CHILD",
                &[],
            )
            .await;
            let repeat = post_snapshot(&state, &repeat_post).await;
            (
                id,
                emission,
                delivered,
                premature,
                early,
                original == duplicate_ticket,
                wanted,
                before,
                pre_duplicate,
                after,
                first_cause.resolved,
                repeat.resolved,
                first.stale,
                second.stale,
            )
        })
        .await;
        let cleanup = gdl_cleanup(&mut task, &protocol).await;
        let expired = task.gate.expired.load(Ordering::SeqCst);
        drop(task);
        drop(emitted);
        drop(port);
        let db = state.db.clone();
        drop(state);
        tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .unwrap();
        let (capture, receipt) = cleanup.unwrap_or_else(|e| std::panic::resume_unwind(e));
        let (
            id,
            emission,
            delivered,
            premature,
            events,
            ticket_same,
            wanted,
            before,
            pre_duplicate,
            after,
            first,
            repeat,
            stale1,
            stale2,
        ) = observation.unwrap_or_else(|e| std::panic::resume_unwind(e));
        assert!(!expired);
        assert_eq!(premature, 0);
        assert_eq!((emission, delivered), (id, id));
        assert_eq!(events, vec![id]);
        assert!(
            ticket_same,
            "D duplicate exact producer callback must not issue a new serial/ticket"
        );
        assert_eq!(
            pre_duplicate, after,
            "D duplicate exact DB ack cannot advance durable frontier/row/cursor/attempts"
        );
        let expected = Some(
            json!([{"key":{"root":{"kind":"message","id":root.id,"seq":root.seq},"reset":{"kind":"initial"}},"hop":1}]),
        );
        assert_eq!(first, expected);
        assert_eq!(
            repeat, expected,
            "D duplicate same-fate ack cannot add a second inherited hop"
        );
        assert_eq!(before.wake.state, cw::WakeState::Delivered);
        assert!(!stale1 && !stale2);
        assert_eq!(capture, wanted);
        assert!(receipt.contains("trailing_bytes=0"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn s42_actual_task_pending_old_first_ack_preserves_new_committed_independent_hook_basis()
    {
        let (state, port, dir) = seeded().await;
        let (_root_a, mut old, text, pending) =
            gdl_claim_human(&state, &port, "GDL_L_PHYSICAL_OLD_A").await;
        // Genuine member DM, not an injected cursor/lease. Establish its empty baseline
        // before the new Human message. The old A exact receipt remains active throughout.
        let channel_b = state
            .collab(|tx| {
                let (channel, _) = collab::channels::ensure_dm(
                    tx,
                    "c-1",
                    &crewd_proto::MemberRef::session("w-1"),
                    &crewd_proto::MemberRef::session("cap-1"),
                )?;
                collab::cursors::pin_to_end(tx, "w-1", &channel.id)?;
                Ok(channel.id)
            })
            .await
            .unwrap();
        let (mut task, mut protocol, mut emitted) = start_owned(&state, &port, dir.path(), 1).await;
        let observation = catch(async {
            let binder = state.causal_fence.binder(&pending, 1).unwrap();
            let id = task
                .handle
                .send_bound(text.clone(), UntrustedContext::default(), binder.as_ref())
                .unwrap();
            protocol.release_input();
            protocol.receipt("filler-drained").await;
            let (emission, wanted) =
                gdl_physical_emit(&task, &mut protocol, &mut emitted, &text, &[]).await;
            let success = state.causal_fence.success(&pending, 1, id).unwrap();
            let before_old = output_value(&state, pending.stable_key()).await;
            let initial_session = gdl_session_value(&state).await;
            let b = post_as(
                &state,
                collab::Sender::Human,
                &channel_b,
                "GDL_L_NEW_INDEPENDENT_B",
                &[],
            )
            .await;
            let b_id = channel_b.clone();
            let b_seq = b.seq;
            let (hook, a_held, a_omitted) = state
                .collab(move |tx| {
                    let prepared = cr::prepare_causal_unread(tx, "w-1")?;
                    let a_held = prepared.snapshot.blocks_channel("c-1/lobby");
                    let a_omitted = !prepared.blocks.iter().any(|(ch, _)| ch == "c-1/lobby");
                    let targets = vec![(b_id, b_seq)];
                    let causes = prepared.causes.select(&targets)?;
                    Ok((
                        cr::commit_hook_output(tx, &prepared.snapshot, &causes, &targets)?,
                        a_held,
                        a_omitted,
                    ))
                })
                .await
                .unwrap();
            // Save the real outcome; do not turn Busy/Stale into successful fixture output.
            let hook_key = match &hook {
                cr::Decision::Applied(v) => v.output.clone(),
                _ => None,
            };
            let new_output = match &hook_key {
                Some(k) => Some(output_value(&state, k).await),
                None => None,
            };
            let after_new_session = gdl_session_value(&state).await;
            let still_old = output_value(&state, pending.stable_key()).await;
            let b_cursor_before = port.cursor("w-1", &channel_b).await.unwrap();
            let mut early = Vec::new();
            let premature = drain_delivered(&mut task, id, &mut early);
            task.gate.release();
            let delivered = gdl_receive_success(&mut task, &mut early).await;
            gdl_witness(&mut old, success);
            let first_ack = port
                .ack(
                    &old,
                    Ack::Delivered {
                        riders: Riders::Release,
                    },
                )
                .await
                .unwrap();
            let old_done = output_value(&state, pending.stable_key()).await;
            let after_old_session = gdl_session_value(&state).await;
            let b_cursor_after = port.cursor("w-1", &channel_b).await.unwrap();
            let after = post_as(
                &state,
                collab::Sender::Session("w-1".into()),
                &channel_b,
                "GDL_L_AFTER_OLD_FIRST_ACK",
                &[],
            )
            .await;
            let descendant = post_snapshot(&state, &after).await;
            (
                b,
                id,
                emission,
                delivered,
                premature,
                early,
                wanted,
                hook_key,
                a_held,
                a_omitted,
                before_old,
                initial_session,
                new_output,
                after_new_session,
                still_old,
                b_cursor_before,
                first_ack.stale,
                old_done,
                after_old_session,
                b_cursor_after,
                descendant.resolved,
            )
        })
        .await;
        let cleanup = gdl_cleanup(&mut task, &protocol).await;
        let expired = task.gate.expired.load(Ordering::SeqCst);
        drop(task);
        drop(emitted);
        drop(port);
        let db = state.db.clone();
        drop(state);
        tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .unwrap();
        let (capture, receipt) = cleanup.unwrap_or_else(|e| std::panic::resume_unwind(e));
        let (
            b,
            id,
            emission,
            delivered,
            premature,
            events,
            wanted,
            hook_key,
            a_held,
            a_omitted,
            before_old,
            initial_session,
            new_output,
            after_new_session,
            still_old,
            b_before,
            stale,
            old_done,
            after_old_session,
            b_after,
            descendant,
        ) = observation.unwrap_or_else(|e| std::panic::resume_unwind(e));
        assert!(!expired);
        assert_eq!(premature, 0);
        assert_eq!((emission, delivered), (id, id));
        assert_eq!(events, vec![id]);
        assert!(
            a_held && a_omitted,
            "L ordinary hook must exclude genuinely held A and preserve its ownership"
        );
        let hook_key = hook_key
            .expect("L genuine independent B hook must actually Applied with nonempty output");
        assert_eq!(before_old["status"], "pending");
        assert_eq!(still_old["status"], "pending");
        assert_eq!(initial_session["active"], pending.stable_key());
        assert_eq!(
            after_new_session["active"],
            pending.stable_key(),
            "L independent hook does not retire or overwrite A ownership"
        );
        assert_eq!(new_output.unwrap()["status"], "committed");
        assert_eq!(after_new_session["last_output"], hook_key);
        assert_eq!(initial_session["epoch"], after_new_session["epoch"]);
        assert_eq!(
            initial_session["applied"], after_new_session["applied"],
            "L distinct actual output, not a new Work epoch"
        );
        assert_eq!(b_before, b.seq);
        assert_eq!(b_after, b.seq);
        assert!(!stale);
        assert_eq!(
            old_done["status"], "committed",
            "L true old first same-fate acknowledgement still succeeds"
        );
        assert_eq!(
            after_old_session["basis"], after_new_session["basis"],
            "L OLDpending then NEWcommitted then OLDfirstAck cannot overwrite actual newer basis"
        );
        assert_eq!(
            after_old_session["last_output"], hook_key,
            "L canonical last output stays genuinely newer committed B"
        );
        assert_eq!(
            descendant,
            Some(
                json!([{"key":{"root":{"kind":"message","id":b.id,"seq":b.seq},"reset":{"kind":"initial"}},"hop":1}])
            )
        );
        assert_eq!(capture, wanted);
        assert!(receipt.contains("trailing_bytes=0"));
    }
}
