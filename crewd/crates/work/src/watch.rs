//! 督办的纯判定：停下即检查与防死循环（spec §5）、等人超时升级与默认方案到点（§6）、
//! 行政巡检的代码检查部分（§8.4；Jev fan-out 与起行政 session 归 M6）。
use crate::book::{Applied, Rejected, WorkBook};
use crate::events::EventKind;
use crate::ids::{AskId, ChannelId, EntryNo, SessionId, Ts, WorkId};
use crate::model::{Actor, AskState, Blocker, Config, Status, StopKind, StopViolation, WaitTier};
use crate::perm::{Action, Ctx};
use crate::views::{EntryStatus, LostOwner};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "violations", rename_all = "snake_case")]
pub enum StopDecision {
    /// 名下都有着落，不用叫。
    Clean,
    /// 立刻再唤醒一次，要求给这些项写下一步。
    Rewake(Vec<StopViolation>),
    /// 连续 `stop_check_limit` 次没改善：不再叫，交行政处。
    Escalate(Vec<StopViolation>),
    /// 已经交过行政处、仍没改善：保持安静。
    Suppressed(Vec<StopViolation>),
}

/// 人的待确认清单一行（spec §11：按紧急度排，显示已等时长）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanWait {
    pub ask: AskId,
    /// 此刻卡在这条问上的活着的项。
    pub items: Vec<WorkId>,
    pub owners: Vec<SessionId>,
    pub waited_ms: i64,
    pub tier: WaitTier,
    pub default_due_at: Option<Ts>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patrol {
    pub unowned_items: Vec<WorkId>,
    pub unclaimed_entries: Vec<EntryNo>,
    pub needs_ruling: Vec<EntryNo>,
    pub lost: Vec<LostOwner>,
    pub stop_escalated: Vec<SessionId>,
    /// 等了 ≥ 24h 的问。
    pub long_waits: Vec<AskId>,
    /// 关联的工作项全部终结的群（候选；「闲置」还要看消息，M6 判）。
    pub archivable_channels: Vec<ChannelId>,
}

impl Patrol {
    /// 只有标出需要动手的事才启动行政 session（spec §8.4）：停下即检查交过来的 session、可归档的群。
    /// 失联重派、待认领、入口派活由调度层直接做；入口裁定归机长 / 人。
    pub fn needs_admin_session(&self) -> bool {
        !self.stop_escalated.is_empty() || !self.archivable_channels.is_empty()
    }
}

pub fn wait_tier(waited_ms: i64, cfg: &Config) -> WaitTier {
    if waited_ms >= cfg.still_wanted_after_ms {
        WaitTier::StillWanted
    } else if waited_ms >= cfg.pin_after_ms {
        WaitTier::Pinned
    } else {
        WaitTier::Fresh
    }
}

/// 巡检触发（spec §8.4）：从没巡过、子树内攒够 N 条消息、或距上次超过间隔。人手动按钮不经过这里。
pub fn patrol_due(
    messages_since_last: u32,
    last_patrol_at: Option<Ts>,
    now: Ts,
    cfg: &Config,
) -> bool {
    match last_patrol_at {
        None => true,
        Some(last) => {
            messages_since_last >= cfg.patrol_every_messages
                || now.since(last) >= cfg.patrol_interval_ms
        }
    }
}

impl WorkBook {
    /// session 进入 idle 时它名下没着落的项（按 W 号、再按种类）。
    pub fn stop_violations(&self, session: &SessionId, now: Ts) -> Vec<StopViolation> {
        let now = self.view_now(now);
        let mut out = Vec::new();
        for it in self.state.items.values() {
            if it.owner.as_ref() != Some(session)
                || !matches!(it.status, Status::Open | Status::Active)
            {
                continue;
            }
            if it.missed_checks > 0 {
                out.push(StopViolation {
                    item: it.id,
                    kind: StopKind::Unanswered,
                });
            }
            if it.next_check_at.is_some_and(|t| t <= now) {
                out.push(StopViolation {
                    item: it.id,
                    kind: StopKind::Overdue,
                });
            }
            let deps_done = it
                .depends_on
                .iter()
                .all(|d| self.state.items[d].status == Status::Done);
            if it.status == Status::Open && it.next_check_at.is_none() && deps_done {
                out.push(StopViolation {
                    item: it.id,
                    kind: StopKind::ReadyUnscheduled,
                });
            }
        }
        out
    }

