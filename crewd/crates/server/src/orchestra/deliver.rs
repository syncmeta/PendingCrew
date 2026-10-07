//! 唤醒的入队与投递（spec §22.4「唤醒规划」「队列」「idle 边沿」「问被回答」；§22.7 Q2 / Q5）。
use std::collections::BTreeMap;

use crewd_collab::render::safe_name;
use crewd_exec::health::HealthKind;
use crewd_exec::session::DeliveryOutcome;
use crewd_wake::breaker::{self, HUMAN, Verdict};
use crewd_wake::plan::{self, ChannelCtx, MsgFacts, Sender};
use crewd_wake::policy::{self, Gate, RestartRow};
use crewd_wake::queue::{self, ids};
use crewd_wake::{
    Latch, LatchScope, MIN, MessageRef, Ms, NewWake, Priority, QueuedWake, WakeKind, source, text,
};

use super::core::{
    CollabFact, ControlError, Core, Inflight, Input, Write, delivery_settles_inflight,
};
use super::exec::Context;
use super::hooks::ArbiterQuestion;
use super::port::*;

/// 不需要正文的唤醒（`restart` 且没有附言）与其余的分开：前者只要「人在跑」就算送到，不依赖渲染结果是否为空。
fn split_silent(batch: Vec<QueuedWake>) -> (Vec<QueuedWake>, Vec<QueuedWake>) {
    batch
        .into_iter()
        .partition(|w| w.kind == WakeKind::Restart && w.note.is_none())
}

/// 一条消息唤醒的来源（环路熔断的键）：**唯一的定义**是 `crewd_wake::breaker::origin_of`。
fn message_origin(msg: &MsgFacts, target: &str) -> String {
    breaker::origin_of(WakeKind::Message, Some(&msg.sender), target)
}

/// 渲染并裁好的一批（[`Core::render_fitted`]）：`batch.riders` = 落在窗口外的（M3a R6），随这批一起 claim。
struct Fitted {
    batch: Batch,
    text: String,
}

fn restart_row(r: &SessionRow) -> RestartRow {
    RestartRow {
        session: r.id.clone(),
        name: r.name(),
        crew: r.crew_id.clone(),
        captain: r.captain,
        was_working: r.was_working,
        last_active_at: r.last_active_at,
    }
}

impl<P: Port> Core<P> {
    pub(crate) async fn on_collab(&mut self, f: CollabFact) -> anyhow::Result<()> {
        match f {
            CollabFact::MessageAppended { channel, seq } => {
                self.on_message(MessageRef { channel, seq }).await
            }
            CollabFact::WakeEnqueued { session } => self.try_deliver(&session).await.map(|_| ()),
            CollabFact::AskAnswered { crew, ask, cleared } => {
                // 叫醒提问者（或机长转达）的唤醒由 M3a 的 `asks::answer` 在同一事务里入队，它的 `WakeEnqueued`
                // 会把它投出去；M3c 这里只把 BlockerCleared 交给 M4b 的回调点（§22.6）。
                self.hooks
                    .ask
                    .on_answered(&crew, &ask, &cleared, self.now())
                    .await;
                Ok(())
            }
            CollabFact::Progress { session } => {
                self.breaker.progress(&session);
                Ok(())
            }
            CollabFact::Lagged => self.on_lagged().await,
        }
    }

    async fn on_message(&mut self, r: MessageRef) -> anyhow::Result<()> {
        let Some((msg, text)) = self.port.message(&r).await? else {
            return Ok(());
        };
        let now = self.now();
        let Some(ctx) = self.port.channel_ctx(&r.channel, now).await? else {
            return Ok(());
        };
        if plan::needs_arbiter(&msg, &ctx, now) {
            // Jev 要联网：放到派生任务里问，答案（或超时）以 `Input::Arbitrated` 回来，不卡 actor。
            let (arbiter, tx, limit) = (
                self.hooks.arbiter.clone(),
                self.inbox.clone(),
                self.cfg.arbiter_timeout,
            );
            tokio::spawn(async move {
                let q = ArbiterQuestion {
                    message: msg,
                    text,
                    ctx,
                };
                let answer = tokio::time::timeout(limit, arbiter.wake_set(&q))
                    .await
                    .ok()
                    .flatten();
                let _ = tx.send(Input::Arbitrated {
                    msg: q.message,
                    ctx: q.ctx,
                    answer,
                });
            });
            return Ok(());
        }
        self.enqueue_targets(&msg, &ctx, None).await
    }

    pub(crate) async fn enqueue_targets(
        &mut self,
        msg: &MsgFacts,
        ctx: &ChannelCtx,
        answer: Option<&[String]>,
    ) -> anyhow::Result<()> {
        let now = self.now();
        let targets = plan::plan(msg, ctx, answer, now);
        if let Sender::Session { id: poster, .. } = &msg.sender {
            // 裁定 3：这条消息算不算产出（回叫叫醒自己的人不算）。`woke` 只取**显式目标**（@ 到的、Arbiter /
            // 群主兜底的）——只因收听被叫醒的不算（Task 3 评审：否则 a↔b 互 @、旁边有个收听者时环路截不住）。
            // 显式目标 = 同一份规划去掉收听者，规则只在 `plan` 一处。
            let explicit = ChannelCtx {
                listens: vec![],
                ..ctx.clone()
            };
            let woke: Vec<String> = plan::plan(msg, &explicit, answer, now)
                .into_iter()
                .map(|t| t.session)
                .collect();
            self.breaker.on_post(poster, &woke, now);
        }
        // 先全部入队、再逐个试投（评审 (3)）：一个目标投递出错不能让后面的目标连唤醒都没有。
        let mut fresh = Vec::new();
        for t in targets {
            let w = NewWake {
                session: t.session.clone(),
                kind: WakeKind::Message,
                source: source::message(&msg.id),
                priority: t.priority,
                not_before: now,
                note: None,
                origin: message_origin(msg, &t.session),
            };
            if self.port.enqueue(w).await? {
                fresh.push(t.session);
            }
        }
        self.deliver_each(&fresh).await;
        Ok(())
    }

    /// 逐个试投；一个出错只记日志（唤醒留在 pending，下一次定时拍 / idle 边沿再试），不连累后面的。
    async fn deliver_each(&mut self, sessions: &[String]) {
        for s in sessions {
            if let Err(e) = self.try_deliver(s).await {
                eprintln!("crewd: 编排：给 {s} 投递失败（留在队列里）：{e:#}");
            }
        }
    }

    /// 能投就投（裁定 7、8）。返回这次有没有投出 / 拉起。
    pub(crate) async fn try_deliver(&mut self, session: &str) -> anyhow::Result<bool> {
        let now = self.now();
        // 每轮可能把「已注入」的消息类唤醒直接记 delivered，然后重选；最多 4 轮。
        for _ in 0..4 {
            if self.starting.contains_key(session) {
                return Ok(false);
            }
            let pending = self.port.pending(session).await?;
            if !pending.iter().any(|w| queue::is_due(w, now)) {
                return Ok(false);
            }
            if !self.live_ready(session, &pending, now) {
                return Ok(false);
            }
            let mut origins: BTreeMap<i64, String> = BTreeMap::new();
            let mut candidates = Vec::new();
            // 退避中的不会到点（退避就是库里的 `not_before`），`is_due` 已经把它们滤掉。
            for w in pending.iter().filter(|w| queue::is_due(w, now)) {
                if self.breaker.held(session, &w.origin).is_none() {
                    origins.insert(w.id, w.origin.clone());
                    candidates.push(w.clone());
                }
            }
            let batch = queue::select_batch(&candidates, now, &self.cfg.batch);
            if batch.is_empty() {
                return Ok(false);
            }
            let mut fresh = Vec::new();
            let mut injected = Vec::new();
            for w in batch {
                if self.injected(session, &w).await? {
                    injected.push(w.id);
                } else {
                    fresh.push(w);
                }
            }
            if !injected.is_empty() {
                self.port.settle(&injected, Settle::Delivered, now).await?;
                self.port.publish(OrchestraEvent::WakeChanged {
                    session: session.into(),
                });
            }
            if fresh.is_empty() {
                continue;
            }
            let running = self.live.contains_key(session);
            if !running && !self.may_launch(session, &fresh, &pending, now) {
                return Ok(false);
            }
            let fresh = self.admit(session, fresh, &origins, now).await?;
            if fresh.is_empty() {
                continue;
            }
            return if running {
                self.send_batch(session, fresh).await
            } else {
                self.launch_for_wakes(session, fresh).await
            };
        }
        Ok(false)
    }

    /// 在跑的目标此刻能不能投（裁定 7）。额度类 health 在身且额度唤醒到点 → 先清 health。没在跑 → true（交给拉起）。
    fn live_ready(&mut self, session: &str, pending: &[QueuedWake], now: Ms) -> bool {
        let Some(l) = self.live.get_mut(session) else {
            return true;
        };
        if l.inflight.is_some() || l.working || l.decision.is_some() || l.stopping || l.halting {
            return false;
        }
        match &l.health {
            Some(h) if h.kind.is_quota() => {
                if !policy::quota_due(pending, now) {
                    return false;
                }
                let _ = l.handle.clear_quota_health();
                l.health = None;
                true
            }
            Some(h) if matches!(h.kind, HealthKind::AuthRequired | HealthKind::LaunchFailed) => {
                false
            }
            _ => true,
        }
    }

    /// 没在跑的目标要不要为这批唤醒拉起（裁定 4、5、13）。
    fn may_launch(
        &mut self,
        session: &str,
        batch: &[QueuedWake],
        pending: &[QueuedWake],
        now: Ms,
    ) -> bool {
        if self.held.contains(session) {
            // 等人答重启询问（§22.7 Q1）：不被自动拉起。例外（控制者裁定 C4）：询问开出之后人新 @ 了它
            // → 只拉起它（`launch_for_wakes` 把它移出名单，算对它单个 resume）；启动前就排着的人类唤醒不算。
            let called_by_human = batch.iter().any(|w| {
                w.priority == Priority::Human
                    && w.kind == WakeKind::Message
                    && w.id > self.held_watermark
            });
            if !called_by_human {
                return false;
            }
        }
        // 人的唤醒不受可选上限挡（C5；错开照样生效）。`select_batch` 只取最高一档，看第一条就够。
        let human = batch.first().is_some_and(|w| w.priority == Priority::Human);
        if !human && policy::quota_pending(pending) && !policy::quota_due(pending, now) {
            return false; // 额度没恢复：自动唤醒别把它拉起来又撞墙
        }
        match self
            .gate
            .check(self.live.len() + self.starting.len(), human, now)
        {
            Gate::Go => true,
            Gate::AtCap => false,
            Gate::WaitUntil(t) => {
                self.timers.insert(t);
                false
            }
        }
    }

