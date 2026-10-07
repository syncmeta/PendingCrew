//! 重启恢复与停机（spec §22.4「重启恢复」；§22.7 Q1：问人，答「恢复」才错开拉起）。
use std::collections::BTreeMap;

use crewd_wake::notice::DECISION_ESCALATE_MS;
use crewd_wake::policy;
use crewd_wake::{Ms, QueuedWake};
use tokio::sync::oneshot;

use super::core::{ControlError, Core};
use super::deliver::restart_row;
use super::port::*;

/// 重启回收放弃一批时记下的原因（进 `{error}` 槽，过 `safe_name`；给人看的诊断，不是给 agent 的提示词）。
const RESTART_ERROR: &str = "crewd restarted before the delivery was confirmed";

impl<P: Port> Core<P> {
    /// serve 启动时调一次（孤儿已由 serve 先回收）：
    /// ① 上次没确认的 inflight 按一次失败的尝试结算（C25：没到上限立刻再试，到了上限放弃、告警一次；消息类靠游标
    ///    防重放，裁定 17）；
    /// ② 上次在跑的（`desired=running`）一个都不起，进「待恢复」名单，开一次询问（M3a 发人类群公告、客户端弹窗）；
    /// ③ 积压补救的规划（裁定 16）；④ 记 C4 的水位（控制者裁定 C3：在重扫之前）；⑤ 能投的投出去（待恢复的不会被拉起）。
    pub(crate) async fn boot(&mut self) -> anyhow::Result<()> {
        let now = self.now();
        let acked = self
            .due
            .touch(&self.port)
            .fail_inflight(RESTART_ERROR, now)
            .await?;
        if !acked.retried.is_empty() || !acked.gave_up.is_empty() {
            eprintln!(
                "crewd: 编排：上次没确认的唤醒 {} 条再试、{} 条到上限放弃",
                acked.retried.len(),
                acked.gave_up.len()
            );
        }
        if !acked.gave_up.is_empty() {
            // 放弃的行由 `fail_inflight` 原样带回：不去封顶的 `list_wakes` 里找（评审 Fix 1）。
            let mut by_session: BTreeMap<String, Vec<QueuedWake>> = BTreeMap::new();
            for w in acked.gave_up {
                by_session.entry(w.session.clone()).or_default().push(w);
            }
            for (session, rows) in by_session {
                if let Err(e) = self.alert_gave_up(&session, &rows, RESTART_ERROR).await {
                    eprintln!("crewd: 编排：{e:#}");
                }
            }
        }
        let rows = policy::restart_order(
            self.port
                .sessions_desired_running()
                .await?
                .iter()
                .map(restart_row)
                .collect(),
        );
        self.held = rows.iter().map(|r| r.session.clone()).collect();
        self.restart_prompt = self
            .port
            .open_restart(rows.into_iter().map(|r| r.session).collect())
            .await?;
        self.replan_unread().await?;
        // C4 的水位：到此为止排着的（含积压补救刚补的旧 @）都不算「询问之后人新 @ 的」。
        self.held_watermark = self
            .port
            .list_wakes(None)
            .await?
            .iter()
            .map(|w| w.id)
            .max()
            .unwrap_or(0);
        self.rescan().await
    }

    pub(crate) async fn set_max_live(&mut self, max: Option<usize>) -> Result<(), ControlError> {
        self.port
            .set_max_live(max)
            .await
            .map_err(ControlError::internal)?;
        self.gate.set_max_live(max);
        self.rescan().await.map_err(ControlError::internal)
    }

    /// 停机：关上停机闸（不投、不拉起、不起 / 恢复会话），用 [`Core::halt`] 叫停全部在跑的（不改 desired /
    /// was_working：下次启动问人时要知道谁在跑、谁被打断；带着的唤醒原样退回），正在起的起来了也叫停
    /// （`Starting::halt_requested`，不是人停）。全部退出、建 worktree 的也回来了才回执（[`Core::shutdown_done`]）。
    /// 期限由调用方（`Handle::shutdown`）掐。
    pub(crate) fn begin_shutdown(&mut self, reply: oneshot::Sender<()>) {
        self.shutting_down = true;
        self.shutdown.push(reply);
        let live: Vec<String> = self.live.keys().cloned().collect();
        for s in live {
            self.halt(&s);
        }
        for st in self.starting.values_mut() {
            st.halt_requested = true;
        }
    }