    /// idle 边沿（spec §5「停下即检查」）。走 idle 边沿重新唤醒，不是 Stop hook 阻塞（spec §2）。
    /// 违反集合跟上次一模一样 = 没改善；连续 `stop_check_limit` 次没改善 → 交行政处，之后保持安静，直到集合变化。
    /// 只动软状态（连续计数），不产出事件。
    pub fn on_idle(&mut self, session: &SessionId, now: Ts) -> StopDecision {
        let now = now.max(self.state.clock);
        let v = self.stop_violations(session, now);
        if v.is_empty() {
            self.state.stop_streaks.remove(session);
            return StopDecision::Clean;
        }
        let limit = self.cfg.stop_check_limit;
        let streak = self.state.stop_streaks.entry(session.clone()).or_default();
        if streak.last != v {
            streak.last = v.clone();
            streak.unimproved = 0;
            streak.escalated = false;
            return StopDecision::Rewake(v);
        }
        if streak.escalated {
            return StopDecision::Suppressed(v);
        }
        streak.unimproved += 1;
        if streak.unimproved >= limit {
            streak.escalated = true;
            StopDecision::Escalate(v)
        } else {
            StopDecision::Rewake(v)
        }
    }

    /// 人的待确认清单：还开着、且此刻卡着至少一个活着的项的问（没人等的问不打扰人）。等得最久的在前。
    pub fn human_waits(&self, now: Ts) -> Vec<HumanWait> {
        let now = self.view_now(now);
        let mut waiting: BTreeMap<AskId, (Vec<WorkId>, BTreeSet<SessionId>)> = BTreeMap::new();
        for it in self.state.items.values() {
            if it.status.is_terminal() {
                continue;
            }
            for b in &it.blocked_on {
                if let Blocker::Human(h) = b {
                    let entry = waiting.entry(*h).or_default();
                    entry.0.push(it.id);
                    entry.1.extend(it.owner.clone());
                }
            }
        }
        let mut out: Vec<HumanWait> = waiting
            .into_iter()
            .filter_map(|(h, (items, owners))| {
                let ask = &self.state.asks[&h];
                if ask.state != AskState::Open {
                    return None;
                }
                let waited_ms = now.since(ask.asked_at).max(0);
                Some(HumanWait {
                    ask: h,
                    items,
                    owners: owners.into_iter().collect(),
                    waited_ms,
                    tier: wait_tier(waited_ms, &self.cfg),
                    default_due_at: ask.default.as_ref().map(|d| ask.asked_at.plus(d.after_ms)),
                })
            })
            .collect();
        out.sort_by_key(|w| (std::cmp::Reverse(w.waited_ms), w.ask));
        out
    }

    /// 升到了更高一档、还没通知过的问（24h 置顶到人的清单；72h 问「还要吗」）。
    pub fn escalations_due(&self, now: Ts) -> Vec<(AskId, WaitTier)> {
        let now = self.view_now(now);
        self.human_waits(now)
            .into_iter()
            .filter(|w| w.tier > self.state.asks[&w.ask].escalated)
            .map(|w| (w.ask, w.tier))
            .collect()
    }