    async fn injected(&self, session: &str, w: &QueuedWake) -> anyhow::Result<bool> {
        let (WakeKind::Message, Some(r)) = (w.kind, &w.msg) else {
            return Ok(false);
        };
        Ok(self.port.cursor(session, &r.channel).await? >= r.seq)
    }

    /// 产出的兜底信号（裁定 3）：正路是 `CollabFact::Progress`；事件丢了（`Lagged`）时，`last_progress_at`
    /// （M3a `note_progress` 写，发言不写）比上次看到的新 = 有过产出。
    async fn observe_progress(&mut self, session: &str) -> anyhow::Result<()> {
        let Some(at) = self
            .port
            .session(session)
            .await?
            .and_then(|r| r.last_progress_at)
        else {
            return Ok(());
        };
        if self.seen_progress.get(session) == Some(&at) {
            return Ok(());
        }
        let first = self.seen_progress.insert(session.to_string(), at).is_none();
        if !first {
            self.breaker.progress(session);
        }
        Ok(())
    }

    /// 环路熔断（§22.7 Q5）：每个来源问一次；被挡的那几条留在 pending。这里只**判**、不计数：计数在投进去了之后
    /// （`on_delivery` 的 `Delivered`，评审 (2)）——渲染失败、投递失败、只有积压的批都不算一次唤醒。
    async fn admit(
        &mut self,
        session: &str,
        batch: Vec<QueuedWake>,
        origins: &BTreeMap<i64, String>,
        now: Ms,
    ) -> anyhow::Result<Vec<QueuedWake>> {
        self.observe_progress(session).await?;
        let mut pass: BTreeMap<String, bool> = BTreeMap::new();
        for w in &batch {
            let o = &origins[&w.id];
            if pass.contains_key(o) {
                continue;
            }
            let ok = match self.breaker.judge(session, o, now) {
                Verdict::Pass => true,
                Verdict::Held(_) => false,
                Verdict::Tripped(latch) => {
                    // 先落库、再在内存里生效（P2 / R12）：写不进去就这次不熔断（错误往上抛，唤醒留在 pending），
                    // 内存与库不分叉——重启后也不会「库里没有、内存里有」地放掉这条环路。
                    self.port.put_latch(&latch).await?;
                    self.breaker.latch(latch.clone());
                    self.notify_loop(session, &latch, o, w).await?;
                    false
                }
            };
            pass.insert(o.clone(), ok);
        }
        Ok(batch
            .into_iter()
            .filter(|w| pass[&origins[&w.id]])
            .collect())
    }

    /// 熔断了：告诉人（人类群 @人）和群主（消息所在的群 / 大厅 @群主）。名字过 `safe_name`（C12 / C20）；
    /// 来源是系统时换成 `origin.system` 的话。
    async fn notify_loop(
        &mut self,
        target: &str,
        latch: &Latch,
        origin: &str,
        w: &QueuedWake,
    ) -> anyhow::Result<()> {
        let lang = self.lang();
        let Some(row) = self.port.session(target).await? else {
            return Ok(());
        };
        let target_name = safe_name(&row.name());
        let origin_name = if origin == breaker::SYSTEM {
            text::get(lang, "origin.system").to_string()
        } else {
            let raw = self
                .port
                .session(origin)
                .await?
                .map_or_else(|| origin.to_string(), |r| r.name());
            safe_name(&raw)
        };
        let count = match latch.scope {
            LatchScope::Edge => self.cfg.loops.edge_max,
            LatchScope::Session => self.cfg.loops.target_max,
        } + 1;
        let minutes = (self.cfg.loops.window_ms / MIN).to_string();
        let count = count.to_string();
        let human = text::render(
            lang,
            "loop.tripped.human",
            &[
                ("target", &target_name),
                ("origin", &origin_name),
                ("count", &count),
                ("minutes", &minutes),
            ],
        );
        self.port
            .post_system(
                Audience::HumanChannel {
                    crew: row.crew_id.clone(),
                },
                human,
            )
            .await?;
        let owner = text::render(
            lang,
            "loop.tripped.owner",
            &[("target", &target_name), ("origin", &origin_name)],
        );
        let to = match (w.kind, &w.msg) {
            (WakeKind::Message, Some(r)) => Audience::Channel {
                channel: r.channel.clone(),
                mention_owner: true,
            },
            _ => Audience::Lobby {
                crew: row.crew_id,
                mention_owner: true,
            },
        };
        self.port.post_system(to, owner).await
    }

    /// 渲染。渲染不出来的（Port 已记 dropped）拿掉；窗口外的（`deferred`，M3a R6）从正文里拿掉、作为随批行
    /// （`Batch::riders`）跟这批一起 claim——送到时放回 pending、下一批带上（C24）。超过 8KB 就从尾部减**非消息**的唤醒（减下的留在 pending）；
    /// 消息项的 8KB 只是建议值：它是一个群「游标之后、最旧的至多 `UNREAD_CAP` 条」的连续窗口，减一条消息行改变不了
    /// 窗口（会被重新算进来），也不能截一半（游标要推到窗口末尾），所以只剩消息项时照原样投。
    /// 返回这批（真正投出去的唤醒、随批行、送到后各群游标推到哪）与正文（[`Fitted`]）。
    /// 渲染出错（M3a `build_batch` 遇到坏账本 fail-loud）原样往上抛：唤醒留在 pending，**不在这里重试**——
    /// 下一次定时拍 / idle 边沿再试（不紧循环）。
    async fn render_fitted(
        &self,
        session: &str,
        mut batch: Vec<QueuedWake>,
    ) -> anyhow::Result<Fitted> {
        loop {
            let r = self.port.render_wake(session, &batch).await?;
            let riders: Vec<QueuedWake> = batch
                .iter()
                .filter(|w| r.deferred.contains(&w.id))
                .cloned()
                .collect();
            batch.retain(|w| !r.dropped.contains(&w.id) && !r.deferred.contains(&w.id));
            let trim = batch.iter().rposition(|w| w.kind != WakeKind::Message);
            match trim {
                Some(i) if r.text.len() > self.cfg.batch.max_bytes && batch.len() > 1 => {
                    batch.remove(i);
                }
                _ => {
                    return Ok(Fitted {
                        batch: Batch {
                            session: session.to_string(),
                            wakes: batch,
                            riders,
                            cursor_targets: r.cursor_targets,
                        },
                        text: r.text,
                    });
                }
            }
        }
    }

    /// 投给在跑的 session：记 inflight、把正文排进它的写队列（与 nudge 按收下的先后写，[`Core::pump_writes`]）。
    async fn send_batch(&mut self, session: &str, batch: Vec<QueuedWake>) -> anyhow::Result<bool> {
        let now = self.now();
        let (silent, speak) = split_silent(batch);
        if !silent.is_empty() {
            // 不需要正文的唤醒（无附言的恢复）：在跑就算送到。
            self.port
                .settle(&ids(&silent), Settle::Delivered, now)
                .await?;
        }
        if speak.is_empty() {
            self.port.publish(OrchestraEvent::WakeChanged {
                session: session.into(),
            });
            return Ok(true);
        }
        let Fitted { batch, text } = self.render_fitted(session, speak).await?;
        // 只有积压（M3a R6：窗口里全是比被叫消息更旧的未读，被叫的那几行 deferred）：`wakes` 空而 `cursor_targets`
        // 非空——照样投，投完游标前进，下一批就轮到被叫的那几条。两样都空才是真没东西可投（随批行不 claim、留在 pending）。
        if !batch.has_work() {
            return Ok(false);
        }
        let held = batch.ids();
        let accepts = self
            .live
            .get(session)
            .is_some_and(|l| l.handle.accepts_context());
        let extras = if accepts {
            self.hooks.extras.extras(session).await
        } else {
            Context::new()
        };
        // 进正文的与随批的一起拿走（C24）：从此它们只属于这一批，直到这批的 ack。
        if !held.is_empty() {
            self.port.claim(&batch).await?;
        }
        let Some(l) = self.live.get_mut(session) else {
            if !held.is_empty() {
                self.port.settle(&held, Settle::Requeue, now).await?;
            }
            return Ok(false);
        };
        l.inflight = Some(Inflight {
            delivery: None,
            batch,
        });
        l.writes.push_back(Write::Deliver { text, extras });
        self.pump_writes(session).await?;
        self.port.publish(OrchestraEvent::WakeChanged {
            session: session.into(),
        });
        // 写失败的那批已由 `pump_writes` 交给 `fail_batch`（inflight 清空了）。
        Ok(self.live.get(session).is_some_and(|l| l.inflight.is_some()))
    }

    async fn launch_for_wakes(
        &mut self,
        session: &str,
        batch: Vec<QueuedWake>,
    ) -> anyhow::Result<bool> {
        let now = self.now();
        let Some(row) = self.port.session(session).await? else {
            let s = Settle::Dropped {
                error: "no such session".into(),
            };
            self.port.settle(&ids(&batch), s, now).await?;
            return Ok(false);
        };
        let exec = match self.execs.get(&row.executor) {
            Ok(e) if row.workdir.is_some() => e,
            other => {
                // 重试也起不来（执行器没接入 / 没有工作目录）：记恢复失败、告警一次，唤醒直接放弃。
                let detail = match other {
                    Err(e) => e.to_string(),
                    Ok(_) => ControlError::NoWorkdir(row.id.clone()).to_string(),
                };
                self.launch_failed(&row, &detail).await?;
                let s = Settle::Dropped { error: detail };
                self.port.settle(&ids(&batch), s, now).await?;
                self.port.publish(OrchestraEvent::WakeChanged {
                    session: session.into(),
                });
                return Ok(false);
            }
        };
        // C4：等重启答复的 session 被人 @ 拉起 = 对它单个 resume；上次停在干活中途的，开场补一句「被打断」。
        let interrupted = (self.held.remove(session) && row.was_working)
            .then(|| text::get(self.lang(), "restart.interrupted").to_string());
        let (silent, speak) = split_silent(batch);
        if !silent.is_empty() {
            // 无附言的恢复：拉起本身就是它要的（起不来由 launch_failed 告警）。
            self.port
                .settle(&ids(&silent), Settle::Delivered, now)
                .await?;
        }
        if speak.is_empty() {
            self.launch(row, exec, interrupted, vec![], vec![]).await?;
            return Ok(true);
        }
        let Fitted { batch, text } = self.render_fitted(session, speak).await?;
        if !batch.has_work() {
            return Ok(false);
        }
        let text = match interrupted {
            Some(note) => format!("{note}\n\n{text}"),
            None => text,
        };
        self.launch_carrying(row, exec, Some(text), batch).await?;
        Ok(true)
    }