    /// 停机完成（没有会话还活着、没有在起的、没有在建 worktree 的）就回执，返回 true。停机闸**不**随回执打开（C4）。
    pub(crate) fn shutdown_done(&mut self) -> bool {
        if !self.shutting_down
            || !self.live.is_empty()
            || !self.starting.is_empty()
            || self.preparing > 0
        {
            return false;
        }
        for reply in self.shutdown.drain(..) {
            let _ = reply.send(());
        }
        true
    }

    /// 下一次定时拍：最早的「未来」not_before、退避 / 错开的时刻、待决策升级时刻，都没有就心跳。
    /// 已到期却投不了的（目标在忙）不算——那些由 idle 边沿接，不能让定时器空转。
    /// 未来的 not_before 来自缓存（`Core::due`），队列没变就不读库（评审 Fix 3）；缓存最多信一个心跳（9a 复审）。
    pub(crate) async fn next_deadline(&self) -> Ms {
        let now = self.now();
        let mut t = now.add(self.cfg.heartbeat_ms);
        let next = match self.due.get(now, self.cfg.heartbeat_ms) {
            Some(v) => v,
            None => match self.port.next_not_before_after(now).await {
                Ok(v) => {
                    self.due.set(v, now);
                    v
                }
                Err(e) => {
                    eprintln!("crewd: 编排：读下一个到点时刻失败（按心跳）：{e:#}");
                    None
                }
            },
        };
        if let Some(nb) = next {
            t = t.min(nb);
        }
        if let Some(first) = self.timers.iter().next() {
            t = t.min((*first).max(now));
        }
        for r in self.decisions.values().filter(|r| !r.escalated) {
            t = t.min(r.at.add(DECISION_ESCALATE_MS).max(now));
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::OrchestraConfig;
    use crate::orchestra::core::{
        Cmd, CollabFact, Input, Origin, PRUNE_BATCH, StartSession, WAKE_RETENTION_MS,
    };
    use crate::orchestra::exec::Executors;
    use crate::orchestra::hooks::Hooks;
    use crate::orchestra::testkit::{Rig, T0};
    use crewd_exec::session::{DeliveryOutcome, SessionEvent};
    use crewd_wake::breaker::edge_id;
    use crewd_wake::plan::{Mention, MsgFacts, Sender};
    use crewd_wake::{
        HOUR, Latch, LatchScope, MIN, MessageRef, NewWake, Priority, SEC, WakeKind, WakeState,
        queue, source, text,
    };
    use tokio::sync::oneshot;

    fn desired_running(rig: &Rig, id: &str, was_working: bool) {
        let mut st = rig.port.lock();
        let row = st.sessions.get_mut(id).unwrap();
        row.desired = Desired::Running;
        row.was_working = was_working;
    }

    async fn human_mention(rig: &Rig, target: &str, text: &str) {
        let seq = {
            let st = rig.port.lock();
            st.messages.keys().filter(|(c, _)| c == "c1/lobby").count() as i64 + 1
        };
        let r = MessageRef {
            channel: "c1/lobby".into(),
            seq,
        };
        let id = format!("m-{seq}");
        let facts = MsgFacts {
            id: id.clone(),
            at: r.clone(),
            sender: Sender::Human,
            mentions: vec![Mention::Session(target.into())],
            created_at: T0,
        };
        rig.port
            .lock()
            .messages
            .insert((r.channel.clone(), r.seq), (facts, text.into()));
        let w = NewWake {
            session: target.into(),
            kind: WakeKind::Message,
            source: source::message(&id),
            priority: Priority::Human,
            not_before: T0,
            note: None,
            origin: "human".into(),
        };
        rig.port.enqueue(w).await.unwrap();
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

    /// 上次崩的时候这些行正被一批拿着（inflight）。
    async fn claim_all(rig: &Rig, session: &str) {
        let batch = Batch {
            session: session.into(),
            wakes: rig.port.wakes_of(session),
            ..Default::default()
        };
        rig.port.claim(&batch).await.unwrap();
    }

    #[tokio::test]
    async fn boot_requeues_inflight_and_skips_injected_messages() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        human_mention(&rig, "w1", "合成派活甲").await;
        rig.port.advance("w1", "c1/lobby", 1).await.unwrap(); // 崩之前 hook 已注入过
        rig.port
            .enqueue(timer("w1", "t1", "合成定时"))
            .await
            .unwrap();
        claim_all(&rig, "w1").await;
        rig.core.boot().await.unwrap();
        rig.settle().await;
        let w = rig.port.wakes_of("w1");
        assert_eq!(
            w[0].state,
            WakeState::Delivered,
            "消息类：游标已越过，不重投"
        );
        assert_eq!(
            rig.exec.launches("w1")[0].initial.as_deref(),
            Some("合成定时"),
            "定时类：至少一次"
        );
    }

    /// crewd 崩了、重起：同一个库与执行器，一个全新的 Core（旧的连同它手里的会话句柄一起没了），跑一次 `boot`。
    async fn crash_and_boot(rig: &mut Rig) {
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
        rig.settle().await;
    }

    /// C25：重启回收算一次尝试。一条每次投都把 crewd 弄崩的唤醒（投出去、还没确认就崩），到上限就放弃、
    /// 告警一次，不无限重投。（崩过之后 w1 是 `desired=running`、进待恢复名单：每次都是人点「恢复」把它接回来的。）
    #[tokio::test]
    async fn a_wake_that_keeps_crashing_crewd_gives_up_at_the_limit_with_one_alert() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.port
            .enqueue(timer("w1", "t1", "合成：弄崩 crewd 的定时"))
            .await
            .unwrap();
        // 评审 Fix 1：它之后又有一大堆更新的行（已结的），多过 `list_wakes` 的封顶——放弃告警不能靠那张封顶的表找它。
        for i in 0..WAKE_LIST_LIMIT + 5 {
            rig.port
                .enqueue(timer("cap", &format!("新-{i}"), "合成：早就结了"))
                .await
                .unwrap();
        }
        let newer: Vec<i64> = rig.port.wakes_of("cap").iter().map(|w| w.id).collect();
        rig.port
            .settle(&newer, Settle::Delivered, T0)
            .await
            .unwrap();
        for life in 1..=queue::MAX_ATTEMPTS + 1 {
            crash_and_boot(&mut rig).await;
            if life > 1 && life <= queue::MAX_ATTEMPTS {
                let w = rig.port.wakes_of("w1")[0].clone();
                assert_eq!(
                    (w.state, w.attempts),
                    (WakeState::Pending, life - 1),
                    "第 {life} 次起来：上次那次算数、立刻可再试"
                );
                rig.run("w1").await; // 人恢复它
                rig.tick(SEC).await;
            }
            let w = rig.port.wakes_of("w1")[0].clone();
            if life <= queue::MAX_ATTEMPTS {
                assert_eq!(
                    (w.state, w.attempts),
                    (WakeState::Inflight, life),
                    "第 {life} 次又投出去了"
                );
                assert!(rig.port.posts().is_empty());
            } else {
                assert_eq!(w.state, WakeState::Dropped, "到上限放弃");
            }
        }
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1, "只告警一次：{posts:?}");
        assert_eq!(
            posts[0].0,
            Audience::Lobby {
                crew: "c1".into(),
                mention_owner: true
            }
        );
        assert!(
            posts[0].1.contains(&queue::MAX_ATTEMPTS.to_string()),
            "{}",
            posts[0].1
        );
    }

