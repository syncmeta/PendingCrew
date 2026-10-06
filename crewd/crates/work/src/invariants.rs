//! 不变量（spec §4.2「crewd 写入时强制，违反即拒」，加上本计划补的几条，见计划「裁定」）。
//! 每条命令在草稿上改完后整份跑一遍；`WorkBook::restore` 读回时也跑一遍。返回第一条违反（按 W 号顺序，确定）。
use crate::graph::{adjacency, find_cycle};
use crate::ids::{AskId, EntryNo, Ts, WorkId};
use crate::model::{AskState, Blocker, BookState, Config, Status};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// 表里的键和项自己的 id 不一致 / 号码计数器落后于已发出的号。
    KeyMismatch(WorkId),
    CounterBehind,
    BlankTitle(WorkId),
    DuplicateRef(WorkId),
    UnknownItemRef {
        item: WorkId,
        missing: WorkId,
    },
    UnknownEntryRef {
        item: WorkId,
        entry: EntryNo,
    },
    UnknownAskRef {
        item: WorkId,
        ask: AskId,
    },
    AskForUnknownItem {
        ask: AskId,
        item: WorkId,
    },
    /// active / blocked 必须有负责人。
    OwnerMissing(WorkId),
    ActiveWithoutCheck(WorkId),
    CheckTooFar {
        item: WorkId,
        at: Ts,
        limit: Ts,
    },
    /// nextCheckAt 只许出现在有负责人的 open / active 项上。
    CheckNotAllowed(WorkId),
    DoneWithoutEvidence(WorkId),
    ActiveBeforeDeps {
        item: WorkId,
        dep: WorkId,
    },
    BlockedWithoutBlocker(WorkId),
    BlockerOnUnblocked(WorkId),
    /// 卡点指向已结束的问 / 已终结的项（应当已被清掉）。
    StaleBlocker {
        item: WorkId,
        blocker: Blocker,
    },
    /// 活着的项挂在已终结的父项下。
    LiveUnderTerminal {
        item: WorkId,
        parent: WorkId,
    },
    /// 活着的项依赖一个已 dropped 的项（永远等不到）。
    LiveOnDropped {
        item: WorkId,
        dep: WorkId,
    },
    /// 活着的项来自一条已裁定关闭的入口。
    LiveOnClosedEntry {
        item: WorkId,
        entry: EntryNo,
    },
    /// 待接受的转交挂在终结项 / 无主项上，或转给现负责人自己。
    StrayTransfer(WorkId),
    Cycle(Vec<WorkId>),
}