    /// 投递回执（裁定 8）。非终局的（Blocked / NotReady / Stalled）驱动层自己会补投，等终局；`Abandoned`（停掉 /
    /// 退出时的 abandon）不结算 inflight——那批只在 `on_exited` 里结算（绑定规则，判据 [`delivery_settles_inflight`]）。
    /// 终局一律**一次** [`Port::ack`]（C24）：送到 = 行、随批行、游标一个事务；没送到 = 整批退避或放弃（`fail_batch`）。
    pub(crate) async fn on_delivery(
        &mut self,
        session: &str,
        id: u64,
        outcome: DeliveryOutcome,
    ) -> anyhow::Result<()> {
        if !delivery_settles_inflight(&outcome) {
            return Ok(());
        }
        let now = self.now();
        let Some(l) = self.live.get_mut(session) else {
            return Ok(());
        };
        if l.inflight.as_ref().is_none_or(|f| f.delivery != Some(id)) {
            return Ok(());
        }
        let delivered = matches!(outcome, DeliveryOutcome::Delivered);
        // 没送到、且正在被停掉（人停 / crewd 叫停）：这批只由 `on_exited` 结算（人停 → 丢弃；crewd 停 → 退回）。这里不能
        // `fail_batch`——那会让人停的那批退避后把它重新拉起（复审 P7b）。**送到了**的照样在这里确认（Task 8 复审 7(a)）：
        // 已经进了会话的不能因为随后的停掉被当成没送到。
        if !delivered && (l.stopping || l.halting) {
            return Ok(());
        }
        let inf = l.inflight.take().expect("上面查过");
        if !delivered {
            return self.fail_batch(inf.batch, &format!("{outcome:?}")).await;
        }
        l.worked = true;
        // 随批行：平时放回 pending、下一批带上；人正在停它时跟别的 pending 一样丢掉（不把刚停掉的拉起来）。
        let riders = if l.stopping {
            Riders::Drop("stopped".into())
        } else {
            Riders::Release
        };
        // M3a R6：游标推到渲染时报回的 `cursor_targets`（批里实际带上的最大 seq），**不是**排队行里最大的 seq——随批行
        // 比它新，推过去就把它们越过、永远不投了。跟行的确认同一个事务（游标是消息唤醒的幂等键）。
        // 出错 = 什么都没改：这几行仍是 inflight、没人拿着，由重启时的回收接走（记日志，不在这里重试）。
        let acked = self.port.ack(&inf.batch, Ack::Delivered { riders }).await?;
        // 环路熔断只数投进去了的（评审 (2)）、而且只在确认提交之后：一次投递、每个来源记一次（同一批里同一来源的几条
        // 合并成一次唤醒）。随批行没投进去，不记；只有积压的批没有唤醒，什么都不记。
        for o in &acked.origins {
            self.breaker.record(session, o, now);
        }
        self.port.publish(OrchestraEvent::WakeChanged {
            session: session.into(),
        });
        Ok(())
    }

    /// idle 边沿（取代 `CrewSessionRunner.swift:490-541 runBecameIdle`）：①到期唤醒合并投递，投了就结束；
    /// ②`IdleHook`（M4b 的停下即检查）；③到此为止（机长 60s 检查点、continue_work、Todo sweep 都不再有）。
    /// IdleHook 只在「没有能投的」时问（评审 (4)(5)）：正在被停掉 / crewd 叫停 / 已经没在跑的不是 idle 边沿
    /// （驱动层 abandon 发的 `Working(false)`）；四轮上限用完、还有此刻就投得出去的 pending 也不问。
    /// 没到点的（明天的定时、额度唤醒、退避中的）和被熔断挡着的**不算**——它们不该让停下即检查失效。
    pub(crate) async fn on_idle(&mut self, session: &str) -> anyhow::Result<()> {
        if self.try_deliver(session).await? {
            return Ok(());
        }
        let idle = self
            .live
            .get(session)
            .is_some_and(|l| !l.stopping && !l.halting);
        let now = self.now();
        if !idle || self.deliverable_now(session, now).await? {
            return Ok(());
        }
        if let Some(w) = self.hooks.idle.on_idle(session, now).await
            && self.port.enqueue(w).await?
        {
            self.try_deliver(session).await?;
        }
        Ok(())
    }

    /// 还有此刻就投得出去的 pending 吗：到点（退避中的 `not_before` 在后面、不到点）、没被熔断挡着
    /// （pending 行本来就不是 inflight）。
    async fn deliverable_now(&self, session: &str, now: Ms) -> anyhow::Result<bool> {
        Ok(self
            .port
            .pending(session)
            .await?
            .iter()
            .any(|w| queue::is_due(w, now) && self.breaker.held(session, &w.origin).is_none()))
    }

    pub(crate) async fn rescan(&mut self) -> anyhow::Result<()> {
        let now = self.now();
        for s in self.port.due_sessions(now).await? {
            // 一个 session 出错（例如 M3a `build_batch` 遇到坏账本 fail-loud）只记日志、不连累别人；它的唤醒留在
            // pending，下一次定时拍 / idle 边沿再试——不在这里原地重试（不紧循环：过期的 not_before 不会让定时器立刻再响，
            // 只剩 30s 心跳）。
            if let Err(e) = self.try_deliver(&s).await {
                eprintln!("crewd: 编排：给 {s} 投递失败（留在队列里）：{e:#}");
            }
        }
        Ok(())
    }

    /// 协作事件落后（`Lagged`）：漏掉的不只是消息，从库里重推状态，不只重排未读——
    /// ① 重启询问答没答（漏了 `RestartAnswered` = 名单永远等着）；② 未读重新规划（裁定 16）；③ 到期的唤醒重扫
    /// （漏了 `WakeEnqueued` 的 ask_answer / 审批结果 / timer 都在库里排着）。漏掉的 `Progress` 由 `observe_progress`
    /// 的 `last_progress_at` 兜底。漏掉的 `AskAnswered.cleared` 无从重推（M3a 没有「某时刻之后答过的问」的查询）：
    /// `AskAnsweredHook` 的实现（M4b）必须容忍漏调、自己能从账本重推（写进交付记录）。
    pub(crate) async fn on_lagged(&mut self) -> anyhow::Result<()> {
        if let Some(id) = self.restart_prompt
            && !self.held.is_empty()
            && self.port.restart_answer(id).await? == Some(true)
        {
            let held: Vec<String> = self.held.iter().cloned().collect();
            self.on_restart_answered(id, true, held).await?;
        }
        self.rescue_backlog().await
    }

    /// 人答了重启询问（§22.7 Q1）。「恢复」→ 名单里仍在等的，按机长在前、最近活跃在前入队一条恢复唤醒（被打断的
    /// 带附言），再逐个试投；错开由启动闸做（裁定 5）。「先不」→ 什么都不做，名单留着。
    /// （控制者裁定 C6：Task 8 的 `on_lagged` 要调它，所以定义在这里；Task 9 的 `boot` / `RestartAnswered` 接上它。）
    pub(crate) async fn on_restart_answered(
        &mut self,
        prompt: i64,
        resume: bool,
        sessions: Vec<String>,
    ) -> anyhow::Result<()> {
        if !resume {
            return Ok(());
        }
        let mut rows = Vec::new();
        for s in sessions.iter().filter(|s| self.held.contains(*s)) {
            if let Some(r) = self.port.session(s).await? {
                rows.push(restart_row(&r));
            }
        }
        let ordered = policy::restart_order(rows);
        let (now, lang) = (self.now(), self.lang());
        for r in &ordered {
            self.held.remove(&r.session);
            let w = NewWake {
                session: r.session.clone(),
                kind: WakeKind::Restart,
                source: policy::restart_source(&prompt.to_string()),
                priority: Priority::Human,
                not_before: now,
                note: r
                    .was_working
                    .then(|| text::get(lang, "restart.interrupted").to_string()),
                origin: HUMAN.into(),
            };
            self.port.enqueue(w).await?;
        }
        let sessions: Vec<String> = ordered.into_iter().map(|r| r.session).collect();
        self.deliver_each(&sessions).await;
        Ok(())
    }

    /// 积压补救（裁定 16）：每个 session 的未读重新规划一遍（不问 Arbiter；6h 陈旧闸照旧；UNIQUE 去重）。
    pub(crate) async fn rescue_backlog(&mut self) -> anyhow::Result<()> {
        let now = self.now();
        let mut ctxs: BTreeMap<String, Option<ChannelCtx>> = BTreeMap::new();
        for s in self.port.all_session_ids().await? {
            for msg in self.port.unread(&s).await? {
                if !ctxs.contains_key(&msg.at.channel) {
                    let ctx = self.port.channel_ctx(&msg.at.channel, now).await?;
                    ctxs.insert(msg.at.channel.clone(), ctx);
                }
                let Some(Some(ctx)) = ctxs.get(&msg.at.channel) else {
                    continue;
                };
                if let Some(t) = plan::plan(&msg, ctx, None, now)
                    .into_iter()
                    .find(|t| t.session == s)
                {
                    let w = NewWake {
                        session: s.clone(),
                        kind: WakeKind::Message,
                        source: source::message(&msg.id),
                        priority: t.priority,
                        not_before: now,
                        note: None,
                        origin: message_origin(&msg, &s),
                    };
                    self.port.enqueue(w).await?;
                }
            }
        }
        self.rescan().await
    }

    pub(crate) async fn set_captain(
        &mut self,
        crew: &str,
        session: &str,
    ) -> Result<(), ControlError> {
        let row = self
            .port
            .session(session)
            .await
            .map_err(ControlError::internal)?
            .filter(|r| r.crew_id == crew)
            .ok_or_else(|| ControlError::NotFound(format!("{crew}/{session}")))?;
        let prev = self
            .port
            .set_captain(crew, session)
            .await
            .map_err(ControlError::internal)?;
        let (now, lang) = (self.now(), self.lang());
        if let Some(l) = self.live.get_mut(session) {
            l.captain = true;
        }
        let mut tell = vec![(
            session.to_string(),
            text::get(lang, "captain.changed.new").to_string(),
        )];
        if let Some(p) = prev {
            if let Some(l) = self.live.get_mut(&p) {
                l.captain = false;
            }
            let name = safe_name(&row.name());
            tell.push((
                p,
                text::render(lang, "captain.changed.old", &[("name", &name)]),
            ));
        }
        let mut told = Vec::new();
        for (s, note) in tell {
            let w = NewWake {
                session: s.clone(),
                kind: WakeKind::System,
                source: source::system(&format!("captain:{crew}:{}", now.0)),
                priority: Priority::System,
                not_before: now,
                note: Some(note),
                origin: breaker::SYSTEM.into(),
            };
            self.port.enqueue(w).await.map_err(ControlError::internal)?;
            told.push(s);
        }
        // 换机长本身已成；附言都已入队，投不出去的留在队列里下次再试（评审 (3)）。
        self.deliver_each(&told).await;
        Ok(())
    }