    /// 记下「这一档已经通知过了」。档位只升不降。
    pub fn note_escalation(
        &mut self,
        by: &Actor,
        now: Ts,
        ask: AskId,
        tier: WaitTier,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            d.permit(Action::NoteEscalation, Ctx::default())?;
            let a = d.s.asks.get_mut(&ask).ok_or(Rejected::UnknownAsk(ask))?;
            if a.state != AskState::Open {
                return Err(Rejected::AskNotOpen(ask));
            }
            if tier <= a.escalated {
                return Err(Rejected::Invalid("escalation tiers only go up"));
            }
            a.escalated = tier;
            d.emit(EventKind::EscalationNoted { ask, tier });
            Ok(())
        })
    }

    /// 默认方案已到点、可以 `apply_default` 的问（只算还卡着活项的）。
    pub fn defaults_due(&self, now: Ts) -> Vec<AskId> {
        let now = self.view_now(now);
        self.human_waits(now)
            .into_iter()
            .filter(|w| w.default_due_at.is_some_and(|t| t <= now))
            .map(|w| w.ask)
            .collect()
    }

    /// 行政巡检的代码检查部分。`unrecoverable` 同 `owner_lost`。
    pub fn patrol(&self, now: Ts, unrecoverable: &BTreeSet<SessionId>) -> Patrol {
        let now = self.view_now(now);
        let views = self.entry_views();
        let mut channel_live: BTreeMap<ChannelId, bool> = BTreeMap::new();
        for it in self.state.items.values() {
            if let Some(ch) = &it.channel {
                *channel_live.entry(ch.clone()).or_default() |= it.status.is_live();
            }
        }
        Patrol {
            unowned_items: self.unowned_items(),
            unclaimed_entries: views
                .iter()
                .filter(|v| v.status == EntryStatus::Unclaimed)
                .map(|v| v.no)
                .collect(),
            needs_ruling: views
                .iter()
                .filter(|v| v.status == EntryStatus::NeedsRuling)
                .map(|v| v.no)
                .collect(),
            lost: self.owner_lost(now, unrecoverable),
            stop_escalated: self
                .state
                .stop_streaks
                .iter()
                .filter(|(_, s)| s.escalated)
                .map(|(sid, _)| sid.clone())
                .collect(),
            long_waits: self
                .human_waits(now)
                .into_iter()
                .filter(|w| w.tier >= WaitTier::Pinned)
                .map(|w| w.ask)
                .collect(),
            archivable_channels: channel_live
                .into_iter()
                .filter(|(_, live)| !live)
                .map(|(ch, _)| ch)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockOn, Continuation};
    use crate::ids::{HOUR, MINUTE};
    use crate::model::{DefaultOption, DropKind, DropReason, Evidence, ItemRef, NewItem};
    use crate::progress::{Progress, Report};
    use crate::testkit::*;

    fn v(item: WorkId, kind: StopKind) -> StopViolation {
        StopViolation { item, kind }
    }

    fn block_on_new_ask(
        b: &mut WorkBook,
        who: &str,
        id: WorkId,
        at: Ts,
        default: Option<DefaultOption>,
    ) {
        b.block(
            &Actor::worker(who),
            at,
            id,
            vec![BlockOn::NewAsk {
                question: "q".into(),
                default,
            }],
            Continuation::Nothing,
        )
        .unwrap();
    }

    #[test]
    fn idle_session_with_unscheduled_ready_work_is_rewoken() {
        let mut b = book();
        let ids = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("ready"),
                    NewItem::titled("waits").after(ItemRef::Batch(0)),
                ],
            )
            .unwrap()
            .value;
        assert_eq!(
            b.stop_violations(&sid("a"), T1),
            [v(ids[0], StopKind::ReadyUnscheduled)],
            "依赖没齐的不算"
        );
        assert_eq!(b.on_idle(&sid("b"), T1), StopDecision::Clean);
        activate(&mut b, "a", ids[0], T1);
        assert_eq!(b.on_idle(&sid("a"), T1), StopDecision::Clean);
    }

    #[test]
    fn overdue_and_unanswered_checks_are_violations() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        activate(&mut b, "a", w, T1);
        let due = T1.plus(HOUR);
        assert_eq!(b.stop_violations(&sid("a"), due), [v(w, StopKind::Overdue)]);
        b.record_checkpoint_wake(&Actor::System, due, &[w]).unwrap();
        assert_eq!(
            b.stop_violations(&sid("a"), due),
            [v(w, StopKind::Unanswered)],
            "叫醒后检查时间顺延了，但它还欠一次回报"
        );
    }

    #[test]
    fn blocked_items_never_trigger_stop_check() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        block_on_new_ask(&mut b, "a", w, T1, None);
        assert_eq!(
            b.on_idle(&sid("a"), T1.plus(10 * HOUR)),
            StopDecision::Clean
        );
    }

    #[test]
    fn three_unimproved_rewakes_then_escalate_then_silence() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let a = sid("a");
        let expect = vec![v(w, StopKind::ReadyUnscheduled)];
        assert_eq!(b.on_idle(&a, T1), StopDecision::Rewake(expect.clone()));
        assert_eq!(b.on_idle(&a, T1), StopDecision::Rewake(expect.clone()));
        assert_eq!(b.on_idle(&a, T1), StopDecision::Rewake(expect.clone()));
        assert_eq!(b.on_idle(&a, T1), StopDecision::Escalate(expect.clone()));
        assert_eq!(b.on_idle(&a, T1), StopDecision::Suppressed(expect.clone()));
        assert_eq!(
            b.patrol(T1, &BTreeSet::new()).stop_escalated,
            vec![a.clone()]
        );
        // 有变化（新领了一项）= 有动作，重新计数。
        let w2 = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("y").owned_by("a"))
            .unwrap()
            .value;
        assert_eq!(
            b.on_idle(&a, T1),
            StopDecision::Rewake(vec![
                v(w, StopKind::ReadyUnscheduled),
                v(w2, StopKind::ReadyUnscheduled)
            ])
        );
        activate(&mut b, "a", w, T1);
        activate(&mut b, "a", w2, T1);
        assert_eq!(b.on_idle(&a, T1), StopDecision::Clean);
        assert!(b.state().stop_streaks.is_empty(), "干净了就清掉计数");
    }

    #[test]
    fn human_wait_tiers_at_24h_and_72h() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y"]);
        block_on_new_ask(&mut b, "a", ids[0], T1, None);
        b.block(
            &Actor::worker("a"),
            T1.plus(HOUR),
            ids[1],
            vec![BlockOn::Ask(crate::ids::AskId(1))],
            Continuation::Nothing,
        )
        .unwrap();
        let at = |h: i64| T1.plus(h * HOUR);
        let waits = b.human_waits(at(23));
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0].items, ids);
        assert_eq!(waits[0].owners, [sid("a")]);
        assert_eq!(waits[0].tier, WaitTier::Fresh);
        assert_eq!(b.escalations_due(at(23)), []);
        assert_eq!(b.escalations_due(at(24)), [(AskId(1), WaitTier::Pinned)]);
        b.note_escalation(&Actor::System, at(24), AskId(1), WaitTier::Pinned)
            .unwrap();
        assert_eq!(b.escalations_due(at(48)), [], "同一档只通知一次");
        assert_eq!(
            b.note_escalation(&Actor::System, at(48), AskId(1), WaitTier::Pinned),
            Err(Rejected::Invalid("escalation tiers only go up"))
        );
        assert_eq!(
            b.escalations_due(at(72)),
            [(AskId(1), WaitTier::StillWanted)]
        );
        assert_eq!(
            b.note_escalation(&Actor::worker("a"), at(72), AskId(1), WaitTier::StillWanted),
            Err(Rejected::Forbidden(Action::NoteEscalation))
        );
    }

    #[test]
    fn asks_nobody_waits_on_any_more_leave_the_human_list() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        block_on_new_ask(&mut b, "a", w, T1, None);
        b.report(
            &Actor::worker("a"),
            T1,
            w,
            Report {
                status: Some(Progress::Active),
                next_check_at: Some(T1.plus(HOUR)),
                note: "不等了".into(),
            },
        )
        .unwrap();
        assert_eq!(b.human_waits(T1.plus(30 * HOUR)), []);
        assert_eq!(b.escalations_due(T1.plus(30 * HOUR)), []);
    }

    #[test]
    fn longest_wait_comes_first() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y"]);
        block_on_new_ask(&mut b, "a", ids[0], T1.plus(HOUR), None);
        block_on_new_ask(&mut b, "a", ids[1], T1.plus(2 * HOUR), None);
        let order: Vec<AskId> = b
            .human_waits(T1.plus(3 * HOUR))
            .iter()
            .map(|w| w.ask)
            .collect();
        assert_eq!(order, [AskId(1), AskId(2)]);
    }

    #[test]
    fn default_options_come_due() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        block_on_new_ask(
            &mut b,
            "a",
            w,
            T1,
            Some(DefaultOption {
                text: "按 A 办".into(),
                after_ms: 4 * HOUR,
            }),
        );
        assert_eq!(b.human_waits(T1)[0].default_due_at, Some(T1.plus(4 * HOUR)));
        assert_eq!(b.defaults_due(T1.plus(4 * HOUR - 1)), []);
        assert_eq!(b.defaults_due(T1.plus(4 * HOUR)), [AskId(1)]);
        b.apply_default(&Actor::System, T1.plus(4 * HOUR), AskId(1))
            .unwrap();
        assert_eq!(b.defaults_due(T1.plus(5 * HOUR)), []);
    }

    #[test]
    fn patrol_collects_what_needs_hands() {
        let mut b = book();
        b.register_entry(T1, EntryNo(2)).unwrap();
        b.register_entry(T1, EntryNo(3)).unwrap();
        let w = claim(&mut b, "a", &["x"])[0];
        let ch = ChannelId::new("ch-x");
        b.link_channel(&Actor::worker("a"), T1, w, Some(ch.clone()))
            .unwrap();
        let orphan = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("help"))
            .unwrap()
            .value;
        let dropped = b
            .claim_entry(
                &Actor::worker("b"),
                T1,
                EntryNo(3),
                vec![NewItem::titled("z")],
            )
            .unwrap()
            .value[0];
        b.drop_item(
            &Actor::worker("b"),
            T1,
            dropped,
            DropReason {
                kind: DropKind::NotNeeded,
                note: "n".into(),
                evidence: None,
            },
        )
        .unwrap();
        let p = b.patrol(T1, &BTreeSet::new());
        assert_eq!(p.unowned_items, [orphan]);
        assert_eq!(p.unclaimed_entries, [EntryNo(2)]);
        assert_eq!(p.needs_ruling, [EntryNo(3)]);
        assert_eq!(p.archivable_channels, Vec::<ChannelId>::new());
        assert!(!p.needs_admin_session());
        b.mark_done(&Actor::worker("a"), T1, w, Evidence::Text("ok".into()))
            .unwrap();
        let p = b.patrol(T1, &BTreeSet::new());
        assert_eq!(p.archivable_channels, [ch]);
        assert!(p.needs_admin_session());
    }

    #[test]
    fn patrol_cadence() {
        let cfg = Config::default();
        assert!(patrol_due(0, None, T1, &cfg));
        assert!(!patrol_due(9, Some(T1), T1.plus(29 * MINUTE), &cfg));
        assert!(patrol_due(10, Some(T1), T1.plus(MINUTE), &cfg));
        assert!(patrol_due(0, Some(T1), T1.plus(30 * MINUTE), &cfg));
    }

    #[test]
    fn wait_tier_boundaries() {
        let cfg = Config::default();
        assert_eq!(wait_tier(24 * HOUR - 1, &cfg), WaitTier::Fresh);
        assert_eq!(wait_tier(24 * HOUR, &cfg), WaitTier::Pinned);
        assert_eq!(wait_tier(72 * HOUR, &cfg), WaitTier::StillWanted);
        assert_eq!(wait_tier(-5, &cfg), WaitTier::Fresh);
    }
}
