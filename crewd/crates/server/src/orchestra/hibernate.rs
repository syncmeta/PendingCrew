//! 休眠（C26）：人要重启电脑而 agent 还在干活时，让 crewd 有序地停下、之后能接着来。
//! 真正冻结进程做不到（macOS 没有检查点 / 恢复，API 连接也序列化不了）；对话本身已经落盘（claude 的 transcript /
//! codex 的 thread），所以休眠 = 一次「等安全点再停」的停机 + 一份记录 + 下次启动时的询问与附言。
//!
//! - 开始（[`Core::begin_hibernate`]）：关上停机闸（不投、不拉起、不起 / 恢复会话），先把记录落库；空闲的立刻
//!   [`Core::halt`]，正在起的起来就叫停；在干活的（或正有一批在投的）等到期限。
//! - 每条输入之后（[`Core::hibernate_step`]）：等着的那几个停在安全点（这一轮结束：不在干活、没有在投的批）就叫停；
//!   到了期限还没做完的先 `interrupt`（claude 一个 Esc / codex `turn/interrupt`）再叫停，记录里记「被打断」。
//! - 全部退出 → 回报结果，actor 随停机收尾；到了「期限 + 叫停宽限」还有没退出的 → 照实回报（`completed = false`），
//!   serve 再用 `Actor::abort_handle` 收尾（同 9a 的 `Handle::shutdown`）。
//! - 唤醒一条不丢：叫停用的是 [`Core::halt`]（不是人停）——pending 的原样留着，投到一半的退回 pending。
//! - 下次启动（`boot`）：重启询问标成「休眠」（`RestartReason::Hibernate`）；恢复时开场给休眠附言
//!   （[`Core::resume_note`]，`crewd_wake::policy::hibernate_note`），一个 session 只给一次；休眠时空闲的静默恢复。
//! - 客户端（macOS 关机 / 重启通知触发休眠）不在 crewd：这里只给 `Handle::hibernate`（RPC `crewd.hibernate` 由 T10 接）。
use std::collections::BTreeMap;

use crewd_collab::hibernate::Outcome;
use crewd_collab::render::safe_name;
use crewd_wake::policy::{self, HibernateFacts};
use crewd_wake::text;
use crewd_wake::{Ms, SEC};

use super::core::{ControlError, Core, Reply};
use super::port::*;

/// 一次休眠的结果（`Handle::hibernate` 的回执；RPC `crewd.hibernate` 的结果，T10 接线）。各名单按 session id 排序。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HibernateReport {
    /// 在安全点停下的：空闲的、期限内做完这一轮的、正在起被叫停的。
    pub clean: Vec<String>,
    /// 到了期限还在干活、被打断之后停下的（恢复后要重跑被打断的命令）。
    pub interrupted: Vec<String>,
    /// 到「期限 + 叫停宽限」还没退出的（不论有没有被打断）。
    pub failed: Vec<String>,
    /// 全部退出了、actor 收尾了（= `Handle::shutdown` 的 true）。false 时 serve 用 `Actor::abort_handle` 收尾。
    pub completed: bool,
}

/// 休眠进行中（[`Core::hibernation`]）。
pub(crate) struct Hibernation {
    /// 在干活的最多等到这一刻（之后打断）。
    pub(crate) deadline: Ms,
    /// 到这一刻还没退出的就照实回报（期限 + `OrchestraConfig::hibernate_stop_grace`）。
    pub(crate) give_up: Ms,
    /// 等结果的（第二次休眠并进来，同一份结果）。
    pub(crate) waiters: Vec<Reply<HibernateReport>>,
    /// 开始时在跑 / 在起的 session。
    pub(crate) tracked: BTreeMap<String, Track>,
    /// 这次休眠的记录（落库的那一份）。
    pub(crate) record: HibernateRecord,
    /// 结果已经发出去了。
    pub(crate) reported: bool,
    /// 发出去的那份结果：回报之后再来的休眠当场回它（评审 m2）。
    pub(crate) report: Option<HibernateReport>,
}

/// 到期限按了 Esc 之后，最多等多久让它自己停下这一轮（`Working(false)`）再叫停（评审 m1）；叫停宽限剩得更少时等到宽限为止。
pub(crate) const INTERRUPT_SETTLE_MS: i64 = 5 * SEC;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Track {
    /// 还在等它到达安全点。
    pub(crate) busy: bool,
    /// 到期限被打断了。
    pub(crate) interrupted: bool,
    /// 按 Esc 的时刻：从这一刻起等它停下这一轮，至多 [`INTERRUPT_SETTLE_MS`]，然后叫停。
    pub(crate) cut_at: Option<Ms>,
}