    #[tokio::test]
    async fn boot_asks_the_human_and_starts_nothing_until_answered() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        desired_running(&rig, "w1", true);
        desired_running(&rig, "cap", false);
        human_mention(&rig, "w1", "人甲").await;
        rig.core.boot().await.unwrap();
        rig.settle().await;
        assert_eq!(
            rig.port.lock().restarts,
            [vec!["cap".to_string(), "w1".to_string()]],
            "机长在前"
        );
        assert!(rig.exec.lock().launches.is_empty());
        rig.tick(10 * MIN).await;
        assert!(
            rig.exec.lock().launches.is_empty(),
            "没答就都不起，唤醒照常排着"
        );
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Pending);
    }

    #[tokio::test]
    async fn a_resume_answer_lost_to_lag_is_recovered_from_the_db() {
        // 人答了「恢复」，但 `RestartAnswered` 在订阅落后时丢了：Lagged 时从库里重推，名单不会永远等着。
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        desired_running(&rig, "cap", false);
        rig.core.boot().await.unwrap();
        assert!(rig.exec.launches("cap").is_empty());
        rig.port.lock().restart_answers.insert(1, true);
        rig.core.handle(Input::Collab(CollabFact::Lagged)).await;
        rig.settle().await;
        assert_eq!(rig.exec.launches("cap").len(), 1);
    }

    #[tokio::test]
    async fn answering_resume_starts_them_staggered_with_the_interrupted_note() {
        let cfg = OrchestraConfig {
            stagger_ms: 2 * SEC,
            ..OrchestraConfig::for_tests()
        };
        let mut rig = Rig::with_cfg(Hooks::default(), cfg).await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        desired_running(&rig, "w1", true);
        desired_running(&rig, "cap", false);
        human_mention(&rig, "w1", "人甲").await;
        rig.core.boot().await.unwrap();
        let answer = CollabFact::RestartAnswered {
            prompt: 1,
            resume: true,
            sessions: vec!["w1".into(), "cap".into()],
        };
        rig.core.handle(Input::Collab(answer)).await;
        rig.settle().await;
        assert_eq!(rig.exec.launches("cap").len(), 1, "机长先起");
        assert_eq!(
            rig.exec.launches("cap")[0].initial,
            None,
            "没被打断：无附言的恢复不带开场正文"
        );
        assert!(rig.exec.launches("w1").is_empty(), "错开 2s");
        rig.tick(2 * SEC).await;
        let initial = rig.exec.launches("w1")[0].initial.clone().unwrap();
        assert!(initial.contains("人甲"), "{initial}");
        assert!(
            initial.contains(text::get(text::Lang::Zh, "restart.interrupted")),
            "{initial}"
        );
    }

    #[tokio::test]
    async fn answering_later_keeps_them_held_and_single_resume_still_works() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        desired_running(&rig, "w1", true);
        desired_running(&rig, "cap", false);
        rig.core.boot().await.unwrap();
        let later = CollabFact::RestartAnswered {
            prompt: 1,
            resume: false,
            sessions: vec!["cap".into(), "w1".into()],
        };
        rig.core.handle(Input::Collab(later)).await;
        rig.settle().await;
        assert!(rig.exec.lock().launches.is_empty());
        rig.cmd(|reply| Cmd::Resume {
            session: "w1".into(),
            origin: Origin::Human,
            reply,
        })
        .await
        .unwrap();
        assert_eq!(
            rig.exec.launches("w1")[0].initial.as_deref(),
            Some(text::get(text::Lang::Zh, "restart.interrupted")),
            "单个恢复也补「被打断」"
        );
        human_mention(&rig, "cap", "人乙").await;
        rig.core
            .handle(Input::Collab(CollabFact::WakeEnqueued {
                session: "cap".into(),
            }))
            .await;
        rig.settle().await;
        assert_eq!(
            rig.exec.launches("cap").len(),
            1,
            "C4：询问之后人 @ 它 = 只恢复它"
        );
        assert!(
            rig.exec.launches("cap")[0]
                .initial
                .as_deref()
                .unwrap_or("")
                .contains("人乙")
        );
    }

    #[tokio::test]
    async fn human_mention_while_held_resumes_only_that_session() {
        // 控制者裁定 C4：其余仍等答复；启动前就排着的人类 @ 不算（boot 测试钉住）。
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.worker("c1", "w2");
        desired_running(&rig, "w1", true);
        desired_running(&rig, "w2", false);
        desired_running(&rig, "cap", false);
        rig.core.boot().await.unwrap();
        rig.settle().await;
        human_mention(&rig, "w1", "人丙").await;
        rig.core
            .handle(Input::Collab(CollabFact::WakeEnqueued {
                session: "w1".into(),
            }))
            .await;
        rig.settle().await;
        let initial = rig.exec.launches("w1")[0].initial.clone().unwrap();
        assert!(
            initial.contains("人丙")
                && initial.contains(text::get(text::Lang::Zh, "restart.interrupted")),
            "{initial}"
        );
        assert!(
            rig.exec.launches("w2").is_empty() && rig.exec.launches("cap").is_empty(),
            "其余照旧等"
        );
        assert!(!rig.core.held.contains("w1") && rig.core.held.contains("w2"));
    }

    /// 控制者裁定 C3：水位在 boot 自己的重扫之前记——积压补救刚补进来的旧 @ 不算「询问之后人新 @ 的」。
    #[tokio::test]
    async fn backlog_rescued_at_boot_does_not_count_as_a_new_human_mention() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        desired_running(&rig, "w1", false);
        // 崩之前落库了、还没来得及规划入队的一条人类 @。
        {
            let mut st = rig.port.lock();
            let r = MessageRef {
                channel: "c1/lobby".into(),
                seq: 1,
            };
            let facts = MsgFacts {
                id: "m-old".into(),
                at: r.clone(),
                sender: Sender::Human,
                mentions: vec![Mention::Session("w1".into())],
                created_at: T0,
            };
            st.messages
                .insert((r.channel, r.seq), (facts, "合成旧 @".into()));
            st.cursors.insert(("w1".into(), "c1/lobby".into()), 0);
        }
        rig.core.boot().await.unwrap();
        rig.settle().await;
        assert_eq!(rig.port.wakes_of("w1").len(), 1, "积压补救补上了");
        assert!(
            rig.exec.launches("w1").is_empty(),
            "可它是启动前的 @：照样等人答"
        );
        assert!(rig.core.held.contains("w1"));
    }

    #[tokio::test]
    async fn shutdown_stops_live_sessions_and_keeps_desired_and_was_working() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("cap").await;
        rig.emit("cap", SessionEvent::Working(true)).await;
        let (tx, rx) = oneshot::channel();
        rig.core.handle(Input::Shutdown(tx)).await;
        rig.settle().await;
        assert!(rig.core.shutdown_done());
        rx.await.expect("停机回执");
        let row = rig.port.row("cap");
        assert_eq!(
            (row.desired, row.was_working),
            (Desired::Running, true),
            "下次启动问人时知道它被打断了"
        );
        human_mention(&rig, "w1", "人甲").await;
        rig.core
            .handle(Input::Collab(CollabFact::WakeEnqueued {
                session: "w1".into(),
            }))
            .await;
        rig.settle().await;
        assert!(rig.exec.launches("w1").is_empty(), "停机中不拉起");
        // C4：回执发过了（`shutdown_done` 拿走了回执），停机闸还在。
        assert!(rig.core.shutdown_done());
        rig.tick(MIN).await;
        assert!(rig.exec.launches("w1").is_empty());
    }

    /// 停机不是人停（Task 7 的 NOTE）：带着一批正在起的，起来之后叫停、那批原样退回 pending（不丢、不算失败）；
    /// 起不来的也一样退回，不记「起不来」、不告警。
    #[tokio::test]
    async fn a_session_mid_start_at_shutdown_has_its_wakes_requeued_not_dropped() {
        for launch_fails in [false, true] {
            let mut rig = Rig::new().await;
            rig.crew("c1", "cap");
            rig.worker("c1", "w1");
            if launch_fails {
                rig.exec.lock().fail_next.push("合成：起不来".into());
            }
            human_mention(&rig, "w1", "人甲").await;
            rig.core
                .handle(Input::Collab(CollabFact::WakeEnqueued {
                    session: "w1".into(),
                }))
                .await;
            assert!(rig.core.starting.contains_key("w1"), "还在起");
            let (tx, rx) = oneshot::channel();
            rig.core.handle(Input::Shutdown(tx)).await;
            rig.settle().await;
            assert!(rig.core.shutdown_done(), "launch_fails={launch_fails}");
            rx.await.unwrap();
            let w = rig.port.wakes_of("w1")[0].clone();
            assert_eq!(
                (w.state, w.attempts),
                (WakeState::Pending, 0),
                "原样退回（launch_fails={launch_fails}）"
            );
            assert!(
                rig.port.posts().is_empty(),
                "不告警：{:?}",
                rig.port.posts()
            );
            assert_eq!(rig.port.row("w1").resume_failed_at, None);
        }
    }

    /// 停机要等正在建 worktree 的起会话请求回来（Task 7 的 NOTE：`preparing == 0`）；回来了也不写行、不起。
    #[tokio::test]
    async fn shutdown_waits_for_a_worktree_being_prepared_and_then_refuses_it() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let hold = rig.worktrees.hold_next();
        let (tx, mut started) = oneshot::channel();
        let req = StartSession {
            crew_id: "c1".into(),
            captain: false,
            executor: "claude".into(),
            brief: "合成任务甲".into(),
            title: Some("w9".into()),
            model: None,
            effort: None,
            isolation: true,
        };
        rig.core
            .handle(Input::Cmd(Cmd::Start {
                req,
                origin: Origin::Human,
                reply: tx,
            }))
            .await;
        rig.settle().await;
        assert_eq!(rig.core.preparing, 1);
        let (stx, _srx) = oneshot::channel();
        rig.core.handle(Input::Shutdown(stx)).await;
        rig.settle().await;
        assert!(!rig.core.shutdown_done(), "worktree 还在建");
        hold.send(()).unwrap();
        rig.settle().await;
        assert!(rig.core.shutdown_done());
        let r = started.try_recv().expect("起会话要有回执");
        assert!(matches!(r, Err(ControlError::Internal(_))), "{r:?}");
        assert_eq!(rig.port.lock().sessions.len(), 1, "没写新行");
        assert!(rig.exec.lock().launches.is_empty());
    }

    #[tokio::test]
    async fn start_and_resume_are_refused_while_shutting_down() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let (tx, _rx) = oneshot::channel();
        rig.core.handle(Input::Shutdown(tx)).await;
        let r = rig
            .cmd(|reply| Cmd::Resume {
                session: "cap".into(),
                origin: Origin::Human,
                reply,
            })
            .await;
        assert_eq!(r, Err(ControlError::ShuttingDown));
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
        assert!(rig.exec.lock().launches.is_empty());
    }

    /// Task 8b 评审 (5)：crewd 叫停途中收到「送到了」——照样记 delivered、游标照样推；随批行放回 pending
    /// （不是人停，不丢）。
    #[tokio::test]
    async fn a_delivered_receipt_while_halting_settles_delivered_and_releases_the_riders() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        human_mention(&rig, "w1", "人甲").await;
        human_mention(&rig, "w1", "人乙").await;
        let rider = rig.port.wakes_of("w1")[1].id;
        rig.port.lock().defer.insert(rider);
        rig.core
            .handle(Input::Collab(CollabFact::WakeEnqueued {
                session: "w1".into(),
            }))
            .await;
        rig.settle().await;
        let generator = rig.live("w1").unwrap().generator;
        let id = rig.exec.last_send_id("w1");
        rig.core.halt("w1");
        rig.core
            .handle(Input::Exec {
                session: "w1".into(),
                generator,
                event: SessionEvent::Delivery {
                    id,
                    subject: crewd_exec::delivery::Subject::Steer,
                    outcome: DeliveryOutcome::Delivered,
                },
            })
            .await;
        rig.settle().await;
        let w = rig.port.wakes_of("w1");
        assert_eq!((w[0].state, w[0].attempts), (WakeState::Delivered, 1));
        assert_eq!(
            (w[1].state, w[1].attempts),
            (WakeState::Pending, 0),
            "随批行放回"
        );
        assert_eq!(rig.port.cursor("w1", "c1/lobby").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn latches_and_the_optional_cap_survive_a_restart() {
        let rig = Rig::new().await;
        rig.port.lock().latches.push(Latch {
            scope: LatchScope::Edge,
            id: edge_id("cap", "w1"),
            reason: "x".into(),
            at: T0,
        });
        rig.port.lock().max_live = Some(3);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let core = Core::new(
            rig.port.clone(),
            Executors::new(),
            Hooks::default(),
            rig.clock.clone(),
            OrchestraConfig::for_tests(),
            tx,
        )
        .await
        .unwrap();
        assert!(core.breaker.held("w1", "cap").is_some());
        assert_eq!(core.gate.max_live(), Some(3));
    }

    /// C7：默认方案到点由定时拍调。S28：队列清理也在定时拍上，一次至多 `PRUNE_BATCH` 条；清满了下一拍接着清，
    /// 清完了一小时内不再清（启动后第一拍就清一次）。
    #[tokio::test]
    async fn tick_applies_due_ask_defaults_and_prunes_old_wakes_in_bounded_batches() {
        let mut rig = Rig::new().await;
        rig.port.lock().prunable = 2 * PRUNE_BATCH + 1;
        rig.tick(MIN).await;
        assert!(rig.port.lock().defaults_applied >= 1);
        let cutoff = |at: crewd_wake::Ms| crewd_wake::Ms(at.0 - WAKE_RETENTION_MS);
        assert_eq!(
            rig.port.lock().prunes,
            [(cutoff(T0.add(MIN)), PRUNE_BATCH)],
            "启动后第一拍清一次、一次至多一批"
        );
        rig.tick(MIN).await;
        rig.tick(MIN).await;
        assert_eq!(rig.port.lock().prunes.len(), 3, "清满了：下一拍接着清");
        assert_eq!(rig.port.lock().prunable, 0);
        rig.tick(MIN).await;
        rig.tick(30 * MIN).await;
        assert_eq!(rig.port.lock().prunes.len(), 3, "清完了：一小时内不再清");
        rig.tick(HOUR).await;
        assert_eq!(rig.port.lock().prunes.len(), 4);
    }

    #[tokio::test]
    async fn next_deadline_skips_due_but_blocked_wakes_and_finds_the_next_future_one() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        human_mention(&rig, "w1", "人甲").await; // 到期了但目标在忙：不能让定时器空转
        assert_eq!(
            rig.core.next_deadline().await,
            T0.add(rig.core.cfg.heartbeat_ms)
        );
        let later = NewWake {
            not_before: T0.add(5 * SEC),
            ..timer("w1", "t2", "x")
        };
        // 别人入队（M3b 的 schedule_wakeup 等）照契约都发 `WakeEnqueued`：下一拍的缓存随之作废。
        enqueue_from_outside(&mut rig, later).await;
        assert_eq!(rig.core.next_deadline().await, T0.add(5 * SEC));
    }

    async fn enqueue_from_outside(rig: &mut Rig, w: NewWake) {
        let session = w.session.clone();
        rig.port.enqueue(w).await.unwrap();
        rig.core
            .handle(Input::Collab(CollabFact::WakeEnqueued { session }))
            .await;
        rig.settle().await;
    }

    /// 评审 Fix 3：下一拍的时刻缓存着，不是每条输入之后都读一次库；队列一变（入队 / 结算 / 拿走，或别人入队的
    /// `WakeEnqueued`）就作废、重读。缓存的时刻过了也重读。
    #[tokio::test]
    async fn next_deadline_reads_the_store_once_until_the_queue_changes() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        let later = NewWake {
            not_before: T0.add(5 * SEC),
            ..timer("w1", "t1", "x")
        };
        enqueue_from_outside(&mut rig, later).await;
        let port = rig.port.clone();
        let reads = move || port.lock().next_due_reads;
        assert_eq!(rig.core.next_deadline().await, T0.add(5 * SEC));
        let first = reads();
        for _ in 0..3 {
            rig.core
                .handle(Input::Cmd(Cmd::Live {
                    session: "w1".into(),
                    reply: oneshot::channel().0,
                }))
                .await;
            assert_eq!(rig.core.next_deadline().await, T0.add(5 * SEC));
        }
        assert_eq!(reads(), first, "队列没变：不再读库");
        let sooner = NewWake {
            not_before: T0.add(2 * SEC),
            ..timer("w1", "t2", "y")
        };
        enqueue_from_outside(&mut rig, sooner).await;
        assert_eq!(
            rig.core.next_deadline().await,
            T0.add(2 * SEC),
            "入队之后重读"
        );
        assert_eq!(reads(), first + 1);
        rig.clock.advance(3 * SEC);
        assert_eq!(
            rig.core.next_deadline().await,
            T0.add(5 * SEC),
            "缓存的时刻过了：重读"
        );
    }

    /// 9a 复审（折进 T9b）：缓存的下一拍最多信一个心跳。别人入队却漏了通告（没发 `WakeEnqueued`）、actor 又一直有
    /// 输入（定时拍不来、缓存不被作废）时，旧缓存也不能把那条唤醒的定时推迟超过一个心跳。
    #[tokio::test]
    async fn a_cached_deadline_older_than_one_heartbeat_is_read_again() {
        let mut rig = Rig::new().await;
        let hb = rig.core.cfg.heartbeat_ms;
        assert_eq!(hb, 30 * SEC, "下面的时刻按 30s 心跳算");
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        assert_eq!(rig.core.next_deadline().await, T0.add(hb), "没有未来的行");
        let first = rig.port.lock().next_due_reads;
        // 漏了通告的入队：只进库，Core 不知道。
        let later = NewWake {
            not_before: T0.add(50 * SEC),
            ..timer("w1", "t1", "x")
        };
        rig.port.enqueue(later).await.unwrap();
        let busy = async |rig: &mut Rig| {
            rig.clock.advance(10 * SEC);
            rig.core
                .handle(Input::Cmd(Cmd::Live {
                    session: "w1".into(),
                    reply: oneshot::channel().0,
                }))
                .await;
        };
        busy(&mut rig).await;
        busy(&mut rig).await;
        assert_eq!(rig.core.next_deadline().await, T0.add(50 * SEC));
        assert_eq!(rig.port.lock().next_due_reads, first, "缓存还新：不读库");
        busy(&mut rig).await;
        assert_eq!(
            rig.core.next_deadline().await,
            T0.add(50 * SEC),
            "缓存满一个心跳就作废、重读：看见了那条（不然是 T0+60s）"
        );
        assert_eq!(rig.port.lock().next_due_reads, first + 1);
    }

    /// 同上，Core 自己入队的未来唤醒（这里是额度唤醒，45 分钟后）也作废缓存（写都经 `self.due.touch(..)`）。
    #[tokio::test]
    async fn a_wake_core_queues_itself_invalidates_the_cached_deadline() {
        let cfg = OrchestraConfig {
            heartbeat_ms: 2 * HOUR,
            ..OrchestraConfig::for_tests()
        };
        let mut rig = Rig::with_cfg(Hooks::default(), cfg).await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        assert_eq!(
            rig.core.next_deadline().await,
            T0.add(2 * HOUR),
            "没有未来的行：心跳"
        );
        let quota =
            crewd_exec::health::Health::new(crewd_exec::health::HealthKind::UsageLimit, "x");
        rig.emit("w1", SessionEvent::Health(Some(quota))).await;
        assert_eq!(rig.core.next_deadline().await, T0.add(45 * MIN));
    }

    #[tokio::test]
    async fn set_max_live_persists_through_the_port() {
        let mut rig = Rig::new().await;
        rig.cmd(|reply| Cmd::SetMaxLive {
            max: Some(4),
            reply,
        })
        .await
        .unwrap();
        assert_eq!(rig.port.lock().max_live, Some(4));
        assert_eq!(rig.cmd(|reply| Cmd::MaxLive { reply }).await, Ok(Some(4)));
    }
}