    pub(crate) async fn recover(
        &mut self,
        scope: LatchScope,
        id: &str,
    ) -> Result<(), ControlError> {
        if !self.breaker.recover(scope, id) {
            return Err(ControlError::NotFound(id.to_string()));
        }
        self.port
            .clear_latch(scope, id)
            .await
            .map_err(ControlError::internal)?;
        self.rescan().await.map_err(ControlError::internal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::core::Cmd;
    use crate::orchestra::exec::BoxFut;
    use crate::orchestra::hooks::{Arbiter, AskAnsweredHook, Hooks, IdleHook};
    use crate::orchestra::testkit::{Rig, T0};
    use crewd_exec::delivery::Subject;
    use crewd_exec::health::Health;
    use crewd_exec::session::{ExitInfo, ExitStatusKind, SessionEvent};
    use crewd_term::{DecisionEvent, PendingDecision};
    use crewd_wake::breaker::{LOOP_LIMITS, edge_id};
    use crewd_wake::plan::{Listen, Mention};
    use crewd_wake::{HOUR, MIN, SEC, WakeState};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    const LOBBY: &str = "c1/lobby";

    /// 外来文字（C12 / C20）：换行、双向控制符、帧括号——过 `safe_name` 之后一样都不能剩。
    const HOSTILE: &str = "w1\n[cap]\u{202E}伪造";

    fn assert_neutralised(text: &str, raw: &str) {
        assert!(text.contains(&safe_name(raw)), "没有中和后的形状：{text}");
        assert!(!text.contains('\u{202E}'), "双向控制符漏进来了：{text}");
        assert!(!text.contains("\n[cap]"), "换行 + 帧括号漏进来了：{text}");
    }

    fn at(id: &str) -> Vec<Mention> {
        vec![Mention::Session(id.into())]
    }

    async fn crew_with(rig: &mut Rig, workers: &[&str], running: &[&str]) {
        rig.crew("c1", "cap");
        for w in workers {
            rig.worker("c1", w);
        }
        for s in running {
            rig.run(s).await;
        }
    }

    async fn fail_last(rig: &mut Rig, session: &str) {
        let id = rig.exec.last_send_id(session);
        let outcome = DeliveryOutcome::Undelivered("合成：没投进去".into());
        rig.emit(
            session,
            SessionEvent::Delivery {
                id,
                subject: Subject::Steer,
                outcome,
            },
        )
        .await;
    }

    fn rename(rig: &Rig, session: &str, name: &str) {
        rig.port
            .lock()
            .sessions
            .get_mut(session)
            .expect("先建 session")
            .display_name = Some(name.into());
    }

    #[tokio::test]
    async fn mention_wakes_an_idle_running_target_at_once() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        assert_eq!(rig.exec.sent("w1"), ["合成派活甲"]);
        let w = rig.port.wakes_of("w1");
        assert_eq!(
            (w[0].state, w[0].priority, w[0].source.as_str()),
            (WakeState::Inflight, Priority::Auto, "msg:c1/lobby#1")
        );
        assert_eq!(w[0].origin, "cap");
    }

    #[tokio::test]
    async fn busy_target_waits_for_the_idle_edge() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        assert!(rig.exec.sent("w1").is_empty());
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(rig.exec.sent("w1"), ["合成派活甲"]);
    }

