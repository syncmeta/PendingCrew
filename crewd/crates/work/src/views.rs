//! 推导视图（只读）：入口状态（spec §4.1，由第二层推导）、待认领 / 就绪项、「我的工作项」（§4.3 注入用的结构化数据，
//! 不渲染文字——给 agent 看的文案归 M4b 的模板）、到期检查点（§5，同一 session 合并一条）、负责人失联候选（§5）。
use crate::book::WorkBook;
use crate::ids::{EntryNo, SessionId, Ts, WorkId};
use crate::model::{Blocker, Config, Status, WorkItem};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    /// 没有关联工作项 → 待认领。
    Unclaimed,
    InProgress,
    /// 没有活着的关联项，且至少一项 done。
    Done,
    /// 关联项全部 dropped → 交机长 / 人裁定（不自动关）。
    NeedsRuling,
    /// 已裁定关闭。
    Closed,
}

/// 入口行内显示「→ W3 W4（1/2）」与负责人（spec §11）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryView {
    pub no: EntryNo,
    pub status: EntryStatus,
    /// 直接关联（sources 含 #N）、没 dropped 的项。
    pub items: Vec<WorkId>,
    pub done: usize,
    /// 活着的关联项的负责人（去重）。
    pub owners: Vec<SessionId>,
}

/// 「你名下的工作项」一行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyItem {
    pub id: WorkId,
    pub title: String,
    pub status: Status,
    pub next_step: Option<String>,
    pub next_check_at: Option<Ts>,
    pub blocked_on: Vec<Blocker>,
    /// 你发起、对方还没接受的转交。
    pub transfer_to: Option<SessionId>,
}

