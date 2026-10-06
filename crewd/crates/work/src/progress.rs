//! 推进与收尾：回报进度 + 下次检查（`work_item_update`）、带证据完成、放弃、改链接、入口裁定关闭。
use crate::book::{Applied, Rejected, WorkBook};
use crate::events::EventKind;
use crate::ids::{EntryNo, Ts, WorkId};
use crate::model::{Actor, Blocker, DropReason, EntryClosure, Evidence, Note, Status};
use crate::perm::{self, Action, Ctx};

/// 回报时可以改到的状态（done / dropped / blocked 各有专门的命令）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Open,
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub status: Option<Progress>,
    /// 必须晚于 now，且不超过 now + 上限（默认 2h）。
    pub next_check_at: Option<Ts>,
    /// 进展 / 下一步（必填；注入「你名下的工作项」时显示）。
    pub note: String,
}

impl WorkBook {
    pub fn report(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        r: Report,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            let now = d.now;
            d.permit_on(Action::Report, id)?;
            let note = r.note.trim().to_string();
            if note.is_empty() {
                return Err(Rejected::Blank("note"));
            }
            if r.next_check_at.is_some_and(|t| t <= now) {
                return Err(Rejected::NextCheckNotFuture);
            }
            let it = d.item_mut(id)?;
            let from = it.status;
            if let Some(p) = r.status {
                // 从 blocked 手动改回 open / active = 不再等；问题本身留着，人仍可回答。
                it.blocked_on.clear();
                it.status = match p {
                    Progress::Open => Status::Open,
                    Progress::Active => Status::Active,
                };
            }
            if let Some(t) = r.next_check_at {
                it.next_check_at = Some(t);
            }
            it.last_report = Some(Note {
                text: note.clone(),
                at: now,
            });
            let (to, next_check_at) = (it.status, it.next_check_at);
            d.emit(EventKind::Reported {
                id,
                from,
                to,
                next_check_at,
                note,
            });
            Ok(())
        })
    }

    /// 完成，证据必填。卡在这一项上的其它项随之放行；返回被放行的项。
    pub fn mark_done(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        evidence: Evidence,
    ) -> Result<Applied<Vec<WorkId>>, Rejected> {
        self.transact(by, now, |d| {
            d.permit_on(Action::Done, id)?;
            if evidence.is_blank() {
                return Err(Rejected::Blank("evidence"));
            }
            let it = d.item_mut(id)?;
            it.status = Status::Done;
            it.evidence = Some(evidence.clone());
            it.next_check_at = None;
            it.pending_transfer = None;
            it.blocked_on.clear();
            d.emit(EventKind::Done { id, evidence });
            Ok(d.release(Blocker::Item(id)))
        })
    }

    /// 放弃。重复 / 被取代 = 事实性关闭（行政处可做，必须带证据）；不做了 = 业务决定。
    /// 事实性关闭时，卡在这一项上的项随之放行、叫醒负责人（同 `mark_done`；那件事在别处做 / 已被取代）；
    /// 返回被放行的项。业务放弃不替等它的人做决定：还有活着的子项、依赖它的项、卡在它上面的项时被拒（不变量）。
    pub fn drop_item(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        reason: DropReason,
    ) -> Result<Applied<Vec<WorkId>>, Rejected> {
        self.transact(by, now, |d| {
            let action = if reason.kind.is_factual() {
                Action::DropFactual
            } else {
                Action::DropDecision
            };
            d.permit_on(action, id)?;
            if reason.note.trim().is_empty() {
                return Err(Rejected::Blank("note"));
            }
            if reason.kind.is_factual() && reason.evidence.as_ref().is_none_or(|e| e.is_blank()) {
                return Err(Rejected::Blank("evidence"));
            }
            let it = d.item_mut(id)?;
            it.status = Status::Dropped;
            it.drop_reason = Some(reason.clone());
            it.next_check_at = None;
            it.pending_transfer = None;
            it.blocked_on.clear();
            let factual = reason.kind.is_factual();
            d.emit(EventKind::Dropped { id, reason });
            Ok(if factual {
                d.release(Blocker::Item(id))
            } else {
                Vec::new()
            })
        })
    }

    /// 改链接（整份替换 parent 与 dependsOn）。成环、依赖未齐却 active 等由不变量拒。
    pub fn relink(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        parent: Option<WorkId>,
        depends_on: Vec<WorkId>,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            d.permit_on(Action::Relink, id)?;
            for w in parent.iter().chain(&depends_on) {
                d.item(*w)?;
            }
            if let Some(p) = parent
                && d.item(id)?.parent != Some(p)
            {
                // 挂到新父项下 = 在它下面建子项：与 insert_batch 同一道 CreateChild 检查（对新父项的负责人）。
                // 能在新父项上改链接的人（机长 / 人 / 行政处的事实修正、新父项自己的负责人）同样放行。
                let owner = d.item(p)?.owner.clone();
                let ctx = Ctx {
                    owner: owner.as_ref(),
                    ..Ctx::default()
                };
                if !perm::allowed(&d.by, Action::CreateChild, ctx)
                    && !perm::allowed(&d.by, Action::Relink, ctx)
                {
                    return Err(Rejected::Forbidden(Action::CreateChild));
                }
            }
            let mut deps = depends_on;
            deps.sort();
            deps.dedup();
            let it = d.item_mut(id)?;
            it.parent = parent;
            it.depends_on = deps.clone();
            d.emit(EventKind::Relinked {
                id,
                parent,
                depends_on: deps,
            });
            Ok(())
        })
    }

    /// 入口裁定关闭（spec §4.1「全部 dropped → 交机长/人裁定」）。带证据 = 事实关闭（行政处可做）。
    /// 还有活着的关联项时被拒（不变量）。
    pub fn close_entry(
        &mut self,
        by: &Actor,
        now: Ts,
        no: EntryNo,
        note: &str,
        evidence: Option<Evidence>,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            let now = d.now;
            let action = if evidence.is_some() {
                Action::CloseEntryFactual
            } else {
                Action::CloseEntryDecision
            };
            d.permit(action, Ctx::default())?;
            let note = note.trim().to_string();
            if note.is_empty() {
                return Err(Rejected::Blank("note"));
            }
            if evidence.as_ref().is_some_and(|e| e.is_blank()) {
                return Err(Rejected::Blank("evidence"));
            }
            let by = d.by.clone();
            let e = d.s.entries.get_mut(&no).ok_or(Rejected::UnknownEntry(no))?;
            if e.closed.is_some() {
                return Err(Rejected::EntryClosed(no));
            }
            e.closed = Some(EntryClosure {
                by,
                note: note.clone(),
                evidence,
                at: now,
            });
            d.emit(EventKind::EntryClosed { entry: no, note });
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockOn, Continuation};
    use crate::ids::{AskId, HOUR, MINUTE};
    use crate::invariants::Violation;
    use crate::model::{DropKind, ItemRef, NewItem};
    use crate::testkit::*;

    fn report(status: Option<Progress>, next: Option<Ts>, note: &str) -> Report {
        Report {
            status,
            next_check_at: next,
            note: note.into(),
        }
    }

    fn commit() -> Evidence {
        Evidence::Commit("abc1234".into())
    }

    fn not_needed() -> DropReason {
        DropReason {
            kind: DropKind::NotNeeded,
            note: "需求取消".into(),
            evidence: None,
        }
    }

    #[test]
    fn reporting_starts_work_with_a_next_check() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let at = T1.plus(MINUTE);
        let got = b
            .report(
                &Actor::worker("a"),
                at,
                w,
                report(Some(Progress::Active), Some(at.plus(HOUR)), "先写 schema"),
            )
            .unwrap();
        assert_eq!(
            got.events[0].kind,
            EventKind::Reported {
                id: w,
                from: Status::Open,
                to: Status::Active,
                next_check_at: Some(at.plus(HOUR)),
                note: "先写 schema".into()
            }
        );
        let it = b.item(w).unwrap();
        assert_eq!(
            (it.status, it.next_check_at),
            (Status::Active, Some(at.plus(HOUR)))
        );
        assert_eq!(
            it.last_report,
            Some(Note {
                text: "先写 schema".into(),
                at
            })
        );
        assert_eq!((it.revision, it.updated_at), (2, at));
    }

    #[test]
    fn report_rules() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let a = Actor::worker("a");
        assert_eq!(
            b.report(&a, T1, w, report(Some(Progress::Active), None, "go")),
            Err(Rejected::Invariant(Violation::ActiveWithoutCheck(w)))
        );
        assert_eq!(
            b.report(&a, T1, w, report(Some(Progress::Active), Some(T1), "go")),
            Err(Rejected::NextCheckNotFuture)
        );
        assert_eq!(
            b.report(
                &a,
                T1,
                w,
                report(Some(Progress::Active), Some(T1.plus(2 * HOUR + 1)), "go")
            ),
            Err(Rejected::Invariant(Violation::CheckTooFar {
                item: w,
                at: T1.plus(2 * HOUR + 1),
                limit: T1.plus(2 * HOUR)
            }))
        );
        assert_eq!(
            b.report(&a, T1, w, report(None, None, " ")),
            Err(Rejected::Blank("note"))
        );
        assert_eq!(
            b.report(&Actor::worker("b"), T1, w, report(None, None, "x")),
            Err(Rejected::Forbidden(Action::Report))
        );
        assert_eq!(
            b.report(&Actor::admin("d"), T1, w, report(None, None, "x")),
            Err(Rejected::Forbidden(Action::Report)),
            "行政处不改业务状态"
        );
        assert!(
            b.report(
                &Actor::captain("c"),
                T1,
                w,
                report(Some(Progress::Active), Some(T1.plus(HOUR)), "机长代改")
            )
            .is_ok()
        );
    }

    #[test]
    fn active_requires_finished_dependencies() {
        let mut b = book();
        let ids = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("dep"),
                    NewItem::titled("x").after(ItemRef::Batch(0)),
                ],
            )
            .unwrap()
            .value;
        let go = report(Some(Progress::Active), Some(T1.plus(HOUR)), "go");
        assert_eq!(
            b.report(&Actor::worker("a"), T1, ids[1], go.clone()),
            Err(Rejected::Invariant(Violation::ActiveBeforeDeps {
                item: ids[1],
                dep: ids[0]
            }))
        );
        b.mark_done(&Actor::worker("a"), T1, ids[0], commit())
            .unwrap();
        assert!(b.report(&Actor::worker("a"), T1, ids[1], go).is_ok());
    }

    #[test]
    fn manual_resume_from_blocked_clears_blockers_but_keeps_the_ask() {
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
            Continuation::Nothing,
        )
        .unwrap();
        assert_eq!(
            b.report(
                &Actor::worker("a"),
                T1,
                w,
                report(None, Some(T1.plus(HOUR)), "x")
            ),
            Err(Rejected::Invariant(Violation::CheckNotAllowed(w))),
            "blocked 项不排检查"
        );
        b.report(
            &Actor::worker("a"),
            T1,
            w,
            report(Some(Progress::Active), Some(T1.plus(HOUR)), "绕过去了"),
        )
        .unwrap();
        assert!(b.item(w).unwrap().blocked_on.is_empty());
        assert!(b.ask(AskId(1)).is_some());
    }

    #[test]
    fn done_needs_evidence_and_finished_children() {
        let mut b = book();
        let w = claim(&mut b, "a", &["parent"])[0];
        let child = b
            .create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("child").under(ItemRef::Id(w)).owned_by("a"),
            )
            .unwrap()
            .value;
        assert_eq!(
            b.mark_done(&Actor::worker("a"), T1, w, Evidence::Text("  ".into())),
            Err(Rejected::Blank("evidence"))
        );
        assert_eq!(
            b.mark_done(&Actor::worker("a"), T1, w, commit()),
            Err(Rejected::Invariant(Violation::LiveUnderTerminal {
                item: child,
                parent: w
            }))
        );
        b.drop_item(&Actor::worker("a"), T1, child, not_needed())
            .unwrap();
        let got = b.mark_done(&Actor::worker("a"), T1, w, commit()).unwrap();
        assert_eq!(kinds(&got.events), ["done"]);
        assert_eq!(b.item(w).unwrap().evidence, Some(commit()));
        assert_eq!(
            b.mark_done(&Actor::worker("a"), T1, w, commit()),
            Err(Rejected::Terminal(w))
        );
    }

    #[test]
    fn finishing_an_item_releases_whoever_waited_on_it() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["blocker", "waiter"]);
        b.block(
            &Actor::worker("a"),
            T1,
            ids[1],
            vec![BlockOn::Item(ids[0])],
            Continuation::Nothing,
        )
        .unwrap();
        let later = T1.plus(HOUR);
        let got = b
            .mark_done(&Actor::worker("a"), later, ids[0], commit())
            .unwrap();
        assert_eq!(got.value, [ids[1]]);
        assert_eq!(kinds(&got.events), ["done", "blocker_cleared"]);
        let it = b.item(ids[1]).unwrap();
        assert_eq!((it.status, it.next_check_at), (Status::Active, Some(later)));
    }

    #[test]
    fn admin_closes_only_with_evidence() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y"]);
        let admin = Actor::admin("d");
        assert_eq!(
            b.drop_item(&admin, T1, ids[0], not_needed()),
            Err(Rejected::Forbidden(Action::DropDecision))
        );
        let dup = |evidence| DropReason {
            kind: DropKind::Duplicate,
            note: "与 W2 重复".into(),
            evidence,
        };
        assert_eq!(
            b.drop_item(&admin, T1, ids[0], dup(None)),
            Err(Rejected::Blank("evidence"))
        );
        b.drop_item(
            &admin,
            T1,
            ids[0],
            dup(Some(Evidence::Url("https://chat/123".into()))),
        )
        .unwrap();
        assert_eq!(b.item(ids[0]).unwrap().status, Status::Dropped);
        b.mark_done(&admin, T1, ids[1], commit()).unwrap();
        assert_eq!(b.item(ids[1]).unwrap().status, Status::Done);
    }

    #[test]
    fn cannot_drop_what_others_still_need() {
        let mut b = book();
        let ids = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("dep"),
                    NewItem::titled("x").after(ItemRef::Batch(0)),
                ],
            )
            .unwrap()
            .value;
        assert_eq!(
            b.drop_item(&Actor::worker("a"), T1, ids[0], not_needed()),
            Err(Rejected::Invariant(Violation::LiveOnDropped {
                item: ids[1],
                dep: ids[0]
            }))
        );
        b.relink(&Actor::worker("a"), T1, ids[1], None, vec![])
            .unwrap();
        assert!(
            b.drop_item(&Actor::worker("a"), T1, ids[0], not_needed())
                .is_ok()
        );
    }

    #[test]
    fn relink_is_checked_for_cycles_and_readiness() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y"]);
        b.relink(&Actor::worker("a"), T1, ids[0], None, vec![ids[1]])
            .unwrap();
        assert_eq!(
            b.relink(&Actor::admin("d"), T1, ids[1], None, vec![ids[0]]),
            Err(Rejected::Invariant(Violation::Cycle(vec![ids[0], ids[1]])))
        );
        assert_eq!(
            b.relink(&Actor::worker("a"), T1, ids[1], Some(WorkId(9)), vec![]),
            Err(Rejected::UnknownItem(WorkId(9)))
        );
        activate(&mut b, "a", ids[1], T1);
        let got = b
            .relink(&Actor::admin("d"), T1, ids[1], Some(ids[0]), vec![])
            .unwrap();
        assert_eq!(
            got.events[0].kind,
            EventKind::Relinked {
                id: ids[1],
                parent: Some(ids[0]),
                depends_on: vec![]
            }
        );
    }

    #[test]
    fn entry_ruling_closes_it_for_good() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        assert_eq!(
            b.close_entry(&Actor::captain("c"), T1, EntryNo(1), "不做了", None),
            Err(Rejected::Invariant(Violation::LiveOnClosedEntry {
                item: w,
                entry: EntryNo(1)
            }))
        );
        b.drop_item(&Actor::worker("a"), T1, w, not_needed())
            .unwrap();
        assert_eq!(
            b.close_entry(&Actor::admin("d"), T1, EntryNo(1), "不做了", None),
            Err(Rejected::Forbidden(Action::CloseEntryDecision))
        );
        assert_eq!(
            b.close_entry(&Actor::captain("c"), T1, EntryNo(1), " ", None),
            Err(Rejected::Blank("note"))
        );
        b.close_entry(&Actor::captain("c"), T1, EntryNo(1), "不做了", None)
            .unwrap();
        assert_eq!(
            b.close_entry(&Actor::Human, T1, EntryNo(1), "again", None),
            Err(Rejected::EntryClosed(EntryNo(1)))
        );
        assert_eq!(
            b.claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("y")]
            ),
            Err(Rejected::EntryClosed(EntryNo(1)))
        );
        b.register_entry(T1, EntryNo(2)).unwrap();
        assert!(
            b.close_entry(
                &Actor::admin("d"),
                T1,
                EntryNo(2),
                "已在 #1 做完",
                Some(Evidence::Commit("abc".into()))
            )
            .is_ok()
        );
    }

    #[test]
    fn relink_under_someone_elses_item_needs_the_same_right_as_creating_a_child() {
        // 评审 P1：worker 不能借改链接把自己的项挂到别人的项下（绕过 CreateChild，还会卡住对方完成）。
        let mut b = book();
        let theirs = claim(&mut b, "b", &["b 的项"])[0];
        let mine = b
            .create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("a 的项").owned_by("a"),
            )
            .unwrap()
            .value;
        assert_eq!(
            b.relink(&Actor::worker("a"), T1, mine, Some(theirs), vec![]),
            Err(Rejected::Forbidden(Action::CreateChild))
        );
        assert_eq!(b.item(mine).unwrap().parent, None, "被拒不动账本");
        assert!(
            b.mark_done(&Actor::worker("b"), T1, theirs, commit())
                .is_ok(),
            "对方的项照样能完成"
        );
        let theirs = b
            .create_item(
                &Actor::worker("b"),
                T1,
                NewItem::titled("b 的另一项").owned_by("b"),
            )
            .unwrap()
            .value;
        assert!(
            b.relink(&Actor::worker("a"), T1, mine, None, vec![theirs])
                .is_ok(),
            "依赖别人的项不需要对方的权限"
        );
        assert!(
            b.relink(&Actor::captain("c"), T1, mine, Some(theirs), vec![])
                .is_ok(),
            "机长可以改到任何父项下"
        );
        assert!(
            b.relink(&Actor::worker("a"), T1, mine, Some(theirs), vec![])
                .is_ok(),
            "父项没变就不再问一次"
        );
        let own_parent = b
            .create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("a 的父项").owned_by("a"),
            )
            .unwrap()
            .value;
        assert!(
            b.relink(&Actor::admin("d"), T1, mine, Some(own_parent), vec![])
                .is_ok(),
            "行政处改链接（事实修正）照旧可以"
        );
    }

    #[test]
    fn a_factual_drop_releases_whoever_waited_on_it() {
        // 评审 P11：被判重复 / 被取代的项，卡在它上面的项随之放行、叫醒其负责人（同 mark_done）。
        let mut b = book();
        let dup = claim(&mut b, "a", &["重复的那件"])[0];
        let waiter = b
            .create_item(
                &Actor::worker("b"),
                T1,
                NewItem::titled("等它").owned_by("b"),
            )
            .unwrap()
            .value;
        b.block(
            &Actor::worker("b"),
            T1,
            waiter,
            vec![BlockOn::Item(dup)],
            Continuation::Nothing,
        )
        .unwrap();
        assert_eq!(
            b.drop_item(&Actor::worker("a"), T1, dup, not_needed()),
            Err(Rejected::Invariant(Violation::StaleBlocker {
                item: waiter,
                blocker: Blocker::Item(dup)
            })),
            "业务上的「不做了」不替等它的人做决定：仍然拒"
        );
        let later = T1.plus(HOUR);
        let got = b
            .drop_item(
                &Actor::admin("d"),
                later,
                dup,
                DropReason {
                    kind: DropKind::Duplicate,
                    note: "与 W9 重复".into(),
                    evidence: Some(Evidence::Text("W9".into())),
                },
            )
            .unwrap();
        assert_eq!(got.value, [waiter]);
        assert_eq!(kinds(&got.events), ["dropped", "blocker_cleared"]);
        assert_eq!(
            got.events[1].kind,
            EventKind::BlockerCleared {
                id: waiter,
                owner: Some(sid("b")),
                blocker: Blocker::Item(dup),
                status: Status::Active
            }
        );
        let it = b.item(waiter).unwrap();
        assert_eq!((it.status, it.next_check_at), (Status::Active, Some(later)));
    }

    #[test]
    fn the_worker_who_asked_for_help_may_cancel_it_while_nobody_took_it() {
        // 评审 P10：request_help 建出的无主项，发起的 worker 可以撤回（业务放弃）；有人接了就归负责人管。
        let mut b = book();
        let help = |b: &mut WorkBook| {
            b.create_item(&Actor::worker("a"), T1, NewItem::titled("帮我查日志"))
                .unwrap()
                .value
        };
        let first = help(&mut b);
        assert_eq!(b.item(first).unwrap().created_by, Some(sid("a")));
        assert_eq!(
            b.drop_item(&Actor::worker("b"), T1, first, not_needed()),
            Err(Rejected::Forbidden(Action::DropDecision)),
            "别人不行"
        );
        assert!(
            b.drop_item(&Actor::worker("a"), T1, first, not_needed())
                .is_ok()
        );
        let second = help(&mut b);
        b.assign(&Actor::System, T1, second, sid("c"), "c 来")
            .unwrap();
        assert_eq!(
            b.drop_item(&Actor::worker("a"), T1, second, not_needed()),
            Err(Rejected::Forbidden(Action::DropDecision)),
            "有人接了就不再是发起人能撤的"
        );
    }
}