    #[tokio::test]
    async fn already_injected_message_is_settled_not_resent() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        let r = rig
            .post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        // 忙时 hook 已把它注入上下文（M3b 推进同一个游标）。
        rig.port.advance("w1", &r.channel, r.seq).await.unwrap();
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert!(rig.exec.sent("w1").is_empty());
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Delivered);
    }

    #[tokio::test]
    async fn delivered_settles_and_advances_the_cursor() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        let r = rig
            .post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        rig.ack("w1").await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Delivered);
        assert_eq!(rig.port.cursor("w1", LOBBY).await.unwrap(), r.seq);
    }

    #[tokio::test]
    async fn deferred_rows_ride_the_batch_and_a_backlog_only_batch_still_goes_out() {
        // M3a R6：一个群一批只带最旧的 UNREAD_CAP 条。被叫的消息落在窗口外 = deferred：不进正文、随这批一起 claim（C24）、
        // 送到后放回 pending、绝不 dropped；窗口里只有更旧的积压时 wake_ids 为空、cursor_targets 非空——照样投；
        // 投完游标推到 cursor_targets，不是排队的最大 seq。
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        let r = rig
            .post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        let id = rig.port.wakes_of("w1")[0].id;
        {
            let mut st = rig.port.lock();
            st.defer.insert(id);
            st.backlog
                .insert("w1".into(), vec![(LOBBY.into(), r.seq - 1)]);
        }
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(rig.exec.sent("w1").len(), 1, "只有积压也投");
        assert_eq!(
            rig.port.wakes_of("w1")[0].state,
            WakeState::Inflight,
            "deferred 随批被拿走（C24）"
        );
        rig.ack("w1").await;
        assert_eq!(
            rig.port.wakes_of("w1")[0].state,
            WakeState::Pending,
            "送到后 deferred 放回队列里"
        );
        assert!(
            rig.port.cursor("w1", LOBBY).await.unwrap() < r.seq,
            "游标没越过被叫的那条"
        );
        {
            let mut st = rig.port.lock();
            st.defer.clear();
            st.backlog.clear();
        }
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(
            rig.exec.sent("w1").len(),
            2,
            "下一个 idle 边沿轮到被叫的那条"
        );
        rig.ack("w1").await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Delivered);
        assert_eq!(rig.port.cursor("w1", LOBBY).await.unwrap(), r.seq);
    }

    #[tokio::test]
    async fn failures_back_off_30s_then_2min_then_drop_with_one_alert() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        fail_last(&mut rig, "w1").await;
        let w = rig.port.wakes_of("w1")[0].clone();
        assert_eq!((w.state, w.attempts), (WakeState::Pending, 1));
        assert_eq!(w.not_before, T0.add(30 * SEC), "退避就是库里的 not_before");
        rig.tick(29 * SEC).await;
        assert_eq!(rig.exec.sent("w1").len(), 1, "退避没到不重投");
        rig.tick(SEC).await;
        assert_eq!(rig.exec.sent("w1").len(), 2);
        fail_last(&mut rig, "w1").await;
        assert_eq!(
            rig.port.wakes_of("w1")[0].not_before,
            T0.add(30 * SEC + 2 * MIN)
        );
        rig.tick(2 * MIN).await;
        fail_last(&mut rig, "w1").await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1, "{posts:?}");
        assert_eq!(
            posts[0].0,
            Audience::Lobby {
                crew: "c1".into(),
                mention_owner: true
            }
        );
    }

    #[tokio::test]
    async fn due_wakes_merge_into_one_delivery_of_the_highest_tier_only() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.post(LOBBY, Sender::Human, at("w1"), "人甲").await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "机长乙")
            .await;
        rig.post(LOBBY, Sender::Human, at("w1"), "人丙").await;
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(rig.exec.sent("w1"), ["人甲\n---\n人丙"]);
        rig.ack("w1").await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.emit("w1", SessionEvent::Working(false)).await;
        // 控制者裁定 C5（只改这一处断言）：人那一档投进去后游标推到 seq 3（M3a R6：推到批里实际带上的最大 seq），
        // 机长乙（seq 2）在窗口之内、已被看到——记 delivered，不再单独投一次。
        assert_eq!(rig.exec.sent("w1").len(), 1, "{:?}", rig.exec.sent("w1"));
        let w = rig.port.wakes_of("w1");
        assert_eq!(
            (w[1].origin.as_str(), w[1].state),
            ("cap", WakeState::Delivered)
        );
    }

    #[tokio::test]
    async fn stopped_session_is_resumed_by_a_mention_with_the_wake_as_opening_text() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &[]).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        assert_eq!(
            rig.exec.launches("w1")[0].initial.as_deref(),
            Some("合成派活甲")
        );
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Inflight);
        rig.ack_brief("w1").await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Delivered);
    }

    #[tokio::test]
    async fn stop_drops_pending_wakes_and_does_not_relaunch() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        rig.cmd(|reply| Cmd::Stop {
            session: "w1".into(),
            reply,
        })
        .await
        .unwrap();
        let info = ExitInfo {
            status: ExitStatusKind::Signal(15),
            stopped_by_user: true,
            hit_limit: false,
        };
        rig.emit("w1", SessionEvent::Exited(info)).await;
        rig.tick(MIN).await;
        assert_eq!(
            rig.exec.launches("w1").len(),
            1,
            "积压的唤醒不能把刚停掉的它拉起来"
        );
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活乙")
            .await;
        assert_eq!(
            rig.exec.launches("w1").len(),
            2,
            "新来的 @ 照样拉起它（§22.4）"
        );
    }

    #[tokio::test]
    async fn pending_decision_holds_delivery_until_cleared() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        let d = PendingDecision {
            prompt: "Trust?".into(),
            options: vec!["Yes".into()],
            numbered: true,
        };
        rig.emit("w1", SessionEvent::Decision(DecisionEvent::Appeared(d)))
            .await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        assert!(rig.exec.sent("w1").is_empty(), "绝不把正文敲进对话框");
        rig.emit("w1", SessionEvent::Decision(DecisionEvent::Cleared))
            .await;
        assert_eq!(rig.exec.sent("w1"), ["合成派活甲"]);
    }

    /// 机长与 w1 互相 @ 打转，直到某一轮 cap → w1 没送到。返回送到的轮数。
    async fn ping_pong_until_cut(rig: &mut Rig) -> usize {
        for round in 0..20 {
            rig.post(
                LOBBY,
                Rig::sender("cap"),
                at("w1"),
                &format!("合成派活 {round}"),
            )
            .await;
            if rig.exec.sent("w1").len() <= round {
                return round;
            }
            rig.ack("w1").await;
            rig.post(
                LOBBY,
                Rig::sender("w1"),
                at("cap"),
                &format!("合成回报 {round}"),
            )
            .await;
            rig.ack("cap").await;
        }
        panic!("20 轮都没截住");
    }

    #[tokio::test]
    async fn mutual_mention_loop_is_cut_on_one_edge_and_human_still_gets_through() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        assert_eq!(ping_pong_until_cut(&mut rig).await, LOOP_LIMITS.edge_max);
        let latches = rig.port.lock().latches.clone();
        assert_eq!(latches.len(), 1);
        // 控制者裁定 C21：边的台账键是 collab 的 `from→to`（U+2192），只经 `edge_id` 拼。
        assert_eq!(
            (latches[0].scope, latches[0].id.as_str()),
            (LatchScope::Edge, edge_id("cap", "w1").as_str())
        );
        let posts = rig.port.posts();
        assert!(
            posts
                .iter()
                .any(|(to, _)| *to == Audience::HumanChannel { crew: "c1".into() }),
            "告诉人"
        );
        assert!(
            posts.iter().any(|(to, _)| *to
                == Audience::Channel {
                    channel: LOBBY.into(),
                    mention_owner: true
                }),
            "告诉群主"
        );
        let before = rig.exec.sent("w1").len();
        rig.post(LOBBY, Sender::Human, at("w1"), "人来了").await;
        assert_eq!(rig.exec.sent("w1").len(), before + 1, "人的唤醒永不被挡");
        assert_eq!(
            rig.exec.sent("w1").last().map(String::as_str),
            Some("人来了")
        );
    }

    /// P2 / R12：熔断先落库、再在内存里生效。台账写不进去 → 这一次不熔断（内存与库一致：都没有），也不发通知；
    /// 那条唤醒留在 pending。写得进去了，下一拍照样判出来、落库、生效、通知。
    #[tokio::test]
    async fn a_trip_that_cannot_be_persisted_does_not_take_effect() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        rig.port.lock().latch_error = Some("合成：台账写不进去".into());
        assert_eq!(ping_pong_until_cut(&mut rig).await, LOOP_LIMITS.edge_max);
        assert!(rig.core.breaker.held("w1", "cap").is_none(), "内存没先生效");
        assert!(rig.port.lock().latches.is_empty());
        assert!(rig.port.posts().is_empty(), "{:?}", rig.port.posts());
        assert_eq!(
            rig.port.pending("w1").await.unwrap().len(),
            1,
            "没投的那条留着"
        );
        rig.port.lock().latch_error = None;
        rig.tick(SEC).await;
        assert!(rig.core.breaker.held("w1", "cap").is_some());
        assert_eq!(rig.port.lock().latches.len(), 1);
        assert!(!rig.port.posts().is_empty(), "落成了才通知");
    }

    #[tokio::test]
    async fn recover_clears_the_latch_and_delivers_what_was_held() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        ping_pong_until_cut(&mut rig).await;
        let before = rig.exec.sent("w1").len();
        let id = edge_id("cap", "w1");
        let i = id.clone();
        rig.cmd(|reply| Cmd::Recover {
            scope: LatchScope::Edge,
            id: i,
            reply,
        })
        .await
        .unwrap();
        assert!(rig.port.lock().latches.is_empty());
        assert_eq!(
            rig.exec.sent("w1").len(),
            before + 1,
            "被挡的那条在恢复后送到"
        );
        let again = rig
            .cmd(|reply| Cmd::Recover {
                scope: LatchScope::Edge,
                id,
                reply,
            })
            .await;
        assert!(matches!(again, Err(ControlError::NotFound(_))));
    }

    #[tokio::test]
    async fn thirty_sessions_working_normally_are_never_tripped() {
        let workers: Vec<String> = (1..=30).map(|i| format!("w{i}")).collect();
        let names: Vec<&str> = workers.iter().map(String::as_str).collect();
        let mut rig = Rig::new().await;
        let mut running = names.clone();
        running.push("cap");
        crew_with(&mut rig, &names, &running).await;
        for round in 0..3 {
            for w in &names {
                rig.post(
                    LOBBY,
                    Rig::sender("cap"),
                    at(w),
                    &format!("合成派活 {w} {round}"),
                )
                .await;
                rig.ack(w).await;
                rig.core
                    .handle(Input::Progress {
                        session: w.to_string(),
                    })
                    .await; // 工具调用（M3b 报）
                rig.post(
                    LOBBY,
                    Rig::sender(w),
                    at("cap"),
                    &format!("合成回报 {w} {round}"),
                )
                .await;
                rig.ack("cap").await;
            }
        }
        assert!(rig.port.lock().latches.is_empty());
        for w in &names {
            assert_eq!(rig.exec.sent(w).len(), 3, "{w}");
        }
        assert_eq!(rig.exec.sent("cap").len(), 90);
    }

    struct Picks(Vec<String>);
    impl Arbiter for Picks {
        fn wake_set<'a>(&'a self, _q: &'a ArbiterQuestion) -> BoxFut<'a, Option<Vec<String>>> {
            Box::pin(async move { Some(self.0.clone()) })
        }
    }

    struct Hangs;
    impl Arbiter for Hangs {
        fn wake_set<'a>(&'a self, _q: &'a ArbiterQuestion) -> BoxFut<'a, Option<Vec<String>>> {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn arbiter_answer_picks_who_wakes() {
        let mut rig = Rig::with(Hooks {
            arbiter: Arc::new(Picks(vec!["w2".into()])),
            ..Hooks::default()
        })
        .await;
        crew_with(&mut rig, &["w1", "w2"], &["cap", "w1", "w2"]).await;
        rig.post(LOBBY, Sender::Human, vec![], "合成随口一句").await;
        assert_eq!(rig.exec.sent("w2"), ["合成随口一句"]);
        assert!(rig.exec.sent("cap").is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn hanging_arbiter_times_out_to_the_owner() {
        let mut rig = Rig::with(Hooks {
            arbiter: Arc::new(Hangs),
            ..Hooks::default()
        })
        .await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        rig.post(LOBBY, Sender::Human, vec![], "合成随口一句").await;
        assert!(rig.exec.sent("cap").is_empty(), "还在等 Arbiter");
        tokio::time::advance(std::time::Duration::from_millis(60)).await;
        rig.settle().await;
        assert_eq!(
            rig.exec.sent("cap"),
            ["合成随口一句"],
            "超时 = 没答案 → 群主"
        );
    }

    struct OnceStopCheck(AtomicBool);
    impl IdleHook for OnceStopCheck {
        fn on_idle<'a>(&'a self, session: &'a str, now: Ms) -> BoxFut<'a, Option<NewWake>> {
            let fire = !self.0.swap(true, Ordering::SeqCst);
            Box::pin(async move {
                fire.then(|| NewWake {
                    session: session.into(),
                    kind: WakeKind::StopCheck,
                    source: source::system("sc-1"),
                    priority: Priority::System,
                    not_before: now,
                    note: Some("合成停下即检查".into()),
                    origin: "system".into(),
                })
            })
        }
    }

    #[tokio::test]
    async fn idle_hook_runs_only_when_the_queue_had_nothing_to_deliver() {
        let mut rig = Rig::with(Hooks {
            idle: Arc::new(OnceStopCheck(AtomicBool::new(false))),
            ..Hooks::default()
        })
        .await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(rig.exec.sent("w1"), ["合成停下即检查"]);
    }

    #[derive(Default)]
    struct RecordAsks(Mutex<Vec<String>>);
    impl AskAnsweredHook for RecordAsks {
        fn on_answered<'a>(
            &'a self,
            crew: &'a str,
            ask: &'a str,
            cleared: &'a [(String, Option<String>)],
            _now: Ms,
        ) -> BoxFut<'a, ()> {
            self.0
                .lock()
                .unwrap()
                .push(format!("{crew}/{ask} {cleared:?}"));
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn wakes_queued_by_others_are_delivered_and_answered_asks_reach_the_m4b_hook() {
        let asks = Arc::new(RecordAsks::default());
        let mut rig = Rig::with(Hooks {
            ask: asks.clone(),
            ..Hooks::default()
        })
        .await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        // M3a 的 asks::answer 在同一事务里入队叫醒提问者，再发 WakeEnqueued / AskAnswered。
        let w = NewWake {
            session: "w1".into(),
            kind: WakeKind::AskAnswer,
            source: "ask:c1/H1".into(),
            priority: Priority::Human,
            not_before: T0,
            note: None,
            origin: "human".into(),
        };
        rig.port.enqueue(w).await.unwrap();
        rig.core
            .handle(Input::Collab(CollabFact::WakeEnqueued {
                session: "w1".into(),
            }))
            .await;
        rig.settle().await;
        assert_eq!(rig.exec.sent("w1"), ["ask_answer:ask:c1/H1"]);
        let cleared = vec![("W3".to_string(), Some("w1".to_string()))];
        rig.core
            .handle(Input::Collab(CollabFact::AskAnswered {
                crew: "c1".into(),
                ask: "H1".into(),
                cleared,
            }))
            .await;
        rig.settle().await;
        assert_eq!(*asks.0.lock().unwrap(), [r#"c1/H1 [("W3", Some("w1"))]"#]);
    }

    #[tokio::test]
    async fn tool_output_between_wakes_counts_as_progress() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        for round in 0..(LOOP_LIMITS.edge_max + 4) {
            rig.post(
                LOBBY,
                Rig::sender("cap"),
                at("w1"),
                &format!("合成派活 {round}"),
            )
            .await;
            assert_eq!(rig.exec.sent("w1").len(), round + 1, "第 {round} 轮被截了");
            rig.ack("w1").await;
            // 两边在回话之间都调过工具（M3a `note_progress` → `Change::Progress` → `CollabFact::Progress`）。
            for s in ["w1", "cap"] {
                rig.core
                    .handle(Input::Collab(CollabFact::Progress { session: s.into() }))
                    .await;
            }
            rig.post(
                LOBBY,
                Rig::sender("w1"),
                at("cap"),
                &format!("合成回报 {round}"),
            )
            .await;
            rig.ack("cap").await;
        }
        assert!(rig.port.lock().latches.is_empty());
    }

    #[tokio::test]
    async fn quota_wake_clears_quota_health_before_delivering_and_holds_others_until_then() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit(
            "w1",
            SessionEvent::Health(Some(Health::new(
                crewd_exec::health::HealthKind::UsageLimit,
                "x",
            ))),
        )
        .await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        assert!(rig.exec.sent("w1").is_empty(), "额度没恢复不投");
        rig.tick(45 * MIN).await;
        assert_eq!(rig.exec.lock().cleared_quota, ["w1"]);
        assert_eq!(
            rig.exec.sent("w1"),
            [crewd_wake::text::get(
                crewd_wake::text::Lang::Zh,
                "quota.wake"
            )]
        );
    }

    #[tokio::test]
    async fn launch_failures_back_off_then_drop_with_one_alert() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &[]).await;
        rig.exec.lock().fail_next = vec!["合成：起不来".into(); 3];
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        rig.tick(30 * SEC).await;
        rig.tick(2 * MIN).await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        assert!(rig.port.row("w1").resume_failed_at.is_some());
        assert_eq!(
            rig.port.posts().len(),
            1,
            "只告警一次：{:?}",
            rig.port.posts()
        );
        rig.tick(10 * MIN).await;
        assert!(
            rig.exec.launches("w1").is_empty(),
            "三次都没起来，没有第四次"
        );
    }

    #[tokio::test]
    async fn wakes_for_an_unsupported_executor_are_dropped_with_one_alert() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &[]).await;
        rig.port.lock().sessions.get_mut("w1").unwrap().executor = "codex".into();
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        assert_eq!(rig.port.posts().len(), 1);
    }

    #[tokio::test]
    async fn set_captain_swaps_roles_and_tells_both() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        rig.cmd(|reply| Cmd::SetCaptain {
            crew: "c1".into(),
            session: "w1".into(),
            reply,
        })
        .await
        .unwrap();
        assert!(rig.port.row("w1").captain && !rig.port.row("cap").captain);
        assert!(rig.live("w1").unwrap().captain);
        assert_eq!(
            rig.exec.sent("w1"),
            [crewd_wake::text::get(
                crewd_wake::text::Lang::Zh,
                "captain.changed.new"
            )]
        );
        assert_eq!(rig.exec.sent("cap").len(), 1);
    }

    #[tokio::test]
    async fn lagged_subscription_replans_unread_mentions() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        // 消息落库了，但 MessageAppended 丢了（订阅落后）。
        rig.port.lock().messages.insert(
            (LOBBY.into(), 1),
            (
                MsgFacts {
                    id: "m-lag".into(),
                    at: MessageRef {
                        channel: LOBBY.into(),
                        seq: 1,
                    },
                    sender: Rig::sender("cap"),
                    mentions: at("w1"),
                    created_at: T0,
                },
                "合成派活甲".into(),
            ),
        );
        rig.core.handle(Input::Collab(CollabFact::Lagged)).await;
        rig.settle().await;
        assert_eq!(rig.exec.sent("w1"), ["合成派活甲"]);
    }

    // —— 以下是 Task 8 实现者补的（控制者裁定 / 评审 NOTE 各一条）

    /// C20 / C12：环路告警与换机长附言里的名字都是外来文字，过 `safe_name`。
    #[tokio::test]
    async fn loop_and_captain_notices_neutralise_session_names() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        rename(&rig, "w1", HOSTILE);
        rename(&rig, "cap", HOSTILE);
        ping_pong_until_cut(&mut rig).await;
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 2, "{posts:?}");
        let name = safe_name(HOSTILE);
        let count = (LOOP_LIMITS.edge_max + 1).to_string();
        let minutes = (LOOP_LIMITS.window_ms / MIN).to_string();
        let want_human = text::render(
            text::Lang::Zh,
            "loop.tripped.human",
            &[
                ("target", &name),
                ("origin", &name),
                ("count", &count),
                ("minutes", &minutes),
            ],
        );
        let want_owner = text::render(
            text::Lang::Zh,
            "loop.tripped.owner",
            &[("target", &name), ("origin", &name)],
        );
        assert_eq!(posts[0].1, want_human);
        assert_eq!(posts[1].1, want_owner);
        for (_, t) in &posts {
            assert_neutralised(t, HOSTILE);
        }
        rig.cmd(|reply| Cmd::SetCaptain {
            crew: "c1".into(),
            session: "w1".into(),
            reply,
        })
        .await
        .unwrap();
        let note = rig
            .port
            .wakes_of("cap")
            .into_iter()
            .find(|w| w.kind == WakeKind::System)
            .and_then(|w| w.note)
            .unwrap();
        assert_eq!(
            note,
            text::render(text::Lang::Zh, "captain.changed.old", &[("name", &name)])
        );
        assert_neutralised(&note, HOSTILE);
    }

    /// 系统来源的唤醒：没有产出地一直叫同一个 session，第 `edge_max + 1` 次熔断（`edge_max == target_max` 时
    /// 先撞上的是「system→目标」这条边——breaker 先查边）；告警里的来源是 `origin.system` 的话，告诉群主的那条发在
    /// 大厅（不是消息类唤醒）。
    #[tokio::test]
    async fn system_wakes_without_progress_trip_a_latch_naming_the_system() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        for i in 0..=LOOP_LIMITS.edge_max {
            let w = NewWake {
                session: "w1".into(),
                kind: WakeKind::System,
                source: source::system(&format!("合成-{i}")),
                priority: Priority::System,
                not_before: T0,
                note: Some(format!("合成系统 {i}")),
                origin: breaker::SYSTEM.into(),
            };
            rig.port.enqueue(w).await.unwrap();
            rig.core
                .handle(Input::Collab(CollabFact::WakeEnqueued {
                    session: "w1".into(),
                }))
                .await;
            rig.settle().await;
            if i < LOOP_LIMITS.edge_max {
                rig.ack("w1").await;
            }
        }
        assert_eq!(rig.exec.sent("w1").len(), LOOP_LIMITS.edge_max);
        let latches = rig.port.lock().latches.clone();
        assert_eq!(
            (latches[0].scope, latches[0].id.as_str()),
            (LatchScope::Edge, edge_id(breaker::SYSTEM, "w1").as_str())
        );
        let posts = rig.port.posts();
        let system = text::get(text::Lang::Zh, "origin.system");
        assert!(posts[0].1.contains(system), "{}", posts[0].1);
        assert_eq!(
            posts[1].0,
            Audience::Lobby {
                crew: "c1".into(),
                mention_owner: true
            }
        );
    }

    /// C19：放弃时告警对任何优先级都发（含没有 origin 的 system / timer 唤醒）。
    #[tokio::test]
    async fn a_system_wake_that_keeps_failing_gives_up_with_one_alert() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        let w = NewWake {
            session: "w1".into(),
            kind: WakeKind::System,
            source: source::system("合成"),
            priority: Priority::System,
            not_before: T0,
            note: Some("合成系统".into()),
            origin: breaker::SYSTEM.into(),
        };
        rig.port.enqueue(w).await.unwrap();
        rig.core
            .handle(Input::Collab(CollabFact::WakeEnqueued {
                session: "w1".into(),
            }))
            .await;
        rig.settle().await;
        fail_last(&mut rig, "w1").await;
        rig.tick(30 * SEC).await;
        fail_last(&mut rig, "w1").await;
        rig.tick(2 * MIN).await;
        fail_last(&mut rig, "w1").await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        assert_eq!(rig.port.posts().len(), 1, "{:?}", rig.port.posts());
    }

    /// C5：可选并发上限只挡自动拉起；人的 @ 照样把停着的 session 拉起来。
    #[tokio::test]
    async fn the_optional_cap_holds_auto_launches_but_never_human_ones() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1", "w2"], &["cap"]).await;
        rig.core.gate.set_max_live(Some(1));
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        assert!(rig.exec.launches("w1").is_empty(), "满了：自动唤醒等着");
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Pending);
        rig.post(LOBBY, Sender::Human, at("w2"), "合成人叫").await;
        assert_eq!(
            rig.exec.launches("w2")[0].initial.as_deref(),
            Some("合成人叫")
        );
    }

    /// Task 3 评审：`on_post` 的 `woke` 只算显式目标。w1 被机长反复叫醒、每次只发一条不 @ 谁的话（只有收听者 w2
    /// 被叫醒）——那不算产出，cap→w1 照样在 `edge_max` 次后熔断。
    #[tokio::test]
    async fn a_reply_that_only_wakes_a_listener_is_not_progress() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1", "w2"], &["cap", "w1", "w2"]).await;
        rig.port.lock().listens.insert(
            LOBBY.into(),
            vec![Listen {
                session: "w2".into(),
                until: T0.add(HOUR),
                senders: None,
            }],
        );
        let mut delivered = 0;
        for round in 0..20 {
            rig.post(
                LOBBY,
                Rig::sender("cap"),
                at("w1"),
                &format!("合成派活 {round}"),
            )
            .await;
            if rig.exec.sent("w1").len() <= round {
                break;
            }
            delivered += 1;
            rig.ack("w1").await;
            rig.post(
                LOBBY,
                Rig::sender("w1"),
                vec![],
                &format!("合成自语 {round}"),
            )
            .await;
        }
        assert_eq!(delivered, LOOP_LIMITS.edge_max);
        assert!(!rig.exec.sent("w2").is_empty(), "收听者确实被叫醒过");
    }

    /// M3a `build_batch` 遇到坏账本会 fail-loud：唤醒留在 pending、不投、不原地重试（只渲染了一次）；
    /// 修好之后下一个定时拍照常投。
    #[tokio::test]
    async fn a_render_error_leaves_the_wake_pending_without_a_tight_retry_loop() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.port.lock().render_error = Some("合成：坏账本".into());
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        assert!(rig.exec.sent("w1").is_empty());
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Pending);
        assert_eq!(rig.port.lock().renders, 1, "只试了一次");
        rig.port.lock().render_error = None;
        rig.tick(30 * SEC).await;
        assert_eq!(rig.exec.sent("w1"), ["合成派活甲"]);
    }

    /// C6：漏了 `RestartAnswered`（订阅落后）时，Lagged 从库里重推「答了恢复」：名单里的按机长在前拉起，
    /// 停在干活中途的带「被打断」附言。
    #[tokio::test]
    async fn lagged_after_an_answered_restart_prompt_resumes_the_held_sessions() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &[]).await;
        rig.port.lock().sessions.get_mut("w1").unwrap().was_working = true;
        rig.core.held.insert("w1".into());
        rig.core.held.insert("cap".into());
        rig.core.restart_prompt = Some(1);
        rig.core.handle(Input::Collab(CollabFact::Lagged)).await;
        rig.settle().await;
        assert!(rig.exec.lock().launches.is_empty(), "没答：都不起");
        rig.port.lock().restart_answers.insert(1, true);
        rig.core.handle(Input::Collab(CollabFact::Lagged)).await;
        rig.settle().await;
        let order: Vec<String> = rig
            .exec
            .lock()
            .launches
            .iter()
            .map(|l| l.session.clone())
            .collect();
        assert_eq!(order, ["cap", "w1"], "机长在前");
        assert_eq!(
            rig.exec.launches("w1")[0].initial.as_deref(),
            Some(text::get(text::Lang::Zh, "restart.interrupted"))
        );
        assert_eq!(rig.exec.launches("cap")[0].initial, None, "无附言的恢复");
        assert!(rig.core.held.is_empty());
    }

    // —— Task 8 评审修复（控制者裁定 (1)–(7)）

    fn crashed() -> SessionEvent {
        SessionEvent::Exited(ExitInfo {
            status: ExitStatusKind::Code(1),
            stopped_by_user: false,
            hit_limit: false,
        })
    }

    /// 让第一条唤醒（id 1）落在窗口外、窗口里只有更旧的积压：之后的批都是「只有积压」的（M3a R6）。
    fn defer_first_wake_behind_a_backlog(rig: &Rig, session: &str) {
        let mut st = rig.port.lock();
        st.defer.insert(1);
        st.backlog.insert(session.into(), vec![(LOBBY.into(), 0)]);
    }

    /// 裁定 (1)：只有积压的拉起，起来就死——带着的 deferred 唤醒照样记一次失败（退避），三次后放弃、告警一次；
    /// 不能因为「批里没有唤醒」就不记账、被退出时的重扫无限拉起。
    #[tokio::test]
    async fn a_backlog_only_launch_that_crashes_every_time_is_bounded_and_alerts_once() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &[]).await;
        defer_first_wake_behind_a_backlog(&rig, "w1");
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        assert_eq!(rig.exec.launches("w1").len(), 1, "只有积压也拉起");
        for _ in 0..10 {
            if rig.live("w1").is_some() {
                rig.emit("w1", crashed()).await;
            }
            rig.tick(3 * MIN).await;
        }
        assert_eq!(rig.exec.launches("w1").len(), queue::MAX_ATTEMPTS as usize);
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        assert_eq!(rig.port.posts().len(), 1, "{:?}", rig.port.posts());
    }

    /// 裁定 (1)：在跑的 session，只有积压的那次投递没投进去——deferred 唤醒退避、三次后放弃并告警一次，
    /// 不是每个定时拍重投一次。
    #[tokio::test]
    async fn a_failing_backlog_only_send_backs_off_and_gives_up_with_one_alert() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        defer_first_wake_behind_a_backlog(&rig, "w1");
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        assert_eq!(rig.exec.sent("w1").len(), 1);
        fail_last(&mut rig, "w1").await;
        let w = rig.port.wakes_of("w1")[0].clone();
        assert_eq!((w.state, w.attempts), (WakeState::Pending, 1));
        rig.tick(SEC).await;
        assert_eq!(rig.exec.sent("w1").len(), 1, "退避没到不重投");
        rig.tick(30 * SEC).await;
        fail_last(&mut rig, "w1").await;
        rig.tick(2 * MIN).await;
        fail_last(&mut rig, "w1").await;
        rig.tick(10 * MIN).await;
        assert_eq!(rig.exec.sent("w1").len(), queue::MAX_ATTEMPTS as usize);
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        assert_eq!(rig.port.posts().len(), 1, "{:?}", rig.port.posts());
    }

    /// 裁定 (2)：熔断只数**投进去了**的唤醒。渲染一直失败（什么都没投）重试再多次也不熔断。
    #[tokio::test]
    async fn a_persistent_render_error_never_trips_the_breaker() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.port.lock().render_error = Some("合成：坏账本".into());
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        for _ in 0..(LOOP_LIMITS.edge_max + 4) {
            rig.tick(30 * SEC).await;
        }
        assert!(rig.port.lock().renders > LOOP_LIMITS.edge_max);
        assert!(rig.port.lock().latches.is_empty());
        assert!(rig.port.posts().is_empty(), "{:?}", rig.port.posts());
    }

    /// 裁定 (2)：一条被一批批「只有积压」的投递带着走的 deferred 唤醒，不会每批都被记一次。
    #[tokio::test]
    async fn a_wake_carried_across_backlog_only_batches_is_not_counted_each_time() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        defer_first_wake_behind_a_backlog(&rig, "w1");
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        let rounds = LOOP_LIMITS.edge_max + 4;
        for _ in 0..rounds {
            rig.ack("w1").await;
            rig.emit("w1", SessionEvent::Working(true)).await;
            rig.emit("w1", SessionEvent::Working(false)).await;
        }
        assert_eq!(rig.exec.sent("w1").len(), rounds + 1);
        assert!(rig.port.lock().latches.is_empty());
    }

    /// 裁定 (3)：一条 @ 两个人，其中一个投递出错——另一个照样入队、照样投到。
    #[tokio::test]
    async fn one_targets_delivery_error_does_not_starve_the_others() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1", "w2"], &["w1", "w2"]).await;
        rig.port
            .lock()
            .render_error_for
            .insert("w1".into(), "合成：坏账本".into());
        let both = vec![Mention::Session("w1".into()), Mention::Session("w2".into())];
        rig.post(LOBBY, Rig::sender("cap"), both, "合成派活甲")
            .await;
        assert_eq!(rig.exec.sent("w2"), ["合成派活甲"]);
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Pending);
    }

    /// 裁定 (3)：换机长时新机长那条投不出去，原机长照样被告知，命令本身成功（唤醒都已入队）。
    #[tokio::test]
    async fn set_captain_tells_the_old_captain_even_if_the_new_ones_delivery_fails() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        rig.port
            .lock()
            .render_error_for
            .insert("w1".into(), "合成：坏账本".into());
        rig.cmd(|reply| Cmd::SetCaptain {
            crew: "c1".into(),
            session: "w1".into(),
            reply,
        })
        .await
        .unwrap();
        assert_eq!(rig.exec.sent("cap").len(), 1);
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Pending);
    }

    /// 裁定 (3)：答了「恢复」，排在前面的那个拉起出错，后面的照样拉起。
    #[tokio::test]
    async fn a_failing_resume_does_not_stop_the_rest_of_the_list() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &[]).await;
        rig.port.lock().sessions.get_mut("cap").unwrap().was_working = true;
        rig.port
            .lock()
            .render_error_for
            .insert("cap".into(), "合成：坏账本".into());
        rig.core.held.insert("w1".into());
        rig.core.held.insert("cap".into());
        rig.core.restart_prompt = Some(1);
        rig.port.lock().restart_answers.insert(1, true);
        rig.core.handle(Input::Collab(CollabFact::Lagged)).await;
        rig.settle().await;
        assert_eq!(rig.exec.launches("w1").len(), 1);
        assert!(rig.exec.launches("cap").is_empty());
    }

    /// 每次都给一条新的停下即检查（M4b 的形状：只要 idle 就想叫它一次）；数被调了几次。
    #[derive(Default)]
    struct AlwaysCheck(std::sync::atomic::AtomicUsize);
    impl IdleHook for AlwaysCheck {
        fn on_idle<'a>(&'a self, session: &'a str, now: Ms) -> BoxFut<'a, Option<NewWake>> {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Some(NewWake {
                    session: session.into(),
                    kind: WakeKind::StopCheck,
                    source: source::system(&format!("sc-{n}")),
                    priority: Priority::System,
                    not_before: now,
                    note: Some("合成停下即检查".into()),
                    origin: breaker::SYSTEM.into(),
                })
            })
        }
    }

    /// 裁定 (4)：人停掉时驱动层 abandon 发的 `Working(false)` 不是 idle 边沿——不问 IdleHook，
    /// 不然它入队的唤醒会在退出后的重扫里把刚停掉的 session 拉起来。
    #[tokio::test]
    async fn a_human_stop_does_not_run_the_idle_hook() {
        let hook = Arc::new(AlwaysCheck::default());
        let mut rig = Rig::with(Hooks {
            idle: hook.clone(),
            ..Hooks::default()
        })
        .await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.cmd(|reply| Cmd::Stop {
            session: "w1".into(),
            reply,
        })
        .await
        .unwrap();
        rig.tick(MIN).await;
        assert_eq!(hook.0.load(Ordering::SeqCst), 0);
        assert_eq!(rig.exec.launches("w1").len(), 1, "停掉的没被拉起来");
    }

    /// 裁定 (5)：`try_deliver` 四轮上限用完、队列里还有 pending 的唤醒时，IdleHook 不跑（它只管「队列空了」）。
    #[tokio::test]
    async fn the_idle_hook_waits_while_wakes_remain_after_the_round_cap() {
        let hook = Arc::new(AlwaysCheck::default());
        let mut rig = Rig::with(Hooks {
            idle: hook.clone(),
            ..Hooks::default()
        })
        .await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        let mut last = 0;
        for i in 0..41 {
            last = rig
                .post(
                    LOBBY,
                    Rig::sender("cap"),
                    at("w1"),
                    &format!("合成派活 {i}"),
                )
                .await
                .seq;
        }
        // 忙时 hook 已把它们全注入了：每轮只能把 10 条记 delivered，四轮后还剩一条。
        rig.port.advance("w1", LOBBY, last).await.unwrap();
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert!(!rig.port.pending("w1").await.unwrap().is_empty());
        assert_eq!(
            hook.0.load(Ordering::SeqCst),
            0,
            "还有 pending 就不问 IdleHook"
        );
        assert!(rig.exec.sent("w1").is_empty());
    }

    /// 裁定 (6)：正在被人停掉的 session，写队列不再往下写（退出时由 `on_exited` 统一结算）。
    #[tokio::test]
    async fn a_stopping_session_gets_no_more_writes() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        let (tx, _stopped) = tokio::sync::oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Stop {
                session: "w1".into(),
                reply: tx,
            }))
            .await;
        let (tx, reply) = tokio::sync::oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Nudge {
                session: "w1".into(),
                input: "继续".into(),
                reply: tx,
            }))
            .await;
        rig.settle().await;
        assert!(rig.exec.sent("w1").is_empty(), "{:?}", rig.exec.sent("w1"));
        assert_eq!(
            reply.await.unwrap(),
            Err(ControlError::NotRunning("w1".into()))
        );
    }

    /// 裁定 (7)：idle 边沿上的投递出错，`session.changed`（不再干活）照样发出去。
    #[tokio::test]
    async fn the_idle_edge_is_published_even_when_its_delivery_fails() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.port.lock().render_error = Some("合成：坏账本".into());
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        rig.port.lock().events.clear();
        rig.emit("w1", SessionEvent::Working(false)).await;
        let events = rig.port.lock().events.clone();
        assert!(
            events.iter().any(|e| matches!(e,
                OrchestraEvent::SessionChanged(v) if v.session_id == "w1" && !v.working)),
            "{events:?}"
        );
    }

    /// 裁定（评审 (5) 细化）：只有「此刻就投得出去」的 pending（到点、没被熔断挡着）才压住 IdleHook。
    /// 明天的 schedule_wakeup（没到点的定时唤醒）不能让今天的停下即检查失效。
    #[tokio::test]
    async fn a_future_timer_does_not_suppress_the_idle_hook() {
        let hook = Arc::new(AlwaysCheck::default());
        let mut rig = Rig::with(Hooks {
            idle: hook.clone(),
            ..Hooks::default()
        })
        .await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        let tomorrow = NewWake {
            session: "w1".into(),
            kind: WakeKind::Timer,
            source: source::timer("明天"),
            priority: Priority::Auto,
            not_before: T0.add(24 * HOUR),
            note: Some("合成明天的定时".into()),
            origin: "w1".into(),
        };
        rig.port.enqueue(tomorrow).await.unwrap();
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(hook.0.load(Ordering::SeqCst), 1);
        assert_eq!(rig.exec.sent("w1"), ["合成停下即检查"]);
    }

    /// 同上：被熔断挡着的 pending 也不压住 IdleHook（它要等人恢复，不是「队列里还有能投的」）。
    #[tokio::test]
    async fn a_held_wake_does_not_suppress_the_idle_hook() {
        let hook = Arc::new(AlwaysCheck::default());
        let mut rig = Rig::with(Hooks {
            idle: hook.clone(),
            ..Hooks::default()
        })
        .await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        ping_pong_until_cut(&mut rig).await;
        let calls = hook.0.load(Ordering::SeqCst);
        assert!(
            rig.port
                .pending("w1")
                .await
                .unwrap()
                .iter()
                .any(|w| w.origin == "cap"),
            "被挡的那条还在 pending"
        );
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(hook.0.load(Ordering::SeqCst), calls + 1);
    }

    // —— Task 8 复审修复（裁定 C23 / B / C）

    /// 人停掉的同时（驱动层的退出事件还没到）来了一条「没投进去」的回执：那批留给 `on_exited`（人停 → 丢弃），
    /// 不走 `fail_batch`。
    async fn undelivered_while_stopping(rig: &mut Rig, session: &str) {
        let generator = rig.live(session).unwrap().generator;
        let id = rig.exec.last_send_id(session);
        let (tx, _stopped) = tokio::sync::oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Stop {
                session: session.into(),
                reply: tx,
            }))
            .await;
        let outcome = DeliveryOutcome::Undelivered("合成：没投进去".into());
        let event = SessionEvent::Delivery {
            id,
            subject: Subject::Steer,
            outcome,
        };
        rig.core
            .handle(Input::Exec {
                session: session.into(),
                generator,
                event,
            })
            .await;
        rig.settle().await;
    }

    /// 评审 P7a：只有积压的批在路上时人停掉：deferred 那条随批被拿着（C24，`stop` 碰不到它），迟到的失败回执不能
    /// 把它记成「再试」——退出时整批跟人停一样丢掉。
    #[tokio::test]
    async fn a_failed_receipt_while_stopping_does_not_revive_dropped_deferred_rows() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        defer_first_wake_behind_a_backlog(&rig, "w1");
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        assert_eq!(rig.exec.sent("w1").len(), 1, "只有积压的那批在路上");
        undelivered_while_stopping(&mut rig, "w1").await;
        let w = rig.port.wakes_of("w1")[0].clone();
        assert_eq!(
            (w.state, w.attempts),
            (WakeState::Dropped, 1),
            "claim 记的那次留着"
        );
        rig.tick(3 * MIN).await;
        assert_eq!(rig.exec.launches("w1").len(), 1);
    }

    /// 评审 P7b：普通的批在路上时人停掉、随后回执说没投进去——那批跟人停一样丢掉，不重试、不把刚停掉的拉起来。
    #[tokio::test]
    async fn a_failed_receipt_while_stopping_drops_the_batch_instead_of_retrying() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        undelivered_while_stopping(&mut rig, "w1").await;
        let w = rig.port.wakes_of("w1")[0].clone();
        assert_eq!(
            (w.state, w.attempts),
            (WakeState::Dropped, 1),
            "claim 记的那次留着"
        );
        rig.tick(3 * MIN).await;
        assert_eq!(rig.exec.launches("w1").len(), 1, "人停的没被重新拉起");
        assert!(rig.port.posts().is_empty(), "{:?}", rig.port.posts());
    }

    /// 裁定 C：正在被停掉的 session 收到 nudge，当场回「没在跑」（不等 `on_exited`——它可能永远不来）。
    #[tokio::test]
    async fn a_nudge_to_a_stopping_session_is_refused_at_once() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        let (tx, _stopped) = tokio::sync::oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Stop {
                session: "w1".into(),
                reply: tx,
            }))
            .await;
        let (tx, mut reply) = tokio::sync::oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Nudge {
                session: "w1".into(),
                input: "继续".into(),
                reply: tx,
            }))
            .await;
        assert_eq!(
            reply.try_recv(),
            Ok(Err(ControlError::NotRunning("w1".into()))),
            "没处理退出事件之前就回了"
        );
    }

    /// 裁定 C：停掉时已经排在写队列里的 nudge（前面有一串键在写）也当场回「没在跑」。
    #[tokio::test]
    async fn queued_nudges_are_refused_when_the_stop_begins() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        // 第一步：一串键（"12"）在派生任务里写着，第二个 nudge 排在它后面。
        let (keys, _keys_done) = tokio::sync::oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Nudge {
                session: "w1".into(),
                input: "12".into(),
                reply: keys,
            }))
            .await;
        let (queued, mut queued_reply) = tokio::sync::oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Nudge {
                session: "w1".into(),
                input: "继续".into(),
                reply: queued,
            }))
            .await;
        // 第二步：人停掉——排着的那个当场回「没在跑」，不等退出事件。
        let (tx, _stopped) = tokio::sync::oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Stop {
                session: "w1".into(),
                reply: tx,
            }))
            .await;
        assert_eq!(
            queued_reply.try_recv(),
            Ok(Err(ControlError::NotRunning("w1".into())))
        );
    }

    // —— Task 8b（裁定 C24）：显式状态机、随批行一起 claim、库记次数、每批一次原子的 ack

    /// 人停掉的同时来了一条「送到了」的回执（驱动层的退出事件还没到）。
    async fn delivered_while_stopping(rig: &mut Rig, session: &str) {
        let generator = rig.live(session).unwrap().generator;
        let id = rig.exec.last_send_id(session);
        let (tx, _stopped) = tokio::sync::oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Stop {
                session: session.into(),
                reply: tx,
            }))
            .await;
        let event = SessionEvent::Delivery {
            id,
            subject: Subject::Steer,
            outcome: DeliveryOutcome::Delivered,
        };
        rig.core
            .handle(Input::Exec {
                session: session.into(),
                generator,
                event,
            })
            .await;
        rig.settle().await;
    }

    /// 复审 7(a)：停掉途中收到「送到了」——已经进了会话的照样记 delivered、游标照样推（不能因为随后的停掉当成没送到）。
    #[tokio::test]
    async fn a_delivered_receipt_while_stopping_still_settles_delivered() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        let r = rig
            .post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        delivered_while_stopping(&mut rig, "w1").await;
        let w = rig.port.wakes_of("w1")[0].clone();
        assert_eq!((w.state, w.attempts), (WakeState::Delivered, 1));
        assert_eq!(rig.port.cursor("w1", LOBBY).await.unwrap(), r.seq);
    }

    /// 同上，带随批行：人正在停它，随批行跟别的 pending 一样丢掉（不放回、不把刚停掉的拉起来）。
    #[tokio::test]
    async fn a_delivered_receipt_while_stopping_drops_the_riders() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        defer_first_wake_behind_a_backlog(&rig, "w1");
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        delivered_while_stopping(&mut rig, "w1").await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        rig.tick(3 * MIN).await;
        assert_eq!(rig.exec.launches("w1").len(), 1, "人停的没被重新拉起");
    }

    /// C24：窗口外的行随它的批一起被拿走（inflight），不是「看起来 pending」——`stop` 丢 pending 时碰不到它，
    /// 别的路径也选不中它；这批送到后它回 pending（退回 claim 那一次），下一批带上。
    #[tokio::test]
    async fn rider_rows_are_claimed_with_their_batch_and_released_when_it_lands() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        defer_first_wake_behind_a_backlog(&rig, "w1");
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        let w = rig.port.wakes_of("w1")[0].clone();
        assert_eq!(
            (w.state, w.attempts),
            (WakeState::Inflight, 1),
            "随批 = 被拿走"
        );
        assert!(rig.port.pending("w1").await.unwrap().is_empty());
        rig.ack("w1").await;
        let w = rig.port.wakes_of("w1")[0].clone();
        assert_eq!(
            (w.state, w.attempts),
            (WakeState::Pending, 0),
            "放回、这次不算"
        );
    }

    /// C24 / R2：放弃按**库里的**次数判，不按这批手里那份副本（Core 不自己 +1）。库里已经到上限的行，这一次没送到就放弃。
    #[tokio::test]
    async fn give_up_follows_the_attempts_the_store_counted() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        // 这批手里的副本记着第 1 次；库里已经是第 MAX_ATTEMPTS 次（别的地方记过，例如重启回收）。
        rig.port.lock().wakes[0].attempts = queue::MAX_ATTEMPTS;
        fail_last(&mut rig, "w1").await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        assert_eq!(rig.port.posts().len(), 1, "放弃告警一次");
    }

    /// C24 / R3：送到的确认、随批行、游标是一个事务。游标那一步出错 → 什么都没改：行仍是 inflight、游标没动，
    /// 环路熔断也不记（只在确认提交之后记）。
    #[tokio::test]
    async fn a_failed_ack_changes_nothing_and_records_no_loop_count() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        rig.port.lock().advance_error = Some("合成：游标写不进去".into());
        rig.ack("w1").await;
        let w = rig.port.wakes_of("w1")[0].clone();
        assert_eq!((w.state, w.attempts), (WakeState::Inflight, 1));
        assert_eq!(rig.port.cursor("w1", LOBBY).await.unwrap(), 0);
        assert_eq!(rig.core.breaker.streak_len("w1"), 0);
    }

    /// 一批没送到：进正文的与随批的**一次**记失败（全有或全无），各自按库里的次数退避。
    #[tokio::test]
    async fn a_failed_batch_fails_its_riders_in_the_same_ack() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活乙")
            .await;
        {
            let mut st = rig.port.lock();
            st.defer.insert(2);
        }
        rig.emit("w1", SessionEvent::Working(false)).await;
        let w = rig.port.wakes_of("w1");
        assert_eq!(
            w.iter().map(|w| w.state).collect::<Vec<_>>(),
            [WakeState::Inflight, WakeState::Inflight]
        );
        fail_last(&mut rig, "w1").await;
        for w in rig.port.wakes_of("w1") {
            assert_eq!((w.state, w.attempts), (WakeState::Pending, 1), "{w:?}");
            assert_eq!(w.not_before, T0.add(30 * SEC));
        }
    }

    /// P2 / R11：一起失败的不在同一刻一起重试。抖动来自注入的种子（测试确定）：每一批抽一次，±20% 以内。
    #[tokio::test]
    async fn simultaneous_failures_do_not_retry_in_lockstep() {
        let cfg = crate::orchestra::OrchestraConfig {
            retry_jitter: queue::RETRY_JITTER,
            jitter_seed: Some(42),
            ..crate::orchestra::OrchestraConfig::for_tests()
        };
        let mut rig = Rig::with_cfg(Hooks::default(), cfg).await;
        crew_with(&mut rig, &["w1", "w2"], &["w1", "w2"]).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲")
            .await;
        rig.post(LOBBY, Rig::sender("cap"), at("w2"), "合成派活乙")
            .await;
        fail_last(&mut rig, "w1").await;
        fail_last(&mut rig, "w2").await;
        let mut j = queue::Jitter::new(42, queue::RETRY_JITTER);
        let base = queue::RETRY_DELAYS_MS[0] as f64;
        let mut seen = Vec::new();
        for s in ["w1", "w2"] {
            let want = T0.add((base * (1.0 + j.draw())).round() as i64);
            let got = rig.port.wakes_of(s)[0].not_before;
            assert_eq!(got, want, "{s}");
            let d = got.since(T0);
            assert!((24 * SEC..=36 * SEC).contains(&d), "{s}: {d}");
            seen.push(got);
        }
        assert_ne!(seen[0], seen[1], "没有一起重试");
    }
}