/// 到期检查点：一个负责人一条，列出它全部到期的项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointDue {
    pub owner: SessionId,
    pub items: Vec<WorkId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LostCause {
    /// 连续 K 次检查点唤醒没有回报，且最后一次之后的复查时间也过了。
    Silent { missed: u32 },
    /// crewd 试过恢复（--resume）没成功。
    Unrecoverable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LostOwner {
    pub item: WorkId,
    pub owner: SessionId,
    pub cause: LostCause,
}

impl WorkBook {
    pub fn entry_view(&self, no: EntryNo) -> Option<EntryView> {
        let entry = self.state.entries.get(&no)?;
        let linked: Vec<&WorkItem> = self
            .state
            .items
            .values()
            .filter(|it| it.sources.contains(&no))
            .collect();
        let any_live = linked.iter().any(|it| it.status.is_live());
        let done = linked.iter().filter(|it| it.status == Status::Done).count();
        let status = if entry.closed.is_some() {
            EntryStatus::Closed
        } else if linked.is_empty() {
            EntryStatus::Unclaimed
        } else if any_live {
            EntryStatus::InProgress
        } else if done > 0 {
            EntryStatus::Done
        } else {
            EntryStatus::NeedsRuling
        };
        let owners: BTreeSet<SessionId> = linked
            .iter()
            .filter(|it| it.status.is_live())
            .filter_map(|it| it.owner.clone())
            .collect();
        Some(EntryView {
            no,
            status,
            items: linked
                .iter()
                .filter(|it| it.status != Status::Dropped)
                .map(|it| it.id)
                .collect(),
            done,
            owners: owners.into_iter().collect(),
        })
    }

    pub fn entry_views(&self) -> Vec<EntryView> {
        self.state
            .entries
            .keys()
            .filter_map(|no| self.entry_view(*no))
            .collect()
    }

    fn deps_done(&self, it: &WorkItem) -> bool {
        it.depends_on
            .iter()
            .all(|d| self.state.items[d].status == Status::Done)
    }

    /// open 且依赖已齐（有没有负责人都算）。
    pub fn ready_items(&self) -> Vec<WorkId> {
        self.state
            .items
            .values()
            .filter(|it| it.status == Status::Open && self.deps_done(it))
            .map(|it| it.id)
            .collect()
    }

    /// 待认领：open 且没有负责人（调度层 / 行政巡检捞这些）。
    pub fn unowned_items(&self) -> Vec<WorkId> {
        self.state
            .items
            .values()
            .filter(|it| it.status == Status::Open && it.owner.is_none())
            .map(|it| it.id)
            .collect()
    }

    /// 该 session 的负载：名下 open + active 项数。blocked 不计（spec §6：卡住的项不算负载）。
    pub fn load_of(&self, session: &SessionId) -> usize {
        self.state
            .items
            .values()
            .filter(|it| it.owner.as_ref() == Some(session))
            .filter(|it| matches!(it.status, Status::Open | Status::Active))
            .count()
    }

    /// 名下活着的项：active 在前，其次 open、blocked；同档按 W 号。
    pub fn my_items(&self, session: &SessionId) -> Vec<MyItem> {
        let rank = |s: Status| match s {
            Status::Active => 0,
            Status::Open => 1,
            _ => 2,
        };
        let mut mine: Vec<&WorkItem> = self
            .state
            .items
            .values()
            .filter(|it| it.status.is_live() && it.owner.as_ref() == Some(session))
            .collect();
        mine.sort_by_key(|it| (rank(it.status), it.id));
        mine.into_iter()
            .map(|it| MyItem {
                id: it.id,
                title: it.title.clone(),
                status: it.status,
                next_step: it.last_report.as_ref().map(|n| n.text.clone()),
                next_check_at: it.next_check_at,
                blocked_on: it.blocked_on.clone(),
                transfer_to: it.pending_transfer.as_ref().map(|t| t.to.clone()),
            })
            .collect()
    }

    /// 别人要转给该 session、等它接受的项。
    pub fn incoming_transfers(&self, session: &SessionId) -> Vec<WorkId> {
        self.state
            .items
            .values()
            .filter(|it| it.pending_transfer.as_ref().map(|t| &t.to) == Some(session))
            .map(|it| it.id)
            .collect()
    }

    /// 到期检查点：有负责人的 open / active 项，nextCheckAt ≤ now，且还没到失联线（missed < K）。
    /// 同一负责人合并成一条（spec §5「同一 session 同一时刻合并为一条」）。
    pub fn due_checkpoints(&self, now: Ts) -> Vec<CheckpointDue> {
        let now = self.view_now(now);
        let mut by_owner: BTreeMap<SessionId, Vec<WorkId>> = BTreeMap::new();
        for it in self.state.items.values() {
            let Some(owner) = &it.owner else { continue };
            if matches!(it.status, Status::Open | Status::Active)
                && it.next_check_at.is_some_and(|t| t <= now)
                && it.missed_checks < self.cfg.missed_checks_limit
            {
                by_owner.entry(owner.clone()).or_default().push(it.id);
            }
        }
        by_owner
            .into_iter()
            .map(|(owner, items)| CheckpointDue { owner, items })
            .collect()
    }

    /// 负责人失联候选（spec §5）：`unrecoverable` 是 crewd 试过恢复失败的 session（退出 ≠ 失联，先 resume）。
    /// 由调度层（排除原负责人）重派 + 告知机长。
    pub fn owner_lost(&self, now: Ts, unrecoverable: &BTreeSet<SessionId>) -> Vec<LostOwner> {
        let now = self.view_now(now);
        let mut out = Vec::new();
        for it in self.state.items.values() {
            let Some(owner) = &it.owner else { continue };
            let Some(cause) = lost_cause(it, now, &self.cfg, unrecoverable) else {
                continue;
            };
            out.push(LostOwner {
                item: it.id,
                owner: owner.clone(),
                cause,
            });
        }
        out
    }
}

/// 这一项的负责人算不算失联（spec §5）：恢复失败名单里有它 → Unrecoverable（含 blocked 项）；
/// open / active 项连续 K 次叫醒没回报、且最后一次之后的复查时间也过了 → Silent。`owner_lost` 与
/// `reassign_lost`（重派前核实）共用这一个判定。
pub(crate) fn lost_cause(
    it: &WorkItem,
    now: Ts,
    cfg: &Config,
    unrecoverable: &BTreeSet<SessionId>,
) -> Option<LostCause> {
    let owner = it.owner.as_ref()?;
    if it.status.is_terminal() {
        return None;
    }
    if unrecoverable.contains(owner) {
        Some(LostCause::Unrecoverable)
    } else if matches!(it.status, Status::Open | Status::Active)
        && it.missed_checks >= cfg.missed_checks_limit
        && it.next_check_at.is_some_and(|t| t <= now)
    {
        Some(LostCause::Silent {
            missed: it.missed_checks,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockOn, Continuation};
    use crate::ids::{HOUR, MINUTE};
    use crate::model::{Actor, DropKind, DropReason, Evidence, ItemRef, NewItem};
    use crate::progress::{Progress, Report};
    use crate::testkit::*;

    fn wait_on_human(b: &mut WorkBook, who: &str, id: WorkId) {
        b.block(
            &Actor::worker(who),
            T1,
            id,
            vec![BlockOn::NewAsk {
                question: "q".into(),
                default: None,
            }],
            Continuation::Nothing,
        )
        .unwrap();
    }

    fn drop_it(b: &mut WorkBook, who: &str, id: WorkId) {
        b.drop_item(
            &Actor::worker(who),
            T1,
            id,
            DropReason {
                kind: DropKind::NotNeeded,
                note: "n".into(),
                evidence: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn entry_status_is_derived_from_its_items() {
        let mut b = book();
        let view = |b: &WorkBook| b.entry_view(EntryNo(1)).unwrap();
        assert_eq!(view(&b).status, EntryStatus::Unclaimed);
        let ids = claim(&mut b, "a", &["x", "y", "z"]);
        let v = view(&b);
        assert_eq!(
            (v.status, v.items.clone(), v.done, v.owners.clone()),
            (EntryStatus::InProgress, ids.clone(), 0, vec![sid("a")])
        );
        b.mark_done(
            &Actor::worker("a"),
            T1,
            ids[0],
            Evidence::Commit("c1".into()),
        )
        .unwrap();
        drop_it(&mut b, "a", ids[1]);
        let v = view(&b);
        assert_eq!(
            (v.status, v.items.clone(), v.done),
            (EntryStatus::InProgress, vec![ids[0], ids[2]], 1),
            "dropped 的不算进「→ W1 W3（1/2）」"
        );
        b.mark_done(
            &Actor::worker("a"),
            T1,
            ids[2],
            Evidence::Commit("c2".into()),
        )
        .unwrap();
        let v = view(&b);
        assert_eq!(
            (v.status, v.done, v.owners.clone()),
            (EntryStatus::Done, 2, vec![])
        );
        assert_eq!(b.entry_view(EntryNo(7)), None);
    }

    #[test]
    fn all_dropped_needs_a_ruling_then_closes() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        drop_it(&mut b, "a", w);
        assert_eq!(
            b.entry_view(EntryNo(1)).unwrap().status,
            EntryStatus::NeedsRuling
        );
        b.close_entry(&Actor::captain("c"), T1, EntryNo(1), "放弃", None)
            .unwrap();
        assert_eq!(
            b.entry_view(EntryNo(1)).unwrap().status,
            EntryStatus::Closed
        );
        b.register_entry(T1, EntryNo(2)).unwrap();
        let all: Vec<(EntryNo, EntryStatus)> =
            b.entry_views().iter().map(|v| (v.no, v.status)).collect();
        assert_eq!(
            all,
            [
                (EntryNo(1), EntryStatus::Closed),
                (EntryNo(2), EntryStatus::Unclaimed)
            ]
        );
    }

    #[test]
    fn split_children_do_not_change_the_entry_count() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        b.block(
            &Actor::worker("a"),
            T1,
            w,
            vec![BlockOn::NewAsk {
                question: "q".into(),
                default: None,
            }],
            Continuation::Split(vec![NewItem::titled("part")]),
        )
        .unwrap();
        assert_eq!(b.entry_view(EntryNo(1)).unwrap().items, [w]);
    }

    #[test]
    fn ready_and_unowned() {
        let mut b = book();
        let ids = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("dep"),
                    NewItem::titled("after dep").after(ItemRef::Batch(0)),
                    NewItem::titled("blocked"),
                ],
            )
            .unwrap()
            .value;
        wait_on_human(&mut b, "a", ids[2]);
        let orphan = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("help"))
            .unwrap()
            .value;
        assert_eq!(b.ready_items(), [ids[0], orphan]);
        assert_eq!(b.unowned_items(), [orphan]);
        b.mark_done(&Actor::worker("a"), T1, ids[0], Evidence::Text("ok".into()))
            .unwrap();
        assert_eq!(b.ready_items(), [ids[1], orphan]);
    }

    #[test]
    fn my_items_lists_active_first_and_excludes_blocked_from_load() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["open", "active", "blocked"]);
        activate(&mut b, "a", ids[1], T1);
        wait_on_human(&mut b, "a", ids[2]);
        b.request_transfer(&Actor::worker("a"), T1, ids[0], sid("b"), "给 b")
            .unwrap();
        let mine = b.my_items(&sid("a"));
        let order: Vec<WorkId> = mine.iter().map(|m| m.id).collect();
        assert_eq!(order, [ids[1], ids[0], ids[2]]);
        assert_eq!(mine[0].next_step.as_deref(), Some("开工"));
        assert_eq!(mine[0].next_check_at, Some(T1.plus(HOUR)));
        assert_eq!(mine[1].transfer_to, Some(sid("b")));
        assert_eq!(mine[2].blocked_on, [Blocker::Human(crate::ids::AskId(1))]);
        assert_eq!(b.load_of(&sid("a")), 2);
        assert_eq!(b.incoming_transfers(&sid("b")), [ids[0]]);
        assert!(b.my_items(&sid("b")).is_empty());
    }

    #[test]
    fn due_checkpoints_merge_per_owner() {
        let mut b = book();
        let a_items = claim(&mut b, "a", &["a1", "a2", "a3"]);
        b.register_entry(T1, EntryNo(2)).unwrap();
        let b_item = b
            .claim_entry(
                &Actor::worker("b"),
                T1,
                EntryNo(2),
                vec![NewItem::titled("b1")],
            )
            .unwrap()
            .value[0];
        activate(&mut b, "a", a_items[0], T1);
        activate(&mut b, "a", a_items[1], T1.plus(MINUTE));
        activate(&mut b, "b", b_item, T1);
        b.report(
            &Actor::worker("a"),
            T1,
            a_items[2],
            Report {
                status: None,
                next_check_at: Some(T1.plus(2 * HOUR)),
                note: "晚点开工".into(),
            },
        )
        .unwrap();
        assert_eq!(b.due_checkpoints(T1.plus(HOUR - 1)), []);
        assert_eq!(
            b.due_checkpoints(T1.plus(HOUR)),
            [
                CheckpointDue {
                    owner: sid("a"),
                    items: vec![a_items[0]]
                },
                CheckpointDue {
                    owner: sid("b"),
                    items: vec![b_item]
                }
            ]
        );
        assert_eq!(
            b.due_checkpoints(T1.plus(2 * HOUR)),
            [
                CheckpointDue {
                    owner: sid("a"),
                    items: a_items.clone()
                },
                CheckpointDue {
                    owner: sid("b"),
                    items: vec![b_item]
                }
            ],
            "同一负责人的到期项合成一条"
        );
    }

    #[test]
    fn silent_owner_is_lost_after_k_unanswered_wakes() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        activate(&mut b, "a", w, T1);
        let none = BTreeSet::new();
        let t = T1.plus(HOUR);
        assert_eq!(b.due_checkpoints(t).len(), 1);
        b.record_checkpoint_wake(&crate::model::Actor::System, t, &sid("a"), &[w])
            .unwrap();
        let t = t.plus(15 * MINUTE);
        assert_eq!(b.owner_lost(t, &none), []);
        assert_eq!(b.due_checkpoints(t)[0].items, [w], "第二次叫醒");
        b.record_checkpoint_wake(&crate::model::Actor::System, t, &sid("a"), &[w])
            .unwrap();
        assert_eq!(b.owner_lost(t.plus(15 * MINUTE - 1), &none), []);
        let t = t.plus(15 * MINUTE);
        assert_eq!(b.due_checkpoints(t), [], "到失联线就不再叫，改判失联");
        assert_eq!(
            b.owner_lost(t, &none),
            [LostOwner {
                item: w,
                owner: sid("a"),
                cause: LostCause::Silent { missed: 2 }
            }]
        );
        b.report(
            &Actor::worker("a"),
            t,
            w,
            Report {
                status: Some(Progress::Active),
                next_check_at: Some(t.plus(HOUR)),
                note: "回来了".into(),
            },
        )
        .unwrap();
        assert_eq!(b.owner_lost(t, &none), [], "回报一次就解除");
    }

    #[test]
    fn unrecoverable_session_loses_every_live_item_including_blocked() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y", "z"]);
        wait_on_human(&mut b, "a", ids[1]);
        b.mark_done(&Actor::worker("a"), T1, ids[2], Evidence::Text("ok".into()))
            .unwrap();
        let dead: BTreeSet<SessionId> = [sid("a")].into();
        let lost: Vec<(WorkId, LostCause)> = b
            .owner_lost(T1, &dead)
            .into_iter()
            .map(|l| (l.item, l.cause))
            .collect();
        assert_eq!(
            lost,
            [
                (ids[0], LostCause::Unrecoverable),
                (ids[1], LostCause::Unrecoverable)
            ],
            "blocked 的也要换人，否则人回复了没人接"
        );
    }

    #[test]
    fn commands_and_views_share_the_book_clock_when_the_caller_is_behind() {
        // 评审 P5：墙钟往回跳（调用方的 now 落后于账本时间）时，视图与命令用同一个时间基准 max(now, clock)。
        let mut b = book();
        let w0 = claim(&mut b, "a", &["等人"])[0];
        b.block(
            &Actor::worker("a"),
            T1,
            w0,
            vec![BlockOn::NewAsk {
                question: "q".into(),
                default: Some(crate::model::DefaultOption {
                    text: "按 A".into(),
                    after_ms: 30 * MINUTE,
                }),
            }],
            Continuation::Nothing,
        )
        .unwrap();
        let ahead = T1.plus(HOUR);
        b.register_entry(ahead, EntryNo(2)).unwrap();
        assert_eq!(b.state().clock, ahead);
        // 迁移 / 派活占位「现在就查」按调用方自己的 now 写；账本时间更晚也收，记成账本的「现在」。
        let w = b
            .claim_entry(
                &Actor::System,
                T1,
                EntryNo(1),
                vec![NewItem::titled("占位").owned_by("a").check_at(T1)],
            )
            .unwrap()
            .value[0];
        assert_eq!(b.item(w).unwrap().next_check_at, Some(ahead));
        assert_eq!(
            b.create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("y").owned_by("a").check_at(T1.plus(MINUTE))
            ),
            Err(crate::book::Rejected::NextCheckNotFuture),
            "不是占位、又不晚于账本时间的检查时间仍然拒"
        );
        assert_eq!(
            b.report(
                &Actor::worker("a"),
                T1,
                w,
                Report {
                    status: None,
                    next_check_at: Some(T1.plus(30 * MINUTE)),
                    note: "n".into(),
                }
            ),
            Err(crate::book::Rejected::NextCheckNotFuture),
            "回报的下次检查要晚于账本时间"
        );
        // 视图按账本时间看：用落后的 now 查，到期项、到点的默认方案一个不少。
        assert_eq!(
            b.due_checkpoints(T1),
            [CheckpointDue {
                owner: sid("a"),
                items: vec![w]
            }]
        );
        assert_eq!(b.defaults_due(T1), [crate::ids::AskId(1)]);
        assert_eq!(b.human_waits(T1)[0].waited_ms, HOUR);
        assert_eq!(b.stop_violations(&sid("a"), T1).len(), 1);
    }

    #[test]
    fn a_forward_jump_ratchets_the_clock_and_is_reported() {
        // 评审 P4：调用方的 now 一下子跑到账本时间前面很远（坏的 NTP 校时）。命令照常做、账本时间跟上（棘轮），
        // 超过 `max_clock_jump_ms` 的前跳由 `clock_jump` 报出来给 M4b（落库前记日志 / 告警）。
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        activate(&mut b, "a", w, T1);
        assert_eq!(b.clock_jump(T1.plus(HOUR)), None);
        assert_eq!(b.clock_jump(T1.plus(24 * HOUR)), None, "正好 24h 不报");
        let jump = T1.plus(25 * HOUR);
        assert_eq!(b.clock_jump(jump), Some(25 * HOUR));
        b.register_entry(jump, EntryNo(2)).unwrap();
        assert_eq!(b.state().clock, jump);
        assert_eq!(b.clock_jump(jump), None);
        // 墙钟回到真实时间：视图仍按账本时间看，w 的检查早已过期。
        let real = T1.plus(2 * MINUTE);
        assert_eq!(b.due_checkpoints(real)[0].items, [w]);
        // 已知风险（交 M4b）：按真实时间写的「30 分钟后再查」早于账本时间，被拒；要按账本时间写。
        let next = |t: Ts| Report {
            status: None,
            next_check_at: Some(t),
            note: "n".into(),
        };
        assert_eq!(
            b.report(&Actor::worker("a"), real, w, next(real.plus(30 * MINUTE))),
            Err(crate::book::Rejected::NextCheckNotFuture)
        );
        assert!(
            b.report(&Actor::worker("a"), real, w, next(jump.plus(30 * MINUTE)))
                .is_ok()
        );
    }
}