pub fn check(s: &BookState, cfg: &Config, now: Ts) -> Result<(), Violation> {
    if s.items
        .keys()
        .next_back()
        .is_some_and(|w| w.0 > s.last_item)
        || s.asks.keys().next_back().is_some_and(|h| h.0 > s.last_ask)
    {
        return Err(Violation::CounterBehind);
    }
    let horizon = now.plus(cfg.max_check_horizon_ms);
    for (key, it) in &s.items {
        let id = it.id;
        if *key != id {
            return Err(Violation::KeyMismatch(*key));
        }
        if it.title.trim().is_empty() {
            return Err(Violation::BlankTitle(id));
        }
        let unique_deps: BTreeSet<_> = it.depends_on.iter().collect();
        let unique_sources: BTreeSet<_> = it.sources.iter().collect();
        let unique_blockers: BTreeSet<_> = it.blocked_on.iter().collect();
        if unique_deps.len() != it.depends_on.len()
            || unique_sources.len() != it.sources.len()
            || unique_blockers.len() != it.blocked_on.len()
        {
            return Err(Violation::DuplicateRef(id));
        }
        for w in it.parent.iter().chain(&it.depends_on) {
            if !s.items.contains_key(w) {
                return Err(Violation::UnknownItemRef {
                    item: id,
                    missing: *w,
                });
            }
        }
        for e in &it.sources {
            if !s.entries.contains_key(e) {
                return Err(Violation::UnknownEntryRef {
                    item: id,
                    entry: *e,
                });
            }
        }
        for b in &it.blocked_on {
            match b {
                Blocker::Item(w) if !s.items.contains_key(w) => {
                    return Err(Violation::UnknownItemRef {
                        item: id,
                        missing: *w,
                    });
                }
                Blocker::Human(h) if !s.asks.contains_key(h) => {
                    return Err(Violation::UnknownAskRef { item: id, ask: *h });
                }
                _ => {}
            }
        }

        let live = it.status.is_live();
        if matches!(it.status, Status::Active | Status::Blocked) && it.owner.is_none() {
            return Err(Violation::OwnerMissing(id));
        }
        match it.next_check_at {
            None if it.status == Status::Active => {
                return Err(Violation::ActiveWithoutCheck(id));
            }
            Some(_)
                if it.owner.is_none() || !matches!(it.status, Status::Open | Status::Active) =>
            {
                return Err(Violation::CheckNotAllowed(id));
            }
            Some(at) if at > horizon => {
                return Err(Violation::CheckTooFar {
                    item: id,
                    at,
                    limit: horizon,
                });
            }
            _ => {}
        }
        if it.status == Status::Done && it.evidence.as_ref().is_none_or(|e| e.is_blank()) {
            return Err(Violation::DoneWithoutEvidence(id));
        }
        if it.status == Status::Active {
            for dep in &it.depends_on {
                if s.items[dep].status != Status::Done {
                    return Err(Violation::ActiveBeforeDeps {
                        item: id,
                        dep: *dep,
                    });
                }
            }
        }
        match (it.status, it.blocked_on.is_empty()) {
            (Status::Blocked, true) => return Err(Violation::BlockedWithoutBlocker(id)),
            (st, false) if st != Status::Blocked => return Err(Violation::BlockerOnUnblocked(id)),
            _ => {}
        }
        for b in &it.blocked_on {
            let stale = match b {
                Blocker::Item(w) => s.items[w].status.is_terminal(),
                Blocker::Human(h) => s.asks[h].state != AskState::Open,
            };
            if stale {
                return Err(Violation::StaleBlocker {
                    item: id,
                    blocker: *b,
                });
            }
        }
        if live {
            if let Some(p) = it.parent
                && s.items[&p].status.is_terminal()
            {
                return Err(Violation::LiveUnderTerminal {
                    item: id,
                    parent: p,
                });
            }
            for dep in &it.depends_on {
                if s.items[dep].status == Status::Dropped {
                    return Err(Violation::LiveOnDropped {
                        item: id,
                        dep: *dep,
                    });
                }
            }
            for e in &it.sources {
                if s.entries[e].closed.is_some() {
                    return Err(Violation::LiveOnClosedEntry {
                        item: id,
                        entry: *e,
                    });
                }
            }
        }
        if let Some(t) = &it.pending_transfer
            && (!live || it.owner.is_none() || it.owner.as_ref() == Some(&t.to))
        {
            return Err(Violation::StrayTransfer(id));
        }
    }
    for (h, ask) in &s.asks {
        if !s.items.contains_key(&ask.item) {
            return Err(Violation::AskForUnknownItem {
                ask: *h,
                item: ask.item,
            });
        }
    }
    match find_cycle(&adjacency(&s.items)) {
        Some(cycle) => Err(Violation::Cycle(cycle)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CrewId, SessionId};
    use crate::model::Actor;
    use crate::model::{
        Body, EntryClosure, EntryTodo, Evidence, HumanAsk, PendingTransfer, WaitTier, WorkItem,
    };

    const NOW: Ts = Ts(1_000_000);

    fn item(n: u32) -> WorkItem {
        WorkItem {
            id: WorkId(n),
            title: format!("item {n}"),
            body: Body::default(),
            parent: None,
            depends_on: vec![],
            sources: vec![],
            owner: None,
            owner_history: vec![],
            status: Status::Open,
            blocked_on: vec![],
            next_check_at: None,
            missed_checks: 0,
            evidence: None,
            drop_reason: None,
            pending_transfer: None,
            channel: None,
            last_report: None,
            revision: 1,
            created_at: NOW,
            updated_at: NOW,
        }
    }

    fn owned(n: u32, status: Status) -> WorkItem {
        let mut it = item(n);
        it.owner = Some(SessionId::new("s1"));
        it.status = status;
        if status == Status::Active {
            it.next_check_at = Some(NOW.plus(1_000));
        }
        if status == Status::Done {
            it.evidence = Some(Evidence::Commit("abc".into()));
        }
        it
    }

    fn book(items: Vec<WorkItem>) -> BookState {
        let mut s = BookState::empty(CrewId::new("c"));
        for it in items {
            s.last_item = s.last_item.max(it.id.0);
            s.items.insert(it.id, it);
        }
        s
    }

    fn ask(s: &mut BookState, n: u32, item: u32, state: AskState) {
        s.last_ask = s.last_ask.max(n);
        s.asks.insert(
            AskId(n),
            HumanAsk {
                id: AskId(n),
                item: WorkId(item),
                asked_by: Actor::worker("s1"),
                question: "q".into(),
                default: None,
                asked_at: NOW,
                state,
                escalated: WaitTier::Fresh,
            },
        );
    }

    fn verdict(s: &BookState) -> Result<(), Violation> {
        check(s, &Config::default(), NOW)
    }

    #[test]
    fn a_well_formed_book_passes() {
        let mut child = owned(2, Status::Active);
        child.parent = Some(WorkId(1));
        let mut parent = owned(1, Status::Open);
        parent.depends_on = vec![WorkId(3)];
        let dep = owned(3, Status::Done);
        let mut s = book(vec![parent, child, dep]);
        let mut blocked = owned(4, Status::Blocked);
        blocked.blocked_on = vec![Blocker::Human(AskId(1)), Blocker::Item(WorkId(2))];
        s.items.insert(WorkId(4), blocked);
        s.last_item = 4;
        ask(&mut s, 1, 4, AskState::Open);
        assert_eq!(verdict(&s), Ok(()));
    }

    #[test]
    fn cycle_through_parent_and_dependency_is_rejected() {
        // 父 → 子 是一条边：子项依赖自己的父项就成环（父要等子，子又等父）。
        let mut child = item(2);
        child.parent = Some(WorkId(1));
        child.depends_on = vec![WorkId(1)];
        let s = book(vec![item(1), child]);
        assert_eq!(
            verdict(&s),
            Err(Violation::Cycle(vec![WorkId(1), WorkId(2)]))
        );
    }

    #[test]
    fn parent_may_depend_on_its_own_child() {
        let mut parent = item(1);
        parent.depends_on = vec![WorkId(2)];
        let mut child = item(2);
        child.parent = Some(WorkId(1));
        assert_eq!(verdict(&book(vec![parent, child])), Ok(()));
    }

    #[test]
    fn blocked_on_item_edges_count_toward_cycles() {
        let mut a = owned(1, Status::Blocked);
        a.blocked_on = vec![Blocker::Item(WorkId(2))];
        let mut b = owned(2, Status::Blocked);
        b.blocked_on = vec![Blocker::Item(WorkId(1))];
        assert_eq!(
            verdict(&book(vec![a, b])),
            Err(Violation::Cycle(vec![WorkId(1), WorkId(2)]))
        );
    }

    #[test]
    fn active_needs_owner_check_and_finished_deps() {
        let mut no_owner = owned(1, Status::Active);
        no_owner.owner = None;
        assert_eq!(
            verdict(&book(vec![no_owner])),
            Err(Violation::OwnerMissing(WorkId(1)))
        );

        let mut no_check = owned(1, Status::Active);
        no_check.next_check_at = None;
        assert_eq!(
            verdict(&book(vec![no_check])),
            Err(Violation::ActiveWithoutCheck(WorkId(1)))
        );

        let mut far = owned(1, Status::Active);
        far.next_check_at = Some(NOW.plus(2 * crate::ids::HOUR + 1));
        assert_eq!(
            verdict(&book(vec![far])),
            Err(Violation::CheckTooFar {
                item: WorkId(1),
                at: NOW.plus(2 * crate::ids::HOUR + 1),
                limit: NOW.plus(2 * crate::ids::HOUR),
            })
        );

        let mut edge = owned(1, Status::Active);
        edge.next_check_at = Some(NOW.plus(2 * crate::ids::HOUR));
        assert_eq!(verdict(&book(vec![edge])), Ok(()), "上限本身可以");

        let mut early = owned(1, Status::Active);
        early.depends_on = vec![WorkId(2)];
        assert_eq!(
            verdict(&book(vec![early, owned(2, Status::Active)])),
            Err(Violation::ActiveBeforeDeps {
                item: WorkId(1),
                dep: WorkId(2)
            })
        );
    }

    #[test]
    fn overdue_check_is_still_valid() {
        // 检查时间已过（还没被处理）不是违反：违反的只是「排得太远」。
        let mut it = owned(1, Status::Active);
        it.next_check_at = Some(Ts(0));
        assert_eq!(verdict(&book(vec![it])), Ok(()));
    }

    #[test]
    fn check_time_only_on_owned_open_or_active() {
        let mut unowned = item(1);
        unowned.next_check_at = Some(NOW);
        assert_eq!(
            verdict(&book(vec![unowned])),
            Err(Violation::CheckNotAllowed(WorkId(1)))
        );
        let mut done = owned(1, Status::Done);
        done.next_check_at = Some(NOW);
        assert_eq!(
            verdict(&book(vec![done])),
            Err(Violation::CheckNotAllowed(WorkId(1)))
        );
        let mut open = owned(1, Status::Open);
        open.next_check_at = Some(NOW);
        assert_eq!(verdict(&book(vec![open])), Ok(()));
    }

    #[test]
    fn done_needs_non_blank_evidence() {
        let mut d = owned(1, Status::Done);
        d.evidence = None;
        assert_eq!(
            verdict(&book(vec![d])),
            Err(Violation::DoneWithoutEvidence(WorkId(1)))
        );
        let mut blank = owned(1, Status::Done);
        blank.evidence = Some(Evidence::Text(" ".into()));
        assert_eq!(
            verdict(&book(vec![blank])),
            Err(Violation::DoneWithoutEvidence(WorkId(1)))
        );
    }

    #[test]
    fn blocked_iff_blockers() {
        assert_eq!(
            verdict(&book(vec![owned(1, Status::Blocked)])),
            Err(Violation::BlockedWithoutBlocker(WorkId(1)))
        );
        let mut open = owned(1, Status::Open);
        open.blocked_on = vec![Blocker::Item(WorkId(2))];
        assert_eq!(
            verdict(&book(vec![open, item(2)])),
            Err(Violation::BlockerOnUnblocked(WorkId(1)))
        );
    }

    #[test]
    fn stale_and_unknown_blockers() {
        let mut b = owned(1, Status::Blocked);
        b.blocked_on = vec![Blocker::Human(AskId(1))];
        let mut s = book(vec![b.clone()]);
        assert_eq!(
            verdict(&s),
            Err(Violation::UnknownAskRef {
                item: WorkId(1),
                ask: AskId(1)
            })
        );
        ask(
            &mut s,
            1,
            1,
            AskState::Answered {
                text: "ok".into(),
                at: NOW,
            },
        );
        assert_eq!(
            verdict(&s),
            Err(Violation::StaleBlocker {
                item: WorkId(1),
                blocker: Blocker::Human(AskId(1))
            })
        );
        b.blocked_on = vec![Blocker::Item(WorkId(2))];
        assert_eq!(
            verdict(&book(vec![b, owned(2, Status::Done)])),
            Err(Violation::StaleBlocker {
                item: WorkId(1),
                blocker: Blocker::Item(WorkId(2))
            })
        );
    }

    #[test]
    fn live_items_cannot_hang_on_terminal_things() {
        let mut child = item(2);
        child.parent = Some(WorkId(1));
        assert_eq!(
            verdict(&book(vec![owned(1, Status::Done), child])),
            Err(Violation::LiveUnderTerminal {
                item: WorkId(2),
                parent: WorkId(1)
            })
        );
        let mut waiting = item(2);
        waiting.depends_on = vec![WorkId(1)];
        let mut dropped = item(1);
        dropped.status = Status::Dropped;
        assert_eq!(
            verdict(&book(vec![dropped, waiting])),
            Err(Violation::LiveOnDropped {
                item: WorkId(2),
                dep: WorkId(1)
            })
        );
        let mut from_entry = item(1);
        from_entry.sources = vec![EntryNo(5)];
        let mut s = book(vec![from_entry]);
        assert_eq!(
            verdict(&s),
            Err(Violation::UnknownEntryRef {
                item: WorkId(1),
                entry: EntryNo(5)
            })
        );
        s.entries.insert(
            EntryNo(5),
            EntryTodo {
                no: EntryNo(5),
                registered_at: NOW,
                closed: Some(EntryClosure {
                    by: Actor::Human,
                    note: "no".into(),
                    evidence: None,
                    at: NOW,
                }),
            },
        );
        assert_eq!(
            verdict(&s),
            Err(Violation::LiveOnClosedEntry {
                item: WorkId(1),
                entry: EntryNo(5)
            })
        );
    }

    #[test]
    fn transfers_need_a_live_owned_item_and_a_different_target() {
        let t = |to: &str| PendingTransfer {
            to: SessionId::new(to),
            requested_by: SessionId::new("s1"),
            note: "n".into(),
            at: NOW,
        };
        let mut ok = owned(1, Status::Open);
        ok.pending_transfer = Some(t("s2"));
        assert_eq!(verdict(&book(vec![ok])), Ok(()));
        let mut to_self = owned(1, Status::Open);
        to_self.pending_transfer = Some(t("s1"));
        assert_eq!(
            verdict(&book(vec![to_self])),
            Err(Violation::StrayTransfer(WorkId(1)))
        );
        let mut done = owned(1, Status::Done);
        done.pending_transfer = Some(t("s2"));
        assert_eq!(
            verdict(&book(vec![done])),
            Err(Violation::StrayTransfer(WorkId(1)))
        );
    }

    #[test]
    fn bookkeeping_shape_is_checked() {
        let mut s = book(vec![item(1)]);
        s.last_item = 0;
        assert_eq!(verdict(&s), Err(Violation::CounterBehind));
        let mut s = book(vec![item(1)]);
        let moved = s.items.remove(&WorkId(1)).unwrap();
        s.items.insert(WorkId(9), moved);
        s.last_item = 9;
        assert_eq!(verdict(&s), Err(Violation::KeyMismatch(WorkId(9))));
        let mut blank = item(1);
        blank.title = "   ".into();
        assert_eq!(
            verdict(&book(vec![blank])),
            Err(Violation::BlankTitle(WorkId(1)))
        );
        let mut dup = item(1);
        dup.depends_on = vec![WorkId(2), WorkId(2)];
        assert_eq!(
            verdict(&book(vec![dup, item(2)])),
            Err(Violation::DuplicateRef(WorkId(1)))
        );
        let mut dangling = item(1);
        dangling.parent = Some(WorkId(7));
        let mut s = book(vec![dangling]);
        s.last_item = 7;
        assert_eq!(
            verdict(&s),
            Err(Violation::UnknownItemRef {
                item: WorkId(1),
                missing: WorkId(7)
            })
        );
        let mut s = book(vec![item(1)]);
        ask(&mut s, 1, 4, AskState::Open);
        assert_eq!(
            verdict(&s),
            Err(Violation::AskForUnknownItem {
                ask: AskId(1),
                item: WorkId(4)
            })
        );
    }
}
