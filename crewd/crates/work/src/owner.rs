//! 负责人（spec §5）：转交（原负责人发起 + 新负责人接受）、机长 / 人 / 调度层指派、失联重派，
//! 以及检查点叫醒的记账（失联判定的依据）。交接说明一律必填，写进 ownerHistory。
use crate::book::{Applied, Draft, Rejected, WorkBook};
use crate::events::EventKind;
use crate::ids::{SessionId, Ts, WorkId};
use crate::model::{Actor, HowChanged, OwnerChange, PendingTransfer, Status};
use crate::perm::{Action, Ctx};

impl WorkBook {
    /// `work_item_transfer_request`：只有现负责人能发起；同一时刻只挂一条。
    pub fn request_transfer(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        to: SessionId,
        note: &str,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            let now = d.now;
            d.permit_on(Action::TransferRequest, id)?;
            let note = note.trim().to_string();
            if note.is_empty() {
                return Err(Rejected::Blank("note"));
            }
            let it = d.live(id)?;
            if it.owner.as_ref() == Some(&to) {
                return Err(Rejected::SameOwner);
            }
            if it.pending_transfer.is_some() {
                return Err(Rejected::TransferPending(id));
            }
            let requested_by =
                d.by.session()
                    .cloned()
                    .expect("permit: only sessions request");
            d.item_mut(id)?.pending_transfer = Some(PendingTransfer {
                to: to.clone(),
                requested_by,
                note: note.clone(),
                at: now,
            });
            d.emit(EventKind::TransferRequested { id, to, note });
            Ok(())
        })
    }

    /// `work_item_transfer_accept`：只有被转交的人能接受。状态与检查时间照旧（接受的人此刻就醒着）。
    pub fn accept_transfer(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            if d.live(id)?.pending_transfer.is_none() {
                return Err(Rejected::NoPendingTransfer(id));
            }
            d.permit_on(Action::TransferAccept, id)?;
            let t = d
                .item_mut(id)?
                .pending_transfer
                .take()
                .expect("checked above");
            d.change_owner(id, t.to, HowChanged::Transfer, t.note, false)
        })
    }

    /// 撤回（原负责人）/ 拒绝（被转交的人）/ 机长取消一条待接受的转交。
    pub fn withdraw_transfer(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            if d.live(id)?.pending_transfer.is_none() {
                return Err(Rejected::NoPendingTransfer(id));
            }
            d.permit_on(Action::TransferWithdraw, id)?;
            d.item_mut(id)?.pending_transfer = None;
            d.emit(EventKind::TransferWithdrawn { id });
            Ok(())
        })
    }

    /// `work_item_assign`：机长 / 人 / 调度层强制指派；worker 只能把无主项领给自己。
    /// active 项换了人 → nextCheckAt = now（新负责人还不知道，下一轮检查点叫醒它）。
    pub fn assign(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        to: SessionId,
        note: &str,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            let it = d.live(id)?;
            d.permit(
                Action::Assign,
                Ctx {
                    owner: it.owner.as_ref(),
                    assignee: Some(&to),
                    ..Ctx::default()
                },
            )?;
            if it.owner.as_ref() == Some(&to) {
                return Err(Rejected::SameOwner);
            }
            let how = if d.by.session() == Some(&to) {
                HowChanged::Claim
            } else {
                HowChanged::Assign
            };
            d.change_owner(id, to, how, note.to_string(), true)
        })
    }

    /// 负责人失联后重派（spec §5：排除原负责人）。调度层 / 行政处 / 机长 / 人可做。
    pub fn reassign_lost(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        to: SessionId,
        note: &str,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            if d.live(id)?.owner.is_none() {
                return Err(Rejected::NotOwned(id));
            }
            d.permit_on(Action::ReassignLost, id)?;
            if d.item(id)?.owner.as_ref() == Some(&to) {
                return Err(Rejected::SameOwner);
            }
            d.change_owner(id, to, HowChanged::Lost, note.to_string(), true)
        })
    }

    /// crewd 刚给负责人投递了一次检查点唤醒（列出这些项）：每项 missedChecks + 1，
    /// 排了检查时间的项顺延到 now + recheck_after（不超过检查上限），到时还没回报就再叫 / 判失联。
    pub fn record_checkpoint_wake(
        &mut self,
        by: &Actor,
        now: Ts,
        ids: &[WorkId],
    ) -> Result<Applied<()>, Rejected> {
        let recheck = self.cfg.recheck_after_ms.min(self.cfg.max_check_horizon_ms);
        self.transact(by, now, |d| {
            let now = d.now;
            d.permit(Action::RecordWake, Ctx::default())?;
            for &id in ids {
                let it = d.live(id)?;
                let owner = it.owner.clone().ok_or(Rejected::NotOwned(id))?;
                if it.status == Status::Blocked {
                    return Err(Rejected::Invalid("blocked items are not checkpointed"));
                }
                let it = d.item_mut(id)?;
                it.missed_checks += 1;
                if it.next_check_at.is_some() {
                    it.next_check_at = Some(now.plus(recheck));
                }
                let (missed_checks, next_check_at) = (it.missed_checks, it.next_check_at);
                d.emit(EventKind::CheckpointWake {
                    id,
                    owner,
                    missed_checks,
                    next_check_at,
                });
            }
            Ok(())
        })
    }
}