impl<P: Port> Core<P> {
    /// 开始休眠（C26）。`deadline_ms` = 在干活的最多等多久；None = 设置里的（没设 = 120 秒）；0 = 立即休眠。
    /// 已在休眠：并进那一次（同一份结果），期限只会提前、不会推后。普通停机进行中：拒绝（`ShuttingDown`）。
    pub(crate) async fn begin_hibernate(
        &mut self,
        deadline_ms: Option<i64>,
        reply: Reply<HibernateReport>,
    ) {
        let now = self.now();
        let grace = self.cfg.hibernate_stop_grace.as_millis() as i64;
        if let Some(h) = self.hibernation.as_mut() {
            if let Some(r) = &h.report {
                // 已经回报过了（actor 在收尾 / 等 serve 中止）：当场回那份，不留永远等不到的回执（m2）。
                let _ = reply.send(Ok(r.clone()));
                return;
            }
            if let Some(ms) = deadline_ms {
                h.deadline = h.deadline.min(now.add(ms.max(0)));
                h.give_up = h.give_up.min(h.deadline.add(grace));
            }
            h.waiters.push(reply);
            let (deadline, give_up) = (h.deadline, h.give_up);
            self.timers.extend([deadline, give_up]);
            return;
        }
        if self.shutting_down {
            let _ = reply.send(Err(ControlError::ShuttingDown));
            return;
        }
        let ms = match deadline_ms {
            Some(ms) => ms.max(0),
            None => {
                let secs = self
                    .port
                    .hibernate_deadline_secs()
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("crewd: 编排：读休眠期限的设置失败（按默认值）：{e:#}");
                        None
                    });
                i64::from(secs.unwrap_or(policy::DEFAULT_HIBERNATE_DEADLINE_SECS)) * 1000
            }
        };
        // 停机闸：从此刻起不投、不拉起、不起 / 恢复会话（同停机，C4：闸不随回执打开）。
        self.shutting_down = true;
        let mut tracked = BTreeMap::new();
        let mut entries = Vec::new();
        for (s, l) in &self.live {
            let busy = l.working || l.inflight.is_some();
            let doing = l
                .inflight
                .as_ref()
                .map(|f| f.batch.wakes.clone())
                .filter(|w| !w.is_empty())
                .unwrap_or_else(|| l.last_wakes.clone());
            tracked.insert(
                s.clone(),
                Track {
                    busy,
                    ..Track::default()
                },
            );
            entries.push(entry(s, busy, &doing));
        }
        for (s, st) in &self.starting {
            tracked.insert(s.clone(), Track::default());
            entries.push(entry(s, false, &st.batch.wakes));
        }
        entries.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        // 上次休眠还没恢复、仍在待恢复名单里的（一直在等人答、这次没在跑）：带进新记录，下次照样给休眠附言。
        // 不在名单里的（人停过 / 已不算待恢复）不带（评审 m4）。
        if let Some(old) = &self.hibernated {
            for h in &old.sessions {
                if !h.resumed
                    && !tracked.contains_key(&h.session_id)
                    && self.held.contains(&h.session_id)
                {
                    entries.push(h.clone());
                }
            }
        }
        let record = HibernateRecord {
            at_ms: now.0,
            sessions: entries,
            ..Default::default()
        };
        // 先落库、再叫停：休眠中途 crewd 被杀（电脑真的关了），下次启动也知道这些是休眠停下的。
        if let Err(e) = self.port.put_hibernate(&record).await {
            eprintln!("crewd: 编排：休眠记录写不进去（照样休眠，下次启动按普通重启问）：{e:#}");
        }
        let idle: Vec<String> = tracked
            .iter()
            .filter(|(s, t)| !t.busy && self.live.contains_key(*s))
            .map(|(s, _)| s.clone())
            .collect();
        for s in idle {
            self.halt(&s);
        }
        for st in self.starting.values_mut() {
            st.halt_requested = true;
        }
        let deadline = now.add(ms);
        let give_up = deadline.add(grace);
        self.timers.extend([deadline, give_up]);
        self.hibernation = Some(Hibernation {
            deadline,
            give_up,
            waiters: vec![reply],
            tracked,
            record,
            reported: false,
            report: None,
        });
    }

    /// 休眠中停机（`Input::Shutdown`）：不另起一套叫停，把期限提前到现在（在干活的照样先打断、记进记录）。
    pub(crate) fn hibernate_now(&mut self) -> bool {
        let now = self.now();
        let grace = self.cfg.hibernate_stop_grace.as_millis() as i64;
        let Some(h) = self.hibernation.as_mut() else {
            return false;
        };
        h.deadline = h.deadline.min(now);
        h.give_up = h.give_up.min(h.deadline.add(grace));
        let give_up = h.give_up;
        self.timers.insert(give_up);
        true
    }

    /// 每条输入之后跑一次（[`Core::handle`]）：到了安全点的叫停、到期限的打断再叫停、全部退出或到了放弃时刻就回报。
    pub(crate) async fn hibernate_step(&mut self) {
        let now = self.now();
        let Some(h) = self.hibernation.as_ref() else {
            return;
        };
        if h.reported {
            return;
        }
        let (mut safe, mut cut) = (Vec::new(), Vec::new());
        for (s, _) in h.tracked.iter().filter(|(_, t)| t.busy) {
            match self.live.get(s) {
                // 自己没了（崩了 / 被人停了）：不再等它。
                None => safe.push((s.clone(), false)),
                Some(l) if l.stopping || l.halting => safe.push((s.clone(), false)),
                Some(l) if !l.working && l.inflight.is_none() => safe.push((s.clone(), true)),
                Some(_) if now >= h.deadline => cut.push(s.clone()),
                Some(_) => {}
            }
        }
        let h = self.hibernation.as_mut().expect("上面查过");
        let mut changed = false;
        for (s, at_safe_point) in &safe {
            if let Some(t) = h.tracked.get_mut(s) {
                t.busy = false;
            }
            // 到了安全点才记「做完了」；自己没了 / 被别人停了的留在 Pending（恢复时按被打断说，I3）。
            if *at_safe_point
                && let Some(e) = h.record.sessions.iter_mut().find(|e| &e.session_id == s)
            {
                e.outcome = Outcome::Finished;
                changed = true;
            }
        }
        for s in &cut {
            if let Some(t) = h.tracked.get_mut(s) {
                t.busy = false;
                t.interrupted = true;
            }
            if let Some(e) = h.record.sessions.iter_mut().find(|e| &e.session_id == s) {
                e.outcome = Outcome::Interrupted;
                changed = true;
            }
        }
        let record = changed.then(|| h.record.clone());
        for (s, halt) in safe {
            if halt {
                self.halt(&s);
            }
        }
        let give_up = self.hibernation.as_ref().map_or(now, |h| h.give_up);
        // 按过 Esc 之后至多等 min(5 秒, 到放弃之前剩下的一半)：另一半留给叫停之后的退出（9b 复审）——等满到放弃那一刻
        // 才叫停，它的退出就来不及在放弃之前到，白白报成没停下。
        let settle_until = |at: Ms| at.add(INTERRUPT_SETTLE_MS.min(give_up.since(at) / 2).max(0));
        for s in &cut {
            // 先按 Esc 打断这一轮，给它一点时间自己停下（m1），再叫停（下面那一段）。等的这几秒里驱动层发来的
            // `Working(false)` 不抹掉 was_working（`Live::hibernate_cut`）：它是被打断的。
            if let Some(l) = self.live.get_mut(s) {
                l.hibernate_cut = true;
                let _ = l.handle.interrupt();
            }
            if let Some(t) = self.hibernation.as_mut().and_then(|h| h.tracked.get_mut(s)) {
                t.cut_at = Some(now);
            }
            self.timers.insert(settle_until(now));
        }
        // 按过 Esc 的：这一轮停下了（不在干活）或等够了 → 叫停。
        let to_halt: Vec<String> = self
            .hibernation
            .as_ref()
            .map(|h| {
                h.tracked
                    .iter()
                    .filter_map(|(s, t)| t.cut_at.map(|at| (s, at)))
                    .filter(|(s, at)| {
                        self.live.get(*s).is_some_and(|l| {
                            !l.halting && !l.stopping && (!l.working || now >= settle_until(*at))
                        })
                    })
                    .map(|(s, _)| s.clone())
                    .collect()
            })
            .unwrap_or_default();
        for s in to_halt {
            self.halt(&s);
        }
        if let Some(r) = record
            && let Err(e) = self.port.put_hibernate(&r).await
        {
            eprintln!("crewd: 编排：休眠记录（这一轮的结局）写不进去：{e:#}");
        }
        if self.live.is_empty() && self.starting.is_empty() && self.preparing == 0 {
            self.finish_hibernate(true);
        } else if self.hibernation.as_ref().is_some_and(|h| now >= h.give_up) {
            self.finish_hibernate(false);
        }
    }

    /// 回报休眠结果（只一次）。`completed` = 全部退出了。
    pub(crate) fn finish_hibernate(&mut self, completed: bool) {
        let Some(h) = self.hibernation.as_mut() else {
            return;
        };
        if h.reported {
            return;
        }
        h.reported = true;
        let mut report = HibernateReport {
            completed,
            ..Default::default()
        };
        for (s, t) in &h.tracked {
            let alive = self.live.contains_key(s) || self.starting.contains_key(s);
            let to = if alive {
                &mut report.failed
            } else if t.interrupted {
                &mut report.interrupted
            } else {
                &mut report.clean
            };
            to.push(s.clone());
        }
        for w in h.waiters.drain(..) {
            let _ = w.send(Ok(report.clone()));
        }
        h.report = Some(report);
    }

    /// 恢复一个等人答复的 session 时开场的附言（§22.7 Q1 / C26）：被休眠停下、还没恢复过的 → 休眠附言（只给一次：
    /// 记录里记下已恢复；休眠时空闲的没有附言、静默恢复，评审 I1——同样记下已恢复）；否则上次停在干活中途的 →
    /// 崩溃重启的「被打断」；否则没有。
    pub(crate) async fn resume_note(&mut self, session: &str, was_working: bool) -> Option<String> {
        let lang = self.lang();
        let found = self.hibernated.as_mut().and_then(|r| {
            r.sessions
                .iter_mut()
                .find(|h| h.session_id == session && !h.resumed)
        });
        let Some(h) = found else {
            return was_working.then(|| text::get(lang, "restart.interrupted").to_string());
        };
        h.resumed = true;
        // 停下时在做什么是外来文字（唤醒附言、群 id）：过 safe_name（C20）。
        let doing = h.doing.as_deref().map(safe_name);
        let note = policy::hibernate_note(
            lang,
            &HibernateFacts {
                was_working: h.was_working,
                interrupted: h.interrupted(),
                doing: doing.as_deref(),
            },
        );
        let record = self.hibernated.clone().expect("上面找到过");
        if let Err(e) = self.port.put_hibernate(&record).await {
            eprintln!("crewd: 编排：休眠记录（已恢复）写不进去：{e:#}");
        }
        note
    }

    /// 这次启动要不要把重启询问标成休眠：待恢复名单里有休眠停下、还没恢复过的。
    pub(crate) fn restart_reason(&self) -> RestartReason {
        let hibernated = self.hibernated.as_ref().is_some_and(|r| {
            r.sessions
                .iter()
                .any(|h| !h.resumed && self.held.contains(&h.session_id))
        });
        if hibernated {
            RestartReason::Hibernate
        } else {
            RestartReason::Restart
        }
    }
}

