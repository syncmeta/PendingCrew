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
                self.try_deliver(&t.session).await?;
            }
        }
        Ok(())
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
            for w in pending.iter().filter(|w| queue::is_due(w, now)) {
                let backing_off = self.backoff.get(&w.id).is_some_and(|t| *t > now);
                if !backing_off && self.breaker.held(session, &w.origin).is_none() {
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

    /// 环路熔断（§22.7 Q5）：每个来源问一次；被挡的那几条留在 pending。
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
            let ok = match self.breaker.admit(session, o, now) {
                Verdict::Pass => true,
                Verdict::Held(_) => false,
                Verdict::Tripped(latch) => {
                    self.port.put_latch(&latch).await?;
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

    /// 渲染。渲染不出来的（Port 已记 dropped）拿掉；窗口外的（`deferred`，M3a R6）也拿掉，但**不碰**它们的状态——
    /// 它们仍是 pending，下一批带上，绝不 settle / dropped。超过 8KB 就从尾部减**非消息**的唤醒（减下的留在 pending）；
    /// 消息项的 8KB 只是建议值：它是一个群「游标之后、最旧的至多 `UNREAD_CAP` 条」的连续窗口，减一条消息行改变不了
    /// 窗口（会被重新算进来），也不能截一半（游标要推到窗口末尾），所以只剩消息项时照原样投。
    /// 返回 (这批真正投出去的唤醒, 正文, 投递成功后各群游标推到哪)。
    /// 渲染出错（M3a `build_batch` 遇到坏账本 fail-loud）原样往上抛：唤醒留在 pending，**不在这里重试**——
    /// 下一次定时拍 / idle 边沿再试（不紧循环）。
    async fn render_fitted(
        &self,
        session: &str,
        mut batch: Vec<QueuedWake>,
    ) -> anyhow::Result<(Vec<QueuedWake>, String, Vec<(String, i64)>)> {
        loop {
            let r = self.port.render_wake(session, &batch).await?;
            batch.retain(|w| !r.dropped.contains(&w.id) && !r.deferred.contains(&w.id));
            let trim = batch.iter().rposition(|w| w.kind != WakeKind::Message);
            match trim {
                Some(i) if r.text.len() > self.cfg.batch.max_bytes && batch.len() > 1 => {
                    batch.remove(i);
                }
                _ => return Ok((batch, r.text, r.cursor_targets)),
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
        let (batch, text, cursor_targets) = self.render_fitted(session, speak).await?;
        // 只有积压（M3a R6：窗口里全是比被叫消息更旧的未读，被叫的那几行 deferred）：`batch` 空而 `cursor_targets`
        // 非空——照样投，投完游标前进，下一批就轮到被叫的那几条。两样都空才是真没东西可投。
        if batch.is_empty() && cursor_targets.is_empty() {
            return Ok(false);
        }
        let batch_ids = ids(&batch);
        let accepts = self
            .live
            .get(session)
            .is_some_and(|l| l.handle.accepts_context());
        let extras = if accepts {
            self.hooks.extras.extras(session).await
        } else {
            Context::new()
        };
        if !batch_ids.is_empty() {
            self.port.settle(&batch_ids, Settle::Inflight, now).await?;
        }
        let Some(l) = self.live.get_mut(session) else {
            if !batch_ids.is_empty() {
                self.port.settle(&batch_ids, Settle::Requeue, now).await?;
            }
            return Ok(false);
        };
        l.inflight = Some(Inflight {
            delivery: None,
            wakes: batch,
            cursor_targets,
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
        let (batch, text, cursor_targets) = self.render_fitted(session, speak).await?;
        if batch.is_empty() && cursor_targets.is_empty() {
            return Ok(false);
        }
        let text = match interrupted {
            Some(note) => format!("{note}\n\n{text}"),
            None => text,
        };
        self.launch(row, exec, Some(text), batch, cursor_targets)
            .await?;
        Ok(true)
    }

    /// 投递回执（裁定 8）。非终局的（Blocked / NotReady / Stalled）驱动层自己会补投，等终局；`Abandoned`（停掉 /
    /// 退出时的 abandon）不结算 inflight——那批只在 `on_exited` 里结算（绑定规则，判据 [`delivery_settles_inflight`]）。
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
        let inf = l.inflight.take().expect("上面查过");
        if matches!(outcome, DeliveryOutcome::Delivered) {
            l.worked = true;
            if !inf.wakes.is_empty() {
                self.port
                    .settle(&ids(&inf.wakes), Settle::Delivered, now)
                    .await?;
            }
            // M3a R6：推到渲染时报回的 `cursor_targets`（批里实际带上的最大 seq），**不是**排队行里最大的 seq——
            // deferred 的那几条比它新，推过去就把它们越过、永远不投了。`advance` 只往前推。
            for (channel, seq) in &inf.cursor_targets {
                self.port.advance(session, channel, *seq).await?;
            }
            self.port.publish(OrchestraEvent::WakeChanged {
                session: session.into(),
            });
        } else {
            self.fail_batch(session, inf.wakes, &format!("{outcome:?}"))
                .await?;
        }
        Ok(())
    }

    /// idle 边沿（取代 `CrewSessionRunner.swift:490-541 runBecameIdle`）：①到期唤醒合并投递，投了就结束；
    /// ②`IdleHook`（M4b 的停下即检查）；③到此为止（机长 60s 检查点、continue_work、Todo sweep 都不再有）。
    pub(crate) async fn on_idle(&mut self, session: &str) -> anyhow::Result<()> {
        if self.try_deliver(session).await? {
            return Ok(());
        }
        let now = self.now();
        if let Some(w) = self.hooks.idle.on_idle(session, now).await
            && self.port.enqueue(w).await?
        {
            self.try_deliver(session).await?;
        }
        Ok(())
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
        for r in &ordered {
            self.try_deliver(&r.session).await?;
        }
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
            self.try_deliver(&s).await.map_err(ControlError::internal)?;
        }
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
    async fn deferred_rows_stay_pending_and_a_backlog_only_batch_still_goes_out() {
        // M3a R6：一个群一批只带最旧的 UNREAD_CAP 条。被叫的消息落在窗口外 = deferred：不进正文、不 settle、绝不 dropped；
        // 窗口里只有更旧的积压时 wake_ids 为空、cursor_targets 非空——照样投；投完游标推到 cursor_targets，不是排队的最大 seq。
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
            WakeState::Pending,
            "deferred 不进 inflight"
        );
        rig.ack("w1").await;
        assert_eq!(
            rig.port.wakes_of("w1")[0].state,
            WakeState::Pending,
            "deferred 留在队列里"
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
        assert_eq!(rig.core.backoff[&w.id], T0.add(30 * SEC));
        rig.tick(29 * SEC).await;
        assert_eq!(rig.exec.sent("w1").len(), 1, "退避没到不重投");
        rig.tick(SEC).await;
        assert_eq!(rig.exec.sent("w1").len(), 2);
        fail_last(&mut rig, "w1").await;
        assert_eq!(rig.core.backoff[&w.id], T0.add(30 * SEC + 2 * MIN));
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
}