impl Draft {
    /// 换负责人的唯一入口：写 ownerHistory、清待接受的转交与 missedChecks、产出 `OwnerChanged`
    /// （带关联群，M4b 把新负责人拉进去）。`wake` = active 项把检查时间提到 now。
    fn change_owner(
        &mut self,
        id: WorkId,
        to: SessionId,
        how: HowChanged,
        note: String,
        wake: bool,
    ) -> Result<(), Rejected> {
        let note = note.trim().to_string();
        if note.is_empty() {
            return Err(Rejected::Blank("note"));
        }
        let (by, now) = (self.by.clone(), self.now);
        let it = self.item_mut(id)?;
        let from = it.owner.replace(to.clone());
        it.owner_history.push(OwnerChange {
            from: from.clone(),
            to: to.clone(),
            by,
            how,
            note: note.clone(),
            at: now,
        });
        it.pending_transfer = None;
        it.missed_checks = 0;
        if wake && it.status == Status::Active {
            it.next_check_at = Some(now);
        }
        let channel = it.channel.clone();
        self.emit(EventKind::OwnerChanged {
            id,
            from,
            to,
            how,
            note,
            channel,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ChannelId, HOUR, MINUTE};
    use crate::model::NewItem;
    use crate::progress::Report;
    use crate::testkit::*;

    #[test]
    fn transfer_is_request_then_accept() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        b.link_channel(&Actor::worker("a"), T1, w, Some(ChannelId::new("ch")))
            .unwrap();
        activate(&mut b, "a", w, T1);
        let a = Actor::worker("a");
        assert_eq!(
            b.request_transfer(&a, T1, w, sid("b"), " "),
            Err(Rejected::Blank("note"))
        );
        assert_eq!(
            b.request_transfer(&a, T1, w, sid("a"), "n"),
            Err(Rejected::SameOwner)
        );
        assert_eq!(
            b.request_transfer(&Actor::worker("b"), T1, w, sid("b"), "给我吧"),
            Err(Rejected::Forbidden(Action::TransferRequest))
        );
        b.request_transfer(&a, T1, w, sid("b"), "我要去修发版")
            .unwrap();
        assert_eq!(
            b.request_transfer(&a, T1, w, sid("c"), "n"),
            Err(Rejected::TransferPending(w))
        );
        assert_eq!(
            b.accept_transfer(&Actor::worker("c"), T1, w),
            Err(Rejected::Forbidden(Action::TransferAccept))
        );
        let later = T1.plus(MINUTE);
        let got = b.accept_transfer(&Actor::worker("b"), later, w).unwrap();
        assert_eq!(
            got.events[0].kind,
            EventKind::OwnerChanged {
                id: w,
                from: Some(sid("a")),
                to: sid("b"),
                how: HowChanged::Transfer,
                note: "我要去修发版".into(),
                channel: Some(ChannelId::new("ch"))
            }
        );
        let it = b.item(w).unwrap();
        assert_eq!(it.owner, Some(sid("b")));
        assert_eq!(it.pending_transfer, None);
        assert_eq!(it.status, Status::Active);
        assert_eq!(
            it.next_check_at,
            Some(T1.plus(HOUR)),
            "接受的人醒着，不提前检查"
        );
        assert_eq!(it.owner_history.last().unwrap().by, Actor::worker("b"));
        assert_eq!(
            b.accept_transfer(&Actor::worker("b"), later, w),
            Err(Rejected::NoPendingTransfer(w))
        );
    }

    #[test]
    fn pending_transfer_can_be_withdrawn_declined_or_cancelled() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let req = |b: &mut WorkBook| {
            b.request_transfer(&Actor::worker("a"), T1, w, sid("b"), "n")
                .unwrap()
        };
        assert_eq!(
            b.withdraw_transfer(&Actor::worker("a"), T1, w),
            Err(Rejected::NoPendingTransfer(w))
        );
        for who in [Actor::worker("a"), Actor::worker("b"), Actor::captain("c")] {
            req(&mut b);
            let got = b.withdraw_transfer(&who, T1, w).unwrap();
            assert_eq!(kinds(&got.events), ["transfer_withdrawn"]);
            assert_eq!(b.item(w).unwrap().pending_transfer, None);
        }
        req(&mut b);
        assert_eq!(
            b.withdraw_transfer(&Actor::worker("x"), T1, w),
            Err(Rejected::Forbidden(Action::TransferWithdraw))
        );
    }