/// 开始时的一项：在干活的记 `Pending`（等安全点；中途 crewd 没了就一直是它，按被打断算，I3），空闲的记 `Finished`。
fn entry(session: &str, was_working: bool, doing: &[crewd_wake::QueuedWake]) -> Hibernated {
    Hibernated {
        session_id: session.to_string(),
        was_working,
        outcome: if was_working {
            Outcome::Pending
        } else {
            Outcome::Finished
        },
        doing: policy::batch_summary(doing),
        last_wakes: doing.iter().map(|w| w.id).collect(),
        resumed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::OrchestraConfig;
    use crate::orchestra::core::{Cmd, CollabFact, ControlError, Input, Origin, StartSession};
    use crate::orchestra::exec::Executors;
    use crate::orchestra::hooks::Hooks;
    use crate::orchestra::testkit::{Rig, T0};
    use crewd_collab::hibernate::Outcome;
    use crewd_collab::render::safe_name;
    use crewd_exec::session::SessionEvent;
    use crewd_wake::plan::{Mention, MsgFacts, Sender};
    use crewd_wake::{
        HOUR, MIN, MessageRef, NewWake, Priority, SEC, WakeKind, WakeState, source, text,
    };
    use tokio::sync::oneshot;

    type Pending = oneshot::Receiver<Result<HibernateReport, ControlError>>;

    /// 外来文字（C20）：换行、双向控制符、帧括号——过 `safe_name` 之后一样都不能剩。
    const HOSTILE: &str = "timer timer:t9: 合成\n[cap]\u{202E}伪造";

    async fn hibernate(rig: &mut Rig, deadline_ms: Option<i64>) -> Pending {
        let (tx, rx) = oneshot::channel();
        rig.core
            .handle(Input::Cmd(Cmd::Hibernate {
                deadline_ms,
                reply: tx,
            }))
            .await;
        rig.settle().await;
        rx
    }

    fn report(rx: &mut Pending) -> HibernateReport {
        rx.try_recv()
            .expect("休眠应该已经有结果了")
            .expect("休眠不该出错")
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn timer(session: &str, token: &str, note: &str) -> NewWake {
        NewWake {
            session: session.into(),
            kind: WakeKind::Timer,
            source: source::timer(token),
            priority: Priority::Auto,
            not_before: T0,
            note: Some(note.into()),
            origin: session.into(),
        }
    }

    async fn enqueue(rig: &mut Rig, w: NewWake) {
        let session = w.session.clone();
        rig.port.enqueue(w).await.unwrap();
        rig.core
            .handle(Input::Collab(CollabFact::WakeEnqueued { session }))
            .await;
        rig.settle().await;
    }

    fn entry(rig: &Rig, id: &str) -> Hibernated {
        rig.port
            .lock()
            .hibernate
            .clone()
            .expect("休眠记录要落库")
            .sessions
            .into_iter()
            .find(|h| h.session_id == id)
            .unwrap_or_else(|| panic!("记录里没有 {id}"))
    }

    async fn three(rig: &mut Rig) {
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.worker("c1", "w2");
        for s in ["cap", "w1", "w2"] {
            rig.run(s).await;
        }
    }

    /// C26：空闲的立刻停；在干活的等到期限——期限内做完这一轮的在安全点停下，到期限还没做完的先打断再停。
    /// 期间不起、不投；desired 不动（下次启动照样问人），被打断的 was_working 留着。
    #[tokio::test]
    async fn idle_sessions_stop_at_once_and_working_ones_get_until_the_deadline() {
        let mut rig = Rig::new().await;
        three(&mut rig).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.emit("w2", SessionEvent::Working(true)).await;
        let mut rx = hibernate(&mut rig, Some(2 * MIN)).await;
        {
            let puts = &rig.port.lock().hibernate_puts;
            assert_eq!(puts.len(), 1, "开始时就先落库（休眠中途被关机也知道）");
            let first: Vec<&str> = puts[0]
                .sessions
                .iter()
                .map(|h| h.session_id.as_str())
                .collect();
            assert_eq!(first, ["cap", "w1", "w2"]);
        }
        assert_eq!(rig.exec.lock().stops, ids(&["cap"]), "空闲的立刻停");
        assert!(rx.try_recv().is_err(), "还在等 w1 / w2");
        let r = rig
            .cmd(|reply| Cmd::Resume {
                session: "cap".into(),
                origin: Origin::Human,
                reply,
            })
            .await;
        assert_eq!(r, Err(ControlError::ShuttingDown), "休眠中不起会话");
        enqueue(&mut rig, timer("cap", "t-late", "合成：休眠中来的定时")).await;
        assert_eq!(rig.exec.launches("cap").len(), 1, "休眠中不拉起");
        rig.clock.advance(30 * SEC);
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(
            rig.exec.lock().stops,
            ids(&["cap", "w1"]),
            "w1 做完这一轮：在安全点停下"
        );
        assert!(rig.exec.lock().interrupts.is_empty());
        rig.tick(MIN).await;
        assert!(rig.exec.lock().interrupts.is_empty(), "还没到期限");
        assert!(rx.try_recv().is_err());
        rig.tick(30 * SEC).await;
        assert_eq!(rig.exec.lock().interrupts, ids(&["w2"]), "到期限：打断");
        assert_eq!(
            rig.exec.lock().writes["w2"].last().map(String::as_str),
            Some("interrupt"),
            "先打断，再叫停"
        );
        assert_eq!(rig.exec.lock().stops, ids(&["cap", "w1", "w2"]));
        assert_eq!(
            report(&mut rx),
            HibernateReport {
                clean: ids(&["cap", "w1"]),
                interrupted: ids(&["w2"]),
                failed: vec![],
                completed: true,
            }
        );
        assert!(rig.core.shutdown_done(), "休眠完了 actor 就收尾");
        for (s, working) in [("cap", false), ("w1", false), ("w2", true)] {
            let row = rig.port.row(s);
            assert_eq!(
                (row.desired, row.was_working),
                (Desired::Running, working),
                "{s}"
            );
        }
        let (cap, w1, w2) = (entry(&rig, "cap"), entry(&rig, "w1"), entry(&rig, "w2"));
        assert_eq!((cap.was_working, cap.outcome), (false, Outcome::Finished));
        assert_eq!(
            (w1.was_working, w1.outcome),
            (true, Outcome::Finished),
            "到了安全点：记「做完了」"
        );
        assert_eq!((w2.was_working, w2.outcome), (true, Outcome::Interrupted));
        assert_eq!(rig.port.lock().hibernate.as_ref().unwrap().at_ms, T0.0);
        assert_eq!(rig.port.wakes_of("cap")[0].state, WakeState::Pending);
    }

    /// 「立即休眠」= 期限 0：在干活的当场打断。
    #[tokio::test]
    async fn deadline_zero_interrupts_working_sessions_at_once() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("cap").await;
        rig.run("w1").await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        let mut rx = hibernate(&mut rig, Some(0)).await;
        assert_eq!(rig.exec.lock().interrupts, ids(&["w1"]));
        assert_eq!(
            report(&mut rx),
            HibernateReport {
                clean: ids(&["cap"]),
                interrupted: ids(&["w1"]),
                failed: vec![],
                completed: true,
            }
        );
    }

    /// 没给期限 = 设置里的（没设 = 120 秒，crewd-wake 的常量）。
    #[tokio::test]
    async fn the_deadline_defaults_to_the_setting_then_to_two_minutes() {
        for (setting, wait) in [(None, 2 * MIN), (Some(10), 10 * SEC)] {
            let mut rig = Rig::new().await;
            rig.port.lock().hibernate_deadline = setting;
            rig.crew("c1", "cap");
            rig.run("cap").await;
            rig.emit("cap", SessionEvent::Working(true)).await;
            let mut rx = hibernate(&mut rig, None).await;
            rig.tick(wait - 1).await;
            assert!(rig.exec.lock().interrupts.is_empty(), "{setting:?}");
            rig.tick(1).await;
            assert_eq!(rig.exec.lock().interrupts, ids(&["cap"]), "{setting:?}");
            assert_eq!(report(&mut rx).interrupted, ids(&["cap"]));
        }
    }

    /// 休眠不是人停：pending 的唤醒原样留着、投到一半的（inflight）退回 pending，正在起的那批也退回——一条都不丢
    /// （同 `Core::halt`）。记录里记下各自停下时在处理哪批。
    #[tokio::test]
    async fn wakes_are_requeued_never_dropped_and_the_record_says_what_each_was_doing() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.worker("c1", "w2");
        rig.worker("c1", "w3");
        rig.run("w1").await;
        rig.run("w2").await;
        enqueue(&mut rig, timer("w1", "t1", "合成定时甲")).await;
        rig.ack("w1").await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        enqueue(&mut rig, timer("w1", "t2", "合成定时乙")).await; // w1 在忙：排着
        enqueue(&mut rig, timer("w2", "t3", "合成定时丙")).await; // 投出去了、还没回执
        rig.port
            .enqueue(timer("w3", "t4", "合成定时丁"))
            .await
            .unwrap();
        rig.core
            .handle(Input::Collab(CollabFact::WakeEnqueued {
                session: "w3".into(),
            }))
            .await;
        assert!(rig.core.starting.contains_key("w3"), "w3 还在起");
        let mut rx = hibernate(&mut rig, Some(0)).await;
        let r = report(&mut rx);
        assert!(r.completed, "{r:?}");
        assert_eq!(r.interrupted, ids(&["w1", "w2"]), "投到一半的也算在忙");
        assert_eq!(r.clean, ids(&["w3"]));
        let state = |s: &str| {
            rig.port
                .wakes_of(s)
                .into_iter()
                .map(|w| (w.source, w.state, w.attempts))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            state("w1"),
            [
                ("timer:t1".to_string(), WakeState::Delivered, 1),
                ("timer:t2".to_string(), WakeState::Pending, 0)
            ]
        );
        assert_eq!(
            state("w2"),
            [("timer:t3".to_string(), WakeState::Pending, 0)],
            "退回，不算失败"
        );
        assert_eq!(
            state("w3"),
            [("timer:t4".to_string(), WakeState::Pending, 0)]
        );
        let (w1, w2, w3) = (entry(&rig, "w1"), entry(&rig, "w2"), entry(&rig, "w3"));
        assert_eq!(w1.doing.as_deref(), Some("timer timer:t1: 合成定时甲"));
        assert_eq!(w1.last_wakes, [rig.port.wakes_of("w1")[0].id]);
        assert!(w2.was_working && w2.doing.unwrap().contains("合成定时丙"));
        assert!(!w3.was_working && w3.doing.unwrap().contains("合成定时丁"));
        assert!(
            rig.port.posts().is_empty(),
            "不告警：{:?}",
            rig.port.posts()
        );
    }

    /// 第二次休眠并进正在进行的那一次（同一份结果）；期限只会提前（「立即休眠」能催一个 2 分钟的），不会推后。
    #[tokio::test]
    async fn a_second_hibernate_joins_the_running_one_and_can_only_bring_the_deadline_forward() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit("cap", SessionEvent::Working(true)).await;
        let mut first = hibernate(&mut rig, Some(MIN)).await;
        let mut later = hibernate(&mut rig, Some(10 * MIN)).await;
        rig.tick(MIN).await;
        assert_eq!(
            rig.exec.lock().interrupts,
            ids(&["cap"]),
            "更晚的期限不推后"
        );
        let a = report(&mut first);
        assert_eq!(a, report(&mut later), "并进同一次：同一份结果");

        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit("cap", SessionEvent::Working(true)).await;
        let mut slow = hibernate(&mut rig, Some(2 * MIN)).await;
        assert!(rig.exec.lock().interrupts.is_empty());
        let mut now = hibernate(&mut rig, Some(0)).await;
        assert_eq!(
            rig.exec.lock().interrupts,
            ids(&["cap"]),
            "立即休眠催到现在"
        );
        assert_eq!(report(&mut slow), report(&mut now));
    }

    /// 到「期限 + 叫停宽限」还没退出的：照实报「没停下」、`completed = false`（serve 再用 abort_handle 收尾，同 9a）。
    #[tokio::test]
    async fn a_session_that_will_not_exit_is_reported_at_the_give_up_time() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("cap").await;
        rig.run("w1").await;
        rig.exec.lock().hang_on_stop.insert("w1".into());
        rig.emit("w1", SessionEvent::Working(true)).await;
        let mut rx = hibernate(&mut rig, Some(0)).await;
        assert!(rx.try_recv().is_err(), "还在等 w1 退出");
        let grace = rig.core.cfg.hibernate_stop_grace.as_millis() as i64;
        rig.tick(grace - 1).await;
        assert!(rx.try_recv().is_err());
        rig.tick(1).await;
        assert_eq!(
            report(&mut rx),
            HibernateReport {
                clean: ids(&["cap"]),
                interrupted: vec![],
                failed: ids(&["w1"]),
                completed: false,
            }
        );
        assert!(!rig.core.shutdown_done());
        assert_eq!(
            entry(&rig, "w1").outcome,
            Outcome::Interrupted,
            "记录里照样记下被打断"
        );
    }

    /// 停机进行中再要休眠 = 拒绝（停机已经不等安全点地叫停了）；休眠进行中来了停机 = 期限提前到现在。
    #[tokio::test]
    async fn hibernate_during_shutdown_is_refused_and_shutdown_during_hibernate_cuts_now() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let (tx, _rx) = oneshot::channel();
        rig.core.handle(Input::Shutdown(tx)).await;
        let mut rx = hibernate(&mut rig, Some(MIN)).await;
        assert_eq!(rx.try_recv().unwrap(), Err(ControlError::ShuttingDown));

        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit("cap", SessionEvent::Working(true)).await;
        let mut rx = hibernate(&mut rig, Some(2 * MIN)).await;
        let (tx, mut down) = oneshot::channel();
        rig.core.handle(Input::Shutdown(tx)).await;
        rig.settle().await;
        assert_eq!(rig.exec.lock().interrupts, ids(&["cap"]));
        assert_eq!(report(&mut rx).interrupted, ids(&["cap"]));
        assert!(rig.core.shutdown_done());
        down.try_recv().expect("停机回执");
    }

    fn desired_running(rig: &Rig, id: &str, was_working: bool) {
        let mut st = rig.port.lock();
        let row = st.sessions.get_mut(id).unwrap();
        row.desired = Desired::Running;
        row.was_working = was_working;
    }

    fn hib(id: &str, was_working: bool, outcome: Outcome, doing: Option<&str>) -> Hibernated {
        Hibernated {
            session_id: id.into(),
            was_working,
            outcome,
            doing: doing.map(str::to_string),
            last_wakes: vec![],
            resumed: false,
        }
    }

    /// 上次是休眠停下的：cap 空闲、w1 期限内做完、w2 被打断（停下时在做的事带着外来文字）。
    fn after_hibernate(rig: &Rig) {
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.worker("c1", "w2");
        desired_running(rig, "cap", false);
        desired_running(rig, "w1", false);
        desired_running(rig, "w2", true);
        rig.port.lock().hibernate = Some(HibernateRecord {
            at_ms: T0.0 - HOUR,
            sessions: vec![
                hib("cap", false, Outcome::Finished, None),
                hib(
                    "w1",
                    true,
                    Outcome::Finished,
                    Some("timer timer:t1: 合成定时"),
                ),
                hib("w2", true, Outcome::Interrupted, Some(HOSTILE)),
            ],
            ..Default::default()
        });
    }

    fn note(was_working: bool, interrupted: bool, doing: Option<&str>) -> String {
        let doing = doing.map(safe_name);
        policy::hibernate_note(
            text::Lang::Zh,
            &policy::HibernateFacts {
                was_working,
                interrupted,
                doing: doing.as_deref(),
            },
        )
        .expect("在干活 / 被打断的才有附言")
    }

    /// C26：休眠之后启动——询问标成休眠；答「恢复」后每个都带休眠附言（被打断的说要重跑；外来文字过 safe_name）、
    /// 不再带崩溃重启的「被打断」；附言只给一次（记录记下已恢复），之后的普通重启又是普通询问。
    #[tokio::test]
    async fn boot_after_hibernate_asks_as_hibernate_and_resume_carries_the_hibernate_note() {
        let mut rig = Rig::new().await;
        after_hibernate(&rig);
        rig.core.boot().await.unwrap();
        rig.settle().await;
        assert_eq!(
            rig.port.lock().restart_reasons,
            [RestartReason::Hibernate],
            "询问载荷能和崩溃重启分开"
        );
        assert!(rig.exec.lock().launches.is_empty(), "没答就不起");
        let answer = CollabFact::RestartAnswered {
            prompt: 1,
            resume: true,
            sessions: ids(&["cap", "w1", "w2"]),
        };
        rig.core.handle(Input::Collab(answer)).await;
        rig.settle().await;
        let initial = |s: &str| rig.exec.launches(s)[0].initial.clone().unwrap();
        assert_eq!(
            rig.exec.launches("cap")[0].initial,
            None,
            "休眠时空闲的：静默恢复，不给附言（评审 I1）"
        );
        assert_eq!(
            initial("w1"),
            note(true, false, Some("timer timer:t1: 合成定时"))
        );
        let w2 = initial("w2");
        assert_eq!(w2, note(true, true, Some(HOSTILE)));
        assert!(w2.contains(text::get(text::Lang::Zh, "hibernate.interrupted")));
        assert!(
            !w2.contains(text::get(text::Lang::Zh, "restart.interrupted")),
            "休眠附言取代崩溃重启的「被打断」：{w2}"
        );
        assert!(w2.contains(&safe_name(HOSTILE)), "{w2}");
        assert!(!w2.contains('\u{202E}') && !w2.contains("\n[cap]"), "{w2}");
        let rec = rig.port.lock().hibernate.clone().unwrap();
        assert!(
            rec.sessions.iter().all(|h| h.resumed),
            "附言给过了：{rec:?}"
        );

        // 崩了、再起：记录里没有没恢复的了 → 普通重启的询问；被打断过的按普通的「被打断」补。
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        rig.rx = rx;
        rig.core = Core::new(
            rig.port.clone(),
            Executors::new().with("claude", std::sync::Arc::new(rig.exec.clone())),
            Hooks::default(),
            rig.clock.clone(),
            OrchestraConfig::for_tests(),
            tx,
        )
        .await
        .unwrap();
        rig.port.lock().sessions.get_mut("w2").unwrap().was_working = true;
        rig.core.boot().await.unwrap();
        assert_eq!(
            rig.port.lock().restart_reasons.last(),
            Some(&RestartReason::Restart)
        );
        rig.cmd(|reply| Cmd::Resume {
            session: "w2".into(),
            origin: Origin::Human,
            reply,
        })
        .await
        .unwrap();
        assert_eq!(
            rig.exec.launches("w2")[1].initial.as_deref(),
            Some(text::get(text::Lang::Zh, "restart.interrupted")),
            "休眠附言只给一次"
        );
    }

    /// 单个恢复、以及询问之后人 @ 名单里的某一个（C4），同样带休眠附言（只给一次）。
    #[tokio::test]
    async fn single_resume_and_a_human_mention_also_carry_the_hibernate_note() {
        let mut rig = Rig::new().await;
        after_hibernate(&rig);
        rig.core.boot().await.unwrap();
        rig.settle().await;
        rig.cmd(|reply| Cmd::Resume {
            session: "w2".into(),
            origin: Origin::Human,
            reply,
        })
        .await
        .unwrap();
        assert_eq!(
            rig.exec.launches("w2")[0].initial.as_deref(),
            Some(note(true, true, Some(HOSTILE)).as_str())
        );
        // 人在大厅 @ w1（询问开出之后）：只恢复它，开场 = 休眠附言 + 那条消息。
        let r = MessageRef {
            channel: "c1/lobby".into(),
            seq: 1,
        };
        let facts = MsgFacts {
            id: "m-1".into(),
            at: r.clone(),
            sender: Sender::Human,
            mentions: vec![Mention::Session("w1".into())],
            created_at: T0,
        };
        rig.port
            .lock()
            .messages
            .insert((r.channel.clone(), r.seq), (facts, "人甲".into()));
        let w = NewWake {
            session: "w1".into(),
            kind: WakeKind::Message,
            source: source::message("m-1"),
            priority: Priority::Human,
            not_before: T0,
            note: None,
            origin: "human".into(),
        };
        enqueue(&mut rig, w).await;
        let initial = rig.exec.launches("w1")[0].initial.clone().unwrap();
        let want = note(true, false, Some("timer timer:t1: 合成定时"));
        assert!(initial.starts_with(&want), "{initial}");
        assert!(initial.contains("人甲"), "{initial}");
        assert!(rig.exec.launches("cap").is_empty(), "其余照旧等");
        let rec = rig.port.lock().hibernate.clone().unwrap();
        let resumed: Vec<&str> = rec
            .sessions
            .iter()
            .filter(|h| h.resumed)
            .map(|h| h.session_id.as_str())
            .collect();
        assert_eq!(resumed, ["w1", "w2"]);
    }

    /// 上次休眠还没恢复的（一直等着人答）、这次又休眠：它们的项带进新记录，下次照样给休眠附言。
    #[tokio::test]
    async fn entries_not_yet_resumed_are_carried_into_the_next_hibernate() {
        let mut rig = Rig::new().await;
        after_hibernate(&rig);
        // 评审 m4：只带还在等人答的（待恢复名单里的）；w3 早就不在名单里了（人停过），它的旧项不带。
        rig.worker("c1", "w3");
        rig.port
            .lock()
            .hibernate
            .as_mut()
            .unwrap()
            .sessions
            .push(hib("w3", true, Outcome::Pending, None));
        rig.core.boot().await.unwrap();
        rig.run("cap").await; // 人只恢复了机长
        let mut rx = hibernate(&mut rig, Some(0)).await;
        assert!(report(&mut rx).completed);
        let rec = rig.port.lock().hibernate.clone().unwrap();
        let got: Vec<(&str, Outcome, bool)> = rec
            .sessions
            .iter()
            .map(|h| (h.session_id.as_str(), h.outcome, h.resumed))
            .collect();
        assert_eq!(
            got,
            [
                ("cap", Outcome::Finished, false),
                ("w1", Outcome::Finished, false),
                ("w2", Outcome::Interrupted, false)
            ]
        );
        assert_eq!(rec.prompt_id, None, "新的一次休眠：还没开询问");
    }

    /// 评审 m2：休眠已经回报过了（actor 正在收尾 / 等 abort）再来一次休眠：当场回那份结果，不留一个永远等不到的回执。
    #[tokio::test]
    async fn a_hibernate_after_the_report_gets_the_same_report_at_once() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("cap").await;
        rig.run("w1").await;
        rig.exec.lock().hang_on_stop.insert("w1".into());
        rig.emit("w1", SessionEvent::Working(true)).await;
        let mut first = hibernate(&mut rig, Some(0)).await;
        rig.tick(rig.core.cfg.hibernate_stop_grace.as_millis() as i64)
            .await;
        let a = report(&mut first);
        assert!(!a.completed);
        let mut again = hibernate(&mut rig, Some(0)).await;
        assert_eq!(report(&mut again), a);
    }

    /// 评审 m1：到期限先按 Esc，给它一点时间自己停下这一轮（`Working(false)`）再叫停；它一直不停就最多等 5 秒
    /// （叫停宽限更短时等到宽限为止）。等的这几秒里驱动层发来的 `Working(false)` 不抹掉 was_working（它是被打断的）。
    #[tokio::test]
    async fn after_esc_it_waits_for_the_turn_to_end_up_to_five_seconds() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.worker("c1", "w2");
        for s in ["cap", "w1", "w2"] {
            rig.run(s).await;
        }
        rig.exec.lock().deaf_to_interrupt.insert("w1".into());
        rig.exec.lock().deaf_to_interrupt.insert("w2".into());
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.emit("w2", SessionEvent::Working(true)).await;
        let mut rx = hibernate(&mut rig, Some(0)).await;
        assert_eq!(rig.exec.lock().interrupts, ids(&["w1", "w2"]));
        assert_eq!(rig.exec.lock().stops, ids(&["cap"]), "按了 Esc，先不叫停");
        rig.clock.advance(SEC);
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(
            rig.exec.lock().stops,
            ids(&["cap", "w1"]),
            "这一轮停下了：叫停"
        );
        rig.tick(4 * SEC - 1).await;
        assert_eq!(rig.exec.lock().stops, ids(&["cap", "w1"]));
        rig.tick(1).await;
        assert_eq!(
            rig.exec.lock().stops,
            ids(&["cap", "w1", "w2"]),
            "一直不停：5 秒后叫停"
        );
        let r = report(&mut rx);
        assert_eq!(r.interrupted, ids(&["w1", "w2"]));
        assert!(r.completed);
        assert!(rig.port.row("w1").was_working, "被打断的：was_working 留着");
    }

    /// 评审 I3：休眠中途 crewd 没了（电脑先关了，期限还没到）——在干活的那个停在「等安全点」，下次恢复时按被打断说
    /// （要重跑），不能说「做完了」。
    #[tokio::test]
    async fn a_crash_before_the_deadline_resumes_the_working_ones_as_interrupted() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("cap").await;
        rig.run("w1").await;
        enqueue(&mut rig, timer("w1", "t1", "合成定时甲")).await;
        rig.ack("w1").await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        let _rx = hibernate(&mut rig, Some(2 * MIN)).await;
        assert_eq!(entry(&rig, "w1").outcome, Outcome::Pending);
        // 崩了：新的 Core，同一个库。
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        rig.rx = rx;
        rig.core = Core::new(
            rig.port.clone(),
            Executors::new().with("claude", std::sync::Arc::new(rig.exec.clone())),
            Hooks::default(),
            rig.clock.clone(),
            OrchestraConfig::for_tests(),
            tx,
        )
        .await
        .unwrap();
        rig.core.boot().await.unwrap();
        assert_eq!(rig.port.lock().restart_reasons, [RestartReason::Hibernate]);
        rig.cmd(|reply| Cmd::Resume {
            session: "w1".into(),
            origin: Origin::Human,
            reply,
        })
        .await
        .unwrap();
        assert_eq!(
            rig.exec.launches("w1")[1].initial.as_deref(),
            Some(note(true, true, Some("timer timer:t1: 合成定时甲")).as_str()),
            "没等到结局的按被打断说"
        );
    }

    /// 评审 I4：恢复一个之后，记录里那条询问的链接还在（`put_hibernate` 写回的是带着询问 id 的那份）——不然之后读这条
    /// 询问就不再是「休眠」了。
    #[tokio::test]
    async fn resuming_one_keeps_the_record_linked_to_the_prompt() {
        let mut rig = Rig::new().await;
        after_hibernate(&rig);
        rig.core.boot().await.unwrap();
        rig.settle().await;
        assert_eq!(
            rig.port.lock().hibernate.as_ref().unwrap().prompt_id,
            Some(1)
        );
        rig.cmd(|reply| Cmd::Resume {
            session: "w2".into(),
            origin: Origin::Human,
            reply,
        })
        .await
        .unwrap();
        let rec = rig.port.lock().hibernate.clone().unwrap();
        assert_eq!(rec.prompt_id, Some(1), "{rec:?}");
        assert!(
            rec.sessions
                .iter()
                .any(|h| h.session_id == "w2" && h.resumed)
        );
    }

    /// 起会话（新建）在休眠中同样被拒；没有会话在跑时休眠立刻完成。
    #[tokio::test]
    async fn nothing_running_completes_at_once_and_starts_are_refused() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let mut rx = hibernate(&mut rig, None).await;
        assert_eq!(
            report(&mut rx),
            HibernateReport {
                completed: true,
                ..Default::default()
            }
        );
        let req = StartSession {
            crew_id: "c1".into(),
            captain: false,
            executor: "claude".into(),
            brief: "合成任务甲".into(),
            title: None,
            model: None,
            effort: None,
            isolation: false,
        };
        let r = rig
            .cmd(|reply| Cmd::Start {
                req,
                origin: Origin::Human,
                reply,
            })
            .await;
        assert_eq!(r, Err(ControlError::ShuttingDown));
    }

    /// 9b 复审：按过 Esc 之后至多等 min(5 秒, 到放弃之前剩下的一半)——宽限短于 10 秒时，叫停之后还留着一半时间给它退出，
    /// 不会等满到放弃那一刻才叫停、再白白报成「没停下」。
    #[tokio::test]
    async fn with_a_short_grace_the_settle_wait_leaves_half_of_it_for_the_exit() {
        let cfg = OrchestraConfig {
            hibernate_stop_grace: std::time::Duration::from_secs(4),
            ..OrchestraConfig::for_tests()
        };
        let mut rig = Rig::with_cfg(Hooks::default(), cfg).await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("cap").await;
        rig.run("w1").await;
        rig.exec.lock().deaf_to_interrupt.insert("w1".into());
        rig.emit("w1", SessionEvent::Working(true)).await;
        let mut rx = hibernate(&mut rig, Some(0)).await;
        assert_eq!(rig.exec.lock().interrupts, ids(&["w1"]));
        rig.tick(2 * SEC - 1).await;
        assert_eq!(rig.exec.lock().stops, ids(&["cap"]), "还在等它停下这一轮");
        rig.tick(1).await;
        assert_eq!(
            rig.exec.lock().stops,
            ids(&["cap", "w1"]),
            "等了宽限的一半（2 秒）就叫停"
        );
        let r = report(&mut rx);
        assert!(r.completed && r.failed.is_empty(), "{r:?}");
        assert_eq!(r.interrupted, ids(&["w1"]));
    }

    /// 9b 复审：休眠到期限、按过 Esc 等它停下的 session 不再收 nudge（当场回「没在跑」，不排队）。
    #[tokio::test]
    async fn a_session_cut_by_hibernate_refuses_nudges() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("cap").await;
        rig.run("w1").await;
        rig.exec.lock().deaf_to_interrupt.insert("w1".into());
        rig.emit("w1", SessionEvent::Working(true)).await;
        let _rx = hibernate(&mut rig, Some(0)).await;
        assert!(rig.live("w1").is_some_and(|l| l.hibernate_cut));
        let r = rig
            .cmd(|reply| Cmd::Nudge {
                session: "w1".into(),
                input: "合成补充".into(),
                reply,
            })
            .await;
        assert_eq!(r, Err(ControlError::NotRunning("w1".into())));
        assert!(rig.exec.sent("w1").is_empty());
    }
}