    #[test]
    fn captain_assignment_wakes_the_new_owner_of_active_work() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        activate(&mut b, "a", w, T1);
        b.request_transfer(&Actor::worker("a"), T1, w, sid("b"), "n")
            .unwrap();
        let later = T1.plus(10 * MINUTE);
        assert_eq!(
            b.assign(&Actor::admin("d"), later, w, sid("c"), "n"),
            Err(Rejected::Forbidden(Action::Assign)),
            "强制指派是机长权力"
        );
        assert_eq!(
            b.assign(&Actor::captain("k"), later, w, sid("a"), "n"),
            Err(Rejected::SameOwner)
        );
        assert_eq!(
            b.assign(&Actor::captain("k"), later, w, sid("c"), ""),
            Err(Rejected::Blank("note"))
        );
        b.assign(&Actor::captain("k"), later, w, sid("c"), "c 更熟这块")
            .unwrap();
        let it = b.item(w).unwrap();
        assert_eq!(it.owner, Some(sid("c")));
        assert_eq!(it.next_check_at, Some(later));
        assert_eq!(it.pending_transfer, None, "强制指派顶掉待接受的转交");
        assert_eq!(it.owner_history.last().unwrap().how, HowChanged::Assign);
    }

    #[test]
    fn workers_may_only_pick_up_unowned_items_for_themselves() {
        let mut b = book();
        let orphan = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("帮忙"))
            .unwrap()
            .value;
        assert_eq!(
            b.assign(&Actor::worker("a"), T1, orphan, sid("b"), "n"),
            Err(Rejected::Forbidden(Action::Assign))
        );
        b.assign(&Actor::worker("b"), T1, orphan, sid("b"), "我来")
            .unwrap();
        let it = b.item(orphan).unwrap();
        assert_eq!(it.owner, Some(sid("b")));
        assert_eq!(it.owner_history[0].how, HowChanged::Claim);
        assert_eq!(it.owner_history[0].from, None);
        assert_eq!(
            b.assign(&Actor::worker("c"), T1, orphan, sid("c"), "我也要"),
            Err(Rejected::Forbidden(Action::Assign))
        );
        assert!(
            b.assign(&Actor::System, T1, orphan, sid("c"), "调度层改派")
                .is_ok()
        );
    }

    #[test]
    fn lost_owner_is_replaced_by_someone_else() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        activate(&mut b, "a", w, T1);
        let later = T1.plus(3 * HOUR);
        assert_eq!(
            b.reassign_lost(&Actor::System, later, w, sid("a"), "失联"),
            Err(Rejected::SameOwner)
        );
        assert_eq!(
            b.reassign_lost(&Actor::worker("b"), later, w, sid("b"), "失联"),
            Err(Rejected::Forbidden(Action::ReassignLost))
        );
        b.reassign_lost(&Actor::admin("d"), later, w, sid("b"), "a 两次没回报")
            .unwrap();
        let it = b.item(w).unwrap();
        assert_eq!(
            (it.owner.clone(), it.next_check_at),
            (Some(sid("b")), Some(later))
        );
        assert_eq!(it.owner_history.last().unwrap().how, HowChanged::Lost);
        let orphan = b
            .create_item(&Actor::worker("a"), later, NewItem::titled("o"))
            .unwrap()
            .value;
        assert_eq!(
            b.reassign_lost(&Actor::System, later, orphan, sid("b"), "n"),
            Err(Rejected::NotOwned(orphan))
        );
    }

    #[test]
    fn checkpoint_wakes_count_until_the_owner_reports() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        activate(&mut b, "a", w, T1);
        let due = T1.plus(HOUR);
        assert_eq!(
            b.record_checkpoint_wake(&Actor::worker("a"), due, &[w]),
            Err(Rejected::Forbidden(Action::RecordWake))
        );
        let got = b.record_checkpoint_wake(&Actor::System, due, &[w]).unwrap();
        assert_eq!(
            got.events[0].kind,
            EventKind::CheckpointWake {
                id: w,
                owner: sid("a"),
                missed_checks: 1,
                next_check_at: Some(due.plus(15 * MINUTE))
            }
        );
        b.record_checkpoint_wake(&Actor::System, due.plus(15 * MINUTE), &[w])
            .unwrap();
        assert_eq!(b.item(w).unwrap().missed_checks, 2);
        b.report(
            &Actor::captain("c"),
            due.plus(16 * MINUTE),
            w,
            Report {
                status: None,
                next_check_at: None,
                note: "机长问过了".into(),
            },
        )
        .unwrap();
        assert_eq!(b.item(w).unwrap().missed_checks, 2, "别人替它说话不算回报");
        b.report(
            &Actor::worker("a"),
            due.plus(17 * MINUTE),
            w,
            Report {
                status: None,
                next_check_at: None,
                note: "还在跑测试".into(),
            },
        )
        .unwrap();
        assert_eq!(
            b.item(w).unwrap().missed_checks,
            0,
            "负责人的任何操作都清零"
        );
    }

    #[test]
    fn checkpoint_wake_rejects_items_that_are_not_checkpointed() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y"]);
        let orphan = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("o"))
            .unwrap()
            .value;
        assert_eq!(
            b.record_checkpoint_wake(&Actor::System, T1, &[ids[0], orphan]),
            Err(Rejected::NotOwned(orphan))
        );
        assert_eq!(
            b.item(ids[0]).unwrap().missed_checks,
            0,
            "整批被拒，前面的也不记"
        );
        b.block(
            &Actor::worker("a"),
            T1,
            ids[1],
            vec![crate::block::BlockOn::Item(ids[0])],
            crate::block::Continuation::Nothing,
        )
        .unwrap();
        assert_eq!(
            b.record_checkpoint_wake(&Actor::System, T1, &[ids[1]]),
            Err(Rejected::Invalid("blocked items are not checkpointed"))
        );
    }
}
